# Contributing to Wax

Bug reports and ideas are welcome in [issues](https://github.com/TonyMontania/Wax/issues). For code, open a pull request against `main`; the Build workflow compiles it for Windows, macOS and Linux.

## Running locally

You need [Rust](https://rustup.rs/) (stable). On Linux, also the libraries from `.github/workflows/build.yml`:

```bash
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libasound2-dev libdbus-1-dev
```

```bash
cargo run                                  # debug build
cargo build --release                      # target/release/wax(.exe)
npx @tauri-apps/cli@2 build --bundles nsis # Windows installer (deb,rpm,appimage on Linux · app,dmg on macOS)
```

The interface is plain HTML/CSS/JS in `ui/` (no build step). To work on it in a normal browser with fake data, serve the project root and open `dev/preview.html`:

```bash
python -m http.server 5174   # then http://localhost:5174/dev/preview.html
```

## Tests

```bash
cargo test --release
```

Some tests need real files or the network, and are skipped unless asked for:

| Variable | Tests |
| --- | --- |
| `WAX_TEST_AUDIO=<folder with a.flac b.mp3 c.m4a d.opus>` | audio engine, CUE, MP3 conversion |
| `WAX_NET=1` | Deezer, MusicBrainz, Last.fm, AutoEQ, update check |
| `WAX_TEST_OUTPUT=1` (+ `WAX_TEST_EXCLUSIVE=1`) | opening the real audio outputs |
| `WAX_TEST_AUTOSTART=1` | "start with Windows" registry entry (added and removed) |

## Structure

| Path | What's there |
| --- | --- |
| `src/audio.rs` | Audio engine: decoding, resampling, gapless mixing, crossfade, EQ / AutoEQ, crossfeed, outputs (shared and WASAPI exclusive), R128 analysis |
| `src/player.rs` | Engine commands, loudness scan, OS media controls |
| `src/library.rs` | Incremental scan, tags, organizer, M3U, history, batch tools, copy / MP3 conversion |
| `src/covers.rs` | Album covers and artist photos (local, Deezer, chosen by hand) |
| `src/cue.rs` | CUE sheets |
| `src/lyrics.rs` | Lyrics (.lrc, tags, LRCLIB) |
| `src/online.rs` | MusicBrainz, Last.fm, ListenBrainz |
| `src/discover.rs` | New releases, credits, lyrics search, AutoEQ, Spotify CSV |
| `src/desktop.rs` | Tray, taskbar buttons, mini player, notifications, global shortcuts, full screen, autostart |
| `src/discord.rs` | Discord status |
| `src/updater.rs` | Updates from GitHub Releases |
| `ui/` | Interface: `app.js`, `extras.js`, `more.js`, `engine.js` (engine bridge), `mini.html`, `osd.html` |
| `packaging/` | Files that go into the portable zip |
| `dev/` | Interface preview and the icon generator (`make_icon.py`) |

## Style

- Rust: `cargo fmt`-like formatting, small focused functions, comments that explain *why*.
- UI text in English, short and plain.
