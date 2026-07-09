// Book detail (v2). A compact hub: the two language variants sit side by side as
// cards — cover + title + primary actions (Edit cover / Preview) above the fold —
// with Build & outputs and Publish readiness tucked behind disclosures so the page
// no longer scrolls forever. ←/→ move to the prev/next book. The library-wide
// "check all books" sweep now lives on the home page, not here.

const $ = (id) => document.getElementById(id);
const params = new URLSearchParams(location.search);
const slug = params.get('book');

let ALLBOOKS = [];   // ordered [{slug, languages}] from /api/books (for prev/next)
let DATA = null;     // /api/book/{slug} response

// One "generate/build" glyph reused by every Generate control (bolt = build it).
const GEN_ICON = '<svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"/></svg>';

// Verdict glyphs as inline SVG (never emoji — they inherit the chip's color via
// currentColor and stay crisp at any zoom). check / alert-triangle / x-circle.
const VICON = {
  ready: '<svg class="vicon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>',
  warn: '<svg class="vicon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/><line x1="12" y1="9" x2="12" y2="13"/><line x1="12" y1="17" x2="12.01" y2="17"/></svg>',
  notready: '<svg class="vicon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="15" y1="9" x2="9" y2="15"/><line x1="9" y1="9" x2="15" y2="15"/></svg>',
};

// A publishing-status pill from the book's [status] ribbon (live/in-review/…).
function statusBadge(ribbon) {
  const map = {
    live: ['ready', 'LIVE'], 'in-review': ['warn', 'IN REVIEW'],
    blocked: ['notready', 'BLOCKED'], draft: ['', 'DRAFT'],
  };
  const [cls, label] = map[ribbon] || map.draft;
  return `<span class="verdict ${cls}" style="font-size:11px;padding:2px 9px">${label}</span>`;
}

init();

async function init() {
  if (!slug) {
    $('main').innerHTML = '<p class="muted">No <code>?book=&lt;slug&gt;</code> given. <a class="crumb" href="/">← all books</a></p>';
    return;
  }
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

  document.addEventListener('keydown', (e) => {
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName) || e.target.isContentEditable;
    if (typing) return;
    if (e.key === 'ArrowLeft') { e.preventDefault(); gotoBook(-1); }
    else if (e.key === 'ArrowRight') { e.preventDefault(); gotoBook(1); }
  });
}

// --- page render ------------------------------------------------------------

function render(d) {
  const langs = d.languages || [];
  const firstTitle = (d.langs[langs[0]] && d.langs[langs[0]].title) || d.slug;
  const firstSub = (d.langs[langs[0]] && d.langs[langs[0]].subtitle) || '';
  $('bookTitle').textContent = firstTitle;

  const firstLang = langs[0] || 'es';
  const q0 = `book=${encodeURIComponent(d.slug)}&lang=${encodeURIComponent(firstLang)}`;

  // ---- hero: editorial title + kicker + pipeline + actions ----
  const editions = (d.editions || []).map((e) => `<span class="tag">${esc(e)}</span>`).join('');
  let hero = '<div class="pagehero"><div class="inner">';
  hero += `<div class="kicker">`;
  if (d.author) hero += `<span>by <b>${esc(d.author)}</b></span>`;
  if (d.series) hero += `<span>·</span><span>${esc(d.series)}</span>`;
  hero += `<span>·</span><span class="slug">${esc(d.slug)}</span>`;
  if (d.protected) hero += `<span class="tag prot">protected</span>`;
  hero += `</div>`;
  hero += `<h1 class="title">${esc(firstTitle)}</h1>`;
  if (firstSub) hero += `<p class="subtitle">${esc(firstSub)}</p>`;
  hero += pipelineStrip(d);
  hero += `<div class="hero-actions">`;
  hero += `<a class="btn primary" href="/cover.html?${q0}">Edit cover</a>`;
  hero += `<a class="btn" href="/preview.html?${q0}">Preview interior</a>`;
  hero += `<span style="flex:1"></span>`;
  hero += `<span class="kicker" style="margin:0">editions: ${editions || '<span class="muted">—</span>'}</span>`;
  hero += `</div>`;
  hero += `</div></div>`;
  $('hero').innerHTML = hero;

  // ---- edition cards ----
  let html = `<div class="langgrid">`;
  langs.forEach((lang) => { html += langCard(d, lang); });
  html += `</div>`;
  $('main').innerHTML = html;

  // Wire per-language controls + lazy-load the outputs list.
  langs.forEach((lang) => {
    const ga = document.querySelector(`[data-genall="${lang}"]`);
    if (ga) ga.onclick = (e) => { e.preventDefault(); e.stopPropagation(); generateAll(d.slug, lang); };
    const rb = $(`rd-${lang}-btn`);
    if (rb) rb.onclick = () => checkReadiness(d.slug, lang);
    loadOutputs(d.slug, lang);
  });
}

