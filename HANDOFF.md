# bookmill — handoff

Last updated: 2026-06-28. Author: Federico Ramallo (framallo@gmail.com).

This is the practical "pick it up and keep going" doc. The **design rationale**
lives in `BOOKMILL-PLAN.md` (gitignored, repo root) — read it for the *why*; read
this for the *where things are and what's left*.

## What bookmill is

A single-binary Rust tool that turns a Markdown manuscript repo into KDP-ready
**EPUB + print PDF + paperback wrap + eBook cover** (and, eventually, audiobook),
for multiple books × editions × languages. It replaced the old Make + pandoc +
`make-covers.py` pipeline. Config is one `bookmill.toml` per repo + one per book,
deep-merged `repo → book → edition → language`.

It is the **sole builder** for the 11 books in
`~/work/argentina_animal_libertaria` — the legacy Makefile/pandoc path no longer
builds them (Phase 8 deleted every `meta.md`).

## Status at a glance

- **Build / covers / validation: production, in daily use.** All 11 books build
  to print PDF + Kindle EPUB; `la-riqueza-de-la-isla` is submitted to KDP and
  reproduced byte-for-byte; `no-hay-plata-en-la-isla-de-las-ratas` is cover- and
  interior-final at $12.99.
- **Audiobook: v1 implemented** (`src/audiobook.rs`). `TtsEngine` trait + `kab`
  adapter, config-driven (`[audiobook]`/`[audiobook.<lang>]`), wired into the CLI
  (`bookmill audiobook`, with `--voice`/`--speed`) and the TUI. Chapters resolve
  through `[content.<lang>]` (appended epilogues included — the old `capitulo-*.md`
  Makefile glob dropped them) and are staged numbered so kab's lexical glob keeps
  bookmill's order. **NOT yet done:** the content-hash segment cache for
  incremental re-renders (kab `convert` re-renders the whole book each call).
- **Project scaffolder: implemented** (`src/create.rs`). `bookmill create`
  auto-detects init (new repo) vs add-book (inside a repo), driven by
  `templates/archetypes.toml` (scope × languages); writes a buildable project
  including the shared build assets in `templates/scaffold/`. Flags + `--interactive`.
- **Web cover editor: works except absolute drag-positions don't round-trip into
  the render** (renderer-side gap, see below).
- **PDF engine still shells out** to pandoc/xelatex — the native Typst swap (v2)
  is not done.
- **Git: this is the first commit.** Before now the whole project was untracked.

## Repo layout

```
bookmill/
  Cargo.toml  Cargo.lock        # main crate
  src/
    main.rs        CLI (clap): list/validate/content/tui/build/covers/shrink/audiobook
    config.rs      TOML model + layered resolution + load-time validation
    discover.rs    walk repo, find books (book cfg has top-level `slug`)
    build.rs       request → Vec<Job> (book×edition/format×lang×output); runs the queue
    deep.rs        `validate --deep`: epubcheck + PDF-geometry + cover-resolution audits
    covers.rs      `bookmill covers` orchestrator (HTML → headless Chrome → PNG/PDF)
    cover_tmpl.rs  fills templates/cover/{front,wrap}.html.tmpl from [cover] config
    cover_svg.rs   SVG cover renderer (resvg path) — see GAP below
    epub_shrink.rs native EPUB image shrinker (Python-free)
    tui.rs         Ratatui terminal UI (build/covers/validate/audiobook actions)
  templates/cover/ front.html.tmpl, wrap.html.tmpl (byte-identical to make-covers.py)
  docs/            design research (kdp-requirements, edition-options, web-ui-design, epub-shrink-tuning)
  examples/sample-repo/  self-contained demo repo (builds clean, epubcheck 0 errors)
  web/             SEPARATE crate: axum + Konva.js cover editor (see web/README.md)
  BOOKMILL-PLAN.md design doc (gitignored)
```

The books repo it operates on defaults to `~/work/argentina_animal_libertaria`
(override with `--repo` / `BOOKMILL_REPO`).

## How to build & run

```bash
cd ~/work/libs/bookmill
cargo build --release            # produces target/release/bookmill (also at ~/.cargo/bin/bookmill)

# from the books repo (or pass --repo):
bookmill list                    # discovered books
bookmill validate <slug>         # fast config/listing/house-rule check
bookmill validate <slug> --deep  # + epubcheck + PDF geometry + cover resolution (needs epubcheck, pdfinfo on PATH)
bookmill build <slug> --format all --lang all     # epub|pdf|kdp|print|all
bookmill build <slug> --edition kdp-epub          # build by edition instead of format
bookmill covers <slug> --lang all                 # front PNG + wrap PDF + eBook JPG
bookmill tui                                       # interactive terminal UI

# Web cover editor (separate crate):
cd web && cargo run              # http://127.0.0.1:7777  (build the main binary first)
```

