//! Library: (incremental) scanning, tags, organizer, M3U, history and
//! new-music detection.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey, ItemValue, Tag, TagExt, TagItem, TagType};
use notify::{RecursiveMode, Watcher as _};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use walkdir::WalkDir;

use crate::cue::{read_sheet, CueRef};
use crate::{clean, data_dir, has_ext, AUDIO_EXT, IMAGE_EXT};

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ReplayGain {
    pub track: Option<f32>,
    pub album: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_peak: Option<f32>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    /// Artists as separate tag values (ARTISTS, or a multi-value ARTIST).
    /// Empty when the file has a single artist: text is never split by guessing.
    #[serde(default)]
    pub artists: Vec<String>,
    pub track_no: u32,
    pub disc_no: u32,
    pub year: u32,
    pub duration: f64,
    /// File modification time (seconds)
    pub added: u64,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub rg: Option<ReplayGain>,
    #[serde(default)]
    pub size: u64,
    /// Track of a CUE sheet: `path` is virtual (`sheet.cue#NN`), this points at the audio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cue: Option<CueRef>,
}

fn file_meta(path: &Path) -> (u64, u64) {
    fs::metadata(path)
        .map(|m| {
            let mt = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
            (mt, m.len())
        })
        .unwrap_or((0, 0))
}

fn parse_db(s: &str) -> Option<f32> {
    s.split_whitespace().next()?.replace(',', ".").parse().ok()
}

pub fn read_track(path: &Path) -> Track {
    let (added, size) = file_meta(path);
    let mut t = Track {
        path: path.to_string_lossy().into_owned(),
        title: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        artist: String::new(),
        album: String::new(),
        album_artist: String::new(),
        artists: Vec::new(),
        track_no: 0,
        disc_no: 0,
        year: 0,
        duration: 0.0,
        added,
        genres: Vec::new(),
        rg: None,
        size,
        cue: None,
    };

    let parsed = Probe::open(path).map(|p| p.options(ParseOptions::new().read_cover_art(false))).and_then(|p| p.read());
    if let Ok(file) = parsed {
        t.duration = file.properties().duration().as_secs_f64();
        if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
            let values = |key: &ItemKey| -> Vec<String> {
                let mut v: Vec<String> = Vec::new();
                for s in tag.get_strings(key).filter_map(|s| clean(Some(s))) {
                    if !v.contains(&s) {
                        v.push(s);
                    }
                }
                v
            };
            if let Some(v) = clean(tag.title()) {
                t.title = v;
            }
            // ID3v2.4 (null separator), Vorbis/FLAC and MP4 can carry several ARTIST values;
            // lofty delivers them as separate items. ARTISTS (Picard, etc.) is the explicit list.
            let multi = values(&ItemKey::TrackArtist);
            let explicit = values(&ItemKey::TrackArtists);
            if let Some(first) = multi.first() {
                t.artist = if multi.len() > 1 { multi.join(", ") } else { first.clone() };
            }
            t.artists = if explicit.len() > 1 {
                explicit
            } else if multi.len() > 1 {
                multi
            } else {
                Vec::new()
            };
            if t.artist.is_empty() && !t.artists.is_empty() {
                t.artist = t.artists.join(", ");
            }
            if let Some(v) = clean(tag.album()) {
                t.album = v;
            }
            if let Some(v) = clean(tag.get_string(&ItemKey::AlbumArtist)) {
                t.album_artist = v;
            }
            t.track_no = tag.track().unwrap_or(0);
            t.disc_no = tag.disk().unwrap_or(0);
            t.year = tag.get_string(&ItemKey::Year).or_else(|| tag.get_string(&ItemKey::RecordingDate)).and_then(|s| s.trim().get(..4).and_then(|y| y.parse().ok())).unwrap_or(0);
            // Genres: multiple values or separated by ";" / ","
            let mut genres: Vec<String> = Vec::new();
            for g in values(&ItemKey::Genre) {
                for part in g.split([';', ',', '\0']).map(str::trim).filter(|x| !x.is_empty()) {
                    if !genres.iter().any(|x| x.eq_ignore_ascii_case(part)) {
                        genres.push(part.to_string());
                    }
                }
            }
            t.genres = genres;
            let rg = ReplayGain {
                track: tag.get_string(&ItemKey::ReplayGainTrackGain).and_then(parse_db),
                album: tag.get_string(&ItemKey::ReplayGainAlbumGain).and_then(parse_db),
                track_peak: tag.get_string(&ItemKey::ReplayGainTrackPeak).and_then(parse_db),
                album_peak: tag.get_string(&ItemKey::ReplayGainAlbumPeak).and_then(parse_db),
            };
            if rg.track.is_some() || rg.album.is_some() {
                t.rg = Some(rg);
            }
        }
    }

    if t.artist.is_empty() {
        t.artist = if t.album_artist.is_empty() { "Unknown Artist".into() } else { t.album_artist.clone() };
    }
    if t.album.is_empty() {
        t.album = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Unknown Album".into());
    }
    t
}

