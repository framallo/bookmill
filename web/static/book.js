// Book detail = attributes + a TUI-style build panorama (editions × languages ×
// formats) + per-language actions and publish readiness. Arrow keys navigate
// prev/next book across the repo (when the panorama is not focused); when the
// panorama has focus, ←/→ switch list focus and ↑/↓ move within the focused list.

const $ = (id) => document.getElementById(id);
const params = new URLSearchParams(location.search);
const slug = params.get('book');
const initialLang = params.get('lang');

let ALLBOOKS = [];   // ordered [{slug, languages}] from /api/books (for nav + check-all)
let DATA = null;     // /api/book/{slug} response

// Unified keyboard model. A single focus ring walks every field on the page —
// the prev-book control, the three lists (Editions/Languages/Formats), and the
// next-book control — in that visual left-to-right order. ←/→ (or Tab) move focus
// between fields; ↑/↓ toggle the value inside a focused list (each list keeps its
// own cursor); Enter on prev/next changes book. Formats mirror the TUI's FORMATS
// (minus the "edition" meta-selector, which the dedicated Editions list covers).
const FORMATS = ['all', 'epub', 'pdf', 'kdp', 'print'];
const LISTS = ['editions', 'langs', 'formats'];
const mx = {
  lists: { editions: [], langs: [], formats: FORMATS.slice() },
  cur: { editions: 0, langs: 0, formats: 0 }, // each list keeps its own cursor
  focus: 'editions',                          // one of: prev, editions, langs, formats, next
};

// The focus ring for the current book: prev-book (unless first) → the three
// lists → next-book (unless last). Disabled nav ends are skipped so ←/→ never
// lands on a dead control.
function focusRing() {
  const i = bookIndex();
  const ring = [];
  if (i > 0) ring.push('prev');
  ring.push(...LISTS);
  if (i >= 0 && i < ALLBOOKS.length - 1) ring.push('next');
  return ring;
}
function isList(f) { return LISTS.includes(f); }

init();

async function init() {
  if (!slug) { $('main').innerHTML = '<p class="muted">No ?book=<slug> given. <a class="crumb" href="/">← all books</a></p>'; return; }

  // Book ordering for prev/next navigation + the "check all" sweep.
  try {
    const r = await fetch('/api/books').then((x) => x.json());
    ALLBOOKS = (r.books || []).map((b) => ({ slug: b.slug, languages: b.languages || [] }));
  } catch (e) { ALLBOOKS = []; }
  wireNav();

  let d;
  try {
    const res = await fetch(`/api/book/${encodeURIComponent(slug)}`);
    if (!res.ok) throw new Error(await res.text());
    d = await res.json();
  } catch (e) {
    $('main').innerHTML = `<p class="muted">Failed to load <code>${esc(slug)}</code>: ${esc((e && e.message) || e)}</p>`;
    return;
  }
  DATA = d;
  initMatrixState(d);
  render(d);
}

// --- prev/next book navigation (arrow keys + header buttons) ----------------

function bookIndex() { return ALLBOOKS.findIndex((b) => b.slug === slug); }

function gotoBook(delta) {
  const i = bookIndex();
  if (i < 0) return;
  const j = i + delta;
  if (j < 0 || j >= ALLBOOKS.length) return;
  location.href = `/book.html?book=${encodeURIComponent(ALLBOOKS[j].slug)}`;
}

function wireNav() {
  const i = bookIndex();
  const prev = $('prevBook'), next = $('nextBook');
  prev.disabled = !(i > 0);
  next.disabled = !(i >= 0 && i < ALLBOOKS.length - 1);
  prev.onclick = () => gotoBook(-1);
  next.onclick = () => gotoBook(1);
  if (i >= 0 && ALLBOOKS.length) $('navpos').textContent = `${i + 1} / ${ALLBOOKS.length}`;
  $('checkAll').onclick = checkAllBooks;

  document.addEventListener('keydown', (e) => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName) || e.target.isContentEditable;
    if (typing) return;

    // One unified focus ring over every field: prev-book · Editions · Languages ·
    // Formats · next-book. ←/→ (and Tab) walk the ring; ↑/↓ toggle inside a list;
    // Enter activates the focused prev/next control.
    switch (e.key) {
      case 'ArrowLeft': e.preventDefault(); moveFocus(-1); break;
      case 'ArrowRight': e.preventDefault(); moveFocus(1); break;
      case 'Tab': e.preventDefault(); moveFocus(e.shiftKey ? -1 : 1); break;
      case 'ArrowUp': if (isList(mx.focus)) { e.preventDefault(); moveWithin(-1); } break;
      case 'ArrowDown': if (isList(mx.focus)) { e.preventDefault(); moveWithin(1); } break;
      case 'Home': if (isList(mx.focus)) { e.preventDefault(); setWithin(0); } break;
      case 'End': if (isList(mx.focus)) { e.preventDefault(); setWithin(mx.lists[mx.focus].length - 1); } break;
      case 'Enter':
      case ' ':
        if (mx.focus === 'prev') { e.preventDefault(); gotoBook(-1); }
        else if (mx.focus === 'next') { e.preventDefault(); gotoBook(1); }
        break;
    }
  });
}

