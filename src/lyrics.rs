//! Lyrics: local .lrc files (LRCGET), tags, cache and LRCLIB.

use std::fs;
use std::path::{Path, PathBuf};

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag, TagExt};
use serde::{Deserialize, Serialize};

use crate::{data_dir, file_name, http};

/* ───────────── Lyrics ─────────────
Lookup order:
1. `song.lrc` / `song.txt` next to the file (what LRCGET creates) or in a `Lyrics` subfolder
2. Lyrics embedded in the tags
3. Cache in data/lyrics
4. LRCLIB (https://lrclib.net), the same database LRCGET uses */

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    synced: Option<String>,
    plain: Option<String>,
    instrumental: bool,
    found: bool,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default)]
    fresh: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrcLibTrack {
    duration: Option<f64>,
    instrumental: Option<bool>,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

fn is_synced(s: &str) -> bool {
    s.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with('[') && l[1..].chars().next().is_some_and(|c| c.is_ascii_digit())
    })
}

fn lyrics_from_text(text: String, source: &str) -> Lyrics {
    let synced = is_synced(&text);
    Lyrics { synced: synced.then(|| text.clone()), plain: (!synced).then_some(text), found: true, source: source.into(), ..Default::default() }
}

/// Names of the subfolder some people keep an album's lyrics in
const LYRICS_DIRS: [&str; 3] = ["Lyrics", "lyrics", "LYRICS"];

/// The song's lyrics files that exist: `song.lrc` / `song.txt` next to it, or in a `Lyrics`
/// subfolder of its folder (github.com/TonyMontania/Wax/issues/3)
pub fn sidecars(path: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (path.parent(), path.file_stem()) else { return Vec::new() };
    let mut out = Vec::new();
    for ext in ["lrc", "txt"] {
        let next_to = path.with_extension(ext);
        if next_to.is_file() {
            out.push(next_to);
        }
        let mut name = stem.to_os_string();
        name.push(".");
        name.push(ext);
        // (folder names are case-insensitive on Windows and macOS: one of these is enough there)
        if let Some(f) = LYRICS_DIRS.iter().map(|d| dir.join(d).join(&name)).find(|f| f.is_file()) {
            out.push(f);
        }
    }
    out
}

/// Where a new .lrc goes: into the album's Lyrics subfolder if it has one, else next to the song
pub fn lrc_target(path: &Path) -> PathBuf {
    let (Some(dir), Some(stem)) = (path.parent(), path.file_stem()) else { return path.with_extension("lrc") };
    match LYRICS_DIRS.iter().map(|d| dir.join(d)).find(|d| d.is_dir()) {
        Some(sub) => {
            let mut name = stem.to_os_string();
            name.push(".lrc");
            sub.join(name)
        }
        None => path.with_extension("lrc"),
    }
}

/// The text of the song's lyrics file, if it has one
pub fn read_sidecar(path: &Path) -> Option<String> {
    sidecars(path).iter().find_map(|f| fs::read_to_string(f).ok().filter(|s| !s.trim().is_empty()))
}

fn local_lyrics(path: &Path) -> Option<Lyrics> {
    if let Some(s) = read_sidecar(path) {
        return Some(lyrics_from_text(s, "Local .lrc file"));
    }
    let file = Probe::open(path).map(|p| p.options(ParseOptions::new().read_cover_art(false))).and_then(|p| p.read()).ok()?;
    file.tags().iter().find_map(|t| non_empty(t.get_string(&ItemKey::Lyrics).map(str::to_string))).map(|s| lyrics_from_text(s, "File tags"))
}

fn lrclib(title: &str, artist: &str, album: &str, duration: f64) -> Result<Option<LrcLibTrack>, String> {
    let agent = http();

    // Exact match (full signature)
    let exact = agent
        .get("https://lrclib.net/api/get")
        .query("track_name", title)
        .query("artist_name", artist)
        .query("album_name", album)
        .query("duration", (duration.round() as u64).to_string())
        .call();
    match exact {
        Ok(mut resp) => return resp.body_mut().read_json::<LrcLibTrack>().map(Some).map_err(|e| e.to_string()),
        Err(ureq::Error::StatusCode(404)) => {}
        Err(e) => return Err(e.to_string()),
    }

    // Fuzzy search: the closest duration, preferably synced
    let list: Vec<LrcLibTrack> = agent
        .get("https://lrclib.net/api/search")
        .query("track_name", title)
        .query("artist_name", artist)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;
    let diff = |t: &LrcLibTrack| t.duration.map(|d| (d - duration).abs()).unwrap_or(5.0);
    Ok(list.into_iter().filter(|t| duration <= 0.0 || diff(t) <= 8.0).min_by(|a, b| {
        let ka = (a.synced_lyrics.is_none() as u8, diff(a));
        let kb = (b.synced_lyrics.is_none() as u8, diff(b));
        ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
    }))
}

