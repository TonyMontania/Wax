'use strict';
/* Wax — user interface and playback logic.
   The Rust backend does library scanning, tags, covers, lyrics and audio output. */

const TAURI = window.__TAURI__;
const invoke = (cmd, args) => TAURI.core.invoke(cmd, args);
const fileUrl = p => TAURI.core.convertFileSrc(p);
const appWin = TAURI.window.getCurrentWindow();

const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const fmt = sec => { sec = Math.max(0, Math.floor(sec || 0)); return `${Math.floor(sec / 60)}:${String(sec % 60).padStart(2, '0')}`; };
const fmtLong = sec => { const h = Math.floor(sec / 3600), m = Math.round((sec % 3600) / 60); return h ? `${h} hr ${m} min` : `${m} min`; };
const fmtDate = s => { if (!s) return ''; const d = new Date(s * 1000); return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`; };
const plural = (n, a, b) => `${n} ${n === 1 ? a : b}`;
const collator = new Intl.Collator('en', { sensitivity: 'base', numeric: true });
const cmp = (a, b) => collator.compare(a || '', b || '');
const norm = s => (s || '').normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();
const dirOf = p => p.replace(/[\\/][^\\/]*$/, '');
// Tracks of a CUE sheet have a virtual path (sheet.cue#NN): this is the real audio file
const realPath = t => t?.cue?.file || t?.path;
const shuffleArr = a => { for (let i = a.length - 1; i > 0; i--) { const j = Math.floor(Math.random() * (i + 1)); [a[i], a[j]] = [a[j], a[i]]; } return a; };

/* ───────────── Icons ───────────── */
const F = d => `<path d="${d}" fill="currentColor"/>`;
const K = (d, w = 1.4) => `<path d="${d}" fill="none" stroke="currentColor" stroke-width="${w}" stroke-linecap="round" stroke-linejoin="round"/>`;
const SPK = F('M1.5 5.8h2.8L8 3v10L4.3 10.2H1.5z');
const ICONS = {
  play: F('M4.5 2.6v10.8L13 8z'),
  pause: F('M4 2.5h2.7v11H4zM9.3 2.5H12v11H9.3z'),
  next: F('M11.4 2.5h1.9v11h-1.9zM2.8 2.6v10.8L10.4 8z'),
  prev: F('M2.7 2.5h1.9v11H2.7zM13.2 2.6v10.8L5.6 8z'),
  shuffle: K('M1.5 4.5h2.2c3.3 0 5 7 8.3 7h2.5M1.5 11.5h2.2c3.3 0 5-7 8.3-7h2.5M12.5 2.5l2 2-2 2M12.5 9.5l2 2-2 2'),
  repeat: K('M2 8V6.5a2 2 0 0 1 2-2h9.5M11.5 2.5l2 2-2 2M14 8v1.5a2 2 0 0 1-2 2H2.5M4.5 13.5l-2-2 2-2'),
  vol: SPK + K('M10.5 5.8a3 3 0 0 1 0 4.4M12.4 3.8a6 6 0 0 1 0 8.4'),
  vol1: SPK + K('M10.5 5.8a3 3 0 0 1 0 4.4'),
  mute: SPK + K('M10.5 6l4 4M14.5 6l-4 4'),
  speaker: SPK + K('M10.5 5.8a3 3 0 0 1 0 4.4M12.4 3.8a6 6 0 0 1 0 8.4'),
  queue: K('M1.5 3.5h13M1.5 7.5h13M1.5 11.5h7') + F('M11 9.3v5.4l4.2-2.7z'),
  search: K('M6.8 2a4.8 4.8 0 1 0 0 9.6 4.8 4.8 0 0 0 0-9.6zM10.4 10.4l4.1 4.1', 1.6),
  clock: K('M8 1.8a6.2 6.2 0 1 0 0 12.4A6.2 6.2 0 0 0 8 1.8zM8 4.5V8l2.5 1.6'),
  cal: K('M2.5 3.5h11v10h-11zM2.5 6.5h11M5.5 2v3M10.5 2v3'),
  left: K('M10 3L5 8l5 5', 1.6), right: K('M6 3l5 5-5 5', 1.6),
  up: K('M3 10.5l5-5 5 5', 1.6), down: K('M3 5.5l5 5 5-5', 1.6),
  sortUp: F('M8 4l4 6H4z'), sortDown: F('M8 12L4 6h8z'),
  plus: K('M8 2.5v11M2.5 8h11'),
  x: K('M3.5 3.5l9 9M12.5 3.5l-9 9'),
  min: K('M3 8.5h10', 1), max: K('M3.5 3.5h9v9h-9z', 1), restore: K('M3.5 5.5h7v7h-7zM5.5 5.5v-2h7v7h-2', 1),
  note: K('M6 12.2a2 2 0 1 1-2-2 2 2 0 0 1 2 2zM6 12.2V3.2l7-1.4v8.8M13 10.6a2 2 0 1 1-2-2 2 2 0 0 1 2 2z', 1.1),
  dots: F('M3 6.6a1.4 1.4 0 1 0 0 2.8 1.4 1.4 0 0 0 0-2.8zM8 6.6a1.4 1.4 0 1 0 0 2.8 1.4 1.4 0 0 0 0-2.8zM13 6.6a1.4 1.4 0 1 0 0 2.8 1.4 1.4 0 0 0 0-2.8z'),
  user: K('M8 2a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM2.5 14c.6-3 2.8-4.5 5.5-4.5s4.9 1.5 5.5 4.5'),
  folder: K('M1.5 3.5h4.5l1.5 1.5h7v8h-13z'),
  person: K('M8 3a2.6 2.6 0 1 0 0 5.2A2.6 2.6 0 0 0 8 3zM3.5 13c.5-2.4 2.3-3.6 4.5-3.6s4 1.2 4.5 3.6', 1),
  radio: K('M2.5 6.5h11v7h-11zM4 6.5l8-4M5.5 10a1.2 1.2 0 1 0 0 .1M9 9h3M9 11h3', 1.2),
  heart: K('M8 13.8S2 10.2 2 6a3 3 0 0 1 6-.9A3 3 0 0 1 14 6c0 4.2-6 7.8-6 7.8z'),
  heartFill: F('M8 13.8S2 10.2 2 6a3 3 0 0 1 6-.9A3 3 0 0 1 14 6c0 4.2-6 7.8-6 7.8z'),
  eq: K('M3 2.5v11M8 2.5v11M13 2.5v11') + F('M1.6 8.6h2.8v2.2H1.6zM6.6 4.2h2.8v2.2H6.6zM11.6 9.8h2.8V12h-2.8z'),
  mini: K('M1.5 3.5h13v9h-13zM8.5 8.5h4.5v3H8.5z', 1.2),
  gear: K('M8 5.6a2.4 2.4 0 1 0 0 4.8 2.4 2.4 0 0 0 0-4.8zM8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4', 1.2),
  chart: K('M2 14h12M4 12V8M7 12V4M10 12V6.5M13 12V9.5', 1.6),
  tag: K('M2 2.5h5.5l6.5 6.5-5 5-6.5-6.5zM5 5.5h.1', 1.4),
  mic: K('M8 1.5a2.2 2.2 0 0 0-2.2 2.2v3.8a2.2 2.2 0 0 0 4.4 0V3.7A2.2 2.2 0 0 0 8 1.5zM3.8 7.3a4.2 4.2 0 0 0 8.4 0M8 11.5v3M5.6 14.5h4.8'),
  refresh: K('M13.5 8a5.5 5.5 0 1 1-1.6-3.9M13.5 2.5v3h-3'),
  moon: K('M13 9.6A5.6 5.6 0 0 1 6.4 3a5.6 5.6 0 1 0 6.6 6.6z'),
  full: K('M2 6V2h4M10 2h4v4M14 10v4h-4M6 14H2v-4'),
  // sidebar
  browse: K('M8 1.8a6.2 6.2 0 1 0 0 12.4A6.2 6.2 0 0 0 8 1.8z') + F('M10.9 5.1L9.2 9.2 5.1 10.9l1.7-4.1z'),
  disc: K('M8 1.8a6.2 6.2 0 1 0 0 12.4A6.2 6.2 0 0 0 8 1.8zM8 6.3a1.7 1.7 0 1 0 0 3.4 1.7 1.7 0 0 0 0-3.4z'),
  computer: K('M1.8 2.8h12.4v8H1.8zM5.5 13.5h5M8 10.8v2.7'),
  mic2: K('M9.6 2.2a3 3 0 1 1 4.2 4.2 3 3 0 0 1-4.2-4.2zM9.8 6.3l-6.6 6.6 1 1 6.6-6.6'),
  playlist: K('M1.5 3.5h9M1.5 7h9M1.5 10.5h5') + K('M12.5 13a1.5 1.5 0 1 1-1.5-1.5h1.5V5.5l2-.7', 1.2),
  star: F('M8 1.6l1.9 4 4.4.5-3.3 3 .9 4.3L8 11.2l-3.9 2.2.9-4.3-3.3-3 4.4-.5z'),
};
const ic = (n, s = 16) => `<svg class="ic" width="${s}" height="${s}" viewBox="0 0 16 16" aria-hidden="true">${ICONS[n] || ''}</svg>`;
function paintIcons(root = document) { $$('[data-ic]', root).forEach(e => { e.innerHTML = ic(e.dataset.ic, +e.dataset.s || 16); }); }
function setIcon(el, name) { el.dataset.ic = name; el.innerHTML = ic(name, +el.dataset.s || 16); }

/* ───────────── State ───────────── */
const S = {
  tracks: [], albums: [], albumMap: new Map(), artists: [], artistMap: new Map(), byPath: new Map(),
  prefs: { folders: [], playlists: [], volume: 0.8, muted: false, shuffle: false, repeat: 'off', bigCover: false, plays: {}, recent: [], last: null, lyricsOnline: true, saveLrc: false, embedLyrics: false, pendingEmbeds: {},
    favorites: [], crossfade: 0, xfadeSameAlbum: false, normalize: 'off', preamp: 0, preventClip: true, autoDJ: false, autoDJMode: 'mix',
    watchFolders: true, fetchCovers: true, saveCovers: false, fetchArtistImages: true, closeToTray: false, notify: true },
  scanning: false,
};

let saveTimer = 0;
function savePrefs() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => invoke('save_state', { json: JSON.stringify(S.prefs) }).catch(console.error), 400);
}

function albumKeyOf(t) { return `${t.album}::${t.albumArtist || dirOf(t.path)}`.toLowerCase(); }
function artistOf(t) { return t.albumArtist || t.artist; }

/* Multiple artists: the file's multi-value tags are used (ARTISTS, or several ARTIST values),
   which Rust delivers in t.artists. Names are never split on commas or slashes, because some
   artists have those characters in their name. Only when the file has a single artist value is
   it split on "feat." / "ft." / "featuring" and ";", which are never part of a name. */
const FEAT_SEP = /\s*;\s*|\s+(?:feat\.?|ft\.|featuring)\s+|\s*\((?:feat\.?|ft\.|featuring)\s+([^)]*)\)/i;
function trackArtists(t) {
  const tagged = (t.artists || []).map(x => x.trim()).filter(Boolean);
  if (tagged.length > 1) return [...new Set(tagged)];
  const one = (t.artist || '').trim();
  const parts = one.split(FEAT_SEP).filter(Boolean).map(x => x.trim()).filter(Boolean);
  return parts.length ? [...new Set(parts)] : [one];
}
/* Artists = artist folders.
   Layout: music folder > Artist > Album/EP > tracks.
   Every track belongs to the artist of its folder; tags are only used to display the
   credit and to link the names that match an artist folder.
   Guests ("feat.", collaborations) never create stand-alone artists.
   The separators include "y", "e" and "con" for Spanish/Portuguese credits. */
const COMBO_SEP = /(\s*[,&+;]\s*|\s+\/\s+|\s+(?:x|×|and|y|e|with|con|feat\.?|ft\.|featuring|vs\.?)\s+)/i;
// Finds the parts of a combined name that are known artists
function comboOf(name, known) {
  const toks = name.split(COMBO_SEP);              // [part, separator, part, ...]
  const n = (toks.length + 1) / 2;
  if (n < 2) return null;
  const segs = [];
  let i = 0, found = false;
  while (i < n) {
    let m = null;
    for (let j = n - 1; j >= i && !m; j--) {       // longest match first
      if (i === 0 && j === n - 1) continue;        // the whole name doesn't count
      const text = toks.slice(2 * i, 2 * j + 1).join('');
      const k = known.get(norm(text.trim()));
      if (k) m = { j, text, artist: k };
    }
    if (m) { segs.push({ text: m.text, artist: m.artist }); found = true; i = m.j + 1; }
    else { segs.push({ text: toks[2 * i] }); i++; }
    if (i < n) segs.push({ sep: toks[2 * i - 1] });
  }
  return found ? segs : null;
}

// Artist folder: first level inside the music folder,
// only when there is another folder (the album) below it before the file.
function folderArtistOf(t) {
  const p = t.path.toLowerCase();
  for (const root of S.prefs.folders) {
    const r = root.toLowerCase().replace(/[\\/]+$/, '');
    if (!p.startsWith(r + '\\') && !p.startsWith(r + '/')) continue;
    const rel = t.path.slice(r.length + 1).split(/[\\/]/).filter(Boolean);
    return rel.length >= 3 ? { name: rel[0].trim(), dir: t.path.slice(0, r.length + 1 + rel[0].length) } : null;
  }
  return null;
}
// Loose tracks outside an artist folder: the tags are used
const fallbackOwner = t => (t.albumArtist || t._artists[0] || t.artist || 'Unknown Artist').trim();

S.artistByNorm = new Map();                         // normalized name → artist (folder)
const comboCache = new Map();
const ownerOf = n => S.artistByNorm.get(norm((n || '').trim()));
function combo(n) {
  if (!comboCache.has(n)) comboCache.set(n, ownerOf(n) ? null : comboOf(n, S.artistByNorm));
  return comboCache.get(n);
}
// Artists (folders) that a tag name points to
function linkTargets(n) {
  const o = ownerOf(n);
  if (o) return [o];
  const segs = combo(n);
  return segs ? segs.filter(x => x.artist).map(x => x.artist) : [];
}
const resolveArtist = n => { const t = linkTargets(n); return t.length ? t : [n]; };
const artistLink = (n, text = n) => S.artistMap.has(n)
  ? `<span class="lnk" data-go="artist" data-arg="${esc(n)}">${esc(text)}</span>` : esc(text);
// The name exactly as tagged, linking only the artists that exist as a folder
function artistHtml(n) {
  const o = ownerOf(n);
  if (o) return artistLink(o, n);
  const segs = combo(n);
  return segs ? segs.map(x => x.sep !== undefined ? esc(x.sep) : x.artist ? artistLink(x.artist, x.text) : esc(x.text)).join('') : esc(n);
}
const artistLinks = names => names.map(artistHtml).join(', ');
// Album subtitle: the tagged credit, linked to the artist that owns the folder
function albumArtistHtml(a) {
  const h = artistHtml(a.artist);
  return h.includes('class="lnk"') ? h : artistLink(a.owner, a.artist);
}

function buildIndex() {
  S.byPath = new Map(S.tracks.map((t, i) => [t.path, i]));
  S.albumMap = new Map();
  S.artistMap = new Map();
  S.artistByNorm = new Map();
  S.artistDir = new Map();
  S.genreMap = new Map();
  if (typeof albumLoudCache !== 'undefined') albumLoudCache.clear();
  comboCache.clear();
  sortCache.arr = null;

  // 1) Owner artist of each track: its folder (or the tags for loose files)
  for (const t of S.tracks) {
    t._artists = trackArtists(t);
    const fa = folderArtistOf(t);
    const name = fa?.name || fallbackOwner(t);
    const k = norm(name);
    if (!S.artistByNorm.has(k)) S.artistByNorm.set(k, name);
    t._owner = S.artistByNorm.get(k);
    if (fa && !S.artistDir.has(t._owner)) S.artistDir.set(t._owner, fa.dir);
  }

  // 2) Albums, artists and genres
  S.tracks.forEach((t, i) => {
    t._names = [...new Set([t._owner, ...t._artists.flatMap(linkTargets)])];
    t._s = norm(`${t.title} ${t.artist} ${t.album} ${t.albumArtist}`);   // precomputed search text
    const key = albumKeyOf(t);
    let a = S.albumMap.get(key);
    if (!a) S.albumMap.set(key, a = { key, title: t.album, artists: new Set(), albumArtist: t.albumArtist, year: 0, tracks: [], added: 0, duration: 0, owner: t._owner });
    a.tracks.push(i); a.artists.add(t.artist);
    a.year = Math.max(a.year, t.year || 0); a.added = Math.max(a.added, t.added || 0); a.duration += t.duration || 0;

    let ar = S.artistMap.get(t._owner);
    if (!ar) S.artistMap.set(t._owner, ar = { name: t._owner, tracks: [], albums: new Set() });
    ar.tracks.push(i); ar.albums.add(key);

    t._g = t.genres?.length ? t.genres : (S.prefs.lfGenres && S.prefs.lfTags?.[norm(t._owner)]) || [];
    for (const g of t._g) {
      const gk = norm(g);
      let ge = S.genreMap.get(gk);
      if (!ge) S.genreMap.set(gk, ge = { key: gk, name: g, tracks: [], albums: new Set() });
      ge.tracks.push(i); ge.albums.add(key);
    }
  });
  // Folders holding several releases (e.g. a "Singles" folder with loose tracks): a generic
  // cover.jpg there doesn't belong to any one of them
  const dirAlbums = new Map();
  for (const a of S.albumMap.values()) for (const i of a.tracks) {
    const d = dirOf(S.tracks[i].path).toLowerCase();
    if (!dirAlbums.has(d)) dirAlbums.set(d, new Set());
    dirAlbums.get(d).add(norm(a.title));   // by title: inconsistent album-artist tags don't count
  }
  for (const a of S.albumMap.values()) {
    a.shared = (dirAlbums.get(dirOf(S.tracks[a.tracks[0]].path).toLowerCase())?.size || 0) > 1;
    a.artist =a.albumArtist || (a.artists.size === 1 ? [...a.artists][0] : 'Various Artists');
    a._s = norm(`${a.title} ${a.artist} ${a.owner}`);
    a.tracks.sort((x, y) => { const p = S.tracks[x], q = S.tracks[y]; return (p.discNo - q.discNo) || (p.trackNo - q.trackNo) || cmp(p.title, q.title); });
  }
  for (const ar of S.artistMap.values()) {
    ar.albums = [...ar.albums].map(k => S.albumMap.get(k)).sort((x, y) => (y.year - x.year) || cmp(x.title, y.title));
  }
  S.albums = [...S.albumMap.values()].sort((x, y) => cmp(x.title, y.title));
  S.artists = [...S.artistMap.values()].sort((x, y) => cmp(x.name, y.name));
  S.mainArtists = S.artists;
  S.genres = [...S.genreMap.values()].filter(g => g.tracks.length >= 2).sort((a, b) => b.tracks.length - a.tracks.length);
}

/* ───────────── Covers (lazy loading) ───────────── */
// Rust generates JPEG thumbnails (300 px for grids/lists, 640 px for headers)
// so multi-MB covers are never decoded for every card.
const coverP = new Map();
function albumCover(key, big = false) {
  const ck = key + (big ? '#lg' : '#sm');
  if (!coverP.has(ck)) {
    const a = S.albumMap.get(key);
    coverP.set(ck, a
      ? invoke('get_cover', { path: realPath(S.tracks[a.tracks[0]]), size: big ? 640 : 300,
          online: S.prefs.fetchCovers !== false ? { artist: a.albumArtist || a.artist, album: a.title, saveToFolder: !!S.prefs.saveCovers } : null,
          shared: a.shared ? { album: a.title } : null, custom: S.prefs.coverOverrides?.[key] || null })
        .then(c => c ? { url: fileUrl(c.path), path: c.path, color: c.color } : null).catch(() => null)
      : Promise.resolve(null));
  }
  return coverP.get(ck);
}
function paintCover(el, key = el.dataset.cover) {
  el.dataset.p = '1';
  el.style.backgroundImage = ''; el.classList.remove('has-img');
  const big = el.dataset.big === '1', art = el.dataset.artist;
  if (!key && !art) return;
  // Artist photos: from the folder or online; otherwise an album cover
  const p = art ? artistImage(art, big).then(c => c || (key ? albumCover(key, big) : null)) : albumCover(key, big);
  p.then(c => {
    if (c && (el.dataset.cover ?? key) === key) { el.style.backgroundImage = `url("${c.url}")`; el.classList.add('has-img'); }
  });
}
const io = new IntersectionObserver(es => es.forEach(e => { if (e.isIntersecting) { io.unobserve(e.target); paintCover(e.target); } }), { rootMargin: '300px' });
function hydrate(root) {
  $$('[data-cover]:not([data-p])', root).forEach(el => io.observe(el));
  $$('[data-grid-more]', root).forEach(s => moreIO.observe(s));
  paintIcons(root);
  paintRows(true);
}
const cvHtml = (key, cls = '', ph = 40, big = false, artist = null) => `<div class="cv ${cls}" data-cover="${esc(key || '')}"${big ? ' data-big="1"' : ''}${artist ? ` data-artist="${esc(artist)}"` : ''}><span class="ph">${ic(artist ? 'person' : 'note', ph)}</span></div>`;

/* Grids that load in batches as you scroll */
const GRID_CHUNK = 60;
const grids = new Map();
function gridHtml(id, items, render) {
  const shown = Math.min(GRID_CHUNK, items.length);
  grids.set(id, { items, render, shown });
  return `<div class="grid" data-grid="${id}">${items.slice(0, shown).map(render).join('')}</div>`
    + (items.length > shown ? `<div class="grid-more" data-grid-more="${id}"></div>` : '');
}
const moreIO = new IntersectionObserver(es => es.forEach(e => { if (e.isIntersecting) growGrid(e.target); }), { root: $('#view'), rootMargin: '700px' });
function growGrid(sentinel) {
  moreIO.unobserve(sentinel);
  const id = sentinel.dataset.gridMore, g = grids.get(id), grid = $(`[data-grid="${id}"]`, $('#view'));
  if (!g || !grid) return;
  grid.insertAdjacentHTML('beforeend', g.items.slice(g.shown, g.shown + GRID_CHUNK).map(g.render).join(''));
  g.shown = Math.min(g.items.length, g.shown + GRID_CHUNK);
  $$('[data-cover]:not([data-p])', grid).forEach(el => io.observe(el));
  markPlaying();
  if (g.shown < g.items.length) moreIO.observe(sentinel); else sentinel.remove();
}

/* ───────────── Navigation ───────────── */
const view = $('#view');
const nav = { stack: [], pos: -1 };
let route = { view: 'browse' };
const lists = new Map();      // list id → visible track indices
const filters = new Map();    // container id → (q) => html

function go(r, replace = false) {
  if (replace && nav.pos >= 0) nav.stack[nav.pos] = r;
  else { nav.stack.length = nav.pos + 1; nav.stack.push(r); nav.pos++; }
  show(r);
}
function back() { if (nav.pos > 0) show(nav.stack[--nav.pos]); }
function fwd() { if (nav.pos < nav.stack.length - 1) show(nav.stack[++nav.pos]); }

function show(r, keepScroll = false) {
  route = r;
  const top = view.scrollTop;
  lists.clear(); filters.clear(); grids.clear(); vtables.clear(); sel.list = null; sel.set.clear();
  const fn = VIEWS[r.view] || VIEWS.browse;
  view.className = '';
  $('#b-queue').classList.toggle('on', r.view === 'queue');
  view.innerHTML = fn(r.arg, r);
  view.scrollTop = keepScroll ? top : 0;
  hydrate(view);
  if (r.view !== 'search') $('#search').value = '';
  markPlaying(); markSidebar();
  $('#nav-back').disabled = nav.pos <= 0;
  $('#nav-fwd').disabled = nav.pos >= nav.stack.length - 1;
}
const rerender = () => show(route, true);

function markSidebar() {
  $$('#sidebar [data-go]').forEach(a => a.classList.toggle('active', a.dataset.go === route.view && (a.dataset.arg || '') === (route.arg || '')));
  $$('#sb-playlists [data-arg]').forEach(a => a.classList.toggle('playing-ctx', P.ctxRoute?.view === 'playlist' && P.ctxRoute.arg === a.dataset.arg));
}

/* ───────────── Track lists ───────────── */
const COLS = {
  songs: { cols: '36px minmax(0,4fr) minmax(0,3fr) minmax(0,3fr) 84px 96px 52px', heads: [['', ''], ['Title', 'title'], ['Artist', 'artist'], ['Album', 'album'], ['Rating', 'rating'], [ic('cal', 14), 'added'], [ic('clock', 14), 'duration', 'r']] },
  album: { cols: '36px minmax(0,5fr) minmax(0,3fr) 84px 52px', heads: [['#', ''], ['Title', ''], ['Artist', ''], ['Rating', ''], [ic('clock', 14), '', 'r']] },
  short: { cols: '36px minmax(0,4fr) minmax(0,3fr) minmax(0,3fr) 52px', heads: [['', ''], ['Title', ''], ['Artist', ''], ['Album', ''], [ic('clock', 14), '', 'r']] },
};
const sortState = { key: 'artist', dir: 1 };
const sortCache = { sig: '', arr: null };

function sortIdx(idxs, key, dir) {
  if (!key) return idxs;
  const T = S.tracks;
  const by = {
    title: (a, b) => cmp(T[a].title, T[b].title),
    artist: (a, b) => cmp(artistOf(T[a]), artistOf(T[b])) || cmp(T[a].album, T[b].album) || (T[a].discNo - T[b].discNo) || (T[a].trackNo - T[b].trackNo),
    album: (a, b) => cmp(T[a].album, T[b].album) || (T[a].discNo - T[b].discNo) || (T[a].trackNo - T[b].trackNo),
    added: (a, b) => (T[a].added - T[b].added) || cmp(T[a].album, T[b].album) || (T[a].trackNo - T[b].trackNo),
    duration: (a, b) => T[a].duration - T[b].duration,
    rating: (a, b) => (ratingOf(T[a].path) - ratingOf(T[b].path)) || cmp(T[a].title, T[b].title),
  }[key];
  return by ? idxs.slice().sort((a, b) => by(a, b) * dir) : idxs;
}
// All tracks sorted; only recomputed when the order or the library changes
function allSorted() {
  const sig = `${sortState.key}:${sortState.dir}`;
  if (!sortCache.arr || sortCache.sig !== sig) {
    sortCache.arr = sortIdx(S.tracks.map((_, i) => i), sortState.key, sortState.dir);
    sortCache.sig = sig;
  }
  return sortCache.arr;
}
function filterIdx(idxs, q) {
  q = norm(q).trim(); if (!q) return idxs;
  const words = q.split(/\s+/);
  return idxs.filter(i => { const h = S.tracks[i]._s; return words.every(w => h.includes(w)); });
}

/* Virtualized lists: only the visible rows (+ a margin) exist in the DOM.
   Every row is exactly ROW px tall and absolutely positioned inside .tb */
const ROW = 40, ROW_BUFFER = 10;
const vtables = new Map();   // list id → { kind, a, b }
const goAttr = (v, arg) => `data-go="${v}" data-arg="${esc(arg)}"`;

function rowHtml(id, kind, ti, pos) {
  const t = S.tracks[ti];
  if (!t) return '';
  const num = kind === 'album' ? (t.trackNo || pos + 1) : '';
  let cells = `<div class="num"><span class="n">${num}</span><span class="pi">${ic('play', 12)}</span><span class="sp">${ic('speaker', 14)}</span></div>`;
  if (kind === 'album') {
    cells += `<div class="c c-title">${esc(t.title)}${S.fav.has(t.path) ? `<span class="fav-ic">${ic('heartFill', 11)}</span>` : ''}</div><div class="c m">${artistLinks(t._artists)}</div>${starsHtml(t.path)}`;
  } else {
    cells += `<div class="c c-title">${esc(t.title)}${S.fav.has(t.path) ? `<span class="fav-ic">${ic('heartFill', 11)}</span>` : ''}</div>`
      + `<div class="c m">${artistLinks(t._artists)}</div>`
      + `<div class="c m"><span class="lnk" ${goAttr('album', albumKeyOf(t))}>${esc(t.album)}</span></div>`;
    if (kind === 'songs') cells += starsHtml(t.path) + `<div class="c m">${fmtDate(t.added)}</div>`;
  }
  cells += `<div class="c m r">${fmt(t.duration)}</div>`;
  const cls = (ti === P.current ? ' playing' : '') + (sel.list === id && sel.set.has(pos) ? ' sel' : '');
  return `<div class="tr${cls}" draggable="true" data-pos="${pos}" data-t="${ti}" style="top:${pos * ROW}px">${cells}</div>`;
}

function trackTable(id, idxs, kind = 'songs', sortable = false) {
  lists.set(id, idxs);
  vtables.set(id, { kind, a: -1, b: -1 });
  const C = COLS[kind];
  const heads = C.heads.map(([label, key, cls]) => {
    if (!sortable || !key) return `<span class="${cls || ''}">${label}</span>`;
    const on = sortState.key === key;
    return `<span class="${cls || ''} ${on ? 'on' : ''}" data-sort="${key}">${label}${on ? ic(sortState.dir > 0 ? 'sortUp' : 'sortDown', 10) : ''}</span>`;
  }).join('');
  return `<div class="tl" data-list="${id}" style="--cols:${C.cols}"><div class="th">${heads}</div><div class="tb" style="height:${idxs.length * ROW}px"></div></div>`;
}

function paintRows(force = false) {
  const view = $('#view');
  const vh = view.clientHeight, vtop = view.getBoundingClientRect().top;
  for (const tl of view.querySelectorAll('.tl')) {
    const id = tl.dataset.list, vt = vtables.get(id), idxs = lists.get(id);
    if (!vt || !idxs) continue;
    const tb = tl.lastElementChild;
    const off = vtop - tb.getBoundingClientRect().top;
    const a = Math.max(0, Math.floor(off / ROW) - ROW_BUFFER);
    const b = Math.max(a, Math.min(idxs.length, Math.ceil((off + vh) / ROW) + ROW_BUFFER));
    if (!force && a === vt.a && b === vt.b) continue;
    vt.a = a; vt.b = b;
    let html = '';
    for (let p = a; p < b; p++) html += rowHtml(id, vt.kind, idxs[p], p);
    tb.innerHTML = html;
  }
}
let rowsRaf = 0;
const scheduleRows = () => { if (!rowsRaf) rowsRaf = requestAnimationFrame(() => { rowsRaf = 0; paintRows(); }); };

const filterBox = (target, ph = 'Filter') => `<label class="filter">${ic('search', 13)}<input data-filter="${target}" placeholder="${ph}" spellcheck="false"></label>`;
const playBtn = (list, label = 'Play') => `<button class="btn btn-green" data-play="${list}">${label}</button>`;

/* ───────────── Cards ───────────── */
function albumCard(a, sub = 'artist') {
  const s = sub === 'year' ? (a.year || '') : albumArtistHtml(a);
  return `<div class="card" ${goAttr('album', a.key)} data-card-album="${esc(a.key)}">
    ${cvHtml(a.key).replace('</div>', `<div class="ov"><button class="pbtn" data-play-album="${esc(a.key)}">${ic('play', 22)}</button></div></div>`)}
    <div class="t">${esc(a.title)}</div><div class="s">${s}</div></div>`;
}
function artistCard(ar, label, sub) {
  const key = ar.albums[0]?.key;
  return `<div class="card round" ${goAttr('artist', ar.name)}>
    ${cvHtml(key, '', 40, false, ar.name).replace(/<\/div>$/, `<div class="ov"><button class="pbtn" data-play-artist="${esc(ar.name)}">${ic('play', 22)}</button></div></div>`)}
    <div class="t">${esc(label || ar.name)}</div><div class="s">${sub || (label ? 'Artist radio' : plural(ar.tracks.length, 'song', 'songs'))}</div></div>`;
}
const secHead = (title, more) => `<div class="sec-h"><h2>${esc(title)}</h2>${more ? `<a class="more" data-go="${more}">See all</a>` : ''}</div>`;

function emptyLibrary() {
  return `<div class="empty"><div class="big-ic">${ic('folder', 88)}</div>
    <h2>Your library is empty</h2><p>Add a folder with your music to start listening.</p>
    <button class="btn btn-green" data-action="add-folder">Add folder</button></div>`;
}
const notFound = () => `<div class="empty"><h2>Not found</h2><p>The folder may no longer be in your library.</p></div>`;

/* ───────────── Views ───────────── */
const VIEWS = {
  browse() {
    view.classList.add('gradient');
    if (!S.tracks.length) return `<div class="page-h"><h1>Browse</h1></div>${emptyLibrary()}`;
    const recentAdded = [...S.albums].sort((a, b) => b.added - a.added).slice(0, 12);
    const recentPlayed = S.prefs.recent.map(k => S.albumMap.get(k)).filter(Boolean).slice(0, 6);
    const random = shuffleArr([...S.albums]).slice(0, 6);
    return `<div class="page-h"><h1>Browse</h1>
      <div class="tabs"><span class="tab on">Overview</span><span class="tab" data-go="releases">New Releases</span><span class="tab" data-go="albums">Albums</span><span class="tab" data-go="genres">Genres</span><span class="tab" data-go="artists">Artists</span><span class="tab" data-go="radio">Radio</span></div></div>
      ${recentPlayed.length ? secHead('Recently played', 'recent') + `<div class="grid">${recentPlayed.map(a => albumCard(a)).join('')}</div>` : ''}
      ${secHead('Recently added', 'albums')}<div class="grid">${recentAdded.map(a => albumCard(a)).join('')}</div>
      ${secHead('Rediscover')}<div class="grid">${random.map(a => albumCard(a)).join('')}</div>`;
  },

  radio() {
    view.classList.add('gradient');
    const top = [...S.mainArtists].sort((a, b) => b.tracks.length - a.tracks.length);
    return `<div class="page-h"><h1>Radio</h1><div class="row"><span class="sub">Pick an artist and Wax shuffles all of their songs.</span><span class="grow"></span>
      <button class="btn btn-ghost" data-action="shuffle-all">${ic('shuffle', 14)} Shuffle all</button></div></div>
      ${top.length ? gridHtml('radio', top, ar => artistCard(ar, `${ar.name} Radio`)) : emptyLibrary()}`;
  },

  recent() {
    const albums = S.prefs.recent.map(k => S.albumMap.get(k)).filter(Boolean);
    return `<div class="page-h"><h1>Recently played</h1></div>
      ${albums.length ? `<div class="grid">${albums.map(a => albumCard(a)).join('')}</div>` : `<div class="empty"><p>You haven't played anything yet.</p></div>`}`;
  },

  songs() {
    if (!S.tracks.length) return `<div class="page-h"><h1>Songs</h1></div>${emptyLibrary()}`;
    const all = allSorted();
    filters.set('w-songs', q => trackTable('songs', filterIdx(all, q), 'songs', true));
    return `<div class="page-h"><h1>Songs</h1><div class="row">${playBtn('songs')}
      <span class="sub">${plural(S.tracks.length, 'song', 'songs')}</span><span class="grow"></span>${filterBox('w-songs')}</div></div>
      <div id="w-songs">${filters.get('w-songs')('')}</div>`;
  },

  albums() {
    if (!S.tracks.length) return `<div class="page-h"><h1>Albums</h1></div>${emptyLibrary()}`;
    filters.set('w-albums', q => { q = norm(q).trim(); return gridHtml('albums', q ? S.albums.filter(a => a._s.includes(q)) : S.albums, a => albumCard(a)); });
    return `<div class="page-h"><h1>Albums</h1><div class="row"><span class="sub">${plural(S.albums.length, 'album', 'albums')}</span><span class="grow"></span>${filterBox('w-albums')}</div></div>
      <div id="w-albums">${filters.get('w-albums')('')}</div>`;
  },

  artists() {
    if (!S.tracks.length) return `<div class="page-h"><h1>Artists</h1></div>${emptyLibrary()}`;
    const list = S.artists;
    filters.set('w-artists', q => { q = norm(q).trim(); return gridHtml('artists', q ? list.filter(a => norm(a.name).includes(q)) : list, a => artistCard(a)); });
    return `<div class="page-h"><h1>Artists</h1><div class="row"><span class="sub">${plural(list.length, 'artist', 'artists')}</span>
      <span class="grow"></span>${filterBox('w-artists')}</div></div>
      <div id="w-artists">${filters.get('w-artists')('')}</div>`;
  },

  local() {
    const f = S.prefs.folders;
    const counts = f.map(dir => S.tracks.filter(t => t.path.toLowerCase().startsWith(dir.toLowerCase())).length);
    return `<div class="page-h"><h1>Local Files</h1><div class="row">
      <button class="btn btn-green" data-action="add-folder">Add folder</button>
      <button class="btn btn-ghost" data-action="rescan" ${f.length ? '' : 'disabled'}>${S.scanning ? '<span class="spin"></span> Scanning…' : 'Refresh library'}</button>
      <span class="grow"></span><span class="sub">${plural(S.tracks.length, 'song', 'songs')} · ${plural(S.albums.length, 'album', 'albums')}</span></div></div>
      ${f.length ? `<div class="label" style="margin-bottom:8px">Music folders</div><div class="folder-list">${f.map((d, i) => `
        <div class="folder"><span style="color:var(--mut)">${ic('folder', 20)}</span><span class="p">${esc(d)}</span>
        <span class="m">${plural(counts[i], 'song', 'songs')}</span>
        <button class="ib" title="Show in Explorer" data-action="reveal-folder" data-arg="${esc(d)}">${ic('right', 14)}</button>
        <button class="ib" title="Remove folder" data-action="remove-folder" data-arg="${esc(d)}">${ic('x', 14)}</button></div>`).join('')}</div>`
        : `<div class="empty"><div class="big-ic">${ic('folder', 88)}</div><h2>No folders</h2><p>Add the folders where you keep your music (MP3, FLAC, OGG, OPUS, M4A, WAV).</p></div>`}`;
  },

  album(key) {
    const a = S.albumMap.get(key); if (!a) return notFound();
    const discs = new Set(a.tracks.map(i => S.tracks[i].discNo || 1)).size;
    return `<div class="hero"><div class="hero-bg">${cvHtml(a.key)}</div>${cvHtml(a.key, 'art', 80, true)}
      <div class="info"><div class="label">Album</div><h1>${esc(a.title)}</h1>
      <div class="by">By ${albumArtistHtml(a)}</div>
      <div class="meta">${[a.year || '', plural(a.tracks.length, 'song', 'songs') + ', ' + fmtLong(a.duration), discs > 1 ? `${discs} discs` : ''].filter(Boolean).join(' • ')}</div>
      <div class="btns">${playBtn('album')}<button class="btn-circle" data-menu-album="${esc(a.key)}" title="More">${ic('dots', 16)}</button></div></div></div>
      <div id="album-about"></div>
      ${trackTable('album', a.tracks, 'album')}${(setTimeout(() => albumAbout(a)), '')}`;
  },

  artist(name, r = {}) {
    const ar = S.artistMap.get(name) || S.artistMap.get(ownerOf(name)) || S.artistMap.get(resolveArtist(name)[0]); if (!ar) return notFound();
    name = ar.name;
    const tab = r.tab || 'overview';
    const plays = S.prefs.plays;
    const popular = [...ar.tracks].sort((x, y) => ((plays[S.tracks[y].path] || 0) - (plays[S.tracks[x].path] || 0))).slice(0, 10);
    setTimeout(() => artistExtras(ar, tab, popular));
    lists.set('artist-all', ar.albums.flatMap(a => a.tracks.filter(i => S.tracks[i]._owner === name)));
    const genres = [...new Set(ar.tracks.flatMap(i => S.tracks[i]._g))].slice(0, 4);
    return `<div class="artist-hero"><div class="hero-bg">${cvHtml(ar.albums[0]?.key, '', 40, true, ar.name)}</div>
      <div style="position:relative">${cvHtml(ar.albums[0]?.key, 'avatar-xl', 60, true, ar.name)}<div class="label">Artist</div><h1>${esc(ar.name)}</h1>
      <div class="meta">${plural(ar.albums.length, 'album', 'albums')} • ${plural(ar.tracks.length, 'song', 'songs')}${genres.length ? ' • ' + genres.map(g => `<span class="lnk" data-go="genre" data-arg="${esc(norm(g))}">${esc(g)}</span>`).join(', ') : ''}</div>
      <div class="btns">${playBtn('artist-all')}<button class="btn btn-ghost" data-play-artist="${esc(ar.name)}">${ic('shuffle', 14)} Radio</button></div>
      ${artistTabs(tab)}</div></div>
      ${tab === 'related' ? `<div id="artist-related"><div class="ly-status"><span class="spin"></span></div></div>`
        : tab === 'about' ? `<div id="artist-about"><div class="ly-status"><span class="spin"></span></div></div>`
        : `<div id="artist-pop">${popularHtml(popular, '')}</div>
      ${secHead('Albums')}<div class="grid">${ar.albums.map(a => albumCard(a, 'year')).join('')}</div>`}`;
  },

  playlist(id) {
    const pl = S.prefs.playlists.find(p => p.id === id); if (!pl) return notFound();
    const idxs = pl.tracks.map(p => S.byPath.get(p)).filter(i => i !== undefined);
    const keys = [...new Set(idxs.map(i => albumKeyOf(S.tracks[i])))].slice(0, 4);
    const art = pl.image
      ? `<div class="cv art has-img pl-img" data-action="pl-edit" data-arg="${esc(pl.id)}" style="background-image:url('${fileUrl(pl.image)}')"></div>`
      : keys.length >= 4
        ? `<div class="art mosaic" data-action="pl-edit" data-arg="${esc(pl.id)}">${keys.map(k => cvHtml(k, '', 24)).join('')}</div>`
        : cvHtml(keys[0], 'art', 80, true).replace('class="cv art"', `class="cv art" data-action="pl-edit" data-arg="${esc(pl.id)}"`);
    const dur = idxs.reduce((s, i) => s + S.tracks[i].duration, 0);
    filters.set('w-pl', q => trackTable('playlist', filterIdx(idxs, q), 'songs'));
    return `<div class="hero"><div class="hero-bg">${cvHtml(keys[0])}</div>${art}
      <div class="info"><div class="label">Playlist</div><h1>${esc(pl.name)}</h1>
      ${pl.description ? `<div class="pl-desc">${esc(pl.description)}</div>` : ''}
      <div class="by">Created by <span class="lnk" style="cursor:default">You</span></div>
      <div class="meta">${plural(idxs.length, 'song', 'songs')}${idxs.length ? ', ' + fmtLong(dur) : ''}</div>
      <div class="btns">${playBtn('playlist')}<button class="btn-circle" data-menu-playlist="${esc(pl.id)}" title="More">${ic('dots', 16)}</button>
      <span class="grow"></span></div></div></div>
      ${idxs.length ? `<div class="row" style="display:flex;justify-content:flex-end;margin:4px 0 10px">${filterBox('w-pl')}</div><div id="w-pl">${filters.get('w-pl')('')}</div>`
        : `<div class="empty"><h2>It's a bit empty here</h2><p>Drag songs onto this playlist in the sidebar, or right-click → "Add to playlist".</p></div>`}`;
  },

  search(q) {
    const nq = norm(q).trim();
    if (!nq) return `<div class="page-h"><h1>Search</h1></div>`;
    const songs = filterIdx(S.tracks.map((_, i) => i), q).slice(0, 50);
    const artists = S.artists.filter(a => norm(a.name).includes(nq)).slice(0, 6);
    const albums = S.albums.filter(a => norm(a.title + ' ' + a.artist).includes(nq)).slice(0, 12);
    setTimeout(() => lyricSearch(q));
    if (!songs.length && !artists.length && !albums.length)
      return `<div class="empty"><h2>No results found for "${esc(q)}"</h2><p>Check the spelling or try different keywords.</p></div><div id="lyr-hits"></div>`;
    return `<div class="page-h"><div class="label">Search results for</div><h1>"${esc(q)}"</h1></div><div id="lyr-hits"></div>
      ${songs.length ? secHead('Songs') + trackTable('search', songs, 'short') : ''}
      ${artists.length ? secHead('Artists') + `<div class="grid">${artists.map(a => artistCard(a)).join('')}</div>` : ''}
      ${albums.length ? secHead('Albums') + `<div class="grid">${albums.map(a => albumCard(a)).join('')}</div>` : ''}`;
  },

  queue() {
    const cur = P.current >= 0 ? [P.current] : [];
    const rest = P.order.slice(P.pos + 1).map(o => P.ctx[o]);
    return `<div class="page-h"><h1>Play Queue</h1></div>
      ${secHead('Now playing')}${cur.length ? trackTable('q-now', cur, 'short') : '<p class="sub" style="color:var(--mut)">Nothing for now.</p>'}
      ${P.upNext.length ? `<div class="sec-h"><h2>Next in queue</h2><a class="more" data-action="clear-queue">Clear</a></div>${trackTable('q-next', P.upNext, 'short')}` : ''}
      ${rest.length ? secHead('Next up') + trackTable('q-ctx', rest, 'short') : ''}`;
  },
};

/* ───────────── Player ───────────── */
const audio = new WaxAudio();
const Sleep = { at: 0, endOfTrack: false, tick: 0 };   // sleep timer (see extras.js)
const P = { ctx: [], order: [], pos: -1, upNext: [], current: -1, ctxRoute: null, autoDJ: false };

function buildOrder(start) {
  const n = P.ctx.length;
  if (S.prefs.shuffle) {
    const rest = shuffleArr([...Array(n).keys()].filter(i => i !== start));
    P.order = start >= 0 ? [start, ...rest] : rest; P.pos = 0;
  } else { P.order = [...Array(n).keys()]; P.pos = Math.max(0, start); }
}

function playList(idxs, start = 0, ctxRoute = route) {
  if (!idxs?.length) return;
  P.ctx = idxs.slice(); P.ctxRoute = { ...ctxRoute }; P.autoDJ = false;
  if (start < 0) start = S.prefs.shuffle ? Math.floor(Math.random() * idxs.length) : 0;
  buildOrder(start);
  playTrack(P.ctx[P.order[P.pos]]);
  markSidebar();
}

/* Picks the next track and advances the state. Returns { i, stop }.
   stop = true: the first track is loaded but not played (end of the list). */
function takeNext(auto) {
  if (auto && S.prefs.repeat === 'one' && P.current >= 0) return { i: P.current, stop: false };
  if (P.upNext.length) return { i: P.upNext.shift(), stop: false };
  if (!P.order.length) return { i: -1, stop: true };
  let np = P.pos + 1;
  if (np >= P.order.length && auto && S.prefs.repeat === 'off' && S.prefs.autoDJ) extendAutoDJ();
  if (np >= P.order.length) {
    if (S.prefs.shuffle) buildOrder(-1);
    np = 0;
    if (S.prefs.repeat === 'off' && auto) { P.pos = 0; return { i: P.ctx[P.order[0]], stop: true }; }
  }
  P.pos = np;
  return { i: P.ctx[P.order[np]], stop: false };
}
// Same thing without changing the state (used to preload the engine)
function peekNext() {
  const snap = { pos: P.pos, order: P.order, upNext: P.upNext.slice() };
  const r = takeNext(true);
  P.pos = snap.pos; P.order = snap.order; P.upNext = snap.upNext;
  return r;
}

/* Volume normalization: ReplayGain from the tags or, if missing, Wax's own analysis
   (EBU R128, −18 LUFS target like ReplayGain 2.0). */
S.loud = {};
const albumLoudCache = new Map();
function albumLufs(key) {
  if (albumLoudCache.has(key)) return albumLoudCache.get(key);
  const a = S.albumMap.get(key);
  let e = 0, d = 0, peak = 0;
  for (const i of a?.tracks || []) {
    const l = S.loud[S.tracks[i].path]; if (!l) continue;
    const dur = S.tracks[i].duration || 1;
    e += dur * Math.pow(10, l.lufs / 10); d += dur; peak = Math.max(peak, l.peak);
  }
  const r = d ? { lufs: 10 * Math.log10(e / d), peak } : null;
  albumLoudCache.set(key, r);
  return r;
}
function gainFor(t) {
  const mode = S.prefs.normalize || 'off';
  if (!t || mode === 'off') return 0;
  let g = null, peak = 0;
  const rg = t.rg;
  if (rg) {
    if (mode === 'album' && rg.album != null) { g = rg.album; peak = rg.albumPeak || 0; }
    else if (rg.track != null) { g = rg.track; peak = rg.trackPeak || 0; }
  }
  if (g == null) {
    const l = mode === 'album' ? (albumLufs(albumKeyOf(t)) || S.loud[t.path]) : S.loud[t.path];
    if (l) { g = -18 - l.lufs; peak = l.peak; }
  }
  if (g == null) return 0;
  g += S.prefs.preamp || 0;
  if (S.prefs.preventClip !== false && peak > 0) g = Math.min(g, -20 * Math.log10(peak));
  return Math.max(-24, Math.min(12, g));
}
// Crossfade: not applied between consecutive tracks of the same album (live and concept albums)
function crossfadeFor(a, b) {
  const secs = +S.prefs.crossfade || 0;
  if (!secs || a < 0 || b < 0) return 0;
  const x = S.tracks[a], y = S.tracks[b];
  if (S.prefs.xfadeSameAlbum !== true && albumKeyOf(x) === albumKeyOf(y) && (y.trackNo === x.trackNo + 1 || y.discNo !== x.discNo)) return 0;
  return Math.min(secs, (x.duration || 0) / 3) * 1000;
}
let nextTimer = 0;
function syncNext() {
  clearTimeout(nextTimer);
  nextTimer = setTimeout(() => {
    if (P.current < 0) return;
    const r = peekNext();
    const t = !r.stop && r.i >= 0 && !Sleep.endOfTrack ? S.tracks[r.i] : null;
    audio.setNext(t, { gainDb: gainFor(t), crossfadeMs: t ? crossfadeFor(P.current, r.i) : 0 });
  }, 150);
}

function playTrack(i, autoplay = true, opts = {}) {
  const t = S.tracks[i]; if (!t) return;
  finishListen();
  P.current = i;
  if (!opts.engineLoaded) audio.load(t, { autoplay, startAt: opts.startAt || 0, gainDb: gainFor(t) });
  if (autoplay) {
    S.prefs.plays[t.path] = (S.prefs.plays[t.path] || 0) + 1;
    const k = albumKeyOf(t);
    S.prefs.recent = [k, ...S.prefs.recent.filter(x => x !== k)].slice(0, 24);
    startListen(i);
  }
  S.prefs.last = t.path;
  savePrefs();
  if (Object.keys(S.prefs.pendingEmbeds || {}).length) flushPendingEmbeds();
  updateNowPlaying();
  markPlaying();
  syncNext();
  if (autoplay) announceTrack(t);
  discordSync();
}

function next(auto = false) {
  const r = takeNext(auto);
  if (r.i < 0) return;
  const sleeping = auto && Sleep.endOfTrack;
  if (sleeping) sleepDone();
  playTrack(r.i, !r.stop && !sleeping);
  refreshQueueView();
}
function prev() {
  if (audio.currentTime > 3 || P.pos <= 0) { audio.currentTime = 0; return; }
  P.pos--; playTrack(P.ctx[P.order[P.pos]]); refreshQueueView();
}
function togglePlay() {
  if (P.current < 0) {
    if (S.tracks.length) playList(lists.get([...lists.keys()][0]) || S.tracks.map((_, i) => i), -1);
    return;
  }
  audio.paused ? audio.play() : audio.pause();
}
function refreshQueueView() { if (route.view === 'queue') rerender(); }

// The engine moved on to the preloaded track by itself: update the state without reloading audio
audio.addEventListener('advanced', e => {
  const r = takeNext(true);
  let i = r.i;
  if (S.tracks[i]?.path !== e.detail.path) i = S.byPath.get(e.detail.path) ?? i;
  playTrack(i, true, { engineLoaded: true });
  refreshQueueView();
});

function playAlbum(key, start = 0) { const a = S.albumMap.get(key); if (a) playList(a.tracks, start, { view: 'album', arg: key }); }
function playArtistRadio(name) {
  const ar = S.artistMap.get(name) || S.artistMap.get(ownerOf(name)) || S.artistMap.get(resolveArtist(name)[0]); if (!ar) return;
  const was = S.prefs.shuffle; S.prefs.shuffle = true;
  playList(ar.tracks, -1, { view: 'artist', arg: ar.name });
  S.prefs.shuffle = was; syncModeButtons();
}

/* Auto DJ: when the list ends, keeps playing similar music */
function extendAutoDJ() {
  const seed = S.tracks[P.current]; if (!seed) return false;
  const mode = S.prefs.autoDJMode || 'mix';
  const recent = new Set(P.ctx.slice(-150));
  const sg = new Set(seed._g.map(norm));
  const cand = [];
  S.tracks.forEach((t, i) => {
    if (recent.has(i)) return;
    const sameArtist = t._owner === seed._owner;
    const genre = t._g.some(g => sg.has(norm(g)));
    const era = seed.year && t.year && Math.abs(t.year - seed.year) <= 4;
    if (mode === 'artist' && !sameArtist) return;
    if (mode === 'genre' && !genre && !sameArtist) return;
    if (mode === 'era' && !era) return;
    const score = (sameArtist ? 3 : 0) + (genre ? 2.5 : 0) + (era ? 1 : 0) + (S.prefs.favorites?.includes(t.path) ? 1 : 0) + Math.random() * 2.5;
    cand.push([score, i]);
  });
  if (!cand.length) return false;
  cand.sort((a, b) => b[0] - a[0]);
  // No more than 3 tracks by the same artist per batch
  const picked = [], perArtist = new Map();
  for (const [, i] of cand) {
    const o = S.tracks[i]._owner, n = perArtist.get(o) || 0;
    if (mode !== 'artist' && n >= 3) continue;
    perArtist.set(o, n + 1); picked.push(i);
    if (picked.length >= 25) break;
  }
  const base = P.ctx.length;
  P.ctx.push(...shuffleArr(picked));
  P.order.push(...picked.map((_, k) => base + k));
  if (!P.autoDJ) { P.autoDJ = true; toast('Auto DJ: keeps playing similar music'); }
  return true;
}

/* Listening history and Last.fm / ListenBrainz scrobbling */
const LS = { idx: -1, start: 0, played: 0, last: 0 };
function startListen(i) { LS.idx = i; LS.start = Math.floor(Date.now() / 1000); LS.played = 0; LS.last = 0; scrobbleNow(S.tracks[i]); }
function finishListen() {
  const t = S.tracks[LS.idx];
  if (t && LS.played >= 30 && (LS.played >= (t.duration || 0) * 0.5 || LS.played >= 240)) {
    invoke('append_history', { listen: { p: t.path, t: LS.start, s: Math.round(LS.played) } }).catch(() => {});
    Stats.dirty = true;
    scrobbleDone(t, LS.start);
  }
  LS.idx = -1;
}
addEventListener('beforeunload', finishListen);
function scrobbleCreds() {
  const p = S.prefs;
  return { lbToken: p.lbToken || '', lastfmKey: p.lastfmKey || '', lastfmSecret: p.lastfmSecret || '', lastfmSession: p.lastfmSession || '' };
}
const scrobbleOn = () => !!(S.prefs.lbToken || S.prefs.lastfmSession);
function scrobbleTrack(t, ts) {
  return { artist: t.artist, title: t.title, album: t.album, albumArtist: t.albumArtist || '', duration: Math.round(t.duration || 0), timestamp: ts };
}
function scrobbleNow(t) { if (t && scrobbleOn()) invoke('scrobble', { creds: scrobbleCreds(), track: scrobbleTrack(t, 0), nowPlaying: true }).catch(() => {}); }
function scrobbleDone(t, ts) { if (scrobbleOn()) invoke('scrobble', { creds: scrobbleCreds(), track: scrobbleTrack(t, ts), nowPlaying: false }).catch(() => {}); }

/* Track-change notification, Windows media controls, tray, mini player */
function announceTrack(t) {
  const key = albumKeyOf(t);
  albumCover(key).then(c => {
    invoke('media_update', { title: t.title, artist: t.artist, album: t.album, cover: c?.path || null, durationMs: Math.round((t.duration || 0) * 1000), playing: !audio.paused, posMs: 0, meta: true }).catch(() => {});
    if (S.prefs.notify !== false && (document.hidden || !document.hasFocus())) {
      invoke('osd_show', { payload: { title: t.title, artist: t.artist, album: t.album, cover: c?.url || null, accent: accent() } }).catch(() => {});
    }
    miniState();
  });
  invoke('tray_tooltip', { text: `${t.title} — ${t.artist}` }).catch(() => {});
}
function miniState() {
  const t = S.tracks[P.current];
  if (!t) return;
  albumCover(albumKeyOf(t)).then(c => TAURI.event.emit('np-state', {
    title: t.title, artist: t.artist, cover: c?.url || null, playing: !audio.paused,
    pos: audio.currentTime || 0, dur: audio.duration || t.duration || 0,
    fav: isFav(t.path), shuffle: S.prefs.shuffle, repeat: S.prefs.repeat, accent: accent(),
  }));
}

// Accent color (Settings → Appearance) for the mini player too
const accent = () => S.prefs.accent || '#1db954';
const npTitle = $('#np-title'), npArtist = $('#np-artist'), npCover = $('#np-cover'), sbCover = $('#sb-cover-img');
function updateNowPlaying() {
  const t = S.tracks[P.current];
  document.body.classList.toggle('empty-np', !t);
  if (!t) { npTitle.textContent = npArtist.innerHTML = ''; return; }
  const key = albumKeyOf(t);
  npTitle.textContent = t.title; npTitle.dataset.go = 'album'; npTitle.dataset.arg = key;
  npArtist.innerHTML = artistLinks(t._artists);
  npCover.dataset.cover = sbCover.dataset.cover = key;
  paintCover(npCover); paintCover(sbCover);
  syncFavButton();
  document.title = `${t.title} · ${t.artist}`;
  if (lyricsOpen() && L.path !== t.path) refreshLyricsPanel();
  npFullSync();
}

function markPlaying() {
  $$('.tr', view).forEach(r => r.classList.toggle('playing', +r.dataset.t === P.current));
  const ck = P.current >= 0 ? albumKeyOf(S.tracks[P.current]) : null;
  $$('[data-card-album]', view).forEach(c => {
    const on = c.dataset.cardAlbum === ck && !audio.paused;
    c.classList.toggle('is-playing', on);
    const b = $('.pbtn', c); if (b) b.innerHTML = ic(on ? 'pause' : 'play', 22);
  });
}

const bPlay = $('#b-play');
function syncPlayButton() {
  const paused = audio.paused;
  setIcon(bPlay, paused ? 'play' : 'pause');
  bPlay.classList.toggle('paused', paused);
  bPlay.title = paused ? 'Play' : 'Pause';
  markPlaying();
  const t = S.tracks[P.current];
  if (t) invoke('media_update', { title: t.title, artist: t.artist, album: t.album, cover: null, durationMs: Math.round((t.duration || 0) * 1000), playing: !paused, posMs: Math.round((audio.currentTime || 0) * 1000), meta: false }).catch(() => {});
  invoke('taskbar_playing', { playing: !paused }).catch(() => {});
  miniState();
  discordSync();
}
audio.addEventListener('play', syncPlayButton);
audio.addEventListener('pause', syncPlayButton);
audio.addEventListener('ended', () => next(true));
let errorStreak = 0;
audio.addEventListener('playing', () => { errorStreak = 0; });
audio.addEventListener('error', () => {
  if (P.current < 0) return;
  toast(`Couldn't play "${S.tracks[P.current]?.title}"`);
  if (++errorStreak < 5) setTimeout(() => next(true), 800);   // don't skip forever if everything fails
});