fn read_cache() -> Vec<Track> {
    fs::read_to_string(data_dir().join("library.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Incremental scan: files with the same date and size aren't read again.
fn scan(folders: &[String], prev: Vec<Track>) -> Vec<Track> {
    let prev: HashMap<String, Track> = prev.into_iter().map(|t| (t.path.clone(), t)).collect();
    let mut seen = HashSet::new();
    let all: Vec<PathBuf> = folders
        .iter()
        .flat_map(|f| {
            WalkDir::new(f)
                .follow_links(true)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_file() && (has_ext(e.path(), AUDIO_EXT) || has_ext(e.path(), &["cue"])))
                .map(|e| e.into_path())
        })
        .filter(|p| seen.insert(p.clone()))
        .collect();
    let (cues, files): (Vec<PathBuf>, Vec<PathBuf>) = all.into_iter().partition(|p| has_ext(p, &["cue"]));

    // CUE sheets: their tracks replace the whole-album file they point at
    let sheets: Vec<(Vec<Track>, Vec<PathBuf>)> = cues
        .par_iter()
        .map(|c| {
            let (mt, sz) = file_meta(c);
            let first = format!("{}#", c.to_string_lossy());
            let old: Vec<Track> = prev.values().filter(|t| t.path.starts_with(&first)).cloned().collect();
            if !old.is_empty() && old.iter().all(|t| t.added == mt && t.size == sz && t.cue.as_ref().is_some_and(|r| Path::new(&r.file).exists())) {
                let used = old.iter().filter_map(|t| t.cue.as_ref().map(|r| PathBuf::from(&r.file))).collect();
                return (old, used);
            }
            read_sheet(c, mt, sz)
        })
        .collect();
    let covered: HashSet<PathBuf> = sheets.iter().flat_map(|(_, f)| f.iter().cloned()).collect();
    let files: Vec<PathBuf> = files.into_iter().filter(|f| !covered.contains(f)).collect();

    let mut tracks: Vec<Track> = files
        .par_iter()
        .map(|p| {
            let key = p.to_string_lossy();
            if let Some(old) = prev.get(key.as_ref()) {
                let (mt, sz) = file_meta(p);
                if old.size != 0 && old.size == sz && old.added == mt {
                    return old.clone();
                }
            }
            read_track(p)
        })
        .collect();
    tracks.extend(sheets.into_iter().flat_map(|(t, _)| t));
    tracks.sort_by(|a, b| a.path.cmp(&b.path));
    tracks
}

pub fn allow_folders(app: &AppHandle, folders: &[String]) {
    let scope = app.asset_protocol_scope();
    for f in folders {
        let _ = scope.allow_directory(f, true);
    }
    let _ = scope.allow_directory(data_dir(), true);
}

fn save_cache(tracks: &[Track]) {
    let tmp = data_dir().join("library.json.tmp");
    if fs::write(&tmp, serde_json::to_string(tracks).unwrap_or_else(|_| "[]".into())).is_ok() {
        let _ = fs::rename(&tmp, data_dir().join("library.json"));
    }
}

#[tauri::command]
pub fn load_library(app: AppHandle, folders: Vec<String>) -> Vec<Track> {
    allow_folders(&app, &folders);
    read_cache()
}

#[tauri::command]
pub async fn scan_folders(app: AppHandle, folders: Vec<String>) -> Result<Vec<Track>, String> {
    allow_folders(&app, &folders);
    tauri::async_runtime::spawn_blocking(move || {
        let tracks = scan(&folders, read_cache());
        // On rescan, local covers that didn't exist before are looked up again.
        crate::covers::forget_local_misses();
        save_cache(&tracks);
        tracks
    })
    .await
    .map_err(|e| e.to_string())
}

/* ───────────── Tag editing ───────────── */

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TagUpdate {
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub artists: Option<Vec<String>>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub year: Option<u32>,
    pub genres: Option<Vec<String>>,
}

fn set_text(tag: &mut Tag, key: ItemKey, v: &str) {
    let v = v.trim();
    if v.is_empty() {
        tag.remove_key(&key);
    } else {
        tag.insert_text(key, v.to_string());
    }
}

/// Saves several values in one field. ID3v2.4 separates them with "\0";
/// Vorbis/FLAC/MP4 use one item per value.
fn set_multi(tag: &mut Tag, key: ItemKey, values: &[String]) {
    tag.remove_key(&key);
    let values: Vec<&str> = values.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if values.is_empty() {
        return;
    }
    if tag.tag_type() == TagType::Id3v2 {
        tag.insert_text(key, values.join("\0"));
    } else {
        for v in values {
            tag.push(TagItem::new(key.clone(), ItemValue::Text(v.to_string())));
        }
    }
}

/// Opens the file's main tag (creating it if needed), lets `f` change it and saves it,
/// keeping the file's modification date (the library sorts "added" by it).
fn with_tag(path: &str, f: impl FnOnce(&mut Tag) -> Result<(), String>) -> Result<Track, String> {
    let p = Path::new(path);
    let mtime = fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut file = lofty::read_from_path(p).map_err(|e| e.to_string())?;
    if file.primary_tag().is_none() {
        let tt = file.primary_tag_type();
        file.insert_tag(Tag::new(tt));
    }
    let tag = file.primary_tag_mut().ok_or("Couldn't create the tag")?;
    f(tag)?;
    tag.save_to_path(p, WriteOptions::default()).map_err(|e| e.to_string())?;
    if let Some(t) = mtime {
        if let Ok(f) = fs::File::options().write(true).open(p) {
            let _ = f.set_modified(t);
        }
    }
    Ok(read_track(p))
}

fn apply_update(u: &TagUpdate) -> Result<Track, String> {
    with_tag(&u.path, |tag| {
        if let Some(v) = &u.title {
            set_text(tag, ItemKey::TrackTitle, v);
        }
        match (&u.artists, &u.artist) {
            (Some(list), display) if list.iter().filter(|s| !s.trim().is_empty()).count() > 1 => {
                let display = display.clone().unwrap_or_else(|| list.join(", "));
                set_text(tag, ItemKey::TrackArtist, &display);
                set_multi(tag, ItemKey::TrackArtists, list);
            }
            (_, Some(a)) => {
                set_text(tag, ItemKey::TrackArtist, a);
                tag.remove_key(&ItemKey::TrackArtists);
            }
            _ => {}
        }
        if let Some(v) = &u.album {
            set_text(tag, ItemKey::AlbumTitle, v);
        }
        if let Some(v) = &u.album_artist {
            set_text(tag, ItemKey::AlbumArtist, v);
        }
        if let Some(n) = u.track_no {
            if n == 0 {
                tag.remove_key(&ItemKey::TrackNumber)
            } else {
                tag.set_track(n)
            }
        }
        if let Some(n) = u.disc_no {
            if n == 0 {
                tag.remove_key(&ItemKey::DiscNumber)
            } else {
                tag.set_disk(n)
            }
        }
        if let Some(y) = u.year {
            let s = if y == 0 { String::new() } else { y.to_string() };
            set_text(tag, ItemKey::Year, &s);
            set_text(tag, ItemKey::RecordingDate, &s);
        }
        if let Some(g) = &u.genres {
            set_multi(tag, ItemKey::Genre, g);
        }
        Ok(())
    })
}

#[derive(Serialize)]
pub struct TagResult {
    tracks: Vec<Track>,
    errors: Vec<String>,
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// Puts rewritten tracks into the library cache.
fn update_cache(tracks: &[Track]) {
    let mut cache = read_cache();
    let by: HashMap<&str, &Track> = tracks.iter().map(|t| (t.path.as_str(), t)).collect();
    for t in cache.iter_mut() {
        if let Some(n) = by.get(t.path.as_str()) {
            *t = (*n).clone();
        }
    }
    save_cache(&cache);
}

#[tauri::command]
pub async fn write_tags(updates: Vec<TagUpdate>) -> Result<TagResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut tracks = Vec::new();
        let mut errors = Vec::new();
        for u in &updates {
            match apply_update(u) {
                Ok(t) => tracks.push(t),
                Err(e) => errors.push(format!("{}: {e}", file_name(&u.path))),
            }
        }
        update_cache(&tracks);
        TagResult { tracks, errors }
    })
    .await
    .map_err(|e| e.to_string())
}

