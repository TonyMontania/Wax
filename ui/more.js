'use strict';
/* Wax — more features, loaded after extras.js (uses the globals of app.js and extras.js):
   choosing covers and artist photos, full-screen "Now playing", playlist details, accent color,
   Wrapped, new releases, searching by lyrics, audio output (device, exclusive mode, AutoEQ,
   crossfeed), tags from file names, capitalization, credits, undoing "Organize files"
   and importing Spotify playlists. */

/* ───────────── Choosing a cover or an artist photo ───────────── */
async function imagePicker({ kind, key, name }) {
  const album = kind === 'album' ? S.albumMap.get(key) : null;
  const artist = album ? (album.albumArtist || album.owner || album.artist) : name;
  if (kind === 'album' && !album) return;
  const store = kind === 'album' ? 'coverOverrides' : 'artistOverrides';
  const id = kind === 'album' ? key : name;
  const current = S.prefs[store]?.[id];
  const folder = kind === 'album' ? dirOf(realPath(S.tracks[album.tracks[0]])) : S.artistDir.get(name);
  const fileName = kind === 'album' ? (album.shared ? safeName(album.title) + '.jpg' : 'cover.jpg') : 'artist.jpg';
  let chosen = null;
  const d = dialog(`<h2>${kind === 'album' ? `Cover for "${esc(album.title)}"` : `Photo of ${esc(name)}`}</h2>
    <p class="sub">Pick one from Deezer or use an image of your own.</p>
    <div class="pick-grid${kind === 'artist' ? ' round' : ''}" id="pick-grid"><div class="ly-status"><span class="spin"></span></div></div>
    ${folder ? `<label class="chk"><input type="checkbox" id="pick-folder"> Also save it as <b>${esc(fileName)}</b> in the ${kind === 'album' ? 'album' : 'artist'} folder (other players use it too)</label>` : ''}
    ${kind === 'album' ? `<label class="chk"><input type="checkbox" id="pick-embed"> Also put it inside the files (replaces the cover they have)</label>` : ''}
    <div class="btns"><button class="btn btn-ghost" data-d="file">Choose a file…</button>${current ? `<button class="btn btn-ghost" data-d="reset">Back to automatic</button>` : ''}<span class="grow"></span>
      <button class="btn btn-ghost" data-d="cancel">Cancel</button><button class="btn btn-green" data-d="ok" disabled>Use this image</button></div>`, 'wide');
  const okBtn = $('[data-d="ok"]', d);
  invoke('image_candidates', { kind, artist: artist || '', album: album?.title || '' }).then(list => {
    const g = $('#pick-grid', d); if (!g) return;
    g.innerHTML = list.length ? list.map((c, i) => `<div class="pick" data-pick="${i}" title="${esc(c.title)} — ${esc(c.sub)}"><img src="${esc(c.thumb)}" loading="lazy"><div class="t">${esc(c.title)}</div><div class="s">${esc(c.sub)}</div></div>`).join('')
      : `<p class="sub">Deezer has nothing for this. Choose a file of your own.</p>`;
    g._list = list;
  }).catch(e => { const g = $('#pick-grid', d); if (g) g.innerHTML = `<p class="sub">${esc(String(e))}</p>`; });
  const apply = async source => {
    const toFolder = $('#pick-folder', d)?.checked, embed = $('#pick-embed', d)?.checked;
    closeDialog();
    let path;
    try { path = await invoke('save_custom_image', { source, name: `${kind}-${id}`, old: current || null }); }
    catch (e) { toast(String(e), 5000); return; }
    (S.prefs[store] ||= {})[id] = path;
    savePrefs();
    if (toFolder && folder) await invoke('place_image', { from: path, to: `${folder}\\${fileName}` }).catch(e => toast("Couldn't save it in the folder: " + e, 5000));
    if (embed) await embedImage(album, path);
    refreshImages();
    toast(kind === 'album' ? 'Cover changed' : 'Photo changed');
  };
  modalBg.onclick = async e => {
    const pk = e.target.closest('[data-pick]');
    if (pk) { $$('.pick.on', d).forEach(x => x.classList.remove('on')); pk.classList.add('on'); chosen = $('#pick-grid', d)._list[+pk.dataset.pick].url; okBtn.disabled = false; return; }
    const b = e.target.closest('[data-d]')?.dataset.d;
    if (b === 'cancel' || e.target === modalBg) { closeDialog(); return; }
    if (b === 'ok' && chosen) apply(chosen);
    if (b === 'file') { const f = await invoke('pick_image'); if (f) apply(f); }
    if (b === 'reset') { delete S.prefs[store][id]; savePrefs(); closeDialog(); refreshImages(); toast('Back to the automatic image'); }
  };
  modalBg.ondblclick = e => { const pk = e.target.closest('[data-pick]'); if (pk && chosen) apply(chosen); };
}
function refreshImages() {
  coverP.clear(); artistP.clear();
  $$('[data-cover]').forEach(el => paintCover(el));
  updateNowPlaying();
}
async function embedImage(album, image) {
  const jobs = [{ paths: album.tracks.map(i => S.tracks[i].path), artist: album.artist, album: album.title, shared: !!album.shared, online: false, image, replace: true }];
  const task = startTask('Embedding the cover'); task.kind = 'covers';
  const r = await withReleased(jobs[0].paths, () => invoke('embed_covers', { jobs })).catch(e => ({ tracks: [], errors: [String(e)] }));
  applyTracks(r.tracks);
  task.finish(r.errors.length ? `Embedded with errors: ${r.errors[0]}` : `Cover embedded in ${plural(r.tracks.length, 'file', 'files')}`);
}
// Right-click on an artist (card or the big photo on their page)
view.addEventListener('contextmenu', e => {
  const card = e.target.closest('.card.round[data-go="artist"], .artist-hero .avatar-xl');
  if (!card) return;
  const name = card.dataset.arg || route.arg;
  const ar = S.artistMap.get(name) || S.artistMap.get(ownerOf(name)); if (!ar) return;
  openMenu(e.clientX, e.clientY, [
    { label: 'Play radio', act: () => playArtistRadio(ar.name) },
    { label: 'Go to artist', act: () => go({ view: 'artist', arg: ar.name }) },
    '-',
    { label: 'Change photo…', act: () => imagePicker({ kind: 'artist', name: ar.name }) },
  ]);
});