/* Sliders (progress and volume) */
function makeSlider(el, { onInput, onCommit }) {
  const ratio = e => { const r = $('.rail', el).getBoundingClientRect(); return Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)); };
  const set = v => el.style.setProperty('--v', v);
  el.addEventListener('mousedown', e => {
    if (e.button !== 0) return;
    el.classList.add('drag');
    const move = ev => { const v = ratio(ev); set(v); onInput?.(v); };
    const up = ev => { el.classList.remove('drag'); removeEventListener('mousemove', move); removeEventListener('mouseup', up); onCommit?.(ratio(ev)); };
    addEventListener('mousemove', move); addEventListener('mouseup', up);
    move(e);
  });
  el.addEventListener('wheel', e => { e.preventDefault(); onWheel?.(e); }, { passive: false });
  let onWheel = null;
  return { set, dragging: () => el.classList.contains('drag'), wheel: fn => (onWheel = fn) };
}

const tCur = $('#t-cur'), tDur = $('#t-dur');
const seek = makeSlider($('#seek'), {
  onInput: v => { tCur.textContent = fmt(v * (audio.duration || 0)); },
  onCommit: v => { if (audio.duration) audio.currentTime = v * audio.duration; },
});
let lastMini = 0;
audio.addEventListener('timeupdate', () => {
  const pos = audio.currentTime || 0;
  if (!audio.paused) { const d = pos - LS.last; if (d > 0 && d < 2) LS.played += d; }
  LS.last = pos;
  if (Date.now() - lastMini > 1000) { lastMini = Date.now(); miniState(); }
  if (seek.dragging()) return;
  const d = audio.duration || S.tracks[P.current]?.duration || 0;
  seek.set(d ? pos / d : 0);
  tCur.textContent = fmt(pos);
  tDur.textContent = fmt(d);
});
audio.addEventListener('seeked', () => { LS.last = audio.currentTime || 0; });
audio.addEventListener('loadedmetadata', () => { tDur.textContent = fmt(audio.duration); });

