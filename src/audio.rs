//! Wax audio engine.
//!
//! - Gapless playback: the next track is opened ahead of time and chained into the
//!   same audio stream as soon as the current one ends.
//! - Crossfade, configurable per track.
//! - 10-band equalizer (RBJ peaking filters) and per-track gain (ReplayGain).
//!
//! Threads:
//!   control   → receives commands from the frontend, opens files and hands them,
//!               ready to play, to the mixer.
//!   decode    → one per open track: reads the file, decodes and resamples it (high quality)
//!               into a ~3 s buffer. The audio thread never touches the disk or the decoder,
//!               so a busy disk or a heavy file can't make the sound stutter.
//!   audio     → keeps the OutputStream alive; the mixer runs inside rodio.
//!   events    → forwards mixer events to the frontend and publishes the position.

use std::f32::consts::{FRAC_PI_2, PI};
use std::fs::File;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{channel, sync_channel, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::{OutputStream, Source};
use rubato::{FftFixedIn, Resampler};
use serde::{Deserialize, Serialize};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder as SymDecoder, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};

pub const EQ_FREQS: [f32; 10] = [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];

#[derive(Clone, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TrackSpec {
    pub id: u64,
    pub path: String,
    #[serde(default)]
    pub gain_db: f32,
    #[serde(default)]
    pub duration_ms: u64,
    /// Crossfade length with the previous track (only used for the "next" track).
    #[serde(default)]
    pub crossfade_ms: u64,
    /// CUE sheet tracks: where the track starts and ends inside the file (0 = whole file).
    #[serde(default)]
    pub start_ms: u64,
    #[serde(default)]
    pub end_ms: u64,
}

#[derive(Clone, Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct EqSettings {
    /// The 10-band graphic equalizer is on
    pub enabled: bool,
    #[serde(default)]
    pub preamp: f32,
    #[serde(default)]
    pub bands: Vec<f32>,
    /// Parametric filters (AutoEQ headphone correction), applied even with the graphic EQ off
    #[serde(default)]
    pub filters: Vec<PeqFilter>,
    #[serde(default)]
    pub filter_preamp: f32,
}

#[derive(Clone, Deserialize, Debug)]
pub struct PeqFilter {
    /// "PK" (peak), "LSC" (low shelf) or "HSC" (high shelf)
    pub kind: String,
    pub fc: f32,
    pub gain: f32,
    pub q: f32,
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AudioEvent {
    Started {
        id: u64,
    },
    /// The engine moved on to the next track by itself (gapless or crossfaded).
    Advanced {
        id: u64,
    },
    /// The track ended and there was no next one.
    Ended {
        id: u64,
    },
    Error {
        id: u64,
        message: String,
    },
    Time {
        id: u64,
        pos: u64,
        playing: bool,
    },
}

struct Voice {
    id: u64,
    src: Buffered,
    gain: f32,
    frames: u64,
    total: u64,
    fade_in: u64,
}

impl Voice {
    #[inline]
    fn pull(&mut self) -> Option<(f32, f32)> {
        self.src.frame()
    }
}

/* ───────────── Decode thread + buffer ─────────────
The decode thread sends stereo chunks at the output rate; the audio thread only copies
samples out of them. If the buffer ever runs dry (it shouldn't: it holds ~3 s), the
audio thread plays silence instead of waiting. */

const CHUNK_FRAMES: usize = 1024;
const BUFFER_CHUNKS: usize = 128; // ≈ 3 s at 44.1/48 kHz

struct Buffered {
    rx: Receiver<Vec<f32>>,
    buf: Vec<f32>,
    pos: usize,
}

impl Buffered {
    fn spawn(dec: SymSource, out_rate: u32) -> Buffered {
        let (tx, rx) = sync_channel::<Vec<f32>>(BUFFER_CHUNKS);
        let _ = std::thread::Builder::new().name("wax-decode".into()).spawn(move || produce(dec, out_rate, &tx));
        Buffered { rx, buf: Vec::new(), pos: 0 }
    }

    /// Waits (off the audio thread) until the first audio is ready, so playback starts cleanly.
    fn prime(&mut self) {
        if self.pos >= self.buf.len() {
            if let Ok(b) = self.rx.recv_timeout(Duration::from_secs(3)) {
                self.buf = b;
                self.pos = 0;
            }
        }
    }

    /// Next stereo frame: None at the end of the track.
    #[inline]
    fn frame(&mut self) -> Option<(f32, f32)> {
        self.frame_with(cfg!(test))
    }

    /// `wait`: block until the decoder catches up (converting files, tests) instead of
    /// playing silence (real time).
    #[inline]
    fn frame_with(&mut self, wait: bool) -> Option<(f32, f32)> {
        if self.pos + 1 >= self.buf.len() {
            let next = if wait { self.rx.recv().map_err(|_| TryRecvError::Disconnected) } else { self.rx.try_recv() };
            match next {
                Ok(b) => {
                    self.buf = b;
                    self.pos = 0;
                }
                Err(TryRecvError::Empty) => return Some((0.0, 0.0)), // buffer ran dry: silence, don't block
                Err(TryRecvError::Disconnected) => return None,
            }
            if self.buf.len() < 2 {
                return Some((0.0, 0.0));
            }
        }
        let f = (self.buf[self.pos], self.buf[self.pos + 1]);
        self.pos += 2;
        Some(f)
    }
}

/// A whole file decoded to stereo at `rate` (for converting, not for playback).
pub struct Decoded(Buffered);

impl Iterator for Decoded {
    type Item = (f32, f32);
    fn next(&mut self) -> Option<(f32, f32)> {
        self.0.frame_with(true)
    }
}

/// Opens a file for converting. Rates the MP3 format doesn't have (88.2/96/192 kHz…) become 44.1 kHz.
pub fn decode_for_mp3(path: &str) -> Result<(Decoded, u32), String> {
    let dec = SymSource::open(path)?;
    let rate = match dec.sample_rate() {
        r @ (32000 | 44100 | 48000) => r,
        _ => 44100,
    };
    Ok((Decoded(Buffered::spawn(dec, rate)), rate))
}

/// Decoder → stereo → output rate, in chunks. Ends when the track ends or nobody listens.
fn produce(mut dec: SymSource, out_rate: u32, tx: &SyncSender<Vec<f32>>) {
    let in_rate = dec.sample_rate();
    let ch = dec.channels().max(1) as usize;
    // One stereo frame from the decoder (mono is doubled, extra channels are dropped)
    let frame = move |d: &mut SymSource| -> Option<(f32, f32)> {
        let l = d.next()?;
        let r = if ch >= 2 { d.next().unwrap_or(l) } else { l };
        for _ in 2..ch {
            d.next();
        }
        Some((l, r))
    };

    if in_rate == out_rate {
        loop {
            let mut chunk = Vec::with_capacity(CHUNK_FRAMES * 2);
            let mut end = false;
            for _ in 0..CHUNK_FRAMES {
                match frame(&mut dec) {
                    Some((l, r)) => {
                        chunk.push(l);
                        chunk.push(r);
                    }
                    None => {
                        end = true;
                        break;
                    }
                }
            }
            if !chunk.is_empty() && tx.send(chunk).is_err() {
                return;
            }
            if end {
                return;
            }
        }
    }

    // Different rates: FFT resampler (band-limited, no aliasing). Its delay is trimmed and the
    // output is cut to the exact length, so gapless chaining stays sample-accurate.
    let Ok(mut rs) = FftFixedIn::<f32>::new(in_rate as usize, out_rate as usize, CHUNK_FRAMES, 2, 2) else { return };
    let mut skip = rs.output_delay();
    let (mut in_frames, mut out_frames, mut eof) = (0u64, 0u64, false);
    let mut input = vec![Vec::with_capacity(CHUNK_FRAMES), Vec::with_capacity(CHUNK_FRAMES)];
    loop {
        let need = rs.input_frames_next();
        input[0].clear();
        input[1].clear();
        for _ in 0..need {
            let (l, r) = if eof { None } else { frame(&mut dec) }.unwrap_or_else(|| {
                eof = true;
                (0.0, 0.0)
            });
            if !eof {
                in_frames += 1;
            }
            input[0].push(l);
            input[1].push(r);
        }
        let Ok(out) = rs.process(&input, None) else { return };
        let expected = eof.then(|| (in_frames * out_rate as u64 + in_rate as u64 / 2) / in_rate as u64);
        let mut chunk = Vec::with_capacity(out[0].len() * 2);
        for (&l, &r) in out[0].iter().zip(&out[1]) {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if expected.is_some_and(|e| out_frames >= e) {
                break;
            }
            chunk.push(l);
            chunk.push(r);
            out_frames += 1;
        }
        if !chunk.is_empty() && tx.send(chunk).is_err() {
            return;
        }
        if expected.is_some_and(|e| out_frames >= e) {
            return;
        }
    }
}

/// Peaks above 0.9 are rounded off instead of hard-clipped (no harsh distortion when
/// ReplayGain or the equalizer push a loud song over the top).
#[inline]
fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= 0.9 {
        x
    } else {
        x.signum() * (0.9 + 0.1 * ((a - 0.9) / 0.1).tanh())
    }
}