/* ───────────── Playlist details: image and description ───────────── */
async function editPlaylistDetails(id) {
  const pl = S.prefs.playlists.find(p => p.id === id); if (!pl) return;
  let image = pl.image || null, newFile = null, removed = false;
  const draw = () => dialog(`<h2>Edit details</h2>
    <div class="pl-edit">
      <div class="pl-edit-img" data-d="img" title="Choose an image">${image || newFile ? `<img src="${fileUrl(newFile || image)}">` : `<span>${ic('note', 48)}</span>`}<div class="ov">Choose image</div></div>
      <div class="form one grow"><label><span>Name</span><input id="pl-name" value="${esc(pl.name)}"></label>
        <label><span>Description</span><textarea id="pl-desc" rows="4" placeholder="Add an optional description">${esc(pl.description || '')}</textarea></label></div>
    </div>
    <div class="btns">${image || newFile ? `<button class="btn btn-ghost" data-d="noimg">Remove image</button>` : ''}<span class="grow"></span><button class="btn btn-ghost" data-d="cancel">Cancel</button><button class="btn btn-green" data-d="save">Save</button></div>`, 'wide');
  let d = draw();
  modalBg.onclick = async e => {
    const b = e.target.closest('[data-d]')?.dataset.d;
    if (b === 'cancel' || e.target === modalBg) { closeDialog(); return; }
    if (b === 'img') {
      const f = await invoke('pick_image'); if (!f) return;
      const keep = { name: $('#pl-name', d).value, desc: $('#pl-desc', d).value };
      newFile = f; removed = false; d = draw(); $('#pl-name', d).value = keep.name; $('#pl-desc', d).value = keep.desc;
      return;
    }
    if (b === 'noimg') {
      const keep = { name: $('#pl-name', d).value, desc: $('#pl-desc', d).value };
      newFile = null; image = null; removed = true; d = draw(); $('#pl-name', d).value = keep.name; $('#pl-desc', d).value = keep.desc;
      return;
    }
    if (b !== 'save') return;
    pl.name = $('#pl-name', d).value.trim() || pl.name;
    pl.description = $('#pl-desc', d).value.trim();
    closeDialog();
    if (newFile) {
      try { pl.image = await invoke('save_custom_image', { source: newFile, name: 'playlist-' + id, old: pl.image || null }); }
      catch (err) { toast(String(err), 5000); }
    } else if (removed) delete pl.image;
    savePrefs(); renderSidebar(); rerender();
  };
}

/* ───────────── Accent color ───────────── */
const ACCENTS = [['#1db954', 'Green'], ['#2e77d0', 'Blue'], ['#8d67ab', 'Purple'], ['#e8115b', 'Pink'], ['#e22134', 'Red'], ['#f59b23', 'Orange'], ['#f1c40f', 'Yellow'], ['#1ed3b5', 'Teal']];
const accentSwatches = () => `<div class="swatches">${ACCENTS.map(([c, n]) => `<button class="sw-c${c === accent() ? ' on' : ''}" data-accent="${c}" title="${n}" style="background:${c}"></button>`).join('')}</div>`;
function lighten(hex, k = 0.18) {
  const n = parseInt(hex.slice(1), 16), ch = [n >> 16, (n >> 8) & 255, n & 255].map(v => Math.round(v + (255 - v) * k));
  return '#' + ch.map(v => v.toString(16).padStart(2, '0')).join('');
}
function applyAccent() {
  const c = accent();
  document.documentElement.style.setProperty('--green', c);
  document.documentElement.style.setProperty('--green-hi', lighten(c));
}
view.addEventListener('click', e => {
  const s = e.target.closest('[data-accent]'); if (!s) return;
  S.prefs.accent = s.dataset.accent; savePrefs(); applyAccent(); miniState();
  $$('.sw-c').forEach(x => x.classList.toggle('on', x === s));
});

/* ───────────── Audio output, AutoEQ, crossfeed ───────────── */
const outStatus = () => {
  const o = S.outInfo;
  if (!audio.rust) return "The audio engine isn't available";
  if (!o) return S.prefs.outDevice ? S.prefs.outDevice : 'The device Windows uses for sound';
  return `${o.device} · ${(o.rate / 1000).toFixed(1).replace('.0', '')} kHz${o.exclusive ? ` · exclusive, ${o.format}` : ''}`;
};
async function applyAudioPrefs() {
  applyAccent();
  if (!audio.rust) return;
  invoke('audio_devices').then(d => { S.devices = d; if (route.view === 'settings') rerender(); }).catch(() => {});
  if (S.prefs.crossfeed) invoke('audio_crossfeed', { level: +S.prefs.crossfeed }).catch(() => {});
  if (S.prefs.outDevice || S.prefs.exclusive) await setOutput(true);
}
async function setOutput(quiet = false) {
  const t = S.tracks[P.current], pos = audio.currentTime, playing = !audio.paused;
  try {
    S.outInfo = await invoke('audio_output', { device: S.prefs.outDevice || null, exclusive: !!S.prefs.exclusive });
    if (!quiet) toast(`Sound: ${outStatus()}`, 3500);
  } catch (e) {
    S.outInfo = null;
    if (S.prefs.exclusive) { S.prefs.exclusive = false; savePrefs(); }
    toast(String(e), 7000);
  }
  // The new output starts from scratch: volume, equalizer, crossfeed and the song come back
  setVolume(S.prefs.volume, S.prefs.muted);
  applyEq();
  invoke('audio_crossfeed', { level: +S.prefs.crossfeed || 0 }).catch(() => {});
  if (t && audio.mode === 'rust' && audio.track) audio.load(t, { autoplay: playing, startAt: pos, gainDb: gainFor(t) });
  syncNext();
  if (route.view === 'settings') rerender();
}
view.addEventListener('change', e => {
  if (e.target.id === 'out-device') { S.prefs.outDevice = e.target.value; savePrefs(); setOutput(); }
  if (e.target.id === 'out-exclusive') { S.prefs.exclusive = e.target.checked; savePrefs(); setOutput(); }
  if (e.target.id === 'xfeed-level') {
    S.prefs.crossfeed = +e.target.value; savePrefs();
    invoke('audio_crossfeed', { level: S.prefs.crossfeed }).catch(() => {});
  }
});

// Equalizer view: AutoEQ (headphone correction) and crossfeed, below the bands
const AEQ = { list: null, loading: false };
const baseEq = VIEWS.eq;
VIEWS.eq = () => {
  const ae = S.prefs.autoeq;
  return baseEq() + `
    <div class="sec-h"><h2>Headphones</h2><span class="src">AutoEQ</span></div>
    <p class="sub">Corrects the sound of your headphones to a neutral target, from the AutoEQ measurements (about 9,000 models). It works with or without the equalizer above.</p>
    ${ae ? `<div class="aeq-cur"><div class="grow"><b>${esc(ae.name)}</b> <span class="sub">${esc(ae.source)} · ${ae.filters.length} filters · preamp ${fmtDb(ae.preamp)}</span></div>
      <label class="switch"><input type="checkbox" id="aeq-on" ${ae.on ? 'checked' : ''}><span></span></label><button class="ib" data-action="aeq-remove" title="Remove">${ic('x', 14)}</button></div>` : ''}
    <label class="filter aeq-search">${ic('search', 13)}<input id="aeq-q" placeholder="Search your headphones (e.g. HD 600, AirPods, Sony WH-1000XM4)" spellcheck="false"></label>
    <div id="aeq-list"></div>
    <div class="sec-h"><h2>Crossfeed</h2></div>
    <div class="set-row"><div class="set-t"><div class="t">Mix a little of each channel into the other</div><div class="d">For headphones: old stereo records with instruments only on one side stop feeling tiring.</div></div>
      <div class="set-c"><select class="sel" id="xfeed-level">${[[0, 'Off'], [1, 'Light'], [2, 'Medium'], [3, 'Strong']].map(([v, l]) => `<option value="${v}" ${(+S.prefs.crossfeed || 0) === v ? 'selected' : ''}>${l}</option>`).join('')}</select></div></div>`;
};
view.addEventListener('input', async e => {
  if (e.target.id !== 'aeq-q') return;
  const q = norm(e.target.value).trim(), box = $('#aeq-list');
  if (q.length < 2) { box.innerHTML = ''; return; }
  if (!AEQ.list) {
    if (AEQ.loading) return;
    AEQ.loading = true; box.innerHTML = '<div class="ly-status"><span class="spin"></span></div>';
    try { AEQ.list = await invoke('autoeq_index'); } catch (err) { box.innerHTML = `<p class="sub">${esc(String(err))}</p>`; AEQ.loading = false; return; }
    AEQ.loading = false;
  }
  const words = norm($('#aeq-q')?.value || '').trim().split(/\s+/);
  const hits = AEQ.list.map((m, i) => [m, i]).filter(([m]) => { const n = norm(m.name); return words.every(w => n.includes(w)); }).slice(0, 40);
  box.innerHTML = hits.length ? hits.map(([m, i]) => `<div class="aeq-row" data-aeq="${i}"><span>${esc(m.name)}</span><span class="sub">${esc(m.source)}</span></div>`).join('') : '<p class="sub">No headphones with that name.</p>';
});
view.addEventListener('click', async e => {
  const r = e.target.closest('[data-aeq]'); if (!r) return;
  const m = AEQ.list[+r.dataset.aeq];
  r.classList.add('on');
  try {
    const p = await invoke('autoeq_profile', { path: m.path });
    S.prefs.autoeq = { name: m.name, source: m.source, path: m.path, preamp: p.preamp, filters: p.filters, on: true };
    savePrefs(); applyEq(); rerender();
    toast(`AutoEQ: ${m.name}`);
  } catch (err) { toast(String(err), 5000); }
});
view.addEventListener('change', e => {
  if (e.target.id === 'aeq-on' && S.prefs.autoeq) { S.prefs.autoeq.on = e.target.checked; savePrefs(); applyEq(); }
});

