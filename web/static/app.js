// bookmill cover editor (v3). The canvas IS the authoritative SVG (served by
// /api/cover/{slug}/{lang}/svg) — no Konva re-implementation, so the editor can no
// longer drift from what `build cover` ships. The SVG is layered: a static
// background (photo + panels + gradients) with each editable text block wrapped in
// its own `<g data-drag="groupId:key">` layer. A pointer-drag overlay of labeled
// boxes captures the drag; as you drag, the block's `<g>` layer is translated live
// so the ACTUAL text tracks the pointer in real time (no background re-render).
// Save persists the fractions and re-renders the authoritative PNG/PDF.
//
// Views (which are offered is driven by the book's edit mode — see EDIT_MODE):
//   • front     — the eBook front only (title/subtitle/author + bg color).
//   • wrap      — the full paperback back·spine·front. BOTH panels are editable:
//                 the back (blurb/badge/author) AND the front (title/subtitle/author).
//   • audiobook — square crop of the front.
// A digital-only book (no print edition) exposes only front + audiobook; a paperback
// book exposes wrap + front + audiobook. `[cover].edit` overrides the derivation.
//
// Each view lays out one or more `groups`, each a set of draggable blocks that share
// a coordinate space (front canvas, wrap back panel, or wrap front panel). Handles are
// keyed "<groupId>:<key>" so the front and back `author` never collide.

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
const FAMILIES = ['Playfair Display', 'Montserrat', 'Baloo 2', 'Oswald', 'Patrick Hand'];

let SLUG = null, LANG = null, VIEW = 'wrap';  // open on the full paperback (if allowed)
let LANGS = [];                      // book's declared languages
let EDIT_MODE = 'wrap';              // 'front' (digital-only) | 'wrap' (paperback)
let data = null;                     // last /api/cover payload
let els = null;                      // front layout {title,subtitle,author} fractions
let wrapEls = null;                  // wrap back-panel {blurb,badge,author} fractions
let current = null;                  // selected "groupId:key", or '__bg__', or null
let handles = {};                    // "groupId:key" -> overlay div
// Draggable groups for the current view. Each: { id, keys, els, space, sizeRef }.
// `space` maps stored fractions to viewBox px: center = (x0 + x_pct·w, y_pct·h),
// wrap width = w_pct·w, size = font_pct·h. `sizeRef` is the height the size FIELD
// reports/edits against (front elements always report against the 2560 canvas so the
// number is stable whether shown in the front view or the wrap's front panel).
let groups = [];
// The block positions the CURRENTLY-INJECTED SVG was rendered at (server bakes
// text at the saved fractions). Keyed "groupId:key" -> {x_pct,y_pct}. Live drag
// translates each block's `<g data-drag>` layer by (working − baked), so the real
// text tracks the pointer without re-rendering the background. Re-synced on load
// and after every save (when the SVG catches up to the working fractions).
let baked = {};

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

// Build the Front/Wrap/Audiobook segmented control from the book's edit mode: a
// digital-only book has no wrap to edit, so it only offers front + audiobook.
function buildViewSeg() {
  const seg = $('viewSeg');
  seg.innerHTML = '';
  const views = EDIT_MODE === 'front'
    ? [['front', 'Front'], ['audiobook', 'Audiobook']]
    : [['wrap', 'Wrap'], ['front', 'Front'], ['audiobook', 'Audiobook']];
  views.forEach(([v, label]) => {
    const b = document.createElement('button');
    b.dataset.view = v;
    b.textContent = label;
    b.className = v === VIEW ? 'on' : '';
    b.onclick = () => setView(v);
    seg.appendChild(b);
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
  current = null;
  renderStage().then(() => select(null));
}

// ---- group helpers ---------------------------------------------------------

function groupOf(id) { return groups.find(g => g.id === id); }
// Resolve the current "groupId:key" selection to {g, key, el}, or null.
function selInfo() {
  if (!current || current === '__bg__') return null;
  const [gid, key] = current.split(':');
  const g = groupOf(gid);
  if (!g || !g.els[key]) return null;
  return { g, key, el: g.els[key] };
}

// The left pane is contextual: a selected text block → block editor; the front
// background (front view only) → background color; otherwise the view's default
// (wrap → blurb text, audiobook → generate, front → hint).
function show(id, on) { const e = $(id); if (e) e.style.display = on ? '' : 'none'; }
function renderPane() {
  const isBlock = !!selInfo();
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
  wrapEls = data.wrap
    ? JSON.parse(JSON.stringify(data.wrap))
    : { blurb: null, badge: null, author: null };
  if (!data.layout_saved) applyFlexDefaults(data, els);

  // Which surfaces this book exposes. A digital-only book can't be viewed as a wrap.
  EDIT_MODE = data.edit_mode === 'front' ? 'front' : 'wrap';
  if (EDIT_MODE === 'front' && VIEW === 'wrap') VIEW = 'front';
  buildViewSeg();

  $('bookLabel').textContent = SLUG;
  $('protBadge').style.display = data.protected ? '' : 'none';
  $('bgcolor').value = toHex6(data.bgcolor);
  $('bgcolorHex').value = data.bgcolor;
  $('blurbText').value = (wrapEls.blurb && wrapEls.blurb.text) || data.blurb || '';
  snapshotBaked();   // the SVG we're about to fetch is baked at these fractions
  select(null);
  await renderStage();
  status('');
}

// Fetch + inject the authoritative SVG for the current view, then lay the drag
// handle groups over it. Cache-busted so a re-render is reflected.
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
    // Clicking the canvas (not a handle) selects the background in front view, or
    // deselects otherwise.
    svgEl.addEventListener('click', () => select(VIEW === 'front' ? '__bg__' : null));
  }

  handles = {};
  groups = buildGroups(svgEl);
  groups.forEach(g => g.keys.forEach(k => makeHandle(g, k)));
  applyAllLive();   // restore any unsaved drags onto the fresh SVG
  positionHandles();
  status('');
}

