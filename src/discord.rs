//! Discord status ("Listening to …") through Discord's local IPC pipe.
//!
//! Needs the ID of a Discord application (free, discord.com/developers), which the user
//! pastes in Settings. A dedicated thread keeps the connection and reconnects on its own
//! when Discord is closed and opened again.

use std::fs::File;
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::State;

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    title: String,
    artist: String,
    album: String,
    /// UNIX ms when the track started and when it ends (for Discord's progress bar)
    start: u64,
    end: u64,
}

struct Msg {
    client_id: String,
    activity: Option<Activity>,
}

#[derive(Default)]
pub struct Discord(Mutex<Option<Sender<Msg>>>);

fn frame(op: u32, body: &Value) -> Vec<u8> {
    let data = body.to_string().into_bytes();
    let mut out = Vec::with_capacity(data.len() + 8);
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out
}

fn read_frame(pipe: &mut File) -> std::io::Result<Value> {
    let mut head = [0u8; 8];
    pipe.read_exact(&mut head)?;
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    let mut data = vec![0u8; len.min(1 << 20)];
    pipe.read_exact(&mut data)?;
    Ok(serde_json::from_slice(&data).unwrap_or(Value::Null))
}

fn connect(client_id: &str) -> Option<File> {
    #[cfg(windows)]
    let names: Vec<String> = (0..10).map(|i| format!(r"\\.\pipe\discord-ipc-{i}")).collect();
    #[cfg(not(windows))]
    let names: Vec<String> = {
        let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
        (0..10).map(|i| format!("{dir}/discord-ipc-{i}")).collect()
    };
    for n in names {
        let Ok(mut pipe) = File::options().read(true).write(true).open(&n) else { continue };
        if pipe.write_all(&frame(0, &json!({ "v": 1, "client_id": client_id }))).is_err() {
            continue;
        }
        // READY (or an error, e.g. a wrong application ID)
        match read_frame(&mut pipe) {
            Ok(v) if v["evt"] == "READY" => return Some(pipe),
            _ => continue,
        }
    }
    None
}

fn set_activity(pipe: &mut File, a: Option<&Activity>) -> std::io::Result<()> {
    let activity = a.map(|a| {
        let mut act = json!({
            "type": 2, // "Listening to"
            "details": cut(&a.title),
            "state": cut(&format!("by {}", a.artist)),
            "timestamps": { "start": a.start, "end": a.end },
        });
        if !a.album.is_empty() {
            act["assets"] = json!({ "large_text": cut(&a.album) });
        }
        act
    });
    let body = json!({
        "cmd": "SET_ACTIVITY",
        "args": { "pid": std::process::id(), "activity": activity },
        "nonce": format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)),
    });
    pipe.write_all(&frame(1, &body))?;
    read_frame(pipe).map(|_| ())
}

/// Discord rejects texts shorter than 2 or longer than 128 characters.
fn cut(s: &str) -> String {
    let mut s: String = s.chars().take(128).collect();
    while s.chars().count() < 2 {
        s.push(' ');
    }
    s
}

fn worker() -> Sender<Msg> {
    let (tx, rx) = channel::<Msg>();
    std::thread::Builder::new()
        .name("wax-discord".into())
        .spawn(move || {
            let mut pipe: Option<File> = None;
            let mut id = String::new();
            while let Ok(mut m) = rx.recv() {
                // Only the latest state matters
                while let Ok(n) = rx.try_recv() {
                    m = n;
                }
                if m.client_id != id {
                    pipe = None;
                    id = m.client_id.clone();
                }
                if id.is_empty() {
                    continue;
                }
                for _ in 0..2 {
                    if pipe.is_none() {
                        pipe = connect(&id);
                    }
                    let Some(p) = pipe.as_mut() else { break };
                    if set_activity(p, m.activity.as_ref()).is_ok() {
                        break;
                    }
                    pipe = None; // Discord was closed: try once more with a new connection
                }
            }
        })
        .ok();
    tx
}

/// Shows (or clears, with `activity: null`) the status. An empty `client_id` disconnects.
#[tauri::command]
pub fn discord_update(state: State<Discord>, client_id: String, activity: Option<Activity>) {
    let Ok(mut slot) = state.0.lock() else { return };
    let tx = slot.get_or_insert_with(worker);
    if tx.send(Msg { client_id: client_id.trim().to_string(), activity }).is_err() {
        *slot = None;
    }
}
