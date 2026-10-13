// Wax — local music player with the classic Spotify interface.
//
// Rust backend:
//   audio    — playback engine (gapless, crossfade, equalizer, normalization)
//   player   — engine commands, loudness analysis, Windows media controls
//   library  — incremental scanning, tags, organizer, M3U, history, watched folders
//   covers   — album covers and artist photos (local and from Deezer)
//   lyrics   — lyrics (LRCGET .lrc files, tags, LRCLIB)
//   online   — MusicBrainz, Last.fm, ListenBrainz
//   cue      — CUE sheets (one file per album, split into tracks)
//   updater  — updates from GitHub releases, installed where Wax is
//   desktop  — tray, taskbar, mini player, notifications, global shortcuts
//   discover — new releases, credits, lyrics search, AutoEQ, Spotify CSV
//   discord  — Discord status ("Listening to …")
// Everything is saved to the `data` folder next to the executable (portable).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod covers;
mod cue;
mod desktop;
mod discord;
mod discover;
mod library;
mod lyrics;
mod online;
mod player;
mod updater;

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_dialog::DialogExt;

pub const AUDIO_EXT: &[&str] = &["mp3", "flac", "ogg", "oga", "opus", "wav", "aif", "aiff", "m4a", "aac", "mp4", "webm"];
pub const IMAGE_EXT: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif"];
pub const USER_AGENT: &str = concat!("Wax/", env!("CARGO_PKG_VERSION"), " ( local music player )");

/// How this copy of Wax was installed (decides where data goes and how it updates itself).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Install {
    /// A folder you can put anywhere (the zip): data goes in `<folder>/data`
    Portable,
    /// Windows setup (the folder has the uninstaller)
    WindowsInstaller,
    /// macOS app bundle
    MacApp,
    /// Linux AppImage
    AppImage,
    /// Linux .deb / .rpm (updated by the package manager)
    LinuxPackage,
}

impl Install {
    pub fn name(self) -> &'static str {
        match self {
            Install::Portable => "portable",
            Install::WindowsInstaller => "installed",
            Install::MacApp => "mac",
            Install::AppImage => "appimage",
            Install::LinuxPackage => "package",
        }
    }
}

fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(|| PathBuf::from("."))
}

pub fn install_kind() -> Install {
    static KIND: OnceLock<Install> = OnceLock::new();
    *KIND.get_or_init(|| {
        let dir = exe_dir();
        // A "portable" file or an existing data folder next to the program always means portable
        if dir.join("portable").exists() || dir.join("data").join("state.json").exists() {
            return Install::Portable;
        }
        if cfg!(windows) {
            if dir.join("uninstall.exe").exists() { Install::WindowsInstaller } else { Install::Portable }
        } else if cfg!(target_os = "macos") {
            Install::MacApp
        } else if std::env::var_os("APPIMAGE").is_some() {
            Install::AppImage
        } else {
            Install::LinuxPackage
        }
    })
}

/// Where the program is, as the user sees it: the portable folder, the install folder,
/// the folder that holds Wax.app, or the folder of the .AppImage file.
pub fn app_dir() -> PathBuf {
    match install_kind() {
        Install::AppImage => std::env::var_os("APPIMAGE").map(PathBuf::from).and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_else(exe_dir),
        // …/Wax.app/Contents/MacOS/wax → the folder with Wax.app
        Install::MacApp => exe_dir().ancestors().nth(3).map(Path::to_path_buf).unwrap_or_else(exe_dir),
        _ => exe_dir(),
    }
}

/// Data folder: next to the program when portable; otherwise the system's place for app data
/// (%APPDATA%\Wax, ~/Library/Application Support/Wax, ~/.local/share/wax).
pub fn data_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    let dir = DIR.get_or_init(|| {
        if install_kind() == Install::Portable {
            return exe_dir().join("data");
        }
        let home = || PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        if cfg!(windows) {
            PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("Wax")
        } else if cfg!(target_os = "macos") {
            home().join("Library/Application Support/Wax")
        } else {
            std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share")).join("wax")
        }
    });
    let _ = fs::create_dir_all(dir);
    dir.clone()
}

/// Opens a folder in Explorer / Finder / the file manager.
pub fn open_folder(dir: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

pub fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| exts.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn clean(s: Option<impl AsRef<str>>) -> Option<String> {
    s.map(|v| v.as_ref().trim().to_string()).filter(|v| !v.is_empty())
}

/// A readable, file-system-safe name for cache files: "Architects - Machine - Single".
pub fn file_name(s: &str) -> String {
    let mut t: String = s.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut t: String = t.chars().take(120).collect();
    while t.ends_with(['.', ' ']) {
        t.pop();
    }
    let upper = t.to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "LPT1", "LPT2", "LPT3"];
    if t.is_empty() || reserved.contains(&upper.as_str()) {
        t.push('_');
    }
    t
}