/* ───────────── Full-screen "Now playing" ───────────── */
const NF = { el: null, lines: [], synced: false, idx: -1, raf: 0, path: null, idle: 0 };
function npFull(open) {
  if (open && P.current < 0) return;
  if (!NF.el) {
    NF.el = document.createElement('div');
    NF.el.id = 'npfull';
    NF.el.innerHTML = `<div class="nf-bg"></div><div class="nf-shade"></div>
      <button class="ib nf-close" title="Close (Esc)">${ic('x', 22)}</button>
      <div class="nf-body"><div class="nf-left"><div class="nf-cover"></div><div class="nf-title"></div><div class="nf-artist"></div><div class="nf-album"></div></div>
        <div class="nf-lyrics"></div></div>
      <div class="nf-bar"><span class="nf-t">0:00</span><div class="nf-prog"><div class="nf-fill"></div></div><span class="nf-d">0:00</span>
        <div class="nf-ctl"><button class="ib" data-nf="fav">${ic('heart', 20)}</button><button class="ib" data-nf="prev">${ic('prev', 20)}</button>
        <button class="nf-play" data-nf="play">${ic('play', 22)}</button><button class="ib" data-nf="next">${ic('next', 20)}</button><button class="ib" data-nf="lyrics" title="Lyrics">${ic('mic', 20)}</button></div></div>`;
    document.body.appendChild(NF.el);
    NF.el.addEventListener('click', e => {
      const b = e.target.closest('[data-nf]')?.dataset.nf;
      if (e.target.closest('.nf-close')) npFull(false);
      else if (b === 'play') togglePlay();
      else if (b === 'prev') prev();
      else if (b === 'next') next();
      else if (b === 'fav' && P.current >= 0) { toggleFav([P.current]); npFullSync(); }
      else if (b === 'lyrics') { S.prefs.nfLyrics = S.prefs.nfLyrics === false; savePrefs(); npFullSync(true); }
      const ln = e.target.closest('[data-nfl]');
      if (ln && NF.synced) audio.currentTime = Math.max(0, NF.lines[+ln.dataset.nfl].t);
      if (e.target.closest('.nf-prog')) { const r = e.target.closest('.nf-prog').getBoundingClientRect(); if (audio.duration) audio.currentTime = (e.clientX - r.left) / r.width * audio.duration; }
    });
    NF.el.addEventListener('mousemove', () => { NF.el.classList.remove('idle'); clearTimeout(NF.idle); NF.idle = setTimeout(() => NF.el.classList.add('idle'), 3000); });
  }
  NF.el.classList.toggle('open', open);
  invoke('set_fullscreen', { on: open }).catch(() => {});
  if (open) { NF.path = null; npFullSync(); nfLoop(); } else cancelAnimationFrame(NF.raf);
}
function npFullSync(force = false) {
  if (!NF.el?.classList.contains('open')) return;
  const t = S.tracks[P.current]; if (!t) return;
  const key = albumKeyOf(t);
  $('.nf-title', NF.el).textContent = t.title;
  $('.nf-artist', NF.el).textContent = t.artist;
  $('.nf-album', NF.el).textContent = [t.album, t.year || ''].filter(Boolean).join(' · ');
  $('[data-nf="fav"]', NF.el).innerHTML = ic(isFav(t.path) ? 'heartFill' : 'heart', 20);
  $('[data-nf="fav"]', NF.el).classList.toggle('on', isFav(t.path));
  $('[data-nf="lyrics"]', NF.el).classList.toggle('on', S.prefs.nfLyrics !== false);
  albumCover(key, true).then(c => {
    $('.nf-cover', NF.el).style.backgroundImage = c ? `url("${c.url}")` : '';
    $('.nf-bg', NF.el).style.backgroundImage = c ? `url("${c.url}")` : '';
    NF.el.style.setProperty('--nfc', c?.color || '#333');
  });
  if (NF.path === t.path && !force) return;
  NF.path = t.path; NF.lines = []; NF.synced = false; NF.idx = -1;
  const box = $('.nf-lyrics', NF.el);
  box.innerHTML = '';
  NF.el.classList.remove('has-lyrics');
  if (S.prefs.nfLyrics === false) return;
  invoke('get_lyrics', { path: t.path, title: t.title, artist: t.artist, album: t.album, duration: t.duration || 0, online: S.prefs.lyricsOnline !== false, force: false }).then(r => {
    if (NF.path !== t.path) return;
    if (r.synced) { NF.lines = parseLrc(r.synced); NF.synced = NF.lines.length > 0; }
    if (NF.synced) box.innerHTML = NF.lines.map((l, i) => `<p data-nfl="${i}">${esc(l.text) || '♪'}</p>`).join('');
    else if (r.plain) box.innerHTML = r.plain.split(/\r?\n/).map(x => `<p class="plain">${esc(x) || '&nbsp;'}</p>`).join('');
    NF.el.classList.toggle('has-lyrics', !!(NF.synced || r.plain));
  }).catch(() => {});
}
function nfLoop() {
  cancelAnimationFrame(NF.raf);
  const tick = () => {
    if (!NF.el.classList.contains('open')) return;
    const pos = audio.currentTime || 0, dur = audio.duration || S.tracks[P.current]?.duration || 0;
    $('.nf-fill', NF.el).style.width = (dur ? pos / dur * 100 : 0) + '%';
    $('.nf-t', NF.el).textContent = fmt(pos); $('.nf-d', NF.el).textContent = fmt(dur);
    $('.nf-play', NF.el).innerHTML = ic(audio.paused ? 'play' : 'pause', 22);
    if (NF.synced) {
      let lo = 0, hi = NF.lines.length - 1, idx = -1;
      while (lo <= hi) { const m = (lo + hi) >> 1; if (NF.lines[m].t <= pos + 0.2) { idx = m; lo = m + 1; } else hi = m - 1; }
      if (idx !== NF.idx) {
        NF.idx = idx;
        const ps = $$('.nf-lyrics p', NF.el);
        ps.forEach((p, i) => { p.classList.toggle('on', i === idx); p.classList.toggle('past', i < idx); });
        const el = ps[Math.max(0, idx)], box = $('.nf-lyrics', NF.el);
        if (el) box.scrollTo({ top: el.offsetTop - box.clientHeight * 0.4, behavior: 'smooth' });
      }
    }
    NF.raf = requestAnimationFrame(tick);
  };
  NF.raf = requestAnimationFrame(tick);
}
addEventListener('keydown', e => {
  const open = NF.el?.classList.contains('open');
  if (e.key === 'Escape' && open) npFull(false);
  else if (e.key === 'F11' && !e.target.matches('input, textarea')) { e.preventDefault(); npFull(!open); }
});
$('#b-full').onclick = () => npFull(true);

