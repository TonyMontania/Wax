//! Desktop integration: system tray, taskbar thumbnail buttons, mini player,
//! track-change notification and global shortcuts.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    if let Some(m) = app.get_webview_window("mini") {
        let _ = m.close();
    }
}

#[tauri::command]
pub fn hide_main(app: AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
}

#[tauri::command]
pub fn show_main_window(app: AppHandle) {
    show_main(&app);
}

/* ───────────── System tray ───────────── */

pub fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Play / Pause", true, None::<&str>)?;
    let prev = MenuItem::with_id(app, "prev", "Previous", true, None::<&str>)?;
    let next = MenuItem::with_id(app, "next", "Next", true, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "Show Wax", true, None::<&str>)?;
    let mini = MenuItem::with_id(app, "mini", "Mini player", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Exit", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&toggle, &prev, &next, &sep1, &show, &mini, &sep2, &quit])?;
    let mut b = TrayIconBuilder::with_id("main")
        .tooltip("Wax")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id.as_ref() {
            "toggle" | "prev" | "next" | "mini" => {
                let _ = app.emit("media-key", e.id.as_ref());
            }
            "show" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        b = b.icon(icon);
    }
    b.build(app)?;
    Ok(())
}

#[tauri::command]
pub fn tray_tooltip(app: AppHandle, text: String) {
    if let Some(t) = app.tray_by_id("main") {
        let _ = t.set_tooltip(Some(text));
    }
}

/* ───────────── Mini player ───────────── */

/// Async on purpose: on Windows, creating a webview window from a synchronous command
/// deadlocks WebView2 and the window stays blank.
#[tauri::command]
pub async fn toggle_mini(app: AppHandle, show: bool) -> Result<(), String> {
    if !show {
        show_main(&app);
        return Ok(());
    }
    if let Some(m) = app.get_webview_window("mini") {
        let _ = m.show();
        let _ = m.set_focus();
    } else {
        let mut b = WebviewWindowBuilder::new(&app, "mini", WebviewUrl::App("mini.html".into()))
            .title("Wax")
            .inner_size(400.0, 104.0)
            .resizable(false)
            .maximizable(false)
            .decorations(false)
            .always_on_top(true)
            .background_color(tauri::window::Color(24, 24, 24, 255));
        // Bottom right, above the taskbar
        if let Ok(Some(mon)) = app.primary_monitor() {
            let wa = mon.work_area();
            let s = mon.scale_factor();
            let x = (wa.position.x as f64 + wa.size.width as f64) / s - 400.0 - 24.0;
            let y = (wa.position.y as f64 + wa.size.height as f64) / s - 104.0 - 24.0;
            b = b.position(x, y);
        }
        b.build().map_err(|e| e.to_string())?;
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    Ok(())
}

/* ───────────── Track-change notification (OSD) ───────────── */

static OSD_GEN: AtomicU64 = AtomicU64::new(0);
const OSD_W: f64 = 360.0;
const OSD_H: f64 = 84.0;

pub fn create_osd(app: &AppHandle) {
    let mut b = WebviewWindowBuilder::new(app, "osd", WebviewUrl::App("osd.html".into()))
        .title("Wax")
        .inner_size(OSD_W, OSD_H)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .focusable(false)
        .visible(false)
        .background_color(tauri::window::Color(40, 40, 40, 255));
    if let Ok(Some(mon)) = app.primary_monitor() {
        let wa = mon.work_area();
        let s = mon.scale_factor();
        let x = (wa.position.x as f64 + wa.size.width as f64) / s - OSD_W - 16.0;
        let y = (wa.position.y as f64 + wa.size.height as f64) / s - OSD_H - 16.0;
        b = b.position(x, y);
    }
    let _ = b.build();
}

#[tauri::command]
pub fn osd_show(app: AppHandle, payload: serde_json::Value) {
    let Some(w) = app.get_webview_window("osd") else { return };
    let _ = app.emit_to("osd", "osd", payload);
    let _ = w.show();
    let gen = OSD_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(4500));
        if OSD_GEN.load(Ordering::SeqCst) == gen {
            let _ = w.hide();
        }
    });
}

/* ───────────── Global shortcuts ───────────── */

#[derive(Default)]
pub struct Shortcuts(pub Mutex<HashMap<u32, String>>);

pub fn shortcut_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, sc, ev| {
            if ev.state != ShortcutState::Pressed {
                return;
            }
            let action = app.state::<Shortcuts>().0.lock().ok().and_then(|m| m.get(&sc.id()).cloned());
            if let Some(a) = action {
                if a == "show" {
                    show_main(app);
                } else {
                    let _ = app.emit("media-key", a);
                }
            }
        })
        .build()
}

