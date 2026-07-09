// bookmill cover editor (v2). The canvas IS the authoritative resvg SVG (served by
// /api/cover/{slug}/{lang}/svg) — no Konva re-implementation, so the editor can no
// longer drift from what `build cover` ships. A thin pointer-drag overlay places
// labeled boxes over the front's title/subtitle/author; dragging updates the saved
// fractions and Save re-renders. Languages and Front/Wrap/Audiobook views toggle
// in place. Wrap shows the full back·spine·front paperback.

const $ = (id) => document.getElementById(id);
const W = 1600, H = 2560;            // authoritative front-cover pixel space
// Flex-layout constants — mirror cover_svg.rs::front_svg. Used ONLY to seed the
// drag-handle positions for books with no saved [cover.<lang>.layout] (so the
// boxes land on the SVG text). Appearance always comes from the SVG itself.
const PAD_X = 120, TOP = 150, BOTTOM = H - 150, CONTENT_W = W - 2 * PAD_X;
const BADGE_SIZE = 32, BADGE_LH = 1.2;
const RULE_H = 3, RULE_GAP = 46;
const KEYS = ['title', 'subtitle', 'author'];
const WRAP_KEYS = ['blurb', 'badge', 'author'];

let SLUG = null, LANG = null, VIEW = 'wrap';  // open on the full paperback
let LANGS = [];                      // book's declared languages
let data = null;                     // last /api/cover payload
let els = null;                      // working front {title,subtitle,author} fractions
let wrapEls = null;                  // working wrap back-panel {blurb,badge,author} fractions
let current = null;                  // selected key (front or wrap)
let handles = {};                    // key -> overlay div
// Coordinate space of the current view's drag overlay. Front is the 1600×2560
// portrait canvas; wrap is the back-panel sub-rectangle of the landscape wrap SVG
// (origin at the wrap's left edge; width = the SVG's data-back-w in viewBox units,
// height = the full viewBox height). Set by renderStage() per view.
let space = { w: W, h: H, x0: 0, vbW: W, vbH: H };

init();

async function init() {
  const q = new URLSearchParams(location.search);
  SLUG = q.get('book');
  LANG = q.get('lang');
  if (!SLUG || !LANG) {
    const res = await fetch('/api/books').then(r => r.json());
    const b = res.books[0];
    if (b) { SLUG = SLUG || b.slug; LANG = LANG || (b.languages[0] || 'es'); }
  }

  // Discover the book's languages for the ES/EN toggle.
  try {
    const res = await fetch('/api/books').then(r => r.json());
    const b = (res.books || []).find(x => x.slug === SLUG);
    LANGS = (b && b.languages) || [LANG];
  } catch { LANGS = [LANG]; }

  $('backBook').href = `/book.html?book=${encodeURIComponent(SLUG)}`;
  $('toPreview').href = `/preview.html?book=${encodeURIComponent(SLUG)}&lang=${encodeURIComponent(LANG)}`;
  $('openWrap').href = `/api/output/${encodeURIComponent(SLUG)}/${encodeURIComponent(LANG)}/wrap-cover`;

  buildLangSeg();
  wireViewSeg();
  $('saveBtn').onclick = save;
  $('genAudiobook').onclick = genAudiobookCover;
  bindStyleInputs();
  wireShortcuts();
  window.addEventListener('resize', () => positionHandles());

  await loadCover();
}

// ---- header toggles --------------------------------------------------------

function buildLangSeg() {
  const seg = $('langSeg');
  seg.innerHTML = '';
  LANGS.forEach(l => {
    const b = document.createElement('button');
    b.textContent = l.toUpperCase();
    b.className = l === LANG ? 'on' : '';
    b.onclick = () => { if (l !== LANG) switchLang(l); };
    seg.appendChild(b);
  });
}

function wireViewSeg() {
  $('viewSeg').querySelectorAll('button').forEach(b => {
    b.onclick = () => setView(b.dataset.view);
  });
}

async function switchLang(l) {
  LANG = l;
  $('toPreview').href = `/preview.html?book=${encodeURIComponent(SLUG)}&lang=${encodeURIComponent(LANG)}`;
  $('openWrap').href = `/api/output/${encodeURIComponent(SLUG)}/${encodeURIComponent(LANG)}/wrap-cover`;
  buildLangSeg();
  await loadCover();
}

function setView(v) {
  VIEW = v;
  $('viewSeg').querySelectorAll('button').forEach(b => b.classList.toggle('on', b.dataset.view === v));
  // The front (KEYS) and wrap (WRAP_KEYS) selections don't overlap cleanly, so
  // clear the selection when switching surfaces.
  current = null;
  renderStage().then(() => select(null));
}

