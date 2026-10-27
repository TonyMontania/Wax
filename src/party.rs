//! Listen Together: one Wax hosts a session and shares what it plays; the others join with a
//! code and hear the same song at the same moment. The host serves its own files over HTTP —
//! on the local network, or over the internet when the router opens the port (UPnP) — with no
//! outside service in between. People without Wax can listen in a web browser.
//!
//! Security: the key in the code never travels. Wax guests sign each request with it (HMAC, with
//! the time, and a signature works once), so someone watching the network can't join. The web
//! link has its own key: it lets a browser listen and ask for songs, nothing more. Whoever keeps
//! guessing keys gets blocked for a while.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tiny_http::{Header, Request, Response, Server, StatusCode};

use crate::data_dir;

const FIRST_PORT: u16 = 47_800;
/// Songs sent by guests are numbered from here
const SHARED_IDS: u32 = 1_000_000_000;
/// The largest song a guest can send
const MAX_SHARE: usize = 400 * 1024 * 1024;
const WEB_PAGE: &str = include_str!("party.html");

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    if getrandom::getrandom(&mut b).is_err() {
        // Never expected; still better than a fixed value
        let n = now_ms() ^ (std::process::id() as u64) << 20;
        for (i, x) in b.iter_mut().enumerate() {
            *x = (n >> ((i % 8) * 8)) as u8 ^ (i as u8).wrapping_mul(97);
        }
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// HMAC-SHA256 (RFC 2104)
fn hmac(key: &[u8], msg: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let (mut ipad, mut opad) = ([0x36u8; 64], [0x5cu8; 64]);
    for i in 0..64 {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let inner = Sha256::new().chain_update(ipad).chain_update(msg).finalize();
    Sha256::new().chain_update(opad).chain_update(inner).finalize().into()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Same answer for equal strings, in the same time whatever matches
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A song of the host's library, as the guests see it (never the path on the host's disk).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Song {
    pub id: u32,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    #[serde(default, skip_serializing)]
    pub path: String,
    /// The file type, so the guest keeps it in its copy's name
    #[serde(default)]
    pub ext: String,
    /// CUE sheet tracks: where the song starts and ends inside the file
    #[serde(default)]
    pub start_ms: u64,
    #[serde(default)]
    pub end_ms: u64,
    /// Shared by a guest from their own library (their name); empty = the host's
    #[serde(default)]
    pub from: String,
}

/* ───────────── Invite codes ───────────── */

// No 0/O or 1/I, so a code read aloud or copied by hand still works
const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Address + port + key packed in letters: 18 for IPv4 ("ABCDEF-GHJKLM-NPQRST"), 37 for IPv6.
pub fn make_code(ip: IpAddr, port: u16, token: &str) -> String {
    let mut bytes = match ip {
        IpAddr::V4(ip) => ip.octets().to_vec(),
        IpAddr::V6(ip) => ip.octets().to_vec(),
    };
    bytes.extend(port.to_be_bytes());
    bytes.extend((0..token.len() / 2).filter_map(|i| u8::from_str_radix(&token[i * 2..i * 2 + 2], 16).ok()));
    let (mut acc, mut bits, mut out) = (0u32, 0, String::new());
    for b in bytes {
        acc = (acc << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out.as_bytes().chunks(6).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("-")
}

/// A code or a link ("http://host:port/?k=key") → ("http://host:port", key)
pub fn parse_invite(input: &str) -> Result<(String, String), String> {
    let s = input.trim();
    let bad = || "That isn't a session code. Ask the host to copy it again.".to_string();
    if s.contains("://") || s.contains("?k=") {
        let (base, query) = s.split_once('?').ok_or_else(bad)?;
        let key = query.split('&').find_map(|p| p.strip_prefix("k=")).ok_or_else(bad)?;
        let base = base.trim_end_matches('/');
        let base = if base.contains("://") { base.to_string() } else { format!("http://{base}") };
        return Ok((base, key.to_string()));
    }
    let clean: Vec<u8> = s.bytes().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
    if clean.len() < 18 {
        return Err(bad());
    }
    // An IPv6 code is 37 letters
    let clean = if clean.len() >= 37 { clean[..37].to_vec() } else { clean[..18].to_vec() };
    let (mut acc, mut bits, mut bytes) = (0u32, 0, Vec::new());
    for c in clean {
        let c = match c {
            b'0' => b'O',
            b'1' => b'I',
            c => c,
        };
        let v = ALPHABET.iter().position(|&a| a == c).ok_or_else(bad)? as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            bytes.push((acc >> bits) as u8);
        }
    }
    // 23 bytes: IPv6 · 11 bytes: IPv4
    let (host, rest) = if bytes.len() >= 23 {
        let mut o = [0u8; 16];
        o.copy_from_slice(&bytes[..16]);
        (format!("[{}]", Ipv6Addr::from(o)), &bytes[16..23])
    } else if bytes.len() >= 11 {
        (Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3]).to_string(), &bytes[4..11])
    } else {
        return Err(bad());
    };
    let port = u16::from_be_bytes([rest[0], rest[1]]);
    let key: String = rest[2..7].iter().map(|x| format!("{x:02x}")).collect();
    Ok((format!("http://{host}:{port}"), key))
}

/// The address other computers on the network reach this one at (nothing is sent).
fn local_ip() -> Ipv4Addr {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|_| s))
        .and_then(|s| s.local_addr())
        .ok()
        .and_then(|a| match a.ip() {
            IpAddr::V4(ip) if !ip.is_unspecified() => Some(ip),
            _ => None,
        })
        .unwrap_or(Ipv4Addr::LOCALHOST)
}

/// This computer's public IPv6 address, if it has one. Many providers that share one IPv4
/// among customers (CGNAT) give each home its own IPv6, reachable from anywhere.
fn global_ipv6() -> Option<Ipv6Addr> {
    let s = UdpSocket::bind("[::]:0").ok()?;
    s.connect("[2001:4860:4860::8888]:9").ok()?; // picks the route; nothing is sent
    match s.local_addr().ok()?.ip() {
        IpAddr::V6(ip) if (ip.segments()[0] & 0xe000) == 0x2000 => Some(ip), // 2000::/3 = global
        _ => None,
    }
}

/* ───────────── Hosting ───────────── */

/// What the host's player is doing, sent by its UI on every change
#[derive(Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HostState {
    pub track: Option<u32>,
    pub pos_ms: u64,
    pub paused: bool,
    #[serde(default)]
    pub queue: Vec<u32>,
    #[serde(default)]
    pub allow_requests: bool,
    /// The current song's cover (a file of the image cache)
    #[serde(default)]
    pub cover: Option<String>,
    /// "Lighter" quality: lossless songs go out as MP3 320 kbps
    #[serde(default)]
    pub compress: bool,
    /// Guests may send songs from their own libraries
    #[serde(default)]
    pub allow_shares: bool,
}

struct Guest {
    name: String,
    web: bool,
    seen: Instant,
}

struct Live {
    seq: u64,
    at: u64,
    state: HostState,
    ended: bool,
    name: String,
    songs: HashMap<u32, Song>,
    order: Vec<u32>,
    guests: HashMap<String, Guest>,
    /// Numbers for the songs guests send (far from the library's)
    next_shared: u32,
}

type Emit = Arc<dyn Fn(Value) + Send + Sync>;

