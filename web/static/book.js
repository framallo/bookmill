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

// Unified keyboard model. A SINGLE focus ring walks EVERY actionable control on
// the page — prev-book, the three lists (Editions/Languages/Formats), each
// language's action buttons (Edit cover · Preview book · Check readiness · the
// readiness details toggle when present), the page-level "Check all books", and
// next-book. ←/→ (and Tab) move focus along the ring; ↑/↓ toggle the value inside
// a focused list (each list keeps its own cursor); Enter/Space activates a focused
// button/link. The focused control's id is persisted per book so a refresh lands
// on the same control. Formats mirror the TUI's FORMATS (minus the "edition"
// meta-selector, which the dedicated Editions list covers).
const FORMATS = ['all', 'epub', 'pdf', 'kdp', 'print'];
const LISTS = ['editions', 'langs', 'formats'];
const mx = {
  lists: { editions: [], langs: [], formats: FORMATS.slice() },
  cur: { editions: 0, langs: 0, formats: 0 }, // each list keeps its own cursor
  focus: 'editions',                          // ring id of the focused control
};

function isList(f) { return LISTS.includes(f); }

// The DOM element for a ring id (every ring control carries data-ring="<id>").
function ringEl(id) {
  try { return document.querySelector(`[data-ring="${window.CSS && CSS.escape ? CSS.escape(id) : id}"]`); }
  catch (e) { return null; }
}
function isHidden(el) { return !el || (el.offsetParent === null && getComputedStyle(el).position !== 'fixed'); }

// The ordered focus ring for the current page state. Intuitive left-to-right /
// top-to-bottom order: prev-book → build panorama lists → per-language action
// buttons (in language order) → Check all books → next-book. Controls that are
// absent (first/last book) or hidden (the readiness caret before a run) are
// filtered out so focus never lands on a dead/invisible target.
function buildRing() {
  const i = bookIndex();
  const want = [];
  if (i > 0) want.push('prev');
  want.push(...LISTS);
  ((DATA && DATA.languages) || []).forEach((l) => {
    want.push(`cover-${l}`, `preview-${l}`, `readiness-${l}`, `caret-${l}`);
  });
  want.push('checkall');
  if (i >= 0 && i < ALLBOOKS.length - 1) want.push('next');
  return want.filter((id) => isList(id) ? !!ringEl(id) : !isHidden(ringEl(id)));
}

// --- focus persistence (per book) ------------------------------------------
const focusKey = () => `bm.book.${slug}.focus`;
function saveFocus() { try { localStorage.setItem(focusKey(), mx.focus); } catch (e) {} }
function loadFocus() { try { return localStorage.getItem(focusKey()); } catch (e) { return null; } }

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

    // One unified focus ring over every control: prev-book · Editions ·
    // Languages · Formats · per-language buttons · Check all · next-book. ←/→ (and
    // Tab) walk the ring; ↑/↓ toggle inside a focused list; Enter/Space activates a
    // focused button/link. Keyboard navigation drops any lingering native focus so
    // the ring highlight is the single source of truth (and Enter never fires twice).
    switch (e.key) {
      case 'ArrowLeft': e.preventDefault(); blurNative(); moveFocus(-1); break;
      case 'ArrowRight': e.preventDefault(); blurNative(); moveFocus(1); break;
      case 'Tab': e.preventDefault(); blurNative(); moveFocus(e.shiftKey ? -1 : 1); break;
      case 'ArrowUp': if (isList(mx.focus)) { e.preventDefault(); moveWithin(-1); } break;
      case 'ArrowDown': if (isList(mx.focus)) { e.preventDefault(); moveWithin(1); } break;
      case 'Home': if (isList(mx.focus)) { e.preventDefault(); setWithin(0); } break;
      case 'End': if (isList(mx.focus)) { e.preventDefault(); setWithin(mx.lists[mx.focus].length - 1); } break;
      case 'Enter':
      case ' ':
        if (isList(mx.focus)) break; // lists change value via ↑/↓, not activation
        // If the browser already focuses this control, let it activate natively
        // (avoids a double fire); otherwise activate the ring target ourselves.
        if (e.target && (e.target.tagName === 'BUTTON' || e.target.tagName === 'A')) break;
        { const el = ringEl(mx.focus); if (el) { e.preventDefault(); el.click(); } }
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

function blurNative() {
  const a = document.activeElement;
  if (a && a !== document.body && a.blur) a.blur();
}

function moveFocus(delta) {
  const ring = buildRing();
  if (!ring.length) return;
  let i = ring.indexOf(mx.focus);
  if (i < 0) i = delta > 0 ? -1 : 0; // fell off the ring → step onto an end
  i = (i + delta + ring.length) % ring.length; // wrap so the ring cycles
  mx.focus = ring[i];
  paintFocus();
}

function moveWithin(delta) {
  if (!isList(mx.focus)) return;
  const key = mx.focus;
  const n = mx.lists[key].length;
  if (!n) return;
  mx.cur[key] = Math.max(0, Math.min(n - 1, mx.cur[key] + delta)); // cursor preserved per list
  paintFocus();
}

function setWithin(idx) {
  if (!isList(mx.focus)) return;
  const key = mx.focus;
  const n = mx.lists[key].length;
  if (!n) return;
  mx.cur[key] = Math.max(0, Math.min(n - 1, idx));
  paintFocus();
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
    return `<div class="mcol" data-col="${key}" data-ring="${key}">
       <div class="mh">${esc(label)}</div>${opts}</div>`;
  };
  return `<h2>Build panorama</h2>
    <div class="matrix" id="matrix">
      ${col('editions', 'Editions')}
      ${col('langs', 'Languages')}
      ${col('formats', 'Formats')}
    </div>
    <div class="cmdprev" id="cmdprev">${esc(matrixCommand())}</div>
    <p class="mhint">One keyboard path over every control: ←/→ (or Tab) move focus ‹ prev-book → Editions → Languages → Formats → each language's Edit cover / Preview / Check readiness (+details) → Check all books → next-book › · ↑/↓ change the focused list · Enter/Space activates the focused button. Focus is remembered across refresh.</p>`;
}

