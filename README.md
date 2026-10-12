# Wax

A **local** music player with the classic Spotify interface (2015–2017). Rust + Tauri 2.

## Install
Download it from [Releases](https://github.com/TonyMontania/Wax/releases/latest):

| System | File | Notes |
|---|---|---|
| Windows | `Wax_x.y.z_x64-setup.exe` | Installer: for your user or for everyone, in any folder or drive |
| Windows | `Wax-portable-x.y.z-win64.zip` | Portable: unzip anywhere (a folder, another drive, a USB stick) |
| macOS (Intel and Apple Silicon) | `Wax_x.y.z_universal.dmg` | Drag Wax to Applications |
| Linux | `.AppImage` | Runs anywhere: make it executable and open it |
| Linux (Debian, Ubuntu, Mint…) | `.deb` | `sudo apt install ./Wax_x.y.z_amd64.deb` |
| Linux (Fedora, openSUSE…) | `.rpm` | `sudo dnf install ./Wax-x.y.z-1.x86_64.rpm` |

Windows uses WebView2 (it comes with Windows 10/11). The macOS build isn't signed by Apple yet:
the first time, right-click Wax → Open.

**Updates**: Settings → About checks GitHub (also automatically once a day) and installs the new
version in the same place Wax is: the portable folder, the install folder, Wax.app or the AppImage.
The `.deb`/`.rpm` packages are handed to your system's installer.

## Where your data is
- **Portable**: in `data` next to `Wax.exe` (take the whole folder with you).
- **Installed**: `%APPDATA%\Wax` (Windows), `~/Library/Application Support/Wax` (macOS),
  `~/.local/share/wax` (Linux).

Settings → About opens both the program folder and the data folder. Inside the data folder:
- `state.json` — folders, playlists, favorites, settings, equalizer, shortcuts
- `library.json` — library cache (rescans only re-read the files that changed)
- `covers`, `artists`, `custom` — cover and photo thumbnails, images you chose
- `lyrics` — downloaded lyrics
- `loudness.json` — loudness analysis (normalization)
- `history.jsonl` — listening history (stats)
- `scrobble_queue.json` — plays waiting to be sent to Last.fm / ListenBrainz
- `lastfm`, `cache` — cached answers from Last.fm, Deezer, MusicBrainz and AutoEQ

## Sound
- **Own Rust engine** (symphonia + rodio): MP3, FLAC, OGG Vorbis, M4A/AAC, ALAC, WAV, AIFF.
- **CUE sheets**: an album ripped as one file plus a `.cue` shows up as separate tracks, played
  sample-exact and gapless (the big file itself is hidden).
  OPUS and other formats are played by the WebView's engine (no equalizer or crossfade).
- **Gapless**: the next track is opened ahead of time and chained into the same stream;
  the silence MP3/AAC encoders add is trimmed.
- **Crossfade** from 0 to 12 s (Settings). By default it isn't applied between consecutive tracks of the same album.
- **Volume normalization**: ReplayGain from the tags or Wax's own EBU R128 analysis (−18 LUFS target),
  per track or per album, with a preamp and clipping protection.
- **10-band equalizer** with presets (button next to the queue, or Settings → Equalizer).
- **AutoEQ**: pick your headphones (about 9,000 measured models) and Wax corrects their sound
  with their parametric profile. **Crossfeed** (light / medium / strong) for headphones.
- **Output device** and **WASAPI exclusive mode** (Settings → Audio output): the sound skips the
  Windows mixer. If exclusive mode can't be used, Wax falls back to the normal output.
- Decoding happens in its own thread with a ~3 s buffer, and sample rates are converted with a
  band-limited resampler (rubato), so a busy disk can't make the sound stutter.

## Library layout
Wax expects this layout inside each music folder you add:

```
Music > Artist > Album or EP > tracks (+ optional cover.jpg)
```

- **Artists** = the artist folders. Every track belongs to the artist of its folder,
  whatever the tags say; guests and collaborations never create stand-alone artists.
- Lists show the artist exactly as tagged, linking only the names that exist as an artist folder.
- **Artist photos**: `artist.jpg` / `folder.jpg` in the artist's folder; if there isn't one, from Deezer.
- **Singles folders**: a folder with several loose releases (e.g. `Artist\Singles\*.mp3`) works too;
  each track keeps its own release. There a generic `cover.jpg` is ignored; only embedded art or an
  image named after the track or the single (`Song.jpg`) is used.
- **Missing covers**: from Deezer, optionally saved as `cover.jpg` (as `Song.jpg` in a singles folder).
- **Genres** (Browse → Genres), **Your Favorites** (♥) and **Your Stats**.
- **Radio** lists every artist; each artist's radio shuffles all of their albums.
- **Auto DJ**: when the list ends, keeps playing similar music (artist, genre, era).
- **New music**: Wax watches your folders and adds whatever you copy or move.
- **Folders** (sidebar): browse the disk exactly as it is arranged, and play any folder.
- **Ratings** (1–5 stars) in the song lists, kept in Wax's data (sortable in Songs).
- **Change a cover or an artist photo** (right-click): pick one from Deezer or a file of your own;
  optionally saved as cover.jpg / artist.jpg and embedded in the files.
- **Full-screen "Now playing"** with synced lyrics (button next to the mini player).
- **Wrapped** (Your Stats): your year in music. **New Releases** (Browse): what your artists
  released that you don't have yet (Deezer). Search also looks **inside the lyrics**.
- **Accent color** (Settings → Appearance). Playlists can have their own **image and description**.

## Last.fm
With your own (free) API key — Settings → Scrobbling:
- **Popular** on the artist page comes from Last.fm (5 songs, "Show 5 more"), like the old Spotify.
- **Related Artists** tab: similar artists in your library first, then the ones you don't have.
- **About** tab: biography, listeners and tags; album pages show the album's description.
- **Tags as genres** for songs without a genre (optional; files aren't changed).
- Once connected: **♥ syncs with your Loved tracks**, and you can **import** your loved tracks
  and your **listening history** (only plays older than Wax's own history, so nothing is counted twice).

## Tags and files
- **Edit info** (right-click): one or several songs at once. Several artists or genres
  are separated with `;` and saved as multiple values.
- **Fill in from MusicBrainz** (right-click an album): pick the release and titles, artists
  (including the `ARTISTS` field), track numbers, year and genres are corrected.
- **Organize files**: renames (and optionally moves) files with a pattern, by default
  `{artist}\{folder}\{track} - {title}`. `{folder}` keeps the folders each file is in now
  (e.g. `Singles\Song`), so the default only renames files; use `{album}` instead to rebuild
  folders from the tags. Patterns can be saved as named presets (Settings → Organize files →
  Edit patterns…) and switched from the organize window. It shows what will change first; it also moves `.lrc` files and
  covers, and updates playlists, favorites and history.
- **M3U playlists**: import (`···` menu) and export (right-click a playlist).
- **Spotify playlists**: export them with [Exportify](https://exportify.app) and import the CSVs;
  Wax rebuilds them with your files and lists the songs you don't have.
- **Tags from the file name** (`{track} - {title}`, folders too), **fix capitalization** in bulk,
  and **credits** from MusicBrainz (right-click a song). **Organize files** can be undone.
- **Playlist folders** and **drag & drop**: songs onto a playlist or the queue button, reorder a
  playlist or the queue, playlists into folders.
- **Copy to folder** (right-click a playlist, album or songs): for a USB stick, the car or a phone,
  as `Artist\Album\song` or numbered in one folder, with an `.m3u8`, optionally converting to MP3
  (LAME, 128–320 kbps, tags and cover kept).
- **Library check** (`···` menu or Settings): albums without a cover or with the cover only in the
  folder, songs without lyrics, year, track number, genre or ReplayGain, with a fix for each:
  embed covers (from the folder or Deezer), download lyrics for everything, MusicBrainz, and
  write Wax's loudness analysis as ReplayGain tags.
- **Duplicates**: same artist, title and length; keeps the best copy (lossless / highest bitrate),
  sends the rest to the Recycle Bin and moves playlists, favorites and ratings to the copy kept.

## Lyrics
Microphone button in the player bar. Wax looks in this order:
1. `song.lrc` / `song.txt` next to the file (what [LRCGET](https://github.com/tranxuanthang/lrcget) downloads)
2. Lyrics embedded in the tags
3. Cache in `data\lyrics`
4. [LRCLIB](https://lrclib.net), the same database LRCGET uses

Click a line to jump to that moment. At the bottom of the panel: **Save .lrc**, **Embed** and search again.

## Desktop
- **Media keys** and the Windows media panel (with cover art).
- **⏮ ⏯ ⏭ buttons** in the taskbar thumbnail.
- **System tray** (optional: closing minimizes to the tray).
- Always-on-top **mini player**.
- **Notification on song change** when Wax isn't in the foreground.
- Configurable **global shortcuts** (by default Ctrl+Alt+P, Ctrl+Alt+← / →, Ctrl+Alt+↑ / ↓, Ctrl+Alt+L…).
- **Last.fm / ListenBrainz** scrobbling, queued when offline.
- **Sleep timer** (moon button): 15–90 minutes, with a fade out, or at the end of the song.
- **Discord status** ("Listening to …") with your own Discord application ID (Settings).

## Shortcuts inside Wax
| Key | Action |
|---|---|
| Space | Play / pause |
| Ctrl + ← / → | Previous / next |
| Ctrl + ↑ / ↓ | Volume |
| Ctrl + F | Filter / search |
| Ctrl + A, Shift/Ctrl + click | Multiple selection |
| Alt + ← / → | Back / forward |
| Double-click | Play song |

## Building
Rust (stable) and, on Linux, the WebKitGTK/GStreamer/ALSA development packages listed in
`.github/workflows/build.yml`.
```
cargo build --release                               # just the program: target/release/wax(.exe)
npx @tauri-apps/cli@2 build --bundles nsis          # Windows installer (deb,rpm,appimage / app,dmg elsewhere)
```
GitHub Actions builds every platform (Actions → Build → Run workflow). Audio engine tests:
`WAX_TEST_AUDIO=<folder with a.flac b.mp3 c.m4a d.opus> cargo test --release audio::`
Online tests (Deezer, MusicBrainz, Last.fm): `WAX_NET=1 cargo test --release deezer musicbrainz lastfm`

## Structure
- `src/audio.rs` — audio engine (decoding, gapless mixing, crossfade, equalizer, R128 analysis)
- `src/player.rs` — engine commands, library analysis, Windows media controls
- `src/library.rs` — incremental scanning, tags, organizer, M3U, history, watched folders
- `src/covers.rs` — album covers and artist photos
- `src/cue.rs` — CUE sheets
- `src/discord.rs` — Discord status
- `src/discover.rs` — new releases, credits, lyrics search, AutoEQ, Spotify CSV
- `src/lyrics.rs` — lyrics
- `src/online.rs` — MusicBrainz, Last.fm (scrobbling and info), ListenBrainz
- `src/desktop.rs` — tray, taskbar, mini player, notifications, global shortcuts
- `ui/` — interface (dependency-free HTML/CSS/JS): `app.js`, `extras.js` and `more.js` (the features above), `engine.js` (engine bridge), `mini.html`, `osd.html`
- `dev/preview.html` — interface preview with fake data (`python -m http.server` in the root)
- `dev/make_icon.py` — generates the icon

## License
[GPL-3.0](LICENSE). The program includes LAME (LGPL) for MP3 conversion.