/* ───────────── Wrapped ───────────── */
VIEWS.wrapped = year => {
  setTimeout(() => renderWrapped(+year || new Date().getFullYear()));
  return `<div id="wrapped"><div class="ly-status"><span class="spin"></span></div></div>`;
};
async function renderWrapped(year) {
  if (!Stats.data || Stats.dirty) { Stats.data = await invoke('load_history').catch(() => []); Stats.dirty = false; }
  const box = $('#wrapped'); if (!box || route.view !== 'wrapped') return;
  const all = Stats.data.filter(l => S.byPath.has(l.p));
  const years = [...new Set(all.map(l => new Date(l.t * 1000).getFullYear()))].sort((a, b) => b - a);
  const list = all.filter(l => new Date(l.t * 1000).getFullYear() === year);
  const yearTabs = `<div class="tabs">${(years.length ? years : [year]).map(y => `<span class="tab ${y === year ? 'on' : ''}" data-go="wrapped" data-arg="${y}">${y}</span>`).join('')}</div>`;
  if (!list.length) {
    box.innerHTML = `<div class="page-h"><h1>${year} Wrapped</h1>${yearTabs}</div><div class="empty"><div class="big-ic">${ic('chart', 80)}</div><h2>No plays in ${year}</h2><p>Listen to some music (or import your Last.fm history in Settings).</p></div>`;
    return;
  }
  const count = (key) => { const m = new Map(); for (const l of list) { const k = key(l); if (k != null) m.set(k, (m.get(k) || 0) + 1); } return [...m].sort((a, b) => b[1] - a[1]); };
  const T = l => S.tracks[S.byPath.get(l.p)];
  const secs = list.reduce((s, l) => s + (l.s || 0), 0), minutes = Math.round(secs / 60);
  const days = new Set(list.map(l => new Date(l.t * 1000).toDateString()));
  const artists = count(l => T(l)._owner).slice(0, 5);
  const songs = count(l => S.byPath.get(l.p)).slice(0, 5);
  const albums = count(l => albumKeyOf(T(l))).slice(0, 1);
  const genres = (() => { const m = new Map(); for (const l of list) for (const g of T(l)._g) m.set(g, (m.get(g) || 0) + 1); return [...m].sort((a, b) => b[1] - a[1]).slice(0, 3); })();
  const months = Array(12).fill(0); list.forEach(l => months[new Date(l.t * 1000).getMonth()] += (l.s || 0) / 60);
  const topMonth = months.indexOf(Math.max(...months));
  const hours = Array(24).fill(0); list.forEach(l => hours[new Date(l.t * 1000).getHours()]++);
  const peak = hours.indexOf(Math.max(...hours));
  const persona = peak >= 22 || peak < 5 ? ['Night owl', 'Most of your music happens after dark.'] : peak < 11 ? ['Early bird', 'You start the day with music.'] : peak < 18 ? ['Daytime listener', 'Music keeps your days going.'] : ['Evening listener', 'Music is how you wind down.'];
  // Artists played for the first time this year
  const firstYear = new Map();
  for (const l of all) { const o = T(l)._owner, y = new Date(l.t * 1000).getFullYear(); if (!firstYear.has(o) || firstYear.get(o) > y) firstYear.set(o, y); }
  const discovered = [...firstYear].filter(([, y]) => y === year).map(([o]) => o);
  // Longest run of consecutive days with music
  const dayNums = [...new Set(list.map(l => Math.floor((l.t - new Date(l.t * 1000).getTimezoneOffset() * 60) / 86400)))].sort((a, b) => a - b);
  let streak = 1, best = 1; for (let i = 1; i < dayNums.length; i++) { streak = dayNums[i] === dayNums[i - 1] + 1 ? streak + 1 : 1; best = Math.max(best, streak); }
  const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];
  const topSongs = count(l => S.byPath.get(l.p)).slice(0, 50).map(x => x[0]);
  lists.set('wrapped-top', topSongs);
  const topAr = S.artistMap.get(artists[0]?.[0]);
  const maxM = Math.max(...months, 1);
  box.innerHTML = `<div class="page-h"><div class="row"><h1>${year} Wrapped</h1><span class="grow"></span>
      ${playBtn('wrapped-top', 'Play your top songs')}<button class="btn btn-ghost" data-action="wrapped-save" data-arg="${year}">Save as playlist</button></div>${yearTabs}</div>
    <div class="wr-grid">
      <div class="wr c1"><div class="k">You listened for</div><div class="big">${minutes.toLocaleString()}</div><div class="k">minutes</div><div class="s">${plural(list.length, 'play', 'plays')} on ${plural(days.size, 'day', 'days')}</div></div>
      <div class="wr c2 wr-artist">${topAr ? cvHtml(topAr.albums[0]?.key, 'wr-photo', 50, true, topAr.name) : ''}<div><div class="k">Your top artist</div><div class="big2">${esc(artists[0]?.[0] || '')}</div><div class="s">${plural(artists[0]?.[1] || 0, 'play', 'plays')}</div></div></div>
      <div class="wr c3"><div class="k">Top artists</div><ol>${artists.map(([n, c]) => `<li><span class="lnk" data-go="artist" data-arg="${esc(n)}">${esc(n)}</span> <span class="s">${c}</span></li>`).join('')}</ol></div>
      <div class="wr c4"><div class="k">Top songs</div><ol>${songs.map(([i, c]) => `<li>${esc(S.tracks[i].title)} <span class="s">${esc(S.tracks[i].artist)} · ${c}</span></li>`).join('')}</ol></div>
      ${genres.length ? `<div class="wr c5"><div class="k">Top genres</div><ol>${genres.map(([g]) => `<li>${esc(g)}</li>`).join('')}</ol></div>` : ''}
      ${albums.length ? `<div class="wr c6 wr-album" data-go="album" data-arg="${esc(albums[0][0])}">${cvHtml(albums[0][0], 'wr-cover', 40, true)}<div><div class="k">Album of the year</div><div class="big2">${esc(S.albumMap.get(albums[0][0])?.title || '')}</div><div class="s">${esc(S.albumMap.get(albums[0][0])?.artist || '')} · ${plural(albums[0][1], 'play', 'plays')}</div></div></div>` : ''}
      <div class="wr c7"><div class="k">Your biggest month</div><div class="big2">${MONTHS[topMonth]}</div><div class="months">${months.map((m, i) => `<div title="${MONTHS[i]}: ${Math.round(m)} min"><div style="height:${Math.round(m / maxM * 100)}%"></div><span>${MONTHS[i][0]}</span></div>`).join('')}</div></div>
      <div class="wr c8"><div class="k">You're a</div><div class="big2">${persona[0]}</div><div class="s">${persona[1]} Your peak hour: ${peak}:00.</div></div>
      <div class="wr c9"><div class="k">Longest streak</div><div class="big">${best}</div><div class="k">days in a row</div></div>
      <div class="wr c10"><div class="k">New to you</div><div class="big">${discovered.length}</div><div class="k">${discovered.length === 1 ? 'artist' : 'artists'} you played for the first time</div><div class="s">${discovered.slice(0, 4).map(esc).join(', ')}</div></div>
    </div>`;
  hydrate(box);
}

