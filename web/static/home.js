// Home = the library. Alexandria-style cover grid over /api/books.
// Each tile shows the book's rendered front cover (falling back to a title
// block when no cover has been rendered yet), the title, and a language badge.
// Clicking a tile opens the existing per-book detail page (book.html?book=…).

const $ = (id) => document.getElementById(id);

let BOOKS = [];
let TILES = [];        // current rendered <a.tile> nodes, in display order
let sel = -1;          // index of the keyboard-selected tile (−1 = none)
const state = { q: '', sort: 'title', dir: 'asc' };

let ACTIVE_REPO = '';   // absolute path of the active project (for recent highlight)

init();

async function init() {
  wireControls();
  wireProjects();
  wireCheckAll();
  await loadBooks();
  loadProjects();
}

// --- Check all books: library-wide publish-readiness sweep ------------------
// Moved here from the per-book page (it's a whole-library operation). Opens a
// modal, runs `validate --deep` for every book × language sequentially, and lists
// a verdict per target that links to that book/lang.
function wireCheckAll() {
  const overlay = $('caOverlay');
  const close = () => overlay.classList.remove('open');
  $('checkAllBtn').addEventListener('click', () => overlay.classList.add('open'));
  $('caClose').addEventListener('click', close);
  overlay.addEventListener('click', (e) => { if (e.target === overlay) close(); });
  $('caRun').addEventListener('click', runCheckAll);
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && overlay.classList.contains('open')) close();
  });
}

async function runCheckAll() {
  const btn = $('caRun'), rows = $('caRows'), status = $('caStatus');
  btn.disabled = true;
  const jobs = [];
  BOOKS.forEach((b) => (b.languages || []).forEach((l) => jobs.push({ slug: b.slug, lang: l })));
  rows.innerHTML = '';
  const rowEls = {};
  jobs.forEach((j) => {
    const row = document.createElement('div');
    row.className = 'carow';
    row.innerHTML = caLabel(j) + `<span class="pill"><span class="spin"></span></span>`;
    rows.appendChild(row);
    rowEls[`${j.slug}/${j.lang}`] = row;
  });
  let done = 0;
  for (const j of jobs) {
    const row = rowEls[`${j.slug}/${j.lang}`];
    try {
      const res = await fetch(`/api/warnings/${encodeURIComponent(j.slug)}/${encodeURIComponent(j.lang)}`);
      if (!res.ok) throw new Error(await res.text());
      const data = await res.json();
      let errs = 0, warns = 0;
      (data.issues || []).forEach((it) => { if (it.level === 'error') errs++; else if (it.level === 'warn') warns++; });
      let vcls = 'ready', v = '✅ ready';
      if (errs) { vcls = 'notready'; v = `❌ ${errs} error${errs === 1 ? '' : 's'}`; }
      else if (warns) { vcls = 'warn'; v = `⚠️ ${warns} warn`; }
      row.innerHTML = caLabel(j) +
        `<a class="verdict ${vcls}" href="/book.html?book=${encodeURIComponent(j.slug)}&lang=${encodeURIComponent(j.lang)}">${v}</a>`;
    } catch (e) {
      row.innerHTML = caLabel(j) + `<span class="err-msg">${esc((e && e.message) || e)}</span>`;
    }
    done++;
    status.innerHTML = done < jobs.length
      ? `<span class="spin"></span> checked ${done}/${jobs.length}…`
      : `Done — ${jobs.length} target${jobs.length === 1 ? '' : 's'} checked.`;
  }
  btn.disabled = false;
  btn.textContent = 'Re-run check';
}

function caLabel(j) {
  return `<span class="caslug">${esc(j.slug)}</span><span class="calang">${esc(j.lang.toUpperCase())}</span>`;
}