#[derive(Serialize, Clone)]
struct Progress {
    kind: &'static str,
    done: usize,
    total: usize,
}

/* ───────────── ReplayGain tags ───────────── */

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RgWrite {
    path: String,
    track_gain: f32,
    track_peak: f32,
    album_gain: Option<f32>,
    album_peak: Option<f32>,
}

/// Writes Wax's loudness analysis as ReplayGain tags, so other players use it too.
#[tauri::command]
pub async fn write_replaygain(app: AppHandle, items: Vec<RgWrite>) -> Result<TagResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (mut tracks, mut errors) = (Vec::new(), Vec::new());
        let total = items.len();
        for (n, it) in items.iter().enumerate() {
            let r = with_tag(&it.path, |tag| {
                tag.insert_text(ItemKey::ReplayGainTrackGain, format!("{:+.2} dB", it.track_gain));
                tag.insert_text(ItemKey::ReplayGainTrackPeak, format!("{:.6}", it.track_peak));
                if let (Some(g), Some(pk)) = (it.album_gain, it.album_peak) {
                    tag.insert_text(ItemKey::ReplayGainAlbumGain, format!("{g:+.2} dB"));
                    tag.insert_text(ItemKey::ReplayGainAlbumPeak, format!("{pk:.6}"));
                }
                Ok(())
            });
            match r {
                Ok(t) => tracks.push(t),
                Err(e) => errors.push(format!("{}: {e}", file_name(&it.path))),
            }
            if n % 10 == 0 || n + 1 == total {
                let _ = app.emit("batch-progress", Progress { kind: "rg", done: n + 1, total });
            }
        }
        update_cache(&tracks);
        TagResult { tracks, errors }
    })
    .await
    .map_err(|e| e.to_string())
}