// The left pane is contextual: a selected text block → block editor; the
// background (clicked on the canvas) → background color; otherwise the view's
// default (wrap → blurb, audiobook → generate, front → hint).
function show(id, on) { const e = $(id); if (e) e.style.display = on ? '' : 'none'; }
function renderPane() {
  const set = activeEls();
  const isBlock = !!current && current !== '__bg__' && !!(set && set[current]);
  const isBg = current === '__bg__' && VIEW === 'front';
  show('blockPane', isBlock);
  show('bgPane', isBg);
  show('wrapPane', VIEW === 'wrap' && !isBlock);
  show('audiobookPane', VIEW === 'audiobook');
  show('hintPane', VIEW === 'front' && !isBlock && !isBg);
  $('saveBtn').style.display = VIEW === 'audiobook' ? 'none' : '';
}

// ---- load + render ---------------------------------------------------------

async function loadCover() {
  status('loading…');
  data = await fetch(`/api/cover/${SLUG}/${LANG}`).then(r => r.json());
  els = JSON.parse(JSON.stringify(data.elements));
  // Wrap back-panel blocks: the backend seeds these (from [cover.<lang>.wrap] when
  // saved, else defaults matching the renderer's back_absolute positions).
  wrapEls = data.wrap
    ? JSON.parse(JSON.stringify(data.wrap))
    : { blurb: null, badge: null, author: null };
  // Seed handle positions to match the SVG's flex render when nothing is saved.
  if (!data.layout_saved) applyFlexDefaults(data, els);

  $('bookLabel').textContent = SLUG;
  $('protBadge').style.display = data.protected ? '' : 'none';
  $('bgcolor').value = toHex6(data.bgcolor);
  $('bgcolorHex').value = data.bgcolor;
  // The blurb textarea is the back-cover blurb *text* source; the wrap blurb
  // handle carries its position/size/color. Seed from the wrap element's text
  // (which the backend fills from [cover.<lang>].blurb) so the two stay in sync.
  $('blurbText').value = (wrapEls.blurb && wrapEls.blurb.text) || data.blurb || '';
  select(null);
  await renderStage();
  status('');
}

// Fetch + inject the authoritative SVG for the current view, then lay the drag
// handles over it (front only). Cache-busted so a re-render is reflected.
async function renderStage() {
  const wrap = VIEW === 'wrap' ? 1 : 0;
  const stage = $('stage');
  status('rendering…');
  let svg;
  try {
    svg = await fetch(`/api/cover/${SLUG}/${LANG}/svg?wrap=${wrap}&t=${Date.now()}`)
      .then(r => r.ok ? r.text() : r.text().then(t => { throw new Error(t); }));
  } catch (e) {
    stage.innerHTML = `<div style="color:#c66;padding:40px;line-height:1.5;max-width:520px">Cover render failed:<br><code>${escapeHtml((e && e.message) || e)}</code></div>`;
    status('render failed');
    return;
  }
  stage.innerHTML = svg;
  const svgEl = stage.querySelector('svg');
  if (svgEl) {
    fitSvg(svgEl);
    // Clicking the canvas itself (not a handle — handles are siblings, not svg
    // children) selects the background in front view, or deselects in wrap.
    svgEl.addEventListener('click', () => select(VIEW === 'front' ? '__bg__' : null));
  }

  handles = {};
  if (VIEW === 'front') {
    // Front canvas: viewBox is 1600×2560, overlay space is the whole canvas.
    space = { w: W, h: H, x0: 0, vbW: W, vbH: H };
    KEYS.forEach(makeHandle);
    positionHandles();
  } else if (VIEW === 'wrap' && svgEl) {
    // Wrap: the back panel is the LEFT sub-rect of the landscape wrap SVG. Its
    // width is exposed as data-back-w (viewBox units); height is the full viewBox.
    const vb = (svgEl.getAttribute('viewBox') || '0 0 0 0').split(/\s+/).map(Number);
    const vbW = vb[2] || 0, vbH = vb[3] || 0;
    const backW = parseFloat(svgEl.getAttribute('data-back-w')) || vbW;
    space = { w: backW, h: vbH, x0: 0, vbW, vbH };
    WRAP_KEYS.forEach(makeHandle);
    positionHandles();
  }
  status('');
}

