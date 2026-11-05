# Changelog

What changed in each version of Wax, as you'd notice it in the app.

## Next version (not released yet)

- **Listen Together:** guests get the song-change notification too, and the lyrics panel shows the host's lyrics (their .lrc files, embedded or saved lyrics); if the host has none, Wax looks online as usual
- **Full-screen Now Playing shows the album cover and the artist's photo in turn**, every 20 seconds with a slow fade (the button next to Lyrics turns it off) ([#5](https://github.com/TonyMontania/Wax/issues/5))
- Full screen: a long song title no longer pushes the album line under the progress bar; titles longer than two lines are cut ([#4](https://github.com/TonyMontania/Wax/issues/4))
- Mini player: a bit taller, so the play button no longer touches the progress bar and the artist has room ([#4](https://github.com/TonyMontania/Wax/issues/4))

## 1.1.1

- **Albums can be sorted** by name, artist, year or recently added, in either direction ([#2](https://github.com/TonyMontania/Wax/issues/2))
- **Lyrics in a `Lyrics` subfolder** of the album folder are found too, and new `.lrc` files go there when the album has one ([#3](https://github.com/TonyMontania/Wax/issues/3))
- **Updates inside Wax**: a bar at the top when a new version is out; Update downloads it in a window with its progress, then **Install and restart**. If you have 1.1.0, download this version by hand once — from 1.1.1 on, Wax updates itself again
- **"Run Wax" at the end of the installer** opens Wax every time, also after installing for all users ([#1](https://github.com/TonyMontania/Wax/issues/1)); the same goes for reopening after an update

## 1.1.0

**Listen Together**
- Host a session and friends hear what you play, at the same moment — from Wax or any web browser,
  on a computer or a phone. Wax connects the computers directly, with no outside service
- Invite with a code: one for your network, one for the internet (your router opens the way by
  itself when it can), and one over IPv6 when your connection has it
- Guests can add songs from your library, or send songs from their own
- "Lighter" quality sends lossless songs as MP3 320 kbps for slow connections
- The session's key never travels over the network, and whoever keeps guessing codes is blocked

**New Releases and artist photos**
- Wax now tells your artist apart from others with the same name (Ghost the band ≠ Ghost the
  rapper): new releases and photos come from the artist whose music matches yours
- Releases filed by mistake under your artist's name on Deezer are left out
- Choosing a photo by hand shows only artists with that exact name, yours first

**Updates**
- Updates are checked against the maintainer's signature before installing
- Builds for Windows on ARM and Linux on ARM

## 1.0.0

The first release.
