//! CUE sheets: a whole album in one audio file (FLAC/WAV/…) split into virtual tracks.
//!
//! Each track gets a virtual path `<sheet.cue>#NN`, and `cue` points at the real file with the
//! start and end of the track inside it. The engine plays just that range (see audio.rs).

use std::fs;
use std::path::{Path, PathBuf};

use lofty::file::AudioFile;
use serde::{Deserialize, Serialize};

use crate::library::Track;
use crate::{has_ext, AUDIO_EXT};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CueRef {
    /// The real audio file
    pub file: String,
    pub start_ms: u64,
    /// 0 = until the end of the file
    pub end_ms: u64,
}

#[derive(Default, Debug)]
struct Sheet {
    title: String,
    performer: String,
    date: u32,
    genre: String,
    tracks: Vec<CueTrack>,
}

#[derive(Default, Debug)]
struct CueTrack {
    no: u32,
    file: String,
    title: String,
    performer: String,
    start_ms: Option<u64>,
}

/// CUE files are usually UTF-8 (with or without BOM) or the system's ANSI code page.
fn decode(bytes: &[u8]) -> String {
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| c as char).collect(), // Latin-1
    }
}

/// `"quoted text"` or a bare word, plus the rest of the line.
fn arg(s: &str) -> String {
    let s = s.trim();
    if let Some(r) = s.strip_prefix('"') {
        r.split('"').next().unwrap_or("").to_string()
    } else {
        s.to_string()
    }
}

/// mm:ss:ff (75 frames per second)
fn msf(s: &str) -> Option<u64> {
    let mut it = s.trim().split(':').map(|x| x.parse::<u64>());
    let (m, sec, f) = (it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?);
    Some((m * 60 + sec) * 1000 + f * 1000 / 75)
}