const bMute = $('#b-mute');
const vol = makeSlider($('#vol'), { onInput: v => setVolume(v), onCommit: v => setVolume(v) });
vol.wheel(e => setVolume(S.prefs.volume + (e.deltaY < 0 ? .05 : -.05)));
function setVolume(v, muted = false) {
  v = Math.min(1, Math.max(0, v));
  S.prefs.volume = v; S.prefs.muted = muted;
  audio.setVolume(muted ? 0 : v * v);      // a curve that sounds more natural
  vol.set(muted ? 0 : v);
  setIcon(bMute, muted || v === 0 ? 'mute' : v < .5 ? 'vol1' : 'vol');
  savePrefs();
}
bMute.onclick = () => setVolume(S.prefs.volume || .5, !S.prefs.muted);

function syncModeButtons() {
  $('#b-shuffle').classList.toggle('on', S.prefs.shuffle);
  const r = $('#b-repeat');
  r.classList.toggle('on', S.prefs.repeat !== 'off');
  r.title = { off: 'Repeat', all: 'Repeat one', one: "Don't repeat" }[S.prefs.repeat];
  r.innerHTML = ic('repeat') + (S.prefs.repeat === 'one' ? '<span class="badge">1</span>' : '');
}
function toggleShuffle() {
  S.prefs.shuffle = !S.prefs.shuffle;
  if (P.ctx.length) { const cur = P.order[P.pos]; buildOrder(cur); }
  syncModeButtons(); savePrefs(); refreshQueueView(); syncNext(); miniState();
}
function cycleRepeat() {
  S.prefs.repeat = { off: 'all', all: 'one', one: 'off' }[S.prefs.repeat];
  syncModeButtons(); savePrefs(); syncNext(); miniState();
}
$('#b-shuffle').onclick = toggleShuffle;
$('#b-repeat').onclick = cycleRepeat;
bPlay.onclick = togglePlay;
$('#b-next').onclick = () => next();
$('#b-prev').onclick = prev;
$('#b-queue').onclick = () => route.view === 'queue' ? back() : go({ view: 'queue' });
$('#b-eq').onclick = () => route.view === 'eq' ? back() : go({ view: 'eq' });
$('#b-fav').onclick = () => { const t = S.tracks[P.current]; if (t) toggleFav([P.current]); };
$('#b-mini').onclick = () => invoke('toggle_mini', { show: true });

