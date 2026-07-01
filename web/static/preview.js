// Two-page interior previewer with KDP-style warnings.
//
// Renders verso+recto page PNGs (pdftoppm, via /api/preview/.../page/N) side by
// side on a Konva stage and overlays trim / bleed / safe-margin guide rectangles
// computed from the book's resolved print geometry. The warnings panel lists the
// issues from `bookmill validate --deep --json` (via /api/warnings), page-stamped
// where available (e.g. low-DPI images), each jumpable.

const $ = (id) => document.getElementById(id);
const PAGE_H = 440; // displayed page height in px (both pages share it)
const GAP = 10;

let meta = null;        // /api/preview metadata (geometry, pages, dpi)
let spread = 0;         // spread index: 0 = [—, p1], k = [p2k, p2k+1]
let stage = null;
let warnPages = new Set();
let SLUG = null, LANG = null; // scope: ?book=<slug>&lang=<lang>
let wantPage = null;          // optional ?page=<n> deep-link (jump on load)

init();

async function init() {
  const q = new URLSearchParams(location.search);
  SLUG = q.get('book');
  LANG = q.get('lang');
  const pg = parseInt(q.get('page'), 10);
  wantPage = isNaN(pg) ? null : pg;

  // Graceful fallback when opened without query params: pick the first book/lang.
  if (!SLUG || !LANG) {
    const res = await fetch('/api/books').then((r) => r.json());
    const b = res.books[0];
    if (b) { SLUG = SLUG || b.slug; LANG = LANG || (b.languages[0] || 'es'); }
  }

  $('bookLabel').textContent = `${SLUG} · ${LANG.toUpperCase()}`;
  $('backBook').href = `/book.html?book=${encodeURIComponent(SLUG)}`;
  $('toCover').href = `/cover.html?book=${encodeURIComponent(SLUG)}&lang=${encodeURIComponent(LANG)}`;

  ['tTrim', 'tBleed', 'tSafe'].forEach((id) => ($(id).onchange = draw));
  $('prev').onclick = () => gotoSpread(spread - 1);
  $('next').onclick = () => gotoSpread(spread + 1);
  $('first').onclick = () => gotoSpread(0);
  $('last').onclick = () => gotoSpread(lastSpread());
  $('go').onclick = jumpToInput;
  $('jump').onkeydown = (e) => { if (e.key === 'Enter') jumpToInput(); };
  $('reload').onclick = loadWarnings;
  wireShortcuts();
  load();
}

// Keyboard shortcuts: arrows page, Home/End jump to ends, `g` focuses the page
// box, t/b/s toggle guides, `?` toggles help. Ignored while typing in a field.
function wireShortcuts() {
  const overlay = $('helpOverlay');
  const toggleHelp = () => overlay.classList.toggle('open');
  $('helpBtn').addEventListener('click', toggleHelp);
  overlay.addEventListener('click', () => overlay.classList.remove('open'));
  const toggle = (id) => { const el = $(id); el.checked = !el.checked; draw(); };

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { overlay.classList.remove('open'); return; }
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName) || e.target.isContentEditable;
    if (typing) return;
    switch (e.key) {
      case 'ArrowLeft': e.preventDefault(); gotoSpread(spread - 1); break;
      case 'ArrowRight': e.preventDefault(); gotoSpread(spread + 1); break;
      case 'Home': e.preventDefault(); gotoSpread(0); break;
      case 'End': e.preventDefault(); gotoSpread(lastSpread()); break;
      case 'g': e.preventDefault(); $('jump').focus(); $('jump').select(); break;
      case 't': e.preventDefault(); toggle('tTrim'); break;
      case 'b': e.preventDefault(); toggle('tBleed'); break;
      case 's': e.preventDefault(); toggle('tSafe'); break;
      case '?': e.preventDefault(); toggleHelp(); break;
    }
  });
}

async function load() {
  const slug = SLUG, lang = LANG;
  warnPages = new Set();
  resetWarningsPanel();
  $('status').textContent = 'loading metadata…';
  try {
    meta = await fetch(`/api/preview/${slug}/${lang}`).then((r) => r.json());
  } catch (e) {
    meta = null;
    $('status').textContent = 'failed to load metadata';
    return;
  }
  if (!meta.pdfExists) {
    $('status').innerHTML = `<span class="err-msg">No built KDP PDF for ${slug}/${lang}.\nRun: bookmill build ${slug} --lang ${lang} --format print</span>`;
    if (stage) { stage.destroy(); stage = null; }
    $('pglabel').textContent = '—';
    return;
  }
  $('jump').max = meta.pages || 1;
  // Honor a ?page=<n> deep-link once, then fall back to the first spread.
  if (wantPage != null) { spread = Math.floor(wantPage / 2); wantPage = null; }
  else spread = 0;
  draw();
}

