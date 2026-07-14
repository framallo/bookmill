// bookmill cover editor (v3). The canvas IS the authoritative SVG (served by
// /api/cover/{slug}/{lang}/svg) — no Konva re-implementation, so the editor can no
// longer drift from what `build cover` ships. The SVG is layered: a static
// background (photo + panels + gradients) with each editable text block wrapped in
// its own `<g data-drag="groupId:key">` layer. A pointer-drag overlay of labeled
// boxes captures the drag; as you drag, the block's `<g>` layer is translated live
// so the ACTUAL text tracks the pointer in real time (no background re-render).
// Save persists the fractions and re-renders the authoritative PNG/PDF.
//
// ONE DOCUMENT: you edit the full paperback wrap (back · spine · front). Both panels
// are editable — the back (blurb/badge/author) and the front (title/subtitle/author).
// The eBook front cover IS the wrap's front panel (they share [cover.layout]), and the
// audiobook cover is a square crop of it: both are GENERATED from this one document,
// never edited separately. That's why there are no view tabs.
//
// The wrap lays out two `groups`, each a set of draggable blocks sharing a coordinate
// space (the back panel, the front panel). Handles are keyed "<groupId>:<key>" so the
// front and back `author` never collide.

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

let SLUG = null, LANG = null;
const VIEW = 'wrap';                 // the only surface: the full paperback wrap
let LANGS = [];                      // book's declared languages
let data = null;                     // last /api/cover payload
let els = null;                      // front layout {title,subtitle,author} fractions
let wrapEls = null;                  // wrap back-panel {blurb,badge,author} fractions
let current = null;                  // selected "groupId:key", or '__bg__', or null
let handles = {};                    // "groupId:key" -> overlay div
// Draggable groups. Each: { id, keys, els, space, sizeRef }.
// `space` maps stored fractions to viewBox px: center = (x0 + x_pct·w, y_pct·h),
// wrap width = w_pct·w, size = font_pct·h. `sizeRef` is the height the size FIELD
// reports/edits against — the SAME for every group (the wrap's height), so the size
// number means one thing across the whole document. It used to be 2560 (the front
// canvas) for front blocks and 888 (the wrap) for back ones, so two authors that
// render at the same size read as "42" and "13" — and setting them to one number
// blew the back one up. Both panels are one document now; one reference.
let groups = [];
// The block positions the CURRENTLY-INJECTED SVG was rendered at (server bakes
// text at the saved fractions). Keyed "groupId:key" -> {x_pct,y_pct}. Live drag
// translates each block's `<g data-drag>` layer by (working − baked), so the real
// text tracks the pointer without re-rendering the background. Re-synced on load
// and after every save (when the SVG catches up to the working fractions).
let baked = {};
let bgcolor = '#000000';             // working background color (no sidebar; bar holds it)
// Inline editor state: which block is open ("groupId:key") and its char model.
// The char model is the per-character styling substrate — one entry per character,
// carrying the run style it belongs to. Selection-based styling edits this array,
// which then collapses back into runs (adjacent same-style chars merge).
let editing = null;                  // { g, key, el } while the inline editor is open

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

  buildLangSeg();
  $('saveBtn').onclick = save;
  $('genAudiobook').onclick = genAudiobookCover;
  bindFormatBar();
  wireShortcuts();
  window.addEventListener('resize', () => { positionHandles(); if (editing) { styleInline(); placeInline(); } });

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

// There is no sidebar: the format bar is the whole chrome. It is live only when a
// text block is selected, and it targets the inline editor's SELECTION when there
// is one (per-run styling), else the whole block.
function show(id, on) { const e = $(id); if (e) e.style.display = on ? '' : 'none'; }
function renderPane() {
  const isBlock = !!selInfo();
  $('fmtbar').classList.toggle('off', !isBlock);
  $('fmtHint').textContent = isBlock
    ? (editing ? 'Select text to style just that run' : 'Double-click the block to edit its text')
    : 'Click a block to select it · double-click to edit its text';
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

  $('bookLabel').textContent = SLUG;
  $('protBadge').style.display = data.protected ? '' : 'none';
  bgcolor = data.bgcolor || '#000000';
  $('bgcolor').value = toHex6(bgcolor);
  $('bgSw').style.background = toHex6(bgcolor);
  // The back-cover blurb is edited inline on the canvas now (no sidebar textarea).
  if (wrapEls.blurb && !wrapEls.blurb.text) wrapEls.blurb.text = data.blurb || '';
  snapshotBaked();   // the SVG we're about to fetch is baked at these fractions
  select(null);
  await renderStage();
  status('');
}

