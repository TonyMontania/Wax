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
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::{data_dir, file_name, http};

/// Small JSON/text cache in data/cache/<kind>/.
fn cached<T: Serialize + for<'de> Deserialize<'de>>(kind: &str, key: &str, days: u64, fetch: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let dir = data_dir().join("cache").join(kind);
    let file = dir.join(format!("{}.json", file_name(key)));
    let fresh = fs::metadata(&file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < Duration::from_secs(days * 86400));
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

/* ───────────── Which Deezer artist is yours ───────────── */

/// A title without "(Deluxe Edition)", "[Remastered 2011]", " - Live", "feat." and the like
fn bare_title(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let out = out.split(" - ").next().unwrap_or("").to_string();
    let out = out.split(" feat").next().unwrap_or("").to_string();
    simple(&out)
}

fn dz_get(url: &str, limit: &str) -> Result<Value, String> {
    let v: Value = http().get(url).query("limit", limit).call().map_err(|e| format!("Deezer: {e}"))?.body_mut().read_json().map_err(|e| e.to_string())?;
    if let Some(e) = v.get("error") {
        return Err(format!("Deezer: {}", e["message"].as_str().unwrap_or("error")));
    }
    Ok(v)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DzArtist {
    pub id: u64,
    pub picture: String,
    /// How many of your album and song titles this artist has
    pub matches: usize,
}

/// Several artists share a name (Ghost the band, Ghost the rapper…). The one that's yours is the
/// one whose albums and songs match the ones in your library; if none match, none is used rather
/// than a stranger. `titles`: your album and song titles of that artist.
pub fn deezer_artist(name: &str, titles: &[String]) -> Result<Option<DzArtist>, String> {
    let mine: std::collections::HashSet<String> = titles.iter().map(|t| bare_title(t)).filter(|t| t.len() > 1).collect();
    let key = format!("{name} - {}", mine.len());
    cached("deezer artists", &key, 14, || {
        let v = http()
            .get("https://api.deezer.com/search/artist")
            .query("q", name)
            .query("limit", "25")
            .call()
            .map_err(|e| format!("Deezer: {e}"))?
            .body_mut()
            .read_json::<Value>()
            .map_err(|e| e.to_string())?;
        let want = simple(name);
        let mut same: Vec<&Value> = v["data"].as_array().into_iter().flatten().filter(|a| simple(a["name"].as_str().unwrap_or("")) == want).collect();
        same.sort_by_key(|a| std::cmp::Reverse(a["nb_fan"].as_u64().unwrap_or(0)));
        let mut best: Option<DzArtist> = None;
        for a in same.into_iter().take(6) {
            let Some(id) = a["id"].as_u64() else { continue };
            let albums = dz_get(&format!("https://api.deezer.com/artist/{id}/albums"), "200")?;
            let top = dz_get(&format!("https://api.deezer.com/artist/{id}/top"), "100")?;
            let theirs: std::collections::HashSet<String> = albums["data"]
                .as_array()
                .into_iter()
                .flatten()
                .chain(top["data"].as_array().into_iter().flatten())
                .map(|x| bare_title(x["title"].as_str().unwrap_or("")))
                .collect();
            let matches = mine.iter().filter(|t| theirs.contains(*t)).count();
            let picture = a["picture_xl"].as_str().filter(|u| !u.contains("/artist//")).unwrap_or("").to_string();
            if matches > 0 && best.as_ref().is_none_or(|b| matches > b.matches) {
                best = Some(DzArtist { id, picture, matches });
            }
        }
        Ok(best)
    })
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

/// The label of a Deezer album ("" if unknown)
fn album_label(id: u64) -> String {
    std::thread::sleep(Duration::from_millis(120)); // Deezer allows 50 requests per 5 s
    dz_get(&format!("https://api.deezer.com/album/{id}"), "1").ok().and_then(|v| v["label"].as_str().map(simple)).unwrap_or_default()
}

/// The releases of your artist on Deezer (newest first), from the last two years. None if
/// Deezer has no artist of that name whose music matches yours. Cached for 3 days.
///
/// Deezer sometimes files songs of other people with the same name under one artist (often
/// AI-made filler): a new release only counts if it's on a label your albums of that artist
/// came out on.
pub fn releases_of(name: &str, titles: &[String]) -> Result<Option<ArtistReleases>, String> {
    let mine: std::collections::HashSet<String> = titles.iter().map(|t| bare_title(t)).collect();
    cached("releases", &format!("{name} - {}", mine.len()), 3, || {
        let Some(artist) = deezer_artist(name, titles)? else { return Ok(None) };
        let v = dz_get(&format!("https://api.deezer.com/artist/{}/albums", artist.id), "200")?;
        let all: Vec<&Value> = v["data"].as_array().into_iter().flatten().collect();
        let since = {
            let days = SystemTimeDays::now() - 760;
            SystemTimeDays::date(days)
        };
        // Labels of the releases you have (a few are enough)
        let labels: std::collections::HashSet<String> = all
            .iter()
            .filter(|a| mine.contains(&bare_title(a["title"].as_str().unwrap_or(""))))
            .take(5)
            .filter_map(|a| a["id"].as_u64())
            .map(album_label)
            .filter(|l| !l.is_empty())
            .collect();
        let mut releases: Vec<Release> = all
            .iter()
            .filter(|a| a["release_date"].as_str().unwrap_or("") >= since.as_str())
            .filter(|a| !mine.contains(&bare_title(a["title"].as_str().unwrap_or(""))))
            .filter(|a| labels.is_empty() || a["id"].as_u64().is_some_and(|id| labels.contains(&album_label(id))))
            .map(|a| Release {
                title: a["title"].as_str().unwrap_or("").into(),
                date: a["release_date"].as_str().unwrap_or("").into(),
                kind: a["record_type"].as_str().unwrap_or("album").into(),
                cover: a["cover_medium"].as_str().unwrap_or("").into(),
                link: a["link"].as_str().unwrap_or("").into(),
            })
            .collect();
        releases.sort_by(|a, b| b.date.cmp(&a.date));
        Ok(Some(ArtistReleases { name: name.to_string(), releases }))
    })
}

/// Days since 1970 and back to "YYYY-MM-DD" (no date library needed for this)
struct SystemTimeDays;
impl SystemTimeDays {
    fn now() -> i64 {
        (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() / 86400) as i64
    }
    fn date(days: i64) -> String {
        // Howard Hinnant's civil_from_days
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
        format!("{y:04}-{m:02}-{d:02}")
    }
}

#[tauri::command]
pub async fn artist_releases(name: String, titles: Option<Vec<String>>) -> Result<Option<ArtistReleases>, String> {
    tauri::async_runtime::spawn_blocking(move || releases_of(&name, &titles.unwrap_or_default())).await.map_err(|e| e.to_string())?
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
        cached("credits", &format!("{artist} - {title}"), 30, || {
            let q = format!("recording:\"{}\" AND artist:\"{}\"", title.replace('"', ""), artist.replace('"', ""));
            let v = crate::online::mb_get("recording", &[("query", &q), ("limit", "15")])?;
            let recs = v["recordings"].as_array().cloned().unwrap_or_default();
            // Best match: high score and the closest length
            let best = recs.iter().filter(|r| r["score"].as_u64().unwrap_or(0) >= 80).min_by_key(|r| {
                let len = r["length"].as_u64().unwrap_or(0);
                if duration_ms == 0 || len == 0 {
                    5000
                } else {
                    len.abs_diff(duration_ms)
                }
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
            let artist_credit: String =
                r["artist-credit"].as_array().into_iter().flatten().map(|c| format!("{}{}", c["name"].as_str().unwrap_or(""), c["joinphrase"].as_str().unwrap_or(""))).collect();
            Ok(Some(Credits { title: r["title"].as_str().unwrap_or(&title).into(), artist: artist_credit, url: format!("https://musicbrainz.org/recording/{id}"), credits }))
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/* ───────────── Searching by lyrics ───────────── */

/// (how many songs it was built from, [(path, words of the lyrics)])
type LyricsIndex = (usize, Vec<(String, Vec<String>)>);
static LYRICS_INDEX: Mutex<Option<LyricsIndex>> = Mutex::new(None);

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
fn lyric_lines(path: &str, name: &str) -> Vec<String> {
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
        let cache = data_dir().join("lyrics").join(format!("{}.json", file_name(name)));
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
pub async fn search_lyrics(query: String, songs: Vec<(String, String)>, rebuild: bool) -> Vec<LyricHit> {
    tauri::async_runtime::spawn_blocking(move || {
        let words: Vec<String> = fold(&query).split(' ').filter(|w| w.len() > 1).map(str::to_string).collect();
        if words.is_empty() {
            return Vec::new();
        }
        let mut idx = LYRICS_INDEX.lock().unwrap_or_else(|e| e.into_inner());
        // songs: (path, "Artist - Title")
        if rebuild || idx.as_ref().is_none_or(|(n, _)| *n != songs.len()) {
            let list: Vec<(String, Vec<String>)> = songs.par_iter().map(|(p, n)| (p.clone(), lyric_lines(p, n))).filter(|(_, l)| !l.is_empty()).collect();
            *idx = Some((songs.len(), list));
        }
        let mut hits = Vec::new();
        for (path, lines) in &idx.as_ref().unwrap().1 {
            let folded: Vec<String> = lines.iter().map(|l| fold(l)).collect();
            let has_all = |t: &str| words.iter().all(|w| t.contains(w.as_str()));
            let line = folded.iter().position(|l| has_all(l)).or_else(|| {
                let all = folded.join(" ");
                if has_all(&all) {
                    folded.iter().position(|l| l.contains(words[0].as_str()))
                } else {
                    None
                }
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
        cached("autoeq", &path.replace("%20", " ").replace('/', " - "), 90, || {
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
        // Ghost the Swedish band, not the rappers and the filler filed under the same name
        let titles: Vec<String> = ["Opus Eponymous", "Infestissumam", "Meliora", "Prequelle", "Impera", "Square Hammer", "Mary On A Cross", "Cirice"].map(String::from).to_vec();
        let ghost = deezer_artist("Ghost", &titles).unwrap().unwrap();
        println!("Ghost: Deezer artist {} ({} titles match) · {}", ghost.id, ghost.matches, ghost.picture);
        assert!(ghost.matches >= 4);
        let r = releases_of("Ghost", &titles).unwrap().unwrap();
        println!("Ghost releases (2 years): {}", r.releases.iter().map(|x| format!("{} ({}, {})", x.title, x.kind, x.date)).collect::<Vec<_>>().join(" | "));
        assert!(!r.releases.iter().any(|x| x.title.contains("Pogoda") || x.title.contains("Где")));
        // An artist nobody with that name matches: nothing rather than a stranger
        assert!(deezer_artist("Ghost", &["zzqx not a real album".into()]).unwrap().is_none());
        let broco = deezer_artist("Don Broco", &["Technology".into(), "Amazing Things".into(), "Priorities".into()]).unwrap().unwrap();
        println!("Don Broco: {} · {}", broco.id, broco.picture);
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
        assert_eq!(bare_title("Meliora (Deluxe Edition) [Remastered]"), "meliora");
        assert_eq!(bare_title("Square Hammer - Live"), "squarehammer");
        assert_eq!(bare_title("Kiss the Go-Goat feat. Someone"), "kissthegogoat");
        assert_eq!(SystemTimeDays::date(0), "1970-01-01");
        assert_eq!(SystemTimeDays::date(20_749), "2026-10-23");
        assert_eq!(SystemTimeDays::date(11_016), "2000-02-29");
    }
}
