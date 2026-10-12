//! Album covers and artist photos.
//!
//! Album:  image chosen in Wax → embedded → image in the folder → (optional) Deezer.
//! Artist: image chosen in Wax → image in the artist's folder → (optional) Deezer.
//! Everything is cached as JPEG thumbnails in data/covers and data/artists.
//! Markers: `.none` = no local image, `.nonet` = not found online either.

use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use lofty::file::TaggedFileExt;
use lofty::picture::PictureType;
use serde::{Deserialize, Serialize};

use crate::{data_dir, has_ext, http, path_hash, IMAGE_EXT};

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

/// Writes a JPEG thumbnail. `fill` crops to a square (artist photos).
fn write_thumb(bytes: &[u8], out: &Path, color_file: &Path, size: u32, fill: bool) -> bool {
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
            if !color_file.exists() {
                let _ = fs::write(color_file, dominant_color(&rgb));
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

fn result(out: &Path, color_file: &Path) -> Cover {
    Cover {
        path: out.to_string_lossy().into_owned(),
        color: fs::read_to_string(color_file).unwrap_or_else(|_| "#535353".into()),
    }
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

fn cover_for(path: &Path, size: u32, online: Option<OnlineCover>, shared: Option<Shared>) -> Option<Cover> {
    let dir = data_dir().join("covers");
    let _ = fs::create_dir_all(&dir);
    let shared = shared.map(|s| s.album);
    // Separate cache key, so covers taken from a shared folder before are looked up again
    let name = format!("{}{}", path_hash(path), if shared.is_some() { "s" } else { "" });
    let none = dir.join(format!("{name}.none"));
    let nonet = dir.join(format!("{name}.nonet"));
    let out = dir.join(format!("{name}_{size}.jpg"));
    let color_file = dir.join(format!("{name}.color"));
    if out.exists() {
        return Some(result(&out, &color_file));
    }

    let mut bytes = if none.exists() { None } else { cover_source(path, shared.as_deref()) };
    if bytes.is_none() {
        let _ = fs::write(&none, b"");
        if let Some(o) = online.filter(|_| !nonet.exists()) {
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
                                let _ = fs::remove_file(&none);
                            }
                        }
                    }
                    bytes = Some(b);
                }
                None => {
                    let _ = fs::write(&nonet, b"");
                }
            }
        }
    }
    let bytes = bytes?;
    write_thumb(&bytes, &out, &color_file, size, false).then(|| result(&out, &color_file))
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

/// Thumbnail of an image the user chose in Wax (data/custom). Each choice is a new file,
/// so its thumbnails never get mixed up with older ones.
fn custom_thumb(file: &str, size: u32, dir: &str, fill: bool) -> Option<Cover> {
    let src = Path::new(file);
    if !src.is_file() {
        return None;
    }
    let cache = data_dir().join(dir);
    let _ = fs::create_dir_all(&cache);
    let key = format!("c{}", path_hash(src));
    let out = cache.join(format!("{key}_{size}.jpg"));
    let color_file = cache.join(format!("{key}.color"));
    if !out.exists() {
        let bytes = fs::read(src).ok()?;
        if !write_thumb(&bytes, &out, &color_file, size, fill) {
            return None;
        }
    }
    Some(result(&out, &color_file))
}

#[tauri::command]
pub async fn get_cover(path: String, size: u32, online: Option<OnlineCover>, shared: Option<Shared>, custom: Option<String>) -> Option<Cover> {
    let size = size.clamp(64, 1200);
    tauri::async_runtime::spawn_blocking(move || {
        custom.as_deref().and_then(|c| custom_thumb(c, size, "covers", false)).or_else(|| cover_for(Path::new(&path), size, online, shared))
    })
        .await
        .ok()
        .flatten()
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
pub async fn get_artist_image(name: String, dir: Option<String>, size: u32, online: bool, custom: Option<String>) -> Option<Cover> {
    let size = size.clamp(64, 1200);
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(c) = custom.as_deref().and_then(|c| custom_thumb(c, size, "artists", true)) {
            return Some(c);
        }
        let cache = data_dir().join("artists");
        let _ = fs::create_dir_all(&cache);
        let key = path_hash(Path::new(&format!("artist:{}", name.to_lowercase())));
        let out = cache.join(format!("{key}_{size}.jpg"));
        let color_file = cache.join(format!("{key}.color"));
        let none = cache.join(format!("{key}.none"));
        let nonet = cache.join(format!("{key}.nonet"));
        if out.exists() {
            return Some(result(&out, &color_file));
        }
        let mut bytes = None;
        if !none.exists() {
            bytes = dir.as_deref().and_then(|d| artist_folder_image(Path::new(d), &name));
            if bytes.is_none() {
                let _ = fs::write(&none, b"");
            }
        }
        if bytes.is_none() && online && !nonet.exists() {
            bytes = deezer_artist_picture(&name);
            if bytes.is_none() {
                let _ = fs::write(&nonet, b"");
            }
        }
        let bytes = bytes?;
        write_thumb(&bytes, &out, &color_file, size, true).then(|| result(&out, &color_file))
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

/// Saves an image chosen by the user (a Deezer URL or a file) into data/custom as a JPEG
/// (at most 1400 px) and returns its path. `old` (a previous choice) is deleted.
#[tauri::command]
pub async fn save_custom_image(source: String, name: String, old: Option<String>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = if source.starts_with("https://") {
            download(&source).ok_or("Couldn't download the image")?
        } else {
            fs::read(&source).map_err(|e| format!("Couldn't read the image: {e}"))?
        };
        let img = image::load_from_memory(&bytes).map_err(|_| "That file isn't an image Wax can read".to_string())?;
        let img = if img.width().max(img.height()) > 1400 { img.resize(1400, 1400, FilterType::Lanczos3) } else { img };
        let dir = data_dir().join("custom");
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).take(60).collect();
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let out = dir.join(format!("{safe}-{stamp}.jpg"));
        let f = fs::File::create(&out).map_err(|e| e.to_string())?;
        JpegEncoder::new_with_quality(BufWriter::new(f), 92).encode_image(&img.to_rgb8()).map_err(|e| e.to_string())?;
        if let Some(o) = old.filter(|o| Path::new(o).starts_with(&dir)) {
            let _ = fs::remove_file(o);
        }
        Ok(out.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
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

/// Deletes the "not found" markers so images are looked up again.
#[tauri::command]
pub fn clear_image_misses() {
    for sub in ["covers", "artists"] {
        if let Ok(rd) = fs::read_dir(data_dir().join(sub)) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "nonet" || x == "none") {
                    let _ = fs::remove_file(p);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