// Load (or reload) the book grid for the currently-active project. Called on
// startup and again after switching projects via Open Folder / Open Recent.
async function loadBooks() {
  let res;
  try {
    res = await fetch('/api/books').then((r) => r.json());
  } catch (e) {
    $('shelf').innerHTML = `<p class="empty">Failed to load books: ${esc(String(e))}</p>`;
    return;
  }
  ACTIVE_REPO = res.repoRoot || '';
  $('repo').textContent = ACTIVE_REPO;
  BOOKS = (res.books || []).map((b) => ({
    slug: b.slug,
    languages: b.languages || [],
    titles: b.titles || {},
    title: (b.titles && (b.titles.es || b.titles.en || Object.values(b.titles)[0])) || b.slug,
    protected: !!b.protected,
    status: b.status || 'draft',
  }));
  sel = -1;
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

// --- Projects: Open Folder / Open Recent (VS Code-style) --------------------
// A drop sheet under the folder icon holds a path field + Open button and the
// recent-folders list. Opening a folder swaps the server's active repo, then we
// reload the book grid. The browser can't show a native folder dialog, so this
// uses a plain absolute-path text field (kept simple, per the spec).
function wireProjects() {
  const sheet = $('projSheet');
  const btn = $('projBtn');
  btn.addEventListener('click', (e) => {
    e.stopPropagation();
    const open = sheet.classList.toggle('open');
    btn.classList.toggle('active', open);
    if (open) { loadProjects(); setTimeout(() => $('projPath').focus(), 0); }
  });
  document.addEventListener('click', (e) => {
    if (!sheet.contains(e.target) && e.target !== btn && !btn.contains(e.target)) {
      sheet.classList.remove('open');
      btn.classList.remove('active');
    }
  });
  $('projOpen').addEventListener('click', () => openProject($('projPath').value));
  $('projPath').addEventListener('keydown', (e) => { if (e.key === 'Enter') openProject($('projPath').value); });
}

async function loadProjects() {
  let data;
  try { data = await fetch('/api/projects').then((r) => r.json()); }
  catch (e) { return; }
  ACTIVE_REPO = data.active || ACTIVE_REPO;
  const host = $('projRecent');
  const recent = data.recent || [];
  if (!recent.length) { host.innerHTML = '<div class="projempty">No recent folders.</div>'; return; }
  host.innerHTML = '';
  recent.forEach((p) => {
    const el = document.createElement('button');
    el.className = 'projrecent-item' + (p === data.active ? ' active' : '');
    el.title = p;
    el.innerHTML = `<span class="pdot"></span><span class="ptxt">${esc(p)}</span>`;
    el.addEventListener('click', () => openProject(p));
    host.appendChild(el);
  });
}

// Switch the active project to `path`, then reload books + the recent list.
async function openProject(path) {
  path = (path || '').trim();
  const errEl = $('projErr');
  errEl.style.display = 'none';
  if (!path) { showProjErr('Enter an absolute folder path.'); return; }
  const btn = $('projOpen');
  btn.disabled = true;
  try {
    const res = await fetch('/api/open', {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ path }),
    });
    if (!res.ok) throw new Error(await res.text());
    const data = await res.json();
    ACTIVE_REPO = data.active || path;
    $('projPath').value = '';
    $('projSheet').classList.remove('open');
    $('projBtn').classList.remove('active');
    await loadBooks();
    loadProjects();
  } catch (e) {
    showProjErr((e && e.message) || String(e));
  } finally {
    btn.disabled = false;
  }
}

function showProjErr(msg) {
  const el = $('projErr');
  el.textContent = msg;
  el.style.display = '';
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

  // Flat tile grid: one cover+title tile per language, emitted book by book so a
  // book's language variants (e.g. its ES and EN tiles) stay adjacent in order.
  list.forEach((b) => {
    (b.languages.length ? b.languages : ['']).forEach((lang) => shelf.appendChild(tile(b, lang)));
  });

  // Keyboard selection spans every tile in DOM order.
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

// One language tile: the rendered front cover for that language + its localized
// title (with a language badge). The <img> onerror swaps in a title block when no
// cover is rendered yet. Book language variants are emitted adjacently by render().
function tile(b, lang) {
  const a = document.createElement('a');
  a.className = 'tile';
  const title = (b.titles && b.titles[lang]) || b.title;
  const coverUrl = lang
    ? `/api/asset/${encodeURIComponent(b.slug)}/${encodeURIComponent(lang)}/rendered`
    : '';
  a.href = lang
    ? `/book.html?book=${encodeURIComponent(b.slug)}&lang=${encodeURIComponent(lang)}`
    : `/book.html?book=${encodeURIComponent(b.slug)}`;

  // Only surface a meaningful status (live / in-review / blocked); draft is the
  // silent default, so we don't stamp it on every cover.
  const stLabels = { live: 'LIVE', 'in-review': 'REVIEW', blocked: 'BLOCKED' };
  const st = stLabels[b.status] ? `<span class="badge st-${b.status} spacer">${stLabels[b.status]}</span>` : '';
  const badges = (lang ? `<span class="badge">${esc(lang)}</span>` : '') +
    (b.protected ? '<span class="badge prot spacer">locked</span>' : '') + st;
  const fake = `<div class="fake">${esc(title)}</div>`;
  const img = coverUrl
    ? `<img src="${coverUrl}" alt="" loading="lazy"
         onerror="var p=this.parentNode;this.remove();if(p)p.insertAdjacentHTML('beforeend', this.getAttribute('data-fb'));"
         data-fb="${escAttr(fake)}" />`
    : fake;

  a.innerHTML =
    `<div class="cover"><div class="badges">${badges}</div>${img}</div>
     <div class="ttl">${esc(title)}</div>`;
  return a;
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}
function escAttr(s) {
  return String(s).replace(/"/g, '&quot;');
}