struct Ctx {
    /// The key in the code: never sent, only used to sign
    token: String,
    /// The key in the web link
    web_token: String,
    /// Signatures already used (each works once)
    seen: Mutex<HashMap<String, Instant>>,
    /// Wrong keys per address: (how many, since when)
    misses: Mutex<HashMap<IpAddr, (u32, Instant)>>,
    live: Mutex<Live>,
    cv: Condvar,
    emit: Emit,
}

struct Host {
    ctx: Arc<Ctx>,
    servers: Vec<Arc<Server>>,
    port: u16,
    stop: Arc<AtomicBool>,
}

static HOST: Mutex<Option<Host>> = Mutex::new(None);
static UPNP_PORT: Mutex<Option<u16>> = Mutex::new(None);

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub port: u16,
    pub code: String,
    pub link: String,
    pub ip: String,
    /// Reachable over IPv6 (when the computer has a public IPv6 address)
    pub ipv6_code: Option<String>,
    pub ipv6_link: Option<String>,
}

fn catalog(songs: Vec<Song>) -> (HashMap<u32, Song>, Vec<u32>) {
    let order = songs.iter().map(|s| s.id).collect();
    let map = songs
        .into_iter()
        .map(|mut s| {
            s.ext = Path::new(&s.path).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            (s.id, s)
        })
        .collect();
    (map, order)
}

/// Starts serving. `port` 0 = the first free one from 47800.
pub fn start_host(name: &str, songs: Vec<Song>, port: u16, emit: Emit) -> Result<HostInfo, String> {
    stop_host();
    let ports: Vec<u16> = if port != 0 { vec![port] } else { (FIRST_PORT..FIRST_PORT + 20).collect() };
    // IPv6 first: on macOS and Linux that socket takes IPv4 too (then the IPv4 one can't open,
    // which is fine); on Windows each needs its own
    let (servers, port, v6) = ports
        .iter()
        .find_map(|&p| {
            let v6 = Server::http(SocketAddr::from((Ipv6Addr::UNSPECIFIED, p))).ok();
            let v4 = Server::http(SocketAddr::from(([0, 0, 0, 0], p))).ok();
            let has_v6 = v6.is_some();
            if v4.is_none() && (cfg!(windows) || !has_v6) {
                return None; // IPv4 has to work
            }
            Some((v6.into_iter().chain(v4).map(Arc::new).collect::<Vec<_>>(), p, has_v6))
        })
        .ok_or("Couldn't open a port for the session")?;
    let token = random_hex(5);
    let web_token = random_hex(8);
    let (songs, order) = catalog(songs);
    let ctx = Arc::new(Ctx {
        token: token.clone(),
        web_token: web_token.clone(),
        seen: Mutex::new(HashMap::new()),
        misses: Mutex::new(HashMap::new()),
        live: Mutex::new(Live {
            seq: 1,
            at: now_ms(),
            state: HostState::default(),
            ended: false,
            name: if name.trim().is_empty() { "Wax".into() } else { name.trim().to_string() },
            songs,
            order,
            guests: HashMap::new(),
            next_shared: SHARED_IDS,
        }),
        cv: Condvar::new(),
        emit,
    });
    let stop = Arc::new(AtomicBool::new(false));
    for server in &servers {
        let (server, ctx) = (server.clone(), ctx.clone());
        thread::spawn(move || {
            for req in server.incoming_requests() {
                let ctx = ctx.clone();
                thread::spawn(move || handle(req, &ctx));
            }
        });
    }
    {
        // Forgets guests that stopped asking (closed the app, lost the connection)
        let (ctx, stop) = (ctx.clone(), stop.clone());
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_secs(5));
                let mut l = ctx.live.lock().unwrap();
                let before = l.guests.len();
                l.guests.retain(|_, g| g.seen.elapsed() < Duration::from_secs(45));
                if l.guests.len() != before {
                    let list = guest_list(&l);
                    drop(l);
                    (ctx.emit)(json!({ "kind": "guests", "guests": list }));
                }
            }
        });
    }
    let ip = local_ip();
    let v6 = if v6 { global_ipv6() } else { None };
    let info = HostInfo {
        port,
        code: make_code(IpAddr::V4(ip), port, &token),
        link: format!("http://{ip}:{port}/?k={web_token}"),
        ip: ip.to_string(),
        ipv6_code: v6.map(|v| make_code(IpAddr::V6(v), port, &token)),
        ipv6_link: v6.map(|v| format!("http://[{v}]:{port}/?k={web_token}")),
    };
    ENCODES.lock().unwrap().clear();
    let _ = fs::remove_dir_all(host_cache());
    *HOST.lock().unwrap() = Some(Host { ctx, servers, port, stop });
    Ok(info)
}

pub fn stop_host() {
    let Some(h) = HOST.lock().unwrap().take() else { return };
    h.stop.store(true, Ordering::Relaxed);
    {
        let mut l = h.ctx.live.lock().unwrap();
        l.ended = true;
        l.seq += 1;
    }
    h.ctx.cv.notify_all();
    // Give the guests waiting for news a moment to hear that it's over
    thread::sleep(Duration::from_millis(300));
    for s in &h.servers {
        s.unblock();
    }
    ENCODES.lock().unwrap().clear();
    let _ = fs::remove_dir_all(host_cache());
    if *UPNP_PORT.lock().unwrap() == Some(h.port) {
        thread::spawn(move || upnp_close(h.port));
    }
}

fn with_live<R>(f: impl FnOnce(&mut Live) -> R) -> Option<R> {
    let g = HOST.lock().unwrap();
    let h = g.as_ref()?;
    let r = f(&mut h.ctx.live.lock().unwrap());
    h.ctx.cv.notify_all();
    Some(r)
}

pub fn update_host(state: HostState) {
    let soon = with_live(|l| {
        l.state = state;
        l.at = now_ms();
        l.seq += 1;
        // Lighter quality: the songs about to be needed are converted ahead of time
        if !l.state.compress {
            return Vec::new();
        }
        let ids = l.state.track.iter().chain(l.state.queue.iter().take(2));
        ids.filter_map(|id| l.songs.get(id)).filter(|s| shrinks(s)).map(|s| s.path.clone()).collect()
    });
    for path in soon.unwrap_or_default() {
        start_encode(&path);
    }
}

fn guest_list(l: &Live) -> Vec<Value> {
    let mut v: Vec<_> = l.guests.values().map(|g| json!({ "name": g.name, "web": g.web })).collect();
    v.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    v
}

fn snapshot(l: &Live) -> Value {
    let song = |id: &u32| {
        l.songs.get(id).map(|s| {
            let mut v = json!(s);
            if l.state.compress && shrinks(s) {
                v["ext"] = json!("mp3");
            }
            v
        })
    };
    json!({
        "seq": l.seq,
        "at": l.at,
        "now": now_ms(),
        "ended": l.ended,
        "name": l.name,
        "track": l.state.track.as_ref().and_then(song),
        "posMs": l.state.pos_ms,
        "paused": l.state.paused,
        "queue": l.state.queue.iter().take(30).filter_map(song).collect::<Vec<_>>(),
        "allowRequests": l.state.allow_requests,
        "allowShares": l.state.allow_shares,
        "compress": l.state.compress,
        "guests": guest_list(l),
    })
}

/* ───────────── Lighter quality: MP3 320 kbps ───────────── */

const LOSSLESS: [&str; 7] = ["flac", "wav", "aif", "aiff", "ape", "wv", "tta"];

/// Lossless songs get about 3 times smaller as MP3. CUE tracks stay as they are (the file is a whole album).
fn shrinks(s: &Song) -> bool {
    s.start_ms == 0 && s.end_ms == 0 && LOSSLESS.contains(&s.ext.as_str())
}