/* ───────────── New releases of your artists (Deezer) ───────────── */
const REL = { data: new Map(), running: false, done: false };
const editionFree = s => normTitle(String(s).replace(/\s*[([][^)\]]*(deluxe|edition|remaster|expanded|anniversary|version|bonus)[^)\]]*[)\]]/gi, ''));
VIEWS.releases = () => {
  setTimeout(() => loadReleases());
  const months = +S.prefs.relMonths || 6;
  return `<div class="page-h"><h1>Browse</h1>
    <div class="tabs"><span class="tab" data-go="browse">Overview</span><span class="tab on">New Releases</span><span class="tab" data-go="albums">Albums</span><span class="tab" data-go="genres">Genres</span><span class="tab" data-go="artists">Artists</span><span class="tab" data-go="radio">Radio</span></div>
    <div class="row"><span class="sub">Releases of the artists in your library that you don't have yet (from Deezer).</span><span class="grow"></span>
      <select class="sel" id="rel-months">${[[1, 'Last month'], [3, 'Last 3 months'], [6, 'Last 6 months'], [12, 'Last year'], [24, 'Last 2 years']].map(([v, l]) => `<option value="${v}" ${v === months ? 'selected' : ''}>${l}</option>`).join('')}</select>
      <label class="chk inline"><input type="checkbox" id="rel-singles" ${S.prefs.relSingles !== false ? 'checked' : ''}> Singles</label></div></div>
    <div id="rel-body"></div>`;
};
view.addEventListener('change', e => {
  if (e.target.id === 'rel-months') { S.prefs.relMonths = +e.target.value; savePrefs(); renderReleases(); }
  if (e.target.id === 'rel-singles') { S.prefs.relSingles = e.target.checked; savePrefs(); renderReleases(); }
});
async function loadReleases() {
  renderReleases();
  if (REL.running || REL.done) return;
  REL.running = true;
  const artists = S.artists.filter(a => !/^(unknown artist|various artists)$/i.test(a.name));
  for (const [n, ar] of artists.entries()) {
    if (REL.data.has(ar.name)) continue;
    const t0 = performance.now();
    const r = await invoke('artist_releases', { name: ar.name }).catch(() => null);
    REL.data.set(ar.name, r);
    REL.progress = [n + 1, artists.length];
    if (n % 15 === 14 || n === artists.length - 1) renderReleases();
    if (performance.now() - t0 > 40) await sleepMs(130);   // only pace real requests (Deezer's limit is 50 per 5 s)
  }
  REL.running = false; REL.done = true; REL.progress = null;
  renderReleases();
}
function renderReleases() {
  const box = $('#rel-body'); if (!box || route.view !== 'releases') return;
  const since = new Date(); since.setMonth(since.getMonth() - (+S.prefs.relMonths || 6));
  const sinceStr = since.toISOString().slice(0, 10), today = new Date().toISOString().slice(0, 10);
  const items = [];
  for (const [name, r] of REL.data) {
    if (!r) continue;
    const ar = S.artistMap.get(name);
    const owned = new Set((ar?.albums || []).map(a => editionFree(a.title)));
    const seen = new Set();
    for (const x of r.releases) {
      if (x.date < sinceStr) continue;
      if (x.kind === 'single' && S.prefs.relSingles === false) continue;
      const k = editionFree(x.title);
      if (owned.has(k) || seen.has(k)) continue;
      seen.add(k);
      items.push({ ...x, artist: name });
    }
  }
  items.sort((a, b) => b.date.localeCompare(a.date));
  const prog = REL.progress ? `<p class="sub">Checking your artists… ${REL.progress[0]} of ${REL.progress[1]}</p>` : '';
  box.innerHTML = prog + (items.length ? `<div class="grid">${items.map(x => `<div class="card" data-action="open-link" data-arg="${esc(x.link)}" title="Open on Deezer">
      <div class="cv has-img" style="background-image:url('${esc(x.cover)}')">${x.date > today ? '<span class="soon">Upcoming</span>' : ''}</div>
      <div class="t">${esc(x.title)}</div><div class="s"><span class="lnk" data-go="artist" data-arg="${esc(x.artist)}">${esc(x.artist)}</span> · ${esc(x.kind === 'ep' ? 'EP' : x.kind[0].toUpperCase() + x.kind.slice(1))} · ${esc(x.date)}</div></div>`).join('')}</div>`
    : REL.running ? '<div class="ly-status"><span class="spin"></span></div>' : `<div class="empty"><h2>Nothing new</h2><p>None of your artists released something you don't have in this period.</p></div>`);
}

/* ───────────── Searching by a line of the lyrics ───────────── */
let lyrSeq = 0;
async function lyricSearch(q) {
  const box = $('#lyr-hits'); if (!box || norm(q).trim().length < 4) return;
  const seq = ++lyrSeq;
  box.innerHTML = `<div class="sec-h"><h2>In lyrics</h2></div><p class="sub"><span class="spin"></span> Searching the lyrics…</p>`;
  const hits = await invoke('search_lyrics', { query: q, paths: S.tracks.filter(t => !t.cue).map(t => t.path), rebuild: false }).catch(() => []);
  if (seq !== lyrSeq || route.view !== 'search') return;
  const b = $('#lyr-hits'); if (!b) return;
  const idxs = hits.map(h => S.byPath.get(h.path)).filter(i => i !== undefined);
  if (!idxs.length) { b.innerHTML = ''; return; }
  lists.set('lyr-hits', idxs);
  b.innerHTML = `<div class="sec-h"><h2>In lyrics</h2></div><div class="lyr-list">${hits.filter(h => S.byPath.has(h.path)).map((h, k) => { const t = S.tracks[S.byPath.get(h.path)]; return `<div class="lyr-row" data-lyr="${k}">
      ${cvHtml(albumKeyOf(t), 'lyr-cv', 16)}<div class="grow"><div><b>${esc(t.title)}</b> <span class="sub">— ${esc(t.artist)}</span></div><div class="line">“${esc(h.line)}”</div></div><span class="pi">${ic('play', 14)}</span></div>`; }).join('')}</div>`;
  hydrate(b);
}
view.addEventListener('click', e => {
  const r = e.target.closest('[data-lyr]'); if (!r) return;
  playList(lists.get('lyr-hits'), +r.dataset.lyr);
});

