// Two-page interior previewer with KDP-style warnings.
//
// Renders verso+recto page PNGs (pdftoppm, via /api/preview/.../page/N) side by
// side on a Konva stage and overlays trim / bleed / safe-margin guide rectangles
// computed from the book's resolved print geometry. The warnings panel lists the
// issues from `bookmill validate --deep --json` (via /api/warnings), page-stamped
// where available (e.g. low-DPI images), each jumpable.

const $ = (id) => document.getElementById(id);
const GAP = 10;
const PAPER = '#ffffff';   // the page stock — blank pages are paper, not background

// The preview fills the window. Height is whatever is left under the nav bar once
// the legend + status line are accounted for; a spread that would then overflow
// sideways is scaled down to fit the width instead. Both views use this, so
// switching between the wrap and an interior spread keeps the same scale.
const CHROME_BELOW = 78;   // legend + status + bottom padding
const MIN_H = 260;
function avail() {
  const host = $('stage');
  const top = host.getBoundingClientRect().top;
  return {
    w: Math.max(MIN_H, host.parentElement.clientWidth - 40),
    h: Math.max(MIN_H, window.innerHeight - top - CHROME_BELOW),
  };
}
// Largest w x h of the given aspect that fits the free area.
function fit(aspect) {
  const a = avail();
  let h = a.h, w = h * aspect;
  if (w > a.w) { w = a.w; h = w / aspect; }
  return { w, h };
}
// Same, for a two-page spread: `GAP` px of gutter that does NOT scale with height.
function fitSpread(pageAspect) {
  const a = avail();
  let h = a.h, w = 2 * pageAspect * h + GAP;
  if (w > a.w) { h = (a.w - GAP) / (2 * pageAspect); w = a.w; }
  return { w, h };
}

let meta = null;        // /api/preview metadata (geometry, pages, dpi)
// spread index: 0 = the full paperback WRAP (back·spine·front); 1 = [—, p1];
// k>=2 = [p(2k-2), p(2k-1)]. The leading spread is the wrap so the previewer
// opens on the whole printed cover, not just the front.
let spread = 0;
let stage = null;
let warnPages = new Set();
let SLUG = null, LANG = null; // scope: ?book=<slug>&lang=<lang>
let LANGS = [];               // book's declared languages (for the ES/EN toggle)
let wantPage = null;          // optional ?page=<n> deep-link (jump on load)

init();

async function init() {
  const q = new URLSearchParams(location.search);
  SLUG = q.get('book');
  LANG = q.get('lang');
  const pg = parseInt(q.get('page'), 10);
  wantPage = isNaN(pg) ? null : pg;

  // Graceful fallback when opened without query params: pick the first book/lang.
  let books = [];
  try { books = (await fetch('/api/books').then((r) => r.json())).books || []; } catch { books = []; }
  if (!SLUG || !LANG) {
    const b = books[0];
    if (b) { SLUG = SLUG || b.slug; LANG = LANG || (b.languages[0] || 'es'); }
  }
  const me = books.find((x) => x.slug === SLUG);
  LANGS = (me && me.languages) || [LANG];

  $('bookLabel').textContent = SLUG;
  $('backBook').href = `/book.html?book=${encodeURIComponent(SLUG)}`;
  $('toCover').href = `/cover.html?book=${encodeURIComponent(SLUG)}&lang=${encodeURIComponent(LANG)}`;
  buildLangSeg();

  ['tTrim', 'tBleed', 'tSafe'].forEach((id) => ($(id).onchange = draw));
  $('prev').onclick = () => gotoSpread(spread - 1);
  $('next').onclick = () => gotoSpread(spread + 1);
  $('first').onclick = () => gotoSpread(0);
  $('last').onclick = () => gotoSpread(lastSpread());
  $('go').onclick = jumpToInput;
  $('jump').onkeydown = (e) => { if (e.key === 'Enter') jumpToInput(); };
  $('reload').onclick = loadWarnings;
  $('resClose').onclick = () => $('resultsPanel').classList.remove('open');
  $('counts').onclick = () => $('resultsPanel').classList.toggle('open');
  wireShortcuts();
  // The preview is sized to the window, so a resize has to re-fit it.
  let rt = null;
  window.addEventListener('resize', () => {
    clearTimeout(rt);
    rt = setTimeout(() => { if (meta && meta.pdfExists) draw(); }, 120);
  });
  load();
}