function lastSpread() {
  const p = meta && meta.pages ? meta.pages : 1;
  return Math.floor(p / 2);
}

function gotoSpread(n) {
  spread = Math.max(0, Math.min(lastSpread(), n));
  draw();
}

function jumpToInput() {
  const n = parseInt($('jump').value, 10);
  if (!isNaN(n)) gotoSpread(Math.floor(n / 2));
}

// pages shown in the current spread: left (verso, even) + right (recto, odd).
function spreadPages() {
  const left = spread === 0 ? null : spread * 2;       // even page
  const right = spread * 2 + 1;                          // odd page
  const P = meta && meta.pages ? meta.pages : 1;
  return [left && left <= P ? left : null, right <= P ? right : null];
}

async function draw() {
  if (!meta || !meta.pdfExists) return;
  const g = meta.geometry;
  const slug = SLUG, lang = LANG;
  const [lp, rp] = spreadPages();
  const P = meta.pages || 1;

  // page aspect from geometry (full page incl. bleed)
  const aspect = g ? g.pageW / g.pageH : 6.125 / 9.25;
  const pageWpx = PAGE_H * aspect;
  const W = pageWpx * 2 + GAP, H = PAGE_H;

  if (stage) stage.destroy();
  stage = new Konva.Stage({ container: 'stage', width: W, height: H });
  const art = new Konva.Layer(), guides = new Konva.Layer();
  stage.add(art, guides);

  // page slots: left at x=0, right at x=pageWpx+GAP
  drawPage(art, guides, lp, 0, pageWpx, H, 'verso', slug, lang);
  drawPage(art, guides, rp, pageWpx + GAP, pageWpx, H, 'recto', slug, lang);

  $('pglabel').textContent = `${lp ? 'p' + lp : '—'} · ${rp ? 'p' + rp : '—'}  (of ${P})`;
  $('prev').disabled = spread <= 0;
  $('first').disabled = spread <= 0;
  $('next').disabled = spread >= lastSpread();
  $('last').disabled = spread >= lastSpread();
  $('status').textContent = `${slug} · ${lang} · ${P} pages · preview ${meta.previewDpi || ''}dpi`;
}

function drawPage(art, guides, page, ox, pageWpx, H, side, slug, lang) {
  // empty slot (e.g. left page of the first spread, or past the last page)
  if (!page) {
    guides.add(new Konva.Rect({ x: ox, y: 0, width: pageWpx, height: H, stroke: '#222b36', strokeWidth: 1, dash: [4, 4] }));
    return;
  }
  // page background + raster — both on `art` (the BOTTOM layer) so the opaque
  // background sits under the page image, not over it. `guides` is the top layer
  // (strokes/tints only); putting the fill there hid the raster and the error text.
  art.add(new Konva.Rect({ x: ox, y: 0, width: pageWpx, height: H, fill: '#15191f' }));
  const pageUrl = `/api/preview/${slug}/${lang}/page/${page}?t=${Date.now()}`;
  Konva.Image.fromURL(pageUrl, (img) => {
    img.setAttrs({ x: ox, y: 0, width: pageWpx, height: H });
    art.add(img);
    art.draw();
  }, () => {
    // The raster failed (e.g. pdftoppm missing / render error). Surface the
    // backend's error text in the page slot instead of leaving it blank.
    showPageError(art, page, ox, pageWpx, H, pageUrl);
  });

  // page warned by the audit? tint the slot.
  if (warnPages.has(page)) {
    guides.add(new Konva.Rect({ x: ox, y: 0, width: pageWpx, height: H, fill: 'rgba(229,96,95,0.12)' }));
  }

  drawGuides(guides, page, ox, pageWpx, H, side);
}

// Fetch the failing page URL to recover the backend error text and paint it on
// the page slot (Konva.Image.fromURL swallows the HTTP body on error), so a
// missing pdftoppm / render failure is visible instead of a blank frame.
async function showPageError(art, page, ox, pageWpx, H, url) {
  let detail = 'could not render this page';
  try {
    const res = await fetch(url);
    if (!res.ok) detail = (await res.text()) || `HTTP ${res.status}`;
  } catch (e) {
    detail = (e && e.message) || String(e);
  }
  const pad = 12;
  art.add(new Konva.Text({
    x: ox + pad, y: pad, width: pageWpx - 2 * pad, height: H - 2 * pad,
    text: `⚠ page ${page}\n\n${detail}`,
    fontSize: 13, lineHeight: 1.4, fill: '#e5605f',
    fontFamily: 'ui-monospace, Menlo, monospace', align: 'left', verticalAlign: 'middle',
  }));
  art.draw();
  $('status').innerHTML = `<span class="err-msg">Preview render failed for page ${page}: ${detail}</span>`;
}

