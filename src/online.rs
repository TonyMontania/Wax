//! Online services: MusicBrainz (tags) and scrobbling
//! (Last.fm / ListenBrainz).

use std::collections::BTreeMap;
use std::fs;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{data_dir, http, open_url};

/* ───────────── MusicBrainz ───────────── */

static MB_LAST: Mutex<Option<Instant>> = Mutex::new(None);

/// MusicBrainz asks for at most 1 request per second.
pub(crate) fn mb_get(path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    {
        let mut last = MB_LAST.lock().map_err(|e| e.to_string())?;
        if let Some(t) = *last {
            let wait = Duration::from_millis(1100).saturating_sub(t.elapsed());
            std::thread::sleep(wait);
        }
        *last = Some(Instant::now());
    }
    // If the server is busy (503), retry a couple of times
    let mut last_err = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(1500 * attempt));
        }
        let mut req = http().get(&format!("https://musicbrainz.org/ws/2/{path}")).query("fmt", "json");
        for (k, v) in query {
            req = req.query(*k, *v);
        }
        match req.call() {
            Ok(mut r) => return r.body_mut().read_json().map_err(|e| e.to_string()),
            Err(ureq::Error::StatusCode(503)) => last_err = "MusicBrainz is busy, try again in a moment".into(),
            Err(e) => return Err(format!("MusicBrainz: {e}")),
        }
    }
    Err(last_err)
}

fn credit(v: &Value) -> (String, Vec<String>) {
    let mut text = String::new();
    let mut names = Vec::new();
    for c in v.as_array().into_iter().flatten() {
        let name = c["name"].as_str().unwrap_or_default();
        text.push_str(name);
        text.push_str(c["joinphrase"].as_str().unwrap_or_default());
        let real = c["artist"]["name"].as_str().unwrap_or(name);
        if !real.is_empty() {
            names.push(real.to_string());
        }
    }
    (text, names)
}

#[derive(Serialize)]
pub struct MbRelease {
    id: String,
    title: String,
    artist: String,
    date: String,
    country: String,
    tracks: u64,
    format: String,
}

