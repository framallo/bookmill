// Book detail = attributes + per-language actions (Edit cover / Preview book).

const $ = (id) => document.getElementById(id);
const slug = new URLSearchParams(location.search).get('book');

init();

async function init() {
  if (!slug) { $('main').innerHTML = '<p class="muted">No ?book=<slug> given. <a class="crumb" href="/">← all books</a></p>'; return; }
  let d;
  try {
    const res = await fetch(`/api/book/${encodeURIComponent(slug)}`);
    if (!res.ok) throw new Error(await res.text());
    d = await res.json();
  } catch (e) {
    $('main').innerHTML = `<p class="muted">Failed to load <code>${esc(slug)}</code>: ${esc((e && e.message) || e)}</p>`;
    return;
  }
  render(d);
}

function render(d) {
  const firstTitle =
    (d.langs[d.languages[0]] && d.langs[d.languages[0]].title) || d.slug;
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

  html += `<h2>Languages</h2>`;
  (d.languages || []).forEach((lang) => {
    const L = d.langs[lang] || {};
    const li = L.listing;
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
    const q = `book=${encodeURIComponent(d.slug)}&lang=${encodeURIComponent(lang)}`;
    html += `<div class="actions">`;
    html += `<a class="btn primary" href="/cover.html?${q}">Edit cover</a>`;
    html += `<a class="btn" href="/preview.html?${q}">Preview book</a>`;
    html += `</div>`;

    // Publish-readiness panel (lazy: deep validate runs only on click).
    const rid = `rd-${lang}`;
    html += `<div class="readiness">`;
    html += `<div class="rhead">`;
    html += `<h3>Publish readiness</h3>`;
    html += `<button class="btn" id="${rid}-btn">Check readiness</button>`;
    html += `<span class="pills" id="${rid}-pills"></span>`;
    html += `</div>`;
    html += `<div id="${rid}-body"></div>`;
    html += `</div>`;

    html += `</div>`;
  });

  $('main').innerHTML = html;

  (d.languages || []).forEach((lang) => {
    const rid = `rd-${lang}`;
    const btn = $(`${rid}-btn`);
    if (btn) btn.onclick = () => checkReadiness(d.slug, lang);
  });
}

// --- Publish readiness -----------------------------------------------------

// Map an issue message to a group (kind), parsed from its prefix. Order here is
// the display order (errors-worthy / structural kinds first).
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

async function checkReadiness(slug, lang) {
  const rid = `rd-${lang}`;
  const btn = $(`${rid}-btn`);
  const body = $(`${rid}-body`);
  const pills = $(`${rid}-pills`);
  if (btn) btn.disabled = true;
  pills.innerHTML = '';
  body.innerHTML = `<p class="muted"><span class="spin"></span> running checks… this can take a minute</p>`;
  try {
    const res = await fetch(`/api/warnings/${encodeURIComponent(slug)}/${encodeURIComponent(lang)}`);
    if (!res.ok) throw new Error(await res.text());
    const data = await res.json();
    renderReadiness(slug, lang, data);
  } catch (e) {
    pills.innerHTML = '';
    body.innerHTML = `<p class="err-msg">${esc((e && e.message) || e)}</p>`;
  } finally {
    if (btn) { btn.disabled = false; btn.textContent = 'Re-check'; }
  }
}

function renderReadiness(slug, lang, data) {
  const rid = `rd-${lang}`;
  // `summary` from validate --json counts the WHOLE book (both languages); the
  // issues are filtered to this language. Derive the per-language verdict from
  // the filtered issues so counts match what's shown below.
  const issues = (data.issues || []).slice();
  let errs = 0, warns = 0, oks = 0;
  issues.forEach((it) => {
    if (it.level === 'error') errs++;
    else if (it.level === 'warn') warns++;
    else oks++;
  });

  // counts as pills
  $(`${rid}-pills`).innerHTML =
    `<span class="pill ok">${oks} ok</span>` +
    `<span class="pill warn">${warns} warn</span>` +
    `<span class="pill err">${errs} error</span>`;

  // overall verdict
  let v, vcls, vtext;
  if (errs > 0) { v = '❌'; vcls = 'notready'; vtext = 'Not ready'; }
  else if (warns > 0) { v = '⚠️'; vcls = 'warn'; vtext = 'Ready with warnings'; }
  else { v = '✅'; vcls = 'ready'; vtext = 'Ready'; }

  const tally = `${errs} error${errs === 1 ? '' : 's'}, ${warns} warning${warns === 1 ? '' : 's'}`;
  let html = `<div class="rhead" style="margin-top:10px">`;
  html += `<span class="verdict ${vcls}">${v} ${esc(vtext)}</span>`;
  html += `<span class="muted">${esc(tally)}</span>`;
  html += `</div>`;

  // group issues by kind, errors/warns sorted first within each group
  const groups = new Map();
  KINDS.forEach((k) => groups.set(k.key, { label: k.label, items: [] }));
  issues.forEach((it) => {
    const k = kindOf(it.message);
    if (!groups.has(k.key)) groups.set(k.key, { label: k.label, items: [] });
    groups.get(k.key).items.push(it);
  });

  let any = false;
  for (const [, grp] of groups) {
    if (!grp.items.length) continue;
    any = true;
    grp.items.sort((a, b) => rank(b.level) - rank(a.level));
    const ge = grp.items.filter((i) => i.level === 'error').length;
    const gw = grp.items.filter((i) => i.level === 'warn').length;
    let badge = '';
    if (ge) badge = `${ge} error${ge === 1 ? '' : 's'}`;
    else if (gw) badge = `${gw} warning${gw === 1 ? '' : 's'}`;
    else badge = 'ok';
    html += `<div class="igroup">`;
    html += `<div class="gh">${esc(grp.label)} <span class="gcount">${esc(badge)}</span></div>`;
    grp.items.forEach((it) => {
      html += `<div class="issue ${esc(it.level)}">`;
      html += `<span class="lv">${esc(it.level)}</span>`;
      html += `<span class="imsg">${esc(it.message)}</span>`;
      if (it.page) {
        const pq = `book=${encodeURIComponent(slug)}&lang=${encodeURIComponent(lang)}&page=${it.page}`;
        html += `<a class="pgbtn" href="/preview.html?${pq}">page ${it.page} →</a>`;
      }
      html += `</div>`;
    });
    html += `</div>`;
  }
  if (!any) html += `<p class="muted">No checks reported.</p>`;

  $(`${rid}-body`).innerHTML = html;
}

function esc(s) {
  return String(s == null ? '' : s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
}
