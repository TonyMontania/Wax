//! Looking things up: new releases of your artists (Deezer), song credits (MusicBrainz),
//! searching the library by a line of the lyrics, AutoEQ headphone profiles and reading
//! playlists exported from Spotify (Exportify CSV).

use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
use lofty::tag::ItemKey;
use md5::{Digest, Md5};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::{data_dir, http, path_hash};

/// Small JSON/text cache in data/cache/<kind>/.
fn cached<T: Serialize + for<'de> Deserialize<'de>>(kind: &str, key: &str, days: u64, fetch: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let dir = data_dir().join("cache").join(kind);
    let file = dir.join(format!("{:x}.json", Md5::digest(key.to_lowercase().as_bytes())));
    let fresh = fs::metadata(&file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(days * 86400));
    if fresh {
        if let Some(v) = fs::read_to_string(&file).ok().and_then(|s| serde_json::from_str(&s).ok()) {
            return Ok(v);
        }
    }
    let v = fetch()?;
    let _ = fs::create_dir_all(&dir);
    let _ = fs::write(&file, serde_json::to_string(&v).unwrap_or_default());
    Ok(v)
}

fn simple(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/* ───────────── New releases (Deezer) ───────────── */

#[derive(Serialize, Deserialize, Clone)]
pub struct Release {
    title: String,
    date: String,
    kind: String,
    cover: String,
    link: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ArtistReleases {
    name: String,
    releases: Vec<Release>,
}

/// The releases Deezer knows for an artist (newest first). None if Deezer doesn't have
/// an artist with exactly that name. Cached for 3 days.
#[tauri::command]
pub async fn artist_releases(name: String) -> Result<Option<ArtistReleases>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        cached("releases", &name, 3, || {
            let v: Value = http()
                .get("https://api.deezer.com/search/artist")
                .query("q", &name)
                .query("limit", "10")
                .call()
                .map_err(|e| format!("Deezer: {e}"))?
                .body_mut()
                .read_json()
                .map_err(|e| e.to_string())?;
            let want = simple(&name);
            let Some(id) = v["data"].as_array().into_iter().flatten().find(|a| simple(a["name"].as_str().unwrap_or("")) == want).and_then(|a| a["id"].as_u64()) else {
                return Ok(None);
            };
            let v: Value = http()
                .get(&format!("https://api.deezer.com/artist/{id}/albums"))
                .query("limit", "100")
                .call()
                .map_err(|e| format!("Deezer: {e}"))?
                .body_mut()
                .read_json()
                .map_err(|e| e.to_string())?;
            let mut releases: Vec<Release> = v["data"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|a| Release {
                    title: a["title"].as_str().unwrap_or("").into(),
                    date: a["release_date"].as_str().unwrap_or("").into(),
                    kind: a["record_type"].as_str().unwrap_or("album").into(),
                    cover: a["cover_medium"].as_str().unwrap_or("").into(),
                    link: a["link"].as_str().unwrap_or("").into(),
                })
                .collect();
            releases.sort_by(|a, b| b.date.cmp(&a.date));
            Ok(Some(ArtistReleases { name: name.clone(), releases }))
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/* ───────────── Credits (MusicBrainz) ───────────── */

#[derive(Serialize, Deserialize, Clone)]
pub struct Credit {
    role: String,
    names: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Credits {
    title: String,
    artist: String,
    url: String,
    credits: Vec<Credit>,
}

fn add(list: &mut Vec<Credit>, role: &str, name: &str) {
    if name.is_empty() {
        return;
    }
    match list.iter_mut().find(|c| c.role == role) {
        Some(c) if !c.names.iter().any(|n| n == name) => c.names.push(name.to_string()),
        Some(_) => {}
        None => list.push(Credit { role: role.to_string(), names: vec![name.to_string()] }),
    }
}

/// "instrument" + ["guitar"] → "Guitar"; "producer" → "Producer"
fn role_name(rel: &Value) -> String {
    let ty = rel["type"].as_str().unwrap_or("");
    let attrs: Vec<&str> = rel["attributes"].as_array().into_iter().flatten().filter_map(|a| a.as_str()).filter(|a| !["additional", "guest", "solo", "co"].contains(a)).collect();
    let base = match ty {
        "instrument" | "performer" if !attrs.is_empty() => attrs.join(", "),
        "vocal" if !attrs.is_empty() => attrs.join(", "),
        "vocal" => "vocals".into(),
        "mix" => "mixing".into(),
        "recording" => "recording engineer".into(),
        other => other.replace('-', " "),
    };
    let mut c = base.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Who wrote, produced and played on a song, from MusicBrainz. Cached for 30 days.
#[tauri::command]
pub async fn song_credits(artist: String, title: String, duration_ms: u64) -> Result<Option<Credits>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        cached("credits", &format!("{artist}|{title}"), 30, || {
            let q = format!("recording:\"{}\" AND artist:\"{}\"", title.replace('"', ""), artist.replace('"', ""));
            let v = crate::online::mb_get("recording", &[("query", &q), ("limit", "15")])?;
            let recs = v["recordings"].as_array().cloned().unwrap_or_default();
            // Best match: high score and the closest length
            let best = recs
                .iter()
                .filter(|r| r["score"].as_u64().unwrap_or(0) >= 80)
                .min_by_key(|r| {
                    let len = r["length"].as_u64().unwrap_or(0);
                    if duration_ms == 0 || len == 0 { 5000 } else { len.abs_diff(duration_ms) }
                });
            let Some(id) = best.and_then(|r| r["id"].as_str()) else { return Ok(None) };
            let r = crate::online::mb_get(&format!("recording/{id}"), &[("inc", "artist-credits+artist-rels+work-rels+work-level-rels")])?;
            let mut credits: Vec<Credit> = Vec::new();
            for w in r["relations"].as_array().into_iter().flatten().filter(|x| x["target-type"] == "work") {
                for wr in w["work"]["relations"].as_array().into_iter().flatten().filter(|x| x["target-type"] == "artist") {
                    let role = match wr["type"].as_str().unwrap_or("") {
                        "composer" => "Composer",
                        "lyricist" => "Lyricist",
                        "writer" => "Writer",
                        "arranger" => "Arranger",
                        _ => continue,
                    };
                    add(&mut credits, role, wr["artist"]["name"].as_str().unwrap_or(""));
                }
            }
            for rel in r["relations"].as_array().into_iter().flatten().filter(|x| x["target-type"] == "artist") {
                add(&mut credits, &role_name(rel), rel["artist"]["name"].as_str().unwrap_or(""));
            }
            let artist_credit: String = r["artist-credit"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| format!("{}{}", c["name"].as_str().unwrap_or(""), c["joinphrase"].as_str().unwrap_or("")))
                .collect();
            Ok(Some(Credits {
                title: r["title"].as_str().unwrap_or(&title).into(),
                artist: artist_credit,
                url: format!("https://musicbrainz.org/recording/{id}"),
                credits,
            }))
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/* ───────────── Searching by lyrics ───────────── */

static LYRICS_INDEX: Mutex<Option<(usize, Vec<(String, Vec<String>)>)>> = Mutex::new(None);

/// Lowercase, without accents or punctuation, so "Corazón," matches "corazon".
fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        let c = match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'í' | 'ì' | 'î' | 'ï' => 'i',
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' => 'o',
            'ú' | 'ù' | 'û' | 'ü' => 'u',
            'ñ' => 'n',
            'ç' => 'c',
            c if c.is_alphanumeric() => c,
            _ => ' ',
        };
        if c != ' ' || !out.ends_with(' ') {
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// Lines of a song's lyrics: the .lrc/.txt next to it, the tags, or Wax's LRCLIB cache.
fn lyric_lines(path: &str) -> Vec<String> {
    let p = Path::new(path);
    let mut text = ["lrc", "txt"].iter().find_map(|e| fs::read_to_string(p.with_extension(e)).ok());
    if text.is_none() {
        text = Probe::open(p)
            .map(|x| x.options(ParseOptions::new().read_cover_art(false)))
            .and_then(|x| x.read())
            .ok()
            .and_then(|f| f.primary_tag().or_else(|| f.first_tag()).and_then(|t| t.get_string(&ItemKey::Lyrics).map(str::to_string)));
    }
    if text.is_none() {
        let cache = data_dir().join("lyrics").join(format!("{}.json", path_hash(p)));
        if let Some(v) = fs::read_to_string(cache).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
            text = v["synced"].as_str().or(v["plain"].as_str()).map(str::to_string);
        }
    }
    text.unwrap_or_default()
        .lines()
        .map(|l| {
            // drop [mm:ss.xx] time tags and [ar:...] headers
            let mut s = l.trim();
            while s.starts_with('[') {
                match s.find(']') {
                    Some(i) => s = s[i + 1..].trim_start(),
                    None => break,
                }
            }
            s.to_string()
        })
        .filter(|l| !l.is_empty())
        .collect()
}

#[derive(Serialize)]
pub struct LyricHit {
    path: String,
    line: String,
}

/// Songs whose lyrics contain all the words of `query` (in one line, or else anywhere).
/// The index is built on the first search (a few seconds) and kept in memory.
#[tauri::command]
pub async fn search_lyrics(query: String, paths: Vec<String>, rebuild: bool) -> Vec<LyricHit> {
    tauri::async_runtime::spawn_blocking(move || {
        let words: Vec<String> = fold(&query).split(' ').filter(|w| w.len() > 1).map(str::to_string).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let mut idx = LYRICS_INDEX.lock().unwrap_or_else(|e| e.into_inner());
        if rebuild || idx.as_ref().map_or(true, |(n, _)| *n != paths.len()) {
            let list: Vec<(String, Vec<String>)> = paths.par_iter().map(|p| (p.clone(), lyric_lines(p))).filter(|(_, l)| !l.is_empty()).collect();
            *idx = Some((paths.len(), list));
        }
        let mut hits = Vec::new();
        for (path, lines) in &idx.as_ref().unwrap().1 {
            let folded: Vec<String> = lines.iter().map(|l| fold(l)).collect();
            let has_all = |t: &str| words.iter().all(|w| t.contains(w.as_str()));
            let line = folded.iter().position(|l| has_all(l)).or_else(|| {
                let all = folded.join(" ");
                if has_all(&all) { folded.iter().position(|l| l.contains(words[0].as_str())) } else { None }
            });
            if let Some(i) = line {
                hits.push(LyricHit { path: path.clone(), line: lines[i].clone() });
                if hits.len() >= 60 {
                    break;
                }
            }
        }
        hits
    })
    .await
    .unwrap_or_default()
}

/* ───────────── AutoEQ (headphone correction) ───────────── */

const AUTOEQ: &str = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/";

#[derive(Serialize, Deserialize, Clone)]
pub struct EqModel {
    name: String,
    source: String,
    path: String,
}

/// Every headphone in AutoEQ (about 8,000 measurements). Cached for 30 days.
#[tauri::command]
pub async fn autoeq_index() -> Result<Vec<EqModel>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        cached("autoeq", "index", 30, || {
            let text = http()
                .get(&format!("{AUTOEQ}INDEX.md"))
                .call()
                .map_err(|e| format!("AutoEQ: {e}"))?
                .body_mut()
                .with_config()
                .limit(10 * 1024 * 1024)
                .read_to_string()
                .map_err(|e| e.to_string())?;
            // - [Sennheiser HD 600](./oratory1990/over-ear/Sennheiser%20HD%20600) by oratory1990
            Ok(text
                .lines()
                .filter_map(|l| {
                    let l = l.strip_prefix("- [")?;
                    let (name, rest) = l.split_once("](./")?;
                    let (path, rest) = rest.split_once(')')?;
                    let source = rest.trim().strip_prefix("by ").unwrap_or("").to_string();
                    Some(EqModel { name: name.to_string(), source, path: path.to_string() })
                })
                .collect::<Vec<_>>())
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EqProfile {
    preamp: f32,
    filters: Vec<Value>,
}

/// The parametric EQ of one model: "Preamp: -6.3 dB" + "Filter 1: ON LSC Fc 105 Hz Gain 6.5 dB Q 0.70".
#[tauri::command]
pub async fn autoeq_profile(path: String) -> Result<EqProfile, String> {
    tauri::async_runtime::spawn_blocking(move || {
        cached("autoeq", &path, 90, || {
            let name = path.rsplit('/').next().unwrap_or("");
            let text = http()
                .get(&format!("{AUTOEQ}{path}/{name}%20ParametricEQ.txt"))
                .call()
                .map_err(|e| format!("AutoEQ: {e}"))?
                .body_mut()
                .read_to_string()
                .map_err(|e| e.to_string())?;
            let mut preamp = 0.0;
            let mut filters = Vec::new();
            for l in text.lines() {
                let t: Vec<&str> = l.split_whitespace().collect();
                if l.starts_with("Preamp:") {
                    preamp = t.get(1).and_then(|v| v.parse().ok()).unwrap_or(0.0);
                } else if l.starts_with("Filter") && t.contains(&"ON") {
                    let after = |k: &str| t.iter().position(|x| *x == k).and_then(|i| t.get(i + 1)).and_then(|v| v.parse::<f32>().ok());
                    let kind = t.iter().find(|x| ["PK", "LSC", "HSC", "LS", "HS"].contains(x)).copied().unwrap_or("PK");
                    if let (Some(fc), Some(gain), Some(q)) = (after("Fc"), after("Gain"), after("Q")) {
                        filters.push(serde_json::json!({ "kind": kind, "fc": fc, "gain": gain, "q": q }));
                    }
                }
            }
            if filters.is_empty() {
                return Err("That profile has no filters".into());
            }
            Ok(EqProfile { preamp, filters })
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/* ───────────── Spotify playlists (Exportify CSV) ───────────── */

#[derive(Serialize)]
pub struct CsvFile {
    name: String,
    text: String,
}

/// Lets the user pick one or more CSV files and returns their contents.
#[tauri::command]
pub async fn pick_csv_files(app: AppHandle) -> Vec<CsvFile> {
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Spotify playlists exported with Exportify (.csv)")
            .add_filter("CSV", &["csv"])
            .blocking_pick_files()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|f| f.into_path().ok())
            .filter_map(|p| {
                let bytes = fs::read(&p).ok()?;
                let text = String::from_utf8_lossy(bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes)).into_owned();
                Some(CsvFile { name: p.file_stem()?.to_string_lossy().replace('_', " "), text })
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn online_lookups() {
        if std::env::var("WAX_NET").is_err() {
            return;
        }
        let r = tauri::async_runtime::block_on(artist_releases("Radiohead".into())).unwrap().unwrap();
        println!("Radiohead releases: {} · newest: {} ({})", r.releases.len(), r.releases[0].title, r.releases[0].date);
        assert!(r.releases.len() > 10);
        let idx = tauri::async_runtime::block_on(autoeq_index()).unwrap();
        let hd600 = idx.iter().find(|m| m.name == "Sennheiser HD 600" && m.source == "oratory1990").unwrap();
        let prof = tauri::async_runtime::block_on(autoeq_profile(hd600.path.clone())).unwrap();
        println!("AutoEQ: {} models · HD 600 preamp {} dB, {} filters", idx.len(), prof.preamp, prof.filters.len());
        assert!(idx.len() > 1000 && prof.filters.len() >= 5);
        let c = tauri::async_runtime::block_on(song_credits("Radiohead".into(), "Paranoid Android".into(), 383_000)).unwrap().unwrap();
        println!("credits for {} — {}: {}", c.title, c.artist, c.credits.iter().map(|x| format!("{}: {}", x.role, x.names.join(", "))).collect::<Vec<_>>().join(" | "));
        assert!(!c.credits.is_empty());
    }
    #[test]
    fn folding() {
        assert_eq!(fold("¡Corazón,  ESPINADO!"), "corazon espinado");
    }
}