// ES/EN language toggle (matches the cover editor). Switches in place: the page
// count and geometry can differ per language, so reload metadata + redraw.
function buildLangSeg() {
  const seg = $('langSeg');
  seg.innerHTML = '';
  LANGS.forEach((l) => {
    const b = document.createElement('button');
    b.textContent = l.toUpperCase();
    b.className = l === LANG ? 'on' : '';
    b.onclick = () => { if (l !== LANG) switchLang(l); };
    seg.appendChild(b);
  });
}

function switchLang(l) {
  LANG = l;
  $('toCover').href = `/cover.html?book=${encodeURIComponent(SLUG)}&lang=${encodeURIComponent(LANG)}`;
  buildLangSeg();
  spread = 0;
  load();
}

// Keyboard shortcuts: arrows page, Home/End jump to ends, `g` focuses the page
// box, t/b/s toggle guides, `?` toggles help. Ignored while typing in a field.
function wireShortcuts() {
  const overlay = $('helpOverlay');
  let helpClose = null;
  const openHelp = () => { overlay.classList.add('open'); helpClose = a11y.openDialog(overlay); };
  const shutHelp = () => { overlay.classList.remove('open'); if (helpClose) { helpClose(); helpClose = null; } };
  const toggleHelp = () => (overlay.classList.contains('open') ? shutHelp() : openHelp());
  $('helpBtn').addEventListener('click', toggleHelp);
  overlay.addEventListener('click', shutHelp);
  const toggle = (id) => { const el = $(id); el.checked = !el.checked; draw(); };

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { shutHelp(); return; }
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
  // Honor a ?page=<n> deep-link once, then fall back to the wrap (spread 0).
  if (wantPage != null) { spread = pageToSpread(wantPage); wantPage = null; }
  else spread = 0;
  draw();
}

// Interior page n → its spread. Spread 0 is the wrap; spread 1 holds p1 alone,
// spread k>=2 holds [p(2k-2), p(2k-1)]. So n maps to floor(n/2)+1.
function pageToSpread(n) { return Math.floor(n / 2) + 1; }

function lastSpread() {
  const p = meta && meta.pages ? meta.pages : 1;
  return Math.floor(p / 2) + 1;   // +1: spread 0 is the wrap
}

function gotoSpread(n) {
  spread = Math.max(0, Math.min(lastSpread(), n));
  draw();
}

function jumpToInput() {
  const n = parseInt($('jump').value, 10);
  if (!isNaN(n)) gotoSpread(pageToSpread(n));
}

// Interior pages in the current spread (spread>=1): left (verso) + right (recto).
// spread 1 = [—, p1]; spread k>=2 = [p(2k-2), p(2k-1)].
function spreadPages() {
  const i = spread;                                   // >=1 here (0 is the wrap)
  const left = i <= 1 ? null : 2 * (i - 1);           // even page (verso)
  const right = 2 * i - 1;                            // odd page (recto)
  const P = meta && meta.pages ? meta.pages : 1;
  return [left && left <= P ? left : null, right <= P ? right : null];
}

async function draw() {
  if (!meta || !meta.pdfExists) return;
  if (spread === 0) return drawWrap();
  return drawInterior();
}

// Leading view: the full paperback WRAP (back·spine·front) — the authoritative
// resvg SVG, the exact output `build cover` rasterizes. Shown wide so the
// previewer opens on the whole printed cover, not just the front face.
async function drawWrap() {
  if (stage) { stage.destroy(); stage = null; }
  const host = $('stage');
  host.innerHTML = '<div class="loadingwrap">loading wrap…</div>';
  updateNav('wrap · back·spine·front', meta.pages || 1);
  try {
    const svg = await fetch(`/api/cover/${SLUG}/${LANG}/svg?wrap=1&t=${Date.now()}`)
      .then((r) => (r.ok ? r.text() : r.text().then((t) => { throw new Error(t); })));
    host.innerHTML = `<div class="wrapview">${svg}</div>`;
    const svgEl = host.querySelector('svg');
    if (svgEl) {
      const vb = (svgEl.getAttribute('viewBox') || '0 0 1200 880').split(/\s+/).map(Number);
      const { w, h } = fit((vb[2] || 1200) / (vb[3] || 880));
      svgEl.removeAttribute('width'); svgEl.removeAttribute('height');
      svgEl.style.width = w + 'px';
      svgEl.style.height = h + 'px';
    }
  } catch (e) {
    host.innerHTML = `<div class="err-msg">wrap render failed:\n${escapeHtml((e && e.message) || e)}</div>`;
  }
}

