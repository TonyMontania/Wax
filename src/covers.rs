//! Album covers and artist photos.
//!
//! Album:  image chosen in Wax → embedded → image in the folder → (optional) Deezer.
//! Artist: image chosen in Wax → image in the artist's folder → (optional) Deezer.
//! Everything is cached as JPEG thumbnails named after the album or artist
//! ("covers/Architects - Machine - Single.jpg", "artists/Currents.jpg"; the bigger size adds
//! " (large)"). One index.json per folder keeps each image's background color and whether it
//! wasn't found (locally / online). An image chosen by hand simply replaces the cached one.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use lofty::file::TaggedFileExt;
use lofty::picture::PictureType;
use serde::{Deserialize, Serialize};

use crate::{data_dir, file_name, has_ext, http, path_hash, IMAGE_EXT};

#[derive(Serialize)]
pub struct Cover {
    path: String,
    color: String,
}

fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn hsl_to_hex(h: f32, s: f32, l: f32) -> String {
    let f = |n: f32| {
        let k = (n + h * 12.0) % 12.0;
        let a = s * l.min(1.0 - l);
        let v = l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0);
        (v * 255.0).round().clamp(0.0, 255.0) as u8
    };
    format!("#{:02x}{:02x}{:02x}", f(0.0), f(8.0), f(4.0))
}

/// Dominant color (favoring saturated pixels), adjusted for backgrounds.
fn dominant_color(img: &image::RgbImage) -> String {
    let (mut r, mut g, mut b, mut w) = (0f32, 0f32, 0f32, 0f32);
    for p in img.pixels().step_by(3) {
        let (pr, pg, pb) = (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
        let (_, s, l) = rgb_to_hsl(pr, pg, pb);
        let weight = 0.05 + s * s * (1.0 - (l - 0.5).abs() * 2.0).max(0.0);
        r += pr * weight;
        g += pg * weight;
        b += pb * weight;
        w += weight;
    }
    if w == 0.0 {
        return "#535353".into();
    }
    let (h, s, _) = rgb_to_hsl(r / w, g / w, b / w);
    hsl_to_hex(h, if s < 0.08 { 0.0 } else { s.clamp(0.35, 0.75) }, 0.36)
}

/* ───────────── Cache index (colors and "not found" marks) ───────────── */

#[derive(Serialize, Deserialize, Default, Clone)]
struct Entry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    /// No image in the files or the folder
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    none: bool,
    /// Not found online either
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    nonet: bool,
    /// Chosen by hand: never replaced automatically
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    custom: bool,
}

type Index = HashMap<String, Entry>;
static INDEX: Mutex<Option<HashMap<&'static str, Index>>> = Mutex::new(None);

