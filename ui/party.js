'use strict';
/* Listen Together: host a session (others hear what you play, in sync) or join someone's.
   The Rust side (party.rs) serves the host's songs and copies them to the guests. */

ICONS.people = K('M6 3.2a2.3 2.3 0 1 0 0 4.6 2.3 2.3 0 0 0 0-4.6zM1.8 13c.4-2.3 2-3.5 4.2-3.5s3.8 1.2 4.2 3.5M10.8 3.4a2.2 2.2 0 0 1 0 4.2M12 9.7c1.2.4 2 1.5 2.2 3.3', 1.2);
ICONS.copy = K('M5.5 5.5h8v8h-8zM10.5 5.5v-3h-8v8h3', 1.2);
ICONS.link = K('M6.8 9.2l2.4-2.4M7.6 4.6l1.3-1.3a2.6 2.6 0 0 1 3.7 3.7l-1.3 1.3M8.4 11.4l-1.3 1.3a2.6 2.6 0 0 1-3.7-3.7l1.3-1.3', 1.3);

// What a guest may do (changes which search boxes the page has)
const partyReqKey = () => `${!!Party.guest?.state.allowRequests}/${!!Party.guest?.state.allowShares}`;
const partyMode = () => Party.host ? 'host' : Party.guest ? 'guest' : 'idle';
const partyName = () => (S.prefs.partyName || '').trim() || 'Wax listener';
const partySongs = () => S.tracks.map((t, i) => ({
  id: i, title: t.title || '', artist: t.artist || '', album: t.album || '', duration: t.duration || 0,
  path: realPath(t), startMs: t.cue?.startMs || 0, endMs: t.cue?.endMs || 0,
}));
const copyText = (text, what = 'Copied') => navigator.clipboard.writeText(text).then(() => toast(what));

function partyMarkSidebar() {
  const live = $('#sb-party-live');
  if (live) live.textContent = Party.host ? (Party.host.guests.length ? `${Party.host.guests.length}` : 'Live') : Party.guest ? 'Live' : '';
}
function partyRender() {
  partyMarkSidebar();
  if (route.view !== 'party') return;
  // Same kind of page (hosting / joined): only the live part changes, so typing isn't interrupted
  const box = $('#party-body');
  if (box && box.dataset.mode === partyMode() && box.dataset.req === partyReqKey()) { box.innerHTML = partyBody(); paintIcons(box); }
  else rerender();
}

/* ───────────── Hosting ───────────── */

async function partyHost() {
  if (Party.guest) { toast('Leave the session you joined first'); return; }
  if (!S.tracks.length) { toast('Add your music first: the guests listen to your songs'); return; }
  try {
    const info = await invoke('party_host', { name: partyName(), songs: partySongs() });
    Party.host = { ...info, internet: null, guests: [] };
    Party.catRef = S.tracks;
    partyPush();
    partyRender();
  } catch (e) { toast(`Couldn't start the session: ${e}`, 5000); }
}
async function partyStop() {
  if (!Party.host) return;
  Party.host = null;
  Party.shared.clear(); Party.sharedQ = [];
  if (Party.sharedNow != null) { Party.sharedNow = null; audio.pause(); updateNowPlaying(); }
  refreshQueueView();
  partyRender();
  await invoke('party_stop').catch(() => {});
  toast('Session ended');
}

// Everything the guests need to follow the host's player
let partyT = 0;
function partyHostSync() { clearTimeout(partyT); partyT = setTimeout(partyPush, 120); }
async function partyPush() {
  if (!Party.host) return;
  if (S.tracks !== Party.catRef) {   // the library was scanned again: new song numbers
    Party.catRef = S.tracks;
    await invoke('party_catalog', { songs: partySongs() }).catch(() => {});
  }
  const t = S.tracks[P.current], sh = Party.shared.get(Party.sharedNow);
  const cover = t ? (await albumCover(albumKeyOf(t)))?.path || null : null;
  // Same order takeNext() plays them in: your queue, the songs guests sent, then the list
  const queue = [...P.upNext.filter(i => S.tracks[i]), ...Party.sharedQ.map(x => x.shared),
    ...P.order.slice(P.pos + 1).map(o => P.ctx[o]).filter(i => S.tracks[i])].slice(0, 30);
  invoke('party_update', { state: {
    track: t ? P.current : sh ? sh.song.id : null, posMs: Math.round((audio.currentTime || 0) * 1000), paused: audio.paused || (!t && !sh),
    queue, allowRequests: S.prefs.partyRequests !== false, allowShares: S.prefs.partyShares !== false, cover, compress: S.prefs.partyQuality === 'mp3',
  } }).catch(() => {});
}
setInterval(() => { if (Party.host) partyPush(); }, 10000);