/* Media keys, the Windows media panel, tray, taskbar buttons,
   global shortcuts and the mini player all arrive as 'media-key' */
function mediaKey(k) {
  switch (k) {
    case 'toggle': togglePlay(); break;
    case 'play': if (audio.paused) togglePlay(); break;
    case 'pause': if (!audio.paused) audio.pause(); break;
    case 'next': next(); break;
    case 'prev': prev(); break;
    case 'volup': setVolume(S.prefs.volume + .05); break;
    case 'voldown': setVolume(S.prefs.volume - .05); break;
    case 'mute': setVolume(S.prefs.volume || .5, !S.prefs.muted); break;
    case 'like': if (P.current >= 0) toggleFav([P.current]); break;
    case 'shuffle': toggleShuffle(); break;
    case 'repeat': cycleRepeat(); break;
    case 'mini': invoke('toggle_mini', { show: true }); break;
    case 'restore': invoke('toggle_mini', { show: false }); break;
    case 'show': invoke('show_main_window'); break;
  }
}
TAURI.event.listen('media-key', e => mediaKey(e.payload));
TAURI.event.listen('mini-cmd', e => {
  if (e.payload?.seek != null) { const d = audio.duration || 0; if (d) audio.currentTime = e.payload.seek * d; }
  else if (e.payload === 'ready') miniState();
  else mediaKey(e.payload);
});
TAURI.event.listen('media-seek', e => { audio.currentTime = e.payload / 1000; });

function setBigCover(on) { S.prefs.bigCover = on; document.body.classList.toggle('big-cover', on); savePrefs(); }
$('#cover-expand').onclick = e => { e.stopPropagation(); setBigCover(true); };
$('#cover-shrink').onclick = () => setBigCover(false);

/* ───────────── Lyrics (LRCGET / LRCLIB) ───────────── */
const lyBody = $('#ly-body');
const lyricsOpen = () => document.body.classList.contains('ly-open');
const L = { path: null, lines: [], els: [], synced: false, idx: -1, userScroll: 0, raf: 0 };

function parseLrc(text) {
  let offset = 0;
  const out = [];
  for (const raw of text.split(/\r?\n/)) {
    const off = raw.match(/^\s*\[offset:\s*([+-]?\d+)\s*\]/i);
    if (off) { offset = +off[1] / 1000; continue; }
    const tags = [...raw.matchAll(/\[(\d+):(\d+(?:[.:]\d+)?)\]/g)];
    if (!tags.length) continue;
    const line = raw.replace(/\[[^\]]*\]/g, '').trim();
    for (const m of tags) out.push({ t: +m[1] * 60 + parseFloat(m[2].replace(':', '.')) - offset, text: line });
  }
  return out.sort((a, b) => a.t - b.t);
}

async function loadLyrics(t, force = false) {
  L.path = t.path; L.lines = []; L.els = []; L.synced = false; L.idx = -1;
  const online = S.prefs.lyricsOnline !== false;
  let r;
  try {
    r = await invoke('get_lyrics', { path: t.path, title: t.title, artist: t.artist, album: t.album, duration: t.duration || 0, online, force });
  } catch (e) { r = { error: String(e) }; }

  const raw = r.synced || r.plain || '';
  // Automatic saving: only when the lyrics were just downloaded from LRCLIB
  if (r.fresh && raw && !t.cue) {
    const kinds = [S.prefs.saveLrc && 'lrc', S.prefs.embedLyrics && 'embed'].filter(Boolean);
    if (kinds.length) saveLyrics(t, raw, kinds, true);
  }

  const body = lyBody;
  if (!lyricsOpen() || L.path !== t.path) return;
  L.raw = raw;

  // Footer pinned to the panel: lyrics source + save .lrc / embed / search again
  const hasLrc = r.source === 'Local .lrc file', inTags = r.source === 'File tags';
  $('#ly-foot').innerHTML = raw ? `
    <div class="ly-src">Lyrics from ${esc(r.source)}</div>
    <div class="ly-actions">
      <button class="btn btn-ghost" data-action="lyrics-save" data-arg="lrc" ${hasLrc ? 'disabled' : ''}>${hasLrc ? '✓ Has .lrc' : 'Save .lrc'}</button>
      <button class="btn btn-ghost" data-action="lyrics-save" data-arg="embed" ${inTags ? 'disabled' : ''}>${inTags ? '✓ Embedded' : 'Embed'}</button>
      <button class="ib" data-action="lyrics-retry" title="Search LRCLIB again">${ic('refresh', 16)}</button>
    </div>` : '';
  const status = (h, p, retry) => `<div class="ly-status"><h2>${h}</h2><p>${p}</p>${retry ? `<button class="btn btn-ghost" data-action="lyrics-retry">Search again</button>` : ''}</div>`;

  if (r.synced) { L.lines = parseLrc(r.synced); L.synced = L.lines.length > 0; }
  if (L.synced) {
    body.innerHTML = L.lines.map((l, i) => `<p class="ly-line" data-ly="${i}">${esc(l.text) || '♪'}</p>`).join('');
    L.els = $$('.ly-line', body);
    syncLyrics(true);
    lyricsLoop();
  } else if (r.plain) {
    body.innerHTML = `<div class="ly-note">These lyrics aren't synced.</div><div class="ly-plain">${r.plain.split(/\r?\n/).map(x => `<p class="ly-line">${esc(x) || '&nbsp;'}</p>`).join('')}</div>`;
  } else if (r.instrumental) {
    body.innerHTML = status('Instrumental', 'This song has no lyrics. Enjoy the music.');
  } else if (r.error) {
    body.innerHTML = status("Couldn't reach LRCLIB", 'Check your internet connection and try again.', true);
  } else if (!online) {
    body.innerHTML = status('No local lyrics', 'There is no .lrc file next to the song and online search is turned off (Settings).');
  } else {
    body.innerHTML = status('No lyrics found', "LRCLIB doesn't have the lyrics for this song yet.", true);
  }
}

/* Save lyrics as .lrc and/or embed them in the tags.
   Embedding rewrites the tags and can shift the audio data, so for the track loaded
   in the player it is deferred until the track changes. */
async function saveLyrics(t, text, kinds, auto = false) {
  if (t.cue) { toast("Songs inside a CUE sheet can't have their own .lrc or embedded lyrics"); return; }
  const lrc = kinds.includes('lrc');
  let embed = kinds.includes('embed');
  const deferred = embed && S.tracks[P.current]?.path === t.path;
  if (deferred) { S.prefs.pendingEmbeds[t.path] = text; savePrefs(); embed = false; }
  try {
    if (lrc || embed) await invoke('save_lyrics', { path: t.path, text, lrc, embed });
    const done = [lrc && '.lrc file saved', embed && 'lyrics embedded'].filter(Boolean).join(' and ');
    if (deferred) toast(`${done ? done[0].toUpperCase() + done.slice(1) + '. ' : ''}The lyrics will be embedded when the track changes`);
    else if (done) toast(auto ? `Lyrics for "${t.title}": ${done}` : done[0].toUpperCase() + done.slice(1));
  } catch (e) { toast(`Couldn't save the lyrics: ${e}`, 4000); }
}
async function flushPendingEmbeds() {
  const cur = S.tracks[P.current]?.path;
  for (const [path, text] of Object.entries(S.prefs.pendingEmbeds || {})) {
    if (path === cur) continue;
    delete S.prefs.pendingEmbeds[path];
    if (!S.byPath.has(path)) continue;
    try { await invoke('save_lyrics', { path, text, lrc: false, embed: true }); }
    catch (e) { console.warn('embed lyrics', path, e); }
  }
  savePrefs();
}

function syncLyrics(jump = false) {
  if (!L.synced || !lyricsOpen()) return;
  const ct = audio.currentTime + 0.2;
  let lo = 0, hi = L.lines.length - 1, idx = -1;
  while (lo <= hi) { const m = (lo + hi) >> 1; if (L.lines[m].t <= ct) { idx = m; lo = m + 1; } else hi = m - 1; }
  if (idx === L.idx && !jump) return;
  const prevIdx = L.idx; L.idx = idx;
  if (jump) L.els.forEach((el, i) => { el.classList.toggle('on', i === idx); el.classList.toggle('past', i < idx); });
  else {
    const [a, b] = [Math.min(prevIdx, idx), Math.max(prevIdx, idx)];
    for (let i = Math.max(0, a); i <= b; i++) { const el = L.els[i]; if (el) { el.classList.toggle('on', i === idx); el.classList.toggle('past', i < idx); } }
  }
  const el = L.els[Math.max(0, idx)];
  if (el && Date.now() - L.userScroll > 3000) {
    lyBody.scrollTo({ top: el.offsetTop - lyBody.clientHeight * 0.4, behavior: jump ? 'auto' : 'smooth' });
  }
}
function lyricsLoop() {
  cancelAnimationFrame(L.raf);
  const tick = () => { if (!lyricsOpen() || audio.paused) { L.raf = 0; return; } syncLyrics(); L.raf = requestAnimationFrame(tick); };
  L.raf = requestAnimationFrame(tick);
}
audio.addEventListener('play', lyricsLoop);
audio.addEventListener('seeked', () => syncLyrics(true));
lyBody.addEventListener('wheel', () => { L.userScroll = Date.now(); }, { passive: true });
lyBody.addEventListener('click', e => {
  const line = e.target.closest('[data-ly]'); if (!line || !L.synced) return;
  audio.currentTime = Math.max(0, L.lines[+line.dataset.ly].t);
  L.userScroll = 0;
  if (audio.paused) audio.play();
});
/* Lyrics panel: a fixed column on the right */
function setLyricsPanel(open) {
  S.prefs.lyricsPanel = open; savePrefs();
  document.body.classList.toggle('ly-open', open);
  $('#b-lyrics').classList.toggle('on', open);
  if (open) refreshLyricsPanel(); else { cancelAnimationFrame(L.raf); L.path = null; }
  scheduleRows();
}
function refreshLyricsPanel(force = false) {
  const t = S.tracks[P.current];
  $('#ly-track').textContent = t ? `${t.title} · ${t.artist}` : '';
  lyBody.scrollTop = 0; L.userScroll = 0;
  if (!t) { L.path = null; lyBody.innerHTML = '<div class="ly-status"><p>Play a song to see its lyrics.</p></div>'; return; }
  lyBody.innerHTML = '<div class="ly-status"><span class="spin"></span></div>';
  $('#ly-foot').innerHTML = '';
  loadLyrics(t, force);
}
$('#b-lyrics').onclick = () => setLyricsPanel(!lyricsOpen());
$('#ly-close').onclick = () => setLyricsPanel(false);

// Width adjustable by dragging the panel's left edge
function setLyricsWidth(w) {
  w = Math.round(Math.min(640, Math.max(260, w)));
  S.prefs.lyricsWidth = w;
  document.body.style.setProperty('--lyw', w + 'px');
}
$('#ly-resize').addEventListener('mousedown', e => {
  e.preventDefault();
  const right = $('#content').getBoundingClientRect().right;
  document.body.classList.add('resizing');
  const move = ev => setLyricsWidth(right - ev.clientX);
  const up = () => { document.body.classList.remove('resizing'); removeEventListener('mousemove', move); removeEventListener('mouseup', up); savePrefs(); scheduleRows(); };
  addEventListener('mousemove', move); addEventListener('mouseup', up);
});

/* ───────────── Row selection ───────────── */
const sel = { list: null, set: new Set(), anchor: -1 };
function selectRow(tr, e) {
  const list = tr.closest('.tl').dataset.list, pos = +tr.dataset.pos;
  if (sel.list !== list) { sel.list = list; sel.set.clear(); sel.anchor = -1; }
  if (e.shiftKey && sel.anchor >= 0) {
    sel.set.clear();
    for (let i = Math.min(sel.anchor, pos); i <= Math.max(sel.anchor, pos); i++) sel.set.add(i);
  } else if (e.ctrlKey) { sel.set.has(pos) ? sel.set.delete(pos) : sel.set.add(pos); sel.anchor = pos; }
  else { sel.set.clear(); sel.set.add(pos); sel.anchor = pos; }
  paintSel();
}
function paintSel() {
  $$('.tr.sel', view).forEach(r => r.classList.remove('sel'));
  const tl = sel.list && view.querySelector(`.tl[data-list="${sel.list}"]`);
  if (tl) $$('.tr', tl).forEach(r => r.classList.toggle('sel', sel.set.has(+r.dataset.pos)));
}
function selTracks() { const l = lists.get(sel.list) || []; return [...sel.set].sort((a, b) => a - b).map(p => l[p]).filter(x => x !== undefined); }

/* ───────────── Context menu ───────────── */
const menu = $('#menu');
let menuActs = [];
function menuHtml(items) {
  return items.map(it => {
    if (it === '-') return '<div class="msep"></div>';
    if (it.sub) return `<div class="mi">${esc(it.label)}${ic('right', 12)}<div class="mmenu sub">${menuHtml(it.sub)}</div></div>`;
    const id = menuActs.push(it.act) - 1;
    return `<div class="mi${it.disabled ? ' dis' : ''}" data-mi="${id}">${esc(it.label)}</div>`;
  }).join('');
}
function openMenu(x, y, items) {
  menuActs = [];
  menu.innerHTML = menuHtml(items);
  menu.style.display = 'block'; menu.style.left = '0'; menu.style.top = '0';
  const r = menu.getBoundingClientRect();
  menu.style.left = Math.min(x, innerWidth - r.width - 4) + 'px';
  menu.style.top = Math.min(y, innerHeight - r.height - 4) + 'px';
  menu.classList.toggle('flip', x + r.width * 2 > innerWidth);
}
const closeMenu = () => { menu.style.display = 'none'; };
menu.addEventListener('click', e => { const mi = e.target.closest('[data-mi]'); if (!mi) return; closeMenu(); menuActs[+mi.dataset.mi]?.(); });
addEventListener('mousedown', e => { if (!menu.contains(e.target)) closeMenu(); }, true);
addEventListener('blur', closeMenu);

function playlistSubmenu(idxs) {
  return [{ label: 'New playlist…', act: () => newPlaylist(idxs) }, ...(S.prefs.playlists.length ? ['-'] : []),
    ...S.prefs.playlists.map(p => ({ label: p.name, act: () => addToPlaylist(p.id, idxs) }))];
}
function trackMenu(x, y, idxs) {
  const t = S.tracks[idxs[0]], many = idxs.length > 1;
  const allFav = idxs.every(i => S.fav.has(S.tracks[i].path));
  const cur = many ? 0 : ratingOf(t.path);
  const items = [
    { label: 'Play', act: () => playList(idxs, 0) },
    { label: 'Add to queue', act: () => { P.upNext.push(...idxs); toast(many ? `${idxs.length} songs added to the queue` : 'Added to the queue'); refreshQueueView(); syncNext(); } },
    { label: allFav ? 'Remove from Your Favorites' : 'Add to Your Favorites', act: () => toggleFav(idxs) },
    '-',
    t._names.length > 1 && !many
      ? { label: 'Go to artist', sub: t._names.map(n => ({ label: n, act: () => go({ view: 'artist', arg: n }) })) }
      : { label: 'Go to artist', act: () => go({ view: 'artist', arg: artistOf(t) }), disabled: many },
    { label: 'Go to album', act: () => go({ view: 'album', arg: albumKeyOf(t) }), disabled: many },
    '-',
    { label: 'Add to playlist', sub: playlistSubmenu(idxs) },
    { label: 'Rate', sub: [5, 4, 3, 2, 1].map(n => ({ label: '★'.repeat(n) + '☆'.repeat(5 - n) + (n === cur ? '  ✓' : ''), act: () => setRating(idxs, n) })).concat(['-', { label: 'Clear rating', act: () => setRating(idxs, 0) }]) },
  ];
  if (route.view === 'playlist') items.push({ label: 'Remove from this playlist', act: () => removeFromPlaylist(route.arg, idxs) });
  if (route.view === 'queue' && sel.list === 'q-next') items.push({ label: 'Remove from queue', act: () => { const rm = new Set([...sel.set]); P.upNext = P.upNext.filter((_, i) => !rm.has(i)); rerender(); } });
  items.push('-',
    { label: many ? `Edit info (${idxs.length})…` : 'Edit info…', act: () => editTags(idxs) },
    { label: 'Organize files…', act: () => organize(idxs) },
    { label: 'Download lyrics…', act: () => batchLyrics(idxs) },
    { label: 'Show credits', act: () => showCredits(t), disabled: many },
    { label: 'Tags from file name…', act: () => tagsFromFilename(idxs) },
    { label: 'Fix capitalization…', act: () => fixCapitalization(idxs) },
    { label: 'Copy to folder…', act: () => copyToFolder(idxs, many ? 'Songs' : t.title) },
    { label: 'Show in Explorer', act: () => invoke('reveal', { path: realPath(t) }), disabled: many },
    { label: 'Copy file path', act: () => navigator.clipboard.writeText(idxs.map(i => realPath(S.tracks[i])).join('\n')).then(() => toast('Path copied')) });
  openMenu(x, y, items);
}
function albumMenu(x, y, key) {
  const a = S.albumMap.get(key); if (!a) return;
  openMenu(x, y, [
    { label: 'Play', act: () => playAlbum(key) },
    { label: 'Add to queue', act: () => { P.upNext.push(...a.tracks); toast('Album added to the queue'); } },
    '-',
    { label: 'Go to artist', act: () => go({ view: 'artist', arg: a.owner || a.artist }) },
    { label: 'Add to playlist', sub: playlistSubmenu(a.tracks) },
    { label: 'Add to Your Favorites', act: () => toggleFav(a.tracks) },
    '-',
    { label: 'Edit album info…', act: () => editTags(a.tracks) },
    { label: 'Fill in from MusicBrainz…', act: () => musicBrainz(a.tracks) },
    { label: 'Organize files…', act: () => organize(a.tracks) },
    { label: 'Download lyrics…', act: () => batchLyrics(a.tracks) },
    { label: 'Change cover…', act: () => imagePicker({ kind: 'album', key }) },
    { label: 'Embed cover in the files', act: () => embedCovers([a], true) },
    { label: 'Fix capitalization…', act: () => fixCapitalization(a.tracks) },
    { label: 'Copy to folder…', act: () => copyToFolder(a.tracks, a.title) },
    { label: 'Show in Explorer', act: () => invoke('reveal', { path: realPath(S.tracks[a.tracks[0]]) }) },
  ]);
}
function playlistMenu(x, y, id) {
  const pl = S.prefs.playlists.find(p => p.id === id); if (!pl) return;
  const idxs = () => pl.tracks.map(p => S.byPath.get(p)).filter(i => i !== undefined);
  openMenu(x, y, [
    { label: 'Play', act: () => playList(idxs(), 0, { view: 'playlist', arg: id }) },
    { label: 'Add to queue', act: () => { P.upNext.push(...idxs()); toast('Playlist added to the queue'); } },
    '-',
    { label: 'Edit details…', act: () => editPlaylistDetails(id) },
    { label: 'Rename', act: () => renamePlaylist(id) },
    { label: 'Move to folder', sub: [
      ...(S.prefs.plFolders || []).map(f => ({ label: (pl.folder === f.id ? '✓ ' : '') + f.name, act: () => movePlaylist(id, f.id) })),
      ...(pl.folder ? [{ label: 'Out of the folder', act: () => movePlaylist(id, null) }] : []),
      ...((S.prefs.plFolders || []).length ? ['-'] : []),
      { label: 'New folder…', act: () => newPlaylistFolder(id) }] },
    { label: 'Export as M3U…', act: () => exportM3U(id) },
    { label: 'Copy to folder…', act: () => copyToFolder(idxs(), pl.name) },
    { label: 'Delete', act: () => deletePlaylist(id) },
  ]);
}

