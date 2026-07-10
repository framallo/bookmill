<div align="center">

<img src="desktop/src-tauri/icons/icon.png" alt="bookmill logo" width="112" />

# bookmill

**A single-binary book publishing pipeline — Markdown in, KDP-ready EPUB, print PDF, covers, and audiobooks out.**

![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange?logo=rust&logoColor=white)
![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-blue)
![No pandoc](https://img.shields.io/badge/pandoc-not%20required-brightgreen)

</div>

bookmill turns a Markdown manuscript repo into publishable books — **EPUB, print
PDF, paperback wrap cover, eBook cover, and audiobook** — for many books ×
editions × languages, all from one layered `bookmill.toml`. It replaces a
Make + pandoc + Python cover-script setup with a single Rust tool: every interior
is rendered by native Rust, covers are Chrome-free, and a scaffolder writes
buildable projects for you.

```
manuscript (Markdown)  ──bookmill──▶  EPUB · print PDF · wrap cover · eBook cover · audiobook
        + bookmill.toml
```

## Features

- **Fully native interiors — no pandoc, no LaTeX.** PDF via the in-process
  [Typst](https://typst.app) crate (no `typst` binary), EPUB via
  epub-builder + comrak, and a `.docx` review doc via docx-rs.
- **Chrome-free covers.** Front PNG, paperback wrap PDF, and eBook JPG rendered
  with native [`resvg`](https://github.com/linebender/resvg); Chrome is an opt-in
  fallback only.
- **Editions are distribution channels.** KDP paperback / hardcover / eBook,
  Gumroad, and regional POD — each resolves its own trim, bleed, margins, and
  cover from config.
- **Convention over configuration.** One `bookmill.toml` per repo + one per book,
  deep-merged repo → book → edition → language. No per-book `meta.md`.
- **Audiobooks.** Chaptered `.m4b` via [`kab`](https://github.com/framallo)
  (Kokoro TTS on the Apple Neural Engine), with a per-chapter content-hash cache.
- **Three ways to drive it.** A CLI, an interactive terminal UI (`bookmill tui`),
  and a native macOS desktop app (library grid + cover editor + previewer).
- **Scaffolder.** `bookmill create` writes a project that builds out of the
  box — repo + book config, a starter chapter, and shared build assets.
- **Rich chapter images.** Size, fit, align, and frame any image from the
  markdown itself with pandoc-style `{width= height= fit= align= border=}`
  attributes — a book-level default plus per-image overrides.

## Install

### Users — Homebrew

```bash
brew install framallo/tap/bookmill        # the CLI (builds from source)
```

The desktop app is packaged as a cask and goes live once a signed, notarized DMG
is published: `brew install --cask framallo/tap/bookmill`.

### Contributors — from source

```bash
git clone https://github.com/framallo/bookmill && cd bookmill
cargo run -- tui              # run without installing
make deploy                   # or: install to ~/.cargo/bin (cargo install --path . --force)
make run                      # dev window for the desktop app on your book repo
```

> [!NOTE]
> **`bookmill build` (interiors + covers) needs no external tools** — verified by
> building with an empty `PATH`. Only optional, non-build commands shell out to a
> single tool each: `epubcheck` (Java) for `validate --deep`, `pdftoppm`
> (poppler) for the interior previewer, `kab` for `audiobook`, and the repo's
> `scripts/lint-prose.py` (LanguageTool) / `scripts/kdp-metadata.py` for `lint`
> and `kdp`. `words` is fully native. Building the CLI needs the Rust toolchain
> and `nasm` (both are pulled in by Homebrew).

## Quickstart

Scaffold a new project, then build it:

```bash
bookmill create --interactive        # a few prompts → a buildable project
cd my-project
bookmill list                        # discovered books
bookmill validate my-book            # config + KDP/house-rule check
bookmill build my-book --format all  # retail + KDP EPUB and PDF
```

> [!TIP]
> Prefer clicking to typing? `bookmill tui` opens an interactive terminal UI for
> build / cover / validate / audiobook, and the desktop app wraps the same tools
> in a native window.

## Commands

| Command | What it does |
|---|---|
| `bookmill create [flags]` | Scaffold a new project (**init**) or add a book to an existing repo (**add-book**) |
| `bookmill list [--json]` | List discovered books |
| `bookmill validate [book] [--deep]` | Config / listing / house-rule check (+ epubcheck, PDF geometry, cover resolution with `--deep`) |
| `bookmill build [book] [--format\|--edition] [--lang]` | Build interiors (EPUB / PDF / KDP / print / docx / all) |
| `bookmill build cover [book] [--lang]` | Render front PNG + paperback wrap PDF + eBook JPG |
| `bookmill build shrink <file> [--px\|--dpi]` | Shrink images in place — EPUB (native, `--px`) or PDF (Ghostscript, `--dpi`) |
| `bookmill audiobook [book] [--lang] [--voice] [--speed]` | Render a chaptered `.m4b` (kab engine) |
| `bookmill words [book] [--lang]` | Word counts (native, over the resolved content) + page counts (from the built interior PDF) |
| `bookmill lint [book] [--lang]` | Prose lint — grammar + Spanish tildes (LanguageTool via `scripts/lint-prose.py`) |
| `bookmill kdp [book]` | Scaffold `kdp/<slug>.md` when missing, else check it against KDP limits (via `scripts/kdp-metadata.py`) |
| `bookmill web [--port] [--pages]` | Launch the cover-editor web UI (localhost) |
| `bookmill tui` | Interactive terminal UI |

`build` is the umbrella for produced artifacts: bare `build` builds **interiors**,
`build cover` renders covers, `build shrink` shrinks an EPUB or PDF in place
(dispatched by extension: `.epub` → native image shrink `--px`; `.pdf` →
Ghostscript downsample `--dpi`, default 150). `--format` is
`epub | pdf | kdp | print | docx | all` (`docx` = an editor review doc);
`--edition` builds by distribution channel instead; `--lang` is a language code
or `all`.

### `bookmill create`

Auto-detects two modes: **init** (no repo found → scaffold a brand-new project)
and **add-book** (run inside a bookmill repo → add one book). It is driven by
**archetypes** (`templates/archetypes.toml`, overridable per project) over two
free dimensions — `scope` (single / series) × `languages` (one / many) — so
everything from a single monolingual book to a multilingual series works.

```bash
# scripted: a bilingual series, first book
bookmill create --archetype series-multilang --slug no-silver \
  --title es="No hay plata" --title en="No Silver" --author "Jane Doe" --yes

# add another book to the repo you're in
bookmill create --slug second-book --title en="Second Book" --yes
```

Add a new archetype or language convention by editing `archetypes.toml` — no code
change needed.

## Configuration

One `bookmill.toml` per repo (defaults, editions, margins, cover, audiobook) plus
one per book (`slug`, titles, content selection, listing, overrides), deep-merged
**repo → book → edition → language**. A *book* config is distinguished from the
*repo* config by a top-level `slug` key.

Content per language is a glob plus optional ordered front/back matter:

```toml
[content.es]
glob   = "es/capitulo-*.md"
append = ["es/epilogo.md"]
```

### Chapter images

A chapter that opens with a standalone image renders it as a full-page **plate**
on the facing (verso) page; other standalone images render inline. Any image
takes an optional pandoc-style `{…}` attribute block after `![alt](src)` to
control how it's placed — a book-level default (`[pdf].plate_width`) plus
per-image overrides:

| Attribute | Values | Applies to |
|---|---|---|
| `width=` | `55%` · `3in` (bare number ⇒ `%`) | plate + inline, EPUB |
| `height=` | `2in` · `40%` | plate + inline (PDF) |
| `fit=` | `cover` · `contain` · `stretch` | plate + inline (PDF) |
| `align=` | `left` · `center` · `right` | inline (PDF + EPUB) |
| `border=false` / `.plain` | drop the framed-plate keyline | plate (PDF) |
| `.spot` | small centered tailpiece | print-only |

```markdown
![Carl Menger (1840–1921)](images/ch03.jpg){width=70% fit=contain .plain}
```

`width` and `align` also carry into the reflowable EPUB `<img>`; `height`, `fit`,
and `border` are print-PDF layout concepts. Set the whole book's default plate
width with `plate_width` under `[pdf]` (`0.0`–`1.0`; `1.0` = full text column).

### Editions = distribution channels

Each edition carries a `target` that drives the output flavor. Page geometry
(trim + bleed + margins) resolves from config per edition, so a new regional trim
is a config edit, not a code change.

| Edition | Output |
|---|---|
| `kdp-paperback` | Bleed print interior (`-kdp.pdf`, full-res) + wrap cover |
| `kdp-epub` | Kindle `-kdp.epub` with lower-res images (smaller delivery fee) |
| `kdp-hardcover` | Same bleed interior; case-laminate cover (needs ≥ 75 pages) |
| `gumroad` | Retail EPUB + standalone PDF (full cover page) |
| `bubok-{us,ar,mx}` | POD print interior + cover, per region (trim / paper / ISBN) |

The **retail PDF** (`gumroad`) can be auto-downsampled so the paid download stays
small while the print interior keeps full-res images. Set `[pdf].digital_pdf_dpi`
(or `[editions.<name>].digital_pdf_dpi` to override) — e.g. `digital_pdf_dpi = 150`
turns a full-bleed color picture book from ~200 MB into a few MB. It runs
Ghostscript on `Out::RetailPdf` only; a missing `gs` warns and keeps the full-res
PDF (it never fails the build). Off (`None`) by default.

### Post-build documents (markdown, free sample, README)

Every interior build also drops three documents into `output/` for each book +
language it touched (best-effort — a failure warns but never fails the build):

| File | What |
|---|---|
| `output/<slug>/<lang>/<slug>-<lang>.md` | The **combined manuscript** — YAML front matter (title/subtitle/author/rights) + all chapters concatenated in reading order |
| `output/<slug>/<lang>/<slug>-<lang>-sample.{epub,pdf}` | A **free sample** — the opening chapters + a localized "end of the sample" note, as a shareable EPUB + PDF (title marked *(Sample)* / *(Muestra)*) |
| `output/<slug>/README.md` | A **README** describing the book: author/series/date, editions, status/ASINs, and per language the title, word & page counts, reading age, keywords, BISAC, blurb, and the files built |

The sample size is `[sample].chapters` (leading content files); unset defaults to
~the first 15% (min 1, capped at half the book). `[sample].enabled = false` skips
it. The sample EPUB is image-shrunk like the retail EPUB, and the sample PDF
honors `[pdf].digital_pdf_dpi`, so a picture-book preview stays a couple of MB.

### Audiobooks

`bookmill audiobook <book> --lang <lang>` renders a chaptered `.m4b` via **kab**.
Chapters resolve through the same `[content.<lang>]` selection as the book (so
appended epilogues are included); voice, speed, and language come from
`[audiobook]` / `[audiobook.<lang>]`:

```toml
[audiobook]
engine = "kab"

[audiobook.es]
voice = "ef_dora"   # Spanish female
code  = "e"         # ane_book language code
```

Override ad hoc with `--voice` / `--speed`. The engine is a `TtsEngine` trait, so
other backends slot in behind the same interface.

## Desktop app

The same UI (library grid, cover editor, previewer) ships as a native macOS app —
a [Tauri v2](https://tauri.app) shell (`desktop/`) that embeds the `bookmill` CLI
as a sidecar, spawns `bookmill web` on an ephemeral port, and points a `WKWebView`
at it. Launch **bookmill** from Applications / Spotlight and it prompts for your
book repo (or remembers the last one).

```bash
make run      # fast dev window on your repo
make app      # build the bookmill.app bundle
make dmg      # build the installable .dmg
```

> [!IMPORTANT]
> The interior previewer needs `pdftoppm` (poppler) on the app's `$PATH`. A
> `brew install --cask bookmill` distribution is planned but **not available
> yet** — it needs a signed + notarized release and a Homebrew tap. See
> [`docs/DESKTOP.md`](docs/DESKTOP.md).

## Project layout

```
src/
  main.rs        CLI (clap)
  config.rs      layered TOML model + resolution + validation
  discover.rs    repo/book discovery
  create.rs      `bookmill create` scaffolder (archetypes + bundled assets)
  build.rs       request → jobs → queue (Typst PDF + native EPUB/docx)
  typst_pdf.rs   native Typst PDF engine (sole PDF backend)
  epub_native.rs native EPUB3 builder (epub-builder + comrak; no pandoc)
  docx_native.rs native .docx review-doc writer (docx-rs; no pandoc)
  audiobook.rs   TtsEngine trait + kab adapter
  covers.rs / cover_svg.rs / cover_tmpl.rs   cover rendering (resvg default)
  epub_shrink.rs native EPUB image shrinker
  pdf_shrink.rs  digital-PDF image downsampler (Ghostscript)
  book_docs.rs   post-build markdown + free sample + per-book README
  deep.rs        validate --deep (epubcheck + geometry + cover res)
  tui.rs         Ratatui terminal UI
templates/       archetypes for `create`, scaffold assets, cover templates
web/             standalone cover-editor crate (superseded by `bookmill web`)
desktop/         Tauri v2 macOS app
examples/sample-repo/   self-contained demo repo
```

## Documentation

- [`docs/DESKTOP.md`](docs/DESKTOP.md) — desktop app build, packaging, and cask notes
- [`docs/edition-options.md`](docs/edition-options.md) — edition / geometry reference
- [`docs/kdp-requirements.md`](docs/kdp-requirements.md) — KDP print & cover rules
- [`docs/epub-shrink-tuning.md`](docs/epub-shrink-tuning.md) — image-shrink codec tuning
- [`docs/web-ui-design.md`](docs/web-ui-design.md) — cover-editor UI design
- [`examples/sample-repo/`](examples/sample-repo/) — a minimal, buildable project

> [!NOTE]
> Build, covers, and validation are in daily production use. The whole build +
> cover pipeline runs with no external tools; optional commands (`validate
> --deep`, the previewer, `audiobook`) each shell out to one tool. See
> [`HANDOFF.md`](HANDOFF.md) for current status.
