// bookmill cover editor (v1). Konva canvas over the bg image; drag/resize/restyle
// title/subtitle/author; save normalized fractions to TOML + re-render via the API.

const $ = (id) => document.getElementById(id);
const W = 1600, H = 2560;            // authoritative front-cover pixel space
// Flex-layout constants — must match cover_svg.rs::front_svg exactly.
const PAD_X = 120, TOP = 150, BOTTOM = H - 150, CONTENT_W = W - 2 * PAD_X; // 1360
const BADGE_SIZE = 32, BADGE_LH = 1.2, BADGE_LS = 9;
const RULE_W = 220, RULE_H = 3, RULE_GAP = 46;
let scale = 1;                       // display scale (canvas px -> screen px)
let stage, artLayer, decoLayer, textLayer, guideLayer, tr;
let nodes = {};                      // {title, subtitle, author} -> Konva.Text
let badgeNode = null, ruleNode = null; // fixed decorations (not draggable/saved)
let accentColor = '#d4a937';
let current = null;                  // selected key
let bgRect, bgImage;
let SLUG = null, LANG = null;        // scope: ?book=<slug>&lang=<lang>

init();

async function init() {
  const q = new URLSearchParams(location.search);
  SLUG = q.get('book');
  LANG = q.get('lang');

  // Graceful fallback when opened without query params: pick the first book/lang.
  if (!SLUG || !LANG) {
    const res = await fetch('/api/books').then(r => r.json());
    const b = res.books[0];
    if (b) { SLUG = SLUG || b.slug; LANG = LANG || (b.languages[0] || 'es'); }
  }

  $('bookLabel').textContent = `${SLUG} · ${LANG.toUpperCase()}`;
  $('backBook').href = `/book.html?book=${encodeURIComponent(SLUG)}`;
  $('toPreview').href = `/preview.html?book=${encodeURIComponent(SLUG)}&lang=${encodeURIComponent(LANG)}`;

  $('saveBtn').onclick = save;
  $('guidesOn').onchange = () => { guideLayer.visible($('guidesOn').checked); guideLayer.draw(); };
  $('coverType').onchange = applyCoverType;
  $('genAudiobook').onclick = genAudiobookCover;
  $('openWrap').href = `/api/output/${encodeURIComponent(SLUG)}/${encodeURIComponent(LANG)}/wrap-cover`;

  bindStyleInputs();
  wireShortcuts();
  loadCover();
}

// Cover-type selector: Front (default eBook front editing), Paperback wrap (front
// title block + back-cover blurb), Audiobook (square, inferred from the front).
// The canvas always edits the shared FRONT title block; wrap/audiobook add their
// own panels. (TODO: full drag layout of all three wrap panels.)
function applyCoverType() {
  const t = $('coverType').value;
  $('wrapSection').style.display = t === 'wrap' ? '' : 'none';
  $('audiobookSection').style.display = t === 'audiobook' ? '' : 'none';
  // In audiobook mode the canvas title edits don't apply; keep Save enabled only
  // for front/wrap (audiobook is generated via its own button).
  $('saveBtn').style.display = t === 'audiobook' ? 'none' : '';
}

// Generate the square audiobook cover by cropping the rendered front (backend).
async function genAudiobookCover() {
  const btn = $('genAudiobook');
  btn.disabled = true; status('generating audiobook cover…');
  try {
    const r = await fetch(`/api/cover/${SLUG}/${LANG}/audiobook`, { method: 'POST' })
      .then((res) => res.ok ? res.json() : res.text().then((t) => { throw new Error(t); }));
    const img = $('audiobookImg');
    img.src = r.url; img.style.display = '';
    status('audiobook cover saved');
  } catch (e) {
    status('audiobook cover failed: ' + ((e && e.message) || e));
  } finally {
    btn.disabled = false;
  }
}

// Keyboard shortcuts: Cmd/Ctrl+S saves; arrow keys nudge the selected element
// (Shift = larger step); Esc deselects; `?` toggles the help overlay. Arrow
// nudging and `?` are ignored while typing in a field (so text edits are safe).
function wireShortcuts() {
  const overlay = $('helpOverlay');
  const toggleHelp = () => overlay.classList.toggle('open');
  $('helpBtn').addEventListener('click', toggleHelp);
  overlay.addEventListener('click', () => overlay.classList.remove('open'));

  document.addEventListener('keydown', (e) => {
    // Cmd/Ctrl+S saves from anywhere (even while typing).
    if ((e.metaKey || e.ctrlKey) && (e.key === 's' || e.key === 'S')) {
      e.preventDefault();
      if (!$('saveBtn').disabled) save();
      return;
    }
    if (e.key === 'Escape') {
      if (overlay.classList.contains('open')) { overlay.classList.remove('open'); return; }
      if (current) select(null);
      return;
    }
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName) || e.target.isContentEditable;
    if (typing || e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === '?') { e.preventDefault(); toggleHelp(); return; }
    // Arrow-key nudge of the selected element.
    const arrows = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
    if (current && arrows[e.key]) {
      e.preventDefault();
      const step = e.shiftKey ? 40 : 8;   // canvas px
      const [dx, dy] = arrows[e.key];
      const n = nodes[current];
      n.x(n.x() + dx * step);
      n.y(n.y() + dy * step);
      if (current === 'title') updateDecorations();
      if (tr) tr.forceUpdate();
      textLayer.batchDraw();
    }
  });
}