// Size the SVG to fit the host box, honoring its own aspect (front is portrait
// 1600×2560; wrap is landscape). Explicit px so the inline-block stage shrink-wraps
// it and the drag overlay can measure a real rendered box.
function fitSvg(svgEl) {
  const vb = (svgEl.getAttribute('viewBox') || `0 0 ${W} ${H}`).split(/\s+/).map(Number);
  const aspect = (vb[2] || W) / (vb[3] || H);
  const host = document.querySelector('.stage-host');
  const availW = host.clientWidth - 40, availH = host.clientHeight - 40;
  let w = availH * aspect, h = availH;
  if (w > availW) { w = availW; h = availW / aspect; }
  svgEl.removeAttribute('width'); svgEl.removeAttribute('height');
  svgEl.style.width = w + 'px';
  svgEl.style.height = h + 'px';
}

function makeHandle(key) {
  const h = document.createElement('div');
  h.className = 'handle';
  h.innerHTML = `<span class="tag">${key}</span>`;
  h.onpointerdown = (e) => startDrag(e, key);
  $('stage').appendChild(h);
  handles[key] = h;
}

// The working element set + key list for the current view (front canvas vs. wrap
// back panel). Fractions in `els`/`wrapEls` are relative to `space`.
function activeEls() { return VIEW === 'wrap' ? wrapEls : els; }
function activeKeys() { return VIEW === 'wrap' ? WRAP_KEYS : KEYS; }

// Map the stored (space-relative) fractions to on-screen pixels. An element's box
// center in viewBox units is (space.x0 + x_pct·space.w, y_pct·space.h); the SVG's
// rendered rect maps the full viewBox (vbW×vbH) onto its on-screen size.
function positionHandles() {
  const svgEl = $('stage').querySelector('svg');
  if (!svgEl || VIEW === 'audiobook') return;
  const r = svgEl.getBoundingClientRect();
  const sx = r.width / space.vbW, sy = r.height / space.vbH;
  const set = activeEls();
  activeKeys().forEach(k => {
    const el = set[k], hd = handles[k];
    if (!hd || !el) return;
    const bw = el.w_pct * space.w, bh = Math.max(el.font_pct * space.h, 26);
    const cx = space.x0 + el.x_pct * space.w, cy = el.y_pct * space.h;
    hd.style.left = ((cx - bw / 2) * sx) + 'px';
    hd.style.top = ((cy - bh / 2) * sy) + 'px';
    hd.style.width = (bw * sx) + 'px';
    hd.style.height = (bh * sy) + 'px';
    hd.classList.toggle('sel', k === current);
  });
}

// ---- dragging --------------------------------------------------------------

function startDrag(e, key) {
  e.preventDefault();
  select(key);
  const set = activeEls();
  if (!set[key]) return;
  const svgEl = $('stage').querySelector('svg');
  const r = svgEl.getBoundingClientRect();
  // Screen px per fraction of `space` (viewBox px per fraction × screen/viewBox).
  const sx = (r.width / space.vbW) * space.w, sy = (r.height / space.vbH) * space.h;
  const startX = e.clientX, startY = e.clientY;
  const ox = set[key].x_pct, oy = set[key].y_pct;
  let moved = false;
  const hd = handles[key];
  hd.setPointerCapture(e.pointerId);

  const move = (ev) => {
    const dx = (ev.clientX - startX) / sx;   // fraction delta (of space)
    const dy = (ev.clientY - startY) / sy;
    if (Math.abs(ev.clientX - startX) + Math.abs(ev.clientY - startY) > 2) moved = true;
    set[key].x_pct = clamp(ox + dx, 0, 1);
    set[key].y_pct = clamp(oy + dy, 0, 1);
    positionHandles();
  };
  const up = (ev) => {
    hd.removeEventListener('pointermove', move);
    hd.removeEventListener('pointerup', up);
    try { hd.releasePointerCapture(ev.pointerId); } catch {}
    if (moved) status('moved · Save to re-render');
  };
  hd.addEventListener('pointermove', move);
  hd.addEventListener('pointerup', up);
}

// ---- selection + style inputs ----------------------------------------------

function select(key) {
  current = key;
  const set = activeEls();
  const isBlock = !!key && key !== '__bg__' && !!(set && set[key]);
  if (isBlock) {
    const el = set[key];
    $('selName').textContent = key;
    $('selText').value = el.text;
    $('selFill').value = toHex6(el.fill);
    $('selFillHex').value = el.fill;
    $('selSize').value = Math.round(el.font_pct * space.h);
  }
  renderPane();
  positionHandles();
}