// The publishing pipeline as an at-a-glance strip: each step aggregates both
// languages so you see the book's overall state without scrolling.
// Interior (KDP PDF built) → Cover (front rendered) → Listing (KDP fields) →
// Published (the [status] ribbon).
function pipelineStrip(d) {
  const langs = d.languages || [];
  const L = (l) => d.langs[l] || {};

  // roll up a boolean predicate across languages into a step state
  const roll = (pred) => {
    const vals = langs.map(pred);
    const done = vals.filter(Boolean).length;
    if (!langs.length) return 'idle';
    if (done === langs.length) return 'done';
    if (done === 0) return 'idle';
    return 'warn';
  };

  const interior = roll((l) => L(l).kdpPdfExists);
  const cover = roll((l) => L(l).coverExists);
  const listing = roll((l) => listingComplete(L(l).listing, L(l).title));

  const pagesMeta = langs.map((l) => L(l).pages).filter((p) => p != null);
  const interiorMeta = pagesMeta.length ? pagesMeta.join(' · ') + ' pp' : 'not built';
  const coverMeta = cover === 'done' ? 'rendered' : cover === 'warn' ? 'partial' : 'not rendered';
  const listingMeta = listing === 'done' ? 'complete' : listing === 'warn' ? 'partial' : 'incomplete';

  const pub = d.status || 'draft';
  const pubMap = { live: ['done', 'Live on KDP'], 'in-review': ['warn', 'In review'],
    blocked: ['err', 'Blocked'], draft: ['idle', 'Draft'] };
  const [pubState, pubMeta] = pubMap[pub] || pubMap.draft;

  const step = (n, label, state, meta) =>
    `<div class="pstep ${state === 'idle' ? '' : state}">` +
    `<span class="pnum"><span class="pdot"></span>${esc(n)}</span>` +
    `<span class="plabel">${esc(label)}</span>` +
    `<span class="pmeta">${esc(meta)}</span></div>`;

  return `<div class="pipeline">` +
    step('1', 'Interior', interior, interiorMeta) +
    step('2', 'Cover', cover, coverMeta) +
    step('3', 'Listing', listing, listingMeta) +
    step('4', 'Published', pubState, pubMeta) +
    `</div>`;
}

// A KDP listing counts as complete when it has a title, exactly 7 keywords,
// 2–3 BISAC codes, a reading age, and a blurb within the 4000-char limit.
function listingComplete(li, title) {
  if (!li || !title) return false;
  const kw = (li.keywords || []).length;
  const bc = (li.bisac || []).length;
  const blurbOk = li.blurbChars != null && li.blurbChars <= 4000;
  return kw === 7 && bc >= 2 && bc <= 3 && !!li.readingAge && blurbOk;
}

// One language column: cover + title + primary actions, then two disclosures
// (Build & outputs, Publish readiness) so depth is one click away, not a scroll.
function langCard(d, lang) {
  const L = d.langs[lang] || {};
  const rid = `rd-${lang}`;
  const q = `book=${encodeURIComponent(d.slug)}&lang=${encodeURIComponent(lang)}`;
  const coverUrl = `/api/asset/${encodeURIComponent(d.slug)}/${encodeURIComponent(lang)}/rendered`;
  const wrapUrl = `/api/output/${encodeURIComponent(d.slug)}/${encodeURIComponent(lang)}/wrap-cover`;

  let h = `<div class="card langcard">`;
  h += `<div class="lgrid">`;
  h += `<div class="lcover">`;
  h += `<img src="${coverUrl}" alt="cover" onerror="this.outerHTML='<div class=&quot;fakecover&quot;>${escAttr(L.title || d.slug)}</div>'" />`;
  h += `<a class="wraplink" href="${wrapUrl}" target="_blank" rel="noopener">wrap PDF ↗</a>`;
  h += `</div>`;
  h += `<div class="lmain">`;
  h += `<div class="lh"><span class="code">${lang.toUpperCase()}</span><span class="title">${esc(L.title || '(untitled)')}</span></div>`;
  if (L.subtitle) h += `<div class="lsub">${esc(L.subtitle)}</div>`;
  h += statStrip(L);
  h += `</div>`; // .lmain
  h += `</div>`; // .lgrid

  // Build & outputs disclosure — "Generate all" is an icon button on the summary row.
  h += `<details class="disc">`;
  h += `<summary class="discsum"><span>Build &amp; outputs</span><button class="iconbtn genall" data-genall="${lang}" title="Generate all">${GEN_ICON}</button></summary>`;
  h += `<div id="out-${lang}"><p class="muted"><span class="spin"></span> loading…</p></div>`;
  h += `<div class="outlog" id="outlog-${lang}"></div>`;
  h += `</details>`;

  // Publish readiness disclosure
  h += `<details class="disc">`;
  h += `<summary>Publish readiness</summary>`;
  h += `<div class="rhead">`;
  h += `<button class="btn sm" id="${rid}-btn">Check readiness</button>`;
  h += `<span class="pills" id="${rid}-pills"></span>`;
  h += `<span class="verdict" id="${rid}-verdict" style="display:none"></span>`;
  h += `</div>`;
  h += `<div id="${rid}-body"><p class="muted">Runs epubcheck + geometry + DPI + cover + house rules.</p></div>`;
  h += `</details>`;

  // KDP listing details (headline facts already live in the stat strip above)
  h += `<details class="disc"><summary>KDP listing details</summary>${kdpBlock(L)}</details>`;

  h += `</div>`; // .card
  return h;
}

