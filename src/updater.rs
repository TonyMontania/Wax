//! Updates from GitHub releases, installed wherever this copy of Wax is:
//!   portable (Windows) → the new Wax.exe replaces the old one in the same folder
//!   installer (Windows) → the new setup runs in update mode over the same install folder
//!   macOS              → the new Wax.app replaces the old one where it is
//!   AppImage (Linux)   → the new .AppImage replaces the old file
//!   .deb / .rpm        → the package is downloaded and opened with the system installer
//! Downloads are checked against the release's SHA256SUMS.txt when it has one.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};

use crate::{app_dir, http, install_kind, Install};

pub const REPO: &str = "TonyMontania/Wax";

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    current: String,
    latest: String,
    newer: bool,
    notes: String,
    page: String,
    /// The file this copy would download (None: no build for this system in the release)
    asset: Option<String>,
    size: u64,
    install: String,
}

/// "v1.2.10" → [1, 2, 10]
fn version(s: &str) -> Vec<u64> {
    let mut v: Vec<u64> = s.trim_start_matches(['v', 'V']).split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect();
    v.resize(3, 0);
    v
}

fn latest_release() -> Result<Option<Value>, String> {
    let resp = http()
        .get(&format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("Accept", "application/vnd.github+json")
        .config()
        .http_status_as_error(false)
        .build()
        .call()
        .map_err(|e| format!("GitHub: {e}"))?;
    let status = resp.status().as_u16();
    if status == 404 {
        return Ok(None); // no releases yet
    }
    if status >= 400 {
        return Err(format!("GitHub answered {status}"));
    }
    let mut resp = resp;
    resp.body_mut().read_json().map(Some).map_err(|e| e.to_string())
}

/// The release file that fits this system and this kind of install.
fn pick_asset(rel: &Value) -> Option<Value> {
    let assets = rel["assets"].as_array()?;
    let name = |a: &Value| a["name"].as_str().unwrap_or("").to_lowercase();
    let find = |f: &dyn Fn(&str) -> bool| assets.iter().find(|a| f(&name(a))).cloned();
    match install_kind() {
        Install::Portable if cfg!(windows) => find(&|n| n.contains("portable") && n.ends_with(".zip")),
        Install::Portable => None,
        Install::WindowsInstaller => find(&|n| n.ends_with("-setup.exe")),
        Install::MacApp => find(&|n| n.ends_with(".app.tar.gz")),
        Install::AppImage => find(&|n| n.ends_with(".appimage")),
        Install::LinuxPackage => {
            let ext = if Path::new("/etc/debian_version").exists() { ".deb" } else { ".rpm" };
            find(&|n| n.ends_with(ext))
        }
    }
}

#[tauri::command]
pub async fn check_update() -> Result<UpdateInfo, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let current = env!("CARGO_PKG_VERSION").to_string();
        let Some(rel) = latest_release()? else {
            return Ok(UpdateInfo { latest: current.clone(), current, newer: false, notes: String::new(), page: format!("https://github.com/{REPO}/releases"), asset: None, size: 0, install: install_kind().name().into() });
        };
        let latest = rel["tag_name"].as_str().unwrap_or("").trim_start_matches(['v', 'V']).to_string();
        let asset = pick_asset(&rel);
        Ok(UpdateInfo {
            newer: version(&latest) > version(&current),
            current,
            latest,
            notes: rel["body"].as_str().unwrap_or("").chars().take(4000).collect(),
            page: rel["html_url"].as_str().unwrap_or("").into(),
            asset: asset.as_ref().and_then(|a| a["name"].as_str().map(str::to_string)),
            size: asset.as_ref().and_then(|a| a["size"].as_u64()).unwrap_or(0),
            install: install_kind().name().into(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone)]
struct Progress {
    done: u64,
    total: u64,
}

fn download(app: &AppHandle, url: &str, total: u64, to: &Path) -> Result<String, String> {
    let mut resp = http().get(url).header("Accept", "application/octet-stream").call().map_err(|e| format!("Download: {e}"))?;
    let mut reader = resp.body_mut().with_config().limit(1 << 30).reader();
    let mut file = fs::File::create(to).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let (mut done, mut last) = (0u64, 0u64);
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("Download: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        hasher.update(&buf[..n]);
        done += n as u64;
        if done - last > 256 * 1024 {
            last = done;
            let _ = app.emit("update-progress", Progress { done, total });
        }
    }
    let _ = app.emit("update-progress", Progress { done, total });
    Ok(format!("{:x}", hasher.finalize()))
}

/// Checks the download against SHA256SUMS.txt ("<hash>  <file name>" lines), if the release has it.
fn verify(rel: &Value, name: &str, hash: &str) -> Result<(), String> {
    let Some(sums) = rel["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case("SHA256SUMS.txt")))) else {
        return Ok(());
    };
    let text = http()
        .get(sums["browser_download_url"].as_str().unwrap_or(""))
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    match text.lines().find_map(|l| l.split_once(char::is_whitespace).filter(|(_, n)| n.trim().trim_start_matches('*') == name).map(|(h, _)| h.trim().to_lowercase())) {
        Some(h) if h == hash => Ok(()),
        Some(_) => Err("The download is damaged (checksum doesn't match). Try again.".into()),
        None => Ok(()),
    }
}

/// Runs a script after Wax closes (to replace files that are in use), without a console window.
fn run_detached(script: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let path = std::env::temp_dir().join("wax-update.cmd");
        fs::write(&path, script).map_err(|e| e.to_string())?;
        std::process::Command::new("cmd")
            .args(["/C", &path.to_string_lossy()])
            .creation_flags(0x0800_0000 | 0x0000_0008) // CREATE_NO_WINDOW | DETACHED_PROCESS
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    #[cfg(unix)]
    {
        std::process::Command::new("sh").args(["-c", script]).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Downloads and installs the update. Returns what the app has to do next:
/// "restart" (the files are in place), "exit" (a script finishes after Wax closes and opens it
/// again) or "opened" (the system installer took over).
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let rel = latest_release()?.ok_or("There are no releases yet")?;
        let asset = pick_asset(&rel).ok_or("This release has no build for your system")?;
        let name = asset["name"].as_str().unwrap_or("update").to_string();
        let url = asset["browser_download_url"].as_str().ok_or("Bad release")?.to_string();
        let total = asset["size"].as_u64().unwrap_or(0);
        let tmp = std::env::temp_dir().join("wax-update");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        let file = tmp.join(&name);
        let hash = download(&app, &url, total, &file)?;
        verify(&rel, &name, &hash)?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = app_dir();
        match install_kind() {
            Install::Portable => {
                // The zip holds Wax/Wax.exe: take the exe and swap it (a running exe can be renamed)
                let zf = fs::File::open(&file).map_err(|e| e.to_string())?;
                let mut zip = zip::ZipArchive::new(zf).map_err(|e| e.to_string())?;
                let idx = (0..zip.len())
                    .find(|&i| zip.by_index(i).map(|f| f.name().to_lowercase().ends_with("wax.exe")).unwrap_or(false))
                    .ok_or("The zip has no Wax.exe")?;
                let new = exe.with_extension("exe.new");
                {
                    let mut src = zip.by_index(idx).map_err(|e| e.to_string())?;
                    let mut out = fs::File::create(&new).map_err(|e| format!("Can't write in {}: {e}", dir.display()))?;
                    std::io::copy(&mut src, &mut out).map_err(|e| e.to_string())?;
                }
                let old = exe.with_extension("exe.old");
                let _ = fs::remove_file(&old);
                fs::rename(&exe, &old).map_err(|e| e.to_string())?;
                if let Err(e) = fs::rename(&new, &exe) {
                    let _ = fs::rename(&old, &exe);
                    return Err(e.to_string());
                }
                Ok("restart".into())
            }
            Install::WindowsInstaller => {
                // Once Wax has closed: the setup in update mode (/UPDATE keeps the folder of the
                // current install, /P shows only a progress bar, /R opens Wax again when done)
                let script = format!("@echo off
ping -n 3 127.0.0.1 >nul
\"{}\" /P /UPDATE /R
", file.display());
                run_detached(&script)?;
                Ok("exit".into())
            }
            Install::MacApp => {
                // …/Wax.app/Contents/MacOS/wax → replace Wax.app where it is
                let bundle = exe.ancestors().nth(3).ok_or("Can't find Wax.app")?.to_path_buf();
                let out = std::process::Command::new("tar").arg("-xzf").arg(&file).arg("-C").arg(&tmp).output().map_err(|e| e.to_string())?;
                if !out.status.success() {
                    return Err(String::from_utf8_lossy(&out.stderr).into());
                }
                let new = fs::read_dir(&tmp).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "app")).ok_or("The download has no Wax.app")?;
                let q = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"));
                let old = PathBuf::from(format!("{}.old", bundle.to_string_lossy()));
                let script = format!(
                    "sleep 2; rm -rf {old}; mv {b} {old} && mv {n} {b} && rm -rf {old}; xattr -dr com.apple.quarantine {b} 2>/dev/null; open {b}",
                    old = q(&old),
                    b = q(&bundle),
                    n = q(&new)
                );
                run_detached(&script)?;
                Ok("exit".into())
            }
            Install::AppImage => {
                let target = PathBuf::from(std::env::var_os("APPIMAGE").ok_or("Not an AppImage")?);
                let new = PathBuf::from(format!("{}.new", target.to_string_lossy()));
                fs::copy(&file, &new).map_err(|e| format!("Can't write in {}: {e}", dir.display()))?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&new, fs::Permissions::from_mode(0o755));
                }
                fs::rename(&new, &target).map_err(|e| e.to_string())?;
                Ok("restart".into())
            }
            Install::LinuxPackage => {
                // Installing a package needs the system's permission: hand it to the software installer
                crate::open_url(&file.to_string_lossy());
                Ok("opened".into())
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Restarts Wax after an update (an AppImage starts the new file).
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    if let Some(img) = std::env::var_os("APPIMAGE") {
        let _ = std::process::Command::new(img).spawn();
        app.exit(0);
    } else {
        app.restart();
    }
}

/// Leftovers of a previous update.
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(exe.with_extension("exe.old"));
    }
    let _ = fs::remove_dir_all(std::env::temp_dir().join("wax-update"));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checks_github() {
        if std::env::var("WAX_NET").is_err() {
            return;
        }
        let u = tauri::async_runtime::block_on(check_update()).unwrap();
        println!("update check: current {} latest {} newer {} asset {:?} install {}", u.current, u.latest, u.newer, u.asset, u.install);
        assert!(!u.newer || u.asset.is_some() || u.latest != u.current);
    }

    #[test]
    fn versions() {
        assert!(version("v1.2.10") > version("1.2.9"));
        assert!(version("2.0.0") > version("v1.99.99"));
        assert!(version("1.0.0") == version("v1.0"));
    }
}