// --- build panorama (TUI-style visible lists) -------------------------------

function initMatrixState(d) {
  mx.lists.editions = ['all', ...(d.editions || [])];
  mx.lists.langs = ['all', ...(d.languages || [])];
  mx.lists.formats = FORMATS.slice();
  mx.cur = { editions: 0, langs: 0, formats: 0 };
  // Honor ?lang=<l>: preselect it in the Languages list.
  if (initialLang) {
    const li = mx.lists.langs.indexOf(initialLang);
    if (li >= 0) mx.cur.langs = li;
  }
  mx.focus = 'editions';
}

function moveFocus(delta) {
  const ring = focusRing();
  let i = ring.indexOf(mx.focus);
  if (i < 0) i = 0; // focus fell off the ring (e.g. prev/next removed) → snap in
  mx.focus = ring[Math.max(0, Math.min(ring.length - 1, i + delta))];
  paintMatrix();
}

function moveWithin(delta) {
  if (!isList(mx.focus)) return;
  const key = mx.focus;
  const n = mx.lists[key].length;
  if (!n) return;
  mx.cur[key] = Math.max(0, Math.min(n - 1, mx.cur[key] + delta)); // cursor preserved per list
  paintMatrix();
}

function setWithin(idx) {
  if (!isList(mx.focus)) return;
  const key = mx.focus;
  const n = mx.lists[key].length;
  if (!n) return;
  mx.cur[key] = Math.max(0, Math.min(n - 1, idx));
  paintMatrix();
}

function selVal(key) { return mx.lists[key][mx.cur[key]]; }

// Mirror the TUI's cmd_string: a specific format wins; else an explicit edition;
// else a plain build. `all`/`all` lang means "every language".
function matrixCommand() {
  const ed = selVal('editions'), lang = selVal('langs'), fmt = selVal('formats');
  let c = `bookmill build ${slug}`;
  if (fmt !== 'all') c += ` --format ${fmt}`;
  else if (ed !== 'all') c += ` --edition ${ed}`;
  if (lang !== 'all') c += ` --lang ${lang}`;
  return c;
}

function matrixHtml() {
  const col = (key, label) => {
    const opts = mx.lists[key].map((o, i) =>
      `<div class="mopt${i === mx.cur[key] ? ' sel' : ''}" data-list="${key}" data-i="${i}">${esc(o)}</div>`).join('');
    return `<div class="mcol${mx.focus === key ? ' active' : ''}" data-col="${key}">
       <div class="mh">${esc(label)}</div>${opts}</div>`;
  };
  return `<h2>Build panorama</h2>
    <div class="matrix" id="matrix">
      ${col('editions', 'Editions')}
      ${col('langs', 'Languages')}
      ${col('formats', 'Formats')}
    </div>
    <div class="cmdprev" id="cmdprev">${esc(matrixCommand())}</div>
    <p class="mhint">One keyboard path: ←/→ (or Tab) move focus across ‹ prev · Editions · Languages · Formats · next › · ↑/↓ change the focused list · Enter on ‹/› switches book.</p>`;
}