/// Loads data/<dir>/index.json once. Marker files of older versions (.color/.none/.nonet)
/// are moved into it and deleted.
fn load_index(dir: &str) -> Index {
    let folder = data_dir().join(dir);
    let mut idx: Index = fs::read_to_string(folder.join("index.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let mut migrated = false;
    if let Ok(rd) = fs::read_dir(&folder) {
        for e in rd.flatten() {
            let p = e.path();
            let (Some(ext), Some(key)) = (p.extension().and_then(|x| x.to_str()).map(str::to_string), p.file_stem().map(|x| x.to_string_lossy().into_owned())) else { continue };
            let entry = idx.entry(key).or_default();
            match ext.as_str() {
                "color" => entry.color = fs::read_to_string(&p).ok().map(|c| c.trim().to_string()),
                "none" => entry.none = true,
                "nonet" => entry.nonet = true,
                _ => continue,
            }
            let _ = fs::remove_file(&p);
            migrated = true;
        }
    }
    idx.retain(|_, e| e.color.is_some() || e.none || e.nonet || e.custom);
    if migrated {
        save_index(dir, &idx);
    }
    idx
}

fn save_index(dir: &str, idx: &Index) {
    let folder = data_dir().join(dir);
    let _ = fs::create_dir_all(&folder);
    let tmp = folder.join("index.json.tmp");
    if fs::write(&tmp, serde_json::to_string(idx).unwrap_or_default()).is_ok() {
        let _ = fs::rename(&tmp, folder.join("index.json"));
    }
}

/// Reads or changes the index of "covers" or "artists" (changes are saved right away).
fn with_index<R>(dir: &'static str, change: bool, f: impl FnOnce(&mut Index) -> R) -> R {
    let mut all = INDEX.lock().unwrap_or_else(|e| e.into_inner());
    let map = all.get_or_insert_with(HashMap::new);
    let idx = map.entry(dir).or_insert_with(|| load_index(dir));
    let r = f(idx);
    if change {
        save_index(dir, idx);
    }
    r
}
fn entry(dir: &'static str, key: &str) -> Entry {
    with_index(dir, false, |i| i.get(key).cloned().unwrap_or_default())
}
fn mark(dir: &'static str, key: &str, f: impl FnOnce(&mut Entry)) {
    with_index(dir, true, |i| f(i.entry(key.to_string()).or_default()));
}

/// After a rescan, albums without a local cover are checked again (new images may have been added).
pub fn forget_local_misses() {
    with_index("covers", true, |i| {
        for e in i.values_mut() {
            e.none = false;
        }
        i.retain(|_, e| e.color.is_some() || e.nonet);
    });
}

/// Writes a JPEG thumbnail and remembers its color. `fill` crops to a square (artist photos).
fn write_thumb(bytes: &[u8], out: &Path, dir: &'static str, key: &str, size: u32, fill: bool) -> bool {
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let img = if fill {
                img.resize_to_fill(size, size, FilterType::Triangle)
            } else if img.width() > size || img.height() > size {
                img.resize(size, size, FilterType::Triangle)
            } else {
                img
            };
            let rgb = img.to_rgb8();
            if entry(dir, key).color.is_none() {
                let c = dominant_color(&rgb);
                mark(dir, key, |e| e.color = Some(c));
            }
            let tmp = out.with_extension("tmp");
            let ok = fs::File::create(&tmp)
                .ok()
                .map(|f| JpegEncoder::new_with_quality(BufWriter::new(f), 86).encode_image(&rgb).is_ok())
                .unwrap_or(false);
            if !ok || fs::rename(&tmp, out).is_err() {
                let _ = fs::remove_file(&tmp);
                return false;
            }
            true
        }
        // A format we can't decode: save it as is and let the WebView try.
        Err(_) => fs::write(out, bytes).is_ok(),
    }
}

fn result(out: &Path, dir: &'static str, key: &str) -> Cover {
    Cover { path: out.to_string_lossy().into_owned(), color: entry(dir, key).color.unwrap_or_else(|| "#535353".into()) }
}

/* ───────────── Deezer (no API key) ───────────── */

#[derive(Deserialize)]
struct DzList<T> {
    data: Vec<T>,
}
#[derive(Deserialize)]
struct DzAlbum {
    title: String,
    cover_xl: Option<String>,
    artist: DzName,
}
#[derive(Deserialize)]
struct DzName {
    name: String,
}
#[derive(Deserialize)]
struct DzArtist {
    name: String,
    picture_xl: Option<String>,
}

