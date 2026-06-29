// bookmill cover editor (v1). Konva canvas over the bg image; drag/resize/restyle
// title/subtitle/author; save normalized fractions to TOML + re-render via the API.

const $ = (id) => document.getElementById(id);
const W = 1600, H = 2560;            // authoritative front-cover pixel space
let scale = 1;                       // display scale (canvas px -> screen px)
let stage, artLayer, textLayer, guideLayer, tr;
let nodes = {};                      // {title, subtitle, author} -> Konva.Text
let current = null;                  // selected key
let bgRect, bgImage;

init();

async function init() {
  const res = await fetch('/api/books').then(r => r.json());
  const sel = $('bookSel');
  res.books.forEach(b => {
    const o = document.createElement('option');
    o.value = b.slug;
    const t = b.titles.es || b.titles.en || b.slug;
    o.textContent = t + (b.protected ? '  (protected)' : '');
    o.dataset.langs = JSON.stringify(b.languages);
    sel.appendChild(o);
  });
  sel.onchange = onBookChange;
  $('langSel').onchange = loadCover;
  $('saveBtn').onclick = save;
  $('guidesOn').onchange = () => { guideLayer.visible($('guidesOn').checked); guideLayer.draw(); };

  bindStyleInputs();
  if (res.books.length) { onBookChange(); }
}

function onBookChange() {
  const opt = $('bookSel').selectedOptions[0];
  const langs = JSON.parse(opt.dataset.langs || '["es","en"]');
  const langSel = $('langSel');
  langSel.innerHTML = '';
  langs.forEach(l => {
    const o = document.createElement('option');
    o.value = l; o.textContent = l.toUpperCase();
    langSel.appendChild(o);
  });
  loadCover();
}

async function loadCover() {
  const slug = $('bookSel').value;
  const lang = $('langSel').value;
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
  textLayer = new Konva.Layer();
  guideLayer = new Konva.Layer({ listening: false });
  stage.add(artLayer, textLayer, guideLayer);

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
  node.on('transformend', () => {
    const s = node.scaleX();
    node.fontSize(Math.max(6, node.fontSize() * s));
    node.width(node.width() * s);
    node.scaleX(1); node.scaleY(1);
    textLayer.batchDraw();
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
  const slug = $('bookSel').value, lang = $('langSel').value;
  const body = {
    title: elJSON('title'),
    subtitle: elJSON('subtitle'),
    author: elJSON('author'),
    bgcolor: $('bgcolorHex').value || '#000000',
  };
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