fn host_cache() -> PathBuf {
    data_dir().join("party-host")
}

/// A conversion: None while it runs
type Encode = Arc<(Mutex<Option<Result<PathBuf, String>>>, Condvar)>;
static ENCODES: Mutex<Vec<(String, Encode)>> = Mutex::new(Vec::new());

fn start_encode(path: &str) -> Encode {
    let mut all = ENCODES.lock().unwrap();
    if let Some((_, e)) = all.iter().find(|(p, _)| p == path) {
        return e.clone();
    }
    let e: Encode = Arc::new((Mutex::new(None), Condvar::new()));
    all.push((path.to_string(), e.clone()));
    // Only the latest few stay on disk
    while all.len() > 12 {
        let (_, old) = all.remove(0);
        let done = old.0.lock().unwrap().clone();
        if let Some(Ok(f)) = done {
            let _ = fs::remove_file(f);
        }
    }
    drop(all);
    let (path, job) = (path.to_string(), e.clone());
    thread::spawn(move || {
        let to = host_cache().join(format!("{}.mp3", random_hex(6)));
        let r = crate::library::encode_mp3(&path, &to, 320).map(|_| to);
        *job.0.lock().unwrap() = Some(r);
        job.1.notify_all();
    });
    e
}

/// The MP3 of a song (waits for the conversion: a few seconds)
fn encoded(path: &str) -> Result<PathBuf, String> {
    let e = start_encode(path);
    let mut g = e.0.lock().unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    while g.is_none() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("timeout".into());
        }
        g = e.1.wait_timeout(g, left).unwrap().0;
    }
    g.clone().unwrap()
}

