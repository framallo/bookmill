// Home = the library. Alexandria-style cover grid over /api/books.
// Each tile shows the book's rendered front cover (falling back to a title
// block when no cover has been rendered yet), the title, and a language badge.
// Clicking a tile opens the existing per-book detail page (book.html?book=…).

const $ = (id) => document.getElementById(id);

let BOOKS = [];
let TILES = [];        // current rendered <a.tile> nodes, in display order
let sel = -1;          // index of the keyboard-selected tile (−1 = none)
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
      else if (sel >= 0) { sel = -1; applySel(false); }
      return;
    }
    if (typing) return;
    if (e.key === '/') { e.preventDefault(); q.focus(); q.select(); }
    else if (e.key === '?') { e.preventDefault(); toggleHelp(); }
    else if (e.key === 'ArrowRight') { e.preventDefault(); moveSel(1); }
    else if (e.key === 'ArrowLeft') { e.preventDefault(); moveSel(-1); }
    else if (e.key === 'ArrowDown') { e.preventDefault(); moveSel(colsPerRow()); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); moveSel(-colsPerRow()); }
    else if (e.key === 'Home') { e.preventDefault(); setSel(0); }
    else if (e.key === 'End') { e.preventDefault(); setSel(TILES.length - 1); }
    else if (e.key === 'Enter') {
      if (sel >= 0 && TILES[sel]) { e.preventDefault(); TILES[sel].click(); }
    }
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

  // Refresh the keyboard-selection state against the new tile set. Keep the
  // current selection if it's still in range; otherwise drop it.
  TILES = [...shelf.children];
  if (sel >= TILES.length) sel = TILES.length - 1;
  applySel(false);
}

// --- keyboard selection (arrow-key navigation over the cover grid) ---

// Number of tiles per row, inferred from their vertical positions (the grid is
// responsive, so columns depend on the window width).
function colsPerRow() {
  if (TILES.length < 2) return TILES.length || 1;
  const top0 = TILES[0].offsetTop;
  let n = 1;
  while (n < TILES.length && TILES[n].offsetTop === top0) n++;
  return n;
}

// Highlight the selected tile (and optionally scroll it into view).
function applySel(scroll = true) {
  TILES.forEach((t, i) => t.classList.toggle('sel', i === sel));
  if (scroll && sel >= 0 && TILES[sel]) {
    TILES[sel].scrollIntoView({ block: 'nearest' });
  }
}

// Move the selection by `delta`, clamped to the grid. From no selection, the
// first move lands on the first (or last) tile.
function moveSel(delta) {
  if (!TILES.length) return;
  if (sel < 0) sel = delta > 0 ? 0 : TILES.length - 1;
  else sel = Math.max(0, Math.min(TILES.length - 1, sel + delta));
  applySel();
}

function setSel(i) {
  if (!TILES.length) return;
  sel = Math.max(0, Math.min(TILES.length - 1, i));
  applySel();
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