/// Registers the shortcuts (action → combination, e.g. "toggle" → "Ctrl+Alt+Space").
/// Returns the combinations that couldn't be registered.
#[tauri::command]
pub fn set_shortcuts(app: AppHandle, state: State<Shortcuts>, map: HashMap<String, String>) -> Vec<String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    let mut ids = state.0.lock().unwrap();
    ids.clear();
    let mut failed = Vec::new();
    for (action, accel) in map {
        if accel.trim().is_empty() {
            continue;
        }
        match accel.parse::<Shortcut>() {
            Ok(sc) => match gs.register(sc) {
                Ok(()) => {
                    ids.insert(sc.id(), action);
                }
                Err(_) => failed.push(accel),
            },
            Err(_) => failed.push(accel),
        }
    }
    failed
}

/* ───────────── Taskbar thumbnail buttons (Windows) ───────────── */

#[cfg(windows)]
pub mod thumbbar {
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
    use std::sync::OnceLock;

    use tauri::{AppHandle, Emitter, Manager};
    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{DefSubclassProc, ITaskbarList3, SetWindowSubclass, TaskbarList, THBF_ENABLED, THBN_CLICKED, THB_FLAGS, THB_ICON, THB_TOOLTIP, THUMBBUTTON};
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconFromResourceEx, RegisterWindowMessageW, HICON, LR_DEFAULTCOLOR, WM_COMMAND};

    static APP: OnceLock<AppHandle> = OnceLock::new();
    static HWND_RAW: AtomicIsize = AtomicIsize::new(0);
    static CREATED_MSG: AtomicU32 = AtomicU32::new(0);
    static ADDED: AtomicBool = AtomicBool::new(false);
    static PLAYING: AtomicBool = AtomicBool::new(false);

    const PREV: u32 = 1;
    const TOGGLE: u32 = 2;
    const NEXT: u32 = 3;

    fn icon(png: &[u8]) -> HICON {
        unsafe { CreateIconFromResourceEx(png, true, 0x0003_0000, 32, 32, LR_DEFAULTCOLOR).unwrap_or_default() }
    }

    fn button(id: u32, png: &[u8], tip: &str) -> THUMBBUTTON {
        let mut sz = [0u16; 260];
        for (i, c) in tip.encode_utf16().take(259).enumerate() {
            sz[i] = c;
        }
        THUMBBUTTON { dwMask: THB_ICON | THB_TOOLTIP | THB_FLAGS, iId: id, iBitmap: 0, hIcon: icon(png), szTip: sz, dwFlags: THBF_ENABLED }
    }

    fn buttons() -> [THUMBBUTTON; 3] {
        let playing = PLAYING.load(Ordering::Relaxed);
        [
            button(PREV, include_bytes!("../icons/tb_prev.png"), "Previous"),
            if playing { button(TOGGLE, include_bytes!("../icons/tb_pause.png"), "Pause") } else { button(TOGGLE, include_bytes!("../icons/tb_play.png"), "Play") },
            button(NEXT, include_bytes!("../icons/tb_next.png"), "Next"),
        ]
    }

    fn apply(hwnd: HWND) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let Ok(tb) = CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER) else { return };
            if tb.HrInit().is_err() {
                return;
            }
            let b = buttons();
            if ADDED.load(Ordering::Relaxed) {
                let _ = tb.ThumbBarUpdateButtons(hwnd, &b);
            } else if tb.ThumbBarAddButtons(hwnd, &b).is_ok() {
                ADDED.store(true, Ordering::Relaxed);
            }
        }
    }

    unsafe extern "system" fn subclass(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
        if msg != 0 && msg == CREATED_MSG.load(Ordering::Relaxed) {
            ADDED.store(false, Ordering::Relaxed); // Explorer restarted
            apply(hwnd);
        } else if msg == WM_COMMAND && ((wp.0 >> 16) & 0xffff) as u32 == THBN_CLICKED {
            let key = match (wp.0 & 0xffff) as u32 {
                PREV => "prev",
                TOGGLE => "toggle",
                NEXT => "next",
                _ => "",
            };
            if let (false, Some(app)) = (key.is_empty(), APP.get()) {
                let _ = app.emit("media-key", key);
            }
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }

    pub fn install(app: &AppHandle) {
        let Some(win) = app.get_webview_window("main") else { return };
        let Ok(h) = win.hwnd() else { return };
        let hwnd = HWND(h.0 as _);
        let _ = APP.set(app.clone());
        HWND_RAW.store(hwnd.0 as isize, Ordering::Relaxed);
        unsafe {
            CREATED_MSG.store(RegisterWindowMessageW(w!("TaskbarButtonCreated")), Ordering::Relaxed);
            let _ = SetWindowSubclass(hwnd, Some(subclass), 0x5741_5800, 0);
        }
        apply(hwnd);
    }

    pub fn set_playing(app: &AppHandle, playing: bool) {
        if PLAYING.swap(playing, Ordering::Relaxed) == playing && ADDED.load(Ordering::Relaxed) {
            return;
        }
        let raw = HWND_RAW.load(Ordering::Relaxed);
        if raw == 0 {
            return;
        }
        let _ = app.run_on_main_thread(move || apply(HWND(raw as _)));
    }
}