// Wire pointer interactions to the same focus model: clicking any ring control
// moves the ring focus onto it (and, for a list option, selects that value).
function wireRing() {
  const m = $('matrix');
  if (m) {
    m.querySelectorAll('.mopt').forEach((el) => {
      el.addEventListener('click', () => {
        const key = el.dataset.list;
        mx.focus = key;
        mx.cur[key] = parseInt(el.dataset.i, 10);
        paintFocus();
      });
    });
    m.querySelectorAll('.mcol').forEach((c) => {
      c.addEventListener('click', (e) => {
        if (e.target.classList.contains('mopt')) return; // header click focuses the list
        mx.focus = c.dataset.col;
        paintFocus();
      });
    });
  }
  // Any other ring control (buttons/links): clicking or natively focusing it
  // moves the ring focus onto it, so the highlight and the persisted focus stay
  // in sync with pointer use. (No mouseenter — focus should not chase the cursor.)
  document.querySelectorAll('[data-ring]').forEach((el) => {
    if (el.classList.contains('mcol')) return; // handled above
    const id = el.dataset.ring;
    el.addEventListener('click', () => { mx.focus = id; paintFocus(); }); // before nav for links
    el.addEventListener('focus', () => { mx.focus = id; paintFocus(); });
  });
}

// Repaint the whole focus state: list selection values, the focused list column,
// the .kfocus ring on the focused control, the command preview — then persist
// the focused id so a refresh restores it.
function paintFocus() {
  const m = $('matrix');
  if (m) {
    m.querySelectorAll('.mopt').forEach((el) =>
      el.classList.toggle('sel', parseInt(el.dataset.i, 10) === mx.cur[el.dataset.list]));
    m.querySelectorAll('.mcol').forEach((c) => c.classList.toggle('active', c.dataset.col === mx.focus));
  }
  const cp = $('cmdprev');
  if (cp) cp.textContent = matrixCommand();
  // .kfocus on exactly the focused ring control.
  document.querySelectorAll('.kfocus').forEach((el) => el.classList.remove('kfocus'));
  const fe = ringEl(mx.focus);
  if (fe) fe.classList.add('kfocus');
  saveFocus();
}

// Restore the persisted focus for this book, falling back to a sensible default
// (the Editions list) when the saved control no longer exists.
function restoreFocus() {
  const ring = buildRing();
  const saved = loadFocus();
  mx.focus = (saved && ring.includes(saved)) ? saved
    : (ring.includes('editions') ? 'editions' : ring[0]);
  paintFocus();
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
    html += `<a class="btn primary" data-ring="cover-${lang}" href="/cover.html?${q}">Edit cover</a>`;
    html += `<a class="btn" data-ring="preview-${lang}" href="/preview.html?${q}">Preview book</a>`;
    html += `<button class="btn" data-ring="readiness-${lang}" id="${rid}-btn">Check readiness</button>`;
    html += `</div>`;

    // Readiness panel (collapsed until run; then expanded only if it has errors).
    html += `<div class="readiness">`;
    html += `<div class="rhead">`;
    html += `<h3>Publish readiness</h3>`;
    html += `<span class="pills" id="${rid}-pills"></span>`;
    html += `<span class="verdict" id="${rid}-verdict" style="display:none"></span>`;
    html += `<button class="caret" data-ring="caret-${lang}" id="${rid}-caret" style="display:none"></button>`;
    html += `</div>`;
    html += `<div class="rbody collapsed" id="${rid}-body"></div>`;
    html += `</div>`;

    html += `</div>`;
  });

  $('main').innerHTML = html;

  (d.languages || []).forEach((lang) => {
    const btn = $(`rd-${lang}-btn`);
    if (btn) btn.onclick = () => checkReadiness(d.slug, lang);
  });
  wireRing();
  restoreFocus();
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