/* ───────────── Decoder (symphonia) ─────────────
`enable_gapless` trims the silence MP3/AAC encoders add at the start and the end:
essential so live albums play without gaps. */

pub struct SymSource {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn SymDecoder>,
    track: u32,
    sb: Option<SampleBuffer<f32>>,
    buf: Vec<f32>,
    pos: usize,
    channels: u16,
    rate: u32,
    dur: Option<Duration>,
    tb: Option<TimeBase>,
    /// Frames still to drop after an accurate seek (the packet starts before the target).
    skip: u64,
    /// Samples left until the end of a CUE track (None = until the end of the file).
    left: Option<u64>,
}

impl SymSource {
    pub fn open(path: &str) -> Result<SymSource, String> {
        let file = File::open(path).map_err(|e| format!("Couldn't open the file: {e}"))?;
        // A song still arriving from a Listen Together host: reading waits for the rest
        let source: Box<dyn symphonia::core::io::MediaSource> = match crate::party::growing(path) {
            Some(dl) => Box::new(crate::party::GrowingFile::new(file, dl)),
            None => Box::new(file),
        };
        let mss = MediaSourceStream::new(source, Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = std::path::Path::new(path).extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let opts = FormatOptions { enable_gapless: true, ..Default::default() };
        let probed = symphonia::default::get_probe().format(&hint, mss, &opts, &MetadataOptions::default()).map_err(|e| format!("Unsupported format: {e}"))?;
        let format = probed.format;
        let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).ok_or("The file has no audio")?;
        let track_id = track.id;
        let params = track.codec_params.clone();
        let decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).map_err(|e| format!("Unsupported codec: {e}"))?;
        let dur = match (params.time_base, params.n_frames) {
            (Some(tb), Some(n)) => {
                let t = tb.calc_time(n);
                Some(Duration::from_secs_f64(t.seconds as f64 + t.frac))
            }
            _ => None,
        };
        let mut s = SymSource {
            format,
            decoder,
            track: track_id,
            sb: None,
            buf: Vec::new(),
            pos: 0,
            channels: params.channels.map(|c| c.count() as u16).unwrap_or(2),
            rate: params.sample_rate.unwrap_or(44100),
            dur,
            tb: params.time_base,
            skip: 0,
            left: None,
        };
        if !s.refill() {
            return Err("Couldn't decode the audio".into());
        }
        Ok(s)
    }

    pub fn seek(&mut self, ms: u64) -> bool {
        let to = SeekTo::Time { time: Time::from(ms as f64 / 1000.0), track_id: Some(self.track) };
        let Ok(seeked) = self.format.seek(SeekMode::Accurate, to) else { return false };
        self.decoder.reset();
        self.buf.clear();
        self.pos = 0;
        // The reader lands on the packet that contains the target: drop the frames before it
        // (sample-accurate, needed so CUE tracks chain without clicks or overlaps)
        let diff = seeked.required_ts.saturating_sub(seeked.actual_ts);
        self.skip = match self.tb {
            Some(tb) if tb.numer == 1 && tb.denom == self.rate => diff,
            Some(tb) => {
                let t = tb.calc_time(diff);
                ((t.seconds as f64 + t.frac) * self.rate as f64).round() as u64
            }
            None => diff,
        };
        self.refill()
    }

    /// Stops the source after `ms` milliseconds (end of a CUE track).
    pub fn limit(&mut self, ms: u64) {
        self.left = Some(ms * self.rate as u64 / 1000 * self.channels.max(1) as u64);
    }

    fn refill(&mut self) -> bool {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(SymError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(_) => return false,
            };
            if packet.track_id() != self.track {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(d) => {
                    if d.frames() == 0 {
                        continue;
                    }
                    let spec = *d.spec();
                    let need = d.capacity() * spec.channels.count();
                    if self.sb.as_ref().is_none_or(|b| b.capacity() < need) {
                        self.sb = Some(SampleBuffer::new(d.capacity() as u64, spec));
                    }
                    let sb = self.sb.as_mut().unwrap();
                    sb.copy_interleaved_ref(d);
                    self.buf.clear();
                    self.channels = spec.channels.count() as u16;
                    self.rate = spec.rate;
                    let ch = self.channels.max(1) as usize;
                    let drop = (self.skip as usize).min(sb.samples().len() / ch);
                    self.skip -= drop as u64;
                    self.buf.extend_from_slice(&sb.samples()[drop * ch..]);
                    self.pos = 0;
                    if self.buf.is_empty() {
                        continue;
                    }
                    return true;
                }
                Err(SymError::DecodeError(_)) | Err(SymError::IoError(_)) => continue,
                Err(_) => return false,
            }
        }
    }
}

impl Iterator for SymSource {
    type Item = f32;
    #[inline]
    fn next(&mut self) -> Option<f32> {
        if let Some(l) = self.left.as_mut() {
            if *l == 0 {
                return None;
            }
            *l -= 1;
        }
        if self.pos >= self.buf.len() && !self.refill() {
            return None;
        }
        let s = self.buf[self.pos];
        self.pos += 1;
        Some(s)
    }
}