TAURI.event.listen('party', e => {
  const v = e.payload, h = Party.host;
  if (!h) return;
  if (v.kind === 'guests') {
    const before = new Set(h.guests.map(g => g.name)), now = new Set(v.guests.map(g => g.name));
    for (const g of v.guests) if (!before.has(g.name)) toast(`${g.name} joined the session`);
    for (const n of before) if (!now.has(n)) toast(`${n} left the session`);
    h.guests = v.guests;
  } else if (v.kind === 'internet') {
    h.internet = v;
  } else if (v.kind === 'request') {
    const t = S.tracks[v.id]; if (!t) return;
    if (P.current < 0) playList([v.id], 0);
    else { P.upNext.push(v.id); refreshQueueView(); syncNext(); }
    toast(`${v.from} added "${t.title}" to the queue`, 4000);
  } else if (v.kind === 'shared') {
    Party.shared.set(v.song.id, { song: v.song, path: v.path });
    Party.sharedQ.push({ shared: v.song.id });
    toast(`${v.song.from} shared "${v.song.title}"`, 4000);
    if (P.current < 0 && Party.sharedNow == null) next();
    else { refreshQueueView(); syncNext(); partyHostSync(); }
  }
  partyRender();
});

/* ───────────── Joining ───────────── */

async function partyJoin(code) {
  if (Party.host) { toast('End your own session first'); return; }
  if (!code.trim()) { toast('Type the code the host gave you'); return; }
  const btn = $('[data-action="party-join"]');
  if (btn) { btn.disabled = true; btn.textContent = 'Joining…'; }
  let state;
  try { state = await invoke('party_join', { code, name: partyName() }); }
  catch (e) { toast(String(e), 6000); if (btn) { btn.disabled = false; btn.textContent = 'Join'; } return; }
  // Your own player steps aside while you listen with others
  finishListen();
  audio.pause();
  P.current = -1; P.ctx = []; P.order = []; P.upNext = []; P.pos = -1;
  const g = Party.guest = { state, seq: state.seq, offset: 0, loaded: null, loadSeq: 0, track: null, localPaused: false, fails: 0, gen: Date.now(), lastSeek: 0, status: '' };
  document.body.classList.add('party-guest');
  partyClock();
  guestApply(state);
  guestPoll(g.gen);
  toast(`You joined ${state.name}'s session`);
}