/* ───────────── Modals ───────────── */
const modalBg = $('#modal-bg');
function modal({ title, text, value, ok = 'Create', input = true, label = 'Name', placeholder = 'New playlist' }) {
  return new Promise(res => {
    modalBg.innerHTML = `<button class="ib close" data-m="cancel">${ic('x', 26)}</button><div class="modal"><h2>${esc(title)}</h2>${text ? `<p>${esc(text)}</p>` : ''}
      ${input ? `<div class="field"><label>${esc(label)}</label><input id="m-in" value="${esc(value || '')}" placeholder="${esc(placeholder)}" spellcheck="false"></div>` : ''}
      <div class="btns"><button class="btn btn-ghost" data-m="cancel">Cancel</button><button class="btn btn-green" data-m="ok">${esc(ok)}</button></div></div>`;
    modalBg.classList.add('open');
    const inp = $('#m-in', modalBg);
    inp?.focus(); inp?.select();
    const done = v => { modalBg.classList.remove('open'); modalBg.onclick = null; modalBg.onkeydown = null; res(v); };
    modalBg.onclick = e => { const m = e.target.closest('[data-m]')?.dataset.m; if (m === 'ok') done(input ? inp.value.trim() || null : true); else if (m === 'cancel' || e.target === modalBg) done(null); };
    modalBg.onkeydown = e => { if (e.key === 'Enter') done(input ? inp.value.trim() || null : true); if (e.key === 'Escape') done(null); };
    if (!input) $('[data-m="ok"]', modalBg).focus();
  });
}

/* ───────────── Playlists ───────────── */
function renderSidebar() {
  const folders = S.prefs.plFolders || [], ids = new Set(folders.map(f => f.id));
  const item = p => `<a class="sb-item" data-go="playlist" data-arg="${esc(p.id)}" data-pl="${esc(p.id)}" draggable="true"><span class="sbi">${ic('playlist', 16)}</span>${esc(p.name)}</a>`;
  $('#sb-playlists').innerHTML = folders.map(f => `<div class="sb-folder${f.open === false ? ' closed' : ''}" data-plf="${esc(f.id)}">
      <a class="sb-item sb-f" data-plf-toggle="${esc(f.id)}" draggable="true"><span class="car">${ic('down', 10)}</span>${ic('folder', 14)}<span class="nm">${esc(f.name)}</span></a>
      <div class="sb-fkids">${S.prefs.playlists.filter(p => p.folder === f.id).map(item).join('')}</div></div>`).join('')
    + S.prefs.playlists.filter(p => !ids.has(p.folder)).map(item).join('');
  markSidebar();
}
async function newPlaylist(idxs = []) {
  const name = await modal({ title: 'Create playlist', value: `My playlist #${S.prefs.playlists.length + 1}` });
  if (!name) return;
  const pl = { id: Date.now().toString(36), name, tracks: idxs.map(i => S.tracks[i].path), created: Date.now() };
  S.prefs.playlists.push(pl); savePrefs(); renderSidebar();
  if (idxs.length) toast(`Added to ${name}`); else go({ view: 'playlist', arg: pl.id });
}
function addToPlaylist(id, idxs) {
  const pl = S.prefs.playlists.find(p => p.id === id); if (!pl) return;
  pl.tracks.push(...idxs.map(i => S.tracks[i].path)); savePrefs();
  toast(idxs.length > 1 ? `${idxs.length} songs added to ${pl.name}` : `Added to ${pl.name}`);
  if (route.view === 'playlist' && route.arg === id) rerender();
}
function removeFromPlaylist(id, idxs) {
  const pl = S.prefs.playlists.find(p => p.id === id); if (!pl) return;
  const rm = new Set(idxs.map(i => S.tracks[i].path));
  pl.tracks = pl.tracks.filter(p => !rm.has(p)); savePrefs(); rerender();
}
async function renamePlaylist(id) {
  const pl = S.prefs.playlists.find(p => p.id === id); if (!pl) return;
  const name = await modal({ title: 'Rename playlist', value: pl.name, ok: 'Save' });
  if (!name) return;
  pl.name = name; savePrefs(); renderSidebar(); if (route.arg === id) rerender();
}
async function deletePlaylist(id) {
  const pl = S.prefs.playlists.find(p => p.id === id); if (!pl) return;
  if (!await modal({ title: 'Delete playlist?', text: `"${pl.name}" will be deleted for good.`, ok: 'Delete', input: false })) return;
  S.prefs.playlists = S.prefs.playlists.filter(p => p.id !== id); savePrefs(); renderSidebar();
  if (route.view === 'playlist' && route.arg === id) go({ view: 'browse' }, true);
}

/* ───────────── Library ───────────── */
async function addFolder() {
  const picked = await invoke('pick_folders');
  const fresh = picked.filter(p => !S.prefs.folders.some(f => f.toLowerCase() === p.toLowerCase()));
  if (!fresh.length) return;
  S.prefs.folders.push(...fresh); savePrefs();
  await rescan();
}
async function rescan({ quiet = false, moved = null } = {}) {
  if (S.scanning) return;
  S.scanning = true; if (route.view === 'local') rerender();
  if (!quiet) toast('Scanning your music…', 60000);
  const before = S.tracks.length;
  try {
    const fix = p => (moved && moved.get(p)) || p;
    const curPath = fix(S.tracks[P.current]?.path);
    const toPaths = a => a.map(i => fix(S.tracks[i]?.path));
    const ctx = toPaths(P.ctx), up = toPaths(P.upNext);
    S.tracks = await invoke('scan_folders', { folders: S.prefs.folders });
    buildIndex(); coverP.clear();
    const back = a => a.map(p => S.byPath.get(p)).filter(i => i !== undefined);
    P.ctx = back(ctx); P.upNext = back(up);
    P.current = curPath ? (S.byPath.get(curPath) ?? -1) : -1;
    if (P.order.length !== P.ctx.length) { P.order = [...P.ctx.keys()]; P.pos = Math.max(0, P.ctx.indexOf(P.current)); }
    const diff = S.tracks.length - before;
    if (!quiet) toast(`Library updated: ${plural(S.tracks.length, 'song', 'songs')}`);
    else if (diff > 0) toast(`New music: ${plural(diff, 'song added', 'songs added')}`);
  } catch (e) { toast('Scan error: ' + e); }
  S.scanning = false;
  updateNowPlaying(); syncNext();
  if (!['settings', 'eq'].includes(route.view)) rerender();
}

/* ───────────── Toast ───────────── */
let toastT = 0;
function toast(msg, ms = 2600) {
  const t = $('#toast'); t.textContent = msg; t.classList.add('show');
  clearTimeout(toastT); toastT = setTimeout(() => t.classList.remove('show'), ms);
}

/* ───────────── Global events ───────────── */
document.addEventListener('click', e => {
  const t = e.target;
  const pa = t.closest('[data-play-album]');
  if (pa) { e.stopPropagation(); const k = pa.dataset.playAlbum; if (P.current >= 0 && albumKeyOf(S.tracks[P.current]) === k) togglePlay(); else playAlbum(k); return; }
  const pr = t.closest('[data-play-artist]');
  if (pr) { e.stopPropagation(); playArtistRadio(pr.dataset.playArtist); return; }
  const pl = t.closest('[data-play]');
  if (pl) { playList(lists.get(pl.dataset.play), -1); return; }
  const ma = t.closest('[data-menu-album]');
  if (ma) { const r = ma.getBoundingClientRect(); albumMenu(r.left, r.bottom + 4, ma.dataset.menuAlbum); return; }
  const mp = t.closest('[data-menu-playlist]');
  if (mp) { const r = mp.getBoundingClientRect(); playlistMenu(r.left, r.bottom + 4, mp.dataset.menuPlaylist); return; }
  const srt = t.closest('[data-sort]');
  if (srt) { const k = srt.dataset.sort; sortState.dir = sortState.key === k ? -sortState.dir : 1; sortState.key = k; rerender(); return; }
  const act = t.closest('[data-action]');
  if (act) { runAction(act.dataset.action, act.dataset.arg); return; }
  const g = t.closest('[data-go]');
  if (g && g.dataset.go) {
    const r = { view: g.dataset.go, arg: g.dataset.arg };
    if (r.view !== route.view || r.arg !== route.arg) go(r);
    return;
  }
});

function runAction(a, arg) {
  if (a === 'add-folder') addFolder();
  else if (a === 'rescan') rescan();
  else if (a === 'remove-folder') {
    S.prefs.folders = S.prefs.folders.filter(f => f !== arg); savePrefs(); rescan();
  } else if (a === 'reveal-folder') invoke('reveal', { path: arg });
  else if (a === 'shuffle-all') { const was = S.prefs.shuffle; S.prefs.shuffle = true; playList(S.tracks.map((_, i) => i), -1, { view: 'songs' }); S.prefs.shuffle = was; }
  else if (a === 'clear-queue') { P.upNext = []; rerender(); }
  else if (a === 'lyrics-save' && P.current >= 0 && L.raw) saveLyrics(S.tracks[P.current], L.raw, [arg]);
  else if (a === 'lyrics-retry' && P.current >= 0) {
    refreshLyricsPanel(true);
  }
  else if (a === 'genre-radio') { const g = S.genreMap.get(arg); if (g) { const was = S.prefs.shuffle; S.prefs.shuffle = true; playList(g.tracks, -1, { view: 'genre', arg }); S.prefs.shuffle = was; } }
  else settingsAction(a, arg);
}

view.addEventListener('mousedown', e => {
  const tr = e.target.closest('.tr');
  if (tr && e.button === 0 && !e.target.closest('.lnk')) selectRow(tr, e);
});
view.addEventListener('dblclick', e => {
  const tr = e.target.closest('.tr'); if (!tr || e.target.closest('.lnk')) return;
  const list = tr.closest('.tl').dataset.list, pos = +tr.dataset.pos;
  if (list === 'q-next') { P.upNext.splice(0, pos); next(); return; }
  if (list === 'q-ctx') { P.pos += pos; next(); return; }
  playList(lists.get(list), pos);
});
view.addEventListener('click', e => {
  const pi = e.target.closest('.tr .pi'); if (!pi) return;
  const tr = pi.closest('.tr'); playList(lists.get(tr.closest('.tl').dataset.list), +tr.dataset.pos);
});
view.addEventListener('contextmenu', e => {
  e.preventDefault();
  const tr = e.target.closest('.tr');
  if (tr) {
    const list = tr.closest('.tl').dataset.list;
    if (sel.list !== list || !sel.set.has(+tr.dataset.pos)) selectRow(tr, {});
    trackMenu(e.clientX, e.clientY, selTracks()); return;
  }
  const card = e.target.closest('[data-card-album]');
  if (card) albumMenu(e.clientX, e.clientY, card.dataset.cardAlbum);
});
view.addEventListener('input', e => {
  const f = e.target.dataset.filter; if (!f || !filters.has(f)) return;
  sel.list = null; sel.set.clear();
  $('#' + f).innerHTML = filters.get(f)(e.target.value);
  hydrate($('#' + f)); markPlaying();
});
view.addEventListener('scroll', () => { $('#topbar').classList.toggle('scrolled', view.scrollTop > 4); scheduleRows(); }, { passive: true });
addEventListener('resize', scheduleRows);

$('#sidebar').addEventListener('contextmenu', e => {
  e.preventDefault();
  const p = e.target.closest('[data-pl]'); if (p) { playlistMenu(e.clientX, e.clientY, p.dataset.pl); return; }
  const f = e.target.closest('[data-plf-toggle]'); if (f) { folderMenu(e.clientX, e.clientY, f.dataset.plfToggle); return; }
  if (e.target.closest('#sb-pl-h, #new-playlist')) openMenu(e.clientX, e.clientY, [
    { label: 'New playlist', act: () => newPlaylist() }, { label: 'New folder', act: () => newPlaylistFolder() }]);
});
document.addEventListener('contextmenu', e => { if (!e.target.closest('input')) e.preventDefault(); });

/* Drag songs onto a playlist in the sidebar */
let dragIdxs = [];
view.addEventListener('dragstart', e => {
  const tr = e.target.closest('.tr'); if (!tr) { e.preventDefault(); return; }
  if (sel.list !== tr.closest('.tl').dataset.list || !sel.set.has(+tr.dataset.pos)) selectRow(tr, {});
  dragIdxs = selTracks();
  const g = $('#drag-ghost');
  g.textContent = dragIdxs.length > 1 ? `${dragIdxs.length} songs` : `${S.tracks[dragIdxs[0]].title} — ${S.tracks[dragIdxs[0]].artist}`;
  e.dataTransfer.setData('text/plain', 'wax');
  e.dataTransfer.effectAllowed = 'copy';
  e.dataTransfer.setDragImage(g, -12, -8);
});
const sbPl = $('#sb-playlists');
sbPl.addEventListener('dragover', e => { const p = e.target.closest('[data-pl]'); if (!p || !dragIdxs.length) return; e.preventDefault(); $$('.drop', sbPl).forEach(x => x !== p && x.classList.remove('drop')); p.classList.add('drop'); });
sbPl.addEventListener('dragleave', e => e.target.closest('[data-pl]')?.classList.remove('drop'));
sbPl.addEventListener('drop', e => { const p = e.target.closest('[data-pl]'); if (!p) return; e.preventDefault(); p.classList.remove('drop'); addToPlaylist(p.dataset.pl, dragIdxs); dragIdxs = []; });
view.addEventListener('dragend', () => { dragIdxs = []; $$('.drop', sbPl).forEach(x => x.classList.remove('drop')); });

/* Search */
let searchT = 0;
$('#search').addEventListener('input', e => {
  clearTimeout(searchT);
  const q = e.target.value;
  searchT = setTimeout(() => {
    if (!q.trim()) { if (route.view === 'search') back(); return; }
    const inp = $('#search'), pos = inp.selectionStart;
    go({ view: 'search', arg: q }, route.view === 'search');
    inp.value = q; inp.setSelectionRange(pos, pos);
  }, 220);
});
$('#search').addEventListener('keydown', e => { if (e.key === 'Escape') { e.target.value = ''; e.target.blur(); if (route.view === 'search') back(); } });

/* Keyboard */
addEventListener('keydown', e => {
  if (e.target.matches('input')) return;
  if (modalBg.classList.contains('open')) return;
  if (e.code === 'Space') { e.preventDefault(); togglePlay(); }
  else if (e.ctrlKey && e.key === 'ArrowRight') next();
  else if (e.ctrlKey && e.key === 'ArrowLeft') prev();
  else if (e.ctrlKey && e.key === 'ArrowUp') { e.preventDefault(); setVolume(S.prefs.volume + .1); }
  else if (e.ctrlKey && e.key === 'ArrowDown') { e.preventDefault(); setVolume(S.prefs.volume - .1); }
  else if (e.ctrlKey && (e.key === 'f' || e.key === 'l')) { e.preventDefault(); ($('[data-filter]', view) || $('#search')).focus(); }
  else if (e.ctrlKey && e.key === 'a' && sel.list) { e.preventDefault(); sel.set = new Set((lists.get(sel.list) || []).map((_, i) => i)); paintSel(); }
  else if (e.key === 'Enter' && sel.list && sel.set.size) playList(lists.get(sel.list), Math.min(...sel.set));
  else if (e.altKey && e.key === 'ArrowLeft') back();
  else if (e.altKey && e.key === 'ArrowRight') fwd();
  else if (e.key === 'BrowserBack') back();
});
addEventListener('mouseup', e => { if (e.button === 3) back(); if (e.button === 4) fwd(); });

/* Window */
$('#nav-back').onclick = back;
$('#nav-fwd').onclick = fwd;
$('#w-min').onclick = () => appWin.minimize();
$('#w-max').onclick = () => appWin.toggleMaximize();
appWin.onResized?.(async () => setIcon($('#w-max'), await appWin.isMaximized() ? 'restore' : 'max'));
$('#new-playlist').onclick = () => newPlaylist();
$('#app-menu').onclick = e => {
  const r = e.currentTarget.getBoundingClientRect();
  openMenu(r.left, r.bottom + 4, [
    { label: 'Add music folder…', act: addFolder },
    { label: 'Refresh library', act: () => rescan(), disabled: !S.prefs.folders.length },
    { label: 'Import M3U playlist…', act: importM3U },
    { label: 'New playlist', act: () => newPlaylist() },
    { label: 'New playlist folder', act: () => newPlaylistFolder() },
    '-',
    { label: 'Library check', act: () => go({ view: 'health' }) },
    { label: 'Find duplicates', act: () => go({ view: 'duplicates' }) },
    { label: 'Import Spotify playlists…', act: () => importSpotify() },
    ...(S.prefs.lastOrganize?.moves?.length ? [{ label: `Undo last organize (${S.prefs.lastOrganize.moves.length} files)`, act: () => undoOrganize() }] : []),
    { label: 'Sleep timer', sub: sleepItems() },
    '-',
    { label: 'Equalizer', act: () => go({ view: 'eq' }) },
    { label: 'Mini player', act: () => invoke('toggle_mini', { show: true }) },
    { label: 'Now playing (full screen)', act: () => npFull(true), disabled: P.current < 0 },
    { label: S.prefs.bigCover ? 'Shrink cover art' : 'Enlarge cover art', act: () => setBigCover(!S.prefs.bigCover) },
    { label: 'Settings', act: () => go({ view: 'settings' }) },
    '-',
    { label: 'Exit', act: () => invoke('show_main_window').then(() => appWin.close()) },
  ]);
};