// The draggable groups for the current view.
function buildGroups(svgEl) {
  if (VIEW === 'front') {
    return [{ id: 'front', keys: KEYS, els, space: { w: W, h: H, x0: 0, vbW: W, vbH: H }, sizeRef: H }];
  }
  if (VIEW === 'wrap' && svgEl) {
    // The wrap SVG exposes the back panel (data-back-w) and the front panel
    // (data-front-x / data-front-w) as sub-rectangles of the landscape viewBox.
    const vb = (svgEl.getAttribute('viewBox') || '0 0 0 0').split(/\s+/).map(Number);
    const vbW = vb[2] || 0, vbH = vb[3] || 0;
    const backW = parseFloat(svgEl.getAttribute('data-back-w')) || vbW;
    const frontX = parseFloat(svgEl.getAttribute('data-front-x')) || 0;
    const frontW = parseFloat(svgEl.getAttribute('data-front-w')) || backW;
    return [
      { id: 'back', keys: WRAP_KEYS, els: wrapEls, space: { w: backW, h: vbH, x0: 0, vbW, vbH }, sizeRef: vbH },
      // Front-panel blocks share the front layout fractions ([cover.<lang>.layout]),
      // mapped into the wrap's front sub-rect. sizeRef stays the 2560 front canvas so
      // the size field reads the same here as in the front view.
      { id: 'front', keys: KEYS, els, space: { w: frontW, h: vbH, x0: frontX, vbW, vbH }, sizeRef: H },
    ];
  }
  return [];
}

// Size the SVG to fit the host box, honoring its own aspect. Explicit px so the
// inline-block stage shrink-wraps it and the drag overlay can measure a real box.
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

function makeHandle(g, key) {
  const id = g.id + ':' + key;
  const h = document.createElement('div');
  h.className = 'handle';
  // In the wrap view the same key exists on both panels; tag the side for clarity.
  const tag = groups.length > 1 ? `${g.id} · ${key}` : key;
  h.innerHTML = `<span class="tag">${tag}</span>`;
  h.onpointerdown = (e) => startDrag(e, g, key);
  $('stage').appendChild(h);
  handles[id] = h;
}

// Map every group's stored fractions to on-screen pixels. Handle height reflects the
// real wrapped block (mirrors the renderer's word-wrap) so multi-line blocks are
// grabbable across their whole extent.
function positionHandles() {
  const svgEl = $('stage').querySelector('svg');
  if (!svgEl || VIEW === 'audiobook') return;
  const r = svgEl.getBoundingClientRect();
  groups.forEach(g => {
    const sx = r.width / g.space.vbW, sy = r.height / g.space.vbH;
    g.keys.forEach(k => {
      const el = g.els[k], hd = handles[g.id + ':' + k];
      if (!hd || !el) return;
      const size = el.font_pct * g.space.h;
      const bw = el.w_pct * g.space.w;
      const lines = wrapCount(el.text, size, el.font_style, el.font_family, bw);
      const bh = Math.max(lines * size, 26);
      const cx = g.space.x0 + el.x_pct * g.space.w, cy = el.y_pct * g.space.h;
      hd.style.left = ((cx - bw / 2) * sx) + 'px';
      hd.style.top = ((cy - bh / 2) * sy) + 'px';
      hd.style.width = (bw * sx) + 'px';
      hd.style.height = (bh * sy) + 'px';
      hd.classList.toggle('sel', current === g.id + ':' + k);
    });
  });
}