function partyLeave(msg = 'You left the session') {
  if (!Party.guest) return;
  Party.guest = null;
  invoke('party_leave').catch(() => {});
  audio.pause();
  audio.track = null;
  document.body.classList.remove('party-guest');
  updateNowPlaying();
  syncPlayButton();
  tCur.textContent = tDur.textContent = '0:00'; seek.set(0);
  partyRender();
  toast(msg, 4000);
}
// Playing one of your own songs while in a session
// `song`: the song they tried to play — it can go to the session for everyone instead
function partyAskLeave(then, song = -1) {
  const s = Party.guest.state, t = S.tracks[song];
  const send = t && s.allowShares && !t.cue;
  dialog(send
    ? `<h3>Play it for everyone?</h3><p>You're listening with ${esc(s.name)}. Send <b>${esc(t.title)}</b> to the session and everyone hears it when its turn comes. You stay in the session.</p>
      <div class="btns"><button class="btn btn-ghost" data-d="yes">Leave and play</button><span class="grow"></span><button class="btn btn-ghost" data-d="no">Cancel</button><button class="btn btn-green" data-d="send">Send to the session</button></div>`
    : `<h3>Leave the session?</h3><p>You're listening with ${esc(s.name)}. To play your own music, leave the session first.${t && !s.allowShares ? ` (${esc(s.name)} isn't taking songs from guests right now.)` : ''}</p>
      <div class="btns"><span class="grow"></span><button class="btn btn-ghost" data-d="no">Stay</button><button class="btn btn-green" data-d="yes">Leave and play</button></div>`, send ? 'wide' : '');
  modalBg.onclick = e => {
    const d = e.target.closest('[data-d]')?.dataset.d;
    if (!d && e.target !== modalBg) return;
    closeDialog();
    if (d === 'send') partyShare([song]);
    if (d === 'yes') { partyLeave(); then(); }
  };
}
// Play/pause for a guest: stops hearing it here, without touching the host
function partyToggleListen() {
  const g = Party.guest; if (!g) return;
  g.localPaused = !g.localPaused;
  if (g.localPaused) audio.pause(); else guestSync(true);
  guestStatus();
}

async function partyClock() {
  const g = Party.guest; if (!g) return;
  const c = await invoke('party_clock').catch(() => null);
  if (c && Party.guest === g) g.offset = c.offset;
}
setInterval(() => { if (Party.guest) partyClock(); }, 30000);

async function guestPoll(gen) {
  while (Party.guest?.gen === gen) {
    const g = Party.guest;
    try {
      const s = await invoke('party_poll', { since: g.seq });
      if (Party.guest?.gen !== gen) return;
      g.fails = 0;
      guestApply(s);
    } catch {
      if (Party.guest?.gen !== gen) return;
      if (++g.fails >= 8) { partyLeave(`Lost the connection to ${g.state.name}'s session`); return; }
      g.status = 'Reconnecting…'; guestStatus();
      await sleepMs(2500);
    }
  }
}

// Where the host is in the song right now (seconds)
function guestPos() {
  const s = Party.guest?.state; if (!s?.track) return 0;
  const ms = s.paused ? s.posMs : s.posMs + Date.now() + Party.guest.offset - s.at;
  return Math.max(0, Math.min(ms / 1000, s.track.duration || Infinity));
}

function guestApply(s) {
  const g = Party.guest; if (!g) return;
  const prevId = g.state?.track?.id;
  g.state = s; g.seq = s.seq;
  if (s.ended) { partyLeave(`${s.name} ended the session`); return; }
  if (!s.track) {
    if (g.loaded != null) { audio.pause(); g.loaded = null; g.track = null; }
    guestNowPlaying(null);
  } else if (g.loaded !== s.track.id) guestLoad(s.track);
  else guestSync(true);
  // The next song starts copying now, so it's ready when it's its turn
  if (s.queue[0]) invoke('party_fetch', { song: s.queue[0], ratio: -1 }).catch(() => {});
  if (prevId !== s.track?.id || route.view === 'party') { partyRender(); guestStatus(); }
}

async function guestLoad(song) {
  const g = Party.guest;
  const my = ++g.loadSeq;
  g.loaded = song.id; g.track = null;
  guestNowPlaying(song);
  audio.pause();
  const cue = song.startMs || song.endMs;
  // Enough of the file to start where the host is (a CUE sheet's file holds other songs too: all of it)
  const ratio = cue ? 1 : Math.min(1, guestPos() / (song.duration || 1) + 0.03);
  g.status = 'Loading the song…'; guestStatus();
  const prog = setInterval(async () => {
    const p = await invoke('party_progress', { id: song.id }).catch(() => 0);
    if (Party.guest === g && g.loadSeq === my && !g.track) { g.status = `Loading the song… ${Math.round(p * 100)}%`; guestStatus(); }
  }, 400);
  let path = null;
  try { path = await invoke('party_fetch', { song, ratio }); }
  catch (e) { g.status = String(e); }
  clearInterval(prog);
  if (Party.guest !== g || g.loadSeq !== my) return;
  if (!path) { guestStatus(); return; }
  g.track = { path, title: song.title, artist: song.artist, album: song.album, duration: song.duration,
    cue: cue ? { file: path, startMs: song.startMs, endMs: song.endMs } : null };
  g.status = '';
  const play = !g.state.paused && !g.localPaused;
  audio.load(g.track, { autoplay: play, startAt: guestPos(), gainDb: 0 });
  g.lastSeek = performance.now();
  guestStatus();
}