async function loadCover() {
  const slug = SLUG;
  const lang = LANG;
  status('loading…');
  const data = await fetch(`/api/cover/${slug}/${lang}`).then(r => r.json());
  buildStage(data);
  $('protBadge').style.display = data.protected ? '' : 'none';

  // authoritative render preview
  const img = $('renderImg');
  if (data.rendered_url) { img.src = data.rendered_url + '?t=' + Date.now(); img.style.display = ''; }
  else { img.style.display = 'none'; }

  $('bgcolor').value = toHex6(data.bgcolor);
  $('bgcolorHex').value = data.bgcolor;
  // Back-cover blurb (paperback wrap mode).
  $('blurbText').value = data.blurb || '';
  applyCoverType();
  status('loaded ' + slug + ' / ' + lang);
}

function buildStage(data) {
  // fit the 1600x2560 canvas into the viewport height
  const host = document.querySelector('.stage-host');
  const targetH = Math.min(host.clientHeight - 40, 900);
  scale = targetH / H;

  if (stage) stage.destroy();
  stage = new Konva.Stage({ container: 'editor', width: W * scale, height: H * scale });
  stage.scale({ x: scale, y: scale });

  artLayer = new Konva.Layer();
  decoLayer = new Konva.Layer({ listening: false });
  textLayer = new Konva.Layer();
  guideLayer = new Konva.Layer({ listening: false });
  stage.add(artLayer, decoLayer, textLayer, guideLayer);

  accentColor = data.accent || '#d4a937';

  // No saved [cover.<lang>.layout] → seed title/subtitle/author at the SAME
  // positions cover_svg.rs's flex render uses (title top with title_mt, subtitle
  // under, author at bottom), so the editor is WYSIWYG with "What ships".
  if (!data.layout_saved) applyFlexDefaults(data);

  // bg solid + image
  bgRect = new Konva.Rect({ x: 0, y: 0, width: W, height: H, fill: data.bgcolor });
  artLayer.add(bgRect);
  bgImage = null;
  Konva.Image.fromURL(data.bg_url, (img) => {
    // cover-fit the image into the canvas
    const iw = img.width(), ih = img.height();
    const s = Math.max(W / iw, H / ih);
    img.setAttrs({ width: iw * s, height: ih * s, x: (W - iw * s) / 2, y: (H - ih * s) / 2, listening: false });
    artLayer.add(img); img.moveToTop();
    bgImage = img;
    artLayer.draw();
  }, () => {});

  // text nodes
  nodes = {};
  current = null;
  ['title', 'subtitle', 'author'].forEach(key => makeText(key, data.elements[key]));

  // Fixed decorations the renderer always draws but the editor does not save:
  // the series badge (top-center) and the accent rule under the title block.
  badgeNode = new Konva.Text({
    text: (data.badge || '').toUpperCase(),
    x: PAD_X, y: TOP, width: CONTENT_W, align: 'center',
    fontSize: BADGE_SIZE, fontFamily: 'Montserrat, sans-serif',
    letterSpacing: BADGE_LS, fill: data.badge_color || accentColor,
    opacity: 0.95, listening: false,
  });
  decoLayer.add(badgeNode);
  ruleNode = new Konva.Rect({
    x: W / 2 - RULE_W / 2, y: TOP, width: RULE_W, height: RULE_H,
    fill: accentColor, opacity: 0.85, listening: false,
  });
  decoLayer.add(ruleNode);
  updateDecorations();

  // transformer
  tr = new Konva.Transformer({
    keepRatio: true,
    enabledAnchors: ['top-left', 'top-right', 'bottom-left', 'bottom-right'],
    rotateEnabled: false,
    borderStroke: '#d4a937', anchorStroke: '#d4a937', anchorFill: '#1a1206',
    boundBoxFunc: (oldB, newB) => (newB.width < 30 ? oldB : newB),
  });
  textLayer.add(tr);

  stage.on('click tap', (e) => {
    if (e.target === stage || e.target === bgRect || (bgImage && e.target === bgImage)) {
      select(null);
    }
  });

  drawGuides();
  selectPanel();
  artLayer.draw(); textLayer.draw();
}