External tools still required: **pandoc + xelatex** (PDF), **headless Chrome**
(covers via `bookmill covers`), **epubcheck + pdfinfo** (`validate --deep`), and
**kab** (audiobook, once implemented). The endgame is to drop all but the
audiobook engine.

## What's done (Phases 1–14, condensed)

Build queue with progress/ETA; `bookmill covers`; per-edition page geometry from
`trim`+`bleed`; all 11 books migrated to `bookmill.toml`; config-driven metadata
(no `meta.md`); covers migrated to TOML + bundled templates (byte-identical to the
old Python); deep validation (epubcheck + geometry + cover-res); a sample repo;
and a full sweep building print PDF + Kindle EPUB for all 11 books / 20 editions
at 100% pass. Full per-phase notes are in `BOOKMILL-PLAN.md` → "Implementation
progress".

## What's NOT done — the work queue

1. **Audiobook segment cache (the remaining audiobook work).** The v1 engine
   (`src/audiobook.rs`: `TtsEngine` trait + `kab` adapter, config-driven, wired
   into CLI + TUI) is done and replaces the Makefile recipe. What's left is the
   **content-hash AST segment cache** (hash = text + voice + engine/model version
   + speed) so fixing one line re-renders only that clip — what makes Federico's
   proofread-by-listening loop cheap. `kab convert` re-renders the whole book, so
   the cache needs per-segment synth: `ane_book.py` exposes a single-chapter
   `--text-file … --out …` path the cache can drive, then mux the `.m4b` with
   chapter markers (kab's `Assemble.swift` does this today). Slot it in as a
   caching `TtsEngine` that wraps `KabEngine`. Spec: `BOOKMILL-PLAN.md` → "Audiobook".

2. **Cover editor position round-trip (renderer gap).** The web editor saves
   per-element `[cover.<lang>.layout]` (xPct/yPct/wPct/fontPct/…) to TOML, and
   color/font-size/text edits already reflect in the re-render — but **absolute
   x/y drag positions are persisted and ignored**, because `cover_svg.rs` lays
   text out with its own flex math instead of reading the layout block. Wiring
   `cover_svg.rs` to honor `[cover.<lang>.layout]` is the fix (design doc
   §A.2/§A.3; web/README.md "Known v1 limitation").

3. **Native PDF engine (v2 portability).** Replace the pandoc/xelatex shell-out
   with AST→Typst via the `typst` crate (and comrak for MD→AST, epub-builder for
   EPUB, docx-rs for DOCX). Goal: zero external deps, one static binary.

4. **EN translations for the two es-only fables** —
   `libre-para-elegir-en-la-isla` (Friedman) and `lo-que-nadie-sabe-de-la-isla`
   (Hayek). Flagged TODO in each `bookmill.toml`; must be **human-translated**,
   not machine. Their cover **art** (bg.jpg/wrap-bg.png) is also still TODO.

5. **Web UI breadth** — multi-version covers, paperback wrap/hardcover modes in
   the editor, the full two-page interior previewer (currently a stub), and a
   publish-checklist panel reusing `validate --deep --json`. Eventually fold the
   `web/` server into the main binary as `bookmill start`.

6. **Image-DPI auditing** in `validate --deep` — left out because it's not
   cheap/reliable via pdfinfo/pdfimages (documented in `deep.rs`).

### Decided

- **Name: `bookmill` (locked).** No longer a placeholder — it's the binary name,
  the crate name, and the GitHub repo (`framallo/bookmill`, private).

### Open decisions (need Federico)

1. DOCX: native `docx-rs` writer, drop entirely, or keep pandoc just for `--format docx`.
2. Distribution: personal (pipx-style) vs shareable/OSS.
3. Graphite cover-editing depth: SVG-bridge (works now) vs procedural integration
   (waits on Graphite's headless API).

## Gotchas

- **Protected books** (`la-riqueza-de-la-isla`,
  `no-hay-plata-en-la-isla-de-las-ratas`): `bookmill covers` renders only a
  side-by-side `*-cover-resvg.*` comparison and **does not overwrite** the tracked
  `cover/` assets. Canonical cover files for these are wired in **manually** — do
  not assume `bookmill covers` updated them.
- **Spine text** needs ≥100 pages (KDP rule). The short text books (e.g.
  no-hay-plata at 86–87pp) ship a **blank spine** by design.
- A book config is distinguished from the repo config by a top-level `slug` key
  (`discover.rs`).
- `web/` is its own crate with its own `target/` — both `target/` dirs are
  gitignored; `BOOKMILL-PLAN.md` is gitignored too.