// Keeps this player at the host's position (more than ⅓ s off → jump)
function guestSync(force = false) {
  const g = Party.guest, s = g?.state;
  if (!g?.track || g.loaded !== s?.track?.id) return;
  const want = guestPos();
  const play = !s.paused && !g.localPaused;
  if (play && audio.paused) { audio.seek(want); audio.play(); g.lastSeek = performance.now(); return; }
  if (!play && !audio.paused) audio.pause();
  if (!force && performance.now() - g.lastSeek < 2000) return;   // let a jump settle before measuring again
  if (Math.abs((audio.currentTime || 0) - want) > (force ? 0.3 : 0.45)) { audio.seek(want); g.lastSeek = performance.now(); }
}
setInterval(() => { if (Party.guest) guestSync(false); }, 1000);

// The player bar for a song that isn't in your library (the host's, or one a guest sent)
function showSong(song, cover = null) {
  document.body.classList.toggle('empty-np', !song);
  npTitle.textContent = song?.title || '';
  delete npTitle.dataset.go; delete npTitle.dataset.arg;
  npArtist.textContent = song ? song.artist + (song.from ? ` · shared by ${song.from}` : '') : '';
  tDur.textContent = fmt(song?.duration || 0);
  for (const el of [npCover, sbCover]) {
    el.dataset.cover = ''; el.style.backgroundImage = cover ? `url("${cover}")` : ''; el.classList.toggle('has-img', !!cover);
  }
  document.title = song ? `${song.title} · ${song.artist}` : 'Wax';
}
function partyNowPlaying() {
  const g = Party.guest;
  const song = g ? g.state.track : Party.shared.get(Party.sharedNow)?.song;
  if (!song) return false;
  showSong(song, g && g.loaded === song.id ? g.cover : null);
  return true;
}

function guestNowPlaying(song) {
  if (Party.guest) Party.guest.cover = null;
  showSong(song);
  if (!song) return;
  invoke('party_cover', { id: song.id }).then(p => {
    if (!p || Party.guest?.loaded !== song.id) return;
    const url = `${TAURI.core.convertFileSrc(p)}?v=${Date.now()}`;
    Party.guest.cover = url;
    for (const el of [npCover, sbCover]) { el.style.backgroundImage = `url("${url}")`; el.classList.add('has-img'); }
    const big = $('#party-cover'); if (big) { big.style.backgroundImage = `url("${url}")`; big.classList.add('has-img'); }
  });
}

/* ───────────── Songs guests send from their own libraries ───────────── */

// Host: plays one (it isn't in the library: no stats, no scrobbling)
function partyPlayShared(item, autoplay = true) {
  const sh = Party.shared.get(item.shared);
  finishListen();
  P.current = -1;
  Party.sharedNow = sh ? item.shared : null;
  if (!sh) { next(true); return; }
  const s = sh.song;
  audio.load({ path: sh.path, title: s.title, artist: s.artist, album: s.album, duration: s.duration }, { autoplay, startAt: 0, gainDb: 0 });
  updateNowPlaying(); markPlaying(); syncNext(); refreshQueueView(); partyHostSync();
}
const partySharedNowHtml = () => {
  const s = Party.shared.get(Party.sharedNow)?.song;
  return s ? `<div class="party-list">${songRow(s)}</div>` : '';
};
const partySharedQueueHtml = () => `<div class="sec-h"><h2>Shared by guests</h2></div><div class="party-list">${Party.sharedQ.map(x => {
  const s = Party.shared.get(x.shared)?.song;
  return s ? songRow(s, `<button class="ib" title="Remove from the queue" data-action="party-unshare" data-arg="${s.id}">${ic('x', 14)}</button>`) : '';
}).join('')}</div>`;

// Guest: sends songs to the host
function partyShareItem(idxs) {
  const g = Party.guest;
  return { label: `Add to ${g.state.name}'s session`, act: () => partyShare(idxs), disabled: !g.state.allowShares };
}
async function partyShare(idxs) {
  for (const i of idxs) {
    const g = Party.guest, t = S.tracks[i]; if (!g || !t) return;
    if (t.cue) { toast(`"${t.title}" is part of a one-file album (CUE) and can't be sent`, 4000); continue; }
    toast(`Sending "${t.title}" to ${g.state.name}…`, 120000);
    try {
      await invoke('party_share', { path: t.path, title: t.title || '', artist: t.artist || '', album: t.album || '', duration: t.duration || 0, compress: !!g.state.compress });
      toast(`"${t.title}" is in ${g.state.name}'s queue`);
    } catch (e) { toast(String(e), 5000); return false; }
  }
  return true;
}