/* ───────────── Credits ───────────── */
async function showCredits(t) {
  const d = dialog(`<h2>Credits</h2><p class="sub">${esc(t.title)} — ${esc(t.artist)}</p><div class="ly-status"><span class="spin"></span></div>`);
  modalBg.onclick = e => { const l = e.target.closest('[data-action="open-link"]'); if (l) { invoke('open_link', { url: l.dataset.arg }); return; } if (e.target === modalBg || e.target.closest('[data-d]')) closeDialog(); };
  let c;
  try { c = await invoke('song_credits', { artist: t._artists[0] || t.artist, title: t.title, durationMs: Math.round((t.duration || 0) * 1000) }); }
  catch (e) { d.innerHTML = `<h2>Credits</h2><p class="sub">${esc(String(e))}</p><div class="btns"><span class="grow"></span><button class="btn btn-ghost" data-d="x">Close</button></div>`; return; }
  if (!modalBg.classList.contains('open')) return;
  d.innerHTML = `<h2>Credits</h2><p class="sub">${esc(t.title)} — ${esc(t.artist)}</p>
    ${c && c.credits.length ? `<div class="credits">${c.credits.map(x => `<div class="cr"><div class="k">${esc(x.role)}</div><div>${x.names.map(n => artistHtml(n)).join(', ')}</div></div>`).join('')}</div>`
      : `<p class="sub">MusicBrainz ${c ? "doesn't have credits for this recording yet" : "doesn't know this song"}.</p>`}
    <div class="btns">${c ? `<span class="lnk" data-action="open-link" data-arg="${esc(c.url)}">See it on MusicBrainz</span>` : ''}<span class="grow"></span><button class="btn btn-ghost" data-d="x">Close</button></div>`;
}

/* ───────────── Tags from the file name ───────────── */
const FN_PATTERNS = ['{track} - {title}', '{track} - {artist} - {title}', '{artist} - {title}', '{track}. {title}', '{track} {title}', '{artist}\\{album}\\{track} - {title}', '{album}\\{track} - {title}'];
function fnRegex(seg) {
  const re = seg.replace(/[.*+?^$()|[\]\\]/g, '\\$&').replace(/\{(\w+)\}/g, (_, k) => /^(track|disc|year)$/.test(k) ? `(?<${k}>\\d+)` : `(?<${k}>.+?)`);
  return new RegExp(`^\\s*${re}\\s*$`, 'i');
}
function parseFileName(t, pattern, underscores) {
  const segs = pattern.split('\\');
  const parts = t.path.replace(/\.[^.\\/]+$/, '').split(/[\\/]/).slice(-segs.length);
  if (parts.length < segs.length) return null;
  const out = {};
  for (let i = 0; i < segs.length; i++) {
    const text = underscores ? parts[i].replace(/_/g, ' ') : parts[i];
    const m = text.match(fnRegex(segs[i]));
    if (!m) return null;
    Object.assign(out, m.groups);
  }
  for (const k of Object.keys(out)) out[k] = out[k].trim();
  return out;
}
function tagsFromFilename(idxs) {
  const ts = idxs.map(i => S.tracks[i]).filter(t => t && !t.cue);
  if (!ts.length) return;
  let pattern = S.prefs.fnPattern || FN_PATTERNS[0];
  const draw = () => {
    const under = $('#fn-under')?.checked ?? true;
    const rows = ts.slice(0, 200).map(t => [t, parseFileName(t, pattern, under)]);
    const ok = rows.filter(r => r[1]).length;
    return `<h2>Tags from the file name</h2>
      <p class="sub">${plural(ts.length, 'song', 'songs')}. Variables: {track} {title} {artist} {album} {disc} {year}; use \\ to read folder names too.</p>
      <div class="pat-pick"><select class="sel" id="fn-preset">${FN_PATTERNS.map(p => `<option ${p === pattern ? 'selected' : ''}>${esc(p)}</option>`).join('')}${FN_PATTERNS.includes(pattern) ? '' : `<option selected>${esc(pattern)}</option>`}</select>
        <input class="txt grow" id="fn-pat" value="${esc(pattern)}" spellcheck="false"></div>
      <label class="chk"><input type="checkbox" id="fn-under" ${under ? 'checked' : ''}> Underscores are spaces</label>
      <div class="mb-map fn-map">${rows.map(([t, r]) => `<div class="mb-row ${r ? '' : 'miss'}"><span class="o">${esc(t.path.split(/[\\/]/).pop())}</span><span class="arr">→</span><span class="nw">${r ? Object.entries(r).map(([k, v]) => `<b>${k}</b> ${esc(v)}`).join(' · ') : "doesn't match"}</span></div>`).join('')}</div>
      <div class="btns"><span class="sub">${ok} of ${rows.length} match</span><span class="grow"></span><button class="btn btn-ghost" data-d="cancel">Cancel</button><button class="btn btn-green" data-d="go" ${ok ? '' : 'disabled'}>Write tags</button></div>`;
  };
  const d = dialog(draw(), 'wide');
  const redraw = () => { const f = document.activeElement?.id, pos = $('#fn-pat', d)?.selectionStart; d.innerHTML = draw(); if (f === 'fn-pat') { const i = $('#fn-pat', d); i.focus(); i.setSelectionRange(pos, pos); } };
  d.addEventListener('input', e => { if (e.target.id === 'fn-pat') { pattern = e.target.value; redraw(); } });
  d.addEventListener('change', e => { if (e.target.id === 'fn-preset') { pattern = e.target.value; redraw(); } if (e.target.id === 'fn-under') redraw(); });
  modalBg.onclick = e => {
    const b = e.target.closest('[data-d]')?.dataset.d;
    if (b === 'cancel' || e.target === modalBg) { closeDialog(); return; }
    if (b !== 'go') return;
    const under = $('#fn-under', d).checked;
    S.prefs.fnPattern = pattern; savePrefs();
    const ups = ts.map(t => [t, parseFileName(t, pattern, under)]).filter(r => r[1]).map(([t, r]) => {
      const u = { path: t.path };
      if (r.title) u.title = r.title;
      if (r.artist) u.artist = r.artist;
      if (r.album) u.album = r.album;
      if (r.track) u.trackNo = parseInt(r.track) || 0;
      if (r.disc) u.discNo = parseInt(r.disc) || 0;
      if (r.year) u.year = parseInt(r.year) || 0;
      return u;
    });
    closeDialog();
    writeTags(ups);
  };
}