impl Source for SymSource {
    fn current_frame_len(&self) -> Option<usize> {
        let n = self.buf.len().saturating_sub(self.pos);
        let n = self.left.map_or(n, |l| n.min(l as usize));
        Some(n.max(1))
    }
    fn channels(&self) -> u16 {
        self.channels.max(1)
    }
    fn sample_rate(&self) -> u32 {
        self.rate.max(1)
    }
    fn total_duration(&self) -> Option<Duration> {
        self.dur
    }
}

/* ───────────── Equalizer ───────────── */

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z: [[f32; 2]; 2], // [channel][z1, z2]
}

impl Biquad {
    fn peaking(f: f32, gain_db: f32, q: f32, fs: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * f / fs;
        let alpha = w0.sin() / (2.0 * q);
        let cos = w0.cos();
        let a0 = 1.0 + alpha / a;
        Biquad { b0: (1.0 + alpha * a) / a0, b1: (-2.0 * cos) / a0, b2: (1.0 - alpha * a) / a0, a1: (-2.0 * cos) / a0, a2: (1.0 - alpha / a) / a0, z: [[0.0; 2]; 2] }
    }
    /// RBJ shelving filters (`high` = high shelf), as AutoEQ defines them.
    fn shelf(high: bool, f: f32, gain_db: f32, q: f32, fs: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * f / fs;
        let (cos, alpha) = (w0.cos(), w0.sin() / (2.0 * q));
        let sa = 2.0 * a.sqrt() * alpha;
        let (b0, b1, b2, a0, a1, a2) = if high {
            (
                a * ((a + 1.0) + (a - 1.0) * cos + sa),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                a * ((a + 1.0) + (a - 1.0) * cos - sa),
                (a + 1.0) - (a - 1.0) * cos + sa,
                2.0 * ((a - 1.0) - (a + 1.0) * cos),
                (a + 1.0) - (a - 1.0) * cos - sa,
            )
        } else {
            (
                a * ((a + 1.0) - (a - 1.0) * cos + sa),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                a * ((a + 1.0) - (a - 1.0) * cos - sa),
                (a + 1.0) + (a - 1.0) * cos + sa,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                (a + 1.0) + (a - 1.0) * cos - sa,
            )
        };
        Biquad { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, z: [[0.0; 2]; 2] }
    }
    #[inline]
    fn run(&mut self, ch: usize, x: f32) -> f32 {
        let z = &mut self.z[ch];
        let y = self.b0 * x + z[0];
        z[0] = self.b1 * x - self.a1 * y + z[1];
        z[1] = self.b2 * x - self.a2 * y;
        y
    }
}

struct Eq {
    pre: f32,
    filters: Vec<Biquad>,
}

impl Eq {
    fn new(s: &EqSettings, rate: u32) -> Option<Eq> {
        if !s.enabled && s.filters.is_empty() {
            return None;
        }
        let fs = rate as f32;
        let mut filters: Vec<Biquad> = Vec::new();
        let mut pre = 0.0;
        if s.enabled {
            pre += s.preamp.clamp(-15.0, 15.0);
            filters.extend(EQ_FREQS.iter().zip(s.bands.iter()).filter(|(f, g)| g.abs() > 0.05 && **f < fs * 0.45).map(|(f, g)| Biquad::peaking(*f, g.clamp(-15.0, 15.0), 1.1, fs)));
        }
        if !s.filters.is_empty() {
            pre += s.filter_preamp.clamp(-30.0, 10.0);
            for f in s.filters.iter().filter(|f| f.fc > 0.0 && f.fc < fs * 0.48 && f.q > 0.0) {
                let g = f.gain.clamp(-24.0, 24.0);
                filters.push(match f.kind.as_str() {
                    "LSC" | "LS" => Biquad::shelf(false, f.fc, g, f.q, fs),
                    "HSC" | "HS" => Biquad::shelf(true, f.fc, g, f.q, fs),
                    _ => Biquad::peaking(f.fc, g, f.q, fs),
                });
            }
        }
        Some(Eq { pre: db_to_lin(pre), filters })
    }
    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let (mut l, mut r) = (l * self.pre, r * self.pre);
        for f in &mut self.filters {
            l = f.run(0, l);
            r = f.run(1, r);
        }
        (l, r)
    }
}

pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/* ───────────── Crossfeed (headphones) ─────────────
Each ear also gets a little of the other channel, low-passed and delayed ~0.3 ms, the way
it happens with speakers. Makes hard-panned old stereo recordings less tiring. */

struct Crossfeed {
    a: f32,
    lp: [f32; 2],
    delay: Vec<[f32; 2]>,
    di: usize,
    cross: f32,
    norm: f32,
}

impl Crossfeed {
    /// level: 1 light, 2 medium, 3 strong (0 = off)
    fn new(level: u8, rate: u32) -> Option<Crossfeed> {
        let (fc, feed_db) = match level {
            0 => return None,
            1 => (650.0, -9.5),
            2 => (700.0, -6.0),
            _ => (700.0, -4.5),
        };
        let cross = db_to_lin(feed_db);
        Some(Crossfeed {
            a: 1.0 - (-2.0 * PI * fc / rate as f32).exp(),
            lp: [0.0; 2],
            delay: vec![[0.0; 2]; ((0.0003 * rate as f32).round() as usize).max(1)],
            di: 0,
            cross,
            norm: 1.0 / (1.0 + cross),
        })
    }
    #[inline]
    fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        self.lp[0] += self.a * (l - self.lp[0]);
        self.lp[1] += self.a * (r - self.lp[1]);
        let d = self.delay[self.di];
        self.delay[self.di] = self.lp;
        self.di = (self.di + 1) % self.delay.len();
        ((l + self.cross * d[1]) * self.norm, (r + self.cross * d[0]) * self.norm)
    }
}

/* ───────────── Mixer (runs on the audio thread) ───────────── */

enum Mix {
    Play(Voice, bool),
    Replace(Voice),
    Next(Option<Voice>),
    Pause(bool),
    Volume(f32),
    Eq(Option<Eq>),
    Crossfeed(Option<Crossfeed>),
    Stop,
}

pub struct Shared {
    pub id: AtomicU64,
    pub pos: AtomicU64, // in frames
    pub playing: AtomicBool,
    pub rate: AtomicU64,
}

struct Mixer {
    rx: Receiver<Mix>,
    ev: Sender<AudioEvent>,
    shared: Arc<Shared>,
    rate: u32,
    cur: Option<Voice>,
    next: Option<Voice>,
    out: Option<(Voice, u64, u64)>, // outgoing track: (voice, position, fade length)
    paused: bool,
    pgain: f32,
    ramp: f32,
    vol: f32,
    vol_t: f32,
    eq: Option<Eq>,
    xfeed: Option<Crossfeed>,
    buf: [f32; 2],
    idx: usize,
    poll: u32,
}

impl Mixer {
    fn new(rx: Receiver<Mix>, ev: Sender<AudioEvent>, shared: Arc<Shared>, rate: u32) -> Mixer {
        Mixer {
            rx,
            ev,
            shared,
            rate,
            cur: None,
            next: None,
            out: None,
            paused: true,
            pgain: 0.0,
            ramp: 1.0 / (rate as f32 * 0.012),
            vol: 0.8,
            vol_t: 0.8,
            eq: None,
            xfeed: None,
            buf: [0.0; 2],
            idx: 0,
            poll: 0,
        }
    }