/* ───────────── Favorites ───────────── */
S.fav = new Set();
const isFav = p => S.fav.has(p);
function syncFavButton() {
  const t = S.tracks[P.current], on = !!t && isFav(t.path);
  const b = $('#b-fav');
  setIcon(b, on ? 'heartFill' : 'heart');
  b.classList.toggle('on', on);
  b.title = on ? 'Remove from Your Favorites' : 'Add to Your Favorites';
}
function toggleFav(idxs) {
  const paths = idxs.map(i => S.tracks[i]?.path).filter(Boolean);
  if (!paths.length) return;
  const all = paths.every(isFav);
  for (const p of paths) all ? S.fav.delete(p) : S.fav.add(p);
  S.prefs.favorites = [...S.fav];
  lfLoveSync(paths, !all);
  savePrefs(); syncFavButton(); miniState();
  toast(all ? 'Removed from Your Favorites' : (paths.length > 1 ? `${paths.length} songs added to Your Favorites` : 'Added to Your Favorites'));
  if (route.view === 'favorites') rerender(); else paintRows(true);
}

/* ───────────── Artist photos ───────────── */
const artistP = new Map();
function artistImage(name, big = false) {
  const k = name + (big ? '#lg' : '#sm');
  if (!artistP.has(k)) {
    artistP.set(k, invoke('get_artist_image', { name, dir: S.artistDir.get(name) || null, size: big ? 640 : 300, online: S.prefs.fetchArtistImages !== false, custom: S.prefs.artistOverrides?.[name] || null })
      .then(c => c ? { url: fileUrl(c.path), path: c.path, color: c.color } : null).catch(() => null));
  }
  return artistP.get(k);
}

/* ───────────── Genres ───────────── */
const GENRE_COLORS = ['#e13300', '#1e3264', '#8d67ab', '#148a08', '#e8115b', '#509bf5', '#ba5d07', '#477d95', '#af2896', '#27856a', '#dc148c', '#503750', '#7358ff', '#e91429', '#006450', '#8c1932'];
function genreColor(name) { let h = 0; for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0; return GENRE_COLORS[h % GENRE_COLORS.length]; }
function genreTile(g) {
  const key = [...g.albums][0];
  return `<div class="gtile" data-go="genre" data-arg="${esc(g.key)}" style="background:${genreColor(g.name)}">
    <div class="gname">${esc(g.name)}</div>${cvHtml(key, 'gcv', 30)}</div>`;
}
Object.assign(VIEWS, {
  genres() {
    view.classList.add('gradient');
    if (!S.genres.length) return `<div class="page-h"><h1>Genres</h1></div><div class="empty"><h2>No genres</h2><p>Your files don't have a genre in their tags. You can add it with "Edit info" or from MusicBrainz.</p></div>`;
    return `<div class="page-h"><h1>Genres &amp; Moods</h1>
      <div class="tabs"><span class="tab" data-go="browse">Overview</span><span class="tab on">Genres</span><span class="tab" data-go="albums">Albums</span><span class="tab" data-go="artists">Artists</span></div></div>
      <div class="ggrid">${S.genres.map(genreTile).join('')}</div>`;
  },
  genre(key) {
    const g = S.genreMap.get(key); if (!g) return notFound();
    const albums = [...g.albums].map(k => S.albumMap.get(k)).filter(Boolean).sort((a, b) => (b.year - a.year) || cmp(a.title, b.title));
    const idxs = sortIdx(g.tracks, 'artist', 1);
    lists.set('genre-all', idxs);
    return `<div class="hero genre-hero" style="--gc:${genreColor(g.name)}"><div class="info"><div class="label">Genre</div><h1>${esc(g.name)}</h1>
      <div class="meta">${plural(albums.length, 'album', 'albums')} • ${plural(g.tracks.length, 'song', 'songs')}</div>
      <div class="btns">${playBtn('genre-all')}<button class="btn btn-ghost" data-action="genre-radio" data-arg="${esc(key)}">${ic('shuffle', 14)} Radio</button></div></div></div>
      ${secHead('Albums')}${gridHtml('genre-albums', albums, a => albumCard(a))}
      ${secHead('Songs')}${trackTable('genre-songs', idxs, 'songs')}`;
  },

  favorites() {
    const idxs = S.prefs.favorites.map(p => S.byPath.get(p)).filter(i => i !== undefined).reverse();
    filters.set('w-fav', q => trackTable('favorites', filterIdx(idxs, q), 'songs'));
    return `<div class="page-h"><h1>Your Favorites</h1><div class="row">${idxs.length ? playBtn('favorites') : ''}
      <span class="sub">${plural(idxs.length, 'song', 'songs')}</span><span class="grow"></span>${idxs.length ? filterBox('w-fav') : ''}</div></div>
      ${idxs.length ? `<div id="w-fav">${filters.get('w-fav')('')}</div>`
        : `<div class="empty"><div class="big-ic">${ic('heart', 80)}</div><h2>No favorites yet</h2><p>Tap the ♥ in the player bar or right-click → "Add to Your Favorites".</p></div>`}`;
  },
});

/* ───────────── Stats ───────────── */
const Stats = { data: null, dirty: true };
const PERIODS = [['4w', 'Last 4 weeks', 28], ['6m', 'Last 6 months', 182], ['1y', 'Last year', 365], ['all', 'All time', 0]];
VIEWS.stats = (period = '4w') => {
  setTimeout(() => renderStats(period));
  return `<div class="page-h"><div class="row"><h1>Your Stats</h1><span class="grow"></span><button class="btn btn-green" data-go="wrapped">${new Date().getFullYear()} Wrapped</button></div>
    <div class="tabs">${PERIODS.map(([k, l]) => `<span class="tab ${k === period ? 'on' : ''}" data-go="stats" data-arg="${k}">${l}</span>`).join('')}</div></div>
    <div id="stats-body"><div class="ly-status"><span class="spin"></span></div></div>`;
};
async function renderStats(period) {
  if (!Stats.data || Stats.dirty) { Stats.data = await invoke('load_history').catch(() => []); Stats.dirty = false; }
  const body = $('#stats-body'); if (!body || route.view !== 'stats') return;
  const days = PERIODS.find(p => p[0] === period)?.[2] || 0;
  const since = days ? Date.now() / 1000 - days * 86400 : 0;
  const list = Stats.data.filter(l => l.t >= since && S.byPath.has(l.p));
  if (!list.length) {
    body.innerHTML = `<div class="empty"><div class="big-ic">${ic('chart', 80)}</div><h2>No data yet</h2><p>A play counts after 30 seconds and half of the song (or 4 minutes).</p></div>`;
    return;
  }
  const byTrack = new Map(), byArtist = new Map(), byAlbum = new Map(), hours = Array(24).fill(0);
  let secs = 0;
  for (const l of list) {
    const i = S.byPath.get(l.p), t = S.tracks[i];
    secs += l.s;
    byTrack.set(i, (byTrack.get(i) || 0) + 1);
    byArtist.set(t._owner, (byArtist.get(t._owner) || 0) + 1);
    const k = albumKeyOf(t); byAlbum.set(k, (byAlbum.get(k) || 0) + 1);
    hours[new Date(l.t * 1000).getHours()]++;
  }
  const top = (m, n) => [...m].sort((a, b) => b[1] - a[1]).slice(0, n);
  const topArtists = top(byArtist, 10).map(([n, c]) => ({ ar: S.artistMap.get(n), c })).filter(x => x.ar);
  const topTracks = top(byTrack, 20);
  const topAlbums = top(byAlbum, 10).map(([k, c]) => ({ a: S.albumMap.get(k), c })).filter(x => x.a);
  lists.set('stats-tracks', topTracks.map(x => x[0]));
  const maxH = Math.max(...hours, 1);
  const h = Math.floor(secs / 3600), m = Math.round((secs % 3600) / 60);
  body.innerHTML = `
    <div class="stat-row">
      <div class="stat"><div class="n">${h ? `${h} hr ${m} min` : `${m} min`}</div><div class="l">listening time</div></div>
      <div class="stat"><div class="n">${list.length}</div><div class="l">plays</div></div>
      <div class="stat"><div class="n">${byArtist.size}</div><div class="l">artists</div></div>
      <div class="stat"><div class="n">${byTrack.size}</div><div class="l">different songs</div></div>
    </div>
    ${secHead('Your top artists')}<div class="grid">${topArtists.map(({ ar, c }, n) => artistCard(ar, null, `${n + 1}. ${plural(c, 'play', 'plays')}`)).join('')}</div>
    ${secHead('Your top songs')}<div class="tl-plays">${trackTable('stats-tracks', topTracks.map(x => x[0]), 'short')}</div>
    ${secHead('Your top albums')}<div class="grid">${topAlbums.map(({ a }) => albumCard(a)).join('')}</div>
    ${secHead('When do you listen?')}
    <div class="hours">${hours.map((v, i) => `<div class="hb" title="${i}:00 · ${v}"><div class="bar" style="height:${Math.round(v / maxH * 100)}%"></div><span>${i % 3 === 0 ? i : ''}</span></div>`).join('')}</div>`;
  hydrate(body); markPlaying();
}