/* ───────────── Fixing capitalization ───────────── */
const SMALL_WORDS = new Set('a an and as at but by en for from in into nor of on or over per the to via vs vs. with feat. feat ft. ft'.split(' '));
const ROMAN = /^(?=[mdclxvi]+$)m*(c[md]|d?c{0,3})(x[cl]|l?x{0,3})(i[xv]|v?i{0,3})$/i;
function caseWord(w, mode, first, last, allCaps) {
  if (!w) return w;
  const letters = w.replace(/[^\p{L}]/gu, '');
  if (ROMAN.test(letters) && letters.length > 1 && !SMALL_WORDS.has(letters.toLowerCase())) return w.toUpperCase();
  // Acronyms (AC/DC, MTV) stay, unless the whole text is in capitals or it's a common short word
  if (!allCaps && letters.length > 1 && letters.length <= 4 && letters === letters.toUpperCase() && /[A-Z]/.test(letters) && !SMALL_WORDS.has(letters.toLowerCase())) return w;
  const lower = w.toLowerCase();
  // First letter after any opening punctuation, and after "-" or "/" (Hip-Hop, AC/DC); never after an apostrophe (Don't)
  const cap = s => s.replace(/^([^\p{L}]*)(\p{L})/u, (m, a, b) => a + b.toUpperCase()).replace(/([-/])(\p{L})/gu, (m, a, b) => a + b.toUpperCase());
  const bare = lower.replace(/^[^\p{L}]+/u, '');
  if (mode === 'every') return cap(lower);
  if (mode === 'sentence') return first ? cap(lower) : (letters === 'I' ? 'I' : lower);
  return !first && !last && SMALL_WORDS.has(bare) ? lower : cap(lower);
}
function fixCase(s, mode) {
  const words = String(s || '').split(/(\s+)/);
  const idx = words.map((w, i) => i).filter(i => words[i].trim());
  let prevBreak = true;
  const letters = String(s || '').replace(/[^\p{L}]/gu, '');
  const allCaps = letters.length > 3 && letters === letters.toUpperCase();
  return words.map((w, i) => {
    if (!w.trim()) return w;
    const out = caseWord(w, mode, prevBreak, i === idx[idx.length - 1], allCaps);
    prevBreak = /[:(\-–—]$/.test(w) || w === '-';
    return out;
  }).join('');
}
function fixCapitalization(idxs) {
  const ts = idxs.map(i => S.tracks[i]).filter(t => t && !t.cue); if (!ts.length) return;
  let mode = S.prefs.caseMode || 'title';
  const fields = { title: true, artist: false, album: true, albumArtist: false, ...(S.prefs.caseFields || {}) };
  const changes = () => ts.map(t => {
    const u = { path: t.path }, diff = [];
    for (const f of ['title', 'artist', 'album', 'albumArtist']) {
      if (!fields[f] || !t[f]) continue;
      const v = fixCase(t[f], mode);
      if (v !== t[f]) { u[f] = v; diff.push([t[f], v]); }
    }
    return [u, diff];
  }).filter(x => x[1].length);
  const draw = () => {
    const ch = changes();
    return `<h2>Fix capitalization</h2><p class="sub">${plural(ts.length, 'song', 'songs')}. Acronyms (AC/DC, MTV) and Roman numerals stay in capitals.</p>
      <div class="radios row3">${[['title', 'Title Case', 'The Dark Side of the Moon'], ['sentence', 'Sentence case', 'The dark side of the moon'], ['every', 'Every Word', 'The Dark Side Of The Moon']].map(([v, l, ex]) => `<label class="rad"><input type="radio" name="cm" value="${v}" ${v === mode ? 'checked' : ''}><span><b>${l}</b><br><span class="sub">${ex}</span></span></label>`).join('')}</div>
      <div class="chks">${[['title', 'Titles'], ['artist', 'Artists'], ['album', 'Albums'], ['albumArtist', 'Album artists']].map(([f, l]) => `<label class="chk inline"><input type="checkbox" data-cf="${f}" ${fields[f] ? 'checked' : ''}> ${l}</label>`).join('')}</div>
      <div class="mb-map">${ch.slice(0, 300).flatMap(([, diff]) => diff).map(([a, b]) => `<div class="mb-row"><span class="o">${esc(a)}</span><span class="arr">→</span><span class="nw">${esc(b)}</span></div>`).join('') || '<p class="sub" style="padding:10px">Nothing to change.</p>'}</div>
      <div class="btns"><span class="grow"></span><button class="btn btn-ghost" data-d="cancel">Cancel</button><button class="btn btn-green" data-d="go" ${ch.length ? '' : 'disabled'}>Fix ${plural(ch.length, 'song', 'songs')}</button></div>`;
  };
  const d = dialog(draw(), 'wide');
  d.addEventListener('change', e => {
    if (e.target.name === 'cm') mode = e.target.value;
    if (e.target.dataset.cf) fields[e.target.dataset.cf] = e.target.checked;
    d.innerHTML = draw();
  });
  modalBg.onclick = e => {
    const b = e.target.closest('[data-d]')?.dataset.d;
    if (b === 'cancel' || e.target === modalBg) { closeDialog(); return; }
    if (b !== 'go') return;
    S.prefs.caseMode = mode; S.prefs.caseFields = fields; savePrefs();
    const ups = changes().map(x => x[0]);
    closeDialog();
    writeTags(ups);
  };
}

/* ───────────── Undoing "Organize files" ───────────── */
async function undoOrganize() {
  const last = S.prefs.lastOrganize; if (!last?.moves?.length) return;
  const back = last.moves.filter(m => S.byPath.has(m.to)).map(m => ({ from: m.to, to: m.from }));
  if (!back.length) { toast('Those files were moved again or are gone'); delete S.prefs.lastOrganize; savePrefs(); return; }
  if (!await confirmBox('Undo the last organize?', `${plural(back.length, 'file goes', 'files go')} back to where ${back.length === 1 ? 'it was' : 'they were'}, with their .lrc files and covers.`, 'Undo')) return;
  const r = await runMoves(back);
  if (!r) return;
  delete S.prefs.lastOrganize; savePrefs();
  toast(r.errors.length ? `${r.moved.length} moved back, ${r.errors.length} failed: ${r.errors[0]}` : plural(r.moved.length, 'file moved back', 'files moved back'), 5000);
}

/* ───────────── Spotify playlists (Exportify CSV) ───────────── */
function parseCsv(text) {
  const rows = []; let row = [], cell = '', q = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (q) { if (c === '"') { if (text[i + 1] === '"') { cell += '"'; i++; } else q = false; } else cell += c; }
    else if (c === '"') q = true;
    else if (c === ',') { row.push(cell); cell = ''; }
    else if (c === '\n' || c === '\r') { if (c === '\r' && text[i + 1] === '\n') i++; row.push(cell); rows.push(row); row = []; cell = ''; }
    else cell += c;
  }
  if (cell || row.length) { row.push(cell); rows.push(row); }
  return rows.filter(r => r.some(x => x.trim()));
}
function matchSpotify(title, artists, ms) {
  for (const a of [artists.join(', '), ...artists]) { const i = findTrack(a, title); if (i !== undefined) return i; }
  // Same title and length, artist written differently
  const nt = normTitle(title), names = artists.map(normKey);
  let best;
  S.tracks.forEach((t, i) => {
    if (best !== undefined || normTitle(t.title) !== nt) return;
    const near = !ms || Math.abs(t.duration * 1000 - ms) < 4000;
    const art = names.some(n => normKey(t.artist).includes(n) || normKey(t._owner).includes(n));
    if (near && art) best = i;
  });
  return best;
}
async function importSpotify() {
  const files = await invoke('pick_csv_files');
  if (!files.length) return;
  const report = [];
  for (const f of files) {
    const rows = parseCsv(f.text); if (rows.length < 2) continue;
    const h = rows[0].map(x => x.toLowerCase().trim());
    const col = (...res) => h.findIndex(x => res.some(r => r.test(x)));
    const ti = col(/^track name$/, /^name$/, /title/), ai = col(/^artist name/, /artist/), di = col(/duration/);
    if (ti < 0 || ai < 0) { report.push({ name: f.name, error: "doesn't look like an Exportify file" }); continue; }
    const found = [], missing = [];
    for (const r of rows.slice(1)) {
      const title = r[ti] || '', artists = (r[ai] || '').split(/\s*[,;]\s*/).filter(Boolean), ms = +r[di] || 0;
      if (!title) continue;
      const i = matchSpotify(title, artists, ms);
      if (i === undefined) missing.push(`${artists.join(', ')} — ${title}`); else found.push(S.tracks[i].path);
    }
    if (found.length) S.prefs.playlists.push({ id: Date.now().toString(36) + report.length, name: f.name, tracks: found, created: Date.now(), description: 'Imported from Spotify' });
    report.push({ name: f.name, found: found.length, missing });
  }
  savePrefs(); renderSidebar();
  const d = dialog(`<h2>Spotify playlists imported</h2>
    <div class="sp-report">${report.map((r, k) => `<div class="sp-pl"><div class="row"><b>${esc(r.name)}</b><span class="grow"></span><span class="sub">${r.error ? esc(r.error) : `${r.found} of ${r.found + r.missing.length} songs`}</span></div>
      ${r.missing?.length ? `<details><summary>${plural(r.missing.length, "song you don't have", "songs you don't have")}</summary><div class="sp-miss">${r.missing.map(esc).join('<br>')}</div></details>` : ''}</div>`).join('')}</div>
    <div class="btns">${report.some(r => r.missing?.length) ? `<button class="btn btn-ghost" data-d="copy">Copy the missing list</button>` : ''}<span class="grow"></span><button class="btn btn-green" data-d="ok">Done</button></div>`, 'wide');
  modalBg.onclick = e => {
    const b = e.target.closest('[data-d]')?.dataset.d;
    if (b === 'copy') { navigator.clipboard.writeText(report.filter(r => r.missing?.length).map(r => `${r.name}\n${r.missing.join('\n')}`).join('\n\n')).then(() => toast('Copied')); return; }
    if (b === 'ok' || e.target === modalBg) closeDialog();
  };
}