/* ───────────── Embedding covers ───────────── */

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverJob {
    paths: Vec<String>,
    artist: String,
    album: String,
    /// The album shares its folder with other releases (see covers.rs)
    shared: bool,
    online: bool,
    /// An image chosen by the user (instead of the folder's / Deezer's)
    #[serde(default)]
    image: Option<String>,
    /// Replace the pictures the files already have
    #[serde(default)]
    replace: bool,
}

/// Image ready to embed: JPEG/PNG up to 1200 px are kept as they are, the rest is
/// re-encoded as a 1000 px JPEG (keeps files small).
fn embeddable(bytes: Vec<u8>) -> Option<(Vec<u8>, MimeType)> {
    let fmt = image::guess_format(&bytes).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let small = img.width().max(img.height()) <= 1200;
    match fmt {
        image::ImageFormat::Jpeg if small => return Some((bytes, MimeType::Jpeg)),
        image::ImageFormat::Png if small && bytes.len() < 1_500_000 => return Some((bytes, MimeType::Png)),
        _ => {}
    }
    let img = if small { img } else { img.resize(1000, 1000, image::imageops::FilterType::Lanczos3) };
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode_image(&img.to_rgb8()).ok()?;
    Some((out, MimeType::Jpeg))
}

/// Embeds a cover into the files that don't have one: the album folder's image or,
/// if there isn't one (and `online`), the one from Deezer.
#[tauri::command]
pub async fn embed_covers(app: AppHandle, jobs: Vec<CoverJob>) -> Result<TagResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (mut tracks, mut errors) = (Vec::new(), Vec::new());
        let total = jobs.len();
        for (n, job) in jobs.iter().enumerate() {
            let _ = app.emit("batch-progress", Progress { kind: "covers", done: n, total });
            let Some(first) = job.paths.first() else { continue };
            let bytes = match &job.image {
                Some(img) => fs::read(img).ok(),
                None => crate::covers::album_image(Path::new(first), job.shared.then_some(job.album.as_str()), job.online.then_some((job.artist.as_str(), job.album.as_str()))),
            };
            let Some((data, mime)) = bytes.and_then(embeddable) else {
                errors.push(format!("{}: no cover found", job.album));
                continue;
            };
            for path in &job.paths {
                let r = with_tag(path, |tag| {
                    if job.replace {
                        while !tag.pictures().is_empty() {
                            tag.remove_picture(0);
                        }
                    }
                    if tag.pictures().is_empty() {
                        tag.push_picture(Picture::new_unchecked(PictureType::CoverFront, Some(mime.clone()), None, data.clone()));
                    }
                    Ok(())
                });
                match r {
                    Ok(t) => tracks.push(t),
                    Err(e) => errors.push(format!("{}: {e}", file_name(path))),
                }
            }
        }
        let _ = app.emit("batch-progress", Progress { kind: "covers", done: total, total });
        update_cache(&tracks);
        TagResult { tracks, errors }
    })
    .await
    .map_err(|e| e.to_string())
}