// Always-visible headline stats for an edition: the four numbers you check most.
function statStrip(L) {
  const li = L.listing || null;
  const kw = li ? (li.keywords || []).length : 0;
  const val = (k, v, cls) => `<div class="stat"><span class="k">${esc(k)}</span>` +
    `<span class="v"${cls ? ` style="color:var(--${cls})"` : ''}>${v}</span></div>`;
  let h = `<div class="statstrip">`;
  h += val('Pages', L.pages != null ? L.pages : '—', L.pages != null ? null : 'muted');
  h += val('KDP PDF', L.kdpPdfExists ? 'Built' : 'Not built', L.kdpPdfExists ? 'ok' : 'muted');
  h += val('Keywords', li ? `${kw}/7` : '—', li ? (kw === 7 ? 'ok' : 'warn') : 'muted');
  const blurb = li && li.blurbChars != null ? li.blurbChars : null;
  h += val('Blurb', blurb != null ? `${blurb}` : '—', blurb != null ? (blurb <= 4000 ? 'ok' : 'err') : 'muted');
  h += `</div>`;
  return h;
}

// --- KDP readiness block ----------------------------------------------------
function kdpBlock(L) {
  const li = L.listing || null;
  const fact = (label, value, flag) => {
    const f = flag ? `<span class="flag ${flag.cls}">${esc(flag.text)}</span>` : '';
    return `<div class="kfact">${esc(label)}: <b>${value}</b>${f}</div>`;
  };
  let h = `<div class="kdp">`;
  h += fact('Title', esc(L.title || '—'), L.title ? null : { cls: 'err', text: 'missing' });
  h += fact('Pages', L.pages != null ? L.pages : '—', L.pages != null ? null : { cls: 'warn', text: 'not built' });
  h += fact('KDP PDF', L.kdpPdfExists ? 'built' : 'not built',
    L.kdpPdfExists ? { cls: 'ok', text: 'ok' } : { cls: 'warn', text: 'build' });
  if (li) {
    const kc = (li.keywords || []).length;
    h += fact('Keywords', kc, kc === 7 ? { cls: 'ok', text: '7/7' } : { cls: kc > 7 ? 'err' : 'warn', text: `${kc}/7` });
    const bc = (li.bisac || []).length;
    h += fact('BISAC', (li.bisac || []).join(', ') || '—',
      bc >= 2 && bc <= 3 ? { cls: 'ok', text: `${bc}` } : { cls: 'warn', text: `${bc} (want 2–3)` });
    h += fact('Reading age', esc(li.readingAge || '—'), li.readingAge ? null : { cls: 'warn', text: 'unset' });
    if (li.blurbChars != null) {
      h += fact('Blurb', `${li.blurbChars} chars`, li.blurbChars > 4000 ? { cls: 'err', text: 'over 4000' } : { cls: 'ok', text: '≤4000' });
    } else {
      h += fact('Blurb', '—', { cls: 'err', text: 'missing' });
    }
  } else {
    h += `<div class="kfact">Listing: <b>—</b><span class="flag err">no [listing]</span></div>`;
  }
  h += `</div>`;
  return h;
}

// --- Outputs (built artifacts: open / generate) -----------------------------