// ---- live text layer (real-time drag) --------------------------------------

// Record the fractions the current SVG was baked at, for every editable block on
// both panels (front + back), so live transforms are measured against the actual
// rendered positions regardless of which view is showing.
function snapshotBaked() {
  baked = {};
  if (els) for (const k of KEYS) if (els[k]) baked['front:' + k] = { x_pct: els[k].x_pct, y_pct: els[k].y_pct };
  if (wrapEls) for (const k of WRAP_KEYS) if (wrapEls[k]) baked['back:' + k] = { x_pct: wrapEls[k].x_pct, y_pct: wrapEls[k].y_pct };
}

// Translate one block's `<g data-drag>` layer to its working position (delta from
// the baked SVG position, in viewBox px). No-op if the SVG has no such layer.
function liveTransform(g, key) {
  const svgEl = $('stage').querySelector('svg');
  if (!svgEl) return;
  const id = g.id + ':' + key;
  const layer = svgEl.querySelector(`[data-drag="${id}"]`);
  const el = g.els[key], b = baked[id];
  if (!layer || !el || !b) return;
  const dx = (el.x_pct - b.x_pct) * g.space.w;
  const dy = (el.y_pct - b.y_pct) * g.space.h;
  if (dx || dy) layer.setAttribute('transform', `translate(${dx.toFixed(2)} ${dy.toFixed(2)})`);
  else layer.removeAttribute('transform');
}

// Re-apply every group's live transform (used after a fresh SVG injection so any
// unsaved drags persist visually across view switches).
function applyAllLive() { groups.forEach(g => g.keys.forEach(k => liveTransform(g, k))); }

// ---- dragging --------------------------------------------------------------