// Render the current WORKING state to the canvas: POST the in-memory layout to the
// authoritative renderer (no disk write) so size/style/font/text/markdown edits all
// preview live — then lay the drag-handle groups over the fresh SVG. Because the
// SVG is baked at the working fractions, snapshot `baked` right before mounting so
// live-drag deltas reset to zero.
async function renderStage() {
  const stage = $('stage');
  status('rendering…');
  let svg;
  try {
    svg = await fetch(`/api/cover/${SLUG}/${LANG}/svg?wrap=1`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(currentBody()),
    }).then(r => r.ok ? r.text() : r.text().then(t => { throw new Error(t); }));
  } catch (e) {
    stage.innerHTML = `<div style="color:#c66;padding:40px;line-height:1.5;max-width:520px">Cover render failed:<br><code>${escapeHtml((e && e.message) || e)}</code></div>`;
    status('render failed');
    return;
  }
  snapshotBaked();
  mountSvg(svg);
  status('');
}

// Inject an SVG string as the editing surface and (re)build the drag overlay.
function mountSvg(svg) {
  const stage = $('stage');
  stage.innerHTML = svg;
  const svgEl = stage.querySelector('svg');
  if (svgEl) {
    fitSvg(svgEl);
    // Clicking the canvas (not a handle) selects the background in front view, or
    // deselects otherwise.
    svgEl.addEventListener('click', () => {
      if (editing) { commitEdit(); return; }
      select(null);
    });
  }
  handles = {};
  groups = buildGroups(svgEl);
  groups.forEach(g => g.keys.forEach(k => makeHandle(g, k)));
  applyAllLive();   // restore any unsaved drags onto the fresh SVG
  positionHandles();
}

// Debounced live re-render used while editing style/text in the sidebar, so the
// canvas reflects size/font/color/markdown changes without a full Save. Coalesces
// bursts and never overlaps two in-flight renders.
let _previewTimer = null, _previewBusy = false, _previewAgain = false;
function scheduleLivePreview() {
  clearTimeout(_previewTimer);
  _previewTimer = setTimeout(runLivePreview, 160);
}
async function runLivePreview() {
  if (_previewBusy) { _previewAgain = true; return; }
  _previewBusy = true;
  try { await renderStage(); }
  finally {
    _previewBusy = false;
    if (_previewAgain) { _previewAgain = false; scheduleLivePreview(); }
  }
}