function bindStyleInputs() {
  const sel = () => { const s = activeEls(); return current && s ? s[current] : null; };
  $('selText').oninput = () => {
    const e = sel();
    if (!e) return;
    e.text = $('selText').value;
    if (VIEW === 'wrap' && current === 'blurb') $('blurbText').value = $('selText').value;
  };
  const setFill = (v) => { const e = sel(); if (e) e.fill = v; };
  $('selFill').oninput = () => { $('selFillHex').value = $('selFill').value; setFill($('selFill').value); };
  $('selFillHex').oninput = () => { $('selFill').value = toHex6($('selFillHex').value); setFill($('selFillHex').value); };
  $('selSize').onchange = () => {
    const e = sel();
    if (!e) return;
    const v = parseFloat($('selSize').value);
    if (v > 0) { e.font_pct = v / space.h; positionHandles(); }
  };
  $('bgcolor').oninput = () => { $('bgcolorHex').value = $('bgcolor').value; };
  $('bgcolorHex').oninput = () => { $('bgcolor').value = toHex6($('bgcolorHex').value); };
  // Back-cover blurb textarea (wrap view) → the wrap blurb element's text.
  $('blurbText').oninput = () => {
    if (wrapEls && wrapEls.blurb) wrapEls.blurb.text = $('blurbText').value;
    if (VIEW === 'wrap' && current === 'blurb') $('selText').value = $('blurbText').value;
  };
}

function wireShortcuts() {
  const overlay = $('helpOverlay');
  const toggleHelp = () => overlay.classList.toggle('open');
  $('helpBtn').addEventListener('click', toggleHelp);
  overlay.addEventListener('click', () => overlay.classList.remove('open'));
  document.addEventListener('keydown', (e) => {
    if ((e.metaKey || e.ctrlKey) && (e.key === 's' || e.key === 'S')) {
      e.preventDefault(); if (!$('saveBtn').disabled) save(); return;
    }
    if (e.key === 'Escape') {
      if (overlay.classList.contains('open')) { overlay.classList.remove('open'); return; }
      if (current) select(null);
      return;
    }
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName) || e.target.isContentEditable;
    if (typing || e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === '?') { e.preventDefault(); toggleHelp(); return; }
    const arrows = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
    const set = activeEls();
    if (current && set && set[current] && VIEW !== 'audiobook' && arrows[e.key]) {
      e.preventDefault();
      const step = (e.shiftKey ? 40 : 8);
      const [dx, dy] = arrows[e.key];
      set[current].x_pct = clamp(set[current].x_pct + dx * step / space.w, 0, 1);
      set[current].y_pct = clamp(set[current].y_pct + dy * step / space.h, 0, 1);
      positionHandles();
      status('moved · Save to re-render');
    }
  });
}

// ---- save ------------------------------------------------------------------

async function genAudiobookCover() {
  const btn = $('genAudiobook');
  btn.disabled = true; status('generating audiobook cover…');
  try {
    const r = await fetch(`/api/cover/${SLUG}/${LANG}/audiobook`, { method: 'POST' })
      .then((res) => res.ok ? res.json() : res.text().then((t) => { throw new Error(t); }));
    const img = $('audiobookImg'); img.src = r.url; img.style.display = '';
    status('audiobook cover saved');
  } catch (e) {
    status('audiobook cover failed: ' + ((e && e.message) || e));
  } finally { btn.disabled = false; }
}

async function save() {
  const body = {
    title: els.title,
    subtitle: els.subtitle,
    author: els.author,
    bgcolor: $('bgcolorHex').value || '#000000',
  };
  if (VIEW === 'wrap') {
    // Keep the blurb element's text in sync with the textarea, persist the blurb
    // text to [cover.<lang>].blurb, and the back-panel drag layout to
    // [cover.<lang>.wrap]. Elements already carry x_pct/y_pct/... snake_case keys.
    if (wrapEls.blurb) wrapEls.blurb.text = $('blurbText').value;
    body.blurb = $('blurbText').value;
    body.wrap = {
      blurb: wrapEls.blurb || undefined,
      badge: wrapEls.badge || undefined,
      author: wrapEls.author || undefined,
    };
  }
  $('saveBtn').disabled = true; status('saving + rendering…');
  try {
    const r = await fetch(`/api/cover/${SLUG}/${LANG}`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
    }).then(r => r.json());
    const log = $('log');
    log.style.display = ''; log.textContent = (r.ok ? '✓ ' : '✗ ') + 'saved ' + r.saved + '\n\n' + (r.renderLog || '');
    data.layout_saved = true;
    await renderStage();                    // reload the authoritative SVG
    status(r.ok ? 'saved + rendered' : 'saved (render failed)');
  } catch (e) {
    status('error: ' + e);
  } finally {
    $('saveBtn').disabled = false;
  }
}

