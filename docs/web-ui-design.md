# bookmill web UI — cover editor + book previewer + publish checklist

> Research/design doc. A browser UI served by a Rust **axum** server embedded in
> the `bookmill` binary. **resvg is the authoritative renderer** (the same
> `cover_svg.rs` path used by the CLI); the browser canvas (**Konva.js**) is only
> a WYSIWYG editing surface — what ships is always what resvg renders from the
> saved TOML. Three views: (A) full cover editor, (B) Amazon-style two-page
> previewer with trim/bleed/safe guides, (C) the publish checklist (kdp-requirements
> §5).

---

## 0. Architecture at a glance

```
 bookmill serve  (axum, default :7777)
   ├── GET  /                          → SPA (Konva editor + previewer + checklist)
   ├── REST /api/*                     → config, assets, save, render, validate
   ├── static /assets/<book>/<lang>/*  → bg images, rendered covers, page PNGs
   └── invokes the SAME Rust modules the CLI uses:
        cover_svg::CoverRenderer  (front PNG, wrap PDF — authoritative)
        config (load/save bookmill.toml)   deep (validate --deep --json)
        build  (per-edition jobs)          poppler `pdftoppm` (page rasterization)
```

Design rule: **the editor never rasterizes the final artifact.** Konva is for
positioning/styling; on save we write fractions+styles into `[cover]` TOML and
re-render via `cover_svg.rs`. The Konva preview and the resvg output must agree
visually, so the editor seeds itself from the resvg SVG (see §A.2).

---

## A. Cover editor (Konva.js)

Modeled on the Konva Canvas Editor pattern (two layers, Transformer-based
select/drag/resize/rotate, textarea-overlay text editing, fractional persistence).

### A.1 Scene graph

```
Stage (container #editor, sized to the wrap or front aspect)
 ├── Layer "art"        → Konva.Image (bg.jpg / wrap_bg panorama), Konva.Rect (solid panels)
 ├── Layer "text"       → Konva.Text nodes (badge, title, subtitle, blurb, author, spine)
 ├── Layer "overlay"    → guides: trim / bleed / safe-margin / barcode keep-out (listening:false)
 └── Konva.Transformer  → on the text layer; nodes() = current selection
```

Konva API used (from research):
- `new Konva.Stage({container,width,height})`, `Layer`, `stage.add`.
- `Konva.Image.fromURL(url, cb)` / `new Konva.Image({image})` — bg + panorama;
  set `crossOrigin='anonymous'` so `toDataURL` (preview export) isn't tainted.
- `Konva.Text` with `text/fontFamily/fontSize/fill/align/lineHeight/letterSpacing`.
- Selection: `stage.on('click tap', …)` → `transformer.nodes([node])`; click empty
  → `nodes([])`. Multi-select with shift/ctrl; rubber-band via `Konva.Rect` +
  `Konva.Util.haveIntersection(box, node.getClientRect())`.
- `Konva.Transformer({ keepRatio, rotationSnaps:[0,90,180,270], boundBoxFunc,
  enabledAnchors })`. On `transformend`, **bake scale → size**: read
  `node.scaleX()`, reset to 1, set `width = width*scaleX` (so persisted fractions
  stay clean). Events: `dragmove`, `dragend`, `transformend`.
- In-place text edit: on `dblclick`, hide the node + transformer, overlay an
  absolutely-positioned `<textarea>` aligned via `node.absolutePosition()` +
  `stage.container().getBoundingClientRect()`, mirror `fontSize/fontFamily/fill/
  align/lineHeight/rotation`; commit on Enter / click-away → `node.text(value)`,
  `transformer.forceUpdate()`.

### A.2 Seeding the canvas from the authoritative renderer

To guarantee the editor matches resvg output:
1. `BOOKMILL_DUMP_SVG` already makes `cover_svg.rs` dump the front SVG. Add a
   `--emit-svg` server path that returns the front/wrap SVG **plus a layout
   manifest** (each text block's x/y/size/anchor it computed in `front_svg`/
   `wrap_svg`).
2. The SPA loads the bg image and creates Konva.Text nodes at those manifest
   positions → pixel-perfect starting point. Edits then diverge intentionally.

### A.3 Persistence — fractions into `[cover]` TOML

Konva works in absolute px; persist **resolution-independent fractions** so the
same layout renders at editor size and at 1600×2560 / wrap-DPI:

```jsonc
// normalized model (also expressible as TOML under [cover.<lang>.layout])
{
  "title":  { "xPct":0.5, "yPct":0.18, "wPct":0.80, "anchor":"middle",
              "fontPct":0.092, "fill":"#2B1B12", "rotation":0,
              "stroke":"6px #000", "shadow":"0 4px 18px rgba(0,0,0,.45)" },
  "subtitle": { ... }, "author": { ... }, "blurb": { ... },
  "bgFit": "cover", "bgPosRight": true
}
```
- Anchor X to stage width, **font size and stroke to stage height** (uniform
  scaling). `rotation` (deg) and ratios are already resolution-independent.
- Save = `POST /api/cover` → server merges into `bookmill.toml`'s
  `[cover.<lang>]` (and a new `[cover.<lang>.layout]` subtable for positions),
  then **re-renders via `cover_svg.rs`** → updates `cover/front-<lang>.png` +
  `wrap-<lang>-KDP.pdf` (respecting the `PROTECTED` list in `covers.rs` —
  submitted books render to a comparison path, never in place).
- We do **not** rely on `stage.toJSON()` as the source of truth (it omits images,
  filters, handlers); our normalized model is canonical and lives in TOML.

### A.4 Multiple versions + front-only vs wrap vs hardcover

- **Versions:** `[cover.<lang>.versions.<name>]` subtables; the active one is
  `[cover.<lang>].active = "v2"`. UI = a version dropdown + "duplicate"/"set
  active". Only the active version renders to the shipped filenames; others render
  to `output/<slug>/<lang>/<slug>-<lang>-cover-<version>.png` for comparison.
- **Mode switch** in the editor toolbar:
  - **Front only** → stage = 1600×2560 (eBook), edits `front` layout.
  - **Paperback wrap** → stage = `(2·trim_w + spine + 2·bleed) × (trim_h+2·bleed)`;
    shows back+spine+front panels and the barcode keep-out overlay.
  - **Hardcover wrap** → wider case-laminate canvas (0.51″ board wrap, calculator
    spine); separate `[cover.<lang>].hardcover` layout + a `wrap-hardcover` SVG
    template. Same back/spine/front editing, different panel geometry.
- Spine width comes from the live page count (`pdfinfo` on the built KDP PDF, as
  `covers.rs::resolve_pages` does); the editor calls `GET /api/spine?...` so the
  spine panel resizes when the interior reflows.

### A.5 Guides & snapping (overlay layer)

`Konva.Line` with `dash:[4,6]`, `listening:false` for: trim box, bleed box
(0.125″ out), safe-text box (0.125″ in), spine seams, and a `Konva.Rect` for the
**2″×1.2″ barcode keep-out** (bottom-right of back panel). Snapping: on `dragmove`
compute `node.getClientRect()` edges, snap to guide stops within ~6px (the Konva
"objects snapping" recipe), draw temporary guide lines, clear on `dragend`.

---

## B. Book previewer (Amazon-style two-page spread)

### B.1 What it shows

- A **two-page spread** (verso | recto) of the interior, plus cover spreads.
- Overlays (Konva on a transparent canvas atop each page image):
  **trim** (solid), **bleed** (red, outside), **safe margin/gutter** (blue,
  inside — gutter grows with page count per kdp-requirements §1.5), and a
  **cutoff/overlap shade** in the bleed band so the user sees what the trimmer
  removes. Toggle each overlay.
- Page navigation (prev/next spread, jump-to-page, thumbnails), and a
  **format/edition switch** so guides reflect that edition's trim+bleed+gutter.

### B.2 Page rasterization (reuse poppler — already a dependency in spirit)

The CLI already shells to `pdfinfo` (poppler) in `covers.rs`/`deep.rs`. Use its
sibling **`pdftoppm`** to rasterize interior pages on demand:

```
pdftoppm -png -r 150 -f <page> -l <page> <slug>-<lang>-kdp.pdf  out
```
- Cache PNGs under `output/<slug>/<lang>/preview/p<NN>@150.png`; serve via static
  route. Render lazily (only requested spread ± a few), at ~150 DPI for screen.
- The cover preview uses the **resvg** front PNG (eBook) and the wrap PDF
  rasterized the same way (`pdftoppm` on `wrap-<lang>-KDP.pdf`).
- Later (Typst engine) can emit page PNGs directly, dropping the poppler hop —
  same API, different backend.

### B.3 Guide math

Given edition `trim_w/h`, `bleed`, page count → gutter from the §1.5 table. At
render scale `s = pxPerInch/72`, draw boxes at:
`bleed_box = full page`, `trim_box = inset bleed`, `safe_box = inset
(trim margins + gutter on the binding side)`. The binding side alternates
verso/recto so the gutter overlay flips per page (matches `openright`/twoside).