function makeText(key, el) {
  const width = el.w_pct * W;
  const node = new Konva.Text({
    text: el.text,
    x: el.x_pct * W,            // temporary; recentred below
    y: el.y_pct * H,
    width,
    align: 'center',
    fontSize: el.font_pct * H,
    fontFamily: el.font_family || 'sans-serif',
    fontStyle: el.font_style || 'normal',
    fill: el.fill || '#ffffff',
    draggable: true,
    shadowColor: 'black', shadowBlur: 8, shadowOpacity: 0.55,
  });
  // position by center: x/y were stored as the block centre
  node.x(el.x_pct * W - width / 2);
  node.y(el.y_pct * H - node.height() / 2);
  node._key = key;

  node.on('click tap', (e) => { e.cancelBubble = true; select(key); });
  node.on('dblclick dbltap', () => { select(key); $('selText').focus(); });
  if (key === 'title') node.on('dragmove', updateDecorations);
  node.on('transformend', () => {
    const s = node.scaleX();
    node.fontSize(Math.max(6, node.fontSize() * s));
    node.width(node.width() * s);
    node.scaleX(1); node.scaleY(1);
    textLayer.batchDraw();
    if (key === 'title') updateDecorations();
    if (current === key) selectPanel();
  });
  textLayer.add(node);
  nodes[key] = node;
}

function select(key) {
  current = key;
  if (!key) { tr.nodes([]); }
  else { tr.nodes([nodes[key]]); }
  textLayer.draw();
  selectPanel();
}

function selectPanel() {
  const has = !!current;
  $('selPanel').style.display = has ? '' : 'none';
  $('noSel').style.display = has ? 'none' : '';
  if (!has) return;
  const n = nodes[current];
  $('selName').textContent = current;
  $('selText').value = n.text();
  $('selFill').value = toHex6(n.fill());
  $('selFillHex').value = n.fill();
  $('selSize').value = Math.round(n.fontSize());
}

function bindStyleInputs() {
  $('selText').oninput = () => { if (current) { nodes[current].text($('selText').value); textLayer.batchDraw(); } };
  const setFill = (v) => { if (current) { nodes[current].fill(v); textLayer.batchDraw(); } };
  $('selFill').oninput = () => { $('selFillHex').value = $('selFill').value; setFill($('selFill').value); };
  $('selFillHex').oninput = () => { $('selFill').value = toHex6($('selFillHex').value); setFill($('selFillHex').value); };
  $('selSize').onchange = () => {
    if (!current) return;
    const v = parseFloat($('selSize').value);
    if (v > 0) { nodes[current].fontSize(v); textLayer.batchDraw(); tr.forceUpdate(); }
  };
  const setBg = (v) => { if (bgRect) { bgRect.fill(v); artLayer.batchDraw(); } };
  $('bgcolor').oninput = () => { $('bgcolorHex').value = $('bgcolor').value; setBg($('bgcolor').value); };
  $('bgcolorHex').oninput = () => { $('bgcolor').value = toHex6($('bgcolorHex').value); setBg($('bgcolorHex').value); };
}

function drawGuides() {
  guideLayer.destroyChildren();
  const dash = [14, 12];
  // trim (full eBook front — no bleed on Kindle eBook)
  guideLayer.add(new Konva.Rect({ x: 0, y: 0, width: W, height: H, stroke: '#ffffff', strokeWidth: 2, opacity: 0.5 }));
  // safe text margin (~0.6in equiv => ~120px inset, matches renderer pad)
  const inset = 120;
  guideLayer.add(new Konva.Rect({ x: inset, y: inset, width: W - 2 * inset, height: H - 2 * inset, stroke: '#56b6ff', strokeWidth: 2, dash, opacity: 0.7 }));
  // center line
  guideLayer.add(new Konva.Line({ points: [W / 2, 0, W / 2, H], stroke: '#d4a937', strokeWidth: 1, dash: [6, 10], opacity: 0.4 }));
  guideLayer.visible($('guidesOn').checked);
  guideLayer.draw();
}

// Keep the accent rule glued under the title block (renderer draws it 46px below
// the title, centered); badge stays fixed at the top.
function updateDecorations() {
  if (ruleNode && nodes.title) {
    const t = nodes.title;
    ruleNode.y(t.y() + t.height() + RULE_GAP);
    ruleNode.x(t.x() + t.width() / 2 - RULE_W / 2);
  }
  if (decoLayer) decoLayer.batchDraw();
}