/* ───────────── Library check helpers ───────────── */

/// For each file: does it have lyrics (a .lrc/.txt next to it or embedded in the tags)?
#[tauri::command]
pub async fn lyrics_status(paths: Vec<String>) -> Vec<bool> {
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .par_iter()
            .map(|p| {
                let path = Path::new(p);
                if ["lrc", "txt"].iter().any(|e| path.with_extension(e).is_file()) {
                    return true;
                }
                Probe::open(path)
                    .map(|p| p.options(ParseOptions::new().read_cover_art(false)))
                    .and_then(|p| p.read())
                    .ok()
                    .and_then(|f| f.primary_tag().or_else(|| f.first_tag()).and_then(|t| t.get_string(&ItemKey::Lyrics).map(|s| !s.trim().is_empty())))
                    .unwrap_or(false)
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Moves files to the Recycle Bin (duplicates). Their .lrc files go too.
#[tauri::command]
pub async fn trash_files(paths: Vec<String>) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut errors = Vec::new();
        for p in &paths {
            let path = Path::new(p);
            match trash::delete(path) {
                Ok(()) => {
                    for e in ["lrc", "txt"] {
                        let side = path.with_extension(e);
                        if side.is_file() {
                            let _ = trash::delete(&side);
                        }
                    }
                }
                Err(e) => errors.push(format!("{}: {e}", file_name(p))),
            }
        }
        let cache: Vec<Track> = read_cache().into_iter().filter(|t| !paths.contains(&t.path)).collect();
        save_cache(&cache);
        errors
    })
    .await
    .map_err(|e| e.to_string())
}

/* ───────────── Copying a playlist to a folder / USB stick ───────────── */

#[derive(Deserialize)]
pub struct CopyItem {
    from: String,
    to: String,
    /// Convert to MP3 at this bitrate (kbps) instead of copying
    #[serde(default)]
    mp3: Option<u32>,
}