#[tauri::command]
pub fn taskbar_playing(app: AppHandle, playing: bool) {
    #[cfg(windows)]
    thumbbar::set_playing(&app, playing);
    #[cfg(not(windows))]
    let _ = (app, playing);
}

/* ───────────── Full screen ("Now playing") ─────────────
The window has no frame; if it was maximized, Windows keeps the work-area size (without the
taskbar) when it goes full screen, leaving a gap at the bottom. So it's un-maximized first
and then sized to the whole monitor; leaving full screen puts it back as it was. */

static WAS_MAXIMIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[tauri::command]
pub async fn set_fullscreen(app: AppHandle, on: bool) -> Result<(), String> {
    use std::sync::atomic::Ordering::Relaxed;
    let w = app.get_webview_window("main").ok_or("no window")?;
    if on {
        let max = w.is_maximized().unwrap_or(false);
        WAS_MAXIMIZED.store(max, Relaxed);
        if max {
            let _ = w.unmaximize();
        }
        w.set_fullscreen(true).map_err(|e| e.to_string())?;
        if let Ok(Some(m)) = w.current_monitor() {
            let _ = w.set_position(*m.position());
            let _ = w.set_size(*m.size());
        }
    } else {
        w.set_fullscreen(false).map_err(|e| e.to_string())?;
        if WAS_MAXIMIZED.load(Relaxed) {
            let _ = w.maximize();
        }
    }
    Ok(())
}

/* ───────────── Start with Windows and shortcuts ─────────────
Portable-friendly: the Run entry and the shortcuts point at wherever Wax.exe is now.
If the folder is moved, the Run entry is corrected on the next start. */

pub const AUTOSTART_ARG: &str = "--autostart";

/// What starts Wax: the exe, or the .AppImage file on Linux (the exe path is inside its mount).
fn exe() -> String {
    if let Ok(img) = std::env::var("APPIMAGE") {
        return img;
    }
    std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Runs a console tool without flashing a window.
#[cfg(windows)]
fn quiet(cmd: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
    let mut c = std::process::Command::new(cmd);
    c.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c.output()
}

#[cfg(not(windows))]
fn home() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default())
}

/* Windows: HKCU\...\Run · Linux: ~/.config/autostart/wax.desktop · macOS: a LaunchAgent */

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// The command saved to start Wax with the system, if there is one.
#[cfg(windows)]
fn run_entry() -> Option<String> {
    let out = quiet("reg", &["query", RUN_KEY, "/v", "Wax"]).ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.lines().find_map(|l| l.split_once("REG_SZ").map(|(_, v)| v.trim().to_string()))
}
#[cfg(windows)]
fn write_run_entry() -> Result<(), String> {
    let value = format!("\"{}\" {AUTOSTART_ARG}", exe());
    let out = quiet("reg", &["add", RUN_KEY, "/v", "Wax", "/t", "REG_SZ", "/d", &value, "/f"]).map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}
#[cfg(windows)]
fn remove_run_entry() -> Result<(), String> {
    let out = quiet("reg", &["delete", RUN_KEY, "/v", "Wax", "/f"]).map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(target_os = "linux")]
