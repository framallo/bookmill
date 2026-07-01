# bookmill

A single-binary, convention-over-configuration **book publishing pipeline**. It
turns a Markdown manuscript repo into KDP-ready **EPUB + print PDF + paperback
wrap + eBook cover** (and audiobook), for many books × editions × languages, from
one layered `bookmill.toml`.

It replaces a Make + pandoc + Python cover-script pipeline with one Rust tool:
config-driven metadata (no per-book `meta.md`), native Chrome-free covers, native
EPUB image shrinking, deep KDP/epubcheck validation, a terminal UI, and a project
scaffolder. **There is no pandoc dependency** — every interior (EPUB, print PDF,
and the `docx` review doc) is rendered by native Rust.

```
manuscript (Markdown)  ──bookmill──▶  EPUB · print PDF · wrap cover · eBook cover · audiobook
        + bookmill.toml
```

## Install

```bash
cargo install --path .          # builds target/release/bookmill, installs to ~/.cargo/bin
```

**`bookmill build` needs no external tools at all** — PDF (Typst compiled
in-process via the `typst` crate, no LaTeX, no `typst` binary), EPUB
(epub-builder + comrak), and `docx` (docx-rs) are all native Rust. Verified by
building with an empty `PATH`. The only external tools left are used by optional,
non-build commands:

| Command | Needs |
|---|---|
| `build` (PDF / EPUB / `docx`) | **nothing — fully native Rust** |
| `build cover` | **nothing — native `resvg`** (all books, incl. the formerly Chrome-only protected covers); `--engine chrome` is an opt-in fallback only |
| `validate --deep` | `epubcheck` (Java reference EPUB validator) — the only external tool left, and only here |
| `audiobook` | [`kab`](https://github.com/framallo) (Kokoro TTS on the Apple Neural Engine) |

## Quickstart

Scaffold a new project, then build it:

```bash
bookmill create --interactive        # answer a few prompts -> a buildable project
cd my-project
bookmill list                        # discovered books
bookmill validate my-book            # config + KDP/house-rule check
bookmill build my-book --format all  # retail+KDP EPUB and PDF
```

Everything `create` writes (repo + book `bookmill.toml`, a starter chapter, and
the shared LaTeX/EPUB/CSS build assets) builds out of the box — no hunting for
templates.

## Commands

```
bookmill create [flags]              scaffold a new project (init) or add a book to an existing repo
bookmill list [--json]               list discovered books
bookmill validate [book] [--deep]    config/listing/house-rule check (+epubcheck, PDF geometry, cover res with --deep)
bookmill build [book] [--format|--edition] [--lang]   build interiors (EPUB/PDF)
bookmill build cover [book] [--lang]      render front PNG + paperback wrap PDF + eBook JPG
bookmill build shrink <epub> [--px]       shrink EPUB images in place (native)
bookmill audiobook [book] [--lang] [--voice] [--speed] [--force]   render a chaptered .m4b (kab engine)
bookmill web [--port] [--pages]      launch the cover-editor web UI (localhost)
bookmill tui                         interactive terminal UI (build/cover/validate/audiobook)
```

**Desktop app.** The same UI (library grid, cover editor, previewer) also ships as
a native macOS app you install and open like any other app — no terminal command.
Install it with `brew install --cask bookmill`, then launch **bookmill** from
Applications / Spotlight; it prompts for your book repo (or remembers the last
one). The app (`desktop/`, a Tauri v2 crate) embeds the `bookmill` CLI as a
sidecar, spawns `bookmill web` on an ephemeral port, and points a `WKWebView` at
it. Build/package/cask details: [`docs/DESKTOP.md`](docs/DESKTOP.md).

`build` is the umbrella for produced artifacts: bare `build` builds **interiors**
(the default action), `build cover` renders covers, `build shrink` shrinks an
EPUB. `--format` is `epub | pdf | kdp | print | docx | all` (`docx` = an editor
review doc); `--edition` builds by distribution channel instead (see below).
`--lang` is a language code or `all`. PDFs render through the native Typst
engine (no LaTeX); EPUB through epub-builder + comrak; the `docx` review doc
through docx-rs — all native Rust, no pandoc.

### `bookmill create`

Auto-detects two modes:

* **init** — no repo found: scaffolds a brand-new project (repo `bookmill.toml` +
  first book + shared build assets + `.gitignore`). If a git repo is detected it
  confirms before initializing.
* **add-book** — run inside an existing bookmill repo: scaffolds one new book into
  it.

Driven by **archetypes** (`templates/archetypes.toml`, bundled and overridable by
a project-local copy) over two free dimensions — `scope` (single/series) ×
`languages` (one/many) — so everything from a single monolingual book up to a
multilingual series works. Fully scriptable from flags, or `--interactive` to be
prompted (`--all` prompts every field; otherwise just the essentials).

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
one per book (`slug`, titles, content selection, listing, pdf, cover overrides),
deep-merged **repo → book → edition → language**. A *book* config is distinguished
from the *repo* config by a top-level `slug` key.

Content per language is a glob plus optional ordered front/back matter:

```toml
[content.es]
glob   = "es/capitulo-*.md"
append = ["es/epilogo.md"]
```

### Editions = distribution channels

Each edition carries a `target` that drives the output flavor:

| Edition | Output |
|---|---|
| `kdp-paperback` | bleed print interior (`-kdp.pdf`, full-res) + wrap cover |
| `kdp-epub` | Kindle `-kdp.epub` with lower-res images (smaller delivery fee) |
| `kdp-hardcover` | same bleed interior; case-laminate cover (needs ≥75 pages) |
| `gumroad` | retail EPUB + standalone PDF (full cover page) |
| `bubok-{us,ar,mx}` | POD print interior + cover, per region (trim/paper/ISBN) |

Page geometry (trim + bleed + margins) is resolved from config per edition, so a
new regional trim is a config edit, not a code change.

### Audiobooks

`bookmill audiobook <book> --lang <lang>` renders a chaptered `.m4b` via **kab**
(Kokoro on the Apple Neural Engine). Chapters resolve through the same
`[content.<lang>]` selection as the book (so appended epilogues are included), and
voice/speed/language come from `[audiobook]` / `[audiobook.<lang>]`:

```toml
[audiobook]
engine = "kab"

[audiobook.es]
voice = "ef_dora"   # Spanish female
code  = "e"         # ane_book language code
```

Override ad hoc with `--voice` / `--speed`. The engine is a `TtsEngine` trait, so
other backends slot in behind the same interface.

## Layout

```
src/
  main.rs        CLI (clap)
  config.rs      layered TOML model + resolution + validation
  discover.rs    repo/book discovery
  create.rs      `bookmill create` scaffolder (archetypes + bundled assets)
  build.rs       request → jobs → queue (Typst PDF + native EPUB/docx)
  typst_pdf.rs   native Typst PDF engine (sole PDF backend)
  epub_native.rs native EPUB3 builder (epub-builder + comrak; no pandoc)
  docx_native.rs native `.docx` review-doc writer (docx-rs; no pandoc)
  audiobook.rs   TtsEngine trait + kab adapter
  covers.rs / cover_svg.rs / cover_tmpl.rs   cover rendering (resvg default)
  epub_shrink.rs native EPUB image shrinker
  deep.rs        validate --deep (epubcheck + geometry + cover res)
  tui.rs         Ratatui terminal UI
templates/
  archetypes.toml   project archetypes for `create`
  scaffold/         build assets written into new projects
  cover/            cover HTML templates
  web.rs / web/    cover-editor server (`bookmill web`); cover.rs/render.rs shared
web/             standalone crate (same editor; superseded by `bookmill web`)
examples/sample-repo/   self-contained demo repo
```

## Status

Build, covers, and validation are in daily production use. The audiobook engine
renders via kab (with a content-hash per-chapter cache). Every interior is native
Rust: PDFs through the in-process **Typst crate** (no LaTeX, no `typst` binary),
EPUB through epub-builder + comrak, the `docx` review doc through docx-rs, and PDF
page metadata + image-DPI/bleed audits through `lopdf` (no poppler) — and covers
render via native `resvg`. **The whole build + cover pipeline runs with no
external tools at all.** The only external left is **`epubcheck`** (the Java
reference EPUB validator), used solely by `validate --deep`, plus `kab` for the
optional `audiobook` command. See `HANDOFF.md`.

## License

See `LICENSE`.