pub fn path_hash(path: &Path) -> String {
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Shared HTTP client (with its own User-Agent and a timeout).
pub fn http() -> ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(15)))
                .user_agent(USER_AGENT)
                .build()
                .into()
        })
        .clone()
}

pub fn open_url(url: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

#[tauri::command]
fn load_state() -> String {
    fs::read_to_string(data_dir().join("state.json")).unwrap_or_default()
}

#[tauri::command]
fn save_state(json: String) -> Result<(), String> {
    let tmp = data_dir().join("state.json.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, data_dir().join("state.json")).map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal(path: String) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").args(["-R", &path]).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(parent) = Path::new(&path).parent() {
            let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
        }
    }
}

#[tauri::command]
fn open_data_dir() {
    open_folder(&data_dir());
}

/// Opens the folder where Wax is (portable folder, install folder, Wax.app's folder…).
#[tauri::command]
fn open_app_dir() {
    open_folder(&app_dir());
}

#[tauri::command]
fn open_link(url: String) {
    if url.starts_with("https://") {
        open_url(&url);
    }
}

#[tauri::command]
fn app_info() -> serde_json::Value {
    serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "dataDir": data_dir().to_string_lossy(),
        "appDir": app_dir().to_string_lossy(),
        "install": install_kind().name(),
        "os": std::env::consts::OS,
    })
}

#[tauri::command]
async fn pick_folders(app: AppHandle) -> Vec<String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Add music folder")
            .blocking_pick_folders()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|p| p.into_path().ok())
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    })
    .await
    .unwrap_or_default()
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| desktop::show_main(app)))
        .plugin(tauri_plugin_dialog::init())
        .plugin(desktop::shortcut_plugin())
        .manage(library::FolderWatch::default())
        .manage(desktop::Shortcuts::default())
        .manage(player::LoudnessScan::default())
        .manage(discord::Discord::default())
        .setup(|app| {
            let handle = app.handle().clone();
            let emitter = handle.clone();
            let engine = audio::Engine::start(move |e| {
                let _ = emitter.emit("audio", e);
            });
            if let Err(e) = &engine {
                eprintln!("Wax: audio engine unavailable: {e}");
            }
            app.manage(player::Player(engine.ok()));
            app.manage(player::Media(Mutex::new(player::init_media(&handle))));
            let _ = desktop::setup_tray(&handle);
            desktop::create_osd(&handle);
            #[cfg(windows)]
            desktop::thumbbar::install(&handle);
            desktop::startup(&handle);
            updater::cleanup();
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window closes everything (mini player, notification, tray)
            if window.label() == "main" && matches!(event, WindowEvent::Destroyed) {
                window.app_handle().exit(0);
            }
            // If the mini player is closed (Alt+F4), bring the main window back
            if window.label() == "mini" && matches!(event, WindowEvent::Destroyed) {
                if let Some(w) = window.app_handle().get_webview_window("main") {
                    let _ = w.show();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            load_state,
            save_state,
            reveal,
            open_data_dir,
            open_app_dir,
            open_link,
            app_info,
            pick_folders,
            library::load_library,
            library::scan_folders,
            library::write_tags,
            library::organize_files,
            library::export_m3u,
            library::import_m3u,
            library::append_history,
            library::load_history,
            library::append_history_bulk,
            library::write_replaygain,
            library::embed_covers,
            library::lyrics_status,
            library::trash_files,
            library::copy_files,
            library::pick_dir,
            library::watch_folders,
            covers::get_cover,
            covers::get_artist_image,
            covers::clear_image_misses,
            covers::clean_image_cache,
            covers::covers_status,
            covers::image_candidates,
            covers::set_image,
            covers::reset_image,
            covers::migrate_cache_names,
            covers::place_image,
            covers::pick_image,
            lyrics::get_lyrics,
            lyrics::save_lyrics,
            player::audio_available,
            player::audio_play,
            player::audio_next,
            player::audio_pause,
            player::audio_seek,
            player::audio_volume,
            player::audio_eq,
            player::audio_stop,
            player::audio_crossfeed,
            player::audio_devices,
            player::audio_output,
            player::load_loudness,
            player::start_loudness_scan,
            player::stop_loudness_scan,
            player::media_update,
            online::mb_search,
            online::mb_release,
            online::scrobble,
            online::lastfm_auth_start,
            online::lastfm_auth_finish,
            online::listenbrainz_check,
            online::lastfm_get,
            online::lastfm_love,
            discord::discord_update,
            discover::artist_releases,
            discover::song_credits,
            discover::search_lyrics,
            discover::autoeq_index,
            discover::autoeq_profile,
            discover::pick_csv_files,
            updater::check_update,
            updater::install_update,
            updater::restart_app,
            desktop::hide_main,
            desktop::show_main_window,
            desktop::tray_tooltip,
            desktop::toggle_mini,
            desktop::osd_show,
            desktop::set_shortcuts,
            desktop::taskbar_playing,
            desktop::autostart_enabled,
            desktop::set_autostart,
            desktop::create_shortcut,
            desktop::set_fullscreen,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Wax");
}