/// Converts a song to MP3 (CBR, LAME), keeping its tags and cover.
pub(crate) fn encode_mp3(from: &str, to: &Path, kbps: u32) -> Result<(), String> {
    use mp3lame_encoder::{max_required_buffer_size, Bitrate, Builder, FlushGap, InterleavedPcm, Quality};
    let (frames, rate) = crate::audio::decode_for_mp3(from)?;
    let mut b = Builder::new().ok_or("Couldn't start the MP3 encoder")?;
    let brate = match kbps {
        320 => Bitrate::Kbps320,
        256 => Bitrate::Kbps256,
        128 => Bitrate::Kbps128,
        _ => Bitrate::Kbps192,
    };
    b.set_num_channels(2).map_err(|e| format!("{e:?}"))?;
    b.set_sample_rate(rate).map_err(|e| format!("{e:?}"))?;
    b.set_brate(brate).map_err(|e| format!("{e:?}"))?;
    b.set_quality(Quality::NearBest).map_err(|e| format!("{e:?}"))?;
    b.set_to_write_vbr_tag(false).map_err(|e| format!("{e:?}"))?;
    let mut enc = b.build().map_err(|e| format!("{e:?}"))?;
    let mut out: Vec<u8> = Vec::new();
    let mut pcm: Vec<f32> = Vec::with_capacity(8192);
    let push = |pcm: &mut Vec<f32>, out: &mut Vec<u8>, enc: &mut mp3lame_encoder::Encoder| -> Result<(), String> {
        out.reserve(max_required_buffer_size(pcm.len() / 2));
        enc.encode_to_vec(InterleavedPcm(pcm.as_slice()), out).map_err(|e| format!("{e:?}"))?;
        pcm.clear();
        Ok(())
    };
    for (l, r) in frames {
        pcm.push(l.clamp(-1.0, 1.0));
        pcm.push(r.clamp(-1.0, 1.0));
        if pcm.len() >= 8192 {
            push(&mut pcm, &mut out, &mut enc)?;
        }
    }
    if !pcm.is_empty() {
        push(&mut pcm, &mut out, &mut enc)?;
    }
    out.reserve(7200);
    enc.flush_to_vec::<FlushGap>(&mut out).map_err(|e| format!("{e:?}"))?;
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = to.with_extension("part.mp3");
    fs::write(&tmp, &out).map_err(|e| e.to_string())?;
    // Same tags and cover as the original
    if let Ok(src) = lofty::read_from_path(from) {
        if let Some(st) = src.primary_tag().or_else(|| src.first_tag()) {
            let mut tag = Tag::new(TagType::Id3v2);
            for item in st.items() {
                tag.insert(item.clone());
            }
            for pic in st.pictures() {
                tag.push_picture(pic.clone());
            }
            let _ = tag.save_to_path(&tmp, WriteOptions::default());
        }
    }
    fs::rename(&tmp, to).map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct CopyResult {
    copied: usize,
    skipped: usize,
    errors: Vec<String>,
}

/// Copies files (skipping the ones already there with the same size) and optionally
/// writes a playlist with relative paths next to them.
#[tauri::command]
pub async fn copy_files(app: AppHandle, items: Vec<CopyItem>, m3u: Option<(String, String)>) -> Result<CopyResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let total = items.len();
        let (copied, skipped, done) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
        let errors = Mutex::new(Vec::new());
        let one = |it: &CopyItem| {
            let (from, to) = (Path::new(&it.from), Path::new(&it.to));
            let r = match it.mp3 {
                // Converted: skipped if an MP3 newer than the original is already there
                Some(kbps) => {
                    let newer = fs::metadata(to).and_then(|m| m.modified()).ok().zip(fs::metadata(from).and_then(|m| m.modified()).ok()).is_some_and(|(a, b)| a >= b);
                    if newer {
                        Ok(false)
                    } else {
                        encode_mp3(&it.from, to, kbps).map(|_| true)
                    }
                }
                None => {
                    let same = fs::metadata(to).ok().zip(fs::metadata(from).ok()).is_some_and(|(a, b)| a.len() == b.len());
                    if same {
                        Ok(false)
                    } else {
                        to.parent().map_or(Ok(()), fs::create_dir_all).and_then(|_| fs::copy(from, to)).map(|_| true).map_err(|e| e.to_string())
                    }
                }
            };
            match r {
                Ok(true) => {
                    copied.fetch_add(1, Ordering::Relaxed);
                }
                Ok(false) => {
                    skipped.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => errors.lock().unwrap().push(format!("{}: {e}", file_name(&it.from))),
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            let _ = app.emit("batch-progress", Progress { kind: "copy", done: n, total });
        };
        // Converting uses the CPU: a few files at a time. Plain copies go one by one (disk-bound).
        if items.iter().any(|i| i.mp3.is_some()) {
            let threads = std::thread::available_parallelism().map(|n| n.get().saturating_sub(1).clamp(1, 4)).unwrap_or(2);
            match rayon::ThreadPoolBuilder::new().num_threads(threads).build() {
                Ok(pool) => pool.install(|| items.par_iter().for_each(one)),
                Err(_) => items.iter().for_each(one),
            }
        } else {
            items.iter().for_each(one);
        }
        let (copied, skipped) = (copied.into_inner(), skipped.into_inner());
        let mut errors = errors.into_inner().unwrap_or_default();
        if let Some((path, text)) = m3u {
            if let Err(e) = fs::write(&path, text) {
                errors.push(format!("playlist: {e}"));
            }
        }
        CopyResult { copied, skipped, errors }
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pick_dir(app: AppHandle, title: String) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_title(title).blocking_pick_folder().and_then(|p| p.into_path().ok()).map(|p| p.to_string_lossy().into_owned())
    })
    .await
    .ok()
    .flatten()
}

/* ───────────── File organizer ───────────── */

#[derive(Deserialize)]
pub struct Move {
    from: String,
    to: String,
}

#[derive(Serialize)]
pub struct OrganizeResult {
    moved: Vec<(String, String)>,
    errors: Vec<String>,
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir)?;
    }
    if to.exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "a file with that name already exists"));
    }
    if fs::rename(from, to).is_err() {
        // Another drive: copy and delete
        fs::copy(from, to)?;
        fs::remove_file(from)?;
    }
    Ok(())
}