#[tauri::command]
pub async fn mb_search(artist: String, album: String) -> Result<Vec<MbRelease>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let esc = |s: &str| s.replace('"', "");
        let q = format!("release:\"{}\" AND artist:\"{}\"", esc(&album), esc(&artist));
        let v = mb_get("release/", &[("query", &q), ("limit", "12")])?;
        Ok(v["releases"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| MbRelease {
                id: r["id"].as_str().unwrap_or_default().into(),
                title: r["title"].as_str().unwrap_or_default().into(),
                artist: credit(&r["artist-credit"]).0,
                date: r["date"].as_str().unwrap_or_default().into(),
                country: r["country"].as_str().unwrap_or_default().into(),
                tracks: r["track-count"].as_u64().unwrap_or(0),
                format: r["media"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| m["format"].as_str())
                    .collect::<Vec<_>>()
                    .join(" + "),
            })
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MbTrack {
    disc: u64,
    number: u64,
    title: String,
    artist: String,
    artists: Vec<String>,
    length_ms: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MbReleaseDetail {
    title: String,
    artist: String,
    year: u32,
    genres: Vec<String>,
    tracks: Vec<MbTrack>,
}

#[tauri::command]
pub async fn mb_release(id: String) -> Result<MbReleaseDetail, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let v = mb_get(&format!("release/{id}"), &[("inc", "recordings+artist-credits+genres+release-groups")])?;
        let mut genres: Vec<(u64, String)> = v["genres"]
            .as_array()
            .into_iter()
            .chain(v["release-group"]["genres"].as_array())
            .flatten()
            .filter_map(|g| Some((g["count"].as_u64().unwrap_or(0), g["name"].as_str()?.to_string())))
            .collect();
        genres.sort_by(|a, b| b.0.cmp(&a.0));
        let mut gnames: Vec<String> = Vec::new();
        for (_, g) in genres {
            let g = g
                .split(' ')
                .map(|w| {
                    let mut c = w.chars();
                    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(" ");
            if !gnames.contains(&g) && gnames.len() < 3 {
                gnames.push(g);
            }
        }
        let mut tracks = Vec::new();
        for m in v["media"].as_array().into_iter().flatten() {
            let disc = m["position"].as_u64().unwrap_or(1);
            for t in m["tracks"].as_array().into_iter().flatten() {
                let (artist, artists) = credit(&t["artist-credit"]);
                tracks.push(MbTrack {
                    disc,
                    number: t["position"].as_u64().unwrap_or(0),
                    title: t["title"].as_str().unwrap_or_default().into(),
                    artist,
                    artists,
                    length_ms: t["length"].as_u64().unwrap_or(0),
                });
            }
        }
        Ok(MbReleaseDetail {
            title: v["title"].as_str().unwrap_or_default().into(),
            artist: credit(&v["artist-credit"]).0,
            year: v["date"].as_str().and_then(|d| d.get(..4)).and_then(|y| y.parse().ok()).unwrap_or(0),
            genres: gnames,
            tracks,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/* ───────────── Scrobbling ───────────── */

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScrobbleCreds {
    #[serde(default)]
    lb_token: String,
    #[serde(default)]
    lastfm_key: String,
    #[serde(default)]
    lastfm_secret: String,
    #[serde(default)]
    lastfm_session: String,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScrobbleTrack {
    artist: String,
    title: String,
    album: String,
    #[serde(default)]
    album_artist: String,
    duration: u32,
    timestamp: u64,
}

const LASTFM_API: &str = "https://ws.audioscrobbler.com/2.0/";

fn lastfm_sign(params: &BTreeMap<&str, String>, secret: &str) -> String {
    let mut s = String::new();
    for (k, v) in params {
        s.push_str(k);
        s.push_str(v);
    }
    s.push_str(secret);
    format!("{:x}", Md5::digest(s.as_bytes()))
}

fn lastfm_call(mut params: BTreeMap<&str, String>, secret: &str, post: bool) -> Result<Value, String> {
    let sig = lastfm_sign(&params, secret);
    params.insert("api_sig", sig);
    params.insert("format", "json".into());
    let resp = if post {
        http().post(LASTFM_API).send_form(params.iter().map(|(k, v)| (*k, v.as_str())))
    } else {
        let mut r = http().get(LASTFM_API);
        for (k, v) in &params {
            r = r.query(*k, v);
        }
        r.call()
    };
    let v: Value = resp.map_err(|e| format!("Last.fm: {e}"))?.body_mut().read_json().map_err(|e| e.to_string())?;
    if let Some(msg) = v["message"].as_str().filter(|_| v["error"].is_number()) {
        return Err(format!("Last.fm: {msg}"));
    }
    Ok(v)
}

fn lastfm_track(c: &ScrobbleCreds, t: &ScrobbleTrack, now: bool) -> Result<(), String> {
    let mut p = BTreeMap::new();
    p.insert("method", if now { "track.updateNowPlaying" } else { "track.scrobble" }.to_string());
    p.insert("api_key", c.lastfm_key.clone());
    p.insert("sk", c.lastfm_session.clone());
    p.insert("artist", t.artist.clone());
    p.insert("track", t.title.clone());
    if !t.album.is_empty() {
        p.insert("album", t.album.clone());
    }
    if !t.album_artist.is_empty() {
        p.insert("albumArtist", t.album_artist.clone());
    }
    if t.duration > 0 {
        p.insert("duration", t.duration.to_string());
    }
    if !now {
        p.insert("timestamp", t.timestamp.to_string());
    }
    lastfm_call(p, &c.lastfm_secret, true).map(|_| ())
}

fn listenbrainz(c: &ScrobbleCreds, t: &ScrobbleTrack, now: bool) -> Result<(), String> {
    let mut listen = json!({
        "track_metadata": {
            "artist_name": t.artist,
            "track_name": t.title,
            "release_name": t.album,
            "additional_info": {
                "duration_ms": t.duration as u64 * 1000,
                "media_player": "Wax",
                "submission_client": "Wax",
                "submission_client_version": env!("CARGO_PKG_VERSION"),
            }
        }
    });
    if !now {
        listen["listened_at"] = json!(t.timestamp);
    }
    let body = json!({ "listen_type": if now { "playing_now" } else { "single" }, "payload": [listen] });
    http()
        .post("https://api.listenbrainz.org/1/submit-listens")
        .header("Authorization", &format!("Token {}", c.lb_token))
        .send_json(body)
        .map(|_| ())
        .map_err(|e| format!("ListenBrainz: {e}"))
}

#[derive(Serialize, Deserialize)]
struct Pending {
    track: ScrobbleTrack,
    lastfm: bool,
    lb: bool,
}

fn queue_file() -> std::path::PathBuf {
    data_dir().join("scrobble_queue.json")
}

fn submit(c: &ScrobbleCreds, t: &ScrobbleTrack, lastfm: bool, lb: bool) -> (bool, bool, Vec<String>) {
    let mut errs = Vec::new();
    let lf_ok = !lastfm || lastfm_track(c, t, false).map_err(|e| errs.push(e)).is_ok();
    let lb_ok = !lb || listenbrainz(c, t, false).map_err(|e| errs.push(e)).is_ok();
    (lf_ok, lb_ok, errs)
}

/// Sends "now playing" or a finished listen. If the connection fails,
/// the listen is queued (data/scrobble_queue.json) and retried later.
#[tauri::command]
pub async fn scrobble(creds: ScrobbleCreds, track: ScrobbleTrack, now_playing: bool) -> Vec<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let use_lf = !creds.lastfm_key.is_empty() && !creds.lastfm_session.is_empty();
        let use_lb = !creds.lb_token.is_empty();
        if !use_lf && !use_lb {
            return Vec::new();
        }
        if now_playing {
            let mut errs = Vec::new();
            if use_lf {
                if let Err(e) = lastfm_track(&creds, &track, true) {
                    errs.push(e);
                }
            }
            if use_lb {
                if let Err(e) = listenbrainz(&creds, &track, true) {
                    errs.push(e);
                }
            }
            return errs;
        }
        // First, whatever is still pending
        let mut queue: Vec<Pending> = fs::read_to_string(queue_file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        queue.push(Pending { track, lastfm: use_lf, lb: use_lb });
        let mut left = Vec::new();
        let mut errs = Vec::new();
        for (i, p) in queue.into_iter().enumerate() {
            if !errs.is_empty() && i > 0 {
                left.push(p); // offline: stop trying for now
                continue;
            }
            let (lf, lb, e) = submit(&creds, &p.track, p.lastfm, p.lb);
            if !(lf && lb) {
                left.push(Pending { track: p.track, lastfm: !lf, lb: !lb });
                errs.extend(e);
            }
        }
        let _ = fs::write(queue_file(), serde_json::to_string(&left).unwrap_or_default());
        errs
    })
    .await
    .unwrap_or_default()
}

#[derive(Serialize)]
pub struct LastfmAuth {
    token: String,
}

/// Step 1: requests a token and opens the browser to authorize Wax.
#[tauri::command]
pub async fn lastfm_auth_start(key: String, secret: String) -> Result<LastfmAuth, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut p = BTreeMap::new();
        p.insert("method", "auth.getToken".to_string());
        p.insert("api_key", key.clone());
        let v = lastfm_call(p, &secret, false)?;
        let token = v["token"].as_str().ok_or("Last.fm didn't return a token")?.to_string();
        open_url(&format!("https://www.last.fm/api/auth/?api_key={key}&token={token}"));
        Ok(LastfmAuth { token })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub struct LastfmSession {
    session: String,
    name: String,
}

/// Step 2: after authorizing in the browser, gets the session.
#[tauri::command]
pub async fn lastfm_auth_finish(key: String, secret: String, token: String) -> Result<LastfmSession, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut p = BTreeMap::new();
        p.insert("method", "auth.getSession".to_string());
        p.insert("api_key", key);
        p.insert("token", token);
        let v = lastfm_call(p, &secret, false)?;
        Ok(LastfmSession {
            session: v["session"]["key"].as_str().ok_or("You haven't authorized Wax on Last.fm yet")?.into(),
            name: v["session"]["name"].as_str().unwrap_or_default().into(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read-only Last.fm call (artist info, similar artists, top tracks, tags, a user's
/// loved tracks and history). Answers are cached in data/lastfm for `cache_days`
/// (0 = no cache); "not found" answers are cached too, so they aren't asked again.
#[tauri::command]
pub async fn lastfm_get(key: String, method: String, params: BTreeMap<String, String>, cache_days: u32) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = data_dir().join("lastfm");
        let id = format!("{method}|{}", params.iter().map(|(k, v)| format!("{k}={}", v.to_lowercase())).collect::<Vec<_>>().join("&"));
        let file = dir.join(format!("{:x}.json", Md5::digest(id.as_bytes())));
        if cache_days > 0 {
            let fresh = fs::metadata(&file)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age < Duration::from_secs(cache_days as u64 * 86400));
            if fresh {
                if let Some(v) = fs::read_to_string(&file).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
                    return Ok(v);
                }
            }
        }
        let mut r = http().get(LASTFM_API).query("method", &method).query("api_key", &key).query("format", "json");
        for (k, v) in &params {
            r = r.query(k, v);
        }
        // Last.fm answers errors (bad key, "artist not found") with HTTP 4xx and a JSON body
        let mut resp = r.config().http_status_as_error(false).build().call().map_err(|e| format!("Last.fm: {e}"))?;
        let status = resp.status().as_u16();
        if status >= 500 {
            return Err(format!("Last.fm: server error {status}"));
        }
        let v: Value = resp.body_mut().read_json().unwrap_or_else(|_| json!({ "error": status, "message": "unreadable answer" }));
        if v["error"].as_i64().is_some_and(|c| c == 10 || c == 26) {
            return Err("Last.fm: invalid API key".into());
        }
        if cache_days > 0 {
            let _ = fs::create_dir_all(&dir);
            let _ = fs::write(&file, v.to_string());
        }
        Ok(v)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// ♥ on Wax → "Loved" on Last.fm (and back).
#[tauri::command]
pub async fn lastfm_love(creds: ScrobbleCreds, artist: String, title: String, love: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if creds.lastfm_key.is_empty() || creds.lastfm_session.is_empty() {
            return Ok(());
        }
        let mut p = BTreeMap::new();
        p.insert("method", if love { "track.love" } else { "track.unlove" }.to_string());
        p.insert("api_key", creds.lastfm_key.clone());
        p.insert("sk", creds.lastfm_session.clone());
        p.insert("artist", artist);
        p.insert("track", title);
        lastfm_call(p, &creds.lastfm_secret, true).map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Checks a ListenBrainz token and returns the user name.
#[tauri::command]
pub async fn listenbrainz_check(token: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let v: Value = http()
            .get("https://api.listenbrainz.org/1/validate-token")
            .header("Authorization", &format!("Token {token}"))
            .call()
            .map_err(|e| format!("ListenBrainz: {e}"))?
            .body_mut()
            .read_json()
            .map_err(|e| e.to_string())?;
        if v["valid"].as_bool() == Some(true) {
            Ok(v["user_name"].as_str().unwrap_or_default().to_string())
        } else {
            Err("The ListenBrainz token isn't valid".into())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Without a real key Last.fm must answer "invalid API key" (and Wax must say so).
    #[test]
    fn lastfm_rejects_bad_key() {
        if std::env::var("WAX_NET").is_err() {
            return;
        }
        let params = BTreeMap::from([("artist".to_string(), "Radiohead".to_string())]);
        let r = tauri::async_runtime::block_on(lastfm_get("0".repeat(32), "artist.getInfo".into(), params, 0));
        println!("bad key: {r:?}");
        assert!(r.unwrap_err().contains("invalid API key"));
    }

    #[test]
    fn musicbrainz_lookup() {
        if std::env::var("WAX_NET").is_err() {
            return;
        }
        let rt = match tauri::async_runtime::block_on(mb_search("Radiohead".into(), "OK Computer".into())) {
            Ok(r) => r,
            Err(e) => panic!("MusicBrainz error: {e}"),
        };
        println!("releases: {}", rt.len());
        for r in rt.iter().take(3) {
            println!("  {} — {} · {} · {} tracks", r.title, r.artist, r.date, r.tracks);
        }
        let d = tauri::async_runtime::block_on(mb_release(rt[0].id.clone())).unwrap();
        println!("detail: {} · {} · {} · genres {:?} · {} tracks, first: {} ({:?})", d.title, d.artist, d.year, d.genres, d.tracks.len(), d.tracks[0].title, d.tracks[0].artists);
        assert!(!d.tracks.is_empty());
    }
}
