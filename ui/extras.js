'use strict';
/* Wax — extra features, loaded after app.js (uses its globals):
   Last.fm (popular songs, related artists, bios, loved tracks, history, tags as genres),
   folder view, duplicates, library check, batch lyrics / covers / ReplayGain, sleep timer,
   playlist folders, drag & drop reordering, ratings, Discord status and copying to a folder. */

/* ───────────── Background tasks: a progress pill with Cancel ───────────── */
const taskEl = $('#task');
function startTask(label) {
  const t = {
    label, cancelled: false,
    update(done, total, note = '') {
      if (t.cancelled) return;
      const pct = total ? Math.round(done / total * 100) : 0;
      taskEl.innerHTML = `<div class="tk-t">${esc(t.label)}</div><div class="tk-s">${total ? `${done} of ${total}` : ''}${note ? ` · ${esc(note)}` : ''}</div>
        <div class="tk-bar"><div style="width:${pct}%"></div></div><button class="ib" data-task-cancel title="Cancel">${ic('x', 14)}</button>`;
      taskEl.classList.add('show');
    },
    finish(msg, ms = 6000) { taskEl.classList.remove('show'); Task.cur = null; if (msg) toast(msg, ms); },
  };
  Task.cur = t;
  t.update(0, 0);
  return t;
}
const Task = { cur: null };
taskEl.addEventListener('click', e => {
  if (!e.target.closest('[data-task-cancel]') || !Task.cur) return;
  Task.cur.cancelled = true; taskEl.classList.remove('show'); toast('Cancelled');
  if (Task.cur.onCancel) Task.cur.onCancel();
});
const busy = () => { if (Task.cur) { toast('Wait for the current task to finish (bottom right)'); return true; } return false; };
TAURI.event.listen('batch-progress', e => { const t = Task.cur; if (t && t.kind === e.payload.kind) t.update(e.payload.done, e.payload.total); });

const sleepMs = ms => new Promise(r => setTimeout(r, ms));
const confirmBox = (title, text, ok) => modal({ title, text, ok, input: false });

// After rewriting tags: put the new tracks in the library
function applyTracks(list) {
  for (const t of list || []) { const i = S.byPath.get(t.path); if (i !== undefined) S.tracks[i] = t; }
  buildIndex(); coverP.clear();
}
// Rewriting tags shifts the audio inside the file: the playing track is released and reloaded
async function withReleased(paths, fn) {
  const cur = S.tracks[P.current];
  const touches = cur && paths.includes(realPath(cur));
  const pos = audio.currentTime, playing = !audio.paused;
  if (touches && audio.mode === 'rust') await invoke('audio_stop').catch(() => {});
  try { return await fn(); }
  finally {
    if (touches) { const t = S.tracks[S.byPath.get(cur.path)] || cur; audio.load(t, { autoplay: playing, startAt: pos, gainDb: gainFor(t) }); }
  }
}

/* ───────────── Matching names against the library ───────────── */
const normKey = s => norm(s).replace(/^the\s+/, '').replace(/[^\p{L}\p{N}]+/gu, '');
// Title without "(Remastered 2011)", "- 2009 Remaster", "(feat. X)" and punctuation
const normTitle = s => norm(s)
  .replace(/\s*[([][^)\]]*\b(remaster\w*|mono|stereo|feat\.?|ft\.|featuring|bonus|deluxe|single version|album version|radio edit)\b[^)\]]*[)\]]/g, '')
  .replace(/\s+-\s+[^-]*\b(remaster\w*|mono|stereo|single version|album version|radio edit)\b.*$/, '')
  .replace(/[^\p{L}\p{N}]+/gu, '');
const TI = { src: null, map: null };
function findTrack(artist, title) {
  if (TI.src !== S.tracks) {
    TI.map = new Map();
    S.tracks.forEach((t, i) => {
      const tk = normTitle(t.title);
      for (const n of new Set([t.artist, t._owner, t.albumArtist, ...t._artists].filter(Boolean).map(normKey))) {
        const k = n + '|' + tk; if (!TI.map.has(k)) TI.map.set(k, i);
      }
    });
    TI.src = S.tracks;
  }
  return TI.map.get(normKey(artist) + '|' + normTitle(title));
}

/* ───────────── Last.fm ───────────── */
const lfKey = () => (S.prefs.lastfmKey || '').trim();
const lfInfoOn = () => !!lfKey() && S.prefs.lfInfo !== false;
async function lf(method, params, days = 14) {
  if (!lfKey()) return null;
  try {
    const v = await invoke('lastfm_get', { key: lfKey(), method, params, cacheDays: days });
    return v && !v.error ? v : null;
  } catch (e) { console.warn(method, e); if (/API key/.test(e)) toast(String(e), 4000); return null; }
}
const fmtCount = n => { n = +n || 0; return n >= 1e6 ? (n / 1e6).toFixed(1).replace('.0', '') + 'M' : n >= 1e3 ? Math.round(n / 1e3) + 'K' : String(n); };
const lfText = html => String(html || '')
  .replace(/<a [^>]*>Read more on Last\.fm<\/a>\.?/gi, '')
  .replace(/User-contributed text is available under the Creative Commons[\s\S]*$/i, '')
  .replace(/<[^>]+>/g, '').trim();
const paras = text => text.split(/\n\s*\n|\n/).map(x => x.trim()).filter(Boolean).map(x => `<p>${esc(x)}</p>`).join('');
const arr = x => Array.isArray(x) ? x : x ? [x] : [];

/* Artist page: Overview / Related Artists / About */
function artistTabs(tab) {
  const t = (k, l) => `<span class="tab ${tab === k ? 'on' : ''}" data-artist-tab="${k}">${l}</span>`;
  return `<div class="tabs">${t('overview', 'Overview')}${t('related', 'Related Artists')}${t('about', 'About')}</div>`;
}
view.addEventListener('click', e => {
  const t = e.target.closest('[data-artist-tab]'); if (!t || route.view !== 'artist') return;
  const tab = t.dataset.artistTab;
  if ((route.tab || 'overview') !== tab) go({ view: 'artist', arg: route.arg, tab });
});
// "Popular": 5 songs, and 5 more on demand, like the old Spotify
const Pop = { list: [], more: false, src: '' };
function popularHtml(idxs, src = Pop.src) {
  Pop.list = idxs;
  const shown = idxs.slice(0, Pop.more ? 10 : 5);
  return `<div class="sec-h"><h2>Popular</h2>${src ? `<span class="src">${esc(src)}</span>` : ''}</div>${trackTable('artist-pop', shown, 'short')}
    ${idxs.length > 5 ? `<a class="more-toggle" data-action="pop-more">${Pop.more ? 'Show only 5 songs' : 'Show 5 more'}</a>` : ''}`;
}
function refreshPopular() {
  const box = $('#artist-pop'); if (!box) return;
  box.innerHTML = popularHtml(Pop.list); hydrate(box); markPlaying();
}

