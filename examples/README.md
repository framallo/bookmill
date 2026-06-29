# bookmill examples

## `sample-repo/` — a minimal, self-contained demo

A complete, tiny bookmill repo you can build end-to-end. It is **not** a cargo
workspace member — it is plain content + config, used to show the conventions and
to smoke-test the CLI.

### Layout

```
sample-repo/
  bookmill.toml                  # REPO config (no top-level `slug`)
  templates/epub.html            # pandoc EPUB template (build asset)
  css/epub.css                   # EPUB stylesheet (build asset)
  scripts/drop-spot-epub.lua     # pandoc Lua filter (build asset)
  scripts/shrink-epub-images.py  # optional image shrinker (non-fatal if it fails)
  books/
    the-clockwork-garden/
      bookmill.toml              # BOOK config (has a top-level `slug`)
      en/
        chapter-01-the-still-garden.md
        chapter-02-the-key-that-turned-itself.md
        chapter-03-the-garden-that-kept-itself.md
  output/                        # generated (gitignored)
```

The repo config and each book config are both named `bookmill.toml`. They are
disambiguated by content: a **book** config has a top-level `slug` key; the
**repo** config does not (it has `books_dir` / `[defaults]` / `[editions]`).
Discovery walks up from the working directory to the first repo-level
`bookmill.toml`.

### Run it

From inside `sample-repo/`:

```bash
cd examples/sample-repo

bookmill list                  # shows: the-clockwork-garden
bookmill validate              # checks KDP limits + house rules
bookmill build --format epub   # -> output/the-clockwork-garden/en/the-clockwork-garden-en.epub
```

Requires `pandoc` on `PATH` (the EPUB build shells out to it). The resulting EPUB
passes `epubcheck` with zero errors. `scripts/shrink-epub-images.py` needs
Pillow but is optional — the build succeeds without it.

### What it demonstrates

- The two-tier `bookmill.toml` config model (repo defaults -> book overrides).
- Convention-based content selection (`glob = "en/chapter-*.md"`, sorted).
- A minimal `[listing.<lang>]` (blurb, 7 keywords, BISAC, reading age) that passes
  `bookmill validate`.
- A minimal `[cover]` block showing the cover schema. No cover art ships with the
  sample, so `bookmill covers` is not part of the demo — add a `cover/bg.jpg`
  front photo first if you want to render covers.
