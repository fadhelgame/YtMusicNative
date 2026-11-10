// Animated lyrics overlay, injected into music.youtube.com.
// Lyrics are fetched by the Rust host (window.ipc) and pushed back through
// window.__ytmLyrics.receive(). DOM is built without innerHTML because the
// page enforces Trusted Types.
(() => {
  if (window.top !== window || location.hostname !== 'music.youtube.com' || window.__ytmLyrics) return;

  const LEAD = 0.25;          // highlight slightly ahead of the audio
  const BROWSE_MS = 3000;     // pause auto-scroll after manual scrolling
  const ANCHOR = 0.3;         // active line sits at 30% of the view height

  const CSS = `
#aml-root{position:fixed;inset:0;z-index:2147483000;display:flex;overflow:hidden;color:#fff;
  font-family:"SF Pro Display","Segoe UI Variable Display","Segoe UI",system-ui,sans-serif;
  opacity:0;visibility:hidden;transform:translateY(24px);
  transition:opacity .35s ease,transform .45s cubic-bezier(.22,1,.36,1),visibility 0s linear .45s}
#aml-root.open{opacity:1;visibility:visible;transform:none;transition-delay:0s}
.aml-bg{position:absolute;inset:-20%;background:#111 center/cover no-repeat;
  filter:blur(70px) saturate(1.6) brightness(.55);animation:aml-drift 48s ease-in-out infinite alternate}
#aml-root:not(.open) .aml-bg,#aml-root:not(.open) .aml-dots i{animation-play-state:paused}
.aml-shade{position:absolute;inset:0;background:rgba(0,0,0,.38)}
@keyframes aml-drift{from{transform:scale(1) rotate(0deg)}to{transform:scale(1.25) rotate(14deg)}}
.aml-close{position:absolute;top:18px;right:22px;z-index:2;width:36px;height:36px;border:0;border-radius:50%;
  background:rgba(255,255,255,.14);color:#fff;font-size:20px;line-height:36px;cursor:pointer}
.aml-close:hover{background:rgba(255,255,255,.26)}
.aml-side{position:relative;flex:0 0 38%;display:flex;flex-direction:column;align-items:center;justify-content:center;
  padding:40px;box-sizing:border-box;min-width:0}
.aml-art{width:min(100%,46vh);aspect-ratio:1;border-radius:14px;object-fit:cover;background:#222;
  box-shadow:0 24px 70px rgba(0,0,0,.55)}
.aml-title{margin-top:26px;font-size:22px;font-weight:700;text-align:center;max-width:100%;
  overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.aml-artist{margin-top:6px;font-size:16px;opacity:.7;text-align:center;max-width:100%;
  overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.aml-main{position:relative;flex:1;min-width:0;display:flex;flex-direction:column}
.aml-view{position:relative;flex:1;overflow:hidden;padding:0 6vw 0 2vw;
  -webkit-mask-image:linear-gradient(transparent,#000 14%,#000 78%,transparent);
  mask-image:linear-gradient(transparent,#000 14%,#000 78%,transparent)}
.aml-line{--y:0px;--blur:0px;padding:.42em 0;font-size:clamp(24px,3.1vw,44px);font-weight:700;line-height:1.2;
  letter-spacing:-.01em;cursor:pointer;opacity:.32;transform-origin:left center;
  transform:translateY(var(--y)) scale(.96);filter:blur(var(--blur));
  transition:transform .7s cubic-bezier(.22,1,.36,1),opacity .45s ease,filter .45s ease;will-change:transform}
.aml-line.on{opacity:1;transform:translateY(var(--y)) scale(1);text-shadow:0 0 28px rgba(255,255,255,.28)}
.aml-line:hover{opacity:.75}
.aml-line.on:hover{opacity:1}
.aml-dots{display:flex;gap:.28em;height:1.2em;align-items:center;opacity:0;cursor:default}
.aml-dots.on{opacity:1}
.aml-dots i{width:.34em;height:.34em;border-radius:50%;background:#fff;animation:aml-pulse 1.4s ease-in-out infinite}
.aml-dots i:nth-child(2){animation-delay:.2s}.aml-dots i:nth-child(3){animation-delay:.4s}
@keyframes aml-pulse{0%,100%{opacity:.3;transform:scale(.8)}50%{opacity:1;transform:scale(1.15)}}
#aml-root.browsing .aml-line{--blur:0px!important;transition-delay:0s!important}
#aml-root.plain .aml-view{overflow-y:auto;padding-top:12vh;padding-bottom:20vh;scrollbar-width:none}
#aml-root.plain .aml-line{opacity:.85;transform:none;filter:none;cursor:default;font-size:clamp(20px,2.3vw,32px);min-height:.6em}
.aml-status{position:absolute;inset:0;display:flex;align-items:center;justify-content:center;
  font-size:20px;font-weight:600;opacity:.6;text-align:center;padding:0 8vw;pointer-events:none}
.aml-foot{position:relative;padding:10px 6vw 14px 2vw;font-size:12px;opacity:.5;min-height:16px}
.aml-btn{width:40px;height:40px;padding:8px;box-sizing:border-box;border:0;border-radius:50%;background:none;
  color:#aaa;cursor:pointer;flex:none}
.aml-btn:hover{color:#fff;background:rgba(255,255,255,.1)}
.aml-btn.active{color:#fff}
.aml-btn.float{position:fixed;right:18px;z-index:2147483001;background:rgba(40,40,40,.92);color:#fff;
  box-shadow:0 4px 16px rgba(0,0,0,.5)}
.aml-btn svg{width:24px;height:24px;fill:currentColor;display:block}
@media (max-width:900px){.aml-side{display:none}.aml-view,.aml-foot{padding-left:6vw}}
@media (prefers-reduced-motion:reduce){.aml-line{transition-duration:.01s}.aml-bg{animation:none}}
`;

  const el = (tag, cls, text) => {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text != null) n.textContent = text;
    return n;
  };

  const s = {
    open: false, key: null, track: null, changedAt: 0,
    data: null, pending: null, cache: new Map(),
    lines: [], els: [], active: -2, y: 0,
    browsing: false, browseUntil: 0, raf: 0,
  };
  let root, bg, art, titleEl, artistEl, view, list, statusEl, foot, btn;

  const video = () => document.querySelector('video');

  function build() {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(CSS);
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];

    root = el('div'); root.id = 'aml-root';
    bg = el('div', 'aml-bg');
    const close = el('button', 'aml-close', '✕');
    close.title = 'Close (Esc)';
    close.addEventListener('click', () => toggle(false));

    const side = el('div', 'aml-side');
    art = el('img', 'aml-art'); art.alt = '';
    titleEl = el('div', 'aml-title');
    artistEl = el('div', 'aml-artist');
    side.append(art, titleEl, artistEl);

    const main = el('div', 'aml-main');
    view = el('div', 'aml-view');
    list = el('div', 'aml-list');
    statusEl = el('div', 'aml-status');
    foot = el('div', 'aml-foot');
    view.append(list, statusEl);
    main.append(view, foot);
    root.append(bg, el('div', 'aml-shade'), side, main, close);
    document.body.append(root);

    view.addEventListener('wheel', onWheel, { passive: false });
    window.addEventListener('resize', () => {
      if (!s.open) return;
      root.style.bottom = barHeight() + 'px';
      layout(false);
    });
    window.addEventListener('keydown', (e) => {
      if (s.open && e.key === 'Escape') { e.stopPropagation(); toggle(false); }
    }, true);

    btn = el('button', 'aml-btn');
    btn.title = 'Animated lyrics';
    const NS = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(NS, 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    const path = document.createElementNS(NS, 'path');
    path.setAttribute('d', 'M6 17h3l2-4V7H5v6h3zm8 0h3l2-4V7h-6v6h3z');
    svg.append(path);
    btn.append(svg);
    btn.addEventListener('click', (e) => { e.stopPropagation(); toggle(); });

    setInterval(tick, 1000);
    tick();
  }

  const visible = (e) => !!e && e.getClientRects().length > 0;
  const firstVisible = (selectors) => {
    const all = selectors.flatMap((q) => [...document.querySelectorAll(q)]);
    return all.find(visible) || all[0] || null;
  };

  // YouTube Music has shipped several player-bar layouts; support all of them.
  const BAR = ['ytmusic-miniplayer', 'ytmusic-player-bar', '[class*="SmallViewportMiniplayerHost"]'];
  const CONTROLS = {
    play: ['.ytmusicPlayerControlsPlayPauseButton button', '.ytMusicSmallViewportMiniplayerPlayPauseButton button',
      'ytmusic-player-bar #play-pause-button'],
    next: ['.ytmusicPlayerControlsNextButton button', '.ytMusicSmallViewportMiniplayerNextButton button',
      'ytmusic-player-bar .next-button'],
    prev: ['.ytmusicPlayerControlsPreviousButton button', 'ytmusic-player-bar .previous-button'],
  };

  function control(action) {
    const target = firstVisible(CONTROLS[action] || []);
    if (target) { target.click(); return; }
    const v = video();
    if (action === 'play' && v) { if (v.paused) v.play(); else v.pause(); }
  }

  function barHeight() {
    const bar = BAR.map((q) => document.querySelector(q)).find(visible);
    return bar ? Math.max(0, window.innerHeight - bar.getBoundingClientRect().top) : 0;
  }

  function ensureButton() {
    const right = document.querySelector('.ytMusicMiniPlayerRightSection');
    const legacy = document.querySelector('ytmusic-player-bar .right-controls-buttons');
    const host = [right, legacy].find(visible);
    if (host) {
      if (btn.parentElement !== host) {
        btn.classList.remove('float');
        btn.style.bottom = '';
        if (host === right) host.insertBefore(btn, host.querySelector('.ytMusicMiniPlayerVolumeWrapper'));
        else host.prepend(btn);
      }
    } else {
      // Unknown layout: fall back to a floating button above the player bar.
      if (btn.parentElement !== document.body) { btn.classList.add('float'); document.body.append(btn); }
      btn.style.bottom = barHeight() + 16 + 'px';
    }
  }

  function getTrack() {
    const md = navigator.mediaSession && navigator.mediaSession.metadata;
    if (!md || !md.title) return null;
    const artist = (md.artist || '').split(' • ')[0].trim();
    const artwork = md.artwork && md.artwork.length ? md.artwork[md.artwork.length - 1].src : '';
    return { title: md.title.trim(), artist, album: (md.album || '').trim(), artwork, key: md.title + '\n' + artist };
  }

  function tick() {
    ensureButton();
    const t = getTrack();
    if (t && t.key !== s.key) {
      s.key = t.key; s.track = t; s.changedAt = performance.now();
      s.data = null; s.pending = null;
      if (s.open) { header(); clearLines('Searching lyrics…'); }
    }
    if (s.open && !s.data) load();
  }

  function header() {
    root.style.bottom = barHeight() + 'px';
    const t = s.track;
    titleEl.textContent = t ? t.title : '';
    artistEl.textContent = t ? t.artist : '';
    if (t && t.artwork) {
      if (art.src !== t.artwork) art.src = t.artwork;
      bg.style.backgroundImage = 'url("' + t.artwork.replace(/"/g, '%22') + '")';
    }
  }

  function load() {
    const t = s.track;
    if (!t) { clearLines('Play a song to see lyrics'); return; }
    const cached = s.cache.get(t.key);
    if (cached) { apply(cached); return; }
    if (s.pending === t.key) return;
    // Give the player a moment so <video>.duration belongs to the new track.
    const v = video();
    const settled = performance.now() - s.changedAt > 1200;
    const dur = v && isFinite(v.duration) ? v.duration : 0;
    if (!settled || (!dur && performance.now() - s.changedAt < 6000)) return;
    s.pending = t.key;
    clearLines('Searching lyrics…');
    window.ipc.postMessage(JSON.stringify({
      type: 'lyrics', key: t.key, title: t.title, artist: t.artist, album: t.album, duration: dur,
    }));
  }

  function clearLines(message) {
    list.replaceChildren();
    s.lines = []; s.els = []; s.active = -2; s.y = 0;
    statusEl.textContent = message || '';
    foot.textContent = '';
  }

  function apply(res) {
    s.data = res; s.pending = null;
    clearLines('');
    root.classList.toggle('plain', !res.synced);
    if (!res.lines.length) {
      statusEl.textContent = 'No lyrics found for this song';
      return;
    }
    let lines = res.lines;
    if (res.synced && lines[0].t > 4) lines = [{ t: 0, text: '' }, ...lines];
    s.lines = lines;
    const frag = document.createDocumentFragment();
    s.els = lines.map((ln) => {
      let node;
      if (res.synced && !ln.text) {
        node = el('div', 'aml-line aml-dots');
        node.append(el('i'), el('i'), el('i'));
      } else {
        node = el('div', 'aml-line', ln.text || ' ');
        if (res.synced) node.addEventListener('click', () => {
          const v = video();
          if (v) { v.currentTime = ln.t; s.browsing = false; s.browseUntil = 0; root.classList.remove('browsing'); }
        });
      }
      frag.append(node);
      return node;
    });
    list.append(frag);
    view.scrollTop = 0;
    foot.textContent = 'Lyrics: ' + res.source + (res.synced ? '' : ' · not time-synced');
    if (res.synced) sync(true);
  }

  function findActive(t) {
    const L = s.lines;
    let lo = 0, hi = L.length - 1, ans = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (L[mid].t <= t) { ans = mid; lo = mid + 1; } else hi = mid - 1;
    }
    return ans;
  }

  function sync(force) {
    const v = video();
    if (!v || !s.data || !s.data.synced || !s.els.length) return;
    const idx = findActive(v.currentTime + LEAD);
    if (force || idx !== s.active) { s.active = idx; layout(true); }
  }

  function layout(stagger) {
    const els = s.els;
    if (!els.length || !s.data || !s.data.synced) return;
    const idx = s.active;
    if (!s.browsing) {
      const target = els[Math.max(idx, 0)];
      s.y = Math.max(0, target.offsetTop - view.clientHeight * ANCHOR);
    }
    const y = -s.y + 'px';
    for (let i = 0; i < els.length; i++) {
      const e = els[i], d = i - idx;
      e.classList.toggle('on', i === idx);
      e.style.setProperty('--y', y);
      e.style.setProperty('--blur', d === 0 ? '0px' : Math.min(Math.abs(d) * 1.2, 6) + 'px');
      // Lines below the active one follow with a small cascade, Apple Music style.
      e.style.transitionDelay = stagger && !s.browsing && d > 0 ? Math.min(d, 10) * 38 + 'ms' : '0ms';
    }
  }

  function onWheel(e) {
    if (!s.data || !s.data.synced || !s.els.length) return;
    e.preventDefault();
    s.browsing = true;
    s.browseUntil = performance.now() + BROWSE_MS;
    root.classList.add('browsing');
    const max = Math.max(0, list.offsetHeight - view.clientHeight * 0.5);
    s.y = Math.min(max, Math.max(0, s.y + e.deltaY));
    layout(false);
  }

  function frame() {
    if (!s.open) return;
    s.raf = requestAnimationFrame(frame);
    if (s.browsing && performance.now() > s.browseUntil) {
      s.browsing = false;
      root.classList.remove('browsing');
      layout(true);
    }
    sync(false);
  }

  function toggle(force) {
    const open = force == null ? !s.open : force;
    if (open === s.open) return;
    s.open = open;
    root.classList.toggle('open', open);
    btn.classList.toggle('active', open);
    cancelAnimationFrame(s.raf);
    if (open) {
      s.track = getTrack() || s.track;
      header();
      if (s.data) { s.active = -2; sync(true); } else { clearLines(''); load(); }
      s.raf = requestAnimationFrame(frame);
    }
  }

  window.__ytmLyrics = {
    toggle,
    control,
    receive(res) {
      s.cache.set(res.key, res);
      if (s.cache.size > 50) s.cache.delete(s.cache.keys().next().value);
      if (res.key === s.key && s.open) apply(res);
      else if (res.key === s.key) s.pending = null;
    },
  };

  if (document.body) build();
  else document.addEventListener('DOMContentLoaded', build, { once: true });
})();
