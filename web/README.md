# bookmill-web — cover editor + previewer (v1)

A **standalone** web UI for bookmill: a Rust [axum] server that serves a small
vanilla-JS + [Konva.js] frontend (Konva via CDN — no build tooling) and a REST
API over a books repo. It lets you load a book's cover, drag/resize/restyle the
**title / subtitle / author** over the background image, save the layout back
into the book's `bookmill.toml`, and trigger an **authoritative re-render** by
shelling out to `bookmill covers`.

This crate is intentionally **separate from the main bookmill crate** (its own
`Cargo.toml`, its own `target/`). It does not modify the bookmill binary or its
sources. A future `bookmill start` will fold this into the main binary — see
[TODOs](#todos).

## Run

```bash
cd web
cargo run                      # serves http://127.0.0.1:7777
# or pick a port / repo:
cargo run -- --port 8080 --repo /path/to/books-repo
```

Then open:

- **Cover editor** — http://127.0.0.1:7777/
- **Previewer (stub)** — http://127.0.0.1:7777/preview.html

Defaults:

| flag / env | default | meaning |
|---|---|---|
| `--repo` / `BOOKMILL_REPO` | `/Users/framallo/work/argentina_animal_libertaria` | the books repo to edit |
| `--port` / `BOOKMILL_WEB_PORT` | `7777` | listen port (localhost only) |
| `--pages` / `BOOKMILL_WEB_PAGES` | `120` | spine page count used **only** when a book's print interior PDF is missing (resvg needs one up front; the eBook front PNG is unaffected) |
| `BOOKMILL_BIN` | auto | path to the `bookmill` binary (else `../target/{release,debug}/bookmill`, else `bookmill` on `PATH`) |

The re-render shells out to `bookmill covers <slug> --lang <lang>`, so build the
main binary first:

```bash
cd ..        # bookmill crate root
cargo build --release
```

## What works (v1)

- **Book/lang picker** from `GET /api/books` (reads every `libros/<slug>/bookmill.toml`).
- **Cover editor** (front-only, single version):
  - Layer 1 = background image (`cover/bg.jpg`, cover-fit), Layer 2 = draggable
    Konva text for title/subtitle/author, seeded from the book's `[cover]` /
    `[title]` / `[subtitle]` config (positions as canvas fractions).
  - Drag to move, corner handles to resize (font + box scale together),
    sidebar to edit text, fill color, and font size; background-color picker.
  - Read-only **trim / safe-margin / center guides** overlay (toggle).
  - **Save & re-render** writes the layout to the book `bookmill.toml` and calls
    `bookmill covers`; the authoritative resvg output is shown in the sidebar.
- **Previewer stub** — shows the rendered cover with KDP **trim / bleed / safe**
  guide rectangles (6×9) via Konva.

### What `Save` writes

Into `libros/<slug>/bookmill.toml` (format/comments preserved via `toml_edit`):

- `[cover.<lang>.layout]` — the **canonical** normalized layout: per element
  `{ xPct, yPct, wPct, fontPct, fill, fontFamily, fontStyle, text }` (positions
  as fractions of the 1600×2560 canvas).
- Renderer-honored fields so the re-render reflects edits **today**:
  `[cover].{title_color, sub_color, author_color, bgcolor}`,
  `[cover].title_size` (derived from the title's font fraction), and
  `[cover.<lang>].{title, sub}` when the text was changed.

> **Known v1 limitation:** bookmill's `cover_svg.rs` renderer does not yet read
> per-element **x/y positions** — it lays text out with its own flex math. So
> after Save, color / font-size / text edits appear in the re-rendered cover,
> but **absolute repositioning is persisted to TOML and not yet reflected in the
> render**. Wiring `cover_svg.rs` to honor `[cover.<lang>.layout]` is the
> companion crate-side change (design doc §A.2/§A.3) and is out of scope here.

Protected books (`la-riqueza-de-la-isla`, `no-hay-plata-en-la-isla-de-las-ratas`)
render to a side-by-side comparison path under `output/.../*-cover-resvg.png`
(never overwriting tracked `cover/` assets); the editor displays that comparison
render automatically.

## REST API

| Method | Path | Body / returns |
|---|---|---|
| GET | `/api/books` | discovered books (slug, languages, titles, protected) |
| GET | `/api/cover/{book}/{lang}` | bg/rendered URLs + bgcolor + normalized `elements` |
| POST | `/api/cover/{book}/{lang}` | layout JSON → writes `[cover.*]` TOML, re-renders, returns render log + new asset URL |
| GET | `/api/asset/{book}/{lang}/{kind}` | static image: `kind` ∈ `bg` \| `rendered` |

## TODOs (toward "Full")

- **Honor `[cover.<lang>.layout]` positions in `cover_svg.rs`** so absolute drag
  positions round-trip into the shipped render (crate-side change).
- Multi-version covers (`[cover.<lang>.versions.<name>]`, active version,
  comparison renders).
- Paperback **wrap** + **hardcover** modes (back/spine/front panels, barcode
  keep-out overlay, live spine width from page count via `pdfinfo`).
- Full two-page **interior previewer** (`pdftoppm` page rasterization, per-page
  gutter/bleed overlays, page nav) — current previewer is a cover-only stub.
- Publish **checklist** panel (`validate --deep --json`, sanitized description).
- Editor polish: snapping to guides, rotation, undo/redo, font/color pickers
  bound to the `[cover]` palette, in-canvas textarea editing.
- **`bookmill start`** — fold this server into the main binary (shared config
  modules instead of re-implementing TOML resolution + shelling out).

[axum]: https://github.com/tokio-rs/axum
[Konva.js]: https://konvajs.org/
