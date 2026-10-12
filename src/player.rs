//! Audio engine commands, loudness analysis and Windows media controls.

use std::collections::HashMap;
use std::ffi::c_void;
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rayon::prelude::*;
use serde::Serialize;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::audio::{analyze_loudness, output_devices, Engine, EqSettings, Loudness, OutputInfo, TrackSpec};
use crate::data_dir;

pub struct Player(pub Option<Engine>);

fn engine<'a>(p: &'a State<Player>) -> Result<&'a Engine, String> {
    p.0.as_ref().ok_or_else(|| "Audio engine unavailable".to_string())
}

#[tauri::command]
pub fn audio_available(p: State<Player>) -> bool {
    p.0.is_some()
}
#[tauri::command]
pub fn audio_play(p: State<Player>, spec: TrackSpec, start_ms: u64, paused: bool) -> Result<(), String> {
    engine(&p)?.play(spec, start_ms, paused);
    Ok(())
}
#[tauri::command]
pub fn audio_next(p: State<Player>, spec: Option<TrackSpec>) -> Result<(), String> {
    engine(&p)?.set_next(spec);
    Ok(())
}
#[tauri::command]
pub fn audio_pause(p: State<Player>, paused: bool) -> Result<(), String> {
    engine(&p)?.pause(paused);
    Ok(())
}
#[tauri::command]
pub fn audio_seek(p: State<Player>, ms: u64) -> Result<(), String> {
    engine(&p)?.seek(ms);
    Ok(())
}
#[tauri::command]
pub fn audio_volume(p: State<Player>, volume: f32) -> Result<(), String> {
    engine(&p)?.volume(volume);
    Ok(())
}
#[tauri::command]
pub fn audio_eq(p: State<Player>, settings: EqSettings) -> Result<(), String> {
    engine(&p)?.eq(settings);
    Ok(())
}
#[tauri::command]
pub fn audio_crossfeed(p: State<Player>, level: u8) -> Result<(), String> {
    engine(&p)?.crossfeed(level);
    Ok(())
}
#[tauri::command]
pub async fn audio_devices() -> Vec<String> {
    tauri::async_runtime::spawn_blocking(output_devices).await.unwrap_or_default()
}
/// Switches the output device and/or exclusive mode. `device: null` = Windows' default.
#[tauri::command]
pub async fn audio_output(app: tauri::AppHandle, device: Option<String>, exclusive: bool) -> Result<OutputInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let p = app.state::<Player>();
        engine(&p)?.set_output(device, exclusive)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub fn audio_stop(p: State<Player>) -> Result<(), String> {
    engine(&p)?.stop();
    Ok(())
}

/* ───────────── Library loudness analysis ───────────── */

#[derive(Default)]
pub struct LoudnessScan {
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

fn loudness_file() -> std::path::PathBuf {
    data_dir().join("loudness.json")
}

fn read_loudness() -> HashMap<String, Loudness> {
    fs::read_to_string(loudness_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[tauri::command]
pub fn load_loudness() -> HashMap<String, Loudness> {
    read_loudness()
}

#[derive(Serialize, Clone)]
struct ScanProgress {
    done: usize,
    total: usize,
    finished: bool,
}

#[tauri::command]
pub fn start_loudness_scan(app: AppHandle, state: State<LoudnessScan>, paths: Vec<String>) -> bool {
    if state.running.swap(true, Ordering::SeqCst) {
        return false;
    }
    state.cancel.store(false, Ordering::SeqCst);
    let running = state.running.clone();
    let cancel = state.cancel.clone();
    std::thread::spawn(move || {
        let results = Mutex::new(read_loudness());
        let todo: Vec<String> = {
            let r = results.lock().unwrap();
            paths.into_iter().filter(|p| !r.contains_key(p)).collect()
        };
        let total = todo.len();
        let done = AtomicUsize::new(0);
        let save = |r: &HashMap<String, Loudness>| {
            let tmp = loudness_file().with_extension("tmp");
            if fs::write(&tmp, serde_json::to_string(r).unwrap_or_default()).is_ok() {
                let _ = fs::rename(&tmp, loudness_file());
            }
        };
        // Few threads, low priority: music has to keep playing smoothly
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build();
        if let Ok(pool) = pool {
            pool.install(|| {
                todo.par_iter().for_each(|p| {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let res = analyze_loudness(p);
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if let Ok(l) = res {
                        let mut r = results.lock().unwrap();
                        r.insert(p.clone(), l);
                        if n % 25 == 0 {
                            save(&r);
                        }
                    }
                    if n % 5 == 0 || n == total {
                        let _ = app.emit("loudness-progress", ScanProgress { done: n, total, finished: false });
                    }
                });
            });
        }
        save(&results.lock().unwrap());
        running.store(false, Ordering::SeqCst);
        let _ = app.emit(
            "loudness-progress",
            ScanProgress { done: done.load(Ordering::Relaxed), total, finished: true },
        );
    });
    true
}

#[tauri::command]
pub fn stop_loudness_scan(state: State<LoudnessScan>) {
    state.cancel.store(true, Ordering::SeqCst);
}

/* ───────────── Windows media controls (SMTC) ─────────────
   Keyboard media keys and the Windows media panel. */

pub struct SendControls(pub MediaControls);
// SMTC WinRT objects are "agile": they can be used from any thread.
unsafe impl Send for SendControls {}

pub struct Media(pub Mutex<Option<SendControls>>);

pub fn init_media(app: &AppHandle) -> Option<SendControls> {
    let win = app.get_webview_window("main")?;
    #[cfg(windows)]
    let hwnd = Some(win.hwnd().ok()?.0 as *mut c_void);
    #[cfg(not(windows))]
    let hwnd: Option<*mut c_void> = {
        let _ = &win;
        None
    };
    let mut controls = MediaControls::new(PlatformConfig { display_name: "Wax", dbus_name: "wax", hwnd }).ok()?;
    let h = app.clone();
    controls
        .attach(move |e| {
            let key = match e {
                MediaControlEvent::Play => "play",
                MediaControlEvent::Pause | MediaControlEvent::Stop => "pause",
                MediaControlEvent::Toggle => "toggle",
                MediaControlEvent::Next => "next",
                MediaControlEvent::Previous => "prev",
                MediaControlEvent::SetPosition(MediaPosition(d)) => {
                    let _ = h.emit("media-seek", d.as_millis() as u64);
                    return;
                }
                MediaControlEvent::Raise => "show",
                _ => return,
            };
            let _ = h.emit("media-key", key);
        })
        .ok()?;
    Some(SendControls(controls))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn media_update(
    state: State<Media>,
    title: String,
    artist: String,
    album: String,
    cover: Option<String>,
    duration_ms: u64,
    playing: bool,
    pos_ms: u64,
    meta: bool,
) {
    let Ok(mut g) = state.0.lock() else { return };
    let Some(c) = g.as_mut() else { return };
    if meta {
        let url = cover.map(|p| format!("file://{p}"));
        let _ = c.0.set_metadata(MediaMetadata {
            title: Some(&title),
            artist: Some(&artist),
            album: Some(&album),
            cover_url: url.as_deref(),
            duration: (duration_ms > 0).then(|| Duration::from_millis(duration_ms)),
        });
    }
    let progress = Some(MediaPosition(Duration::from_millis(pos_ms)));
    let _ = c.0.set_playback(if playing { MediaPlayback::Playing { progress } } else { MediaPlayback::Paused { progress } });
}