function wireMatrix() {
  const m = $('matrix');
  if (!m) return;
  m.querySelectorAll('.mopt').forEach((el) => {
    el.addEventListener('click', () => {
      const key = el.dataset.list;
      mx.focus = key;
      mx.cur[key] = parseInt(el.dataset.i, 10);
      paintMatrix();
    });
  });
  // Clicking a column header (not an option) focuses that list.
  m.querySelectorAll('.mcol').forEach((c) => {
    c.addEventListener('click', (e) => {
      if (e.target.classList.contains('mopt')) return;
      mx.focus = c.dataset.col;
      paintMatrix();
    });
  });
  // Hovering prev/next parks focus there so a click reads as "focus then move".
  ['prev', 'next'].forEach((k) => {
    const b = $(k === 'prev' ? 'prevBook' : 'nextBook');
    if (b) b.addEventListener('mouseenter', () => { if (!b.disabled) { mx.focus = k; paintMatrix(); } });
  });
  paintMatrix();
}

// Repaint selection + focus (columns AND the prev/next controls) without a
// full page rebuild.
function paintMatrix() {
  const m = $('matrix');
  if (!m) return;
  m.querySelectorAll('.mcol').forEach((c) => c.classList.toggle('active', c.dataset.col === mx.focus));
  m.querySelectorAll('.mopt').forEach((el) =>
    el.classList.toggle('sel', parseInt(el.dataset.i, 10) === mx.cur[el.dataset.list]));
  const cp = $('cmdprev');
  if (cp) cp.textContent = matrixCommand();
  const prev = $('prevBook'), next = $('nextBook');
  if (prev) prev.classList.toggle('kfocus', mx.focus === 'prev');
  if (next) next.classList.toggle('kfocus', mx.focus === 'next');
}

// --- page render ------------------------------------------------------------

function render(d) {
  const firstTitle = (d.langs[d.languages[0]] && d.langs[d.languages[0]].title) || d.slug;
  const editions = (d.editions || []).map((e) => `<span class="tag">${esc(e)}</span>`).join('') || '<span class="muted">—</span>';

  let html = '';
  html += `<h1>${esc(firstTitle)}</h1>`;
  html += `<div class="slug">${esc(d.slug)}</div>`;
  html += `<div class="meta">`;
  if (d.author) html += `<span>by <b style="color:var(--ink)">${esc(d.author)}</b></span>`;
  if (d.series) html += `<span>series: ${esc(d.series)}</span>`;
  html += `<span>${(d.languages || []).map((l) => l.toUpperCase()).join(' · ')}</span>`;
  if (d.protected) html += `<span class="tag prot">protected</span>`;
  html += `</div>`;

  html += `<h2>Editions</h2><div class="editions">${editions}</div>`;

  html += matrixHtml();

  html += `<h2>Languages</h2>`;
  (d.languages || []).forEach((lang) => {
    const L = d.langs[lang] || {};
    const li = L.listing;
    const rid = `rd-${lang}`;
    const q = `book=${encodeURIComponent(d.slug)}&lang=${encodeURIComponent(lang)}`;
    html += `<div class="langcard">`;
    html += `<div class="lh"><span class="code">${lang.toUpperCase()}</span>`;
    html += `<span class="title">${esc(L.title || '(untitled)')}</span></div>`;
    if (L.subtitle) html += `<div class="sub">${esc(L.subtitle)}</div>`;
    html += `<div class="facts">`;
    html += `<span><b>${L.pages != null ? L.pages : '—'}</b> pages</span>`;
    html += `<span>KDP PDF: <b>${L.kdpPdfExists ? 'built' : 'not built'}</b></span>`;
    if (li) {
      html += `<span><b>${(li.keywords || []).length}</b> keywords</span>`;
      html += `<span>BISAC: <b>${(li.bisac || []).join(', ') || '—'}</b></span>`;
      if (li.readingAge) html += `<span>age: <b>${esc(li.readingAge)}</b></span>`;
      if (li.blurbChars != null) html += `<span>blurb: <b>${li.blurbChars}</b> chars</span>`;
    }
    html += `</div>`;

    // One aligned action bar: cover / preview / readiness trigger.
    html += `<div class="actions">`;
    html += `<a class="btn primary" href="/cover.html?${q}">Edit cover</a>`;
    html += `<a class="btn" href="/preview.html?${q}">Preview book</a>`;
    html += `<button class="btn" id="${rid}-btn">Check readiness</button>`;
    html += `</div>`;

    // Readiness panel (collapsed until run; then expanded only if it has errors).
    html += `<div class="readiness">`;
    html += `<div class="rhead">`;
    html += `<h3>Publish readiness</h3>`;
    html += `<span class="pills" id="${rid}-pills"></span>`;
    html += `<span class="verdict" id="${rid}-verdict" style="display:none"></span>`;
    html += `<button class="caret" id="${rid}-caret" style="display:none"></button>`;
    html += `</div>`;
    html += `<div class="rbody collapsed" id="${rid}-body"></div>`;
    html += `</div>`;

    html += `</div>`;
  });

  $('main').innerHTML = html;

  wireMatrix();
  (d.languages || []).forEach((lang) => {
    const btn = $(`rd-${lang}-btn`);
    if (btn) btn.onclick = () => checkReadiness(d.slug, lang);
  });
}