function guestStatus() {
  const g = Party.guest, el = $('#party-status'); if (!g || !el) return;
  const s = g.state;
  el.textContent = g.status || (!s.track ? `${s.name} isn't playing anything right now` : g.localPaused ? 'Paused here (the session goes on)'
    : s.paused ? `Paused by ${s.name}` : 'In sync');
}

async function guestSearch(q) {
  const box = $('#party-hits'); if (!box || !Party.guest) return;
  if (q.trim().length < 2) { box.innerHTML = ''; return; }
  const my = (guestSearch.seq = (guestSearch.seq || 0) + 1);
  const hits = await invoke('party_search', { query: q }).catch(() => []);
  if (my !== guestSearch.seq || !$('#party-hits')) return;
  $('#party-hits').innerHTML = hits.length ? `<div class="party-list">${hits.map(h => songRow(h, `<button class="btn btn-ghost btn-sm" data-action="party-add" data-arg="${h.id}">Add</button>`)).join('')}</div>`
    : '<p class="sub">Nothing found.</p>';
}

// Guest: their own songs, to send to the session
function mineSearch(q) {
  const box = $('#party-mine-hits'); if (!box) return;
  if (q.trim().length < 2) { box.innerHTML = ''; return; }
  const idxs = filterIdx(S.tracks.map((_, i) => i), q).filter(i => !S.tracks[i].cue).slice(0, 40);
  box.innerHTML = idxs.length ? `<div class="party-list">${idxs.map(i => { const t = S.tracks[i];
    return songRow({ title: t.title, artist: t.artist, album: t.album, duration: t.duration }, `<button class="btn btn-ghost btn-sm" data-action="party-send" data-arg="${i}">Send</button>`); }).join('')}</div>`
    : '<p class="sub">Nothing found in your music.</p>';
}

/* ───────────── The view ───────────── */

const songRow = (s, extra = '') => `<div class="party-song"><div class="t">${esc(s.title)}<div class="s">${esc(s.artist)}${s.album ? ` · ${esc(s.album)}` : ''}${s.from ? ` · <span class="party-from">shared by ${esc(s.from)}</span>` : ''}</div></div><span class="d">${fmt(s.duration)}</span>${extra}</div>`;
const sharesBox = () => `<label class="chk"><input type="checkbox" id="party-shares" ${S.prefs.partyShares !== false ? 'checked' : ''}> Let guests send songs from their own libraries</label>`;
// A code, with buttons to copy it and to copy the link for web browsers
const codeBox = (code, link) => `<div class="party-code"><span>${esc(code)}</span>
  <button class="ib" title="Copy the code" data-action="party-copy" data-arg="${esc(code)}" data-msg="Code copied">${ic('copy', 15)}</button>
  <button class="ib" title="Copy the link for web browsers" data-action="party-copy" data-arg="${esc(link)}" data-msg="Web link copied">${ic('link', 15)}</button></div>`;

const qualityPick = () => `<label class="party-quality">Sound quality
    <select class="sel" id="party-quality">${[['original', 'Original files'], ['mp3', 'Lighter (MP3 320 kbps)']].map(([v, l]) =>
      `<option value="${v}" ${(S.prefs.partyQuality || 'original') === v ? 'selected' : ''}>${l}</option>`).join('')}</select></label>
  <p class="sub dim">Lighter sends lossless songs (FLAC, WAV…) about 3 times smaller, with almost no audible difference. Use it when your upload is slow or many people join.</p>`;