// Overlay trim / bleed / safe-margin rectangles for one page. Geometry is in
// inches; KDP print pages add bleed on the OUTER edge + top + bottom (not the
// spine/inner edge). recto = right page (outer edge right), verso = left (outer left).
function drawGuides(guides, page, ox, pageWpx, H, side) {
  const g = meta.geometry;
  if (!g) return;
  const sx = pageWpx / g.pageW, sy = H / g.pageH; // inches -> px
  const X = (xin) => ox + xin * sx;
  const Y = (yin) => yin * sy;
  const recto = side === 'recto';
  const m = g.margins;

  // trim box (in inches, in page-local coords). bleed on outer + top + bottom.
  const ty0 = g.bleed, ty1 = g.bleed + g.trimH;
  let tx0, tx1;
  if (recto) { tx0 = 0; tx1 = g.trimW; }            // outer = right; inner flush left
  else       { tx0 = g.bleed; tx1 = g.pageW; }      // outer = left; inner flush right

  // safe (text block): inset from trim by margins; spine side adds bindingoffset.
  const innerInset = m.inner + (m.bindingoffset || 0);
  let sx0, sx1;
  if (recto) { sx0 = tx0 + innerInset; sx1 = tx1 - m.outer; }   // inner=left, outer=right
  else       { sx0 = tx0 + m.outer;    sx1 = tx1 - innerInset; } // inner=right, outer=left
  const sy0 = ty0 + m.top, sy1 = ty1 - m.bottom;

  if ($('tBleed').checked) {
    // bleed = full page edge (everything outside trim gets cut)
    guides.add(new Konva.Rect({ x: ox, y: 0, width: pageWpx, height: H, stroke: '#ff5a5a', strokeWidth: 1.5 }));
  }
  if ($('tTrim').checked) {
    guides.add(new Konva.Rect({ x: X(tx0), y: Y(ty0), width: (tx1 - tx0) * sx, height: (ty1 - ty0) * sy, stroke: '#ffffff', strokeWidth: 1.5 }));
  }
  if ($('tSafe').checked) {
    guides.add(new Konva.Rect({ x: X(sx0), y: Y(sy0), width: (sx1 - sx0) * sx, height: (sy1 - sy0) * sy, stroke: '#56b6ff', strokeWidth: 1.5, dash: [8, 6] }));
  }
}

// -------------------------------------------------------------------------
// Warnings panel (/api/warnings -> bookmill validate --deep --json)
// -------------------------------------------------------------------------

function resetWarningsPanel() {
  $('counts').innerHTML = '<span class="muted">not loaded</span>';
  $('issues').innerHTML = '<p class="muted">Click “run validate --deep” to audit this book.</p>';
}

async function loadWarnings() {
  const slug = SLUG, lang = LANG;
  $('reload').disabled = true;
  $('counts').innerHTML = '<span class="muted">running validate --deep… (may take a while)</span>';
  $('issues').innerHTML = '';
  try {
    const res = await fetch(`/api/warnings/${slug}/${lang}`);
    if (!res.ok) throw new Error(await res.text());
    const data = await res.json();
    renderWarnings(data);
  } catch (e) {
    $('counts').innerHTML = '';
    $('issues').innerHTML = `<p class="err-msg">${(e && e.message) || e}</p>`;
  } finally {
    $('reload').disabled = false;
  }
}

function renderWarnings(data) {
  const s = data.summary || {};
  $('counts').innerHTML =
    `<span class="pill ok">${s.ok || 0} ok</span>` +
    `<span class="pill warn">${s.warn || 0} warn</span>` +
    `<span class="pill err">${s.error || 0} error</span>`;

  warnPages = new Set();
  const issues = (data.issues || []).slice().sort((a, b) => rank(b.level) - rank(a.level));
  const host = $('issues');
  host.innerHTML = '';
  if (!issues.length) { host.innerHTML = '<p class="muted">No issues.</p>'; return; }
  issues.forEach((it) => {
    if (it.page && it.level !== 'ok') warnPages.add(it.page);
    const div = document.createElement('div');
    div.className = `issue ${it.level}`;
    const meta = document.createElement('div');
    meta.className = 'meta';
    meta.innerHTML =
      `<span>${it.level.toUpperCase()}</span>` +
      (it.lang ? `<span>${it.lang}</span>` : '') +
      (it.page ? `<span class="pgbtn" data-pg="${it.page}">page ${it.page} →</span>` : '');
    const msg = document.createElement('div');
    msg.textContent = it.message;
    div.appendChild(meta);
    div.appendChild(msg);
    host.appendChild(div);
  });
  host.querySelectorAll('.pgbtn').forEach((b) => {
    b.onclick = () => gotoSpread(Math.floor(parseInt(b.dataset.pg, 10) / 2));
  });
  draw(); // re-tint warned pages
}

function rank(level) { return level === 'error' ? 3 : level === 'warn' ? 2 : 1; }
