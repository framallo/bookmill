// Previewer stub: render the authoritative cover image with trim/bleed/safe
// guide rectangles. Full interior two-page spread previewer is a TODO.

const $ = (id) => document.getElementById(id);
const TRIM_W = 6.0, TRIM_H = 9.0, BLEED = 0.125, SAFE = 0.125; // inches (KDP 6x9)
let stage;

init();

async function init() {
  const res = await fetch('/api/books').then(r => r.json());
  const sel = $('bookSel');
  res.books.forEach(b => {
    const o = document.createElement('option');
    o.value = b.slug;
    o.textContent = b.titles.es || b.titles.en || b.slug;
    o.dataset.langs = JSON.stringify(b.languages);
    sel.appendChild(o);
  });
  sel.onchange = onBook;
  $('langSel').onchange = render;
  if (res.books.length) onBook();
}

function onBook() {
  const langs = JSON.parse($('bookSel').selectedOptions[0].dataset.langs || '["es","en"]');
  const ls = $('langSel'); ls.innerHTML = '';
  langs.forEach(l => { const o = document.createElement('option'); o.value = l; o.textContent = l.toUpperCase(); ls.appendChild(o); });
  render();
}

async function render() {
  const slug = $('bookSel').value, lang = $('langSel').value;
  const data = await fetch(`/api/cover/${slug}/${lang}`).then(r => r.json());

  // Display the rendered cover (eBook front, no bleed) — for v1 we overlay the
  // 6x9 trim/bleed/safe relationships at a representative scale on the front.
  const url = data.rendered_url || data.bg_url;
  const dispH = 760;
  // front cover full = trim + bleed all around (for the paperback front panel)
  const fullW = TRIM_W + 2 * BLEED, fullH = TRIM_H + 2 * BLEED;
  const ppi = dispH / fullH;
  const W = fullW * ppi, Hh = fullH * ppi;

  if (stage) stage.destroy();
  stage = new Konva.Stage({ container: 'stage', width: W, height: Hh });
  const art = new Konva.Layer(), guides = new Konva.Layer();
  stage.add(art, guides);

  Konva.Image.fromURL(url + '?t=' + Date.now(), (img) => {
    const iw = img.width(), ih = img.height();
    const s = Math.max(W / iw, Hh / ih);
    img.setAttrs({ width: iw * s, height: ih * s, x: (W - iw * s) / 2, y: (Hh - ih * s) / 2 });
    art.add(img); art.draw();
  }, () => {});

  const b = BLEED * ppi, sf = (BLEED + SAFE) * ppi;
  // bleed = full page edge (red)
  guides.add(new Konva.Rect({ x: 0, y: 0, width: W, height: Hh, stroke: '#ff5a5a', strokeWidth: 2 }));
  // trim = inset by bleed (white solid)
  guides.add(new Konva.Rect({ x: b, y: b, width: W - 2 * b, height: Hh - 2 * b, stroke: '#ffffff', strokeWidth: 2 }));
  // safe = inset by bleed+safe (blue dashed)
  guides.add(new Konva.Rect({ x: sf, y: sf, width: W - 2 * sf, height: Hh - 2 * sf, stroke: '#56b6ff', strokeWidth: 2, dash: [10, 8] }));
  guides.draw();
}