fn simple(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

fn download(url: &str) -> Option<Vec<u8>> {
    let mut resp = http().get(url).call().ok()?;
    resp.body_mut().with_config().limit(15 * 1024 * 1024).read_to_vec().ok()
}

fn deezer_album_cover(artist: &str, album: &str) -> Option<Vec<u8>> {
    let (sa, sb) = (simple(artist), simple(album));
    // The advanced query sometimes returns nothing for albums that exist,
    // so a plain "artist album" search is tried next (results are still validated).
    let queries = [format!("artist:\"{artist}\" album:\"{album}\""), format!("{artist} {album}")];
    for q in &queries {
        let Some(list) = http()
            .get("https://api.deezer.com/search/album")
            .query("q", q)
            .query("limit", "15")
            .call()
            .ok()
            .and_then(|mut r| r.body_mut().read_json::<DzList<DzAlbum>>().ok())
        else {
            continue;
        };
        let best = list
            .data
            .iter()
            .find(|a| simple(&a.title) == sb && simple(&a.artist.name) == sa)
            .or_else(|| list.data.iter().find(|a| simple(&a.title).starts_with(&sb) && simple(&a.artist.name).contains(&sa)));
        if let Some(url) = best.and_then(|b| b.cover_xl.as_deref()).filter(|u| !u.contains("/cover//")) {
            return download(url);
        }
    }
    None
}

fn deezer_artist_picture(name: &str) -> Option<Vec<u8>> {
    let list: DzList<DzArtist> = http()
        .get("https://api.deezer.com/search/artist")
        .query("q", name)
        .query("limit", "5")
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    let s = simple(name);
    let best = list.data.iter().find(|a| simple(&a.name) == s)?;
    let url = best.picture_xl.as_deref().filter(|u| !u.contains("/artist//"))?;
    download(url)
}

/* ───────────── Album covers ───────────── */

/// Cover bytes: embedded in the file or an image in the album's folder.
/// `shared` = the folder holds several releases (e.g. a "Singles" folder with loose tracks):
/// then only an image named after the track or the album counts, never a generic cover.jpg.
fn cover_source(path: &Path, shared: Option<&str>) -> Option<Vec<u8>> {
    if let Ok(file) = lofty::read_from_path(path) {
        for tag in file.tags() {
            let pics = tag.pictures();
            if let Some(pic) = pics
                .iter()
                .find(|p| p.pic_type() == PictureType::CoverFront)
                .or_else(|| pics.first())
            {
                return Some(pic.data().to_vec());
            }
        }
    }
    let parent = path.parent()?;
    let mut imgs: Vec<PathBuf> = fs::read_dir(parent)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && has_ext(p, IMAGE_EXT))
        .collect();
    if let Some(album) = shared {
        let stem = path.file_stem().map(|s| simple(&s.to_string_lossy())).unwrap_or_default();
        let album = simple(album);
        return imgs
            .iter()
            .find(|p| {
                let n = p.file_stem().map(|s| simple(&s.to_string_lossy())).unwrap_or_default();
                n == stem || (!album.is_empty() && n == album)
            })
            .and_then(|p| fs::read(p).ok());
    }
    let score = |p: &PathBuf| {
        let n = p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        ["cover", "folder", "front", "album", "portada"]
            .iter()
            .position(|k| n.contains(k))
            .unwrap_or(99)
    };
    imgs.sort_by_key(score);
    fs::read(imgs.first()?).ok()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnlineCover {
    artist: String,
    album: String,
    save_to_folder: bool,
}

/// Album name of a release that shares its folder with others (see `cover_source`).
#[derive(Deserialize)]
pub struct Shared {
    album: String,
}

fn cover_for(path: &Path, size: u32, online: Option<OnlineCover>, shared: Option<Shared>, key: &str) -> Option<Cover> {
    let shared = shared.map(|s| s.album);
    let name = file_name(key);
    let out = thumb_path("covers", &name, size);
    if out.exists() {
        return Some(result(&out, "covers", &name));
    }
    let known = entry("covers", &name);
    let mut bytes = if known.none { None } else { cover_source(path, shared.as_deref()) };
    if bytes.is_none() {
        mark("covers", &name, |e| e.none = true);
        if let Some(o) = online.filter(|_| !known.nonet) {
            match deezer_album_cover(&o.artist, &o.album) {
                Some(b) => {
                    if o.save_to_folder {
                        if let Some(folder) = path.parent() {
                            // In a shared folder the image is named after the track
                            let target = match (&shared, path.file_stem()) {
                                (Some(_), Some(stem)) => folder.join(format!("{}.jpg", stem.to_string_lossy())),
                                _ => folder.join("cover.jpg"),
                            };
                            if !target.exists() {
                                let _ = fs::write(target, &b);
                                mark("covers", &name, |e| e.none = false);
                            }
                        }
                    }
                    bytes = Some(b);
                }
                None => mark("covers", &name, |e| e.nonet = true),
            }
        }
    }
    let bytes = bytes?;
    write_thumb(&bytes, &out, "covers", &name, size, false).then(|| result(&out, "covers", &name))
}

/// "Architects - Machine.jpg" (300 px), "Architects - Machine (large).jpg" (640 px)
fn thumb_path(dir: &str, key: &str, size: u32) -> PathBuf {
    let folder = data_dir().join(dir);
    let _ = fs::create_dir_all(&folder);
    match size {
        300 => folder.join(format!("{key}.jpg")),
        640 => folder.join(format!("{key} (large).jpg")),
        n => folder.join(format!("{key} ({n}).jpg")),
    }
}
/// File name → cache key ("X (large).jpg" → "X")
fn key_of(file: &str) -> String {
    let stem = file.strip_suffix(".jpg").unwrap_or(file);
    match stem.rfind(" (") {
        Some(i) if stem.ends_with(')') && (stem[i + 2..stem.len() - 1] == *"large" || stem[i + 2..stem.len() - 1].chars().all(|c| c.is_ascii_digit())) => stem[..i].to_string(),
        _ => stem.to_string(),
    }
}
/// Keys of older versions (hashes of the first track's path / the artist's name)
fn old_cover_key(path: &Path, shared: bool) -> String {
    format!("{}{}", path_hash(path), if shared { "s" } else { "" })
}
fn old_artist_key(name: &str) -> String {
    path_hash(Path::new(&format!("artist:{}", name.to_lowercase())))
}

/// Full-size image for an album (to embed it): embedded, the folder's, or Deezer's.
pub fn album_image(path: &Path, shared: Option<&str>, online: Option<(&str, &str)>) -> Option<Vec<u8>> {
    cover_source(path, shared).or_else(|| online.and_then(|(artist, album)| deezer_album_cover(artist, album)))
}

#[derive(Deserialize)]
pub struct CoverCheck {
    path: String,
    shared: Option<String>,
}

#[derive(Serialize)]
pub struct CoverStatus {
    /// The file has the cover inside
    embedded: bool,
    /// There's some cover (embedded or an image in the folder)
    any: bool,
}

/// For the library check: which albums have a cover, and which have it embedded.
#[tauri::command]
pub async fn covers_status(albums: Vec<CoverCheck>) -> Vec<CoverStatus> {
    use rayon::prelude::*;
    tauri::async_runtime::spawn_blocking(move || {
        albums
            .par_iter()
            .map(|a| {
                let p = Path::new(&a.path);
                let embedded = lofty::read_from_path(p).is_ok_and(|f| f.tags().iter().any(|t| !t.pictures().is_empty()));
                CoverStatus { embedded, any: embedded || cover_source(p, a.shared.as_deref()).is_some() }
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// `name`: "Album artist - Album", used to name the cached files.
#[tauri::command]
pub async fn get_cover(path: String, size: u32, online: Option<OnlineCover>, shared: Option<Shared>, name: String) -> Option<Cover> {
    let size = size.clamp(64, 1200);
    tauri::async_runtime::spawn_blocking(move || cover_for(Path::new(&path), size, online, shared, &name)).await.ok().flatten()
}

/* ───────────── Artist photos ───────────── */

fn artist_folder_image(dir: &Path, name: &str) -> Option<Vec<u8>> {
    let imgs: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && has_ext(p, IMAGE_EXT))
        .collect();
    let lname = name.to_lowercase();
    let score = |p: &PathBuf| {
        let n = p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        if n == lname {
            return 0;
        }
        ["artist", "artista", "band", "photo", "foto", "folder", "cover"]
            .iter()
            .position(|k| n.contains(k))
            .map(|i| i + 1)
            .unwrap_or(99)
    };
    let best = imgs.iter().min_by_key(|p| score(p))?;
    fs::read(best).ok()
}

#[tauri::command]
pub async fn get_artist_image(name: String, dir: Option<String>, size: u32, online: bool) -> Option<Cover> {
    let size = size.clamp(64, 1200);
    tauri::async_runtime::spawn_blocking(move || {
        let key = file_name(&name);
        let out = thumb_path("artists", &key, size);
        if out.exists() {
            return Some(result(&out, "artists", &key));
        }
        let known = entry("artists", &key);
        let mut bytes = None;
        if !known.none {
            bytes = dir.as_deref().and_then(|d| artist_folder_image(Path::new(d), &name));
            if bytes.is_none() {
                mark("artists", &key, |e| e.none = true);
            }
        }
        if bytes.is_none() && online && !known.nonet {
            bytes = deezer_artist_picture(&name);
            if bytes.is_none() {
                mark("artists", &key, |e| e.nonet = true);
            }
        }
        let bytes = bytes?;
        write_thumb(&bytes, &out, "artists", &key, size, true).then(|| result(&out, "artists", &key))
    })
    .await
    .ok()
    .flatten()
}

/* ───────────── Choosing images by hand ───────────── */

#[derive(Serialize)]
pub struct Candidate {
    title: String,
    sub: String,
    thumb: String,
    url: String,
}

#[derive(Deserialize)]
struct DzAlbumFull {
    title: String,
    cover_medium: Option<String>,
    cover_xl: Option<String>,
    artist: DzName,
    #[serde(default)]
    record_type: String,
}
#[derive(Deserialize)]
struct DzArtistFull {
    name: String,
    picture_medium: Option<String>,
    picture_xl: Option<String>,
    #[serde(default)]
    nb_fan: u64,
}

/// Images to choose from on Deezer: album covers (`kind = "album"`) or artist photos.
#[tauri::command]
pub async fn image_candidates(kind: String, artist: String, album: String) -> Result<Vec<Candidate>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let empty = |u: &Option<String>| u.as_deref().map_or(true, |u| u.contains("//images") || u.ends_with("//"));
        if kind == "album" {
            let mut out: Vec<Candidate> = Vec::new();
            for q in [format!("artist:\"{artist}\" album:\"{album}\""), format!("{artist} {album}"), album.clone()] {
                let list: DzList<DzAlbumFull> = http()
                    .get("https://api.deezer.com/search/album")
                    .query("q", &q)
                    .query("limit", "24")
                    .call()
                    .map_err(|e| format!("Deezer: {e}"))?
                    .body_mut()
                    .read_json()
                    .map_err(|e| e.to_string())?;
                for a in list.data.into_iter().filter(|a| !empty(&a.cover_xl)) {
                    let url = a.cover_xl.clone().unwrap_or_default();
                    if !out.iter().any(|c| c.url == url) {
                        out.push(Candidate { title: a.title, sub: format!("{} · {}", a.artist.name, a.record_type), thumb: a.cover_medium.unwrap_or_default(), url });
                    }
                }
                if out.len() >= 12 {
                    break;
                }
            }
            Ok(out)
        } else {
            let list: DzList<DzArtistFull> = http()
                .get("https://api.deezer.com/search/artist")
                .query("q", &artist)
                .query("limit", "12")
                .call()
                .map_err(|e| format!("Deezer: {e}"))?
                .body_mut()
                .read_json()
                .map_err(|e| e.to_string())?;
            Ok(list
                .data
                .into_iter()
                .filter(|a| !empty(&a.picture_xl) && !a.picture_xl.as_deref().unwrap_or("").contains("/artist//"))
                .map(|a| Candidate { title: a.name, sub: format!("{} fans", a.nb_fan), thumb: a.picture_medium.unwrap_or_default(), url: a.picture_xl.unwrap_or_default() })
                .collect())
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

fn load_image(source: &str) -> Result<image::DynamicImage, String> {
    let bytes = if source.starts_with("https://") {
        download(source).ok_or("Couldn't download the image")?
    } else {
        fs::read(source).map_err(|e| format!("Couldn't read the image: {e}"))?
    };
    image::load_from_memory(&bytes).map_err(|_| "That file isn't an image Wax can read".to_string())
}

fn save_jpeg(img: &image::DynamicImage, out: &Path, quality: u8) -> Result<(), String> {
    let tmp = out.with_extension("tmp");
    let f = fs::File::create(&tmp).map_err(|e| e.to_string())?;
    JpegEncoder::new_with_quality(BufWriter::new(f), quality).encode_image(&img.to_rgb8()).map_err(|e| e.to_string())?;
    fs::rename(&tmp, out).map_err(|e| e.to_string())
}

/// Puts the thumbnails of a chosen image in place of the cached ones (the old ones are replaced).
fn replace_thumbs(dir: &'static str, key: &str, img: &image::DynamicImage) -> Result<(), String> {
    let fill = dir == "artists";
    for size in [300u32, 640] {
        let t = if fill { img.resize_to_fill(size, size, FilterType::Triangle) } else if img.width().max(img.height()) > size { img.resize(size, size, FilterType::Triangle) } else { img.clone() };
        save_jpeg(&t, &thumb_path(dir, key, size), 86)?;
    }
    // other sizes from before go away
    if let Ok(rd) = fs::read_dir(data_dir().join(dir)) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if key_of(&n) == key && !n.ends_with(" (large).jpg") && n != format!("{key}.jpg") {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    let color = dominant_color(&img.resize(200, 200, FilterType::Triangle).to_rgb8());
    mark(dir, key, |e| {
        e.color = Some(color);
        e.custom = true;
        e.none = false;
        e.nonet = false;
    });
    Ok(())
}

/// An image chosen by hand (a Deezer URL or a file):
///   kind "album" / "artist" → replaces that album's / artist's image in the cache
///   kind "playlist"         → saved as playlists/<name>.jpg (`old` is deleted if different)
/// Returns a full-size copy (to embed it or put it in the music folder): a temporary file for
/// albums and artists, the saved file for playlists.
#[tauri::command]
pub async fn set_image(kind: String, name: String, source: String, old: Option<String>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let img = load_image(&source)?;
        let big = if img.width().max(img.height()) > 1400 { img.resize(1400, 1400, FilterType::Lanczos3) } else { img.clone() };
        let key = file_name(&name);
        if kind == "playlist" {
            let dir = data_dir().join("playlists");
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let out = dir.join(format!("{key}.jpg"));
            save_jpeg(&big, &out, 92)?;
            if let Some(o) = old.filter(|o| Path::new(o) != out && Path::new(o).starts_with(data_dir())) {
                let _ = fs::remove_file(o);
            }
            return Ok(out.to_string_lossy().into_owned());
        }
        replace_thumbs(if kind == "artist" { "artists" } else { "covers" }, &key, &img)?;
        let tmp = std::env::temp_dir().join("wax-chosen-image.jpg");
        save_jpeg(&big, &tmp, 92)?;
        Ok(tmp.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Back to the automatic image: the chosen one is deleted and looked up again.
#[tauri::command]
pub fn reset_image(kind: String, name: String) {
    let dir: &'static str = if kind == "artist" { "artists" } else { "covers" };
    let key = file_name(&name);
    if let Ok(rd) = fs::read_dir(data_dir().join(dir)) {
        for e in rd.flatten() {
            if key_of(&e.file_name().to_string_lossy()) == key {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    with_index(dir, true, |i| {
        i.remove(&key);
    });
}

/// Copies a chosen image next to the music (cover.jpg / artist.jpg). If a file with that name
/// exists it's kept as "name (old).jpg" instead of being overwritten.
#[tauri::command]
pub fn place_image(from: String, to: String) -> Result<(), String> {
    let dest = Path::new(&to);
    if dest.exists() {
        let stem = dest.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let old = dest.with_file_name(format!("{stem} (old).jpg"));
        let _ = fs::remove_file(&old);
        fs::rename(dest, &old).map_err(|e| e.to_string())?;
    }
    fs::copy(&from, dest).map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pick_image(app: tauri::AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Choose an image")
            .add_filter("Images", &["jpg", "jpeg", "png", "webp", "bmp", "gif"])
            .blocking_pick_file()
            .and_then(|p| p.into_path().ok())
            .map(|p| p.to_string_lossy().into_owned())
    })
    .await
    .ok()
    .flatten()
}

/// Forgets every "not found" mark so images are looked up again.
#[tauri::command]
pub fn clear_image_misses() {
    for dir in ["covers", "artists"] {
        with_index(dir, true, |i| {
            for e in i.values_mut() {
                e.none = false;
                e.nonet = false;
            }
            i.retain(|_, e| e.color.is_some() || e.custom);
        });
    }
}

#[derive(Serialize, Default)]
pub struct Cleaned {
    files: usize,
    bytes: u64,
}

/// Deletes cached covers and photos of albums and artists that aren't in the library anymore,
/// and playlist images nothing uses. Only Wax's cache is touched, never your music folders.
/// `albums`: "Album artist - Album" names, `artists`: artist names, `playlists`: image files in use.
#[tauri::command]
pub async fn clean_image_cache(albums: Vec<String>, artists: Vec<String>, playlists: Vec<String>) -> Cleaned {
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Cleaned::default();
        let keep_covers: HashSet<String> = albums.iter().map(|n| file_name(n)).collect();
        let keep_artists: HashSet<String> = artists.iter().map(|n| file_name(n)).collect();
        let mut sweep = |dir: &'static str, keep: &HashSet<String>| {
            with_index(dir, true, |i| i.retain(|k, _| keep.contains(k)));
            let Ok(rd) = fs::read_dir(data_dir().join(dir)) else { return };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("index.json") || keep.contains(&key_of(&name)) {
                    continue;
                }
                out.bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
                if fs::remove_file(e.path()).is_ok() {
                    out.files += 1;
                }
            }
        };
        sweep("covers", &keep_covers);
        sweep("artists", &keep_artists);
        let keep_pl: HashSet<PathBuf> = playlists.iter().map(PathBuf::from).collect();
        if let Ok(rd) = fs::read_dir(data_dir().join("playlists")) {
            for e in rd.flatten() {
                if !keep_pl.contains(&e.path()) {
                    out.bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
                    if fs::remove_file(e.path()).is_ok() {
                        out.files += 1;
                    }
                }
            }
        }
        out
    })
    .await
    .unwrap_or_default()
}

/* ───────────── Moving the cache of older versions to readable names ───────────── */

#[derive(Deserialize)]
pub struct OldAlbum {
    path: String,
    shared: bool,
    name: String,
}
#[derive(Deserialize)]
pub struct OldLyrics {
    path: String,
    name: String,
}
#[derive(Deserialize)]
pub struct OldCustom {
    /// "album", "artist" or "playlist"
    kind: String,
    name: String,
    file: String,
}

/// Renames the hash-named cache of older versions ("0aef35ad0e580b7b_300.jpg") to readable
/// names, moves images chosen by hand out of data/custom, and deletes what can't be named
/// (it's downloaded again when needed). Returns the new paths of playlist images (old → new).
#[tauri::command]
pub async fn migrate_cache_names(albums: Vec<OldAlbum>, artists: Vec<String>, lyrics: Vec<OldLyrics>, custom: Vec<OldCustom>) -> Vec<(String, String)> {
    tauri::async_runtime::spawn_blocking(move || {
        let is_hash = |s: &str| s.len() >= 16 && s.chars().take(16).all(|c| c.is_ascii_hexdigit());
        let mut moved = Vec::new();
        let rename = |dir: &'static str, old: &str, new: &str| {
            for (from, size) in [(format!("{old}_300.jpg"), 300u32), (format!("{old}_640.jpg"), 640)] {
                let src = data_dir().join(dir).join(&from);
                if !src.exists() {
                    continue;
                }
                let dst = thumb_path(dir, new, size);
                if dst.exists() || fs::rename(&src, &dst).is_err() {
                    let _ = fs::remove_file(&src);
                }
            }
            with_index(dir, true, |i| {
                if let Some(e) = i.remove(old) {
                    i.entry(new.to_string()).or_insert(e);
                }
            });
        };
        for a in &albums {
            rename("covers", &old_cover_key(Path::new(&a.path), a.shared), &file_name(&a.name));
            rename("covers", &old_cover_key(Path::new(&a.path), !a.shared), &file_name(&a.name));
        }
        for n in &artists {
            rename("artists", &old_artist_key(n), &file_name(n));
        }
        // Images chosen by hand (data/custom)
        for c in &custom {
            let src = Path::new(&c.file);
            if c.kind == "playlist" {
                let dir = data_dir().join("playlists");
                let _ = fs::create_dir_all(&dir);
                let dst = dir.join(format!("{}.jpg", file_name(&c.name)));
                if fs::rename(src, &dst).is_ok() || fs::copy(src, &dst).is_ok() {
                    moved.push((c.file.clone(), dst.to_string_lossy().into_owned()));
                }
            } else if let Ok(img) = fs::read(src).ok().and_then(|b| image::load_from_memory(&b).ok()).ok_or(()) {
                let _ = replace_thumbs(if c.kind == "artist" { "artists" } else { "covers" }, &file_name(&c.name), &img);
            }
            let _ = fs::remove_file(src);
        }
        let _ = fs::remove_dir(data_dir().join("custom"));
        // Whatever is still named by a hash can't be matched: it's simply fetched again
        for dir in ["covers", "artists"] {
            if let Ok(rd) = fs::read_dir(data_dir().join(dir)) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    let stem = n.trim_start_matches('c');
                    if n.ends_with(".jpg") && is_hash(stem) && stem.as_bytes().get(16).is_some_and(|b| *b == b'_' || *b == b's') {
                        let _ = fs::remove_file(e.path());
                    }
                }
            }
            with_index(dir, true, |i| i.retain(|k, _| !(is_hash(k) && k.len() <= 17)));
        }
        // Lyrics: data/lyrics/<hash>.json → "Artist - Title.json"
        let ldir = data_dir().join("lyrics");
        for l in &lyrics {
            let src = ldir.join(format!("{}.json", path_hash(Path::new(&l.path))));
            if src.exists() {
                let dst = ldir.join(format!("{}.json", file_name(&l.name)));
                if dst.exists() || fs::rename(&src, &dst).is_err() {
                    let _ = fs::remove_file(&src);
                }
            }
        }
        // Last.fm / Deezer / MusicBrainz / AutoEQ answers named by an MD5: dropped (asked again when needed)
        let md5ish = |p: &Path| p.file_stem().is_some_and(|s| s.len() == 32 && s.to_string_lossy().chars().all(|c| c.is_ascii_hexdigit()));
        let mut dirs = vec![data_dir().join("lastfm"), ldir];
        if let Ok(rd) = fs::read_dir(data_dir().join("cache")) {
            dirs.extend(rd.flatten().map(|e| e.path()));
        }
        for d in dirs {
            if let Ok(rd) = fs::read_dir(&d) {
                for e in rd.flatten() {
                    let p = e.path();
                    if md5ish(&p) || (p.file_stem().is_some_and(|s| s.len() == 16 && s.to_string_lossy().chars().all(|c| c.is_ascii_hexdigit()))) {
                        let _ = fs::remove_file(p);
                    }
                }
            }
        }
        moved
    })
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Old marker files move into index.json, old hash names become readable names, and
    /// cleaning keeps only what the library uses.
    #[test]
    fn cache_names_index_and_cleaning() {
        let dir = data_dir().join("covers");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (a, b) = (old_cover_key(Path::new("C:/m/A/a.mp3"), false), old_cover_key(Path::new("C:/m/B/b.mp3"), true));
        fs::write(dir.join(format!("{a}.color")), "#123456").unwrap();
        fs::write(dir.join(format!("{a}.none")), "").unwrap();
        fs::write(dir.join(format!("{a}_300.jpg")), "x").unwrap();
        fs::write(dir.join(format!("{a}_640.jpg")), "x").unwrap();
        fs::write(dir.join(format!("{b}_300.jpg")), "xx").unwrap();
        fs::write(dir.join(format!("{b}.nonet")), "").unwrap();
        let albums = vec![
            OldAlbum { path: "C:/m/A/a.mp3".into(), shared: false, name: "Artist A - Album: One?".into() },
            OldAlbum { path: "C:/m/B/b.mp3".into(), shared: true, name: "Artist B - Single".into() },
        ];
        tauri::async_runtime::block_on(migrate_cache_names(albums, vec![], vec![], vec![]));
        let mut left: Vec<String> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        println!("{left:?}");
        assert_eq!(left, vec!["Artist A - Album_ One_ (large).jpg", "Artist A - Album_ One_.jpg", "Artist B - Single.jpg", "index.json"]);
        assert_eq!(entry("covers", "Artist A - Album_ One_").color.as_deref(), Some("#123456"));
        assert!(entry("covers", "Artist B - Single").nonet);
        // Only album A is still in the library
        let r = tauri::async_runtime::block_on(clean_image_cache(vec!["Artist A - Album: One?".into()], vec![], vec![]));
        assert_eq!(r.files, 1);
        assert!(!dir.join("Artist B - Single.jpg").exists() && dir.join("Artist A - Album_ One_.jpg").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn shared_folder_covers() {
        let dir = std::env::temp_dir().join("wax_shared_cover_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (f, b) in [("Song A.mp3", &b"x"[..]), ("Song B.mp3", b"x"), ("cover.jpg", b"generic"), ("Song B.jpg", b"b-art"), ("Album C.png", b"c-art")] {
            fs::write(dir.join(f), b).unwrap();
        }
        let a = dir.join("Song A.mp3");
        assert_eq!(cover_source(&a, None).as_deref(), Some(&b"generic"[..]));    // album folder: cover.jpg
        assert_eq!(cover_source(&a, Some("Song A")), None);                      // shared: cover.jpg ignored
        assert_eq!(cover_source(&a, Some("Album C")).as_deref(), Some(&b"c-art"[..]));
        assert_eq!(cover_source(&dir.join("Song B.mp3"), Some("Song B")).as_deref(), Some(&b"b-art"[..]));
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn deezer_lookups() {
        if std::env::var("WAX_NET").is_err() {
            return;
        }
        let a = deezer_artist_picture("Radiohead").map(|b| b.len());
        let c = deezer_album_cover("Daft Punk", "Discovery").map(|b| b.len());
        let none = deezer_artist_picture("zzqx artist that does not exist 123").map(|b| b.len());
        println!("Radiohead photo: {a:?} bytes · Discovery cover: {c:?} bytes · missing artist: {none:?}");
        assert!(a.unwrap_or(0) > 5000 && c.unwrap_or(0) > 5000 && none.is_none());
    }
}