    fn fade_out_current(&mut self, ms: u64) {
        if let Some(c) = self.cur.take() {
            let len = (self.rate as u64 * ms / 1000).max(1);
            self.out = Some((c, 0, len));
        }
    }

    fn poll_msgs(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Mix::Play(v, paused) => {
                    if !self.paused && self.pgain > 0.0 {
                        self.fade_out_current(25);
                    } else {
                        self.cur = None;
                    }
                    self.shared.id.store(v.id, Relaxed);
                    self.shared.pos.store(v.frames, Relaxed);
                    let _ = self.ev.send(AudioEvent::Started { id: v.id });
                    self.cur = Some(v);
                    self.next = None;
                    self.paused = paused;
                    if paused {
                        self.pgain = 0.0;
                    }
                }
                Mix::Replace(v) => {
                    self.fade_out_current(20);
                    self.shared.pos.store(v.frames, Relaxed);
                    self.cur = Some(v);
                }
                Mix::Next(v) => self.next = v,
                Mix::Pause(p) => self.paused = p,
                Mix::Volume(v) => self.vol_t = v.clamp(0.0, 1.5),
                Mix::Eq(e) => self.eq = e,
                Mix::Crossfeed(x) => self.xfeed = x,
                Mix::Stop => {
                    self.fade_out_current(25);
                    self.next = None;
                }
            }
        }
        self.shared.playing.store(!self.paused && self.cur.is_some(), Relaxed);
    }

    fn maybe_crossfade(&mut self) {
        if self.out.is_some() {
            return;
        }
        let (Some(c), Some(n)) = (&self.cur, &self.next) else { return };
        if n.fade_in == 0 || c.total == 0 || c.frames + n.fade_in < c.total {
            return;
        }
        let c = self.cur.take().unwrap();
        let len = c.total.saturating_sub(c.frames).max(1);
        self.out = Some((c, 0, len));
        let n = self.next.take().unwrap();
        self.shared.id.store(n.id, Relaxed);
        let _ = self.ev.send(AudioEvent::Advanced { id: n.id });
        self.cur = Some(n);
    }

    fn frame(&mut self) -> (f32, f32) {
        if self.poll == 0 {
            self.poll_msgs();
            self.poll = 64;
        }
        self.poll -= 1;

        let target = if self.paused { 0.0 } else { 1.0 };
        self.pgain += (target - self.pgain).clamp(-self.ramp, self.ramp);
        if self.paused && self.pgain <= 0.0 {
            return (0.0, 0.0);
        }

        self.maybe_crossfade();
        let (mut l, mut r) = (0.0f32, 0.0f32);

        if let Some(v) = self.cur.as_mut() {
            match v.pull() {
                Some((a, b)) => {
                    let fade = if v.fade_in > 0 && v.frames < v.fade_in { (v.frames as f32 / v.fade_in as f32 * FRAC_PI_2).sin() } else { 1.0 };
                    let g = v.gain * fade;
                    l += a * g;
                    r += b * g;
                    v.frames += 1;
                    self.shared.pos.store(v.frames, Relaxed);
                }
                None => {
                    let id = v.id;
                    self.cur = None;
                    match self.next.take() {
                        Some(mut n) => {
                            n.fade_in = 0; // no crossfade: chained directly (gapless)
                            self.shared.id.store(n.id, Relaxed);
                            self.shared.pos.store(n.frames, Relaxed);
                            let _ = self.ev.send(AudioEvent::Advanced { id: n.id });
                            self.cur = Some(n);
                        }
                        None => {
                            self.shared.playing.store(false, Relaxed);
                            let _ = self.ev.send(AudioEvent::Ended { id });
                        }
                    }
                }
            }
        }

        if let Some((v, pos, len)) = self.out.as_mut() {
            let done = *pos >= *len;
            match (done, v.pull()) {
                (false, Some((a, b))) => {
                    let g = v.gain * ((*pos as f32 / *len as f32) * FRAC_PI_2).cos();
                    l += a * g;
                    r += b * g;
                    *pos += 1;
                }
                _ => self.out = None,
            }
        }

        if let Some(eq) = self.eq.as_mut() {
            (l, r) = eq.process(l, r);
        }
        if let Some(x) = self.xfeed.as_mut() {
            (l, r) = x.process(l, r);
        }
        self.vol += (self.vol_t - self.vol) * 0.002;
        let g = self.vol * self.pgain;
        (soft_clip(l * g), soft_clip(r * g))
    }
}

impl Iterator for Mixer {
    type Item = f32;
    #[inline]
    fn next(&mut self) -> Option<f32> {
        if self.idx == 0 {
            let (l, r) = self.frame();
            self.buf = [l, r];
        }
        let s = self.buf[self.idx];
        self.idx ^= 1;
        Some(s)
    }
}

impl Source for Mixer {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/* ───────────── Control ───────────── */

enum Ctl {
    Play(TrackSpec, u64, bool),
    Next(Option<TrackSpec>),
    Seek(u64),
    Pause(bool),
    Volume(f32),
    Eq(EqSettings),
    Crossfeed(u8),
    Stop,
    Advanced(u64),
}

fn open_voice(spec: &TrackSpec, rate: u32, start_ms: u64) -> Result<Voice, String> {
    let mut dec = SymSource::open(&spec.path)?;
    let total_ms = if spec.duration_ms > 0 { spec.duration_ms } else { dec.total_duration().map(|d| d.as_millis() as u64).unwrap_or(0) };
    // CUE tracks: positions are relative to the track, the file is seeked from its start
    let base = spec.start_ms;
    let start = if start_ms > 0 || base > 0 {
        if dec.seek(base + start_ms) {
            start_ms
        } else if base > 0 {
            return Err("Couldn't seek to the CUE track".into());
        } else {
            0
        }
    } else {
        0
    };
    if spec.end_ms > base {
        dec.limit(spec.end_ms.saturating_sub(base + start));
    }
    let mut src = Buffered::spawn(dec, rate);
    src.prime();
    Ok(Voice {
        id: spec.id,
        src,
        gain: db_to_lin(spec.gain_db.clamp(-24.0, 18.0)),
        frames: start * rate as u64 / 1000,
        total: total_ms * rate as u64 / 1000,
        fade_in: spec.crossfade_ms * rate as u64 / 1000,
    })
}

/* ───────────── Outputs ─────────────
The mixer can run on a normal (shared) Windows output through rodio/cpal, or on a
WASAPI exclusive stream that skips the Windows mixer. Switching output creates a new mixer;
the frontend then reloads the current track where it was. */

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OutputInfo {
    pub device: String,
    pub rate: u32,
    pub exclusive: bool,
    /// Sample format used in exclusive mode (e.g. "32-bit float", "24-bit")
    pub format: String,
}

struct OutputReq {
    device: Option<String>,
    exclusive: bool,
    reply: Sender<Result<OutputInfo, String>>,
}

/// What keeps an output alive: dropping it stops the sound.
enum Keep {
    Shared(#[allow(dead_code)] OutputStream),
    #[cfg(windows)]
    Exclusive(Arc<AtomicBool>, Option<std::thread::JoinHandle<()>>),
}

impl Drop for Keep {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Keep::Exclusive(stop, h) = self {
            stop.store(true, Relaxed);
            if let Some(h) = h.take() {
                let _ = h.join();
            }
        }
    }
}

use rodio::cpal::traits::{DeviceTrait, HostTrait};

/// Names of the output devices (the default one first).
pub fn output_devices() -> Vec<String> {
    let host = rodio::cpal::default_host();
    let def = host.default_output_device().and_then(|d| d.name().ok());
    let mut names: Vec<String> = host.output_devices().map(|it| it.filter_map(|d| d.name().ok()).collect()).unwrap_or_default();
    names.sort();
    names.dedup();
    if let Some(d) = def {
        names.retain(|n| *n != d);
        names.insert(0, d);
    }
    names
}

fn open_shared(device: Option<&str>, mixer_for: impl FnOnce(u32) -> Mixer) -> Result<(Keep, OutputInfo), String> {
    let host = rodio::cpal::default_host();
    let dev = match device {
        Some(name) => {
            host.output_devices().map_err(|e| e.to_string())?.find(|d| d.name().ok().as_deref() == Some(name)).ok_or_else(|| format!("The device \"{name}\" isn't connected"))?
        }
        None => host.default_output_device().ok_or("No audio device")?,
    };
    let rate = dev.default_output_config().map(|c| c.sample_rate().0).map_err(|e| e.to_string())?;
    let name = dev.name().unwrap_or_default();
    let (stream, handle) = OutputStream::try_from_device(&dev).map_err(|e| format!("Couldn't open {name}: {e}"))?;
    handle.play_raw(mixer_for(rate)).map_err(|e| e.to_string())?;
    Ok((Keep::Shared(stream), OutputInfo { device: name, rate, exclusive: false, format: String::new() }))
}

#[cfg(windows)]
mod exclusive {
    use super::*;
    use wasapi::{DeviceEnumerator, Direction, SampleType, StreamMode, WaveFormat};