// Reproduce cover_svg.rs::front_svg's flex stack to seed default element centers
// when no [cover.<lang>.layout] is saved. Mutates data.elements.{title,subtitle,
// author}.{x_pct,y_pct} in place. Coordinates are canvas px (1600x2560).
function applyFlexDefaults(data) {
  const els = data.elements;
  const titleSize = els.title.font_pct * H;
  const subSize = els.subtitle.font_pct * H;

  const badgeH = BADGE_SIZE * BADGE_LH;                      // 38.4
  const titleLines = twoLineParts(els.title.text).length;   // 1 or 2
  const titleH = titleLines * (titleSize * 1.05);
  const ruleBlock = RULE_GAP + RULE_H;                       // 49
  const subLines = wrapCount(els.subtitle.text, subSize, els.subtitle.font_style, CONTENT_W);
  const subBlockH = subLines * (subSize * 1.3);
  const subH = 40 + subBlockH;
  const titleBlockH = titleH + ruleBlock + subH;
  const authorH = 42 * 1.2;                                 // 50.4

  const g1m = parseMargin(data.title_mt, CONTENT_W);         // title block top
  const g2m = parseMargin(data.author_mt, CONTENT_W);        // author top
  const fixed = badgeH + titleBlockH + authorH + (g1m.auto ? 0 : g1m.px) + (g2m.auto ? 0 : g2m.px);
  const autos = (g1m.auto ? 1 : 0) + (g2m.auto ? 1 : 0);
  const leftover = Math.max(0, BOTTOM - TOP - fixed);
  const share = autos > 0 ? leftover / autos : 0;
  const g1 = g1m.auto ? share : g1m.px;
  const g2 = g2m.auto ? share : g2m.px;

  let y = TOP;
  y += badgeH + g1;                       // title top
  const titleCenter = y + titleH / 2;
  y += titleH + ruleBlock;                // past title + rule
  const subTop = y + 40;
  const subCenter = subTop + subBlockH / 2;
  y = subTop + subBlockH + g2;            // author top
  const authorCenter = y + authorH / 2;

  els.title.y_pct = titleCenter / H;
  els.subtitle.y_pct = subCenter / H;
  els.author.y_pct = authorCenter / H;
  els.title.x_pct = els.subtitle.x_pct = els.author.x_pct = 0.5;
}

// Balanced two-line split, mirroring cover_tmpl.rs::two_line_parts.
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

// Count greedy word-wrap lines for `text` at `size` px within `maxW`, using
// canvas metrics (approximates cover_svg.rs::wrap; exact wrap needs bundled font
// metrics, but line-count is what drives the vertical stack).
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

// Parse a flex margin: `auto`, `<n>%` (of ref width), or `<n>px`/`<n>`.
function parseMargin(s, refW) {
  s = (s || '').trim();
  if (s === 'auto') return { auto: true, px: 0 };
  if (s.endsWith('%')) return { auto: false, px: (parseFloat(s) || 0) / 100 * refW };
  return { auto: false, px: parseFloat(s) || 0 };
}

function elJSON(key) {
  const n = nodes[key];
  const w = n.width();
  const h = n.height();
  return {
    text: n.text(),
    x_pct: (n.x() + w / 2) / W,
    y_pct: (n.y() + h / 2) / H,
    w_pct: w / W,
    font_pct: n.fontSize() / H,
    fill: n.fill(),
    font_family: n.fontFamily(),
    font_style: n.fontStyle(),
  };
}

async function save() {
  const slug = SLUG, lang = LANG;
  const body = {
    title: elJSON('title'),
    subtitle: elJSON('subtitle'),
    author: elJSON('author'),
    bgcolor: $('bgcolorHex').value || '#000000',
  };
  // In paperback-wrap mode, also persist the back-cover blurb before re-render.
  if ($('coverType').value === 'wrap') body.blurb = $('blurbText').value;
  $('saveBtn').disabled = true; status('saving + rendering…');
  try {
    const r = await fetch(`/api/cover/${slug}/${lang}`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
    }).then(r => r.json());
    const log = $('log');
    log.style.display = ''; log.textContent = (r.ok ? '✓ ' : '✗ ') + 'saved ' + r.saved + '\n\n' + (r.renderLog || '');
    if (r.renderedUrl) { const img = $('renderImg'); img.src = r.renderedUrl; img.style.display = ''; }
    status(r.ok ? 'saved + rendered' : 'saved (render failed)');
  } catch (e) {
    status('error: ' + e);
  } finally {
    $('saveBtn').disabled = false;
  }
}

function status(s) { $('status').textContent = s; }

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