/* ───────────── The HTTP side ───────────── */

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = |c: u8| (c as char).to_digit(16);
                match (hex(b[i + 1]), hex(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn pct_encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn header(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap()
}

fn reply_json(req: Request, status: u16, v: &Value) {
    let r = Response::from_string(v.to_string())
        .with_status_code(status)
        .with_header(header("Content-Type", "application/json; charset=utf-8"))
        .with_header(header("Cache-Control", "no-store"));
    let _ = req.respond(r);
}

fn mime(ext: &str) -> &'static str {
    match ext {
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "m4a" | "mp4" | "aac" | "alac" => "audio/mp4",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "webm" | "mka" => "audio/webm",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

/// "bytes=a-b" → the inclusive range inside a file of `len` bytes
fn parse_range(v: &str, len: u64) -> Option<(u64, u64)> {
    let (a, b) = v.trim().strip_prefix("bytes=")?.split(',').next()?.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        (len.saturating_sub(n), len.checked_sub(1)?)
    } else {
        let s: u64 = a.parse().ok()?;
        let e = if b.is_empty() { len.checked_sub(1)? } else { b.parse::<u64>().ok()?.min(len.checked_sub(1)?) };
        (s, e)
    };
    (start <= end && start < len).then_some((start, end))
}

fn serve_file(req: Request, path: &Path) {
    let Ok(mut f) = File::open(path) else {
        return reply_json(req, 404, &json!({ "error": "missing" }));
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let range = req.headers().iter().find(|h| h.field.equiv("Range")).and_then(|h| parse_range(h.value.as_str(), len));
    let base = [header("Content-Type", mime(&ext)), header("Accept-Ranges", "bytes"), header("Cache-Control", "no-store")];
    let resp = match range {
        Some((a, b)) if f.seek(SeekFrom::Start(a)).is_ok() => {
            let mut h = base.to_vec();
            h.push(header("Content-Range", &format!("bytes {a}-{b}/{len}")));
            Response::new(StatusCode(206), h, Box::new(f.take(b - a + 1)) as Box<dyn Read + Send>, Some((b - a + 1) as usize), None)
        }
        _ => Response::new(StatusCode(200), base.to_vec(), Box::new(f) as Box<dyn Read + Send>, Some(len as usize), None),
    };
    let _ = req.respond(resp);
}

#[derive(PartialEq, Debug)]
enum Access {
    /// Anyone (the clock)
    Open,
    /// The web link's key
    Web,
    /// A request signed with the code's key (Wax)
    App,
}

const WINDOW_MS: u64 = 120_000;

/// Who's asking. Wrong keys count against the address: 20 in 10 minutes and it's blocked.
fn authorize(ctx: &Ctx, url: &str, path: &str, web_key: &str, time: &str, ip: Option<IpAddr>) -> Result<Access, u16> {
    if path == "/api/time" {
        return Ok(Access::Open);
    }
    let blocked = ip.is_some_and(|ip| ctx.misses.lock().unwrap().get(&ip).is_some_and(|(n, since)| *n >= 20 && since.elapsed() < Duration::from_secs(600)));
    if blocked {
        return Err(429);
    }
    if !web_key.is_empty() && same(web_key, &ctx.web_token) {
        return Ok(Access::Web);
    }
    // "…&t=<host time>&s=<signature of everything before &s=>"
    if let Some((unsigned, sig)) = url.rsplit_once("&s=") {
        let fresh = time.parse::<u64>().is_ok_and(|t| now_ms().abs_diff(t) < WINDOW_MS);
        let key: Vec<u8> = (0..ctx.token.len() / 2).filter_map(|i| u8::from_str_radix(&ctx.token[i * 2..i * 2 + 2], 16).ok()).collect();
        if fresh && same(sig, &hex(&hmac(&key, unsigned.as_bytes()))) {
            let mut seen = ctx.seen.lock().unwrap();
            seen.retain(|_, at| at.elapsed() < Duration::from_millis(WINDOW_MS * 2));
            if seen.insert(sig.to_string(), Instant::now()).is_none() {
                return Ok(Access::App);
            }
        }
    }
    if let Some(ip) = ip {
        let mut misses = ctx.misses.lock().unwrap();
        let e = misses.entry(ip).or_insert((0, Instant::now()));
        if e.1.elapsed() > Duration::from_secs(600) {
            *e = (0, Instant::now());
        }
        e.0 += 1;
    }
    Err(403)
}

fn handle(req: Request, ctx: &Ctx) {
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let q: HashMap<String, String> = query.split('&').filter_map(|p| p.split_once('=').map(|(k, v)| (k.to_string(), pct_decode(v)))).collect();
    let arg = |k: &str| q.get(k).map(String::as_str).unwrap_or("");
    let ip = req.remote_addr().map(|a| a.ip());
    let access = match authorize(ctx, &url, path, arg("k"), arg("t"), ip) {
        Ok(a) => a,
        Err(code) => {
            let msg = if code == 429 { "Too many wrong keys. Try again in a few minutes." } else { "This session has ended or the link is wrong." };
            let _ = req.respond(Response::from_string(msg).with_status_code(code));
            return;
        }
    };
    let guest = arg("g").to_string();
    let touch = |l: &mut Live| {
        if let Some(g) = l.guests.get_mut(&guest) {
            g.seen = Instant::now();
        }
    };
    match path {
        "/" | "/index.html" => {
            let r = Response::from_string(WEB_PAGE).with_header(header("Content-Type", "text/html; charset=utf-8")).with_header(header("Cache-Control", "no-store"));
            let _ = req.respond(r);
        }
        "/api/time" => reply_json(req, 200, &json!({ "now": now_ms() })),
        "/api/join" => {
            let name: String = arg("name").trim().chars().take(40).collect();
            let id = random_hex(6);
            let (list, snap) = {
                let mut l = ctx.live.lock().unwrap();
                l.guests.insert(id.clone(), Guest { name: if name.is_empty() { "Guest".into() } else { name }, web: arg("web") == "1", seen: Instant::now() });
                (guest_list(&l), snapshot(&l))
            };
            (ctx.emit)(json!({ "kind": "guests", "guests": list }));
            reply_json(req, 200, &json!({ "guest": id, "state": snap }));
        }
        "/api/leave" => {
            let list = {
                let mut l = ctx.live.lock().unwrap();
                l.guests.remove(&guest);
                guest_list(&l)
            };
            (ctx.emit)(json!({ "kind": "guests", "guests": list }));
            reply_json(req, 200, &json!({ "ok": true }));
        }
        "/api/state" => {
            // Waits (up to 20 s) until something changes after `since`
            let since: u64 = arg("since").parse().unwrap_or(0);
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut l = ctx.live.lock().unwrap();
            touch(&mut l);
            while l.seq <= since && !l.ended {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                l = ctx.cv.wait_timeout(l, left).unwrap().0;
            }
            touch(&mut l);
            let snap = snapshot(&l);
            drop(l);
            reply_json(req, 200, &snap);
        }
        "/api/search" => {
            let words: Vec<String> = arg("q").to_lowercase().split_whitespace().map(String::from).collect();
            let l = ctx.live.lock().unwrap();
            let hits: Vec<&Song> = if words.is_empty() {
                Vec::new()
            } else {
                l.order
                    .iter()
                    .filter_map(|id| l.songs.get(id))
                    .filter(|s| {
                        let hay = format!("{} {} {}", s.title, s.artist, s.album).to_lowercase();
                        words.iter().all(|w| hay.contains(w.as_str()))
                    })
                    .take(60)
                    .collect()
            };
            let v = json!(hits);
            drop(l);
            reply_json(req, 200, &v);
        }
        "/api/upload" if access != Access::App => reply_json(req, 403, &json!({ "error": "join with the code to send songs" })),
        "/api/upload" => {
            // A guest sends a song of their own library: it joins the host's queue
            let mut req = req;
            let (allowed, from) = {
                let mut l = ctx.live.lock().unwrap();
                touch(&mut l);
                (l.state.allow_shares, l.guests.get(&guest).map(|g| g.name.clone()))
            };
            let Some(from) = from else {
                return reply_json(req, 403, &json!({ "error": "join first" }));
            };
            if !allowed {
                return reply_json(req, 403, &json!({ "error": "sharing is off" }));
            }
            let len = req.body_length().unwrap_or(0);
            if len == 0 || len > MAX_SHARE {
                return reply_json(req, 413, &json!({ "error": "too big" }));
            }
            let ext: String = arg("ext").chars().filter(|c| c.is_ascii_alphanumeric()).take(5).collect::<String>().to_lowercase();
            let ext = if ext.is_empty() { "bin".to_string() } else { ext };
            let _ = fs::create_dir_all(host_cache());
            let path = host_cache().join(format!("shared-{}.{ext}", random_hex(6)));
            let saved = File::create(&path).and_then(|mut f| {
                let n = std::io::copy(&mut req.as_reader().take(len as u64), &mut f)?;
                if n == len as u64 {
                    Ok(())
                } else {
                    Err(std::io::Error::other("cut short"))
                }
            });
            if saved.is_err() {
                let _ = fs::remove_file(&path);
                return reply_json(req, 500, &json!({ "error": "couldn't save it" }));
            }
            let clip = |k: &str| arg(k).chars().take(200).collect::<String>();
            let song = {
                let mut l = ctx.live.lock().unwrap();
                let id = l.next_shared;
                l.next_shared += 1;
                let song = Song {
                    id,
                    title: clip("title"),
                    artist: clip("artist"),
                    album: clip("album"),
                    duration: arg("duration").parse().unwrap_or(0.0),
                    path: path.to_string_lossy().into_owned(),
                    ext,
                    from,
                    ..Default::default()
                };
                l.songs.insert(id, song.clone());
                song
            };
            (ctx.emit)(json!({ "kind": "shared", "song": song, "path": song.path }));
            reply_json(req, 200, &json!({ "ok": true, "id": song.id }));
        }
        "/api/request" => {
            let id: u32 = arg("id").parse().unwrap_or(u32::MAX);
            let (ok, from) = {
                let mut l = ctx.live.lock().unwrap();
                touch(&mut l);
                let from = l.guests.get(&guest).map(|g| g.name.clone()).unwrap_or_else(|| "A guest".into());
                (l.state.allow_requests && l.songs.contains_key(&id), from)
            };
            if ok {
                (ctx.emit)(json!({ "kind": "request", "id": id, "from": from }));
            }
            reply_json(req, if ok { 200 } else { 403 }, &json!({ "ok": ok }));
        }
        p if p.starts_with("/api/file/") => {
            // Only the song playing and the ones queued: guests can't copy the whole library
            let id: u32 = p[10..].parse().unwrap_or(u32::MAX);
            let file = {
                let l = ctx.live.lock().unwrap();
                let shared = l.state.track == Some(id) || l.state.queue.iter().take(30).any(|&q| q == id);
                shared.then(|| l.songs.get(&id).map(|s| (s.path.clone(), l.state.compress && shrinks(s)))).flatten()
            };
            match file {
                Some((f, true)) => match encoded(&f) {
                    Ok(mp3) => serve_file(req, &mp3),
                    Err(_) => serve_file(req, Path::new(&f)), // the conversion failed: the original then
                },
                Some((f, false)) => serve_file(req, Path::new(&f)),
                None => reply_json(req, 404, &json!({ "error": "not playing or queued" })),
            }
        }
        p if p.starts_with("/api/cover/") => {
            // Only the cover of the song playing now
            let id: u32 = p[11..].parse().unwrap_or(u32::MAX);
            let file = {
                let l = ctx.live.lock().unwrap();
                (l.state.track == Some(id)).then(|| l.state.cover.clone()).flatten()
            };
            match file {
                Some(f) => serve_file(req, Path::new(&f)),
                None => reply_json(req, 404, &json!({ "error": "no cover" })),
            }
        }
        _ => reply_json(req, 404, &json!({ "error": "unknown" })),
    }
}

/* ───────────── Opening the port on the router (UPnP) ───────────── */

/// Private, CGNAT or otherwise unreachable from the internet
fn not_public(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || (o[0] == 100 && (64..128).contains(&o[1]))
}

fn upnp_open(port: u16) -> Result<Ipv4Addr, String> {
    use igd_next::{PortMappingProtocol, SearchOptions};
    let gw = igd_next::search_gateway(SearchOptions { timeout: Some(Duration::from_secs(4)), ..Default::default() })
        .map_err(|_| "Your router didn't answer (UPnP is off or not supported).".to_string())?;
    let ip = match gw.get_external_ip() {
        Ok(IpAddr::V4(ip)) => ip,
        _ => return Err("Your router didn't say its internet address.".into()),
    };
    if not_public(ip) {
        return Err("Your internet provider shares one address among many customers (CGNAT), so people outside your network can't reach you.".into());
    }
    gw.add_port(PortMappingProtocol::TCP, port, SocketAddr::new(IpAddr::V4(local_ip()), port), 4 * 3600, "Wax: Listen Together")
        .map_err(|e| format!("Your router didn't open the port ({e})."))?;
    *UPNP_PORT.lock().unwrap() = Some(port);
    Ok(ip)
}

fn upnp_close(port: u16) {
    use igd_next::{PortMappingProtocol, SearchOptions};
    if let Ok(gw) = igd_next::search_gateway(SearchOptions { timeout: Some(Duration::from_secs(3)), ..Default::default() }) {
        let _ = gw.remove_port(PortMappingProtocol::TCP, port);
    }
    let mut p = UPNP_PORT.lock().unwrap();
    if *p == Some(port) {
        *p = None;
    }
}

/* ───────────── Joining ───────────── */

struct Conn {
    base: String,
    key: String,
    guest: String,
    /// Joined with a web link (its key goes in the address) rather than a code (signed requests)
    web: bool,
    /// The host's clock minus ours, for the signatures' time
    offset: i64,
}

impl Conn {
    fn url(&self, path: &str, extra: &str) -> String {
        if self.web {
            return format!("{}{path}?k={}&g={}{extra}", self.base, self.key, self.guest);
        }
        let t = (now_ms() as i64 + self.offset).max(0);
        let unsigned = format!("{path}?g={}{extra}&t={t}", self.guest);
        let key: Vec<u8> = (0..self.key.len() / 2).filter_map(|i| u8::from_str_radix(&self.key[i * 2..i * 2 + 2], 16).ok()).collect();
        format!("{}{unsigned}&s={}", self.base, hex(&hmac(&key, unsigned.as_bytes())))
    }
}

static CONN: Mutex<Option<Conn>> = Mutex::new(None);

fn agent() -> ureq::Agent {
    static A: OnceLock<ureq::Agent> = OnceLock::new();
    A.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(6)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .user_agent(crate::USER_AGENT)
            .build()
            .into()
    })
    .clone()
}

fn conn_url(path: &str, extra: &str) -> Result<String, String> {
    let c = CONN.lock().unwrap();
    let c = c.as_ref().ok_or("You aren't in a session")?;
    Ok(c.url(path, extra))
}

fn get_json(url: &str) -> Result<Value, String> {
    let mut r = agent().get(url).call().map_err(|e| match e {
        ureq::Error::StatusCode(403) => "The session has ended or the code is wrong.".to_string(),
        ureq::Error::StatusCode(429) => "Too many wrong codes from here. Wait a few minutes and try again.".to_string(),
        ureq::Error::StatusCode(c) => format!("The host answered {c}"),
        _ => "Couldn't reach the host. Check the code, and that you're on the same network (or that the host's internet invite is on).".to_string(),
    })?;
    r.body_mut().read_json().map_err(|e| e.to_string())
}

fn cache_dir() -> PathBuf {
    data_dir().join("party")
}

pub fn join(input: &str, name: &str) -> Result<Value, String> {
    leave();
    let (base, key) = parse_invite(input)?;
    let web = input.contains("?k=");
    // The host's clock first: signatures carry its time
    let t0 = now_ms();
    let host_now = get_json(&format!("{base}/api/time"))?["now"].as_u64().unwrap_or(t0);
    let offset = host_now as i64 - ((t0 + now_ms()) / 2) as i64;
    let mut conn = Conn { base, key, guest: String::new(), web, offset };
    let v = get_json(&conn.url("/api/join", &format!("&name={}", pct_encode(name))))?;
    conn.guest = v["guest"].as_str().ok_or("The host didn't answer as expected")?.to_string();
    let _ = fs::remove_dir_all(cache_dir());
    let _ = fs::create_dir_all(cache_dir());
    *CONN.lock().unwrap() = Some(conn);
    Ok(v["state"].clone())
}

#[cfg(test)]
fn leave_downloads() {
    DLS.lock().unwrap().clear();
}

pub fn leave() {
    let Some(c) = CONN.lock().unwrap().take() else { return };
    let url = c.url("/api/leave", "");
    thread::spawn(move || {
        let _ = agent().get(&url).call();
    });
    DLS.lock().unwrap().clear();
    let _ = fs::remove_dir_all(cache_dir());
}

/// The host's clock minus ours, from the quickest of a few round trips
pub fn clock() -> Result<(i64, u64), String> {
    let url = conn_url("/api/time", "")?;
    let mut best: Option<(i64, u64)> = None;
    for _ in 0..4 {
        let t0 = now_ms();
        let v = get_json(&url)?;
        let t1 = now_ms();
        let rtt = t1 - t0;
        let host = v["now"].as_u64().unwrap_or(t1);
        let offset = host as i64 - ((t0 + t1) / 2) as i64;
        if best.is_none_or(|(_, r)| rtt < r) {
            best = Some((offset, rtt));
        }
    }
    best.ok_or_else(|| "No answer".into())
}

/* Songs being copied from the host. The engine can start playing one before it's all here:
reading past what has arrived waits for it (see GrowingFile). */

pub struct Download {
    path: PathBuf,
    total: AtomicU64,
    done: AtomicU64,
    finished: AtomicBool,
    failed: AtomicBool,
}

static DLS: Mutex<Vec<(u32, Arc<Download>)>> = Mutex::new(Vec::new());
const KEEP: usize = 8;

fn start_download(song: &Song) -> Result<Arc<Download>, String> {
    let mut dls = DLS.lock().unwrap();
    if let Some((_, d)) = dls.iter().find(|(id, d)| *id == song.id && !d.failed.load(Ordering::Relaxed)) {
        return Ok(d.clone());
    }
    let ext = if song.ext.is_empty() { "bin".to_string() } else { song.ext.clone() };
    let path = cache_dir().join(format!("{}-{}.{ext}", song.id, random_hex(3)));
    let d = Arc::new(Download { path, total: AtomicU64::new(0), done: AtomicU64::new(0), finished: AtomicBool::new(false), failed: AtomicBool::new(false) });
    dls.retain(|(id, _)| *id != song.id);
    dls.push((song.id, d.clone()));
    // Only the latest few songs stay on disk
    while dls.len() > KEEP {
        let (_, old) = dls.remove(0);
        let _ = fs::remove_file(&old.path);
    }
    drop(dls);
    let url = conn_url(&format!("/api/file/{}", song.id), "")?;
    let dl = d.clone();
    thread::spawn(move || {
        let ok = (|| -> Result<(), String> {
            let _ = fs::create_dir_all(cache_dir());
            let resp = ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(8)))
                .timeout_recv_response(Some(Duration::from_secs(20)))
                .user_agent(crate::USER_AGENT)
                .build()
                .new_agent()
                .get(&url)
                .call()
                .map_err(|e| e.to_string())?;
            let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
            dl.total.store(total, Ordering::Relaxed);
            let mut body = resp.into_body().into_reader();
            let mut out = File::create(&dl.path).map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = body.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                dl.done.fetch_add(n as u64, Ordering::Relaxed);
            }
            Ok(())
        })();
        match ok {
            Ok(()) => dl.finished.store(true, Ordering::Relaxed),
            Err(_) => dl.failed.store(true, Ordering::Relaxed),
        }
    });
    Ok(d)
}