fn dir_has_audio(dir: &Path) -> bool {
    fs::read_dir(dir).map(|rd| rd.flatten().any(|e| e.path().is_dir() || has_ext(&e.path(), AUDIO_EXT))).unwrap_or(true)
}

#[tauri::command]
pub async fn organize_files(moves: Vec<Move>, roots: Vec<String>) -> Result<OrganizeResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut moved = Vec::new();
        let mut errors = Vec::new();
        // source folder → destination folders (to take the covers along)
        let mut dirs: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
        for m in &moves {
            let (from, to) = (Path::new(&m.from), Path::new(&m.to));
            if from == to {
                continue;
            }
            match move_file(from, to) {
                Ok(()) => {
                    for ext in ["lrc", "txt"] {
                        let side = from.with_extension(ext);
                        if side.exists() {
                            let _ = move_file(&side, &to.with_extension(ext));
                        }
                    }
                    if let (Some(a), Some(b)) = (from.parent(), to.parent()) {
                        dirs.entry(a.to_path_buf()).or_default().insert(b.to_path_buf());
                    }
                    moved.push((m.from.clone(), m.to.clone()));
                }
                Err(e) => errors.push(format!("{}: {e}", from.file_name().unwrap_or_default().to_string_lossy())),
            }
        }
        let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
        for (src, dests) in dirs {
            if dir_has_audio(&src) {
                continue;
            }
            // The folder has no music left: move covers/images to the new album (if there's only one)
            if dests.len() == 1 {
                let dest = dests.iter().next().unwrap();
                if let Ok(rd) = fs::read_dir(&src) {
                    for e in rd.flatten() {
                        let p = e.path();
                        if has_ext(&p, IMAGE_EXT) {
                            let _ = move_file(&p, &dest.join(e.file_name()));
                        }
                    }
                }
            }
            // Delete empty folders, never going above the music folder
            let mut d = Some(src.as_path());
            while let Some(dir) = d {
                if roots.iter().any(|r| r == dir) || fs::remove_dir(dir).is_err() {
                    break;
                }
                d = dir.parent();
            }
        }
        rewrite_history(&moved.iter().cloned().collect());
        OrganizeResult { moved, errors }
    })
    .await
    .map_err(|e| e.to_string())
}

/* ───────────── M3U playlists ───────────── */

#[derive(Deserialize)]
pub struct M3uEntry {
    path: String,
    title: String,
    artist: String,
    duration: f64,
}