// Interior two-page spread (verso+recto) with trim/bleed/safe guides — the
// proven Konva path over the pdftoppm page rasters.
function drawInterior() {
  const g = meta.geometry;
  const slug = SLUG, lang = LANG;
  const [lp, rp] = spreadPages();
  const P = meta.pages || 1;

  const aspect = g ? g.pageW / g.pageH : 6.125 / 9.25;
  const { w: W, h: H } = fitSpread(aspect);
  const pageWpx = H * aspect;

  const host = $('stage');
  if (stage) stage.destroy();
  host.innerHTML = '';                 // drop any prior wrap SVG before Konva mounts
  stage = new Konva.Stage({ container: 'stage', width: W, height: H });
  const art = new Konva.Layer(), guides = new Konva.Layer();
  stage.add(art, guides);

  drawPage(art, guides, lp, 0, pageWpx, H, 'verso', slug, lang);
  drawPage(art, guides, rp, pageWpx + GAP, pageWpx, H, 'recto', slug, lang);

  updateNav(`${lp ? 'p' + lp : '—'} · ${rp ? 'p' + rp : '—'}`, P);
}

// Shared page-label + nav-button state. (No status line in the normal case —
// book/lang live in the header, the page count in the label; status is reserved
// for errors.)
function updateNav(leftLabel, P) {
  $('pglabel').textContent = `${leftLabel}  ·  of ${P}`;
  $('prev').disabled = spread <= 0;
  $('first').disabled = spread <= 0;
  $('next').disabled = spread >= lastSpread();
  $('last').disabled = spread >= lastSpread();
  $('status').textContent = '';
}

function drawPage(art, guides, page, ox, pageWpx, H, side, slug, lang) {
  // An empty slot is not "nothing" — it is a blank sheet of paper the reader will
  // hold (the verso facing page 1, or the tail of an odd-length book). Paint it
  // white so the spread reads like the printed book, not like a hole in the UI.
  if (!page) {
    art.add(new Konva.Rect({ x: ox, y: 0, width: pageWpx, height: H, fill: PAPER }));
    guides.add(new Konva.Rect({ x: ox, y: 0, width: pageWpx, height: H, stroke: '#222b36', strokeWidth: 1, dash: [4, 4] }));
    return;
  }
  // page background + raster — both on `art` (the BOTTOM layer) so the opaque
  // background sits under the page image, not over it. `guides` is the top layer
  // (strokes/tints only); putting the fill there hid the raster and the error text.
  art.add(new Konva.Rect({ x: ox, y: 0, width: pageWpx, height: H, fill: PAPER }));
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

  // trim box (in inches, in page-local coords). bleed on outer + top + bottom.
  const ty0 = g.bleed, ty1 = g.bleed + g.trimH;
  let tx0, tx1;
  if (recto) { tx0 = 0; tx1 = g.trimW; }            // outer = right; inner flush left
  else       { tx0 = g.bleed; tx1 = g.pageW; }      // outer = left; inner flush right

  // safe area: KDP's Print Previewer draws the "safe" guide at a FIXED 0.25"
  // inside the trim on all four sides — it is a keep-important-content-inside
  // guide, not the book's actual text-block margins. Earlier versions drew this
  // box from the Typst page margins (m.inner/m.outer/m.top/m.bottom), which are
  // larger than 0.25", so the guide sat too far in and never matched Amazon's
  // Print Previewer. Inset the *trim* box (tx*/ty*, already bleed-aware per side)
  // by KDP_SAFE so the overlay lands exactly on KDP's guide.
  const KDP_SAFE = 0.25; // inches from trim, per KDP paperback guidelines
  const sx0 = tx0 + KDP_SAFE, sx1 = tx1 - KDP_SAFE;
  const sy0 = ty0 + KDP_SAFE, sy1 = ty1 - KDP_SAFE;

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
  $('counts').innerHTML = '';
  $('issues').innerHTML = '';
  $('resultsPanel').classList.remove('open');
}

async function loadWarnings() {
  const slug = SLUG, lang = LANG;
  $('reload').disabled = true;
  $('resultsPanel').classList.add('open');
  $('counts').innerHTML = '';
  $('issues').innerHTML = '<p class="muted">validating — this can take a minute…</p>';
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
  a11y.announce(`Validation complete: ${s.ok || 0} ok, ${s.warn || 0} warnings, ${s.error || 0} errors`);

  $('resultsPanel').classList.add('open');
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

function escapeHtml(s) { return String(s).replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c])); }