// --- Publish readiness ------------------------------------------------------

const KINDS = [
  { key: 'config', label: 'Listing & house rules', test: (m) => /^config\b/.test(m) },
  { key: 'epub', label: 'EPUB / epubcheck', test: (m) => /^EPUB\b/.test(m) },
  { key: 'pdf', label: 'PDF geometry', test: (m) => /^PDF\b/.test(m) },
  { key: 'bleed', label: 'Bleed coverage', test: (m) => /^bleed\b/.test(m) },
  { key: 'dpi', label: 'Image DPI', test: (m) => /^image DPI\b/.test(m) },
  { key: 'cover', label: 'Cover', test: (m) => /^cover\b/.test(m) },
];
function kindOf(message) {
  const m = String(message || '');
  return KINDS.find((k) => k.test(m)) || { key: 'other', label: 'Other' };
}
function rank(level) { return level === 'error' ? 3 : level === 'warn' ? 2 : 1; }

function tally(issues) {
  let errs = 0, warns = 0, oks = 0;
  issues.forEach((it) => {
    if (it.level === 'error') errs++;
    else if (it.level === 'warn') warns++;
    else oks++;
  });
  return { errs, warns, oks };
}

async function checkReadiness(bslug, lang) {
  const rid = `rd-${lang}`;
  const btn = $(`${rid}-btn`);
  const body = $(`${rid}-body`);
  const pills = $(`${rid}-pills`);
  if (btn) btn.disabled = true;
  pills.innerHTML = '';
  $(`${rid}-verdict`).style.display = 'none';
  $(`${rid}-caret`).style.display = 'none';
  body.classList.remove('collapsed');
  body.innerHTML = `<p class="muted"><span class="spin"></span> running checks… this can take a minute</p>`;
  try {
    const res = await fetch(`/api/warnings/${encodeURIComponent(bslug)}/${encodeURIComponent(lang)}`);
    if (!res.ok) throw new Error(await res.text());
    const data = await res.json();
    renderReadiness(bslug, lang, data);
  } catch (e) {
    body.innerHTML = `<p class="err-msg">${esc((e && e.message) || e)}</p>`;
  } finally {
    if (btn) { btn.disabled = false; btn.textContent = 'Re-check'; } // always re-runnable
  }
}