---

## C. Publish checklist panel

Implements kdp-requirements §5: format tabs + listing tabs, ✓/!/✗ rows from
`validate --deep --json`, the **sanitized rendered description** (KDP HTML
whitelist + live char count incl. markup), and **copy-ready** title/subtitle/
keywords/BISAC/age/price/description blocks. A **Re-validate** button streams deep
checks as they finish (reusing `build::QueueEvent`-style progress over SSE).

---

## D. REST API

| Method | Path | Body / query | Returns |
|---|---|---|---|
| GET | `/api/books` | — | discovered books/langs/editions (`bookmill list --json`) |
| GET | `/api/book/:slug` | — | resolved config (title/subtitle/listing/`[cover]`/editions) |
| GET | `/api/cover/:slug/:lang` | `?version&mode=front\|wrap\|hardcover` | layout manifest + asset URLs + SVG |
| GET | `/api/assets/:slug/:lang/*` | — | static: bg.jpg, front PNG, wrap PDF, page PNGs |
| GET | `/api/spine/:slug/:lang` | `?edition` | spine width in (from page count) |
| POST | `/api/cover/:slug/:lang` | normalized layout JSON (+version) | writes `[cover.*]` TOML, re-renders via `cover_svg`, returns new asset URLs |
| GET | `/api/page/:slug/:lang/:n` | `?edition&dpi=150` | page PNG (lazily `pdftoppm`-rendered, cached) |
| GET | `/api/pages/:slug/:lang` | `?edition` | page count + cover/spread map |
| POST | `/api/build/:slug` | `{edition,lang,format}` | enqueue build job; SSE progress (reuses `build::run_queue`) |
| GET | `/api/validate/:slug` | `?deep=1&lang` | checklist JSON (`deep.rs` + new checks) |
| POST | `/api/listing/:slug/:lang` | listing fields | writes `[listing.<lang>]`, re-validates |
| GET | `/api/description/:slug/:lang` | — | sanitized description HTML + char count |

Server crates: `axum`, `tower-http` (static + CORS), `serde_json`, `tokio`.
Cover re-render is synchronous (resvg is fast); builds and deep-validate go
through the existing job queue with SSE.

---

## E. Data model summary

- **Source of truth = `bookmill.toml`.** The UI reads/writes it; nothing is stored
  only in the browser.
- **Cover layout** = normalized fractions/styles in `[cover.<lang>]`
  (+ `.layout`, `.versions.<name>`, `.hardcover`). resvg renders it; Konva edits it.
- **Listing** = `[listing.<lang>]` (existing struct in `config.rs`), extended with
  description-HTML validation.
- **Previewer state** is ephemeral (page index, overlay toggles); page images are
  a derived cache under `output/.../preview/`.

---

## F. Scope

**Minimal v1**
- `bookmill serve`; SPA shell.
- Cover editor: front-only mode, single version, seeded from resvg SVG manifest,
  drag/resize/rotate/restyle text over the bg image, save → `[cover.<lang>]` TOML
  → resvg re-render. Guides shown (read-only), basic snapping.
- Previewer: read-only two-page interior spread via `pdftoppm` + trim/bleed/safe
  overlays + page nav.
- Checklist: render `validate --deep --json` + sanitized description preview +
  copy-ready listing.

**Full**
- Wrap + hardcover modes; multiple versions with comparison renders; full-bleed
  panorama editing; live spine resize on reflow.
- Editor snapping polish, undo/redo, font/color pickers bound to the `[cover]`
  palette fields.
- Build/validate over SSE with live queue; cover JPEG/size/colorspace audits;
  image-DPI audit once a native PDF page model lands.
- Honor the `PROTECTED` submitted-book guard (comparison-only renders).

## Sources

- Konva Canvas Editor (pattern): https://konvajs.org/docs/sandbox/Canvas_Editor.html
- Transformer: https://konvajs.org/docs/select_and_transform/Basic_demo.html
- Serialize/deserialize: https://konvajs.org/docs/data_and_serialization/Serialize_a_Stage.html
- Stage → data URL: https://konvajs.org/docs/data_and_serialization/Stage_Data_URL.html
- Editable text overlay: https://konvajs.org/docs/sandbox/Editable_Text.html
- poppler `pdftoppm` (interior rasterization) — same toolset as `pdfinfo` already used.
