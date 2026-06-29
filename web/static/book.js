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
    html += `</div>`;
  });

  $('main').innerHTML = html;
}

function esc(s) {
  return String(s == null ? '' : s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
}