fn autostart_file() -> std::path::PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| home().join(".config"));
    base.join("autostart").join("wax.desktop")
}
#[cfg(target_os = "linux")]
fn desktop_entry(extra_args: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Wax\nComment=Local music player\nExec=\"{}\"{extra_args}\nIcon=wax\nCategories=AudioVideo;Audio;Player;\nTerminal=false\n",
        exe()
    )
}
#[cfg(target_os = "linux")]
fn run_entry() -> Option<String> {
    std::fs::read_to_string(autostart_file()).ok()?.lines().find_map(|l| l.strip_prefix("Exec=").map(str::to_string))
}
#[cfg(target_os = "linux")]
fn write_run_entry() -> Result<(), String> {
    let f = autostart_file();
    std::fs::create_dir_all(f.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(f, desktop_entry(&format!(" {AUTOSTART_ARG}"))).map_err(|e| e.to_string())
}
#[cfg(target_os = "linux")]
fn remove_run_entry() -> Result<(), String> {
    std::fs::remove_file(autostart_file()).map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn agent_file() -> std::path::PathBuf {
    home().join("Library/LaunchAgents/app.wax.player.plist")
}
#[cfg(target_os = "macos")]
fn run_entry() -> Option<String> {
    // <array><string>/path/to/Wax</string>…
    let text = std::fs::read_to_string(agent_file()).ok()?;
    let rest = &text[text.find("<array><string>")? + 15..];
    rest.find("</string>").map(|j| rest[..j].to_string())
}
#[cfg(target_os = "macos")]
fn write_run_entry() -> Result<(), String> {
    let f = agent_file();
    std::fs::create_dir_all(f.parent().unwrap()).map_err(|e| e.to_string())?;
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>app.wax.player</string>\n<key>ProgramArguments</key><array><string>{}</string><string>{AUTOSTART_ARG}</string></array>\n<key>RunAtLoad</key><true/>\n</dict></plist>\n",
        exe().replace('&', "&amp;").replace('<', "&lt;")
    );
    std::fs::write(f, plist).map_err(|e| e.to_string())
}
#[cfg(target_os = "macos")]
fn remove_run_entry() -> Result<(), String> {
    std::fs::remove_file(agent_file()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn autostart_enabled() -> bool {
    run_entry().is_some()
}

#[tauri::command]
pub fn set_autostart(enabled: bool) -> Result<bool, String> {
    if enabled {
        write_run_entry()?;
    } else if run_entry().is_some() {
        remove_run_entry()?;
    }
    Ok(run_entry().is_some())
}

/// Windows: Wax.lnk in the Start menu or on the desktop (from the Start menu it can be pinned to
/// the taskbar, and Wax shows up in the search). Linux: a wax.desktop launcher. macOS: apps live
/// in Applications, so there's nothing to create.
#[tauri::command]
pub async fn create_shortcut(place: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(windows)]
        {
            let base = match place.as_str() {
                "desktop" => "[Environment]::GetFolderPath('Desktop')",
                _ => "[Environment]::GetFolderPath('Programs')",
            };
            let exe = exe();
            let dir = std::path::Path::new(&exe).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let q = |s: &str| s.replace('\'', "''");
            let script = format!(
                "$p = Join-Path ({base}) 'Wax.lnk'; $s = (New-Object -ComObject WScript.Shell).CreateShortcut($p); \
                 $s.TargetPath = '{}'; $s.WorkingDirectory = '{}'; $s.IconLocation = '{},0'; $s.Description = 'Wax music player'; $s.Save(); $p",
                q(&exe),
                q(&dir),
                q(&exe)
            );
            let out = quiet("powershell", &["-NoProfile", "-NonInteractive", "-Command", &script]).map_err(|e| e.to_string())?;
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            }
        }
        #[cfg(target_os = "linux")]
        {
            let dir = match place.as_str() {
                "desktop" => home().join("Desktop"),
                _ => std::env::var("XDG_DATA_HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| home().join(".local/share")).join("applications"),
            };
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let f = dir.join("wax.desktop");
            std::fs::write(&f, desktop_entry("")).map_err(|e| e.to_string())?;
            Ok(f.to_string_lossy().into_owned())
        }
        #[cfg(target_os = "macos")]
        {
            let _ = place;
            Err("On macOS, keep Wax in Applications and drag it to the Dock".to_string())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// At startup: shows the main window (it's created hidden), or keeps it minimized / in the tray
/// when Windows started Wax and the user asked for that. Also fixes the Run entry if the
/// portable folder was moved.
pub fn startup(app: &AppHandle) {
    let auto = std::env::args().any(|a| a == AUTOSTART_ARG);
    if let Some(v) = run_entry() {
        if !v.contains(&exe()) {
            let _ = write_run_entry();
        }
    }
    let prefs: serde_json::Value = std::fs::read_to_string(crate::data_dir().join("state.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let Some(w) = app.get_webview_window("main") else { return };
    if auto && prefs["startMinimized"].as_bool().unwrap_or(false) {
        if prefs["closeToTray"].as_bool().unwrap_or(false) {
            return; // stays hidden, in the tray
        }
        let _ = w.minimize();
        let _ = w.show();
        let _ = w.minimize();
        return;
    }
    let _ = w.show();
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    /// Turns "start with Windows" on and off again (only when asked: it touches the registry).
    #[test]
    fn autostart_roundtrip() {
        if std::env::var("WAX_TEST_AUTOSTART").is_err() {
            return;
        }
        assert!(run_entry().is_none(), "there was already an entry; not touching it");
        assert!(set_autostart(true).unwrap());
        let v = run_entry().unwrap();
        println!("entry: {v}");
        assert!(v.contains(&exe()) && v.ends_with(AUTOSTART_ARG));
        assert!(!set_autostart(false).unwrap());
        assert!(run_entry().is_none());
    }
}