fn parse(text: &str) -> Sheet {
    let mut sh = Sheet::default();
    let mut file = String::new();
    for line in text.lines() {
        let l = line.trim();
        let (cmd, rest) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
        let in_track = !sh.tracks.is_empty();
        match cmd.to_ascii_uppercase().as_str() {
            "FILE" => {
                // FILE "name.flac" WAVE — the type comes after the closing quote
                file = if rest.trim_start().starts_with('"') {
                    arg(rest)
                } else {
                    rest.rsplit_once(' ').map(|(a, _)| a.trim().to_string()).unwrap_or_else(|| rest.trim().to_string())
                };
            }
            "TRACK" => {
                let mut p = rest.split_whitespace();
                let no = p.next().and_then(|n| n.parse().ok()).unwrap_or(sh.tracks.len() as u32 + 1);
                if p.next().is_some_and(|k| k.eq_ignore_ascii_case("AUDIO")) {
                    sh.tracks.push(CueTrack { no, file: file.clone(), ..Default::default() });
                } else {
                    // data tracks are ignored; keep a placeholder so its fields don't leak
                    sh.tracks.push(CueTrack { no: 0, ..Default::default() });
                }
            }
            "TITLE" if in_track => sh.tracks.last_mut().unwrap().title = arg(rest),
            "TITLE" => sh.title = arg(rest),
            "PERFORMER" if in_track => sh.tracks.last_mut().unwrap().performer = arg(rest),
            "PERFORMER" => sh.performer = arg(rest),
            "INDEX" if in_track => {
                let mut p = rest.split_whitespace();
                if p.next().and_then(|n| n.parse::<u32>().ok()) == Some(1) {
                    sh.tracks.last_mut().unwrap().start_ms = p.next().and_then(msf);
                }
            }
            "REM" => {
                let (k, v) = rest.trim().split_once(char::is_whitespace).unwrap_or((rest.trim(), ""));
                match k.to_ascii_uppercase().as_str() {
                    "DATE" => sh.date = v.trim().get(..4).and_then(|y| y.parse().ok()).unwrap_or(0),
                    "GENRE" => sh.genre = arg(v),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    sh.tracks.retain(|t| t.no > 0 && t.start_ms.is_some());
    sh
}

/// The audio file a sheet points at. Sheets often name the original .wav while the
/// folder holds the compressed copy, so the same name with another extension also counts.
fn resolve(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(name.replace('/', "\\"));
    if p.is_file() {
        return Some(p);
    }
    let stem = Path::new(name).file_stem()?.to_string_lossy().to_string();
    AUDIO_EXT.iter().map(|e| dir.join(format!("{stem}.{e}"))).find(|p| p.is_file())
}

fn duration_ms(p: &Path) -> u64 {
    lofty::probe::Probe::open(p)
        .and_then(|p| p.options(lofty::config::ParseOptions::new().read_cover_art(false)).read())
        .map(|f| f.properties().duration().as_millis() as u64)
        .unwrap_or(0)
}

/// Virtual tracks of a sheet, plus the audio files they cover (so the scan hides them).
pub fn read_sheet(cue: &Path, added: u64, size: u64) -> (Vec<Track>, Vec<PathBuf>) {
    let Ok(bytes) = fs::read(cue) else { return (Vec::new(), Vec::new()) };
    let sh = parse(&decode(&bytes));
    let dir = cue.parent().unwrap_or(Path::new("."));
    let mut out = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();
    let mut lengths: Vec<(PathBuf, u64)> = Vec::new();
    for (i, t) in sh.tracks.iter().enumerate() {
        let Some(file) = resolve(dir, &t.file) else { continue };
        if !has_ext(&file, AUDIO_EXT) {
            continue;
        }
        let start = t.start_ms.unwrap_or(0);
        // Ends where the next track of the same file starts
        let end = sh.tracks.get(i + 1).filter(|n| n.file == t.file).and_then(|n| n.start_ms).unwrap_or(0);
        let len = match lengths.iter().find(|(f, _)| *f == file) {
            Some((_, l)) => *l,
            None => {
                let l = duration_ms(&file);
                lengths.push((file.clone(), l));
                l
            }
        };
        let stop = if end > 0 { end } else { len };
        if !files.contains(&file) {
            files.push(file.clone());
        }
        let performer = if t.performer.is_empty() { sh.performer.clone() } else { t.performer.clone() };
        out.push(Track {
            path: format!("{}#{:02}", cue.to_string_lossy(), t.no),
            title: if t.title.is_empty() { format!("Track {}", t.no) } else { t.title.clone() },
            artist: if performer.is_empty() { "Unknown Artist".into() } else { performer },
            album: if sh.title.is_empty() { cue.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default() } else { sh.title.clone() },
            album_artist: sh.performer.clone(),
            artists: Vec::new(),
            track_no: t.no,
            disc_no: 0,
            year: sh.date,
            duration: stop.saturating_sub(start) as f64 / 1000.0,
            added,
            genres: if sh.genre.is_empty() { Vec::new() } else { vec![sh.genre.clone()] },
            rg: None,
            size,
            cue: Some(CueRef { file: file.to_string_lossy().into_owned(), start_ms: start, end_ms: end }),
        });
    }
    (out, files)
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A real sheet next to a real file: two tracks, the file itself hidden by the scan.
    #[test]
    fn sheet_with_audio() {
        let src = crate::test_audio_dir();
        let dir = std::env::temp_dir().join("wax_cue_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::copy(Path::new(&src).join("a.flac"), dir.join("Album.flac")).unwrap();
        // The sheet names a .wav, as rippers usually do
        fs::write(dir.join("Album.cue"), "PERFORMER \"Band\"\nTITLE \"Album\"\nFILE \"Album.wav\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"One\"\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    TITLE \"Two\"\n    INDEX 01 00:01:30\n").unwrap();
        let (tracks, files) = read_sheet(&dir.join("Album.cue"), 1, 2);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(files.len(), 1);
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].cue.as_ref().unwrap().end_ms, 1400);
        assert_eq!(tracks[1].cue.as_ref().unwrap().start_ms, 1400);
        assert!((tracks[0].duration + tracks[1].duration - 3.0).abs() < 0.05, "{} + {}", tracks[0].duration, tracks[1].duration);
        assert!(tracks[1].path.ends_with("Album.cue#02"));
        assert_eq!(tracks[1].album, "Album");
    }

    #[test]
    fn parses_sheet() {
        let text = "\u{feff}REM GENRE \"Trip Hop\"\nREM DATE 1998\nPERFORMER \"Massive Attack\"\nTITLE \"Mezzanine\"\nFILE \"Mezzanine.wav\" WAVE\n  TRACK 01 AUDIO\n    TITLE \"Angel\"\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    TITLE \"Risingson\"\n    PERFORMER \"Massive Attack feat. X\"\n    INDEX 00 06:16:50\n    INDEX 01 06:18:37\n";
        let sh = parse(&decode(text.as_bytes()));
        assert_eq!(sh.title, "Mezzanine");
        assert_eq!(sh.performer, "Massive Attack");
        assert_eq!(sh.date, 1998);
        assert_eq!(sh.genre, "Trip Hop");
        assert_eq!(sh.tracks.len(), 2);
        assert_eq!(sh.tracks[0].file, "Mezzanine.wav");
        assert_eq!(sh.tracks[1].start_ms, Some(378_493)); // 6:18 + 37/75 s
        assert_eq!(sh.tracks[1].performer, "Massive Attack feat. X");
        assert_eq!(sh.tracks[0].title, "Angel");
    }
}