    #[derive(Clone, Copy)]
    enum Fmt {
        F32,
        I32,
        I24,
        I16,
    }

    fn label(f: Fmt) -> &'static str {
        match f {
            Fmt::F32 => "32-bit float",
            Fmt::I32 => "32-bit",
            Fmt::I24 => "24-bit",
            Fmt::I16 => "16-bit",
        }
    }

    #[inline]
    fn put(f: Fmt, x: f32, out: &mut Vec<u8>) {
        let x = x.clamp(-1.0, 1.0);
        match f {
            Fmt::F32 => out.extend_from_slice(&x.to_le_bytes()),
            Fmt::I32 => out.extend_from_slice(&((x as f64 * 2147483647.0) as i32).to_le_bytes()),
            Fmt::I24 => out.extend_from_slice(&((x * 8388607.0) as i32).to_le_bytes()[..3]),
            Fmt::I16 => out.extend_from_slice(&((x * 32767.0) as i16).to_le_bytes()),
        }
    }

    fn friendly(e: wasapi::WasapiError) -> String {
        let t = e.to_string();
        if t.contains("0x8889000A") || t.to_lowercase().contains("in use") {
            "Another app is using the device in exclusive mode".into()
        } else if t.contains("0x88890019") || t.to_lowercase().contains("not allowed") {
            "Windows doesn't allow exclusive mode on this device (Sound settings → device → Advanced → \"Allow applications to take exclusive control\")".into()
        } else if t.contains("0x88890008") {
            "The device doesn't accept any of the formats Wax can send".into()
        } else {
            format!("Exclusive mode failed: {t}")
        }
    }

    /// Runs the mixer on a WASAPI exclusive stream until `stop`. Polling timing is used:
    /// it works with more devices and avoids the stutter some USB DACs have with events.
    pub fn open(device: Option<String>, mixer_for: impl FnOnce(u32) -> Mixer + Send + 'static) -> Result<(Keep, OutputInfo), String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = channel::<Result<OutputInfo, String>>();
        let st = stop.clone();
        let handle = std::thread::Builder::new()
            .name("wax-exclusive".into())
            .spawn(move || {
                let _ = wasapi::initialize_mta();
                let setup = (|| -> Result<_, String> {
                    let en = DeviceEnumerator::new().map_err(friendly)?;
                    let dev = match &device {
                        Some(n) => en.get_device_collection(&Direction::Render).and_then(|c| c.get_device_with_name(n)).map_err(friendly)?,
                        None => en.get_default_device(&Direction::Render).map_err(friendly)?,
                    };
                    let name = dev.get_friendlyname().unwrap_or_default();
                    let mut client = dev.get_iaudioclient().map_err(friendly)?;
                    let rate = client.get_mixformat().map(|f| f.get_samplespersec()).unwrap_or(48000) as usize;
                    let tries = [
                        (Fmt::F32, 32, 32, SampleType::Float),
                        (Fmt::I32, 32, 32, SampleType::Int),
                        (Fmt::I32, 32, 24, SampleType::Int),
                        (Fmt::I24, 24, 24, SampleType::Int),
                        (Fmt::I16, 16, 16, SampleType::Int),
                    ];
                    let (fmt, wf) = tries
                        .iter()
                        .find_map(|(f, store, valid, st)| {
                            let wf = WaveFormat::new(*store, *valid, st, rate, 2, None);
                            client.is_supported_exclusive_with_quirks(&wf).ok().map(|w| (*f, w))
                        })
                        .ok_or("The device doesn't accept any of the formats Wax can send in exclusive mode")?;
                    let (def_period, _min) = client.get_device_period().map_err(friendly)?;
                    let period = client.calculate_aligned_period_near(def_period, Some(128), &wf).unwrap_or(def_period);
                    let mode = StreamMode::PollingExclusive { buffer_duration_hns: period * 4, period_hns: period };
                    if let Err(e) = client.initialize_client(&wf, &Direction::Render, &mode) {
                        // Buffer not aligned: retry with the size the driver wants (as Microsoft documents)
                        let size = client.get_buffer_size().map_err(|_| friendly(e))?;
                        let aligned = wasapi::calculate_period_100ns(size as i64, rate as i64);
                        client = dev.get_iaudioclient().map_err(friendly)?;
                        client
                            .initialize_client(&wf, &Direction::Render, &StreamMode::PollingExclusive { buffer_duration_hns: aligned * 4, period_hns: aligned })
                            .map_err(friendly)?;
                    }
                    let render = client.get_audiorenderclient().map_err(friendly)?;
                    Ok((client, render, fmt, rate as u32, name, period))
                })();
                let (client, render, fmt, rate, name, period) = match setup {
                    Ok(x) => x,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let mut mixer = mixer_for(rate);
                let _ = ready_tx.send(Ok(OutputInfo { device: name, rate, exclusive: true, format: label(fmt).into() }));
                let mut data = Vec::new();
                let fill = |frames: usize, mixer: &mut Mixer, data: &mut Vec<u8>| {
                    data.clear();
                    for _ in 0..frames * 2 {
                        put(fmt, mixer.next().unwrap_or(0.0), data);
                    }
                };
                if let Ok(n) = client.get_available_space_in_frames() {
                    fill(n as usize, &mut mixer, &mut data);
                    let _ = render.write_to_device(n as usize, &data, None);
                }
                if client.start_stream().is_err() {
                    return;
                }
                let nap = Duration::from_micros((period as u64 / 10 / 2).clamp(1000, 10_000));
                while !st.load(Relaxed) {
                    std::thread::sleep(nap);
                    match client.get_available_space_in_frames() {
                        Ok(0) => {}
                        Ok(n) => {
                            fill(n as usize, &mut mixer, &mut data);
                            if render.write_to_device(n as usize, &data, None).is_err() {
                                break;
                            }
                        }
                        Err(_) => break, // device unplugged
                    }
                }
                let _ = client.stop_stream();
            })
            .map_err(|e| e.to_string())?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(info)) => Ok((Keep::Exclusive(stop, Some(handle)), info)),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => {
                stop.store(true, Relaxed);
                Err("The device didn't answer".into())
            }
        }
    }
}