async function loadOutputs(bslug, lang) {
  const host = $(`out-${lang}`);
  if (!host) return;
  let data;
  try {
    const res = await fetch(`/api/outputs/${encodeURIComponent(bslug)}/${encodeURIComponent(lang)}`);
    if (!res.ok) throw new Error(await res.text());
    data = await res.json();
  } catch (e) {
    host.innerHTML = `<p class="err-msg">${esc((e && e.message) || e)}</p>`;
    return;
  }
  host.innerHTML = '';
  (data.outputs || []).forEach((o) => {
    const row = document.createElement('div');
    row.className = 'outrow';
    row.id = `outrow-${lang}-${o.kind}`;
    const when = o.exists && o.mtime ? new Date(o.mtime).toLocaleString() : '';
    const openUrl = `/api/output/${encodeURIComponent(bslug)}/${encodeURIComponent(lang)}/${encodeURIComponent(o.kind)}`;
    row.innerHTML =
      `<span class="olabel">${esc(o.label)}</span>` +
      `<span class="ostatus ${o.exists ? 'built' : ''}">${o.exists ? 'built · ' + esc(when) : 'not built'}</span>` +
      `<a class="obtn" ${o.exists ? `href="${openUrl}" target="_blank" rel="noopener"` : 'aria-disabled="true"'} data-open>Open</a>` +
      `<button class="obtn gen" data-gen="${esc(o.kind)}" title="Generate">${GEN_ICON}<span>Generate</span></button>`;
    host.appendChild(row);
    row.querySelector('[data-gen]').onclick = () => generateOutput(bslug, lang, o.kind, o.label);
    if (!o.exists) {
      const open = row.querySelector('[data-open]');
      open.style.pointerEvents = 'none';
      open.style.opacity = '.4';
    }
  });
}

async function generateOutput(bslug, lang, kind, label) {
  const row = $(`outrow-${lang}-${kind}`);
  const log = $(`outlog-${lang}`);
  const genBtn = row ? row.querySelector('[data-gen]') : null;
  if (genBtn) { genBtn.disabled = true; genBtn.innerHTML = '<span class="spin"></span>'; }
  const status = row ? row.querySelector('.ostatus') : null;
  if (status) { status.textContent = 'building…'; status.classList.remove('built'); }
  log.style.display = '';
  log.textContent = `Building ${label} (${kind})…\n`;
  try {
    const res = await fetch(`/api/build/${encodeURIComponent(bslug)}/${encodeURIComponent(lang)}`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ kind }),
    });
    if (!res.ok) throw new Error(await res.text());
    const data = await res.json();
    log.textContent = (data.ok ? '✓ ' : '✗ ') + `${label}\n\n` + (data.log || '');
  } catch (e) {
    log.textContent = `✗ ${label}\n\n` + ((e && e.message) || e);
  } finally {
    if (genBtn) { genBtn.disabled = false; genBtn.innerHTML = GEN_ICON + '<span>Generate</span>'; }
    await loadOutputs(bslug, lang);
  }
}

async function generateAll(bslug, lang) {
  const btn = document.querySelector(`[data-genall="${lang}"]`);
  const log = $(`outlog-${lang}`);
  if (btn) { btn.disabled = true; btn.innerHTML = '<span class="spin"></span>'; }
  log.style.display = '';
  log.textContent = `Building everything for ${bslug} · ${lang.toUpperCase()}…\n`;
  try {
    const res = await fetch(`/api/build/${encodeURIComponent(bslug)}/${encodeURIComponent(lang)}`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ kind: 'all' }),
    });
    if (!res.ok) throw new Error(await res.text());
    const data = await res.json();
    log.textContent = (data.ok ? '✓ ' : '✗ ') + `all outputs\n\n` + (data.log || '');
  } catch (e) {
    log.textContent = '✗ generate all\n\n' + ((e && e.message) || e);
  } finally {
    if (btn) { btn.disabled = false; btn.innerHTML = GEN_ICON; }
    await loadOutputs(bslug, lang);
  }
}

function escAttr(s) {
  return String(s == null ? '' : s).replace(/[&<>"']/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
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
  body.innerHTML = `<p class="muted"><span class="spin"></span> running checks… this can take a minute</p>`;
  try {
    const res = await fetch(`/api/warnings/${encodeURIComponent(bslug)}/${encodeURIComponent(lang)}`);
    if (!res.ok) throw new Error(await res.text());
    const data = await res.json();
    renderReadiness(bslug, lang, data);
  } catch (e) {
    body.innerHTML = `<p class="err-msg">${esc((e && e.message) || e)}</p>`;
  } finally {
    if (btn) { btn.disabled = false; btn.textContent = 'Re-check'; }
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

  let vcls, vtext;
  if (errs > 0) { vcls = 'notready'; vtext = 'Not ready'; }
  else if (warns > 0) { vcls = 'warn'; vtext = 'Ready with warnings'; }
  else { vcls = 'ready'; vtext = 'Ready'; }
  const vd = $(`${rid}-verdict`);
  vd.className = `verdict ${vcls}`;
  vd.innerHTML = `${VICON[vcls]} ${esc(vtext)}`;
  vd.style.display = '';

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
  $(`${rid}-body`).innerHTML = bodyHtml;
}

function esc(s) {
  return String(s == null ? '' : s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
}
