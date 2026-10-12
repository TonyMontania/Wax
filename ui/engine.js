'use strict';
/* Bridge to the Rust audio engine.
   Mimics the <audio> interface (paused, currentTime, duration, play(), pause() and the
   play/pause/timeupdate/ended/error/loadedmetadata/seeked events) and adds:
     load(track, opts)        loads and plays a track
     setNext(track, opts)     preloads the next one (gapless / crossfade)
     setVolume(v), setEq(eq)
     'advanced' event         the engine moved on to the preloaded track (detail = track)
   If the engine isn't available or can't decode a file (e.g. OPUS),
   that track is played with the WebView's <audio>. */

class WaxAudio extends EventTarget {
  constructor() {
    super();
    this.rust = false;
    this.mode = 'html';
    this.html = new Audio();
    this.html.preload = 'auto';
    this.seq = 0;
    this.id = 0;
    this.track = null;
    this.next = null;
    this._paused = true;
    this._pos = 0;
    this._posAt = 0;
    this._dur = 0;
    this.vol = 0.8;
    this.gain = 1;
    for (const ev of ['play', 'pause', 'ended', 'error', 'timeupdate', 'loadedmetadata', 'playing', 'seeked']) {
      this.html.addEventListener(ev, () => { if (this.mode === 'html') this.emit(ev); });
    }
  }

  async init() {
    const T = window.__TAURI__;
    try { this.rust = await T.core.invoke('audio_available'); } catch { this.rust = false; }
    if (this.rust) await T.event.listen('audio', e => this.onEngine(e.payload));
    return this.rust;
  }

  emit(name, detail) { this.dispatchEvent(detail === undefined ? new Event(name) : new CustomEvent(name, { detail })); }
  invoke(cmd, args) { return window.__TAURI__.core.invoke(cmd, args).catch(e => console.warn(cmd, e)); }

  get paused() { return this.mode === 'html' ? this.html.paused : this._paused; }
  get currentTime() {
    if (this.mode === 'html') return this.html.currentTime;
    // The engine reports the position 4 times per second: interpolate in between
    const p = this._paused ? this._pos : this._pos + (performance.now() - this._posAt) / 1000;
    return this._dur ? Math.min(p, this._dur) : p;
  }
  set currentTime(s) { this.seek(s); }
  get duration() {
    if (this.mode === 'html') return isFinite(this.html.duration) ? this.html.duration : (this.track?.duration || 0);
    return this._dur;
  }

  spec(track, gainDb = 0, crossfadeMs = 0) {
    // CUE sheet tracks: the real file plus where the track starts and ends inside it
    const c = track.cue;
    return { id: ++this.seq, path: c ? c.file : track.path, gainDb, durationMs: Math.round((track.duration || 0) * 1000), crossfadeMs,
      startMs: c ? c.startMs : 0, endMs: c ? c.endMs : 0 };
  }

  load(track, { autoplay = true, startAt = 0, gainDb = 0 } = {}) {
    this.track = track;
    this.next = null;
    this._dur = track.duration || 0;
    this._pos = startAt;
    this._posAt = performance.now();
    this.gain = Math.pow(10, gainDb / 20);
    if (this.rust && !track._noEngine) {
      if (this.mode === 'html') { this.html.pause(); this.html.removeAttribute('src'); this.html.load(); }
      this.mode = 'rust';
      const s = this.spec(track, gainDb);
      this.id = s.id;
      this._paused = !autoplay;
      this.invoke('audio_play', { spec: s, startMs: Math.round(startAt * 1000), paused: !autoplay });
      this.emit('loadedmetadata');
      this.emit(autoplay ? 'play' : 'pause');
      this.emit('timeupdate');
    } else {
      this.loadHtml(track, autoplay, startAt);
    }
  }

  loadHtml(track, autoplay, startAt) {
    if (this.mode === 'rust') this.invoke('audio_stop');
    this.mode = 'html';
    this.id = ++this.seq;
    this.html.src = window.__TAURI__.core.convertFileSrc(track.path);
    this.applyHtmlVolume();
    if (startAt > 0) this.html.addEventListener('loadedmetadata', () => { this.html.currentTime = startAt; }, { once: true });
    if (autoplay) this.html.play().catch(e => { if (e.name !== 'AbortError') console.warn(e); });
  }

  play() {
    if (this.mode === 'html') return this.html.play();
    if (!this.track) return Promise.resolve();
    this._posAt = performance.now();
    this._paused = false;
    this.invoke('audio_pause', { paused: false });
    this.emit('play');
    return Promise.resolve();
  }
  pause() {
    if (this.mode === 'html') return this.html.pause();
    this._pos = this.currentTime;
    this._paused = true;
    this.invoke('audio_pause', { paused: true });
    this.emit('pause');
  }
  seek(s) {
    s = Math.max(0, s);
    if (this.mode === 'html') { this.html.currentTime = s; return; }
    this._pos = s;
    this._posAt = performance.now();
    this.invoke('audio_seek', { ms: Math.round(s * 1000) });
    this.emit('timeupdate');
    this.emit('seeked');
  }
  setVolume(v) {
    this.vol = v;
    this.applyHtmlVolume();
    if (this.rust) this.invoke('audio_volume', { volume: v });
  }
  applyHtmlVolume() { this.html.volume = Math.min(1, Math.max(0, this.vol * this.gain)); }
  setEq(settings) { if (this.rust) this.invoke('audio_eq', { settings }); }

  /** Preloads the next track in the engine (gapless / crossfade). */
  setNext(track, { gainDb = 0, crossfadeMs = 0 } = {}) {
    if (!this.rust || this.mode !== 'rust') { this.next = null; return; }
    if (!track || track._noEngine) { this.next = null; this.invoke('audio_next', { spec: null }); return; }
    const s = this.spec(track, gainDb, crossfadeMs);
    this.next = { id: s.id, track, gainDb };
    this.invoke('audio_next', { spec: s });
  }

  onEngine(ev) {
    switch (ev.kind) {
      case 'time':
        if (ev.id !== this.id || this.mode !== 'rust') return;
        this._pos = ev.pos / 1000;
        this._posAt = performance.now();
        this.emit('timeupdate');
        break;
      case 'advanced': {
        if (!this.next || ev.id !== this.next.id) return;
        const n = this.next;
        this.next = null;
        this.id = n.id;
        this.track = n.track;
        this._dur = n.track.duration || 0;
        this._pos = 0;
        this._posAt = performance.now();
        this.gain = Math.pow(10, n.gainDb / 20);
        this.emit('advanced', n.track);
        break;
      }
      case 'ended':
        if (ev.id !== this.id) return;
        this._paused = true;
        this.emit('pause');
        this.emit('ended');
        break;
      case 'error':
        if (ev.id === this.id && this.track?.cue) {
          // The WebView's player can't play just a part of a file
          this._paused = true; this.emit('pause'); this.emit('error');
        } else if (ev.id === this.id && this.track) {
          // The engine can't handle this format: use the WebView's player
          this.track._noEngine = true;
          this.loadHtml(this.track, !this._paused, this._pos);
        } else if (this.next && ev.id === this.next.id) {
          this.next.track._noEngine = true;
          this.next = null;
        }
        break;
    }
  }
}

window.WaxAudio = WaxAudio;