// ---- flex-default seeding (handles only; see cover_svg.rs::front_svg) -------

function applyFlexDefaults(data, els) {
  const titleSize = els.title.font_pct * H;
  const subSize = els.subtitle.font_pct * H;
  const badgeH = BADGE_SIZE * BADGE_LH;
  const titleLines = twoLineParts(els.title.text).length;
  const titleH = titleLines * (titleSize * 1.05);
  const ruleBlock = RULE_GAP + RULE_H;
  const subLines = wrapCount(els.subtitle.text, subSize, els.subtitle.font_style, CONTENT_W);
  const subBlockH = subLines * (subSize * 1.3);
  const subH = 40 + subBlockH;
  const titleBlockH = titleH + ruleBlock + subH;
  const authorH = 42 * 1.2;

  const g1m = parseMargin(data.title_mt, CONTENT_W);
  const g2m = parseMargin(data.author_mt, CONTENT_W);
  const fixed = badgeH + titleBlockH + authorH + (g1m.auto ? 0 : g1m.px) + (g2m.auto ? 0 : g2m.px);
  const autos = (g1m.auto ? 1 : 0) + (g2m.auto ? 1 : 0);
  const leftover = Math.max(0, BOTTOM - TOP - fixed);
  const share = autos > 0 ? leftover / autos : 0;
  const g1 = g1m.auto ? share : g1m.px;
  const g2 = g2m.auto ? share : g2m.px;

  let y = TOP;
  y += badgeH + g1;
  const titleCenter = y + titleH / 2;
  y += titleH + ruleBlock;
  const subTop = y + 40;
  const subCenter = subTop + subBlockH / 2;
  y = subTop + subBlockH + g2;
  const authorCenter = y + authorH / 2;

  els.title.y_pct = titleCenter / H;
  els.subtitle.y_pct = subCenter / H;
  els.author.y_pct = authorCenter / H;
  els.title.x_pct = els.subtitle.x_pct = els.author.x_pct = 0.5;
}

function twoLineParts(s) {
  const words = (s || '').split(/\s+/).filter(Boolean);
  if (words.length < 2) return [s || ''];
  const half = [...(s || '')].length / 2;
  let bestI = 1, bestD = Infinity, cur = 0;
  for (let i = 1; i < words.length; i++) {
    cur += [...words[i - 1]].length + 1;
    const d = Math.abs(cur - half);
    if (d < bestD) { bestD = d; bestI = i; }
  }
  return [words.slice(0, bestI).join(' '), words.slice(bestI).join(' ')];
}

const _measureCtx = document.createElement('canvas').getContext('2d');
function wrapCount(text, size, style, maxW) {
  const words = (text || '').split(/\s+/).filter(Boolean);
  if (!words.length) return 1;
  const italic = (style || '').includes('italic') ? 'italic ' : '';
  _measureCtx.font = `${italic}500 ${size}px Montserrat, sans-serif`;
  let lines = 1, cur = '';
  for (const w of words) {
    const trial = cur ? cur + ' ' + w : w;
    if (_measureCtx.measureText(trial).width <= maxW || !cur) cur = trial;
    else { lines++; cur = w; }
  }
  return lines;
}

function parseMargin(s, refW) {
  s = (s || '').trim();
  if (s === 'auto') return { auto: true, px: 0 };
  if (s.endsWith('%')) return { auto: false, px: (parseFloat(s) || 0) / 100 * refW };
  return { auto: false, px: parseFloat(s) || 0 };
}

// ---- utils -----------------------------------------------------------------

function status(s) { $('status').textContent = s; }
function clamp(v, lo, hi) { return Math.max(lo, Math.min(hi, v)); }
function escapeHtml(s) { return String(s).replace(/[&<>]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c])); }

function toHex6(c) {
  if (!c) return '#000000';
  c = c.trim();
  if (/^#[0-9a-fA-F]{6}$/.test(c)) return c;
  if (/^#[0-9a-fA-F]{3}$/.test(c)) return '#' + c.slice(1).split('').map(x => x + x).join('');
  const m = c.match(/rgba?\(([^)]+)\)/);
  if (m) {
    const p = m[1].split(',').map(s => parseFloat(s.trim()));
    return '#' + p.slice(0, 3).map(v => Math.max(0, Math.min(255, v | 0)).toString(16).padStart(2, '0')).join('');
  }
  return '#000000';
}