async function artistExtras(ar, tab, localPopular) {
  const here = () => route.view === 'artist' && (route.tab || 'overview') === tab && (S.artistMap.get(route.arg) === ar || ownerOf(route.arg) === ar.name);
  if (tab === 'overview') {
    Pop.src = '';
    if (!lfInfoOn()) return;
    const v = await lf('artist.getTopTracks', { artist: ar.name, limit: '100', autocorrect: '1' });
    if (!here() || !v) return;
    const mine = new Map();
    for (const i of ar.tracks) { const k = normTitle(S.tracks[i].title); if (!mine.has(k)) mine.set(k, i); }
    const seen = new Set(), idxs = [];
    for (const t of arr(v.toptracks?.track)) {
      const i = mine.get(normTitle(t.name));
      if (i !== undefined && !seen.has(i)) { seen.add(i); idxs.push(i); if (idxs.length >= 10) break; }
    }
    if (!idxs.length) return;
    for (const i of localPopular) if (idxs.length < 10 && !seen.has(i)) { seen.add(i); idxs.push(i); }
    Pop.src = 'Last.fm';
    const box = $('#artist-pop'); if (!box) return;
    box.innerHTML = popularHtml(idxs, 'Last.fm'); hydrate(box); markPlaying();
    return;
  }
  const box = $(tab === 'related' ? '#artist-related' : '#artist-about');
  const noKey = `<div class="empty"><h2>This comes from Last.fm</h2><p>Add your Last.fm API key in Settings → Scrobbling (it's free).</p><button class="btn btn-ghost" data-go="settings">Open Settings</button></div>`;
  if (!lfKey() || S.prefs.lfInfo === false) { box.innerHTML = noKey; return; }

  if (tab === 'related') {
    const v = await lf('artist.getSimilar', { artist: ar.name, limit: '80', autocorrect: '1' });
    if (!here()) return;
    const names = arr(v?.similarartists?.artist);
    const inLib = [], outside = [], seen = new Set([ar.name]);
    for (const a of names) {
      const own = ownerOf(a.name);
      if (own && S.artistMap.has(own)) { if (!seen.has(own)) { seen.add(own); inLib.push(S.artistMap.get(own)); } }
      else if (outside.length < 18) outside.push(a);
    }
    if (!inLib.length && !outside.length) { box.innerHTML = `<div class="empty"><h2>No related artists</h2><p>Last.fm doesn't know artists similar to ${esc(ar.name)}.</p></div>`; return; }
    box.innerHTML = (inLib.length ? secHead('In your library') + gridHtml('related-lib', inLib, x => artistCard(x)) : '')
      + (outside.length ? `<div class="sec-h"><h2>More like ${esc(ar.name)}</h2><span class="src">Not in your library · Last.fm</span></div>
        <div class="grid">${outside.map(a => `<div class="card round" data-action="open-link" data-arg="${esc(a.url || '')}" title="Open on Last.fm">
          ${cvHtml(null, '', 40, false, a.name)}<div class="t">${esc(a.name)}</div><div class="s">${Math.round((+a.match || 0) * 100)}% match</div></div>`).join('')}</div>` : '');
    hydrate(box);
    return;
  }

  // About
  const v = await lf('artist.getInfo', { artist: ar.name, autocorrect: '1' });
  if (!here()) return;
  const info = v?.artist;
  const bio = lfText(info?.bio?.content || info?.bio?.summary);
  const tags = arr(info?.tags?.tag).map(t => t.name).slice(0, 6);
  const years = ar.albums.map(a => a.year).filter(Boolean);
  const dur = ar.tracks.reduce((s, i) => s + (S.tracks[i].duration || 0), 0);
  box.innerHTML = `<div class="about">
    <div class="about-side">
      ${info ? `<div class="stat"><div class="n">${fmtCount(info.stats?.listeners)}</div><div class="l">listeners on Last.fm</div></div>
        <div class="stat"><div class="n">${fmtCount(info.stats?.playcount)}</div><div class="l">plays on Last.fm</div></div>` : ''}
      <div class="stat"><div class="n">${plural(ar.albums.length, 'album', 'albums')}</div><div class="l">${plural(ar.tracks.length, 'song', 'songs')} · ${fmtLong(dur)} in your library${years.length ? ` · ${Math.min(...years)}–${Math.max(...years)}` : ''}</div></div>
      ${tags.length ? `<div class="tags">${tags.map(t => `<span class="tagc">${esc(t)}</span>`).join('')}</div>` : ''}
    </div>
    <div class="about-bio">${bio ? paras(bio) : `<p class="sub">Last.fm has no biography for ${esc(ar.name)}.</p>`}
      ${info?.url ? `<p class="src"><span class="lnk" data-action="open-link" data-arg="${esc(info.url)}">Read more on Last.fm</span></p>` : ''}</div>
  </div>`;
}

async function albumAbout(a) {
  if (!lfInfoOn()) return;
  const v = await lf('album.getInfo', { artist: a.albumArtist || a.owner || a.artist, album: a.title, autocorrect: '1' });
  const box = $('#album-about');
  if (!box || route.view !== 'album' || route.arg !== a.key) return;
  const text = lfText(v?.album?.wiki?.content || v?.album?.wiki?.summary);
  if (!text) return;
  box.innerHTML = `<div class="album-about clamp">${paras(text)}</div><a class="more-toggle" data-action="about-more">More</a>`;
}