/// Starts copying the song and waits until enough of it is here to start at `ratio` of its length
/// (a negative ratio doesn't wait). Returns the local file.
pub fn fetch(song: &Song, ratio: f64) -> Result<String, String> {
    let d = start_download(song)?;
    let path = d.path.to_string_lossy().into_owned();
    if ratio < 0.0 {
        return Ok(path);
    }
    let started = Instant::now();
    loop {
        if d.failed.load(Ordering::Relaxed) {
            return Err("Couldn't get the song from the host".into());
        }
        if d.finished.load(Ordering::Relaxed) {
            return Ok(path);
        }
        let (total, done) = (d.total.load(Ordering::Relaxed), d.done.load(Ordering::Relaxed));
        if total > 0 && done >= ((total as f64 * ratio.min(1.0)) as u64 + 512 * 1024).min(total) {
            return Ok(path);
        }
        if started.elapsed() > Duration::from_secs(90) {
            return Err("The host's connection is too slow".into());
        }
        thread::sleep(Duration::from_millis(40));
    }
}

/// How much of the song has arrived (0–1), for the "Loading…" text
pub fn progress(id: u32) -> f64 {
    let dls = DLS.lock().unwrap();
    dls.iter().find(|(i, _)| *i == id).map_or(0.0, |(_, d)| {
        if d.finished.load(Ordering::Relaxed) {
            1.0
        } else {
            let t = d.total.load(Ordering::Relaxed);
            if t == 0 {
                0.0
            } else {
                d.done.load(Ordering::Relaxed) as f64 / t as f64
            }
        }
    })
}