function renderReadiness(bslug, lang, data) {
  const rid = `rd-${lang}`;
  const issues = (data.issues || []).slice();
  const { errs, warns, oks } = tally(issues);

  $(`${rid}-pills`).innerHTML =
    `<span class="pill ok">${oks} ok</span>` +
    `<span class="pill warn">${warns} warn</span>` +
    `<span class="pill err">${errs} error</span>`;

  let v, vcls, vtext;
  if (errs > 0) { v = '❌'; vcls = 'notready'; vtext = 'Not ready'; }
  else if (warns > 0) { v = '⚠️'; vcls = 'warn'; vtext = 'Ready with warnings'; }
  else { v = '✅'; vcls = 'ready'; vtext = 'Ready'; }
  const vd = $(`${rid}-verdict`);
  vd.className = `verdict ${vcls}`;
  vd.textContent = `${v} ${vtext}`;
  vd.style.display = '';

  // Build the grouped issue body.
  const groups = new Map();
  KINDS.forEach((k) => groups.set(k.key, { label: k.label, items: [] }));
  issues.forEach((it) => {
    const k = kindOf(it.message);
    if (!groups.has(k.key)) groups.set(k.key, { label: k.label, items: [] });
    groups.get(k.key).items.push(it);
  });
  let bodyHtml = '';
  let any = false;
  for (const [, grp] of groups) {
    if (!grp.items.length) continue;
    any = true;
    grp.items.sort((a, b) => rank(b.level) - rank(a.level));
    const ge = grp.items.filter((i) => i.level === 'error').length;
    const gw = grp.items.filter((i) => i.level === 'warn').length;
    const badge = ge ? `${ge} error${ge === 1 ? '' : 's'}` : gw ? `${gw} warning${gw === 1 ? '' : 's'}` : 'ok';
    bodyHtml += `<div class="igroup"><div class="gh">${esc(grp.label)} <span class="gcount">${esc(badge)}</span></div>`;
    grp.items.forEach((it) => {
      bodyHtml += `<div class="issue ${esc(it.level)}"><span class="lv">${esc(it.level)}</span>`;
      bodyHtml += `<span class="imsg">${esc(it.message)}</span>`;
      if (it.page) {
        const pq = `book=${encodeURIComponent(bslug)}&lang=${encodeURIComponent(lang)}&page=${it.page}`;
        bodyHtml += `<a class="pgbtn" href="/preview.html?${pq}">page ${it.page} →</a>`;
      }
      bodyHtml += `</div>`;
    });
    bodyHtml += `</div>`;
  }
  if (!any) bodyHtml = `<p class="muted">No checks reported.</p>`;
  const body = $(`${rid}-body`);
  body.innerHTML = bodyHtml;

  // Collapse unless there is an error; expose a caret to toggle either way.
  const caret = $(`${rid}-caret`);
  caret.style.display = '';
  const setCollapsed = (collapsed) => {
    body.classList.toggle('collapsed', collapsed);
    caret.textContent = collapsed ? `▸ show details` : `▾ hide details`;
  };
  setCollapsed(errs === 0);
  caret.onclick = () => setCollapsed(!body.classList.contains('collapsed'));
}

// --- Check all books --------------------------------------------------------

async function checkAllBooks() {
  const btn = $('checkAll');
  const panel = $('checkAllPanel');
  btn.disabled = true;
  const jobs = [];
  ALLBOOKS.forEach((b) => (b.languages || []).forEach((l) => jobs.push({ slug: b.slug, lang: l })));

  panel.innerHTML =
    `<h2 style="margin-top:18px">All books · publish readiness</h2>` +
    `<p class="muted" id="ca-status"><span class="spin"></span> running validate --deep across ${jobs.length} book/language targets…</p>` +
    `<div id="ca-rows"></div>`;
  const rows = $('ca-rows');

  let done = 0;
  for (const j of jobs) {
    const rowId = `ca-${j.slug}-${j.lang}`;
    const row = document.createElement('div');
    row.className = 'carow';
    row.id = rowId;
    row.innerHTML = `<span class="caslug">${esc(j.slug)}</span><span class="calang">${esc(j.lang.toUpperCase())}</span><span class="pill muted"><span class="spin"></span></span>`;
    rows.appendChild(row);
    try {
      const res = await fetch(`/api/warnings/${encodeURIComponent(j.slug)}/${encodeURIComponent(j.lang)}`);
      if (!res.ok) throw new Error(await res.text());
      const data = await res.json();
      const { errs, warns } = tally(data.issues || []);
      let vcls = 'ready', v = '✅ ready';
      if (errs > 0) { vcls = 'notready'; v = `❌ ${errs} error${errs === 1 ? '' : 's'}`; }
      else if (warns > 0) { vcls = 'warn'; v = `⚠️ ${warns} warn`; }
      row.innerHTML =
        `<span class="caslug">${esc(j.slug)}</span><span class="calang">${esc(j.lang.toUpperCase())}</span>` +
        `<a class="verdict ${vcls}" style="text-decoration:none" href="/book.html?book=${encodeURIComponent(j.slug)}&lang=${encodeURIComponent(j.lang)}">${v}</a>`;
    } catch (e) {
      row.innerHTML =
        `<span class="caslug">${esc(j.slug)}</span><span class="calang">${esc(j.lang.toUpperCase())}</span>` +
        `<span class="err-msg">${esc((e && e.message) || e)}</span>`;
    }
    done++;
    const st = $('ca-status');
    if (st) st.innerHTML = done < jobs.length
      ? `<span class="spin"></span> checked ${done}/${jobs.length}…`
      : `Done — checked ${jobs.length} book/language targets.`;
  }
  btn.disabled = false; // re-runnable once finished
}

function esc(s) {
  return String(s == null ? '' : s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
}