/* ───────────── Updates (GitHub releases) ───────────── */
const UPD = { info: null, checking: false, installing: false, ready: null };
const installLabel = () => ({ portable: 'Portable', installed: 'Installed', mac: 'macOS app', appimage: 'AppImage', package: 'Linux package' })[S.appInfo?.install] || '';
function updateStatus() {
  const u = UPD.info;
  if (UPD.installing) return 'Downloading the update…';
  if (UPD.ready === 'restart') return 'Update installed: restart Wax to use it';
  if (UPD.checking) return 'Checking for updates…';
  if (!u) return '';
  if (u.newer) return u.asset ? `Version ${u.latest} is available` : `Version ${u.latest} is out, but not for this system yet`;
  return "You're up to date";
}
function updateButton() {
  if (UPD.ready === 'restart') return `<button class="btn btn-green btn-sm" data-action="upd-restart">Restart</button>`;
  if (UPD.info?.newer && UPD.info.asset) return `<button class="btn btn-ghost btn-sm" data-action="upd-notes">What's new</button> <button class="btn btn-green btn-sm" data-action="upd-install" ${UPD.installing ? 'disabled' : ''}>Update to ${esc(UPD.info.latest)}</button>`;
  return `<button class="btn btn-ghost btn-sm" data-action="upd-check" ${UPD.checking ? 'disabled' : ''}>Check for updates</button>`;
}
async function checkUpdates(manual = false) {
  UPD.checking = true; if (route.view === 'settings') rerender();
  try {
    UPD.info = await invoke('check_update');
    S.prefs.updateCheckedAt = Date.now(); savePrefs();
    if (UPD.info.newer && UPD.info.asset && !manual) toast(`Wax ${UPD.info.latest} is available · Settings → About`, 6000);
    if (manual && !UPD.info.newer) toast("You're up to date");
  } catch (e) { if (manual) toast("Couldn't check for updates: " + e, 5000); }
  UPD.checking = false; if (route.view === 'settings') rerender();
}
async function installUpdate() {
  if (UPD.installing || busy()) return;
  UPD.installing = true; if (route.view === 'settings') rerender();
  const task = startTask(`Updating Wax to ${UPD.info.latest}`);
  const off = await TAURI.event.listen('update-progress', e => task.update(Math.round(e.payload.done / 1048576), Math.round((e.payload.total || 0) / 1048576), 'MB'));
  try {
    const next = await invoke('install_update');
    task.finish();
    if (next === 'restart') { UPD.ready = 'restart'; toast('Update installed · restart Wax to use it', 6000); }
    else if (next === 'exit') { toast('Wax closes to finish the update and opens again by itself', 4000); setTimeout(() => appWin.close(), 1500); }
    else if (next === 'opened') toast('The installer opened: follow it to finish the update', 6000);
  } catch (e) { task.finish("Couldn't update: " + e, 7000); }
  off(); UPD.installing = false;
  if (route.view === 'settings') rerender();
}
function updateNotes() {
  const u = UPD.info; if (!u) return;
  dialog(`<h2>Wax ${esc(u.latest)}</h2><div class="upd-notes">${u.notes ? esc(u.notes).replace(/\n/g, '<br>') : '<p class="sub">No notes for this version.</p>'}</div>
    <div class="btns"><span class="lnk" data-action="open-link" data-arg="${esc(u.page)}">See it on GitHub</span><span class="grow"></span><button class="btn btn-ghost" data-d="x">Close</button><button class="btn btn-green" data-d="go">Update</button></div>`, 'wide');
  modalBg.onclick = e => {
    const l = e.target.closest('[data-action="open-link"]'); if (l) { invoke('open_link', { url: l.dataset.arg }); return; }
    const b = e.target.closest('[data-d]')?.dataset.d;
    if (b === 'go') { closeDialog(); installUpdate(); } else if (b || e.target === modalBg) closeDialog();
  };
}
// Once a day, a few seconds after starting
setTimeout(() => {
  if (S.prefs.autoUpdate !== false && Date.now() - (S.prefs.updateCheckedAt || 0) > 20 * 3600 * 1000) checkUpdates(false);
}, 8000);

/* ───────────── Cleaning the image cache ───────────── */
async function cleanImageCache(manual = false) {
  // Never with an empty library (e.g. the music drive is unplugged): it would delete everything
  if (!S.tracks.length) { if (manual) toast('Your library is empty: nothing is cleaned'); return; }
  const albums = S.albums.map(a => ({ path: realPath(S.tracks[a.tracks[0]]), shared: !!a.shared }));
  const artists = S.artists.map(a => a.name);
  const custom = [...Object.values(S.prefs.coverOverrides || {}), ...Object.values(S.prefs.artistOverrides || {}), ...S.prefs.playlists.map(p => p.image)].filter(Boolean);
  const r = await invoke('clean_image_cache', { albums, artists, custom }).catch(() => null);
  if (!r) return;
  S.prefs.cacheCleanedAt = Date.now(); savePrefs();
  if (manual) toast(r.files ? `Removed ${plural(r.files, 'unused image', 'unused images')} (${(r.bytes / 1048576).toFixed(1)} MB)` : 'Nothing to clean: every saved image is in use', 4000);
}
setTimeout(() => { if (Date.now() - (S.prefs.cacheCleanedAt || 0) > 7 * 86400 * 1000) cleanImageCache(false); }, 60000);

/* ───────────── Actions ───────────── */
const extraActionBase = extraAction;
extraAction = function (a, arg) {
  switch (a) {
    case 'pl-edit': editPlaylistDetails(arg); return;
    case 'clean-images': cleanImageCache(true); return;
    case 'upd-check': checkUpdates(true); return;
    case 'upd-install': installUpdate(); return;
    case 'upd-notes': updateNotes(); return;
    case 'upd-restart': invoke('restart_app'); return;
    case 'spotify-import': importSpotify(); return;
    case 'aeq-remove': delete S.prefs.autoeq; savePrefs(); applyEq(); rerender(); return;
    case 'wrapped-save': {
      const idxs = lists.get('wrapped-top') || []; if (!idxs.length) return;
      const pl = { id: Date.now().toString(36), name: `Your Top Songs ${arg}`, tracks: idxs.map(i => S.tracks[i].path), created: Date.now(), description: `Your most played songs of ${arg}` };
      S.prefs.playlists.push(pl); savePrefs(); renderSidebar(); toast(`Saved as "${pl.name}"`);
      return;
    }
  }
  return extraActionBase(a, arg);
};

extrasReady();