/// The download a file the engine is opening belongs to (if it's still arriving)
pub fn growing(path: &str) -> Option<Arc<Download>> {
    let dls = DLS.lock().unwrap();
    dls.iter().map(|(_, d)| d).find(|d| d.path == Path::new(path) && !d.finished.load(Ordering::Relaxed)).cloned()
}

/// A file that's still being written: reading past its end waits for more instead of stopping.
pub struct GrowingFile {
    file: File,
    dl: Arc<Download>,
    pos: u64,
}

impl GrowingFile {
    pub fn new(file: File, dl: Arc<Download>) -> GrowingFile {
        GrowingFile { file, dl, pos: 0 }
    }
    fn wait_for(&self, upto: u64) {
        let start = Instant::now();
        while self.dl.done.load(Ordering::Relaxed) < upto
            && !self.dl.finished.load(Ordering::Relaxed)
            && !self.dl.failed.load(Ordering::Relaxed)
            && start.elapsed() < Duration::from_secs(30)
        {
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Read for GrowingFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let total = self.dl.total.load(Ordering::Relaxed);
        if total > 0 && self.pos >= total {
            return Ok(0);
        }
        self.wait_for(self.pos + 1);
        let avail = self.dl.done.load(Ordering::Relaxed).saturating_sub(self.pos);
        let want = if self.dl.finished.load(Ordering::Relaxed) { buf.len() } else { buf.len().min(avail as usize) };
        let n = self.file.read(&mut buf[..want])?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for GrowingFile {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let total = self.dl.total.load(Ordering::Relaxed);
        let target = match to {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::Current(d) => self.pos as i64 + d,
            SeekFrom::End(d) => total as i64 + d,
        };
        if target < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before the start"));
        }
        self.pos = self.file.seek(SeekFrom::Start(target as u64))?;
        Ok(self.pos)
    }
}

impl symphonia::core::io::MediaSource for GrowingFile {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.dl.total.load(Ordering::Relaxed)).filter(|&t| t > 0)
    }
}

/* ───────────── Commands ───────────── */

#[tauri::command]
pub async fn party_host(app: AppHandle, name: String, songs: Vec<Song>) -> Result<HostInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let emit_app = app.clone();
        let info = start_host(
            &name,
            songs,
            0,
            Arc::new(move |v| {
                let _ = emit_app.emit("party", v);
            }),
        )?;
        // People outside the network: ask the router to open the port (takes a few seconds)
        let port = info.port;
        let (key, web) = HOST.lock().unwrap().as_ref().map(|h| (h.ctx.token.clone(), h.ctx.web_token.clone())).unwrap_or_default();
        thread::spawn(move || {
            let r = upnp_open(port);
            let still = HOST.lock().unwrap().as_ref().is_some_and(|h| h.port == port);
            if !still {
                if r.is_ok() {
                    upnp_close(port);
                }
                return;
            }
            let _ = app.emit(
                "party",
                match r {
                    Ok(ip) => json!({ "kind": "internet", "ok": true, "code": make_code(IpAddr::V4(ip), port, &key), "link": format!("http://{ip}:{port}/?k={web}") }),
                    Err(e) => json!({ "kind": "internet", "ok": false, "message": e, "port": port }),
                },
            );
        });
        Ok(info)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn party_update(state: HostState) {
    update_host(state);
}

#[tauri::command]
pub fn party_catalog(songs: Vec<Song>) {
    let (mut map, order) = catalog(songs);
    with_live(|l| {
        map.extend(l.songs.drain().filter(|(id, _)| *id >= SHARED_IDS));
        l.songs = map;
        l.order = order;
        l.seq += 1;
    });
}

#[tauri::command]
pub async fn party_stop() {
    let _ = tauri::async_runtime::spawn_blocking(stop_host).await;
}

#[tauri::command]
pub async fn party_join(code: String, name: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || join(&code, &name)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn party_leave() {
    let _ = tauri::async_runtime::spawn_blocking(leave).await;
}

#[tauri::command]
pub async fn party_poll(since: u64) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || get_json(&conn_url("/api/state", &format!("&since={since}"))?)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn party_clock() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| clock().map(|(offset, rtt)| json!({ "offset": offset, "rtt": rtt }))).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn party_fetch(song: Song, ratio: f64) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || fetch(&song, ratio)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn party_progress(id: u32) -> f64 {
    progress(id)
}

#[tauri::command]
pub async fn party_cover(id: u32) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let url = conn_url(&format!("/api/cover/{id}"), "").ok()?;
        let resp = agent().get(&url).call().ok()?;
        let ext = match resp.headers().get("content-type").and_then(|v| v.to_str().ok()) {
            Some("image/png") => "png",
            Some("image/webp") => "webp",
            _ => "jpg",
        };
        let bytes = resp.into_body().read_to_vec().ok()?;
        let path = cache_dir().join(format!("cover-{id}.{ext}"));
        fs::write(&path, bytes).ok()?;
        Some(path.to_string_lossy().into_owned())
    })
    .await
    .ok()
    .flatten()
}

#[tauri::command]
pub async fn party_search(query: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || get_json(&conn_url("/api/search", &format!("&q={}", pct_encode(&query)))?)).await.map_err(|e| e.to_string())?
}

/// Sends one of this guest's songs to the host (as MP3 320 when the host asks for lighter files)
pub fn share(path: &str, title: &str, artist: &str, album: &str, duration: f64, compress: bool) -> Result<u32, String> {
    let ext = Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let lossless = LOSSLESS.contains(&ext.as_str());
    let tmp = cache_dir().join(format!("send-{}.mp3", random_hex(4)));
    let (file, ext) = if compress && lossless {
        crate::library::encode_mp3(path, &tmp, 320)?;
        (tmp.clone(), "mp3".to_string())
    } else {
        (PathBuf::from(path), ext)
    };
    let bytes = fs::read(&file).map_err(|e| format!("Couldn't read the song: {e}"));
    let _ = fs::remove_file(&tmp);
    let bytes = bytes?;
    if bytes.len() > MAX_SHARE {
        return Err("The song is too big to send".into());
    }
    let query = format!("&title={}&artist={}&album={}&duration={duration}&ext={}", pct_encode(title), pct_encode(artist), pct_encode(album), pct_encode(&ext));
    let url = conn_url("/api/upload", &query)?;
    let mut r = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(8)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .user_agent(crate::USER_AGENT)
        .build()
        .new_agent()
        .post(&url)
        .send(&bytes[..])
        .map_err(|e| match e {
            ureq::Error::StatusCode(403) if CONN.lock().unwrap().as_ref().is_some_and(|c| c.web) => {
                "To send your songs, join with the host's code instead of the web link".to_string()
            }
            ureq::Error::StatusCode(403) => "The host doesn't take songs from guests right now".to_string(),
            ureq::Error::StatusCode(413) => "The song is too big to send".to_string(),
            e => format!("Couldn't send the song: {e}"),
        })?;
    let v: Value = r.body_mut().read_json().map_err(|e| e.to_string())?;
    Ok(v["id"].as_u64().unwrap_or(0) as u32)
}