/* ♥ ↔ Loved tracks */
let loveQueue = Promise.resolve();
function lfLoveSync(paths, love) {
  const p = S.prefs;
  if (!p.lastfmSession || !p.lastfmKey || p.lfLoves === false) return;
  const creds = scrobbleCreds();
  for (const path of paths.slice(0, 500)) {
    const t = S.tracks[S.byPath.get(path)]; if (!t) continue;
    loveQueue = loveQueue.then(() => invoke('lastfm_love', { creds, artist: t.artist, title: t.title, love }).catch(e => console.warn('love', e))).then(() => sleepMs(250));
  }
}
async function importLoved() {
  const user = S.prefs.lastfmUser; if (!user || busy()) return;
  const task = startTask('Importing loved tracks from Last.fm');
  let page = 1, pages = 1, found = 0, missing = 0;
  while (page <= pages && !task.cancelled) {
    const v = await lf('user.getLovedTracks', { user, limit: '200', page: String(page) }, 0);
    if (!v) break;
    pages = +v.lovedtracks?.['@attr']?.totalPages || 1;
    for (const t of arr(v.lovedtracks?.track)) {
      const i = findTrack(t.artist?.name || '', t.name);
      if (i === undefined) { missing++; continue; }
      if (!S.fav.has(S.tracks[i].path)) { S.fav.add(S.tracks[i].path); found++; }
    }
    task.update(page, pages);
    page++;
  }
  S.prefs.favorites = [...S.fav]; savePrefs();
  if (route.view === 'favorites' || route.view === 'settings') rerender();
  task.finish(`${plural(found, 'loved track added', 'loved tracks added')} to Your Favorites${missing ? ` · ${missing} aren't in your library` : ''}`);
}
async function importHistory() {
  const user = S.prefs.lastfmUser; if (!user || busy()) return;
  const hist = await invoke('load_history').catch(() => []);
  // Only plays from before the oldest one Wax has: nothing gets counted twice
  const oldest = hist.reduce((m, l) => Math.min(m, l.t), Infinity);
  const task = startTask('Importing your Last.fm history');
  let page = 1, pages = 1, added = 0, missing = 0;
  while (page <= pages && !task.cancelled) {
    const params = { user, limit: '200', page: String(page) };
    if (isFinite(oldest)) params.to = String(oldest - 1);
    const v = await lf('user.getRecentTracks', params, 0);
    if (!v) break;
    pages = +v.recenttracks?.['@attr']?.totalPages || 1;
    const listens = [];
    for (const t of arr(v.recenttracks?.track)) {
      if (t['@attr']?.nowplaying || !t.date?.uts) continue;
      const i = findTrack(t.artist?.['#text'] || t.artist?.name || '', t.name);
      if (i === undefined) { missing++; continue; }
      const tr = S.tracks[i];
      listens.push({ p: tr.path, t: +t.date.uts, s: Math.round(tr.duration || 0) });
      S.prefs.plays[tr.path] = (S.prefs.plays[tr.path] || 0) + 1;
    }
    if (listens.length) { await invoke('append_history_bulk', { listens }); added += listens.length; }
    task.update(page, pages, plural(added, 'play', 'plays'));
    page++;
    await sleepMs(220);   // Last.fm asks for at most ~5 requests per second
  }
  S.prefs.lfHistory = (S.prefs.lfHistory || 0) + added; savePrefs();
  Stats.dirty = true;
  if (route.view === 'settings' || route.view === 'stats') rerender();
  task.finish(`${plural(added, 'play imported', 'plays imported')}${missing ? ` · ${missing} were songs that aren't in your library` : ''}`);
}

/* Last.fm tags as genres (for artists whose songs have no genre) */
const TAG_SKIP = /^(seen live|favou?rites?|favorite|albums i own|under \d+|my |love|beautiful|awesome|best|spotify|check out)/;
function cleanTags(list, artist) {
  return arr(list).filter(t => (+t.count || 0) >= 10).map(t => t.name.trim())
    .filter(n => n && !TAG_SKIP.test(n.toLowerCase()) && !/^\d{4}$/.test(n) && normKey(n) !== normKey(artist))
    .slice(0, 3).map(n => n.replace(/(^|[\s-])(\p{L})/gu, (m, a, b) => a + b.toUpperCase()));
}
let tagsRunning = false;
// force = the "Update" button: asks Last.fm again (no cache) and always says what happened
async function fetchArtistTags(force = false) {
  if (!lfKey()) { if (force) toast('Add your Last.fm API key first (Settings → Scrobbling)', 4000); return; }
  if (tagsRunning) { if (force) toast('Already getting genres from Last.fm (bottom right)'); return; }
  const have = S.prefs.lfTags || (S.prefs.lfTags = {});
  const todo = S.artists.filter(ar => ar.tracks.some(i => !(S.tracks[i].genres || []).length) && (force || !(norm(ar.name) in have)));
  if (!todo.length) {
    if (force) toast('All your songs already have a genre in their tags: Last.fm tags are only used for songs without one', 5000);
    return;
  }
  tagsRunning = true;
  const task = (force || todo.length > 5) && !Task.cur ? startTask('Getting genres from Last.fm') : null;
  let found = 0, failed = 0;
  for (const [n, ar] of todo.entries()) {
    if (task?.cancelled) break;
    const v = await lf('artist.getTopTags', { artist: ar.name, autocorrect: '1' }, force ? 0 : 60);
    if (!v) failed++;
    const tags = cleanTags(v?.toptags?.tag, ar.name);
    if (v) have[norm(ar.name)] = tags;
    if (tags.length) found++;
    task?.update(n + 1, todo.length, `${found} with genres`);
    if (n % 25 === 24) savePrefs();
    await sleepMs(150);
  }
  tagsRunning = false;
  savePrefs(); buildIndex();
  if (!['settings', 'eq'].includes(route.view)) rerender();
  const msg = `Genres from Last.fm: ${found} of ${plural(todo.length, 'artist', 'artists')} got genres` + (failed ? ` · ${failed} couldn't be checked (connection or API key)` : '');
  if (task) task.finish(msg, 6000); else if (force) toast(msg, 6000);
}

/* ───────────── Ratings ───────────── */
const ratingOf = p => S.prefs.ratings?.[p] || 0;
const starsHtml = p => { const r = ratingOf(p); return `<div class="c rt${r ? ' has' : ''}">${[1, 2, 3, 4, 5].map(n => `<span data-star="${n}" class="${n <= r ? 'on' : ''}">${ic('star', 11)}</span>`).join('')}</div>`; };
function setRating(idxs, n) {
  const r = S.prefs.ratings || (S.prefs.ratings = {});
  for (const i of idxs) { const p = S.tracks[i]?.path; if (!p) continue; if (n) r[p] = n; else delete r[p]; }
  savePrefs(); paintRows(true);
}
view.addEventListener('click', e => {
  const st = e.target.closest('[data-star]'); if (!st) return;
  const tr = st.closest('.tr'); const i = +tr.dataset.t, n = +st.dataset.star;
  setRating([i], ratingOf(S.tracks[i].path) === n ? 0 : n);   // the same star again clears it
});

/* ───────────── Sleep timer ───────────── */
const bSleep = $('#b-sleep');
function sleepItems() {
  const on = Sleep.at || Sleep.endOfTrack;
  return [
    ...[15, 30, 45, 60, 90].map(m => ({ label: `${m} minutes`, act: () => setSleep(m) })),
    { label: 'End of this song', act: () => setSleep('track') },
    ...(on ? ['-', { label: 'Turn off the timer', act: () => setSleep(0) }] : []),
  ];
}
function setSleep(v) {
  clearInterval(Sleep.tick);
  Sleep.at = 0; Sleep.endOfTrack = false;
  if (v === 'track') { Sleep.endOfTrack = true; toast('Wax will stop at the end of this song'); }
  else if (v) { Sleep.at = Date.now() + v * 60000; Sleep.tick = setInterval(sleepTick, 1000); toast(`Wax will stop in ${v} minutes`); }
  else toast('Sleep timer off');
  syncNext(); sleepButton();
}
function sleepTick() {
  const left = Sleep.at - Date.now();
  if (left <= 10000 && !audio.paused) audio.setVolume(Math.max(0, left / 10000) * (S.prefs.muted ? 0 : S.prefs.volume ** 2));   // fade out
  if (left <= 0) { if (!audio.paused) audio.pause(); sleepDone(); }
  sleepButton();
}
function sleepDone() {
  clearInterval(Sleep.tick);
  Sleep.at = 0; Sleep.endOfTrack = false;
  audio.setVolume(S.prefs.muted ? 0 : S.prefs.volume ** 2);
  sleepButton(); syncNext();
}
function sleepButton() {
  const on = !!(Sleep.at || Sleep.endOfTrack);
  bSleep.classList.toggle('on', on);
  const min = Sleep.at ? Math.max(1, Math.ceil((Sleep.at - Date.now()) / 60000)) : 0;
  bSleep.innerHTML = ic('moon') + (min ? `<span class="badge">${min}</span>` : Sleep.endOfTrack ? '<span class="badge">♪</span>' : '');
  bSleep.title = Sleep.at ? `Stops in ${min} min` : Sleep.endOfTrack ? 'Stops at the end of this song' : 'Sleep timer';
}
bSleep.onclick = e => {
  e.stopPropagation();
  const r = bSleep.getBoundingClientRect();
  openMenu(r.left, 0, sleepItems());
  menu.style.top = Math.max(4, r.top - menu.offsetHeight - 6) + 'px';   // above the player bar
};

/* ───────────── Discord ───────────── */
let discordT = 0, discordWas = false;
function discordSync() {
  clearTimeout(discordT);
  discordT = setTimeout(() => {
    const id = (S.prefs.discordId || '').trim();
    if (!S.prefs.discord || !id) {
      if (discordWas) { invoke('discord_update', { clientId: '', activity: null }).catch(() => {}); discordWas = false; }
      return;
    }
    const t = S.tracks[P.current];
    let activity = null;
    if (t && !audio.paused) {
      const start = Date.now() - Math.round((audio.currentTime || 0) * 1000);
      activity = { title: t.title, artist: t.artist, album: t.album, start, end: start + Math.round((t.duration || 0) * 1000) };
    }
    discordWas = true;
    invoke('discord_update', { clientId: id, activity }).catch(() => {});
  }, 700);
}
audio.addEventListener('seeked', discordSync);

/* ───────────── Folder view ───────────── */
VIEWS.folders = path => {
  const roots = S.prefs.folders;
  if (!path && roots.length === 1) path = roots[0].replace(/[\\/]+$/, '');
  if (!path) {
    return `<div class="page-h"><h1>Folders</h1></div>${roots.length ? `<div class="flist">${roots.map(r => folderRow(r, r, S.tracks.filter(t => t.path.toLowerCase().startsWith(r.toLowerCase())).length, null)).join('')}</div>` : emptyLibrary()}`;
  }
  const lp = path.toLowerCase() + '\\';
  const subs = new Map(), here = [], all = [];
  S.tracks.forEach((t, i) => {
    if (!t.path.toLowerCase().startsWith(lp)) return;
    all.push(i);
    const rest = t.path.slice(lp.length), k = rest.search(/[\\/]/);
    if (k < 0) { here.push(i); return; }
    const name = rest.slice(0, k);
    const e = subs.get(name) || { name, n: 0, first: i };
    e.n++; subs.set(name, e);
  });
  const root = roots.map(r => r.replace(/[\\/]+$/, '')).filter(r => path.toLowerCase().startsWith(r.toLowerCase())).sort((a, b) => b.length - a.length)[0] || path;
  const crumbs = [[root, root.split(/[\\/]/).pop() || root]];
  let acc = root;
  for (const seg of path.slice(root.length).split(/[\\/]/).filter(Boolean)) { acc += '\\' + seg; crumbs.push([acc, seg]); }
  lists.set('folder-all', all.sort((a, b) => cmp(S.tracks[a].path, S.tracks[b].path)));
  const T = S.tracks;
  const sortedHere = here.sort((a, b) => ((T[a].discNo || 0) - (T[b].discNo || 0)) || ((T[a].trackNo || 0) - (T[b].trackNo || 0)) || cmp(T[a].path, T[b].path));
  const folders = [...subs.values()].sort((a, b) => cmp(a.name, b.name));
  return `<div class="page-h"><div class="crumbs">${roots.length > 1 ? `<span class="lnk" data-go="folders">Folders</span><span class="sep">›</span>` : ''}${crumbs.map(([p, n], k) => k === crumbs.length - 1 ? `<b>${esc(n)}</b>` : `<span class="lnk" data-go="folders" data-arg="${esc(p)}">${esc(n)}</span><span class="sep">›</span>`).join('')}</div>
    <div class="row">${all.length ? playBtn('folder-all', 'Play folder') : ''}<span class="sub">${plural(folders.length, 'folder', 'folders')} · ${plural(all.length, 'song', 'songs')}</span><span class="grow"></span>
      <button class="btn btn-ghost btn-sm" data-action="reveal-folder" data-arg="${esc(path)}">Show in Explorer</button></div></div>
    ${folders.length ? `<div class="flist">${folders.map(f => folderRow(path + '\\' + f.name, f.name, f.n, albumKeyOf(T[f.first]))).join('')}</div>` : ''}
    ${sortedHere.length ? `${folders.length ? secHead('Songs in this folder') : ''}${trackTable('folder', sortedHere, 'songs')}` : ''}`;
};
const folderRow = (path, name, n, key) => `<div class="frow" data-go="folders" data-arg="${esc(path)}" data-folder-row="${esc(path)}">
  ${key ? cvHtml(key, 'fcv', 18) : `<span class="fic">${ic('folder', 20)}</span>`}<span class="fn">${esc(name)}</span><span class="m">${plural(n, 'song', 'songs')}</span></div>`;
const underFolder = path => { const lp = path.toLowerCase() + '\\'; return S.tracks.map((t, i) => [t, i]).filter(([t]) => t.path.toLowerCase().startsWith(lp)).sort((a, b) => cmp(a[0].path, b[0].path)).map(x => x[1]); };
view.addEventListener('contextmenu', e => {
  const f = e.target.closest('[data-folder-row]'); if (!f) return;
  const path = f.dataset.folderRow;
  openMenu(e.clientX, e.clientY, [
    { label: 'Play', act: () => playList(underFolder(path), 0, { view: 'folders', arg: path }) },
    { label: 'Add to queue', act: () => { P.upNext.push(...underFolder(path)); toast('Folder added to the queue'); syncNext(); } },
    { label: 'Add to playlist', sub: playlistSubmenu(underFolder(path)) },
    '-',
    { label: 'Show in Explorer', act: () => invoke('reveal', { path }) },
  ]);
});

/* ───────────── Duplicates ───────────── */
const LOSSLESS = /\.(flac|wav|aiff?|alac)$/i;
const kbps = t => t.duration ? Math.round(t.size * 8 / t.duration / 1000) : 0;
const quality = t => (LOSSLESS.test(t.path) ? 100000 : 0) + kbps(t);
const extOf = p => (p.match(/\.([^.\\/]+)$/)?.[1] || '').toUpperCase();
function findDuplicates() {
  const groups = new Map();
  S.tracks.forEach((t, i) => {
    if (t.cue) return;
    const k = normKey(t._owner) + '|' + normTitle(t.title);
    if (!groups.has(k)) groups.set(k, []);
    groups.get(k).push(i);
  });
  const out = [];
  for (const g of groups.values()) {
    if (g.length < 2) continue;
    g.sort((a, b) => S.tracks[a].duration - S.tracks[b].duration);
    let run = [g[0]];
    const flush = () => { if (run.length > 1) out.push(run.sort((a, b) => quality(S.tracks[b]) - quality(S.tracks[a]))); };
    for (const i of g.slice(1)) {
      if (Math.abs(S.tracks[i].duration - S.tracks[run[run.length - 1]].duration) <= 2.5) run.push(i);
      else { flush(); run = [i]; }
    }
    flush();
  }
  return out.sort((a, b) => cmp(S.tracks[a[0]]._owner, S.tracks[b[0]]._owner) || cmp(S.tracks[a[0]].title, S.tracks[b[0]].title));
}
VIEWS.duplicates = () => {
  const groups = findDuplicates();
  S.dupGroups = groups;
  const extra = groups.reduce((s, g) => s + g.length - 1, 0);
  const short = p => { const r = rootOf(p); return r ? p.slice(r.length + 1) : p; };
  return `<div class="page-h"><h1>Duplicates</h1><div class="row"><span class="sub">${groups.length ? `${plural(groups.length, 'song', 'songs')} with more than one copy · ${plural(extra, 'extra file', 'extra files')}` : ''}</span><span class="grow"></span>
    ${groups.length ? `<button class="btn btn-ghost" data-action="dup-keep-all">Keep the best copy of each…</button>` : ''}</div></div>
    ${groups.length ? `<p class="sub">Same artist, same title and the same length (±2 s). The best copy (lossless, or the highest bitrate) is first. Files go to the Recycle Bin, and playlists, favorites and ratings move to the copy you keep.</p>
    ${groups.map((g, gi) => `<div class="dup">
      <div class="dup-h"><b>${esc(S.tracks[g[0]].title)}</b> <span class="sub">— ${esc(S.tracks[g[0]].artist)}</span><span class="grow"></span><button class="btn btn-ghost btn-sm" data-action="dup-keep" data-arg="${gi}">Keep the best</button></div>
      ${g.map((i, k) => { const t = S.tracks[i]; return `<div class="dup-r">
        <span class="q ${k === 0 ? 'best' : ''}">${k === 0 ? 'BEST' : ''}</span><span class="fmt">${extOf(t.path)}</span><span class="kb">${kbps(t)} kbps</span><span class="mb">${(t.size / 1048576).toFixed(1)} MB</span>
        <span class="p" title="${esc(t.path)}"><span class="lnk" data-go="album" data-arg="${esc(albumKeyOf(t))}">${esc(t.album)}</span> · ${esc(short(t.path))}</span>
        <button class="ib" title="Play" data-action="dup-play" data-arg="${i}">${ic('play', 14)}</button>
        <button class="ib" title="Show in Explorer" data-action="dup-reveal" data-arg="${i}">${ic('folder', 14)}</button>
        <button class="ib" title="Move to the Recycle Bin" data-action="dup-trash" data-arg="${i}">${ic('x', 14)}</button></div>`; }).join('')}
    </div>`).join('')}`
    : `<div class="empty"><div class="big-ic">${ic('note', 80)}</div><h2>No duplicates</h2><p>Every song in your library appears only once.</p></div>`}`;
};
// Recycles files; `keep` maps each removed path to the copy that stays
async function recycle(paths, keep = new Map()) {
  const cur = S.tracks[P.current]?.path;
  if (paths.includes(cur)) { toast("The song that's playing can't be removed now"); paths = paths.filter(p => p !== cur); }
  if (!paths.length) return;
  const errors = await invoke('trash_files', { paths }).catch(e => [String(e)]);
  const gone = new Set(paths);
  const fix = p => keep.get(p) || p;
  const pr = S.prefs;
  for (const pl of pr.playlists) pl.tracks = [...new Set(pl.tracks.map(fix))].filter(p => !gone.has(p) || keep.has(p));
  pr.favorites = [...new Set((pr.favorites || []).map(fix))]; S.fav = new Set(pr.favorites);
  for (const [k, v] of Object.entries(pr.plays)) if (keep.has(k)) { pr.plays[keep.get(k)] = (pr.plays[keep.get(k)] || 0) + v; delete pr.plays[k]; }
  for (const [k, v] of Object.entries(pr.ratings || {})) if (keep.has(k)) { pr.ratings[keep.get(k)] ??= v; delete pr.ratings[k]; }
  savePrefs();
  await rescan({ quiet: true });
  toast(errors.length ? `${errors.length} couldn't be removed: ${errors[0]}` : plural(paths.length - errors.length, 'file moved to the Recycle Bin', 'files moved to the Recycle Bin'), 4000);
}
async function dupKeep(groups) {
  const keep = new Map();
  for (const g of groups) for (const i of g.slice(1)) keep.set(S.tracks[i].path, S.tracks[g[0]].path);
  if (!keep.size) return;
  if (!await confirmBox('Remove the extra copies?', `${plural(keep.size, 'file goes', 'files go')} to the Recycle Bin. You can restore them from there.`, 'Move to Recycle Bin')) return;
  recycle([...keep.keys()], keep);
}

/* ───────────── Library check ───────────── */
const H = { src: null, covers: null, lyrics: null, running: false };
VIEWS.health = () => {
  setTimeout(healthScan);
  return `<div class="page-h"><h1>Library check</h1><div class="row"><span class="sub">What's missing in your files, and how to fix it.</span><span class="grow"></span>
    <button class="btn btn-ghost btn-sm" data-action="health-refresh">${ic('refresh', 14)} Check again</button></div></div>
    <div id="health-body">${healthHtml()}</div>`;
};
const albumsWhere = f => S.albums.filter(a => a.tracks.some(i => f(S.tracks[i])));
function healthHtml() {
  const real = S.albums.filter(a => !S.tracks[a.tracks[0]].cue);
  const noYear = albumsWhere(t => !t.year && !t.cue), noNum = albumsWhere(t => !t.trackNo && !t.cue), noGenre = albumsWhere(t => !(t.genres || []).length && !t.cue);
  const noRg = S.tracks.filter(t => !t.rg && !t.cue), analyzed = noRg.filter(t => S.loud[t.path]);
  const spin = '<span class="spin"></span>';
  const covers = H.covers, lyr = H.lyrics;
  const noCover = covers ? real.filter(a => covers.get(a.key) && !covers.get(a.key).any) : null;
  const notEmb = covers ? real.filter(a => covers.get(a.key)?.any && !covers.get(a.key).embedded) : null;
  const noLyr = lyr ? S.tracks.map((t, i) => i).filter(i => lyr.get(S.tracks[i].path) === false) : null;
  const sec = (title, n, desc, action, list) => `<div class="hsec${n === 0 ? ' ok' : ''}"><div class="hs-h"><span class="hn">${n === null ? spin : n === 0 ? '✓' : n}</span>
    <div class="grow"><div class="ht">${title}</div><div class="sub">${n === 0 ? 'All good.' : desc}</div></div>${n ? action : ''}</div>${n ? list || '' : ''}</div>`;
  const albumList = (id, albums, btn) => {
    const all = H.open === id, shown = all ? albums : albums.slice(0, 6);
    return `<div class="hlist">${shown.map(a => `<div class="hrow">${cvHtml(a.key, 'hcv', 16)}<div class="grow"><span class="lnk" data-go="album" data-arg="${esc(a.key)}">${esc(a.title)}</span><div class="sub">${esc(a.artist)}</div></div>${btn ? btn(a) : ''}</div>`).join('')}
      ${albums.length > 6 ? `<a class="more-toggle" data-action="health-more" data-arg="${all ? '' : id}">${all ? 'Show less' : `Show all ${albums.length}`}</a>` : ''}</div>`;
  };
  const mb = a => `<button class="btn btn-ghost btn-sm" data-action="health-mb" data-arg="${esc(a.key)}">MusicBrainz…</button>`;
  return [
    sec('Albums without a cover', noCover ? noCover.length : null, 'No image inside the files or in the folder.', `<button class="btn btn-green btn-sm" data-action="health-covers-online">Find online and embed</button>`, noCover && albumList('nocover', noCover)),
    sec("Covers that aren't inside the files", notEmb ? notEmb.length : null, "The cover is only an image in the folder: other players and phones won't show it.", `<button class="btn btn-green btn-sm" data-action="health-covers-embed">Embed covers</button>`, notEmb && albumList('notemb', notEmb)),
    sec('Songs without lyrics', noLyr ? noLyr.length : null, 'No .lrc next to the file and no lyrics in the tags.', `<button class="btn btn-green btn-sm" data-action="health-lyrics">Download lyrics…</button>`),
    sec('Albums without a year', noYear.length, 'Some of their songs have no year.', '', albumList('noyear', noYear, mb)),
    sec('Albums without track numbers', noNum.length, 'They play in the wrong order.', '', albumList('nonum', noNum, mb)),
    sec('Albums without a genre', noGenre.length, 'They don\'t show up in Genres. MusicBrainz or Last.fm tags (Settings) can fill them in.', '', albumList('nogenre', noGenre, mb)),
    sec('Songs without ReplayGain tags', noRg.length, `${analyzed.length} of them were analyzed by Wax and can get tags now.`,
      `${analyzed.length ? `<button class="btn btn-green btn-sm" data-action="rg-write">Write tags…</button>` : ''} <button class="btn btn-ghost btn-sm" data-go="settings">Analyze…</button>`),
  ].join('');
}
async function healthScan(force = false) {
  if (H.running) return;
  if (force || H.src !== S.tracks) { H.covers = null; H.lyrics = null; H.src = S.tracks; }
  const paint = () => { const b = $('#health-body'); if (b && route.view === 'health') { b.innerHTML = healthHtml(); hydrate(b); } };
  H.running = true;
  try {
    if (!H.covers) {
      const albums = S.albums.filter(a => !S.tracks[a.tracks[0]].cue);
      const st = await invoke('covers_status', { albums: albums.map(a => ({ path: realPath(S.tracks[a.tracks[0]]), shared: a.shared ? a.title : null })) }).catch(() => []);
      H.covers = new Map(albums.map((a, k) => [a.key, st[k] || { embedded: false, any: false }]));
      paint();
    }
    if (!H.lyrics) {
      const ts = S.tracks.filter(t => !t.cue);
      const st = await invoke('lyrics_status', { paths: ts.map(t => t.path) }).catch(() => []);
      H.lyrics = new Map(ts.map((t, k) => [t.path, !!st[k]]));
      paint();
    }
  } finally { H.running = false; }
}

/* ───────────── Batch work: lyrics, covers, ReplayGain ───────────── */
async function batchLyrics(idxs) {
  const ts = idxs.map(i => S.tracks[i]).filter(t => t && !t.cue);
  if (!ts.length) { toast("Songs inside a CUE sheet can't have their own lyrics files"); return; }
  if (busy()) return;
  const def = S.prefs.batchLyrics || (S.prefs.embedLyrics && !S.prefs.saveLrc ? 'embed' : 'lrc');
  const d = dialog(`<h2>Download lyrics</h2><p class="sub">${plural(ts.length, 'song', 'songs')}. Songs that already have lyrics are skipped. Lyrics come from LRCLIB.</p>
    <div class="radios">${[['lrc', 'Save as .lrc next to each song', 'Like LRCGET. Works with every player.'], ['embed', 'Embed in the files', 'Inside the tags.'], ['both', 'Both', '']].map(([v, l, s]) => `<label class="rad"><input type="radio" name="lym" value="${v}" ${v === def ? 'checked' : ''}><span><b>${l}</b>${s ? `<br><span class="sub">${s}</span>` : ''}</span></label>`).join('')}</div>
    <div class="btns"><span class="grow"></span><button class="btn btn-ghost" data-d="cancel">Cancel</button><button class="btn btn-green" data-d="go">Download</button></div>`);
  const mode = await new Promise(res => {
    modalBg.onclick = e => { const b = e.target.closest('[data-d]')?.dataset.d; if (b === 'cancel' || e.target === modalBg) { closeDialog(); res(null); } if (b === 'go') { const v = $('input[name="lym"]:checked', d).value; closeDialog(); res(v); } };
  });
  if (!mode) return;
  S.prefs.batchLyrics = mode; savePrefs();
  const lrc = mode !== 'embed', embed = mode !== 'lrc';
  const task = startTask('Downloading lyrics');
  let saved = 0, had = 0, none = 0, errors = 0;
  for (const [n, t] of ts.entries()) {
    if (task.cancelled) break;
    task.update(n, ts.length, `${saved} saved`);
    const r = await invoke('get_lyrics', { path: t.path, title: t.title, artist: t.artist, album: t.album, duration: t.duration || 0, online: true, force: false }).catch(e => ({ error: String(e) }));
    if (r.source === 'Local .lrc file' || r.source === 'File tags') { had++; continue; }
    const raw = r.synced || r.plain;
    if (!raw) { r.error ? errors++ : none++; continue; }
    const isCur = S.tracks[P.current]?.path === t.path;
    try {
      if (embed && isCur) { S.prefs.pendingEmbeds[t.path] = raw; savePrefs(); }
      await invoke('save_lyrics', { path: t.path, text: raw, lrc, embed: embed && !isCur });
      saved++;
    } catch { errors++; }
  }
  H.lyrics = null;
  task.finish([`${saved} saved`, had && `${had} already had lyrics`, none && `${none} not on LRCLIB`, errors && `${errors} errors`].filter(Boolean).join(' · '), 8000);
}

async function embedCovers(albums, online) {
  albums = albums.filter(a => !S.tracks[a.tracks[0]].cue);
  if (!albums.length || busy()) return;
  const jobs = albums.map(a => ({ paths: a.tracks.map(i => S.tracks[i].path), artist: a.albumArtist || a.artist, album: a.title, shared: !!a.shared, online }));
  const paths = jobs.flatMap(j => j.paths);
  const task = startTask('Embedding covers'); task.kind = 'covers';
  const r = await withReleased(paths, () => invoke('embed_covers', { jobs })).catch(e => ({ tracks: [], errors: [String(e)] }));
  applyTracks(r.tracks);
  H.covers = null;
  coverP.clear();
  if (['health', 'album'].includes(route.view)) rerender();
  const ok = new Set(r.tracks.map(t => t.path)).size;
  task.finish(`Cover embedded in ${plural(ok, 'file', 'files')}${r.errors.length ? ` · ${r.errors.length} problems (${r.errors[0]})` : ''}`, 8000);
}

async function writeReplayGain() {
  if (busy()) return;
  const r2 = x => Math.round(x * 100) / 100;
  const items = S.tracks.filter(t => !t.cue && !t.rg && S.loud[t.path]).map(t => {
    const l = S.loud[t.path], key = albumKeyOf(t), a = S.albumMap.get(key);
    const full = a && a.tracks.every(i => S.loud[S.tracks[i].path]);
    const al = full ? albumLufs(key) : null;
    return { path: t.path, trackGain: r2(-18 - l.lufs), trackPeak: l.peak, albumGain: al ? r2(-18 - al.lufs) : null, albumPeak: al ? al.peak : null };
  });
  if (!items.length) { toast('Nothing to write: analyze the library first (Settings → Playback)', 4000); return; }
  if (!await confirmBox('Write ReplayGain tags?', `${plural(items.length, 'song', 'songs')} analyzed by Wax will get ReplayGain tags (−18 LUFS reference). The audio isn't changed.`, 'Write tags')) return;
  const task = startTask('Writing ReplayGain tags'); task.kind = 'rg';
  const r = await withReleased(items.map(x => x.path), () => invoke('write_replaygain', { items })).catch(e => ({ tracks: [], errors: [String(e)] }));
  applyTracks(r.tracks);
  if (['settings', 'health'].includes(route.view)) rerender();
  task.finish(`${plural(r.tracks.length, 'song tagged', 'songs tagged')}${r.errors.length ? ` · ${r.errors.length} errors (${r.errors[0]})` : ''}`, 6000);
}

/* ───────────── Copy to a folder / USB stick ───────────── */
async function copyToFolder(idxs, name) {
  const ts = idxs.map(i => S.tracks[i]).filter(t => t && !t.cue);
  const skipped = idxs.length - ts.length;
  if (!ts.length) { toast("Songs inside a CUE sheet can't be copied one by one"); return; }
  if (busy()) return;
  let dest = S.prefs.copyDest || '';
  const draw = () => dialog(`<h2>Copy to a folder</h2><p class="sub">${plural(ts.length, 'song', 'songs')} from "${esc(name)}"${skipped ? ` (${skipped} inside CUE sheets are skipped)` : ''}. For a USB stick, the car or a phone. Files already there are skipped.</p>
    <div class="form one"><label><span>Destination</span><div class="pick"><input id="cp-dest" value="${esc(dest)}" placeholder="Choose a folder…" spellcheck="false"><button class="btn btn-ghost btn-sm" data-d="pick">Choose…</button></div></label></div>
    <div class="radios">${[['tree', 'Artist \\ Album \\ song', ''], ['flat', 'All in one folder', 'Numbered in playlist order: 01 - Artist - Title']].map(([v, l, s]) => `<label class="rad"><input type="radio" name="cpl" value="${v}" ${v === (S.prefs.copyLayout || 'tree') ? 'checked' : ''}><span><b>${l}</b>${s ? `<br><span class="sub">${s}</span>` : ''}</span></label>`).join('')}</div>
    <div class="pat-pick"><span>Format</span><select class="sel" id="cp-fmt">${[['', 'Keep the original files'], ['320', 'Convert to MP3 · 320 kbps'], ['256', 'Convert to MP3 · 256 kbps'], ['192', 'Convert to MP3 · 192 kbps'], ['128', 'Convert to MP3 · 128 kbps']].map(([v, l]) => `<option value="${v}" ${v === (S.prefs.copyMp3 || '') ? 'selected' : ''}>${l}</option>`).join('')}</select>
      <label class="chk inline"><input type="checkbox" id="cp-lossless" ${S.prefs.copyOnlyLossless !== false ? 'checked' : ''}> Only lossless files (FLAC, WAV, AIFF)</label></div>
    <label class="chk"><input type="checkbox" id="cp-m3u" ${S.prefs.copyM3u !== false ? 'checked' : ''}> Also write a playlist file (.m3u8) with the songs in order</label>
    <div class="btns"><span class="grow"></span><button class="btn btn-ghost" data-d="cancel">Cancel</button><button class="btn btn-green" data-d="go">Copy</button></div>`);
  let d = draw();
  modalBg.onclick = async e => {
    const b = e.target.closest('[data-d]')?.dataset.d;
    if (b === 'cancel' || e.target === modalBg) { closeDialog(); return; }
    if (b === 'pick') { const p = await invoke('pick_dir', { title: 'Copy to' }); if (p) { dest = p; $('#cp-dest', d).value = p; } return; }
    if (b !== 'go') return;
    dest = $('#cp-dest', d).value.trim().replace(/[\\/]+$/, '');
    if (!dest) { toast('Choose a destination folder'); return; }
    const layout = $('input[name="cpl"]:checked', d).value, m3u = $('#cp-m3u', d).checked;
    const mp3 = +$('#cp-fmt', d).value || 0, onlyLossless = $('#cp-lossless', d).checked;
    S.prefs.copyDest = dest; S.prefs.copyLayout = layout; S.prefs.copyM3u = m3u; S.prefs.copyMp3 = mp3 ? String(mp3) : ''; S.prefs.copyOnlyLossless = onlyLossless; savePrefs();
    // Which files get converted: not MP3s, and only lossless ones if asked
    const convert = t => mp3 && !/\.mp3$/i.test(t.path) && (!onlyLossless || LOSSLESS.test(t.path));
    closeDialog();
    const ext = t => convert(t) ? '.mp3' : (t.path.match(/\.[^.\\/]+$/)?.[0] || '');
    const seen = new Set(), items = [], rels = [];
    ts.forEach((t, n) => {
      const file = t.path.split(/[\\/]/).pop().replace(/\.[^.]+$/, '') + ext(t);
      let rel = layout === 'flat'
        ? `${String(n + 1).padStart(ts.length > 99 ? 3 : 2, '0')} - ${safeName(t.artist)} - ${safeName(t.title)}${ext(t)}`
        : `${safeName(t._owner || t.artist)}\\${safeName(t.album)}\\${file}`;
      if (seen.has(rel.toLowerCase())) return;
      seen.add(rel.toLowerCase()); rels.push([t, rel]);
      items.push({ from: t.path, to: `${dest}\\${rel}`, mp3: convert(t) ? mp3 : null });
    });
    const list = m3u ? [`${dest}\\${safeName(name)}.m3u8`, '#EXTM3U\n' + rels.map(([t, rel]) => `#EXTINF:${Math.round(t.duration || 0)},${t.artist} - ${t.title}\n${rel}`).join('\n') + '\n'] : null;
    const task = startTask(`${items.some(i => i.mp3) ? 'Converting and copying' : 'Copying'} to ${dest}`); task.kind = 'copy';
    const r = await invoke('copy_files', { items, m3u: list }).catch(e => ({ copied: 0, skipped: 0, errors: [String(e)] }));
    task.finish(`${plural(r.copied, 'file copied', 'files copied')}${r.skipped ? ` · ${r.skipped} were already there` : ''}${r.errors.length ? ` · ${r.errors.length} errors (${r.errors[0]})` : ''}`, 8000);
  };
}

/* ───────────── Playlist folders ───────────── */
async function newPlaylistFolder(moveId) {
  const name = await modal({ title: 'New folder', value: 'New folder', ok: 'Create', placeholder: 'Folder name' });
  if (!name) return;
  const f = { id: 'f' + Date.now().toString(36), name, open: true };
  (S.prefs.plFolders ||= []).push(f);
  if (moveId) { const pl = S.prefs.playlists.find(p => p.id === moveId); if (pl) pl.folder = f.id; }
  savePrefs(); renderSidebar();
}
function movePlaylist(id, folder) {
  const pl = S.prefs.playlists.find(p => p.id === id); if (!pl) return;
  if (folder) pl.folder = folder; else delete pl.folder;
  const f = (S.prefs.plFolders || []).find(x => x.id === folder); if (f) f.open = true;
  savePrefs(); renderSidebar();
}
const folderPlaylists = id => S.prefs.playlists.filter(p => p.folder === id);
const folderTracks = id => folderPlaylists(id).flatMap(p => p.tracks).map(p => S.byPath.get(p)).filter(i => i !== undefined);
function folderMenu(x, y, id) {
  const f = (S.prefs.plFolders || []).find(x => x.id === id); if (!f) return;
  openMenu(x, y, [
    { label: 'Play', act: () => playList(folderTracks(id), 0, { view: 'playlist' }), disabled: !folderTracks(id).length },
    { label: 'Add to queue', act: () => { P.upNext.push(...folderTracks(id)); toast('Folder added to the queue'); syncNext(); } },
    '-',
    { label: 'Rename', act: async () => { const n = await modal({ title: 'Rename folder', value: f.name, ok: 'Save' }); if (n) { f.name = n; savePrefs(); renderSidebar(); } } },
    { label: 'Delete folder', act: async () => {
      if (!await confirmBox('Delete folder?', `"${f.name}" is removed; its playlists stay, outside the folder.`, 'Delete')) return;
      for (const p of folderPlaylists(id)) delete p.folder;
      S.prefs.plFolders = S.prefs.plFolders.filter(x => x.id !== id); savePrefs(); renderSidebar();
    } },
  ]);
}
const sbPlBox = $('#sb-playlists');
sbPlBox.addEventListener('click', e => {
  const t = e.target.closest('[data-plf-toggle]'); if (!t) return;
  const f = (S.prefs.plFolders || []).find(x => x.id === t.dataset.plfToggle); if (!f) return;
  f.open = f.open === false; savePrefs(); renderSidebar();
});

/* ───────────── Drag & drop ─────────────
   Songs: onto a playlist in the sidebar (app.js), onto the queue button, and to reorder a
   playlist or the queue. Playlists: into folders and between each other. */
let dragSrc = null, dragPl = null, dragFolder = null;
view.addEventListener('dragstart', e => {
  if (!e.target.closest('.tr')) return;
  dragSrc = { list: sel.list, pos: [...sel.set].sort((a, b) => a - b) };
  e.dataTransfer.effectAllowed = 'copyMove';
});
view.addEventListener('dragend', () => { dragSrc = null; $$('.drop-line', view).forEach(x => x.remove()); $('#b-queue').classList.remove('drop'); });
// Lists that can be reordered: a playlist (without a filter) and "Next in queue"
function dropTarget(e) {
  const tl = e.target.closest('.tl'); if (!tl || !dragIdxs.length) return null;
  const list = tl.dataset.list;
  if (list === 'playlist' && route.view === 'playlist' && !$('[data-filter="w-pl"]')?.value) return { tl, list };
  if (list === 'q-next') return { tl, list };
  return null;
}
const dropPos = (tl, e) => {
  const tb = tl.lastElementChild, n = lists.get(tl.dataset.list)?.length || 0;
  return Math.max(0, Math.min(n, Math.round((e.clientY - tb.getBoundingClientRect().top) / ROW)));
};
view.addEventListener('dragover', e => {
  const t = dropTarget(e); if (!t) return;
  e.preventDefault();
  e.dataTransfer.dropEffect = dragSrc?.list === t.list ? 'move' : 'copy';
  const pos = dropPos(t.tl, e), tb = t.tl.lastElementChild;
  let line = $('.drop-line', tb);
  if (!line) { line = document.createElement('div'); line.className = 'drop-line'; tb.appendChild(line); }
  line.style.top = (pos * ROW - 1) + 'px';
});
view.addEventListener('dragleave', e => { if (!e.relatedTarget || !view.contains(e.relatedTarget)) $$('.drop-line', view).forEach(x => x.remove()); });
view.addEventListener('drop', e => {
  const t = dropTarget(e); if (!t) return;
  e.preventDefault();
  const pos = dropPos(t.tl, e);
  $$('.drop-line', view).forEach(x => x.remove());
  const same = dragSrc?.list === t.list;
  if (t.list === 'q-next') {
    P.upNext = reorder(P.upNext, same ? dragSrc.pos : null, dragIdxs, pos);
    rerender(); syncNext();
  } else {
    const pl = S.prefs.playlists.find(p => p.id === route.arg); if (!pl) return;
    // visible position → position in pl.tracks (songs no longer in the library are hidden)
    const vis = pl.tracks.map((p, k) => [p, k]).filter(([p]) => S.byPath.has(p)).map(x => x[1]);
    const at = pos < vis.length ? vis[pos] : pl.tracks.length;
    pl.tracks = reorder(pl.tracks, same ? dragSrc.pos.map(k => vis[k]) : null, dragIdxs.map(i => S.tracks[i].path), at);
    savePrefs(); rerender();
  }
  dragIdxs = [];
});
// Moves the items at `from` (or inserts `items` when they come from another list) before `at`
function reorder(arr, from, items, at) {
  if (!from) { const out = arr.slice(); out.splice(at, 0, ...items); return out; }
  const set = new Set(from), moving = from.map(k => arr[k]);
  const before = arr.slice(0, at).filter((_, k) => !set.has(k)).length;
  const rest = arr.filter((_, k) => !set.has(k));
  rest.splice(before, 0, ...moving);
  return rest;
}
// Songs onto the queue button
const bQueue = $('#b-queue');
bQueue.addEventListener('dragover', e => { if (!dragIdxs.length) return; e.preventDefault(); bQueue.classList.add('drop'); });
bQueue.addEventListener('dragleave', () => bQueue.classList.remove('drop'));
bQueue.addEventListener('drop', e => {
  e.preventDefault(); bQueue.classList.remove('drop');
  if (!dragIdxs.length) return;
  P.upNext.push(...dragIdxs); toast(dragIdxs.length > 1 ? `${dragIdxs.length} songs added to the queue` : 'Added to the queue');
  dragIdxs = []; refreshQueueView(); syncNext();
});
// Playlists and playlist folders in the sidebar
sbPlBox.addEventListener('dragstart', e => {
  const p = e.target.closest('[data-pl]'), f = e.target.closest('[data-plf-toggle]');
  if (!p && !f) return;
  dragPl = p ? p.dataset.pl : null; dragFolder = f ? f.dataset.plfToggle : null;
  e.dataTransfer.setData('text/plain', 'wax-pl'); e.dataTransfer.effectAllowed = 'move';
});
sbPlBox.addEventListener('dragend', () => { dragPl = dragFolder = null; $$('.drop, .drop-before', sbPlBox).forEach(x => x.classList.remove('drop', 'drop-before')); });
sbPlBox.addEventListener('dragover', e => {
  if (!dragPl && !dragFolder) return;
  e.preventDefault();
  $$('.drop, .drop-before', sbPlBox).forEach(x => x.classList.remove('drop', 'drop-before'));
  const f = e.target.closest('[data-plf-toggle]'), p = e.target.closest('[data-pl]');
  if (dragPl && f) f.classList.add('drop');
  else if (p && p.dataset.pl !== dragPl) p.classList.add('drop-before');
  else if (dragFolder && f && f.dataset.plfToggle !== dragFolder) f.classList.add('drop-before');
});
sbPlBox.addEventListener('drop', e => {
  if (!dragPl && !dragFolder) return;
  e.preventDefault();
  const pls = S.prefs.playlists, folders = S.prefs.plFolders || [];
  const f = e.target.closest('[data-plf-toggle]'), p = e.target.closest('[data-pl]');
  if (dragPl) {
    const pl = pls.find(x => x.id === dragPl);
    if (f) movePlaylist(dragPl, f.dataset.plfToggle);
    else {
      pls.splice(pls.indexOf(pl), 1);
      const target = p && pls.find(x => x.id === p.dataset.pl);
      if (target) { pls.splice(pls.indexOf(target), 0, pl); if (target.folder) pl.folder = target.folder; else delete pl.folder; }
      else { pls.push(pl); delete pl.folder; }
      savePrefs(); renderSidebar();
    }
  } else if (dragFolder && f && f.dataset.plfToggle !== dragFolder) {
    const a = folders.findIndex(x => x.id === dragFolder), [moved] = folders.splice(a, 1);
    folders.splice(folders.findIndex(x => x.id === f.dataset.plfToggle), 0, moved);
    savePrefs(); renderSidebar();
  }
  dragPl = dragFolder = null;
});
// Dropping a playlist on the "Playlists" header takes it out of its folder
$('#sb-pl-h').addEventListener('dragover', e => { if (dragPl) e.preventDefault(); });
$('#sb-pl-h').addEventListener('drop', e => {
  if (!dragPl) return; e.preventDefault();
  const pls = S.prefs.playlists, pl = pls.find(x => x.id === dragPl);
  delete pl.folder; pls.splice(pls.indexOf(pl), 1); pls.unshift(pl);
  savePrefs(); renderSidebar(); dragPl = null;
});

/* ───────────── Last.fm key: step-by-step guide ───────────── */
function lastfmGuide() {
  const field = (name, what) => `<div class="g-f"><span class="g-n">${name}</span><span>${what}</span></div>`;
  dialog(`<h2>Getting your Last.fm API key</h2>
    <p class="sub">It's free and takes a minute. You need a Last.fm account (sign up at last.fm if you don't have one).</p>
    <ol class="guide">
      <li>Open <span class="lnk" data-action="open-link" data-arg="https://www.last.fm/api/account/create">last.fm/api/account/create</span> while signed in to Last.fm.</li>
      <li>Fill in the form:
        ${field('Contact email', 'Your email (it usually comes filled in).')}
        ${field('Application name', 'Anything, for example <b>Wax</b>.')}
        ${field('Application description', 'Optional. For example <i>My music player</i>.')}
        ${field('Callback URL', '<b>Leave it empty.</b> Wax signs in the desktop way and doesn\'t use it.')}
        ${field('Application homepage', '<b>Leave it empty.</b>')}
      </li>
      <li>Tick <b>I'm not a robot</b> and press <b>Submit</b>.</li>
      <li>The next page shows an <b>API key</b> and a <b>Shared secret</b>. Copy both into Wax (Settings → Scrobbling → Last.fm) and press <b>Connect</b>.</li>
      <li>Your browser opens Last.fm asking to allow Wax: press <b>Yes, allow access</b>. Back in Wax, press <b>I authorized it</b>.</li>
    </ol>
    <p class="sub">If you lose them, your keys are always at <span class="lnk" data-action="open-link" data-arg="https://www.last.fm/api/accounts">last.fm/api/accounts</span>. With only the API key you already get popular songs, related artists and bios; connecting adds scrobbling, loved tracks and history.</p>
    <div class="btns"><span class="grow"></span><button class="btn btn-green" data-d="ok">Got it</button></div>`, 'wide');
  modalBg.onclick = guideClick;
}

function discordGuide() {
  dialog(`<h2>Showing Wax on Discord</h2>
    <p class="sub">Discord only shows activity from "applications". You create one for yourself (free, a minute), and its name is what your friends see: "Listening to <b>Wax</b>".</p>
    <ol class="guide">
      <li>Open <span class="lnk" data-action="open-link" data-arg="https://discord.com/developers/applications">discord.com/developers/applications</span> and sign in with your Discord account.</li>
      <li>Press <b>New Application</b> (top right).</li>
      <li>Name it <b>Wax</b> (or whatever you want Discord to show), tick the box accepting the terms and press <b>Create</b>.</li>
      <li>You land on <b>General Information</b>. Copy the <b>Application ID</b> (a long number, there's a <b>Copy</b> button).</li>
      <li>In Wax: Settings → Discord, paste it in <b>Application ID</b> and turn on <b>Show what you listen to on Discord</b>.</li>
    </ol>
    <p class="sub">Optional: in <b>Rich Presence → Art Assets</b> you can upload the Wax icon as the app image. Nothing else is needed: no bot, no token, no secret.</p>
    <p class="sub">The Discord app has to be open on this PC, and in Discord → Settings → <b>Activity Privacy</b>, "Share your detected activities with others" must be on.</p>
    <div class="btns"><span class="grow"></span><button class="btn btn-green" data-d="ok">Got it</button></div>`, 'wide');
  modalBg.onclick = guideClick;
}
function pinGuide() {
  dialog(`<h2>Pinning Wax to the taskbar</h2>
    <p class="sub">A portable app pins like any other, as long as Wax.exe stays in the same place.</p>
    <ol class="guide">
      <li>Put the Wax folder somewhere fixed, for example <b>C:\\Apps\\Wax</b>. Pins and shortcuts point at that exact path: on a USB stick the drive letter can change and the pin stops working.</li>
      <li>Open Wax from there.</li>
      <li>Right-click its icon on the taskbar → <b>Pin to taskbar</b>.</li>
    </ol>
    <p class="sub">Clicking the pin while Wax is already open (or in the tray) brings the same window back; it never opens a second Wax. If you move the folder later, unpin it, open Wax from the new place and pin it again.</p>
    <div class="btns"><span class="grow"></span><button class="btn btn-green" data-d="ok">Got it</button></div>`, 'wide');
  modalBg.onclick = guideClick;
}
function guideClick(e) {
  const l = e.target.closest('[data-action="open-link"]'); if (l) { invoke('open_link', { url: l.dataset.arg }); return; }
  if (e.target === modalBg || e.target.closest('[data-d]')) closeDialog();
}

/* ───────────── Actions (from data-action buttons) ───────────── */
function extraAction(a, arg) {
  switch (a) {
    case 'pop-more': Pop.more = !Pop.more; refreshPopular(); return;
    case 'about-more': { const b = $('.album-about'); b?.classList.toggle('clamp'); const t = $('[data-action="about-more"]'); if (t && b) t.textContent = b.classList.contains('clamp') ? 'More' : 'Less'; return; }
    case 'lf-guide': lastfmGuide(); return;
    case 'discord-guide': discordGuide(); return;
    case 'pin-guide': pinGuide(); return;
    case 'shortcut':
      invoke('create_shortcut', { place: arg })
        .then(p => toast(arg === 'desktop' ? 'Shortcut created on the desktop' : 'Wax is in the Start menu: search "Wax" to open or pin it', 4000))
        .catch(e => toast("Couldn't create the shortcut: " + e, 5000));
      return;
    case 'lf-loved': importLoved(); return;
    case 'lf-history': importHistory(); return;
    case 'lf-tags': fetchArtistTags(true); return;
    case 'rg-write': writeReplayGain(); return;
    case 'lyrics-all': batchLyrics(S.tracks.map((_, i) => i)); return;
    case 'dup-play': playList([+arg], 0); return;
    case 'dup-reveal': invoke('reveal', { path: S.tracks[+arg].path }); return;
    case 'dup-trash': {
      const t = S.tracks[+arg], g = (S.dupGroups || []).find(g => g.includes(+arg));
      const keepI = g?.find(i => i !== +arg);
      confirmBox('Remove this copy?', `"${t.path}" goes to the Recycle Bin.`, 'Move to Recycle Bin')
        .then(ok => ok && recycle([t.path], new Map(keepI !== undefined ? [[t.path, S.tracks[keepI].path]] : [])));
      return;
    }
    case 'dup-keep': dupKeep([S.dupGroups[+arg]]); return;
    case 'dup-keep-all': dupKeep(S.dupGroups || []); return;
    case 'health-refresh': { healthScan(true); const b = $('#health-body'); if (b) { b.innerHTML = healthHtml(); hydrate(b); } return; }
    case 'health-more': { H.open = arg || null; const b = $('#health-body'); if (b) { b.innerHTML = healthHtml(); hydrate(b); } return; }
    case 'health-mb': { const al = S.albumMap.get(arg); if (al) musicBrainz(al.tracks); return; }
    case 'health-covers-online': embedCovers(S.albums.filter(x => H.covers?.get(x.key) && !H.covers.get(x.key).any), true); return;
    case 'health-covers-embed': embedCovers(S.albums.filter(x => H.covers?.get(x.key)?.any && !H.covers.get(x.key).embedded), false); return;
    case 'health-lyrics': batchLyrics(S.tracks.map((t, i) => i).filter(i => H.lyrics?.get(S.tracks[i].path) === false)); return;
  }
}
sleepButton();