#[tauri::command]
pub async fn export_m3u(app: AppHandle, name: String, entries: Vec<M3uEntry>) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let safe: String = name.chars().map(|c| if r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
        let Some(file) = app.dialog().file().set_title("Export playlist").set_file_name(format!("{safe}.m3u8")).add_filter("Playlist M3U", &["m3u8", "m3u"]).blocking_save_file()
        else {
            return Ok(None);
        };
        let path = file.into_path().map_err(|e| e.to_string())?;
        let mut out = String::from("#EXTM3U\n");
        out.push_str(&format!("#PLAYLIST:{name}\n"));
        for e in &entries {
            out.push_str(&format!("#EXTINF:{},{} - {}\n{}\n", e.duration.round() as i64, e.artist, e.title, e.path));
        }
        fs::write(&path, out).map_err(|e| e.to_string())?;
        Ok(Some(path.to_string_lossy().into_owned()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub struct M3uImport {
    name: String,
    paths: Vec<String>,
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[tauri::command]
pub async fn import_m3u(app: AppHandle) -> Result<Option<M3uImport>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(file) = app.dialog().file().set_title("Import playlist").add_filter("Playlist M3U", &["m3u8", "m3u"]).blocking_pick_file() else {
            return Ok(None);
        };
        let path = file.into_path().map_err(|e| e.to_string())?;
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&bytes);
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Playlist".into());
        let mut paths = Vec::new();
        for line in text.lines() {
            let line = line.trim().trim_start_matches('\u{feff}');
            if let Some(n) = line.strip_prefix("#PLAYLIST:") {
                name = n.trim().to_string();
                continue;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let raw = if let Some(rest) = line.strip_prefix("file:///") { percent_decode(rest) } else { line.to_string() };
            let p = PathBuf::from(raw.replace('/', "\\"));
            let full = if p.is_absolute() { p } else { base.join(p) };
            let full = fs::canonicalize(&full).map(|c| c.to_string_lossy().trim_start_matches(r"\\?\").to_string()).unwrap_or_else(|_| full.to_string_lossy().into_owned());
            paths.push(full);
        }
        Ok(Some(M3uImport { name, paths }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/* ───────────── Listening history (for stats) ───────────── */

#[derive(Serialize, Deserialize)]
pub struct Listen {
    /// file path
    p: String,
    /// when (UNIX seconds)
    t: u64,
    /// seconds listened
    s: u32,
}

fn history_file() -> PathBuf {
    data_dir().join("history.jsonl")
}

#[tauri::command]
pub fn append_history(listen: Listen) -> Result<(), String> {
    let mut f = fs::OpenOptions::new().create(true).append(true).open(history_file()).map_err(|e| e.to_string())?;
    writeln!(f, "{}", serde_json::to_string(&listen).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// Adds many listens at once (history imported from Last.fm).
#[tauri::command]
pub fn append_history_bulk(listens: Vec<Listen>) -> Result<(), String> {
    let mut f = fs::OpenOptions::new().create(true).append(true).open(history_file()).map_err(|e| e.to_string())?;
    let mut out = String::new();
    for l in &listens {
        out.push_str(&serde_json::to_string(l).map_err(|e| e.to_string())?);
        out.push('\n');
    }
    f.write_all(out.as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn load_history() -> Vec<Listen> {
    tauri::async_runtime::spawn_blocking(|| fs::read_to_string(history_file()).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
        .await
        .unwrap_or_default()
}

fn rewrite_history(moves: &HashMap<String, String>) {
    if moves.is_empty() {
        return;
    }
    let Ok(text) = fs::read_to_string(history_file()) else { return };
    let mut out = String::with_capacity(text.len());
    for l in text.lines() {
        if let Ok(mut e) = serde_json::from_str::<Listen>(l) {
            if let Some(n) = moves.get(&e.p) {
                e.p = n.clone();
            }
            out.push_str(&serde_json::to_string(&e).unwrap_or_default());
            out.push('\n');
        }
    }
    let _ = fs::write(history_file(), out);
}

/* ───────────── New-music detection ───────────── */

#[derive(Default)]
pub struct FolderWatch(pub Mutex<Option<notify::RecommendedWatcher>>);

#[tauri::command]
pub fn watch_folders(app: AppHandle, state: State<FolderWatch>, folders: Vec<String>, enabled: bool) -> Result<(), String> {
    let mut slot = state.0.lock().map_err(|e| e.to_string())?;
    *slot = None; // drop the previous one (ends its thread)
    if !enabled || folders.is_empty() {
        return Ok(());
    }
    let (tx, rx) = channel::<()>();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if ev.paths.iter().any(|p| has_ext(p, AUDIO_EXT) || has_ext(p, &["cue"]) || p.extension().is_none()) {
                let _ = tx.send(());
            }
        }
    })
    .map_err(|e| e.to_string())?;
    for f in &folders {
        let _ = w.watch(Path::new(f), RecursiveMode::Recursive);
    }
    std::thread::spawn(move || {
        // Waits for 4 s of quiet (large copies) before notifying
        while rx.recv().is_ok() {
            loop {
                match rx.recv_timeout(Duration::from_secs(4)) {
                    Ok(()) => continue,
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            let _ = app.emit("library-changed", ());
        }
    });
    *slot = Some(w);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn converts_to_mp3() {
        let dir = crate::test_audio_dir();
        let out = std::env::temp_dir().join("wax_convert_test.mp3");
        let _ = fs::remove_file(&out);
        encode_mp3(&format!("{dir}/a.flac"), &out, 192).unwrap();
        let size = fs::metadata(&out).unwrap().len();
        let dec = crate::audio::SymSource::open(out.to_str().unwrap()).unwrap();
        let rate = rodio::Source::sample_rate(&dec) as f64;
        let frames = dec.count() as f64 / 2.0;
        let _ = fs::remove_file(&out);
        println!("mp3: {size} bytes, {:.3} s at {rate} Hz", frames / rate);
        assert!((frames / rate - 3.0).abs() < 0.06, "length {}", frames / rate);
        assert!(size > 60_000 && size < 90_000, "192 kbps × 3 s ≈ 72 KB, got {size}");
    }
}