#[tauri::command]
pub async fn party_share(path: String, title: String, artist: String, album: String, duration: f64, compress: bool) -> Result<u32, String> {
    tauri::async_runtime::spawn_blocking(move || share(&path, &title, &artist, &album, duration, compress)).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn party_request(id: u32) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        get_json(&conn_url("/api/request", &format!("&id={id}"))?).map(|_| ()).map_err(|e| if e.contains("403") { "The host doesn't take requests right now".into() } else { e })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        let code = make_code(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 37)), 47_801, "a1b2c3d4e5");
        assert_eq!(code.len(), 20);
        assert_eq!(parse_invite(&code).unwrap(), ("http://192.168.1.37:47801".into(), "a1b2c3d4e5".into()));
        // Lowercase, spaces and 0/1 typed for O/I still work
        let messy = code.to_lowercase().replace('-', " ");
        assert_eq!(parse_invite(&messy).unwrap().1, "a1b2c3d4e5");
        assert_eq!(parse_invite("http://10.0.0.2:47800/?k=00ff00ff00").unwrap(), ("http://10.0.0.2:47800".into(), "00ff00ff00".into()));
        assert!(parse_invite("hello").is_err());
        let v6: Ipv6Addr = "2800:810:4e2:1a7::5c3".parse().unwrap();
        let code = make_code(IpAddr::V6(v6), 47_800, "00112233ff");
        assert_eq!(code.len(), 37 + 6);
        assert_eq!(parse_invite(&code).unwrap(), ("http://[2800:810:4e2:1a7::5c3]:47800".into(), "00112233ff".into()));
        assert_eq!(parse_invite("http://[2800:810::1]:47800/?k=aa").unwrap().0, "http://[2800:810::1]:47800");
        assert_eq!(parse_range("bytes=10-19", 100), Some((10, 19)));
        assert_eq!(parse_range("bytes=90-", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-5", 100), Some((95, 99)));
        assert_eq!(parse_range("bytes=200-", 100), None);
        assert_eq!(pct_decode("a%20b+c%C3%A9"), "a b cé");
        // RFC 4231, test case 2
        assert_eq!(hex(&hmac(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    /// A mono 16-bit WAV: a 440 Hz tone that changes volume every second
    fn tone_wav(secs: u32) -> Vec<u8> {
        let rate = 44_100u32;
        let n = rate * secs;
        let mut wav = Vec::new();
        wav.extend(b"RIFF");
        wav.extend((36 + n * 2).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(rate.to_le_bytes());
        wav.extend((rate * 2).to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((n * 2).to_le_bytes());
        for i in 0..n {
            let t = i as f32 / rate as f32;
            let v = (t * 440.0 * std::f32::consts::TAU).sin() * 0.2 * if (t as u32).is_multiple_of(2) { 1.0 } else { 0.5 };
            wav.extend(((v * 32767.0) as i16).to_le_bytes());
        }
        wav
    }

    /// Hosts a generated tone for a while, to try the web page by hand: WAX_TEST_PARTY_SERVE=1
    #[test]
    fn serve_for_a_browser() {
        if std::env::var("WAX_TEST_PARTY_SERVE").is_err() {
            return;
        }
        let path = std::env::temp_dir().join("wax-party-tone.wav");
        let secs = 60;
        fs::write(&path, tone_wav(secs)).unwrap();
        let song = Song {
            id: 1,
            title: "Test tone".into(),
            artist: "Wax".into(),
            album: "Tests".into(),
            duration: secs as f64,
            path: path.to_string_lossy().into(),
            ..Default::default()
        };
        let info = start_host("Tester", vec![song], 0, Arc::new(|v| println!("event {v}"))).unwrap();
        println!("LINK http://127.0.0.1:{}/?k={}", info.port, info.link.split("k=").nth(1).unwrap());
        update_host(HostState { track: Some(1), pos_ms: 0, paused: false, queue: vec![], allow_requests: false, cover: None, compress: false, allow_shares: false });
        thread::sleep(Duration::from_secs(150));
        stop_host();
    }

    #[test]
    fn host_and_guest() {
        let dir = std::env::temp_dir().join(format!("wax-party-{}", random_hex(4)));
        fs::create_dir_all(&dir).unwrap();
        let song_path = dir.join("song.flac");
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
        fs::write(&song_path, &data).unwrap();
        let wav_path = dir.join("tone.wav");
        fs::write(&wav_path, tone_wav(20)).unwrap();

        let events = Arc::new(Mutex::new(Vec::<Value>::new()));
        let ev = events.clone();
        let songs = vec![
            Song {
                id: 3,
                title: "Paranoid Android".into(),
                artist: "Radiohead".into(),
                album: "OK Computer".into(),
                duration: 383.0,
                path: song_path.to_string_lossy().into(),
                ..Default::default()
            },
            Song {
                id: 4,
                title: "Karma Police".into(),
                artist: "Radiohead".into(),
                album: "OK Computer".into(),
                duration: 264.0,
                path: song_path.to_string_lossy().into(),
                ..Default::default()
            },
            Song { id: 5, title: "Tone".into(), duration: 20.0, path: wav_path.to_string_lossy().into(), ext: "wav".into(), ..Default::default() },
        ];
        let info = start_host("Tony", songs, 0, Arc::new(move |v| ev.lock().unwrap().push(v))).unwrap();
        let web = info.link.split("k=").nth(1).unwrap().to_string();
        let key = HOST.lock().unwrap().as_ref().unwrap().ctx.token.clone();
        let code = make_code(IpAddr::V4(Ipv4Addr::LOCALHOST), info.port, &key);

        let st = join(&code, "Ana").unwrap();
        assert_eq!(st["name"], "Tony");
        assert!(st["track"].is_null());
        assert!(events.lock().unwrap().iter().any(|e| e["kind"] == "guests" && e["guests"][0]["name"] == "Ana"));

        // The guest waits for news; the host starts a song
        let seq = st["seq"].as_u64().unwrap();
        let waiter = thread::spawn(move || get_json(&conn_url("/api/state", &format!("&since={seq}")).unwrap()).unwrap());
        thread::sleep(Duration::from_millis(200));
        update_host(HostState { track: Some(3), pos_ms: 1500, paused: false, queue: vec![4], allow_requests: true, cover: None, compress: false, allow_shares: false });
        let st = waiter.join().unwrap();
        assert_eq!(st["track"]["title"], "Paranoid Android");
        assert_eq!(st["track"]["ext"], "flac");
        assert!(st["track"].get("path").is_none(), "the host's paths stay private");
        assert_eq!(st["queue"][0]["id"], 4);
        let (offset, rtt) = clock().unwrap();
        assert!(offset.abs() < 1000 && rtt < 1000);

        // The song arrives whole; the engine can read it while it's arriving
        let song: Song = serde_json::from_value(st["track"].clone()).unwrap();
        let local = fetch(&song, 0.0).unwrap();
        if let Some(g) = growing(&local) {
            let mut f = GrowingFile::new(File::open(&local).unwrap(), g);
            let mut all = Vec::new();
            f.read_to_end(&mut all).unwrap();
            assert_eq!(all.len(), data.len());
        }
        let local = fetch(&song, 1.0).unwrap();
        assert_eq!(fs::read(&local).unwrap(), data);
        assert!(local.ends_with(".flac"));

        // Songs that aren't playing or queued can't be copied
        let tone = Song { id: 5, ext: "wav".into(), ..Default::default() };
        assert!(fetch(&tone, 1.0).is_err());
        leave_downloads();
        update_host(HostState { track: Some(3), pos_ms: 1500, paused: false, queue: vec![4, 5], allow_requests: true, cover: None, compress: false, allow_shares: false });

        // The engine decodes a song as it arrives: every sample is there
        let tone = Song { id: 5, ext: "wav".into(), ..Default::default() };
        let local = fetch(&tone, 0.0).unwrap();
        let samples = crate::audio::SymSource::open(&local).unwrap().count();
        assert_eq!(samples, 44_100 * 20);

        // Lighter quality: the lossless tone goes out as an MP3 of the same length
        update_host(HostState { track: Some(5), pos_ms: 0, paused: false, queue: vec![], allow_requests: true, cover: None, compress: true, allow_shares: false });
        let st = get_json(&conn_url("/api/state", "&since=0").unwrap()).unwrap();
        assert_eq!(st["track"]["ext"], "mp3");
        let mp3: Song = serde_json::from_value(st["track"].clone()).unwrap();
        leave_downloads();
        let local = fetch(&mp3, 1.0).unwrap();
        assert!(local.ends_with(".mp3"));
        let size = fs::metadata(&local).unwrap().len();
        assert!(size < 1_000_000 && size > 700_000, "20 s at 320 kbps is about 800 kB, got {size}");
        let samples = crate::audio::SymSource::open(&local).unwrap().count();
        assert!((samples as i64 - 44_100 * 20 * 2).abs() < 44_100 / 5, "{samples}"); // the MP3 is stereo
        update_host(HostState { track: Some(3), pos_ms: 1500, paused: false, queue: vec![4, 5], allow_requests: true, cover: None, compress: false, allow_shares: false });

        // IPv6: the same session answers on the IPv6 side
        println!("IPv6 code: {:?}", info.ipv6_link);
        // (when this computer has IPv6 at all)
        if let Ok(mut r) = agent().get(&format!("http://[::1]:{}/api/time", info.port)).call() {
            assert!(r.body_mut().read_json::<Value>().unwrap()["now"].as_u64().is_some());
        }

        // A guest sends a song of their own: it arrives whole, named, and can be played to everyone
        assert!(share(&wav_path.to_string_lossy(), "My tone", "Ana", "Home", 20.0, false).is_err(), "sharing is off");
        update_host(HostState { track: Some(3), pos_ms: 0, paused: false, queue: vec![4], allow_requests: true, cover: None, compress: false, allow_shares: true });
        let id = share(&wav_path.to_string_lossy(), "My tone", "Ana", "Home", 20.0, false).unwrap();
        assert!(id >= SHARED_IDS);
        let ev = events.lock().unwrap().iter().find(|e| e["kind"] == "shared").cloned().unwrap();
        assert_eq!(ev["song"]["from"], "Ana");
        assert_eq!(ev["song"]["title"], "My tone");
        assert_eq!(fs::read(ev["path"].as_str().unwrap()).unwrap(), fs::read(&wav_path).unwrap());
        // Lighter quality: it travels as MP3
        let id2 = share(&wav_path.to_string_lossy(), "My tone", "Ana", "Home", 20.0, true).unwrap();
        let ev = events.lock().unwrap().iter().rev().find(|e| e["kind"] == "shared").cloned().unwrap();
        assert_eq!(ev["song"]["id"], id2);
        assert_eq!(ev["song"]["ext"], "mp3");
        // The host queues it: the others see who shared it and can get it
        update_host(HostState { track: Some(id), pos_ms: 0, paused: false, queue: vec![4], allow_requests: true, cover: None, compress: false, allow_shares: true });
        let st = get_json(&conn_url("/api/state", "&since=0").unwrap()).unwrap();
        assert_eq!(st["track"]["from"], "Ana");
        let shared: Song = serde_json::from_value(st["track"].clone()).unwrap();
        leave_downloads();
        assert_eq!(fs::read(fetch(&shared, 1.0).unwrap()).unwrap(), fs::read(&wav_path).unwrap());
        update_host(HostState { track: Some(3), pos_ms: 1500, paused: false, queue: vec![4, 5], allow_requests: true, cover: None, compress: false, allow_shares: false });

        // Searching and asking for a song
        let hits = get_json(&conn_url("/api/search", "&q=karma%20ok").unwrap()).unwrap();
        assert_eq!(hits.as_array().unwrap().len(), 1);
        get_json(&conn_url("/api/request", "&id=4").unwrap()).unwrap();
        assert!(events.lock().unwrap().iter().any(|e| e["kind"] == "request" && e["id"] == 4 && e["from"] == "Ana"));

        // A browser asking for part of the file
        let url = conn_url("/api/file/3", "").unwrap();
        let mut r = agent().get(&url).header("Range", "bytes=100-199").call().unwrap();
        assert_eq!(r.status().as_u16(), 206);
        assert_eq!(r.body_mut().read_to_vec().unwrap(), data[100..200].to_vec());
        // The key never travels: requests carry a signature instead
        assert!(!url.contains(&key));
        // A signature works once (someone watching can't replay it), and a changed one doesn't work
        let once = conn_url("/api/state", "&since=0").unwrap();
        assert!(agent().get(&once).call().is_ok());
        assert!(matches!(agent().get(&once).call(), Err(ureq::Error::StatusCode(403))));
        let forged = conn_url("/api/state", "&since=0").unwrap().replace("&since=0", "&since=1");
        assert!(matches!(agent().get(&forged).call(), Err(ureq::Error::StatusCode(403))));
        // The web link's key: listening yes, sending songs no
        let base = format!("http://127.0.0.1:{}", info.port);
        assert!(agent().get(&format!("{base}/api/state?k={web}")).call().is_ok());
        assert!(agent().get(&format!("{base}/?k={web}")).call().is_ok());
        let up = agent().post(&format!("{base}/api/upload?k={web}&title=x&ext=mp3")).send(&b"abc"[..]);
        assert!(matches!(up, Err(ureq::Error::StatusCode(403))));

        // Guessing keys: after 20 wrong ones the address is blocked, even with the right key
        for _ in 0..20 {
            let _ = agent().get(&format!("{base}/api/state?k=0123456789abcdef")).call();
        }
        assert!(matches!(agent().get(&format!("{base}/api/state?k={web}")).call(), Err(ureq::Error::StatusCode(429))));
        HOST.lock().unwrap().as_ref().unwrap().ctx.misses.lock().unwrap().clear();

        // The host ends it: the guest hears about it
        let seq = get_json(&conn_url("/api/state", "&since=0").unwrap()).unwrap()["seq"].as_u64().unwrap();
        let waiter = thread::spawn(move || get_json(&conn_url("/api/state", &format!("&since={seq}")).unwrap()).unwrap());
        thread::sleep(Duration::from_millis(200));
        stop_host();
        assert_eq!(waiter.join().unwrap()["ended"], true);
        leave();
        let _ = fs::remove_dir_all(&dir);
    }
}