function partyBody() {
  const h = Party.host, g = Party.guest;
  if (h) {
    const net = h.internet;
    const internet = !net ? '<p class="sub"><span class="spin"></span> Asking your router to let people from outside in…</p>'
      : net.ok ? codeBox(net.code, net.link) + '<p class="sub">Works from anywhere. Share it only with people you trust.</p>'
      : `<p class="sub">${esc(net.message)} ${h.ipv6Code ? 'Try the IPv6 code below.' : `People on your network can still join. To let others in, forward port ${net.port} (TCP) to this computer in your router's settings.`}</p>`;
    const ipv6 = h.ipv6Code ? `<div class="party-l">Anywhere else, over IPv6</div>${codeBox(h.ipv6Code, h.ipv6Link)}
        <p class="sub">Works when the other person's connection has IPv6 too, and your router lets connections in. If one code doesn't work, try the other.</p>` : '';
    return `<div class="party-cards">
      <div class="party-card">
        <h2>Invite people</h2>
        <div class="party-l">Same Wi-Fi or network as you</div>${codeBox(h.code, h.link)}
        <div class="party-l">Anywhere else (other homes, other countries)</div>${internet}${ipv6}
        <p class="sub">Friends without Wax can listen in a web browser, on a computer or a phone: copy the link with ${ic('link', 13)} instead of the code.</p>
        <p class="sub dim">If your system asks whether Wax can use the network, allow it so others can connect.</p>
      </div>
      <div class="party-card">
        <h2>Listening with you${h.guests.length ? ` (${h.guests.length})` : ''}</h2>
        ${h.guests.length ? `<div class="party-people">${h.guests.map(p => `<div class="party-person">${ic('person', 16)}<span>${esc(p.name)}</span>${p.web ? '<span class="tag">Browser</span>' : ''}</div>`).join('')}</div>`
          : '<p class="sub">Nobody yet. Share a code to invite people.</p>'}
        <p class="sub">They hear what you play, when you play it. Pause, skip or change songs as usual.</p>
        <label class="chk"><input type="checkbox" id="party-req" ${S.prefs.partyRequests !== false ? 'checked' : ''}> Let guests add songs to your queue</label>
        ${sharesBox()}
        ${qualityPick()}
        <div><button class="btn btn-ghost" data-action="party-stop">End session</button></div>
      </div></div>`;
  }
  if (g) {
    const s = g.state, t = s.track;
    return `<div class="party-np">
        <div class="cv party-cover${g.cover && t ? ' has-img' : ''}" id="party-cover" style="${g.cover && t ? `background-image:url('${g.cover}')` : ''}"><span class="ph">${ic('note', 64)}</span></div>
        <div class="party-np-t">
          <div class="party-l">Now playing</div>
          <h2>${t ? esc(t.title) : 'Nothing yet'}</h2>
          <div class="sub">${t ? `${esc(t.artist)} · ${esc(t.album)}` : ''}</div>
          <p class="party-status" id="party-status"></p>
          <button class="btn btn-ghost" data-action="party-leave">Leave session</button>
        </div></div>
      <div class="party-cards">
        <div class="party-card"><h2>Up next</h2>${s.queue.length ? `<div class="party-list">${s.queue.slice(0, 15).map(q => songRow(q)).join('')}</div>` : '<p class="sub">Nothing queued.</p>'}</div>
        <div class="party-card"><h2>People</h2><div class="party-people">
          <div class="party-person">${ic('person', 16)}<span>${esc(s.name)}</span><span class="tag">Host</span></div>
          ${s.guests.map(p => `<div class="party-person">${ic('person', 16)}<span>${esc(p.name)}</span>${p.web ? '<span class="tag">Browser</span>' : ''}</div>`).join('')}</div>
          ${s.allowShares ? `<p class="sub">To play a song of your own for everyone, right-click it anywhere in your library and choose <b>Add to ${esc(s.name)}'s session</b>.</p>` : ''}</div>
      </div>`;
  }
  return '';
}

VIEWS.party = () => {
  const h = Party.host, g = Party.guest;
  setTimeout(guestStatus);
  if (g) return `<div class="page-h"><h1>Listening with ${esc(g.state.name)}</h1></div><div id="party-body" data-mode="${partyMode()}" data-req="${partyReqKey()}">${partyBody()}</div>
    ${g.state.allowRequests ? `<div class="sec-h"><h2>Add a song</h2></div>
      <label class="filter party-search">${ic('search', 13)}<input id="party-q" placeholder="Search ${esc(g.state.name)}'s music" spellcheck="false"></label><div id="party-hits"></div>` : ''}
    ${g.state.allowShares ? `<div class="sec-h"><h2>Play one of your songs</h2></div>
      <p class="sub party-hint">Send a song from your own library: everyone hears it when its turn comes.</p>
      <label class="filter party-search">${ic('search', 13)}<input id="party-mine" placeholder="Search your music" spellcheck="false"></label><div id="party-mine-hits"></div>` : ''}`;
  if (h) return `<div class="page-h"><h1>Listen Together <span class="live-tag">Live</span></h1></div><div id="party-body" data-mode="${partyMode()}" data-req="${partyReqKey()}">${partyBody()}</div>`;
  return `<div class="page-h"><h1>Listen Together</h1></div>
    <p class="sub party-intro">Listen to the same music at the same moment with friends. One person hosts and plays songs from their own library; everyone who joins hears them in sync. Wax connects the computers directly, with no other service in between.</p>
    <label class="party-name">Your name <input class="txt" id="party-name" value="${esc(S.prefs.partyName || '')}" placeholder="Wax listener" maxlength="40" spellcheck="false"></label>
    <div class="party-cards">
      <div class="party-card">
        <div class="party-ic">${ic('people', 30)}</div>
        <h2>Host a session</h2>
        <p class="sub">Share what you're playing. People on your network join with your code. People elsewhere can join too if your router allows it.</p>
        <label class="chk"><input type="checkbox" id="party-req" ${S.prefs.partyRequests !== false ? 'checked' : ''}> Let guests add songs to your queue</label>
        ${sharesBox()}
        ${qualityPick()}
        <div><button class="btn btn-green" data-action="party-host">Start session</button></div>
      </div>
      <div class="party-card">
        <div class="party-ic">${ic('speaker', 30)}</div>
        <h2>Join a session</h2>
        <p class="sub">Type the code the host gave you, or paste their link.</p>
        <input class="txt party-code-in" id="party-code" placeholder="ABCDEF-GHJKLM-NPQRST" spellcheck="false" autocomplete="off">
        <div><button class="btn btn-green" data-action="party-join">Join</button></div>
      </div>
    </div>`;
};

view.addEventListener('input', e => {
  if (e.target.id === 'party-name') { S.prefs.partyName = e.target.value.trim(); savePrefs(); }
  if (e.target.id === 'party-mine') { clearTimeout(mineSearch.t); mineSearch.t = setTimeout(() => mineSearch(e.target.value), 200); }
  if (e.target.id === 'party-q') { clearTimeout(guestSearch.t); guestSearch.t = setTimeout(() => guestSearch(e.target.value), 250); }
});
view.addEventListener('change', e => {
  if (e.target.id === 'party-req') { S.prefs.partyRequests = e.target.checked; savePrefs(); partyHostSync(); }
  if (e.target.id === 'party-shares') { S.prefs.partyShares = e.target.checked; savePrefs(); partyHostSync(); }
  if (e.target.id === 'party-quality') { S.prefs.partyQuality = e.target.value; savePrefs(); partyHostSync(); }
});
view.addEventListener('keydown', e => {
  if (e.key === 'Enter' && e.target.id === 'party-code') partyJoin(e.target.value);
});

addActions({
  'party-host': () => partyHost(),
  'party-stop': () => partyStop(),
  'party-join': () => partyJoin($('#party-code')?.value || ''),
  'party-leave': () => partyLeave(),
  'party-copy': arg => copyText(arg, $(`[data-action="party-copy"][data-arg="${CSS.escape(arg)}"]`)?.dataset.msg || 'Copied'),
  'party-unshare': arg => { Party.sharedQ = Party.sharedQ.filter(x => x.shared !== +arg); refreshQueueView(); syncNext(); partyHostSync(); },
  'party-send': arg => {
    const b = $(`[data-action="party-send"][data-arg="${arg}"]`); if (b) { b.disabled = true; b.textContent = 'Sending…'; }
    partyShare([+arg]).then(ok => { if (b) { b.textContent = ok ? 'Sent' : 'Send'; b.disabled = ok; } });
  },
  'party-add': arg => invoke('party_request', { id: +arg }).then(() => toast(`Added to ${Party.guest?.state.name}'s queue`)).catch(e => toast(String(e))),
});

// Closing Wax: the guests hear it's over
addEventListener('beforeunload', () => { if (Party.host) invoke('party_stop'); if (Party.guest) invoke('party_leave'); });