function startDrag(e, g, key) {
  e.preventDefault();
  select(g.id + ':' + key);
  const el = g.els[key];
  if (!el) return;
  const svgEl = $('stage').querySelector('svg');
  const r = svgEl.getBoundingClientRect();
  const sx = (r.width / g.space.vbW) * g.space.w, sy = (r.height / g.space.vbH) * g.space.h;
  const startX = e.clientX, startY = e.clientY;
  const ox = el.x_pct, oy = el.y_pct;
  let moved = false;
  const hd = handles[g.id + ':' + key];
  hd.setPointerCapture(e.pointerId);

  const move = (ev) => {
    const dx = (ev.clientX - startX) / sx;
    const dy = (ev.clientY - startY) / sy;
    if (Math.abs(ev.clientX - startX) + Math.abs(ev.clientY - startY) > 2) moved = true;
    el.x_pct = clamp(ox + dx, 0, 1);
    el.y_pct = clamp(oy + dy, 0, 1);
    liveTransform(g, key);   // move the actual text in real time
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

function select(sel) {
  current = sel;
  const info = selInfo();
  if (info) {
    const el = info.el;
    $('selName').textContent = groups.length > 1 ? `${info.g.id} · ${info.key}` : info.key;
    $('selText').value = el.text || '';
    $('selFill').value = toHex6(el.fill);
    $('selFillHex').value = el.fill || '';
    $('selSize').value = Math.round(el.font_pct * info.g.sizeRef);
    const st = el.font_style || 'normal';
    $('selBold').classList.toggle('on', st.includes('bold'));
    $('selItalic').classList.toggle('on', st.includes('italic'));
    setFamilySelect(el.font_family || '');
  }
  renderPane();
  positionHandles();
}

// The font-style string bookmill expects: "normal" | "bold" | "italic" | "bold italic".
function styleStr(bold, italic) {
  if (bold && italic) return 'bold italic';
  if (bold) return 'bold';
  if (italic) return 'italic';
  return 'normal';
}

function setFamilySelect(fam) {
  const sel = $('selFamily');
  // Keep an unknown (hand-authored) family selectable rather than silently losing it.
  if (fam && !FAMILIES.includes(fam) && ![...sel.options].some(o => o.value === fam)) {
    const o = document.createElement('option');
    o.value = o.textContent = fam;
    sel.appendChild(o);
  }
  sel.value = fam || FAMILIES[0];
}

function bindStyleInputs() {
  const sel = () => { const i = selInfo(); return i ? i.el : null; };
  $('selText').oninput = () => {
    const e = sel();
    if (!e) return;
    e.text = $('selText').value;
    if (VIEW === 'wrap' && current === 'back:blurb') $('blurbText').value = $('selText').value;
  };
  const setFill = (v) => { const e = sel(); if (e) e.fill = v; };
  $('selFill').oninput = () => { $('selFillHex').value = $('selFill').value; setFill($('selFill').value); };
  $('selFillHex').oninput = () => { $('selFill').value = toHex6($('selFillHex').value); setFill($('selFillHex').value); };
  $('selSize').onchange = () => {
    const i = selInfo();
    if (!i) return;
    const v = parseFloat($('selSize').value);
    if (v > 0) { i.el.font_pct = v / i.g.sizeRef; positionHandles(); }
  };
  $('selFamily').onchange = () => { const e = sel(); if (e) { e.font_family = $('selFamily').value; positionHandles(); } };
  const applyStyle = () => {
    const e = sel();
    if (!e) return;
    e.font_style = styleStr($('selBold').classList.contains('on'), $('selItalic').classList.contains('on'));
    positionHandles();
  };
  $('selBold').onclick = () => { $('selBold').classList.toggle('on'); applyStyle(); };
  $('selItalic').onclick = () => { $('selItalic').classList.toggle('on'); applyStyle(); };
  $('bgcolor').oninput = () => { $('bgcolorHex').value = $('bgcolor').value; };
  $('bgcolorHex').oninput = () => { $('bgcolor').value = toHex6($('bgcolorHex').value); };
  // Back-cover blurb textarea (wrap view) → the wrap blurb element's text.
  $('blurbText').oninput = () => {
    if (wrapEls && wrapEls.blurb) wrapEls.blurb.text = $('blurbText').value;
    if (VIEW === 'wrap' && current === 'back:blurb') $('selText').value = $('blurbText').value;
  };
}

function wireShortcuts() {
  const overlay = $('helpOverlay');
  let helpClose = null;
  const openHelp = () => { overlay.classList.add('open'); helpClose = a11y.openDialog(overlay); };
  const shutHelp = () => { overlay.classList.remove('open'); if (helpClose) { helpClose(); helpClose = null; } };
  const toggleHelp = () => (overlay.classList.contains('open') ? shutHelp() : openHelp());
  $('helpBtn').addEventListener('click', toggleHelp);
  overlay.addEventListener('click', shutHelp);
  document.addEventListener('keydown', (e) => {
    if ((e.metaKey || e.ctrlKey) && (e.key === 's' || e.key === 'S')) {
      e.preventDefault(); if (!$('saveBtn').disabled) save(); return;
    }
    if (e.key === 'Escape') {
      if (overlay.classList.contains('open')) { shutHelp(); return; }
      if (current) select(null);
      return;
    }
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName) || e.target.isContentEditable;
    if (typing || e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === '?') { e.preventDefault(); toggleHelp(); return; }
    const arrows = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
    const info = selInfo();
    if (info && VIEW !== 'audiobook' && arrows[e.key]) {
      e.preventDefault();
      const step = (e.shiftKey ? 40 : 8);
      const [dx, dy] = arrows[e.key];
      info.el.x_pct = clamp(info.el.x_pct + dx * step / info.g.space.w, 0, 1);
      info.el.y_pct = clamp(info.el.y_pct + dy * step / info.g.space.h, 0, 1);
      liveTransform(info.g, info.key);
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
  // The front layout (title/subtitle/author) is always persisted; the wrap view also
  // persists the back panel. So dragging front blocks while in the wrap view saves
  // them to [cover.<lang>.layout] just as the front view does.
  const body = {
    title: els.title,
    subtitle: els.subtitle,
    author: els.author,
    bgcolor: $('bgcolorHex').value || '#000000',
  };
  if (VIEW === 'wrap') {
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
    snapshotBaked();                        // the re-render bakes the working fractions → deltas reset to 0
    await renderStage();                    // reload the authoritative SVG
    status(r.ok ? 'saved + rendered' : 'saved (render failed)');
    a11y.announce(r.ok ? 'Cover saved and re-rendered' : 'Cover saved, but render failed');
  } catch (e) {
    status('error: ' + e);
    a11y.announce('Cover save failed');
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
  const subLines = wrapCount(els.subtitle.text, subSize, els.subtitle.font_style, els.subtitle.font_family, CONTENT_W);
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
function wrapCount(text, size, style, family, maxW) {
  const words = (text || '').split(/\s+/).filter(Boolean);
  if (!words.length) return 1;
  const italic = (style || '').includes('italic') ? 'italic ' : '';
  const weight = (style || '').includes('bold') ? 700 : 500;
  const fam = family ? `'${family}', ` : '';
  _measureCtx.font = `${italic}${weight} ${size}px ${fam}Montserrat, sans-serif`;
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