/// The lyrics this computer already has for a song, without going online: its .lrc/.txt, its
/// tags, or Wax's cache of earlier downloads (what a Listen Together host shares with guests)
pub fn known_lyrics(path: &Path, title: &str, artist: &str) -> Option<Lyrics> {
    local_lyrics(path).or_else(|| {
        let cache = data_dir().join("lyrics").join(format!("{}.json", file_name(&format!("{artist} - {title}"))));
        fs::read_to_string(cache).ok().and_then(|s| serde_json::from_str::<Lyrics>(&s).ok()).filter(|l| l.found)
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn get_lyrics(path: String, title: String, artist: String, album: String, duration: f64, online: bool, force: bool) -> Lyrics {
    tauri::async_runtime::spawn_blocking(move || {
        let p = Path::new(&path);
        if let Some(l) = local_lyrics(p) {
            return l;
        }

        let dir = data_dir().join("lyrics");
        let _ = fs::create_dir_all(&dir);
        let cache = dir.join(format!("{}.json", file_name(&format!("{artist} - {title}"))));
        if !force {
            if let Some(l) = fs::read_to_string(&cache).ok().and_then(|s| serde_json::from_str::<Lyrics>(&s).ok()) {
                return l;
            }
        }
        if !online {
            return Lyrics { source: "offline".into(), ..Default::default() };
        }

        let result = match lrclib(&title, &artist, &album, duration) {
            Ok(Some(t)) => {
                let synced = non_empty(t.synced_lyrics);
                let plain = non_empty(t.plain_lyrics);
                let instrumental = t.instrumental.unwrap_or(false);
                Lyrics { found: synced.is_some() || plain.is_some() || instrumental, synced, plain, instrumental, source: "LRCLIB".into(), ..Default::default() }
            }
            Ok(None) => Lyrics { source: "LRCLIB".into(), ..Default::default() },
            Err(e) => return Lyrics { source: "LRCLIB".into(), error: Some(e), ..Default::default() },
        };

        let _ = fs::write(&cache, serde_json::to_string(&result).unwrap_or_default());
        // `fresh` is only sent to the frontend (it stays false in the cache) so
        // automatic saving (.lrc / embed) happens only once.
        Lyrics { fresh: true, ..result }
    })
    .await
    .unwrap_or_default()
}

/// Embeds the lyrics in the file's tags (USLT in MP3, LYRICS in FLAC/OGG, ©lyr in M4A),
/// keeping the modification time so the "Added" date doesn't change.
fn embed_lyrics(path: &Path, text: &str) -> Result<(), String> {
    let mtime = fs::metadata(path).and_then(|m| m.modified()).ok();
    let mut file = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    if file.primary_tag().is_none() {
        let tag_type = file.primary_tag_type();
        file.insert_tag(Tag::new(tag_type));
    }
    let tag = file.primary_tag_mut().ok_or("Couldn't create the tag")?;
    if !tag.insert_text(ItemKey::Lyrics, text.to_string()) {
        return Err("This format doesn't support embedded lyrics".into());
    }
    tag.save_to_path(path, WriteOptions::default()).map_err(|e| e.to_string())?;
    if let Some(t) = mtime {
        if let Ok(f) = fs::File::options().write(true).open(path) {
            let _ = f.set_modified(t);
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn save_lyrics(path: String, text: String, lrc: bool, embed: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let p = Path::new(&path);
        if lrc {
            fs::write(lrc_target(p), &text).map_err(|e| format!("Couldn't write the .lrc: {e}"))?;
        }
        if embed {
            embed_lyrics(p, &text)?;
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An album keeping its lyrics in a "Lyrics" subfolder, and a single with its .lrc next to it
    #[test]
    fn lyrics_subfolder() {
        let dir = std::env::temp_dir().join("wax_lyrics_dirs");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Album/Lyrics")).unwrap();
        fs::create_dir_all(dir.join("Singles")).unwrap();
        let song = dir.join("Album/01 - Intro.flac");
        let single = dir.join("Singles/Song.mp3");
        fs::write(&song, b"").unwrap();
        fs::write(&single, b"").unwrap();
        fs::write(dir.join("Album/Lyrics/01 - Intro.lrc"), "[00:01.00]Hello").unwrap();
        fs::write(dir.join("Singles/Song.lrc"), "[00:01.00]Hi").unwrap();

        assert_eq!(sidecars(&song), vec![dir.join("Album/Lyrics/01 - Intro.lrc")]);
        assert_eq!(read_sidecar(&song).as_deref(), Some("[00:01.00]Hello"));
        assert_eq!(read_sidecar(&single).as_deref(), Some("[00:01.00]Hi"));
        assert!(local_lyrics(&song).is_some_and(|l| l.synced.is_some()));
        // New .lrc files follow the folder's habit
        assert_eq!(lrc_target(&dir.join("Album/02 - Two.flac")), dir.join("Album/Lyrics/02 - Two.lrc"));
        assert_eq!(lrc_target(&dir.join("Singles/Other.mp3")), dir.join("Singles/Other.lrc"));
        let _ = fs::remove_dir_all(&dir);
    }
}
