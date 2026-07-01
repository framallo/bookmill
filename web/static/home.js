// Home = the library. Alexandria-style cover grid over /api/books.
// Each tile shows the book's rendered front cover (falling back to a title
// block when no cover has been rendered yet), the title, and a language badge.
// Clicking a tile opens the existing per-book detail page (book.html?book=…).

const $ = (id) => document.getElementById(id);

let BOOKS = [];
const state = { q: '', sort: 'title', dir: 'asc' };

init();

async function init() {
  wireControls();
  let res;
  try {
    res = await fetch('/api/books').then((r) => r.json());
  } catch (e) {
    $('shelf').innerHTML = `<p class="empty">Failed to load books: ${esc(String(e))}</p>`;
    return;
  }
  $('repo').textContent = res.repoRoot || '';
  BOOKS = (res.books || []).map((b) => ({
    slug: b.slug,
    languages: b.languages || [],
    title: (b.titles && (b.titles.es || b.titles.en || Object.values(b.titles)[0])) || b.slug,
    protected: !!b.protected,
  }));
  render();
}

function wireControls() {
  $('q').addEventListener('input', (e) => { state.q = e.target.value; render(); });

  const sheet = $('sheet');
  $('filterBtn').addEventListener('click', (e) => {
    e.stopPropagation();
    const open = sheet.classList.toggle('open');
    $('filterBtn').classList.toggle('active', open);
  });
  document.addEventListener('click', (e) => {
    if (!sheet.contains(e.target) && e.target !== $('filterBtn')) {
      sheet.classList.remove('open');
      $('filterBtn').classList.remove('active');
    }
  });
  sheet.querySelectorAll('[data-sort]').forEach((b) =>
    b.addEventListener('click', () => { state.sort = b.dataset.sort; syncSheet(); render(); }));
  sheet.querySelectorAll('[data-dir]').forEach((b) =>
    b.addEventListener('click', () => { state.dir = b.dataset.dir; syncSheet(); render(); }));
  syncSheet();

  // Settings icon: no dedicated page yet — surface the repo path (useful in the
  // desktop app, where there is no address bar).
  $('settingsBtn').addEventListener('click', () => {
    alert(`bookmill\nRepo: ${$('repo').textContent || '(unknown)'}\nBooks: ${BOOKS.length}`);
  });

  wireShortcuts();
}

// Keyboard shortcuts: `/` focuses search, `Esc` clears/blurs it, `?` toggles the
// help overlay. Typing in the search box is never hijacked (only Esc acts there).
function wireShortcuts() {
  const q = $('q');
  const overlay = $('helpOverlay');
  const toggleHelp = () => overlay.classList.toggle('open');
  $('helpBtn').addEventListener('click', toggleHelp);
  overlay.addEventListener('click', () => overlay.classList.remove('open'));

  document.addEventListener('keydown', (e) => {
    const typing = e.target === q || /^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName) || e.target.isContentEditable;
    if (e.key === 'Escape') {
      if (overlay.classList.contains('open')) { overlay.classList.remove('open'); return; }
      if (e.target === q) { q.value = ''; state.q = ''; render(); q.blur(); }
      return;
    }
    if (typing) return;
    if (e.key === '/') { e.preventDefault(); q.focus(); q.select(); }
    else if (e.key === '?') { e.preventDefault(); toggleHelp(); }
  });
}

function syncSheet() {
  document.querySelectorAll('#sheet [data-sort]').forEach((b) =>
    b.classList.toggle('sel', b.dataset.sort === state.sort));
  document.querySelectorAll('#sheet [data-dir]').forEach((b) =>
    b.classList.toggle('sel', b.dataset.dir === state.dir));
}

function render() {
  const q = state.q.trim().toLowerCase();
  let list = BOOKS.filter(
    (b) => !q || b.title.toLowerCase().includes(q) || b.slug.toLowerCase().includes(q)
  );
  const key = state.sort === 'lang'
    ? (b) => (b.languages[0] || '') + b.title.toLowerCase()
    : (b) => b.title.toLowerCase();
  list.sort((a, b) => key(a) < key(b) ? -1 : key(a) > key(b) ? 1 : 0);
  if (state.dir === 'desc') list.reverse();

  $('count').textContent = `${list.length} book${list.length === 1 ? '' : 's'}`;
  const shelf = $('shelf');
  shelf.innerHTML = '';
  $('empty').style.display = list.length ? 'none' : '';

  list.forEach((b) => shelf.appendChild(tile(b)));
}

function tile(b) {
  const lang = b.languages[0] || '';
  const a = document.createElement('a');
  a.className = 'tile';
  a.href = `/book.html?book=${encodeURIComponent(b.slug)}`;

  const badges = b.languages.map((l) => `<span class="badge">${esc(l)}</span>`).join('') +
    (b.protected ? '<span class="badge prot spacer">locked</span>' : '');

  // Cover image: the rendered front cover for the first language. If the book
  // has no rendered cover yet, the <img> onerror swaps in a title block.
  const coverUrl = lang ? `/api/asset/${encodeURIComponent(b.slug)}/${encodeURIComponent(lang)}/rendered` : '';
  const fake = `<div class="fake">${esc(b.title)}</div>`;

  a.innerHTML =
    `<div class="cover">
       <div class="badges">${badges}</div>
       ${coverUrl
         ? `<img src="${coverUrl}" alt="" loading="lazy"
              onerror="this.remove();this.parentNode.insertAdjacentHTML('beforeend', this.getAttribute('data-fb'));"
              data-fb="${escAttr(fake)}" />`
         : fake}
     </div>
     <div class="ttl">${esc(b.title)}</div>
     <div class="slug">${esc(b.slug)}</div>`;
  return a;
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}
function escAttr(s) {
  return String(s).replace(/"/g, '&quot;');
}