/* ───────────── Equalizer ───────────── */
const EQ_LABELS = ['31', '62', '125', '250', '500', '1k', '2k', '4k', '8k', '16k'];
const EQ_PRESETS = {
  'Flat': [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
  'Bass boost': [6, 5, 4, 2, 0, 0, 0, 0, 0, 0],
  'Treble boost': [0, 0, 0, 0, 0, 1, 2, 4, 5, 6],
  'Rock': [5, 3, -1, -2, -1, 1, 3, 4, 5, 5],
  'Metal': [4, 3, 0, -2, -1, 1, 3, 4, 4, 3],
  'Pop': [-1, 1, 3, 4, 3, 0, -1, -1, 0, 1],
  'Electronic': [5, 4, 1, 0, -2, 1, 0, 1, 4, 5],
  'Hip hop': [5, 4, 1, 2, -1, -1, 1, 0, 2, 3],
  'Acoustic': [3, 2, 1, 1, 2, 2, 3, 3, 2, 1],
  'Vocals': [-2, -1, 0, 2, 4, 4, 3, 1, 0, -1],
  'Jazz': [3, 2, 1, 2, -1, -1, 0, 1, 2, 3],
  'Classical': [4, 3, 2, 1, -1, -1, 0, 2, 3, 4],
  'Low volume (loudness)': [6, 4, 0, 0, -2, 0, -1, -3, 4, 2],
};
const eqState = () => S.prefs.eq || (S.prefs.eq = { enabled: false, preset: 'Flat', preamp: 0, bands: [...EQ_PRESETS['Flat']] });
let eqTimer = 0;
function applyEq() {
  clearTimeout(eqTimer);
  eqTimer = setTimeout(() => {
    const e = eqState(), ae = S.prefs.autoeq?.on ? S.prefs.autoeq : null;
    audio.setEq({ enabled: e.enabled, preamp: e.preamp, bands: e.bands, filters: ae ? ae.filters : [], filterPreamp: ae ? ae.preamp : 0 });
    savePrefs();
  }, 40);
  $('#b-eq').classList.toggle('on', eqState().enabled || !!S.prefs.autoeq?.on);
}
VIEWS.eq = () => {
  const e = eqState();
  return `<div class="page-h"><h1>Equalizer</h1><div class="row">
      <label class="switch"><input type="checkbox" id="eq-on" ${e.enabled ? 'checked' : ''}><span></span></label><span>${e.enabled ? 'On' : 'Off'}</span>
      <span class="grow"></span>
      <select class="sel" id="eq-preset">${[...Object.keys(EQ_PRESETS), ...(EQ_PRESETS[e.preset] ? [] : ['Custom'])].map(p => `<option ${p === e.preset ? 'selected' : ''}>${esc(p)}</option>`).join('')}</select>
      <button class="btn btn-ghost" id="eq-reset">Reset</button></div></div>
    ${audio.rust ? '' : `<p class="sub warn">The audio engine isn't available: the equalizer won't be applied.</p>`}
    <div class="eq ${e.enabled ? '' : 'off'}">
      <div class="eqb pre"><span class="val" id="eq-pre-v">${fmtDb(e.preamp)}</span><input type="range" min="-12" max="12" step="0.5" value="${e.preamp}" id="eq-pre" orient="vertical"><span class="lab">Pre</span></div>
      <div class="eq-sep"></div>
      ${e.bands.map((g, i) => `<div class="eqb"><span class="val">${fmtDb(g)}</span><input type="range" min="-12" max="12" step="0.5" value="${g}" data-band="${i}"><span class="lab">${EQ_LABELS[i]}</span></div>`).join('')}
    </div>
    <p class="sub">Changes apply instantly. "Pre" lowers or raises everything before equalizing (useful to avoid clipping when boosting bands).</p>`;
};
const fmtDb = v => `${v > 0 ? '+' : ''}${(+v).toFixed(1).replace('.0', '')} dB`;
view.addEventListener('input', e => {
  if (route.view !== 'eq') return;
  const st = eqState();
  if (e.target.dataset.band != null) {
    st.bands[+e.target.dataset.band] = +e.target.value;
    e.target.parentElement.querySelector('.val').textContent = fmtDb(+e.target.value);
    const match = Object.entries(EQ_PRESETS).find(([, b]) => b.every((x, i) => x === st.bands[i]));
    st.preset = match ? match[0] : 'Custom';
    const sel = $('#eq-preset');
    if (![...sel.options].some(o => o.value === st.preset)) sel.insertAdjacentHTML('beforeend', '<option>Custom</option>');
    sel.value = st.preset;
    applyEq();
  } else if (e.target.id === 'eq-pre') {
    st.preamp = +e.target.value; $('#eq-pre-v').textContent = fmtDb(st.preamp); applyEq();
  }
});
view.addEventListener('change', e => {
  if (route.view !== 'eq') return;
  const st = eqState();
  if (e.target.id === 'eq-on') { st.enabled = e.target.checked; applyEq(); rerender(); }
  else if (e.target.id === 'eq-preset' && EQ_PRESETS[e.target.value]) { st.preset = e.target.value; st.bands = [...EQ_PRESETS[st.preset]]; if (!st.enabled) st.enabled = true; applyEq(); rerender(); }
});
view.addEventListener('click', e => {
  if (e.target.id === 'eq-reset') { const st = eqState(); st.bands = [...EQ_PRESETS['Flat']]; st.preamp = 0; st.preset = 'Flat'; applyEq(); rerender(); }
});

/* ───────────── Settings ───────────── */
const SHORTCUT_ACTIONS = [
  ['toggle', 'Play / pause'], ['next', 'Next'], ['prev', 'Previous'], ['volup', 'Volume up'],
  ['voldown', 'Volume down'], ['like', 'Favorite (♥)'], ['show', 'Show Wax'], ['mini', 'Mini player'],
];
const DEFAULT_SHORTCUTS = { toggle: 'Ctrl+Alt+KeyP', next: 'Ctrl+Alt+ArrowRight', prev: 'Ctrl+Alt+ArrowLeft', volup: 'Ctrl+Alt+ArrowUp', voldown: 'Ctrl+Alt+ArrowDown', like: 'Ctrl+Alt+KeyL', show: 'Ctrl+Alt+KeyW', mini: 'Ctrl+Alt+KeyM' };
const accelLabel = a => (a || '').replace(/Key([A-Z])/g, '$1').replace(/Digit(\d)/g, '$1').replace('ArrowRight', '→').replace('ArrowLeft', '←').replace('ArrowUp', '↑').replace('ArrowDown', '↓').replace(/\+/g, ' + ');
async function registerShortcuts() {
  const failed = await invoke('set_shortcuts', { map: S.prefs.shortcuts || {} }).catch(() => []);
  if (failed.length) toast(`Couldn't use ${failed.map(accelLabel).join(', ')} (another app is using it)`, 4000);
}

const setRow = (title, desc, control) => `<div class="set-row"><div class="set-t"><div class="t">${title}</div>${desc ? `<div class="d">${desc}</div>` : ''}</div><div class="set-c">${control}</div></div>`;
const sw = (key, def = false) => `<label class="switch"><input type="checkbox" data-pref="${key}" ${(S.prefs[key] ?? def) ? 'checked' : ''}><span></span></label>`;
const selectPref = (key, opts, def) => `<select class="sel" data-pref="${key}">${opts.map(([v, l]) => `<option value="${v}" ${(S.prefs[key] ?? def) == v ? 'selected' : ''}>${l}</option>`).join('')}</select>`;

VIEWS.settings = () => {
  const p = S.prefs, analyzed = S.tracks.filter(t => S.loud[t.path]).length, withRg = S.tracks.filter(t => t.rg).length;
  const sc = p.shortcuts || {};
  return `<div class="page-h"><h1>Settings</h1></div>
  <div class="settings">
    <h3>Appearance</h3>
    ${setRow('Accent color', 'Buttons, progress bars and highlights.', accentSwatches())}

    <h3>Library</h3>
    ${setRow('Music folders', `${plural(p.folders.length, 'folder', 'folders')} · ${plural(S.tracks.length, 'song', 'songs')}`, `<button class="btn btn-ghost btn-sm" data-go="local">Manage</button>`)}
    ${setRow('Detect new music automatically', 'Adds whatever you copy, move or delete in your folders to the library, without refreshing by hand.', sw('watchFolders', true))}
    ${setRow('Organize files', `Pattern used to rename and move: <code>${esc(p.organizePattern || DEFAULT_PATTERN)}</code>`, `<select class="sel" data-pattern-pick>${patternOptions(p.organizePattern || DEFAULT_PATTERN)}</select> <button class="btn btn-ghost btn-sm" data-action="organize-pattern">Edit patterns…</button> <button class="btn btn-ghost btn-sm" data-action="organize-all">Organize library…</button>`)}
    ${setRow('M3U playlists', 'Import playlists from other players. To export: right-click a playlist.', `<button class="btn btn-ghost btn-sm" data-action="import-m3u">Import M3U…</button>`)}
    ${setRow('Library check', 'Albums without a cover, songs without lyrics, year, track number or genre, and fixes for all of them.', `<button class="btn btn-ghost btn-sm" data-go="health">Open</button>`)}
    ${setRow('Duplicates', 'The same song in two formats or several copies.', `<button class="btn btn-ghost btn-sm" data-go="duplicates">Find duplicates</button>`)}
    ${setRow('Spotify playlists', `Export them with <span class="lnk" data-action="open-link" data-arg="https://exportify.app">exportify.app</span> (one CSV per playlist) and Wax rebuilds them with your files.`, `<button class="btn btn-ghost btn-sm" data-action="spotify-import">Import CSV…</button>`)}

    <h3>Audio output</h3>
    ${setRow('Device', `<span id="out-status">${esc(outStatus())}</span>`, `<select class="sel" id="out-device"><option value="">Windows default</option>${(S.devices || []).map(d => `<option ${d === S.prefs.outDevice ? 'selected' : ''}>${esc(d)}</option>`).join('')}</select>`)}
    ${setRow('Exclusive mode (WASAPI)', 'Wax takes the device for itself: the sound skips the Windows mixer (no system sounds, no Windows volume or effects). Other apps go silent while Wax is open.', `<label class="switch"><input type="checkbox" id="out-exclusive" ${S.prefs.exclusive ? 'checked' : ''}><span></span></label>`)}

    <h3>Playback</h3>
    ${setRow('Crossfade between songs', `<span id="xf-v">${p.crossfade ? `${p.crossfade} s` : 'No crossfade (songs play back to back with no gaps)'}</span>`, `<input type="range" class="range" min="0" max="12" step="1" value="${p.crossfade || 0}" data-pref="crossfade">`)}
    ${setRow('Also crossfade consecutive tracks of the same album', 'When off, live and concept albums play with no gaps and no fades.', sw('xfadeSameAlbum', false))}
    ${setRow('Normalize volume', 'Make every song play at a similar volume (ReplayGain).', selectPref('normalize', [['off', 'Off'], ['track', 'Per track'], ['album', "Per album (keeps the album's dynamics)"]], 'off'))}
    ${setRow('Preamp', `<span id="pre-v">${fmtDb(p.preamp || 0)}</span>`, `<input type="range" class="range" min="-6" max="6" step="0.5" value="${p.preamp || 0}" data-pref="preamp">`)}
    ${setRow('Prevent clipping', "Never raises the volume beyond the song's peak.", sw('preventClip', true))}
    ${setRow('Analyze library volume', `<span id="loud-status">${withRg} with ReplayGain tags · ${analyzed} analyzed by Wax</span>`, `<button class="btn btn-ghost btn-sm" data-action="loudness-scan">${S.loudScanning ? 'Stop' : 'Analyze'}</button>`)}
    ${setRow('Save the analysis in the files', 'Writes ReplayGain tags to the analyzed songs that have none, so other players use them too.', `<button class="btn btn-ghost btn-sm" data-action="rg-write">Write tags…</button>`)}
    ${setRow('Equalizer', eqState().enabled ? `On · ${esc(eqState().preset)}` : 'Off', `<button class="btn btn-ghost btn-sm" data-go="eq">Open</button>`)}
    ${setRow('Auto DJ', 'When the list or album ends, keeps playing similar music from your library.', sw('autoDJ', false))}
    ${setRow('Auto DJ style', '', selectPref('autoDJMode', [['mix', 'Mix (artist, genre and era)'], ['artist', 'Same artist'], ['genre', 'Same genre'], ['era', 'Same era']], 'mix'))}

    <h3>Lyrics</h3>
    ${setRow('Search lyrics online (LRCLIB)', "When there's no .lrc next to the song and no lyrics in the tags.", sw('lyricsOnline', true))}
    ${setRow('Save .lrc automatically', 'When lyrics are found on LRCLIB, saves them next to the song (like LRCGET).', sw('saveLrc', false))}
    ${setRow('Embed lyrics in the file automatically', '', sw('embedLyrics', false))}
    ${setRow('Download lyrics for the whole library', 'For every song without lyrics, like LRCGET.', `<button class="btn btn-ghost btn-sm" data-action="lyrics-all">Download…</button>`)}

    <h3>Images</h3>
    ${setRow('Find missing album covers online', 'For albums without a cover (from Deezer).', sw('fetchCovers', true))}
    ${setRow('Save downloaded covers as cover.jpg', "In each album's folder.", sw('saveCovers', false))}
    ${setRow('Artist photos', "Uses artist.jpg / folder.jpg from the artist's folder; if there isn't one, looks it up online.", sw('fetchArtistImages', true))}
    ${setRow('Search for images again', 'In case you added images or something changed.', `<button class="btn btn-ghost btn-sm" data-action="clear-images">Search again</button>`)}
    ${setRow('Clean the image cache', "Deletes saved covers and photos of albums and artists that aren't in your library anymore, and images you replaced. Your music folders aren't touched. It also runs by itself once a week.", `<button class="btn btn-ghost btn-sm" data-action="clean-images">Clean</button>`)}
    ${setRow('Embed covers in the files', "For albums whose cover is only an image in the folder (or online). It's in the library check.", `<button class="btn btn-ghost btn-sm" data-go="health">Open</button>`)}

    <h3>Window</h3>
    ${setRow('Closing minimizes to the tray', 'Wax keeps playing next to the clock. To quit: right-click the icon → Exit.', sw('closeToTray', false))}
    ${setRow('Start with Windows', 'Opens Wax when you sign in. If you move the Wax folder, open it once from the new place and it updates itself.', `<label class="switch"><input type="checkbox" data-autostart ${S.autostart ? 'checked' : ''}><span></span></label>`)}
    ${setRow('Start minimized', 'When Windows opens it: minimized, or only in the tray if "Closing minimizes to the tray" is on.', sw('startMinimized', false))}
    ${setRow('Shortcuts', 'A Start menu shortcut makes Wax show up in the Windows search, and from there you can pin it to Start or the taskbar. Keep the Wax folder in a fixed place (not a USB stick) so pins keep working.', `<button class="btn btn-ghost btn-sm" data-action="shortcut" data-arg="start">Add to Start menu</button> <button class="btn btn-ghost btn-sm" data-action="shortcut" data-arg="desktop">Desktop shortcut</button> <span class="lnk" data-action="pin-guide">Pinning to the taskbar</span>`)}
    ${setRow('Notify on song change', "Only when Wax isn't in the foreground.", sw('notify', true))}
    ${setRow('Mini player', 'A small always-on-top window with the cover and controls.', `<button class="btn btn-ghost btn-sm" data-action="mini">Open</button>`)}

    <h3>Global keyboard shortcuts</h3>
    <p class="sub">They work even when Wax is minimized. Click a shortcut and press the new combination (Delete to clear it).</p>
    ${SHORTCUT_ACTIONS.map(([a, l]) => setRow(l, '', `<input class="kbd" readonly data-shortcut="${a}" value="${esc(accelLabel(sc[a] || ''))}" placeholder="No shortcut">`)).join('')}
    ${setRow('', '', `<button class="btn btn-ghost btn-sm" data-action="shortcuts-default">Reset shortcuts</button>`)}

    <h3>Scrobbling</h3>
    ${setRow('ListenBrainz', p.lbUser ? `Connected as <b>${esc(p.lbUser)}</b>` : 'Paste your token (listenbrainz.org → Settings → User token).',
      p.lbUser ? `<button class="btn btn-ghost btn-sm" data-action="lb-off">Disconnect</button>`
        : `<input class="txt" id="lb-token" placeholder="User token" spellcheck="false"> <button class="btn btn-ghost btn-sm" data-action="lb-on">Connect</button>`)}
    ${setRow('Last.fm', p.lastfmUser ? `Connected as <b>${esc(p.lastfmUser)}</b>`
      : `You need your own (free) API key: <span class="lnk" data-action="open-link" data-arg="https://www.last.fm/api/account/create">last.fm/api/account/create</span> · <span class="lnk" data-action="lf-guide">How do I fill it in?</span>`,
      p.lastfmUser ? `<button class="btn btn-ghost btn-sm" data-action="lf-off">Disconnect</button>`
        : `<input class="txt" id="lf-key" data-pref-text="lastfmKey" placeholder="API key" value="${esc(p.lastfmKey || '')}" spellcheck="false"> <input class="txt" id="lf-secret" placeholder="Shared secret" value="${esc(p.lastfmSecret || '')}" spellcheck="false"> <button class="btn btn-ghost btn-sm" data-action="lf-on">${S.lfToken ? 'I authorized it' : 'Connect'}</button>`)}
    <p class="sub">A play is scrobbled after 30 seconds and half of the song (or 4 minutes). When offline, plays are queued and sent later.</p>

    <h3>Last.fm</h3>
    ${p.lastfmKey ? '' : `<p class="sub">These need your Last.fm API key (above). Popular songs, related artists, bios and tags only need the key; loved tracks and history also need you to connect.</p>`}
    ${setRow('Artist and album info', 'Popular songs, related artists, biographies and album descriptions.', sw('lfInfo', true))}
    ${setRow('Sync ♥ with your Loved tracks', 'Marking a song as a favorite also loves it on Last.fm.', sw('lfLoves', true))}
    ${setRow('Use Last.fm tags as genres', 'For songs without a genre in their tags (files are not changed).', sw('lfGenres', false) + (p.lfGenres ? ` <button class="btn btn-ghost btn-sm" data-action="lf-tags">Update</button>` : ''))}
    ${setRow('Import your loved tracks', 'Adds the songs you loved on Last.fm to Your Favorites.', `<button class="btn btn-ghost btn-sm" data-action="lf-loved" ${p.lastfmUser ? '' : 'disabled'}>Import</button>`)}
    ${setRow('Import your listening history', p.lfHistory ? `Imported: ${plural(p.lfHistory, 'play', 'plays')}. Running it again only adds older plays.` : 'Your scrobbles before using Wax, for Your Stats and the play counts.', `<button class="btn btn-ghost btn-sm" data-action="lf-history" ${p.lastfmUser ? '' : 'disabled'}>Import</button>`)}

    <h3>Discord</h3>
    ${setRow('Show what you listen to on Discord', `Needs the ID of a Discord application of your own (free): <span class="lnk" data-action="open-link" data-arg="https://discord.com/developers/applications">discord.com/developers</span> · <span class="lnk" data-action="discord-guide">How do I get it?</span>`, sw('discord', false))}
    ${setRow('Application ID', '', `<input class="txt" data-pref-text="discordId" value="${esc(p.discordId || '')}" placeholder="123456789012345678" spellcheck="false">`)}

    <h3>About</h3>
    ${setRow(`Wax ${esc(S.appInfo?.version || '')}`, `${esc(installLabel())} · <span id="upd-status">${esc(updateStatus())}</span>`, updateButton())}
    ${setRow('Check for updates automatically', 'Once a day, when Wax starts. Updates install in the same place Wax is.', sw('autoUpdate', true))}
    ${setRow('Program folder', `<code>${esc(S.appInfo?.appDir || '')}</code>`, `<button class="btn btn-ghost btn-sm" data-action="open-app">Open</button>`)}
    ${setRow('Data folder', `Library, playlists, settings and image cache: <code>${esc(S.appInfo?.dataDir || 'data')}</code>`, `<button class="btn btn-ghost btn-sm" data-action="open-data">Open</button>`)}
  </div>`;
};

function applyPref(key) {
  switch (key) {
    case 'crossfade': case 'xfadeSameAlbum': case 'preventClip': case 'autoDJ': case 'autoDJMode':
      syncNext(); break;
    case 'normalize': case 'preamp': albumLoudCache.clear(); syncNext(); break;
    case 'watchFolders': watchFolders(); break;
    case 'fetchCovers': case 'saveCovers': coverP.clear(); break;
    case 'fetchArtistImages': artistP.clear(); break;
    case 'lfGenres': if (S.prefs.lfGenres) fetchArtistTags(); buildIndex(); rerender(); break;
    case 'lfInfo': rerender(); break;
    case 'discord': discordSync(); break;
  }
}
view.addEventListener('change', e => {
  if ('patternPick' in e.target.dataset) { S.prefs.organizePattern = e.target.value; savePrefs(); rerender(); return; }
  if ('autostart' in e.target.dataset) {
    const want = e.target.checked;
    invoke('set_autostart', { enabled: want }).then(on => { S.autostart = on; toast(on ? 'Wax will start with Windows' : "Wax won't start with Windows"); })
      .catch(err => { e.target.checked = !want; toast("Couldn't change it: " + err, 4000); });
    return;
  }
  const tk = e.target.dataset.prefText;
  if (tk) { S.prefs[tk] = e.target.value.trim(); savePrefs(); if (tk === 'discordId') discordSync(); if (tk === 'lastfmKey') rerender(); return; }
  const k = e.target.dataset.pref; if (!k) return;
  const v = e.target.type === 'checkbox' ? e.target.checked : e.target.type === 'range' ? +e.target.value : e.target.value;
  S.prefs[k] = v; savePrefs(); applyPref(k);
});
view.addEventListener('input', e => {
  const k = e.target.dataset.pref; if (!k || e.target.type !== 'range') return;
  if (k === 'crossfade') $('#xf-v').textContent = +e.target.value ? `${e.target.value} s` : 'No crossfade (songs play back to back with no gaps)';
  if (k === 'preamp') $('#pre-v').textContent = fmtDb(+e.target.value);
});
view.addEventListener('keydown', e => {
  const a = e.target.dataset?.shortcut; if (!a) return;
  e.preventDefault(); e.stopPropagation();
  const sc = S.prefs.shortcuts || (S.prefs.shortcuts = {});
  if (e.key === 'Delete' || e.key === 'Backspace') { sc[a] = ''; }
  else {
    if (['Control', 'Alt', 'Shift', 'Meta'].includes(e.key)) return;
    const mods = [e.ctrlKey && 'Ctrl', e.altKey && 'Alt', e.shiftKey && 'Shift', e.metaKey && 'Super'].filter(Boolean);
    if (!mods.length && !/^(F\d+|Media|Audio)/.test(e.code)) { toast('Use at least one Ctrl, Alt or Shift key'); return; }
    sc[a] = [...mods, e.code].join('+');
  }
  e.target.value = accelLabel(sc[a]);
  savePrefs(); registerShortcuts();
}, true);

async function settingsAction(a, arg) {
  const p = S.prefs;
  switch (a) {
    case 'loudness-scan':
      if (S.loudScanning) { invoke('stop_loudness_scan'); return; }
      S.loudScanning = true; rerender();
      await invoke('start_loudness_scan', { paths: S.tracks.filter(t => !t.rg && !t.cue).map(t => t.path) });
      return;
    case 'clear-images': await invoke('clear_image_misses'); coverP.clear(); artistP.clear(); toast('Images will be looked up again'); return;
    case 'mini': invoke('toggle_mini', { show: true }); return;
    case 'open-data': invoke('open_data_dir'); return;
    case 'open-app': invoke('open_app_dir'); return;
    case 'open-link': invoke('open_link', { url: arg }); return;
    case 'shortcuts-default': p.shortcuts = { ...DEFAULT_SHORTCUTS }; savePrefs(); registerShortcuts(); rerender(); return;
    case 'lb-on': {
      const token = $('#lb-token')?.value.trim(); if (!token) return;
      try { p.lbUser = await invoke('listenbrainz_check', { token }); p.lbToken = token; savePrefs(); toast(`ListenBrainz connected: ${p.lbUser}`); rerender(); }
      catch (e) { toast(String(e), 4000); }
      return;
    }
    case 'lb-off': p.lbToken = ''; p.lbUser = ''; savePrefs(); rerender(); return;
    case 'lf-on': {
      const key = $('#lf-key')?.value.trim(), secret = $('#lf-secret')?.value.trim();
      if (!key || !secret) { toast('Fill in the API key and the shared secret'); return; }
      p.lastfmKey = key; p.lastfmSecret = secret; savePrefs();
      try {
        if (!S.lfToken) {
          S.lfToken = (await invoke('lastfm_auth_start', { key, secret })).token;
          toast('Authorize Wax in your browser, then click "I authorized it"', 6000); rerender();
        } else {
          const s = await invoke('lastfm_auth_finish', { key, secret, token: S.lfToken });
          p.lastfmSession = s.session; p.lastfmUser = s.name; S.lfToken = null; savePrefs(); toast(`Last.fm connected: ${s.name}`); rerender();
        }
      } catch (e) { toast(String(e), 5000); }
      return;
    }
    case 'lf-off': p.lastfmSession = ''; p.lastfmUser = ''; S.lfToken = null; savePrefs(); rerender(); return;
    case 'import-m3u': importM3U(); return;
    case 'organize-all': organize(S.tracks.map((_, i) => i)); return;
    case 'organize-pattern': patternEditor(p.organizePattern || DEFAULT_PATTERN, () => rerender()); return;
    default: extraAction(a, arg);
  }
}
TAURI.event.listen('loudness-progress', async e => {
  const { done, total, finished } = e.payload;
  const st = $('#loud-status');
  if (st) st.textContent = finished ? 'Analysis finished' : `Analyzing… ${done} of ${total}`;
  if (finished) {
    S.loudScanning = false;
    S.loud = await invoke('load_loudness').catch(() => S.loud);
    albumLoudCache.clear(); syncNext();
    if (route.view === 'settings') rerender();
    toast('Volume analysis finished');
  }
});

/* ───────────── New music detection ───────────── */
function watchFolders() {
  invoke('watch_folders', { folders: S.prefs.folders, enabled: S.prefs.watchFolders !== false }).catch(() => {});
}
TAURI.event.listen('library-changed', () => rescan({ quiet: true }));

/* ───────────── Tag editor (+ MusicBrainz) ───────────── */
const common = (vals) => vals.every(v => v === vals[0]) ? vals[0] : null;
function dialog(html, cls = '') {
  modalBg.innerHTML = `<div class="dlg ${cls}">${html}</div>`;
  modalBg.classList.add('open');
  return $('.dlg', modalBg);
}
const closeDialog = () => { modalBg.classList.remove('open'); modalBg.onclick = null; modalBg.onkeydown = null; };

function editTags(idxs) {
  const ts = idxs.map(i => S.tracks[i]).filter(t => t && !t.cue);
  if (!ts.length) { toast("Songs inside a CUE sheet can't be edited from Wax"); return; }
  idxs = ts.map(t => S.byPath.get(t.path));
  const one = ts.length === 1;
  const val = f => common(ts.map(f));
  const fields = [
    ['title', 'Title', one ? ts[0].title : null, one],
    ['artist', 'Artist (separate several with ;)', val(t => (t.artists?.length > 1 ? t.artists.join('; ') : t.artist)), true],
    ['album', 'Album', val(t => t.album), true],
    ['albumArtist', 'Album artist', val(t => t.albumArtist || ''), true],
    ['trackNo', 'Track #', one ? (ts[0].trackNo || '') : null, one],
    ['discNo', 'Disc', val(t => t.discNo || ''), true],
    ['year', 'Year', val(t => t.year || ''), true],
    ['genres', 'Genre (separate several with ;)', val(t => (t.genres || []).join('; ')), true],
  ];
  const sameAlbum = new Set(ts.map(albumKeyOf)).size === 1;
  const d = dialog(`<h2>${one ? 'Edit info' : `Edit ${ts.length} songs`}</h2>
    ${one ? `<p class="sub path">${esc(ts[0].path)}</p>` : ''}
    <div class="form">${fields.filter(f => f[3]).map(([k, l, v]) => `<label><span>${l}</span><input data-f="${k}" value="${esc(v ?? '')}" placeholder="${v === null ? '(multiple values — left unchanged)' : ''}" spellcheck="false"></label>`).join('')}</div>
    <div class="btns">${sameAlbum ? `<button class="btn btn-ghost" data-d="mb">Fill in from MusicBrainz</button>` : ''}<span class="grow"></span>
      <button class="btn btn-ghost" data-d="cancel">Cancel</button><button class="btn btn-green" data-d="save">Save</button></div>`, 'wide');
  const initial = Object.fromEntries($$('[data-f]', d).map(i => [i.dataset.f, i.value]));
  $('[data-f]', d)?.focus();
  modalBg.onclick = async e => {
    const b = e.target.closest('[data-d]')?.dataset.d;
    if (b === 'cancel' || e.target === modalBg) closeDialog();
    else if (b === 'mb') { closeDialog(); musicBrainz(idxs); }
    else if (b === 'save') {
      const changed = {};
      for (const i of $$('[data-f]', d)) if (i.value !== initial[i.dataset.f]) changed[i.dataset.f] = i.value.trim();
      closeDialog();
      if (!Object.keys(changed).length) return;
      const ups = ts.map(t => {
        const u = { path: t.path };
        if ('title' in changed) u.title = changed.title;
        if ('artist' in changed) {
          const list = changed.artist.split(';').map(x => x.trim()).filter(Boolean);
          u.artist = list.join(', '); if (list.length > 1) u.artists = list;
        }
        if ('album' in changed) u.album = changed.album;
        if ('albumArtist' in changed) u.albumArtist = changed.albumArtist;
        if ('trackNo' in changed) u.trackNo = parseInt(changed.trackNo) || 0;
        if ('discNo' in changed) u.discNo = parseInt(changed.discNo) || 0;
        if ('year' in changed) u.year = parseInt(changed.year) || 0;
        if ('genres' in changed) u.genres = changed.genres.split(';').map(x => x.trim()).filter(Boolean);
        return u;
      });
      writeTags(ups);
    }
  };
  modalBg.onkeydown = e => { if (e.key === 'Escape') closeDialog(); if (e.key === 'Enter') $('[data-d="save"]', d).click(); };
}

// Writes tags; if they touch the playing track, it is reloaded at the same position
async function writeTags(ups) {
  const cur = S.tracks[P.current];
  const touchesCur = cur && ups.some(u => u.path === cur.path);
  const pos = audio.currentTime, wasPlaying = !audio.paused;
  if (touchesCur && audio.mode === 'rust') invoke('audio_stop');
  toast('Saving tags…', 30000);
  try {
    const r = await invoke('write_tags', { updates: ups });
    for (const t of r.tracks) { const i = S.byPath.get(t.path); if (i !== undefined) S.tracks[i] = t; }
    buildIndex(); coverP.clear();
    toast(r.errors.length ? `Saved with ${r.errors.length} errors: ${r.errors[0]}` : plural(r.tracks.length, 'song updated', 'songs updated'), r.errors.length ? 5000 : 2600);
  } catch (e) { toast("Couldn't save: " + e, 5000); }
  if (touchesCur) audio.load(S.tracks[S.byPath.get(cur.path)] || cur, { autoplay: wasPlaying, startAt: pos, gainDb: gainFor(cur) });
  updateNowPlaying(); rerender(); syncNext();
}

async function musicBrainz(idxs) {
  const ts = idxs.map(i => S.tracks[i]);
  const a = S.albumMap.get(albumKeyOf(ts[0]));
  const albumTracks = a.tracks.map(i => S.tracks[i]);
  let d = dialog(`<h2>MusicBrainz</h2><p class="sub">Searching for "${esc(a.title)}" by ${esc(a.artist)}…</p><div class="ly-status"><span class="spin"></span></div>`, 'wide');
  modalBg.onclick = e => { if (e.target === modalBg || e.target.closest('[data-d="cancel"]')) closeDialog(); };
  let releases;
  try { releases = await invoke('mb_search', { artist: a.albumArtist || a.artist, album: a.title }); }
  catch (e) { d.innerHTML = `<h2>MusicBrainz</h2><p class="sub">${esc(String(e))}</p><div class="btns"><span class="grow"></span><button class="btn btn-ghost" data-d="cancel">Close</button></div>`; return; }
  if (!releases.length) { d.innerHTML = `<h2>MusicBrainz</h2><p class="sub">"${esc(a.title)}" wasn't found.</p><div class="btns"><span class="grow"></span><button class="btn btn-ghost" data-d="cancel">Close</button></div>`; return; }
  const n = albumTracks.length;
  releases.sort((x, y) => (Math.abs(x.tracks - n) - Math.abs(y.tracks - n)));
  d.innerHTML = `<h2>Choose the release</h2><p class="sub">Your album has ${plural(n, 'song', 'songs')}.</p>
    <div class="mb-list">${releases.map(r => `<div class="mb-rel" data-rel="${esc(r.id)}"><div><b>${esc(r.title)}</b> — ${esc(r.artist)}</div>
      <div class="m">${[r.date, r.country, r.format, plural(r.tracks, 'track', 'tracks')].filter(Boolean).map(esc).join(' · ')}${r.tracks === n ? ' <span class="ok">✓ matches</span>' : ''}</div></div>`).join('')}</div>
    <div class="btns"><span class="grow"></span><button class="btn btn-ghost" data-d="cancel">Cancel</button></div>`;
  modalBg.onclick = async e => {
    if (e.target === modalBg || e.target.closest('[data-d="cancel"]')) { closeDialog(); return; }
    const rel = e.target.closest('[data-rel]')?.dataset.rel; if (!rel) return;
    d.innerHTML = `<h2>MusicBrainz</h2><div class="ly-status"><span class="spin"></span></div>`;
    let info;
    try { info = await invoke('mb_release', { id: rel }); } catch (err) { toast(String(err), 4000); closeDialog(); return; }
    // Match by disc + track number; when numbers are missing, by order
    const sorted = [...albumTracks];
    const hasNums = sorted.every(t => t.trackNo > 0);
    const pairs = sorted.map((t, k) => [t, hasNums ? info.tracks.find(m => m.number === t.trackNo && m.disc === (t.discNo || 1)) || info.tracks.find(m => m.number === t.trackNo) : info.tracks[k]]);
    const multiDisc = new Set(info.tracks.map(t => t.disc)).size > 1;
    d.innerHTML = `<h2>${esc(info.title)}</h2><p class="sub">${esc(info.artist)}${info.year ? ' · ' + info.year : ''}${info.genres.length ? ' · ' + esc(info.genres.join(', ')) : ''}</p>
      <div class="mb-map">${pairs.map(([t, m]) => `<div class="mb-row ${m ? '' : 'miss'}"><span class="o">${esc(t.title)}</span><span class="arr">→</span><span class="nw">${m ? `${m.number}. ${esc(m.title)}${m.artist !== info.artist ? ` <i>— ${esc(m.artist)}</i>` : ''}` : 'no match'}</span></div>`).join('')}</div>
      <label class="chk"><input type="checkbox" id="mb-genres" ${info.genres.length ? 'checked' : 'disabled'}> Apply genres (${esc(info.genres.join(', ') || 'none')})</label>
      <div class="btns"><span class="grow"></span><button class="btn btn-ghost" data-d="cancel">Cancel</button><button class="btn btn-green" data-d="apply">Apply to ${plural(pairs.filter(p => p[1]).length, 'song', 'songs')}</button></div>`;
    modalBg.onclick = ev => {
      if (ev.target === modalBg || ev.target.closest('[data-d="cancel"]')) { closeDialog(); return; }
      if (!ev.target.closest('[data-d="apply"]')) return;
      const genres = $('#mb-genres')?.checked ? info.genres : null;
      closeDialog();
      writeTags(pairs.filter(p => p[1]).map(([t, m]) => ({
        path: t.path, title: m.title, artist: m.artist, artists: m.artists.length > 1 ? m.artists : null,
        album: info.title, albumArtist: info.artist, trackNo: m.number, discNo: multiDisc ? m.disc : (t.discNo || null), year: info.year || null,
        genres,
      })));
    };
  };
}

/* ───────────── File organizer ───────────── */
// {folder} = the file's current folders below the artist folder (e.g. "Singles\Song"), so the
// default only renames files and never changes how the user arranged their folders.
const DEFAULT_PATTERN = '{artist}\\{folder}\\{track} - {title}';
const PATTERN_VARS = ['artist', 'folder', 'album', 'track', 'disc', 'title', 'year'];
const BUILTIN_PATTERNS = [
  { name: 'Rename only (keep folders)', pattern: DEFAULT_PATTERN, builtin: true },
  { name: 'Artist \\ Album', pattern: '{artist}\\{album}\\{track} - {title}', builtin: true },
  { name: 'Artist \\ Year - Album', pattern: '{artist}\\{year} - {album}\\{track} - {title}', builtin: true },
];
const patternPresets = () => [...BUILTIN_PATTERNS, ...(S.prefs.patternPresets || [])];
const patternOptions = cur => {
  const list = patternPresets();
  return (list.some(x => x.pattern === cur) ? '' : `<option value="${esc(cur)}" selected>Custom: ${esc(cur)}</option>`)
    + list.map(x => `<option value="${esc(x.pattern)}" ${x.pattern === cur ? 'selected' : ''}>${esc(x.name)}</option>`).join('');
};
const safeName = s => String(s ?? '').replace(/[<>:"/\\|?*\u0000-\u001f]/g, '_').replace(/[. ]+$/, '').trim() || '_';
function rootOf(path) {
  const lp = path.toLowerCase();
  return S.prefs.folders.map(f => f.replace(/[\\/]+$/, '')).filter(f => lp.startsWith(f.toLowerCase() + '\\') || lp.startsWith(f.toLowerCase() + '/')).sort((a, b) => b.length - a.length)[0];
}
function targetPath(t, pattern) {
  const root = rootOf(t.path); if (!root) return null;
  const ext = t.path.match(/\.[^.\\/]+$/)?.[0] || '';
  const a = S.albumMap.get(albumKeyOf(t));
  const multi = a && new Set(a.tracks.map(i => S.tracks[i].discNo || 1)).size > 1;
  const fa = folderArtistOf(t);
  const dirs = fa ? t.path.slice(fa.dir.length + 1).split(/[\\/]/).slice(0, -1) : [];
  const vals = {
    artist: fa?.name || t._owner || t.albumArtist || t.artist, album: t.album, title: t.title,
    folder: dirs.length ? dirs.join('\u0001') : t.album,  // \u0001 = folder break, split below
    track: t.trackNo ? (multi ? `${t.discNo || 1}-` : '') + String(t.trackNo).padStart(2, '0') : '', disc: t.discNo || 1, year: t.year || '',
  };
  const rel = pattern.split(/[\\/]/).flatMap(seg => seg.replace(/\{([^}]+)\}/g, (_, k) => vals[k.toLowerCase()] ?? '').split('\u0001'))
    .map(seg => safeName(seg.replace(/^\s*-\s*/, ''))).join('\\');
  return `${root}\\${rel}${ext}`;
}
function organize(idxs, pattern = S.prefs.organizePattern || DEFAULT_PATTERN) {
  const moves = [], seen = new Map(), clashes = [];
  for (const i of idxs) {
    const t = S.tracks[i], to = !t.cue && targetPath(t, pattern);
    if (!to || to.toLowerCase() === t.path.toLowerCase()) continue;
    const k = to.toLowerCase();
    if (seen.has(k)) { clashes.push(t.path); continue; }
    seen.set(k, t.path);
    moves.push({ from: t.path, to });
  }
  const short = p => { const r = rootOf(p); return r ? p.slice(r.length + 1) : p; };
  const d = dialog(`<h2>Organize files</h2>
    <div class="pat-pick"><select class="sel" id="org-pat">${patternOptions(pattern)}</select><code>${esc(pattern)}</code></div>
    <p class="sub">${moves.length ? plural(moves.length, 'file will be moved', 'files will be moved') : 'Everything already matches this pattern'}${clashes.length ? ` · ${clashes.length} skipped (same destination)` : ''}. The .lrc files and the covers of folders left empty are moved too.</p>
    ${moves.length ? `<div class="mb-map">${moves.slice(0, 300).map(m => `<div class="mb-row"><span class="o">${esc(short(m.from))}</span><span class="arr">→</span><span class="nw">${esc(short(m.to))}</span></div>`).join('')}${moves.length > 300 ? `<div class="sub">… and ${moves.length - 300} more</div>` : ''}</div>` : ''}
    <div class="btns"><button class="btn btn-ghost" data-d="edit">Edit patterns…</button><span class="grow"></span><button class="btn btn-ghost" data-d="cancel">Cancel</button><button class="btn btn-green" data-d="go" ${moves.length ? '' : 'disabled'}>Move files</button></div>`, 'wide');
  $('#org-pat', d).onchange = e => organize(idxs, e.target.value);
  modalBg.onclick = async e => {
    if (e.target === modalBg || e.target.closest('[data-d="cancel"]')) { closeDialog(); return; }
    if (e.target.closest('[data-d="edit"]')) { patternEditor(pattern, pat => organize(idxs, pat)); return; }
    if (!e.target.closest('[data-d="go"]') || !moves.length) return;
    if (pattern !== S.prefs.organizePattern) { S.prefs.organizePattern = pattern; savePrefs(); }
    closeDialog();
    const r = await runMoves(moves);
    if (!r) return;
    // Kept so it can be undone (app menu)
    if (r.moved.length) { S.prefs.lastOrganize = { when: Date.now(), moves: r.moved.map(([from, to]) => ({ from, to })) }; savePrefs(); }
    toast(r.errors.length ? `${r.moved.length} moved, ${r.errors.length} failed: ${r.errors[0]}` : plural(r.moved.length, 'file organized', 'files organized') + ' · it can be undone from the ··· menu', 5000);
  };
}

// Moves files and updates everything that points at them (playlists, favorites, history…)
async function runMoves(moves) {
    const cur = S.tracks[P.current], pos = audio.currentTime, wasPlaying = !audio.paused;
    toast('Moving files…', 60000);
    let r;
    try { r = await invoke('organize_files', { moves, roots: S.prefs.folders }); } catch (err) { toast('Error: ' + err, 5000); return null; }
    const map = new Map(r.moved);
    const fix = p => map.get(p) || p;
    const pr = S.prefs;
    for (const pl of pr.playlists) pl.tracks = pl.tracks.map(fix);
    pr.favorites = (pr.favorites || []).map(fix); S.fav = new Set(pr.favorites);
    pr.ratings = Object.fromEntries(Object.entries(pr.ratings || {}).map(([k, v]) => [fix(k), v]));
    pr.plays = Object.fromEntries(Object.entries(pr.plays).map(([k, v]) => [fix(k), v]));
    pr.pendingEmbeds = Object.fromEntries(Object.entries(pr.pendingEmbeds || {}).map(([k, v]) => [fix(k), v]));
    if (pr.last) pr.last = fix(pr.last);
    S.loud = Object.fromEntries(Object.entries(S.loud).map(([k, v]) => [fix(k), v]));
    Stats.dirty = true;
    savePrefs();
    await rescan({ quiet: true, moved: map });
    if (cur && map.has(cur.path)) {
      const ni = S.byPath.get(map.get(cur.path));
      if (ni !== undefined) { P.current = ni; audio.load(S.tracks[ni], { autoplay: wasPlaying, startAt: pos, gainDb: gainFor(S.tracks[ni]) }); }
    }
    return r;
}

// Pattern editor: edit the pattern, preview it on a real song and keep named presets.
// `done(pattern)` runs after "Use pattern" (it also becomes the default pattern).
function patternEditor(start, done) {
  let pat = start;
  const sample = S.tracks[P.current] || S.tracks.find(t => folderArtistOf(t)) || S.tracks[0];
  const preview = () => {
    if (!sample) return '';
    const to = targetPath(sample, pat), r = rootOf(sample.path);
    return to && r ? to.slice(r.length + 1) : '';
  };
  const draw = () => {
    const d = dialog(`<h2>Organize pattern</h2>
      <p class="sub">{folder} keeps the folders each file is in now (e.g. Singles); {album} rebuilds them from the tags. Use \\ to separate folders.</p>
      <div class="form one"><label><span>Pattern</span><input id="pat-in" value="${esc(pat)}" spellcheck="false"></label></div>
      <div class="pat-vars">${PATTERN_VARS.map(v => `<button class="chip" data-v="${v}">{${v}}</button>`).join('')}</div>
      ${sample ? `<p class="sub pat-ex">Example: <span id="pat-ex">${esc(preview())}</span></p>` : ''}
      <h3 class="pat-h">Presets</h3>
      <div class="mb-list pat-list">${patternPresets().map((x, i) => `<div class="pat-row ${x.pattern === pat ? 'on' : ''}" data-p="${i}"><div class="grow"><div>${esc(x.name)}</div><code>${esc(x.pattern)}</code></div>${x.builtin ? '' : `<button class="ib" data-del="${i}" title="Delete preset">${ic('x', 16)}</button>`}</div>`).join('')}</div>
      <div class="btns"><input class="txt" id="pat-name" placeholder="Preset name" spellcheck="false"><button class="btn btn-ghost" data-d="save-preset">Save as preset</button><span class="grow"></span>
        <button class="btn btn-ghost" data-d="cancel">Cancel</button><button class="btn btn-green" data-d="ok">Use pattern</button></div>`, 'wide');
    const inp = $('#pat-in', d);
    inp.oninput = () => { pat = inp.value; const ex = $('#pat-ex', d); if (ex) ex.textContent = preview(); };
    modalBg.onkeydown = e => { if (e.key === 'Escape') closeDialog(); };
    modalBg.onclick = e => {
      const v = e.target.closest('[data-v]')?.dataset.v;
      if (v) {   // insert the variable at the cursor
        const a = inp.selectionStart ?? pat.length, b = inp.selectionEnd ?? a;
        pat = pat.slice(0, a) + `{${v}}` + pat.slice(b); draw();
        const n = $('#pat-in', modalBg); n.focus(); n.setSelectionRange(a + v.length + 2, a + v.length + 2);
        return;
      }
      const del = e.target.closest('[data-del]')?.dataset.del;
      if (del !== undefined) {
        const own = S.prefs.patternPresets || [];
        own.splice(+del - BUILTIN_PATTERNS.length, 1); S.prefs.patternPresets = own; savePrefs(); draw(); return;
      }
      const row = e.target.closest('[data-p]');
      if (row) { pat = patternPresets()[+row.dataset.p].pattern; draw(); return; }
      const b = e.target.closest('[data-d]')?.dataset.d;
      if (b === 'cancel' || e.target === modalBg) { closeDialog(); return; }
      if (b === 'save-preset') {
        const name = $('#pat-name', d).value.trim();
        if (!name) { toast('Give the preset a name'); $('#pat-name', d).focus(); return; }
        if (!/\{title\}/i.test(pat)) { toast('The pattern needs {title} so files keep distinct names'); return; }
        const own = (S.prefs.patternPresets || []).filter(x => x.name.toLowerCase() !== name.toLowerCase());
        own.push({ name, pattern: pat.trim() }); S.prefs.patternPresets = own; savePrefs();
        toast(`Preset "${name}" saved`); draw(); return;
      }
      if (b === 'ok') {
        if (!/\{title\}/i.test(pat)) { toast('The pattern needs {title} so files keep distinct names'); return; }
        S.prefs.organizePattern = pat.trim(); savePrefs(); closeDialog();
        done?.(S.prefs.organizePattern);
      }
    };
  };
  draw();
}

/* ───────────── M3U playlists ───────────── */
async function exportM3U(id) {
  const pl = S.prefs.playlists.find(p => p.id === id); if (!pl) return;
  const entries = pl.tracks.map(p => S.tracks[S.byPath.get(p)]).filter(t => t && !t.cue).map(t => ({ path: t.path, title: t.title, artist: t.artist, duration: t.duration || 0 }));
  try { const out = await invoke('export_m3u', { name: pl.name, entries }); if (out) toast(`Playlist exported: ${out}`, 4000); }
  catch (e) { toast("Couldn't export: " + e, 4000); }
}
async function importM3U() {
  let r;
  try { r = await invoke('import_m3u'); } catch (e) { toast("Couldn't read the playlist: " + e, 4000); return; }
  if (!r) return;
  const lower = new Map(S.tracks.map(t => [t.path.toLowerCase(), t.path]));
  const found = r.paths.map(p => lower.get(p.toLowerCase())).filter(Boolean);
  if (!found.length) { toast('None of the songs in that playlist are in your library', 4000); return; }
  const pl = { id: Date.now().toString(36), name: r.name, tracks: found, created: Date.now() };
  S.prefs.playlists.push(pl); savePrefs(); renderSidebar();
  go({ view: 'playlist', arg: pl.id });
  const miss = r.paths.length - found.length;
  toast(miss ? `Imported ${found.length} songs (${miss} aren't in your library)` : `Playlist imported: ${found.length} songs`, 4000);
}

/* ───────────── Window / user menu ───────────── */
$('#w-close').onclick = () => S.prefs.closeToTray ? invoke('hide_main') : appWin.close();
$('#user-menu').onclick = e => {
  const r = e.currentTarget.getBoundingClientRect();
  openMenu(r.left, r.bottom + 4, [
    { label: 'Settings', act: () => go({ view: 'settings' }) },
    { label: 'Your Stats', act: () => go({ view: 'stats' }) },
    { label: 'Equalizer', act: () => go({ view: 'eq' }) },
    '-',
    { label: 'Mini player', act: () => invoke('toggle_mini', { show: true }) },
  ]);
};

/* ───────────── Startup ───────────── */
// extras.js (loaded right after this file) adds features that startup uses
let extrasReady;
const extrasLoaded = new Promise(r => { extrasReady = r; });
(async function init() {
  await extrasLoaded;
  paintIcons();
  try { const s = await invoke('load_state'); if (s) Object.assign(S.prefs, JSON.parse(s)); } catch (e) { console.warn(e); }
  if (!S.prefs.shortcuts) S.prefs.shortcuts = { ...DEFAULT_SHORTCUTS };
  S.fav = new Set(S.prefs.favorites || []);
  await audio.init();
  invoke('app_info').then(i => { S.appInfo = i; }).catch(() => {});
  invoke('autostart_enabled').then(on => { S.autostart = on; }).catch(() => {});
  S.loud = await invoke('load_loudness').catch(() => ({}));
  applyEq();
  applyAudioPrefs();
  registerShortcuts();
  try { S.tracks = await invoke('load_library', { folders: S.prefs.folders }); } catch (e) { console.warn(e); }
  buildIndex();
  renderSidebar();
  setVolume(S.prefs.volume, S.prefs.muted);
  syncModeButtons();
  document.body.classList.toggle('big-cover', !!S.prefs.bigCover);

  const last = S.prefs.last && S.byPath.get(S.prefs.last);
  if (last !== undefined && last !== null) {
    const a = S.albumMap.get(albumKeyOf(S.tracks[last]));
    P.ctx = a.tracks.slice(); P.ctxRoute = { view: 'album', arg: a.key };
    buildOrder(P.ctx.indexOf(last));
    playTrack(last, false);
  } else updateNowPlaying();
  syncPlayButton();

  go({ view: S.tracks.length ? 'browse' : 'local' });
  setLyricsWidth(S.prefs.lyricsWidth || 340);
  if (S.prefs.lyricsPanel) setLyricsPanel(true);
  flushPendingEmbeds();   // lyrics left pending from the previous session
  watchFolders();
  if (S.prefs.lfGenres) fetchArtistTags();
  discordSync();
  // Library saved by an older version (without genres/ReplayGain): rescan
  if (S.prefs.folders.length && (!S.tracks.length || !('genres' in S.tracks[0]))) rescan({ quiet: S.tracks.length > 0 });
})();
