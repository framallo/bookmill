// Home = books list. Cards link to the per-book detail page (book.html?book=…).

const $ = (id) => document.getElementById(id);

init();

async function init() {
  let res;
  try {
    res = await fetch('/api/books').then((r) => r.json());
  } catch (e) {
    $('grid').innerHTML = `<p class="muted">Failed to load books: ${e}</p>`;
    return;
  }
  $('repo').textContent = res.repoRoot || '';
  const books = res.books || [];
  $('count').textContent = `${books.length} book${books.length === 1 ? '' : 's'}`;
  if (!books.length) { $('empty').style.display = ''; return; }

  const grid = $('grid');
  books.forEach((b) => {
    const a = document.createElement('a');
    a.className = 'card';
    a.href = `/book.html?book=${encodeURIComponent(b.slug)}`;
    const title = b.titles.es || b.titles.en || b.slug;
    const langs = (b.languages || []).map((l) => `<span class="tag">${l.toUpperCase()}</span>`).join('');
    const prot = b.protected ? '<span class="tag prot">protected</span>' : '';
    a.innerHTML = `<h2>${esc(title)}</h2><div class="slug">${esc(b.slug)}</div><div class="tags">${langs}${prot}</div>`;
    grid.appendChild(a);
  });
}

function esc(s) {
  return String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
}