pub struct Engine {
    tx: Mutex<Sender<Ctl>>,
    out_tx: Mutex<Sender<OutputReq>>,
    #[allow(dead_code)]
    pub shared: Arc<Shared>,
}

impl Engine {
    pub fn start<F>(emit: F) -> Result<Engine, String>
    where
        F: Fn(AudioEvent) + Send + 'static,
    {
        let shared = Arc::new(Shared { id: AtomicU64::new(0), pos: AtomicU64::new(0), playing: AtomicBool::new(false), rate: AtomicU64::new(44100) });
        let (ev_tx, ev_rx) = channel::<AudioEvent>();
        // The mixer currently playing: the control thread sends it voices through this
        let (first_tx, _) = channel::<Mix>();
        let mix_tx: Arc<Mutex<Sender<Mix>>> = Arc::new(Mutex::new(first_tx));
        let (out_tx, out_rx) = channel::<OutputReq>();
        let (ready_tx, ready_rx) = channel::<Result<OutputInfo, String>>();

        // Output thread: owns the current output (an OutputStream isn't Send, so it lives here)
        let sh = shared.clone();
        let slot = mix_tx.clone();
        let evs = ev_tx.clone();
        std::thread::Builder::new()
            .name("wax-audio".into())
            .spawn(move || {
                let open = |device: Option<String>, exclusive: bool| -> Result<(Keep, OutputInfo), String> {
                    let (tx, rx) = channel::<Mix>();
                    let (sh2, ev2) = (sh.clone(), evs.clone());
                    let make = move |rate: u32| Mixer::new(rx, ev2, sh2, rate);
                    let r = if exclusive {
                        #[cfg(windows)]
                        {
                            exclusive::open(device, make)
                        }
                        #[cfg(not(windows))]
                        {
                            let _ = make;
                            Err("Exclusive mode is only available on Windows".to_string())
                        }
                    } else {
                        open_shared(device.as_deref(), make)
                    };
                    if let Ok((_, info)) = &r {
                        sh.rate.store(info.rate as u64, Relaxed);
                        if let Ok(mut s) = slot.lock() {
                            *s = tx;
                        }
                    }
                    r
                };
                let mut keep = match open(None, false) {
                    Ok((k, info)) => {
                        let _ = ready_tx.send(Ok(info));
                        Some(k)
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        None
                    }
                };
                while let Ok(req) = out_rx.recv() {
                    keep = None; // release the device first (exclusive mode needs it free)
                    let r = open(req.device.clone(), req.exclusive);
                    let r = match r {
                        Ok((k, info)) => {
                            keep = Some(k);
                            Ok(info)
                        }
                        Err(e) => {
                            // Fall back to the normal output so there's always sound
                            if let Ok((k, _)) = open(req.device.clone(), false).or_else(|_| open(None, false)) {
                                keep = Some(k);
                            }
                            Err(e)
                        }
                    };
                    let _ = req.reply.send(r);
                }
                drop(keep);
            })
            .map_err(|e| e.to_string())?;

        ready_rx.recv().map_err(|e| e.to_string())??;

        // Control thread: opens files off the audio thread.
        let (ctl_tx, ctl_rx) = channel::<Ctl>();
        let sh = shared.clone();
        let (err_tx, err_rx) = channel::<AudioEvent>();
        let mixer = mix_tx.clone();
        std::thread::Builder::new()
            .name("wax-audio-ctl".into())
            .spawn(move || {
                let send = |m: Mix| {
                    if let Ok(tx) = mixer.lock() {
                        let _ = tx.send(m);
                    }
                };
                let rate = || sh.rate.load(Relaxed).max(1) as u32;
                let mut cur: Option<TrackSpec> = None;
                let mut next: Option<TrackSpec> = None;
                while let Ok(c) = ctl_rx.recv() {
                    match c {
                        Ctl::Play(spec, start_ms, paused) => {
                            next = None;
                            match open_voice(&spec, rate(), start_ms) {
                                Ok(v) => {
                                    cur = Some(spec);
                                    send(Mix::Play(v, paused));
                                }
                                Err(message) => {
                                    let _ = err_tx.send(AudioEvent::Error { id: spec.id, message });
                                }
                            }
                        }
                        Ctl::Next(spec) => {
                            next = spec.clone();
                            let v = spec.and_then(|s| open_voice(&s, rate(), 0).ok());
                            if next.is_some() && v.is_none() {
                                next = None; // can't be opened: the frontend will play it when the current one ends
                            }
                            send(Mix::Next(v));
                        }
                        Ctl::Seek(ms) => {
                            if let Some(spec) = &cur {
                                if let Ok(mut v) = open_voice(spec, rate(), ms) {
                                    v.fade_in = 0;
                                    send(Mix::Replace(v));
                                }
                            }
                        }
                        Ctl::Advanced(id) => {
                            if next.as_ref().map(|n| n.id) == Some(id) {
                                cur = next.take();
                            }
                        }
                        Ctl::Pause(p) => send(Mix::Pause(p)),
                        Ctl::Volume(v) => send(Mix::Volume(v)),
                        Ctl::Eq(s) => send(Mix::Eq(Eq::new(&s, rate()))),
                        Ctl::Crossfeed(l) => send(Mix::Crossfeed(Crossfeed::new(l, rate()))),
                        Ctl::Stop => {
                            cur = None;
                            next = None;
                            send(Mix::Stop);
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        // Event thread: forwards to the frontend and publishes the position ~4 times per second.
        let sh2 = shared.clone();
        let ctl_for_ev = ctl_tx.clone();
        drop(ev_tx);
        std::thread::Builder::new()
            .name("wax-audio-ev".into())
            .spawn(move || {
                let mut idle = 0u32;
                loop {
                    let ev = match ev_rx.recv_timeout(Duration::from_millis(250)) {
                        Ok(e) => Some(e),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    while let Ok(e) = err_rx.try_recv() {
                        emit(e);
                    }
                    if let Some(e) = ev {
                        if let AudioEvent::Advanced { id } = &e {
                            let _ = ctl_for_ev.send(Ctl::Advanced(*id));
                        }
                        emit(e);
                    }
                    let playing = sh2.playing.load(Relaxed);
                    idle = if playing { 0 } else { idle + 1 };
                    if playing || idle % 4 == 1 {
                        let rate = sh2.rate.load(Relaxed).max(1);
                        emit(AudioEvent::Time { id: sh2.id.load(Relaxed), pos: sh2.pos.load(Relaxed) * 1000 / rate, playing });
                    }
                }
            })
            .map_err(|e| e.to_string())?;

        Ok(Engine { tx: Mutex::new(ctl_tx), out_tx: Mutex::new(out_tx), shared })
    }

    fn send(&self, c: Ctl) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send(c);
        }
    }
    pub fn play(&self, spec: TrackSpec, start_ms: u64, paused: bool) {
        self.send(Ctl::Play(spec, start_ms, paused));
    }
    pub fn set_next(&self, spec: Option<TrackSpec>) {
        self.send(Ctl::Next(spec));
    }
    pub fn seek(&self, ms: u64) {
        self.send(Ctl::Seek(ms));
    }
    pub fn pause(&self, p: bool) {
        self.send(Ctl::Pause(p));
    }
    pub fn volume(&self, v: f32) {
        self.send(Ctl::Volume(v));
    }
    pub fn eq(&self, s: EqSettings) {
        self.send(Ctl::Eq(s));
    }
    pub fn crossfeed(&self, level: u8) {
        self.send(Ctl::Crossfeed(level));
    }
    pub fn stop(&self) {
        self.send(Ctl::Stop);
    }
    /// Switches the output (blocks until the new one is open). On failure the normal output
    /// of that device (or the default one) is used, and the error is returned.
    pub fn set_output(&self, device: Option<String>, exclusive: bool) -> Result<OutputInfo, String> {
        self.send(Ctl::Stop);
        let (reply, rx) = channel();
        self.out_tx.lock().map_err(|e| e.to_string())?.send(OutputReq { device, exclusive, reply }).map_err(|e| e.to_string())?;
        rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "The audio device didn't answer".to_string())?
    }
}

/* ───────────── Loudness analysis (EBU R128 / ReplayGain 2.0) ───────────── */

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Loudness {
    /// Integrated loudness in LUFS
    pub lufs: f64,
    /// Sample peak (linear, 1.0 = 0 dBFS)
    pub peak: f64,
}

pub fn analyze_loudness(path: &str) -> Result<Loudness, String> {
    let mut dec = SymSource::open(path)?;
    let ch = dec.channels().max(1) as u32;
    let rate = dec.sample_rate().max(1);
    let mut meter = ebur128::EbuR128::new(ch, rate, ebur128::Mode::I | ebur128::Mode::SAMPLE_PEAK).map_err(|e| e.to_string())?;
    // Measured packet by packet, straight from the decoder's buffer
    loop {
        let whole = dec.buf.len() - dec.buf.len() % ch as usize;
        if whole > 0 && dec.channels as u32 == ch {
            meter.add_frames_f32(&dec.buf[..whole]).map_err(|e| e.to_string())?;
        }
        if !dec.refill() {
            break;
        }
    }
    let lufs = meter.loudness_global().map_err(|e| e.to_string())?;
    let peak = (0..ch).filter_map(|c| meter.sample_peak(c).ok()).fold(0.0f64, f64::max);
    if !lufs.is_finite() {
        return Err("silence".into());
    }
    Ok(Loudness { lufs, peak })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Short test files (3 s tones) in tests/fixtures; WAX_TEST_AUDIO points elsewhere
    fn dir() -> String {
        crate::test_audio_dir()
    }
    fn spec(id: u64, f: &str, xf: u64) -> TrackSpec {
        TrackSpec { id, path: format!("{}/{f}", dir()), gain_db: 0.0, duration_ms: 3000, crossfade_ms: xf, start_ms: 0, end_ms: 0 }
    }
    fn mixer(rate: u32) -> (Mixer, Sender<Mix>, Receiver<AudioEvent>) {
        let (tx, rx) = channel();
        let (etx, erx) = channel();
        let shared = Arc::new(Shared { id: AtomicU64::new(0), pos: AtomicU64::new(0), playing: AtomicBool::new(false), rate: AtomicU64::new(rate as u64) });
        let mut m = Mixer::new(rx, etx, shared, rate);
        m.vol = 1.0;
        m.vol_t = 1.0;
        (m, tx, erx)
    }
    /// Runs the mixer and returns (events, seconds with sound, longest silent gap in ms)
    fn run(m: &mut Mixer, erx: &Receiver<AudioEvent>, secs: f32) -> (Vec<String>, f32, f32) {
        let rate = m.rate as f32;
        let (mut audible, mut gap, mut max_gap, mut started) = (0u64, 0u64, 0u64, false);
        let block = (rate / 100.0) as u64;
        for _ in 0..((rate * secs) as u64 / block) {
            let mut peak = 0f32;
            for _ in 0..block {
                let l = m.next().unwrap();
                let _r = m.next().unwrap();
                peak = peak.max(l.abs());
            }
            if peak > 0.01 {
                audible += block;
                started = true;
                max_gap = max_gap.max(gap);
                gap = 0;
            } else if started {
                gap += block;
            }
        }
        let evs = erx
            .try_iter()
            .map(|e| match e {
                AudioEvent::Started { id } => format!("start{id}"),
                AudioEvent::Advanced { id } => format!("adv{id}"),
                AudioEvent::Ended { id } => format!("end{id}"),
                other => format!("{other:?}"),
            })
            .collect();
        (evs, audible as f32 / rate, max_gap as f32 / rate * 1000.0)
    }

    /// Gain of a filter at a frequency, measured with a sine.
    fn gain_at(mut f: impl FnMut(f32) -> f32, hz: f32, fs: f32) -> f32 {
        let (mut peak, n) = (0f32, (fs * 0.5) as usize);
        for i in 0..n {
            let y = f((2.0 * PI * hz * i as f32 / fs).sin());
            if i > n / 2 {
                peak = peak.max(y.abs());
            }
        }
        20.0 * peak.log10()
    }

    #[test]
    fn autoeq_filters_and_crossfeed() {
        let fs = 48000.0;
        let mut ls = Biquad::shelf(false, 105.0, 6.0, 0.7, fs);
        let mut hs = Biquad::shelf(true, 10000.0, -4.0, 0.7, fs);
        let mut pk = Biquad::peaking(1000.0, 3.0, 1.0, fs);
        let (ls_lo, ls_hi) = (gain_at(|x| ls.run(0, x), 20.0, fs), gain_at(|x| ls.run(1, x), 5000.0, fs));
        let (hs_lo, hs_hi) = (gain_at(|x| hs.run(0, x), 200.0, fs), gain_at(|x| hs.run(1, x), 20000.0, fs));
        let pk_c = gain_at(|x| pk.run(0, x), 1000.0, fs);
        println!("low shelf +6: {ls_lo:.2} dB @20Hz, {ls_hi:.2} dB @5k · high shelf -4: {hs_lo:.2} @200, {hs_hi:.2} @20k · peak +3: {pk_c:.2} @1k");
        assert!((ls_lo - 6.0).abs() < 0.3 && ls_hi.abs() < 0.3);
        assert!(hs_lo.abs() < 0.3 && (hs_hi + 4.0).abs() < 0.4);
        assert!((pk_c - 3.0).abs() < 0.1);
        // Crossfeed: centered (mono) bass keeps its level, a hard-left sound leaks to the right
        let mut x = Crossfeed::new(2, 48000).unwrap();
        let mono = gain_at(|v| x.process(v, v).0, 100.0, fs);
        let mut x = Crossfeed::new(2, 48000).unwrap();
        let leak = gain_at(|v| x.process(v, 0.0).1, 100.0, fs);
        println!("crossfeed: mono bass {mono:.2} dB, left→right leak {leak:.2} dB");
        assert!(mono.abs() < 0.3 && leak < -6.0 && leak > -12.0);
    }

    /// Opens the real outputs: the normal one, then exclusive mode for a moment.
    #[test]
    fn outputs() {
        if std::env::var("WAX_TEST_OUTPUT").is_err() {
            return;
        }
        let e = Engine::start(|_| {}).unwrap();
        println!("devices: {:?}", output_devices());
        println!("shared: {:?}", e.set_output(None, false));
        if std::env::var("WAX_TEST_EXCLUSIVE").is_ok() {
            let ex = e.set_output(None, true);
            println!("exclusive: {ex:?}");
            std::thread::sleep(Duration::from_millis(300));
            println!("back: {:?}", e.set_output(None, false));
            assert!(ex.is_ok());
        }
    }

    #[test]
    fn gapless_mixed_formats() {
        let rate = 48000;
        let (mut m, tx, erx) = mixer(rate);
        tx.send(Mix::Play(open_voice(&spec(1, "a.flac", 0), rate, 0).unwrap(), false)).unwrap();
        tx.send(Mix::Next(Some(open_voice(&spec(2, "b.mp3", 0), rate, 0).unwrap()))).unwrap();
        let (evs, audible, gap) = run(&mut m, &erx, 4.5);
        println!("gapless: events={evs:?} sound={audible:.2}s max_gap={gap:.1}ms");
        assert!(evs.contains(&"adv2".to_string()));
        assert!(gap < 15.0, "there was a {gap} ms gap");
        assert!(audible > 4.3);
    }

    /// 44.1 kHz → 48 kHz: exact length, and a 10 kHz tone must come out clean
    /// (the old linear interpolation left distortion around −40 dB, audible on guitars and cymbals).
    #[test]
    fn resampling_is_clean() {
        let path = std::env::temp_dir().join("wax_tone.wav");
        let (rate, secs, f) = (44100u32, 2u32, 10_000.0f64);
        let n = rate * secs;
        let mut wav = Vec::new();
        let data_len = n * 4;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for i in 0..n {
            let v = ((2.0 * std::f64::consts::PI * f * i as f64 / rate as f64).sin() * 0.5 * 32767.0).round() as i16;
            wav.extend_from_slice(&v.to_le_bytes());
            wav.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(&path, wav).unwrap();
        let mut b = Buffered::spawn(SymSource::open(path.to_str().unwrap()).unwrap(), 48000);
        let mut left = Vec::new();
        while let Some((l, _)) = b.frame() {
            left.push(l as f64);
        }
        let _ = std::fs::remove_file(&path);
        assert_eq!(left.len(), 96000, "length after resampling");
        // Least-squares fit of the tone on the middle second; what's left is noise/distortion
        let seg = &left[24000..72000];
        let w = 2.0 * std::f64::consts::PI * f / 48000.0;
        let (mut ss, mut sc, mut cc, mut ys, mut yc) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (k, y) in seg.iter().enumerate() {
            let (s, c) = ((w * (k + 24000) as f64).sin(), (w * (k + 24000) as f64).cos());
            ss += s * s;
            sc += s * c;
            cc += c * c;
            ys += y * s;
            yc += y * c;
        }
        let det = ss * cc - sc * sc;
        let (a, bb) = ((ys * cc - yc * sc) / det, (yc * ss - ys * sc) / det);
        let (mut sig, mut err) = (0.0, 0.0);
        for (k, y) in seg.iter().enumerate() {
            let t = w * (k + 24000) as f64;
            let fit = a * t.sin() + bb * t.cos();
            sig += fit * fit;
            err += (y - fit) * (y - fit);
        }
        let snr = 10.0 * (sig / err).log10();
        println!("resampling 44.1k→48k: {} frames, tone-to-distortion {snr:.1} dB", left.len());
        assert!(snr > 75.0, "distortion too high: {snr:.1} dB");
    }

    /// CUE tracks: two ranges of the same file, played back to back, must give exactly
    /// the same samples as the whole file (no gap, no overlap, no click).
    #[test]
    fn cue_ranges_are_sample_exact() {
        let path = format!("{}/a.flac", dir());
        let whole: Vec<f32> = SymSource::open(&path).unwrap().collect();
        let mut first = SymSource::open(&path).unwrap();
        first.limit(1500);
        let mut second = SymSource::open(&path).unwrap();
        assert!(second.seek(1500));
        let joined: Vec<f32> = first.chain(second).collect();
        let diff = whole.iter().zip(&joined).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        println!("cue: whole={} joined={} max_diff={diff}", whole.len(), joined.len());
        assert_eq!(whole.len(), joined.len());
        assert!(diff < 1e-6);
    }

    #[test]
    fn crossfade_and_end() {
        let rate = 44100;
        let (mut m, tx, erx) = mixer(rate);
        tx.send(Mix::Play(open_voice(&spec(1, "a.flac", 0), rate, 0).unwrap(), false)).unwrap();
        tx.send(Mix::Next(Some(open_voice(&spec(2, "c.m4a", 1500), rate, 0).unwrap()))).unwrap();
        let (evs, audible, _) = run(&mut m, &erx, 6.0);
        println!("crossfade: events={evs:?} sound={audible:.2}s");
        assert_eq!(evs, vec!["start1", "adv2", "end2"]);
        // 3 s + 3 s with 1.5 s overlapping ≈ 4.5 s of sound
        assert!((audible - 4.5).abs() < 0.4, "length with crossfade: {audible}");
    }

    #[test]
    fn seek_and_eq() {
        let rate = 44100;
        let v = open_voice(&spec(1, "b.mp3", 0), rate, 2000).unwrap();
        assert!(v.frames >= rate as u64 * 19 / 10, "frames after seeking: {}", v.frames);
        let (mut m, tx, erx) = mixer(rate);
        tx.send(Mix::Eq(Eq::new(&EqSettings { enabled: true, preamp: -3.0, bands: vec![6.0; 10], ..Default::default() }, rate))).unwrap();
        tx.send(Mix::Play(v, false)).unwrap();
        let (_, audible, _) = run(&mut m, &erx, 1.5);
        println!("seek to 2 s: remaining sound={audible:.2}s");
        assert!(audible > 0.8 && audible < 1.2);
    }

    #[test]
    fn loudness_and_opus() {
        for f in ["a.flac", "b.mp3", "c.m4a"] {
            let l = analyze_loudness(&format!("{}/{f}", dir())).unwrap();
            println!("{f}: {:.1} LUFS, peak {:.3}", l.lufs, l.peak);
            assert!(l.lufs < -5.0 && l.lufs > -40.0);
        }
        println!("opus in the engine: {:?}", open_voice(&spec(9, "d.opus", 0), 44100, 0).err());
    }
}