// The draggable groups for the current view.
function buildGroups(svgEl) {
  if (svgEl) {
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
      { id: 'front', keys: KEYS, els, space: { w: frontW, h: vbH, x0: frontX, vbW, vbH }, sizeRef: vbH },
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
  h.innerHTML = `<span class="tag">${tag}</span><span class="grip l"></span><span class="grip r"></span>`;
  h.querySelectorAll('.grip').forEach(gr =>
    gr.addEventListener('pointerdown', (e) => { e.stopPropagation(); startResize(e, g, key); }));
  h.onpointerdown = (e) => startDrag(e, g, key);
  h.ondblclick = (e) => { e.preventDefault(); enterEdit(g, key); };
  $('stage').appendChild(h);
  handles[id] = h;
}

// Map every group's stored fractions to on-screen pixels. Handle height reflects the
// real wrapped block (mirrors the renderer's word-wrap) so multi-line blocks are
// grabbable across their whole extent.
function positionHandles() {
  const svgEl = $('stage').querySelector('svg');
  if (!svgEl) return;
  const r = svgEl.getBoundingClientRect();
  groups.forEach(g => {
    const sx = r.width / g.space.vbW, sy = r.height / g.space.vbH;
    g.keys.forEach(k => {
      const el = g.els[k], hd = handles[g.id + ':' + k];
      if (!hd || !el) return;
      const size = el.font_pct * g.space.h;
      const bw = el.w_pct * g.space.w;
      const lines = wrapCount(el.text, size, el.font_style, el.font_family, bw);
      // Match the renderer: a line's box is size × line-height (cover_svg::line_box),
      // so the handle wraps the text instead of ending short of it.
      const lh = el.line_height > 0 ? el.line_height : 1.0;
      const bh = Math.max(lines * size * lh, 26);
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

// Drag a width grip: the wrap box grows/shrinks symmetrically about the block's
// center, and the text reflows inside it. Only the WIDTH is settable — the block's
// height is whatever the wrapped text needs (see positionHandles).
function startResize(e, g, key) {
  e.preventDefault();
  select(g.id + ':' + key);
  const el = g.els[key];
  if (!el) return;
  const svgEl = $('stage').querySelector('svg');
  const r = svgEl.getBoundingClientRect();
  const sx = r.width / g.space.vbW;           // screen px per viewBox px
  const grip = e.target;
  grip.setPointerCapture(e.pointerId);

  const move = (ev) => {
    // pointer position in the block's panel coordinate space
    const px = (ev.clientX - r.left) / sx - g.space.x0;
    const cx = el.x_pct * g.space.w;
    const half = Math.abs(px - cx);
    el.w_pct = clamp((2 * half) / g.space.w, 0.04, 1);
    positionHandles();
    syncBar();
    scheduleLivePreview();                    // the text reflows in the real SVG
  };
  const up = (ev) => {
    grip.removeEventListener('pointermove', move);
    grip.removeEventListener('pointerup', up);
    try { grip.releasePointerCapture(ev.pointerId); } catch {}
    status('box resized · Save to re-render');
  };
  grip.addEventListener('pointermove', move);
  grip.addEventListener('pointerup', up);
}

// ---- selection + the format bar --------------------------------------------

function select(sel) {
  if (editing && sel !== current) commitEdit();
  current = sel;
  syncBar();
  renderPane();
  positionHandles();
}

// Reflect the active button in an icon group keyed by a data-<attr>.
function setSeg(attr, val) {
  $('fmtbar').querySelectorAll(`[data-${attr}]`).forEach(b => b.classList.toggle('on', b.dataset[attr] === val));
}
// Parse an SVG-ish stroke "2px #rrggbb" into {w, color}; empty/none → no stroke.
function parseStroke(s) {
  s = (s || '').trim();
  const m = s.match(/^([\d.]+)px\s+(#[0-9a-fA-F]{3,8}|[a-zA-Z]+)/);
  if (m && parseFloat(m[1]) > 0) return { w: parseFloat(m[1]), color: toHex6(m[2]) };
  return { w: 0, color: '#000000' };
}
// Build the stroke string, or undefined when width is 0 (no outline).
function composeStroke(w, color) {
  w = parseFloat(w);
  return w > 0 ? `${w}px ${color || '#000000'}` : undefined;
}

// The font-style string bookmill expects: "normal" | "bold" | "italic" | "bold italic".
function styleStr(bold, italic) {
  if (bold && italic) return 'bold italic';
  if (bold) return 'bold';
  if (italic) return 'italic';
  return 'normal';
}

// Push the selected block's style into the bar. When the inline editor has a
// non-empty selection, the type controls show that RUN's style instead, so the
// bar always describes what a click would change.
function syncBar() {
  const info = selInfo();
  if (!info) return;
  const el = info.el;
  const st = el.font_style || 'normal';
  const r = editing ? selectionStyle() : null;   // run style under the cursor/selection

  setFamilySelect((r && r.family) || el.font_family || '');
  const blockSize = Math.round(el.font_pct * info.g.sizeRef);
  $('fSize').value = r && r.size ? Math.round(blockSize * r.size) : blockSize;
  $('fBold').classList.toggle('on', r ? !!r.bold : st.includes('bold'));
  $('fItalic').classList.toggle('on', r ? !!r.italic : st.includes('italic'));
  const fill = (r && r.fill) || el.fill || '#FFFFFF';
  $('fFill').value = toHex6(fill);
  $('fFillSw').style.background = toHex6(fill);

  // Block-level controls (never per-run).
  $('fBoxW').value = Math.round(el.w_pct * info.g.space.w);
  setSeg('align', el.align || 'center');
  setSeg('case', el.text_transform || 'none');
  $('fLineHeight').value = el.line_height != null ? el.line_height : '';
  $('fLetterSpacing').value = el.letter_spacing != null ? el.letter_spacing : '';
  $('fOpacity').value = el.opacity != null ? el.opacity : 1;
  const s = parseStroke(el.stroke);
  $('fStrokeColor').value = s.color;
  $('fStrokeSw').style.background = s.color;
  $('fStrokeWidth').value = s.w || '';
  $('fShadow').classList.toggle('on', el.shadow !== false);
}

function setFamilySelect(fam) {
  const sel = $('fFamily');
  if (fam && ![...sel.options].some(o => o.value === fam)) {
    const o = document.createElement('option');
    o.value = o.textContent = fam;
    sel.appendChild(o);
  }
  sel.value = fam || FAMILIES[0];
}

function bindFormatBar() {
  const sel = () => { const i = selInfo(); return i ? i.el : null; };
  FAMILIES.forEach(f => {
    const o = document.createElement('option');
    o.value = o.textContent = f;
    $('fFamily').appendChild(o);
  });

  // --- type controls: apply to the inline selection if there is one, else block.
  $('fFamily').onchange = () => applyType({ family: $('fFamily').value });
  $('fSize').oninput = () => {
    const v = parseFloat($('fSize').value);
    if (v > 0) applyType({ sizePx: v });
  };
  $('fBold').onclick = () => applyType({ toggleBold: true });
  $('fItalic').onclick = () => applyType({ toggleItalic: true });
  $('fFill').oninput = () => { $('fFillSw').style.background = $('fFill').value; applyType({ fill: $('fFill').value }); };
  $('fClear').onclick = () => applyType({ clear: true });

  // --- block-only controls
  $('fmtbar').querySelectorAll('[data-align]').forEach(b => {
    b.onclick = () => { const e = sel(); if (!e) return; e.align = b.dataset.align; setSeg('align', b.dataset.align); afterStyle(); };
  });
  $('fmtbar').querySelectorAll('[data-case]').forEach(b => {
    b.onclick = () => {
      const e = sel(); if (!e) return;
      e.text_transform = b.dataset.case === 'none' ? undefined : b.dataset.case;
      setSeg('case', b.dataset.case);
      afterStyle();
    };
  });
  // Box width — the wrap box the text flows inside (same reference as the size field).
  $('fBoxW').oninput = () => {
    const i = selInfo(); if (!i) return;
    const v = parseFloat($('fBoxW').value);
    if (v > 0) { i.el.w_pct = clamp(v / i.g.space.w, 0.04, 1); positionHandles(); afterStyle(); }
  };
  // Center the block in its panel (the front panel or the back panel, not the whole wrap).
  $('fCenterH').onclick = () => centerBlock('x');
  $('fCenterV').onclick = () => centerBlock('y');
  $('fLineHeight').oninput = () => { const e = sel(); if (!e) return; const v = parseFloat($('fLineHeight').value); e.line_height = v > 0 ? v : undefined; afterStyle(); };
  $('fLetterSpacing').oninput = () => { const e = sel(); if (!e) return; const v = parseFloat($('fLetterSpacing').value); e.letter_spacing = isNaN(v) ? undefined : v; afterStyle(); };
  $('fOpacity').oninput = () => { const e = sel(); if (!e) return; e.opacity = parseFloat($('fOpacity').value); afterStyle(); };
  const applyStroke = () => {
    const e = sel(); if (!e) return;
    $('fStrokeSw').style.background = $('fStrokeColor').value;
    e.stroke = composeStroke($('fStrokeWidth').value, $('fStrokeColor').value);
    afterStyle();
  };
  $('fStrokeColor').oninput = applyStroke;
  $('fStrokeWidth').oninput = applyStroke;
  $('fShadow').onclick = () => {
    const e = sel(); if (!e) return;
    const on = !$('fShadow').classList.contains('on');
    $('fShadow').classList.toggle('on', on);
    e.shadow = on;
    afterStyle();
  };
  $('bgcolor').oninput = () => {
    bgcolor = $('bgcolor').value;
    $('bgSw').style.background = bgcolor;
    scheduleLivePreview();
  };
}

// Center the selected block within ITS panel — the back panel or the front panel,
// whichever it belongs to (each group's space is that panel's rect), not the whole
// wrap. Position is stored as the block's center, so centering is just 0.5.
function centerBlock(axis) {
  const i = selInfo();
  if (!i) return;
  if (axis === 'x') i.el.x_pct = 0.5;
  else i.el.y_pct = 0.5;
  liveTransform(i.g, i.key);     // move the real text immediately
  positionHandles();
  if (editing) placeInline();
  status('centered · Save to re-render');
  scheduleLivePreview();
}

// After a block-level style change: reflow handles, and re-render. While the
// inline editor is open we restyle IT instead of re-rendering the SVG (a render
// would blow the editor away mid-keystroke).
function afterStyle() {
  positionHandles();
  if (editing) { styleInline(); return; }
  scheduleLivePreview();
}

// ---- inline text editor (on the cover) -------------------------------------
//
// Per-character styling, canvas-editor style: the block's text becomes an array of
// {ch, style} — one entry per character. The contenteditable renders that array as
// styled <span>s; a toolbar click rewrites the style of the selected character
// range; the array then collapses back into runs (adjacent same-style chars merge)
// which is exactly what bookmill.toml stores and resvg renders.

let chars = [];   // [{ ch, s:{bold,italic,fill,family,size} }] for the open block

// Explode an element's runs into the per-character model.
function elToChars(el) {
  const runs = (el.runs && el.runs.length) ? el.runs : [{ t: el.text || '' }];
  const out = [];
  runs.forEach(r => {
    const s = {
      bold: r.bold ?? undefined, italic: r.italic ?? undefined,
      fill: r.fill ?? undefined, family: r.family ?? undefined, size: r.size ?? undefined,
    };
    for (const ch of (r.t || '')) out.push({ ch, s: { ...s } });
  });
  return out;
}

const sameStyle = (a, b) =>
  a.bold === b.bold && a.italic === b.italic && a.fill === b.fill && a.family === b.family && a.size === b.size;

// Collapse the char model back into runs (adjacent same-style chars merge).
function charsToRuns(cs) {
  const runs = [];
  for (const c of cs) {
    const last = runs[runs.length - 1];
    if (last && sameStyle(last._s, c.s)) last.t += c.ch;
    else runs.push({ _s: { ...c.s }, t: c.ch, ...c.s });
  }
  return runs.map(r => {
    const o = { t: r.t };
    for (const k of ['bold', 'italic', 'fill', 'family', 'size']) if (r._s[k] !== undefined) o[k] = r._s[k];
    return o;
  });
}

const charsToText = (cs) => cs.map(c => c.ch).join('');
const isStyled = (s) => s.bold !== undefined || s.italic !== undefined || s.fill !== undefined || s.family !== undefined || s.size !== undefined;

// Open the inline editor over a block. It mirrors the block's typography so the
// text you type sits where the printed text sits.
function enterEdit(g, key) {
  if (editing) commitEdit();
  const el = g.els[key];
  if (!el) return;
  select(g.id + ':' + key);
  editing = { g, key, el };
  chars = elToChars(el);

  const box = document.createElement('div');
  box.id = 'inline';
  box.contentEditable = 'true';
  box.spellcheck = false;
  $('stage').appendChild(box);
  renderInline();
  styleInline();
  placeInline();

  // Hide the SVG's own text for this block so we don't see it twice.
  const layer = $('stage').querySelector(`[data-drag="${g.id}:${key}"]`);
  if (layer) layer.classList.add('editing-hidden');
  handles[g.id + ':' + key]?.classList.add('editing');

  box.addEventListener('input', onInlineInput);
  box.addEventListener('keydown', onInlineKey);
  document.addEventListener('selectionchange', onInlineSelChange);
  box.focus();
  // put the caret at the end
  const r = document.createRange();
  r.selectNodeContents(box);
  r.collapse(false);
  const s = window.getSelection();
  s.removeAllRanges(); s.addRange(r);
  renderPane();
}

// Render the char model into the contenteditable as styled spans.
function renderInline() {
  const box = $('inline');
  if (!box) return;
  const runs = charsToRuns(chars);
  const info = selInfo();
  const blockSize = info ? info.el.font_pct * info.g.space.h : 40;
  box.innerHTML = runs.map(r => {
    const st = [];
    if (r.bold) st.push('font-weight:700');
    if (r.bold === false) st.push('font-weight:400');
    if (r.italic) st.push('font-style:italic');
    if (r.italic === false) st.push('font-style:normal');
    if (r.fill) st.push(`color:${r.fill}`);
    if (r.family) st.push(`font-family:'${r.family}'`);
    if (r.size) st.push(`font-size:${(blockSize * r.size).toFixed(2)}px`);
    const html = escapeHtml(r.t).replace(/\n/g, '<br>');
    return `<span data-r="1"${st.length ? ` style="${st.join(';')}"` : ''}>${html}</span>`;
  }).join('') || '<span data-r="1"></span>';
}

// Mirror the BLOCK's typography onto the editor box (font, size, color, alignment,
// line-height, tracking, case) so the overlay looks like the render.
function styleInline() {
  const box = $('inline');
  const info = selInfo();
  if (!box || !info) return;
  const { el, g } = info;
  const svgEl = $('stage').querySelector('svg');
  if (!svgEl) return;
  const r = svgEl.getBoundingClientRect();
  const scale = r.width / g.space.vbW;           // viewBox px → screen px
  const size = el.font_pct * g.space.h * scale;
  const st = el.font_style || 'normal';
  box.style.fontFamily = `'${el.font_family || 'Playfair Display'}'`;
  box.style.fontSize = size + 'px';
  box.style.fontWeight = st.includes('bold') ? 700 : 400;
  box.style.fontStyle = st.includes('italic') ? 'italic' : 'normal';
  box.style.color = el.fill || '#FFFFFF';
  box.style.textAlign = el.align || 'center';
  box.style.lineHeight = (el.line_height && el.line_height > 0 ? el.line_height : 1.0) * 1.32;
  box.style.letterSpacing = ((el.letter_spacing || 0) * scale) + 'px';
  box.style.textTransform = el.text_transform === 'upper' ? 'uppercase'
    : el.text_transform === 'lower' ? 'lowercase' : 'none';
  box.style.opacity = el.opacity != null ? el.opacity : 1;
  renderInline();   // per-run sizes are relative to the block size → re-emit
}

// Put the editor box exactly over the block's wrap box on the canvas.
function placeInline() {
  const box = $('inline');
  const info = selInfo();
  if (!box || !info) return;
  const { el, g } = info;
  const svgEl = $('stage').querySelector('svg');
  if (!svgEl) return;
  const r = svgEl.getBoundingClientRect();
  const sx = r.width / g.space.vbW, sy = r.height / g.space.vbH;
  const bw = el.w_pct * g.space.w;
  const cx = g.space.x0 + el.x_pct * g.space.w, cy = el.y_pct * g.space.h;
  const h = box.offsetHeight || 40;
  box.style.left = ((cx - bw / 2) * sx) + 'px';
  box.style.width = (bw * sx) + 'px';
  box.style.top = (cy * sy - h / 2) + 'px';
}

// Read the DOM back into the char model (typing changes it), preserving the style
// of each surviving character via its span.
function onInlineInput() {
  const box = $('inline');
  const out = [];
  const walk = (node, s) => {
    for (const n of node.childNodes) {
      if (n.nodeType === 3) {
        for (const ch of n.nodeValue) out.push({ ch, s: { ...s } });
      } else if (n.nodeName === 'BR') {
        out.push({ ch: '\n', s: { ...s } });
      } else if (n.nodeType === 1) {
        walk(n, styleOfSpan(n, s));
      }
    }
  };
  walk(box, {});
  chars = out;
  syncEl();
  placeInline();
}

// The run style a span carries (inline styles we wrote, or a browser-inserted
// <b>/<i> from a native bold shortcut).
function styleOfSpan(n, inherited) {
  const s = { ...inherited };
  const st = n.style || {};
  if (n.nodeName === 'B' || n.nodeName === 'STRONG' || st.fontWeight === '700' || st.fontWeight === 'bold') s.bold = true;
  else if (st.fontWeight === '400') s.bold = false;
  if (n.nodeName === 'I' || n.nodeName === 'EM' || st.fontStyle === 'italic') s.italic = true;
  else if (st.fontStyle === 'normal') s.italic = false;
  if (st.color) s.fill = rgbToHex(st.color);
  if (st.fontFamily) s.family = st.fontFamily.replace(/['"]/g, '');
  if (st.fontSize) {
    const info = selInfo();
    const blockSize = info ? info.el.font_pct * info.g.space.h : 0;
    const px = parseFloat(st.fontSize);
    if (blockSize > 0 && px > 0) {
      const mult = px / blockSize;
      if (Math.abs(mult - 1) > 0.01) s.size = Math.round(mult * 100) / 100;
    }
  }
  return s;
}

// Push the char model onto the selected element (plain text + runs).
function syncEl() {
  if (!editing) return;
  const el = editing.el;
  el.text = charsToText(chars);
  const runs = charsToRuns(chars);
  el.runs = runs.some(r => isStyled(r)) ? runs : undefined;
}

function onInlineKey(e) {
  if (e.key === 'Escape') { e.preventDefault(); commitEdit(); return; }
  if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') { e.preventDefault(); commitEdit(); return; }
  if ((e.metaKey || e.ctrlKey) && (e.key === 'b' || e.key === 'B')) { e.preventDefault(); applyType({ toggleBold: true }); return; }
  if ((e.metaKey || e.ctrlKey) && (e.key === 'i' || e.key === 'I')) { e.preventDefault(); applyType({ toggleItalic: true }); return; }
  if ((e.metaKey || e.ctrlKey) && (e.key === 's' || e.key === 'S')) { e.preventDefault(); commitEdit(); save(); }
}

function onInlineSelChange() {
  if (!editing) return;
  const box = $('inline');
  const s = window.getSelection();
  if (!s.rangeCount || !box.contains(s.anchorNode)) return;
  syncBar();
}

// The selected character range as [start, end) offsets into `chars`.
function selRange() {
  const box = $('inline');
  const s = window.getSelection();
  if (!box || !s.rangeCount || !box.contains(s.anchorNode)) return null;
  const r = s.getRangeAt(0);
  const off = (node, offset) => {
    // count characters before (node, offset) in document order
    let n = 0, done = false;
    const walk = (el) => {
      for (const c of el.childNodes) {
        if (done) return;
        if (c === node) {
          if (c.nodeType === 3) { n += offset; done = true; return; }
          // element node: offset counts child nodes
          for (let i = 0; i < offset && i < c.childNodes.length; i++) n += len(c.childNodes[i]);
          done = true; return;
        }
        if (c.nodeType === 3) n += c.nodeValue.length;
        else if (c.nodeName === 'BR') n += 1;
        else if (c.nodeType === 1) { walk(c); }
      }
    };
    const len = (el) => el.nodeType === 3 ? el.nodeValue.length : el.nodeName === 'BR' ? 1 : [...el.childNodes].reduce((a, x) => a + len(x), 0);
    walk(box);
    return n;
  };
  const a = off(r.startContainer, r.startOffset);
  const b = off(r.endContainer, r.endOffset);
  return { start: Math.min(a, b), end: Math.max(a, b) };
}

// The style at the caret / across the selection (undefined where runs disagree).
function selectionStyle() {
  const r = selRange();
  if (!r) return null;
  const i = r.start === r.end ? Math.max(0, r.start - 1) : r.start;
  const c = chars[i];
  if (!c) return null;
  if (r.end > r.start) {
    const slice = chars.slice(r.start, r.end);
    const s = {};
    for (const k of ['bold', 'italic', 'fill', 'family', 'size']) {
      const v = slice[0]?.s[k];
      s[k] = slice.every(x => x.s[k] === v) ? v : undefined;
    }
    return s;
  }
  return { ...c.s };
}

// Restore a character-offset selection after re-rendering the spans.
function restoreSel(start, end) {
  const box = $('inline');
  if (!box) return;
  let pos = 0, sNode = null, sOff = 0, eNode = null, eOff = 0;
  const visit = (el) => {
    for (const c of el.childNodes) {
      if (c.nodeType === 3) {
        const l = c.nodeValue.length;
        if (sNode === null && pos + l >= start) { sNode = c; sOff = start - pos; }
        if (eNode === null && pos + l >= end) { eNode = c; eOff = end - pos; }
        pos += l;
      } else if (c.nodeName === 'BR') {
        if (sNode === null && pos + 1 > start) { sNode = c.parentNode; sOff = 0; }
        pos += 1;
      } else if (c.nodeType === 1) visit(c);
    }
  };
  visit(box);
  if (!sNode || !eNode) return;
  const r = document.createRange();
  try { r.setStart(sNode, Math.max(0, sOff)); r.setEnd(eNode, Math.max(0, eOff)); } catch { return; }
  const s = window.getSelection();
  s.removeAllRanges(); s.addRange(r);
}

// Apply a type change. With an inline selection → per-character styling of just
// that range (a run). Without one → the whole block's style.
function applyType(op) {
  const info = selInfo();
  if (!info) return;
  const r = editing ? selRange() : null;

  if (r && r.end > r.start) {
    const cur = selectionStyle() || {};
    for (let i = r.start; i < r.end; i++) {
      const s = chars[i].s;
      if (op.clear) { chars[i].s = {}; continue; }
      if (op.toggleBold) s.bold = !cur.bold ? true : undefined;
      if (op.toggleItalic) s.italic = !cur.italic ? true : undefined;
      if (op.fill) s.fill = op.fill;
      if (op.family) s.family = op.family;
      if (op.sizePx) {
        const blockSize = Math.round(info.el.font_pct * info.g.sizeRef);
        const mult = Math.round((op.sizePx / blockSize) * 100) / 100;
        s.size = Math.abs(mult - 1) < 0.01 ? undefined : mult;
      }
    }
    syncEl();
    renderInline();
    restoreSel(r.start, r.end);
    syncBar();
    return;
  }

  // No selection → the block itself.
  const el = info.el;
  const st = el.font_style || 'normal';
  if (op.clear) { chars.forEach(c => (c.s = {})); syncEl(); if (editing) renderInline(); }
  if (op.toggleBold) el.font_style = styleStr(!st.includes('bold'), st.includes('italic'));
  if (op.toggleItalic) el.font_style = styleStr(st.includes('bold'), !st.includes('italic'));
  if (op.fill) el.fill = op.fill;
  if (op.family) el.font_family = op.family;
  if (op.sizePx) el.font_pct = op.sizePx / info.g.sizeRef;
  syncBar();
  afterStyle();
}

// Close the inline editor: write the text/runs back and re-render the real SVG.
function commitEdit() {
  const box = $('inline');
  if (!editing) return;
  const { g, key } = editing;
  syncEl();
  document.removeEventListener('selectionchange', onInlineSelChange);
  if (box) box.remove();
  const layer = $('stage').querySelector(`[data-drag="${g.id}:${key}"]`);
  if (layer) layer.classList.remove('editing-hidden');
  handles[g.id + ':' + key]?.classList.remove('editing');
  editing = null;
  renderPane();
  scheduleLivePreview();   // the authoritative SVG catches up
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
      if (editing) { commitEdit(); return; }
      if (current) select(null);
      return;
    }
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName) || e.target.isContentEditable;
    if (typing || e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === '?') { e.preventDefault(); toggleHelp(); return; }
    const arrows = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
    const info = selInfo();
    if (info && arrows[e.key]) {
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

// The editor's working layout, in the shape both the live-preview POST and the
// Save POST expect. The front layout (title/subtitle/author) is always included;
// the wrap view also carries the back panel (blurb/badge/author).
function currentBody() {
  const body = {
    title: els.title,
    subtitle: els.subtitle,
    author: els.author,
    bgcolor: bgcolor || '#000000',
  };
  body.blurb = (wrapEls.blurb && wrapEls.blurb.text) || '';
  body.wrap = {
    blurb: wrapEls.blurb || undefined,
    badge: wrapEls.badge || undefined,
    author: wrapEls.author || undefined,
  };
  return body;
}

async function save() {
  const body = currentBody();
  $('saveBtn').disabled = true; status('saving + rendering…');
  try {
    const r = await fetch(`/api/cover/${SLUG}/${LANG}`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
    }).then(r => r.json());
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
// How many lines the renderer will lay this block out on. Mirrors cover_svg.rs's
// `wrap`: an explicit \n is a HARD break and a blank line stays blank (that's what
// separates the blurb's paragraphs). Splitting on /\s+/ instead — as this used to —
// swallowed both, so a 4-paragraph blurb measured far shorter than it renders and
// the drag handle didn't cover its own text.
function wrapCount(text, size, style, family, maxW) {
  const italic = (style || '').includes('italic') ? 'italic ' : '';
  const weight = (style || '').includes('bold') ? 700 : 500;
  const fam = family ? `'${family}', ` : '';
  _measureCtx.font = `${italic}${weight} ${size}px ${fam}Montserrat, sans-serif`;

  let lines = 0;
  for (const hard of String(text || '').trim().split('\n')) {
    if (!hard.trim()) { lines++; continue; }        // paragraph gap
    const words = hard.split(/\s+/).filter(Boolean);
    let cur = '';
    lines++;
    for (const w of words) {
      const trial = cur ? cur + ' ' + w : w;
      if (_measureCtx.measureText(trial).width <= maxW || !cur) cur = trial;
      else { lines++; cur = w; }
    }
  }
  return Math.max(lines, 1);
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
// "rgb(212, 169, 55)" (what the DOM gives back) -> "#d4a937".
function rgbToHex(c) {
  const m = String(c).match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/);
  if (!m) return toHex6(c);
  return '#' + [1, 2, 3].map(i => (+m[i]).toString(16).padStart(2, '0')).join('');
}
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
