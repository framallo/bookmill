//! Read + resolve + persist a book's cover config for the web editor.
//!
//! Standalone v1: we re-implement just enough of bookmill's layered `[cover]`
//! resolution (book over repo over built-in defaults) using `toml_edit`, so the
//! main crate stays untouched. Reads return what the Konva editor needs (text,
//! colors, sizes, a normalized fractional layout); writes merge the edited
//! layout + style fields back into the book `bookmill.toml`, preserving comments
//! and formatting. The *authoritative* render is then produced by shelling out
//! to `bookmill covers` (see `render.rs`); we never rasterize here.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use bookmill::config::TextRun;
use toml_edit::{value, DocumentMut, Item, Table, Value};

pub const CANVAS_W: f64 = 1600.0;
pub const CANVAS_H: f64 = 2560.0;

// Fallback author for the book-list summary when a repo sets none; the cover
// editor's resolved defaults come from `bookmill::resolve_editor_cover` (M8).
const DEFAULT_AUTHOR: &str = "Federico Ramallo";
const BADGE_ES: &str = "Serie Isla de la Libertad";
const BADGE_EN: &str = "Liberty Island Series";
const DEFAULT_ACCENT: &str = "#000000";
const DEFAULT_TITLE_MT: &str = "auto";
const DEFAULT_AUTHOR_MT: &str = "84px";

/// Repo + books_dir, discovered once at startup.
pub struct Repo {
    pub root: PathBuf,
    pub books_dir: PathBuf,
    pub author: String,
    /// repo-level [cover] table (defaults), if any.
    pub repo_doc: DocumentMut,
}

impl Repo {
    pub fn open(root: &Path) -> Result<Repo> {
        let cfg = root.join("bookmill.toml");
        let text = std::fs::read_to_string(&cfg)
            .with_context(|| format!("reading repo config {}", cfg.display()))?;
        let doc: DocumentMut = text.parse().context("parsing repo bookmill.toml")?;
        let books_dir = doc
            .get("books_dir")
            .and_then(|i| i.as_str())
            .unwrap_or("books")
            .to_string();
        let author = doc
            .get("author")
            .and_then(|i| i.as_str())
            .unwrap_or(DEFAULT_AUTHOR)
            .to_string();
        Ok(Repo {
            root: root.to_path_buf(),
            books_dir: root.join(books_dir),
            author,
            repo_doc: doc,
        })
    }

    /// List discovered books (dirs under books_dir with a book-level bookmill.toml).
    pub fn books(&self) -> Result<Vec<BookSummary>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.books_dir)
            .with_context(|| format!("reading books dir {}", self.books_dir.display()))?
        {
            let p = entry?.path();
            let cfg = p.join("bookmill.toml");
            if !cfg.exists() {
                continue;
            }
            let text = match std::fs::read_to_string(&cfg) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let doc: DocumentMut = match text.parse() {
                Ok(d) => d,
                Err(_) => continue,
            };
            // A *book* config has a top-level `slug`.
            let slug = match doc.get("slug").and_then(|i| i.as_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let languages = str_array(doc.get("languages"));
            let mut titles = std::collections::BTreeMap::new();
            if let Some(t) = doc.get("title").and_then(|i| i.as_table()) {
                for (k, v) in t.iter() {
                    if let Some(s) = v.as_str() {
                        titles.insert(k.to_string(), s.to_string());
                    }
                }
            }
            out.push(BookSummary { slug, dir: p, languages, titles });
        }
        out.sort_by(|a, b| a.slug.cmp(&b.slug));
        Ok(out)
    }

    pub fn find_book(&self, slug: &str) -> Result<BookSummary> {
        self.books()?
            .into_iter()
            .find(|b| b.slug == slug)
            .with_context(|| format!("book not found: {slug}"))
    }
}

pub struct BookSummary {
    pub slug: String,
    pub dir: PathBuf,
    pub languages: Vec<String>,
    pub titles: std::collections::BTreeMap<String, String>,
}

// --------------------------------------------------------------------------
// Resolved cover for the editor
// --------------------------------------------------------------------------

#[derive(Serialize)]
pub struct CoverResponse {
    pub slug: String,
    pub lang: String,
    pub canvas: Canvas,
    pub bgcolor: String,
    pub bg_url: String,
    pub rendered_url: Option<String>,
    pub protected: bool,
    pub elements: Elements,
    /// True when `[cover.<lang>.layout]` was present (positions came from the
    /// saved layout). When false, the editor should compute the default flex
    /// positions itself (matching `cover_svg.rs`'s flex render).
    pub layout_saved: bool,
    /// Fixed series badge drawn at top-center by the renderer (not draggable).
    pub badge: String,
    pub badge_color: String,
    /// Accent color for the rule under the title block.
    pub accent: String,
    /// Flex spacing inputs the editor needs to reproduce the default layout:
    /// title block's top margin and the author's top margin. CSS-ish: `auto`,
    /// `<n>%` (of the 1360px content width), or `<n>px`.
    pub title_mt: String,
    pub author_mt: String,
    /// Back-cover blurb for the paperback wrap (`[cover.<lang>].blurb`, falling
    /// back to `[listing.<lang>].blurb`). Editable in the editor's wrap mode.
    pub blurb: String,
    /// Draggable back-panel blocks for the wrap (blurb/badge/author), as
    /// back-panel fractions. Seeded from `[cover.<lang>.wrap]` when saved, else
    /// from defaults matching the renderer's `back_absolute` flex-equivalent
    /// positions. The editor positions these within the back-panel sub-rect of the
    /// wrap SVG (whose width the SVG exposes as `data-back-w`).
    pub wrap: WrapLayoutResponse,
    /// True when `[cover.<lang>.wrap]` was present (positions came from the saved
    /// wrap layout). When false, the seeded defaults match the renderer's flex back.
    pub wrap_saved: bool,
    /// Which surface the editor should expose: "front" (digital-only) or "wrap"
    /// (paperback — front and back both editable). Set by the axum handler from the
    /// book's editions / explicit `[cover].edit`; `load_cover` seeds a safe default.
    #[serde(default)]
    pub edit_mode: String,
}

/// Back-panel layout the editor seeds its wrap drag handles from. Each block is a
/// full [`Element`] (back-panel fractions).
#[derive(Serialize)]
pub struct WrapLayoutResponse {
    pub blurb: Element,
    pub badge: Element,
    pub author: Element,
}

#[derive(Serialize)]
pub struct Canvas {
    pub w: f64,
    pub h: f64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Elements {
    pub title: Element,
    pub subtitle: Element,
    pub author: Element,
}

/// One draggable text block, persisted as canvas fractions (resolution-free).
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Element {
    pub text: String,
    /// center X as fraction of canvas width
    pub x_pct: f64,
    /// center Y as fraction of canvas height
    pub y_pct: f64,
    /// block width as fraction of canvas width (wrap box)
    pub w_pct: f64,
    /// font size as fraction of canvas height
    pub font_pct: f64,
    pub fill: String,
    pub font_family: String,
    pub font_style: String,
    // Optional formatting (default = renderer's prior behavior). snake_case JSON,
    // matching what app.js reads/writes on each element.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub letter_spacing: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_transform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// Per-character styling: the block's text as **styled runs**. When present it
    /// is the source of truth for the text (`text` stays the plain concatenation,
    /// for anything that just wants the string). Absent/unstyled => plain `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runs: Option<Vec<bookmill::config::TextRun>>,
}

/// Build the editor payload for (book, lang).
pub fn load_cover(repo: &Repo, book: &BookSummary, lang: &str) -> Result<CoverResponse> {
    let book_doc = read_book_doc(book)?;

    // Resolve text/colors/fonts/size via the build's OWN layered resolver so the
    // editor's initial canvas matches exactly what `build cover` renders (M8).
    // Replaces the former hand-rolled partial `toml_edit` merge (hardcoded author +
    // title size, book-over-repo only) that could drift from the build.
    let d = bookmill::resolve_editor_cover(&repo.root, &book.dir, lang)?;

    // Fixed-decoration + flex-spacing fields so the editor can reproduce the
    // renderer's default flex layout (title top with title_mt, badge above,
    // subtitle under, author at bottom). Mirrors cover_tmpl.rs::resolve.
    let r = &repo.repo_doc;
    let accent = cover_str(&book_doc, r, "accent").unwrap_or_else(|| DEFAULT_ACCENT.into());
    let badge = match lang {
        "es" => cover_str(&book_doc, r, "badge_es").unwrap_or_else(|| BADGE_ES.into()),
        _ => cover_str(&book_doc, r, "badge_en").unwrap_or_else(|| BADGE_EN.into()),
    };
    let badge_color = cover_str(&book_doc, r, "badge_color").unwrap_or_else(|| accent.clone());
    let title_mt = cover_str(&book_doc, r, "title_mt").unwrap_or_else(|| DEFAULT_TITLE_MT.into());
    let author_mt = cover_str(&book_doc, r, "author_mt").unwrap_or_else(|| DEFAULT_AUTHOR_MT.into());

    // Back-cover blurb for the wrap: `[cover.<lang>].blurb`, else `[listing.<lang>].blurb`.
    let blurb = lang_str(&book_doc, "cover", lang, "blurb")
        .or_else(|| lang_str(&book_doc, "listing", lang, "blurb"))
        .unwrap_or_default();
    // Blurb color + serif for the wrap back-panel default seed (before `d`'s fields
    // are moved into the front elements below).
    let wrap_blurb_color = d.blurb_color.clone();
    let wrap_serif = d.serif.clone();

    // Existing saved layout, if any, wins over computed defaults.
    let saved = read_saved_layout(&book_doc, lang);

    // A saved block legitimately carries NO text: the shared `[cover.layout]` holds
    // geometry only, and `[cover.<lang>.layout]` overrides the text of just the
    // blocks that need it (usually only the title's line break). The text of the
    // rest resolves per-language from [title]/[subtitle]/author — which is exactly
    // what the renderer's `def_text` does. Do the same here, or the editor opens
    // with an empty subtitle/author box.
    let (d_title, d_sub, d_author) = (d.title.clone(), d.sub.clone(), d.author.clone());
    let with_text = |mut e: Element, fallback: &str| -> Element {
        if e.runs.is_none() && e.text.trim().is_empty() {
            e.text = fallback.to_string();
        }
        e
    };

    let title_el = saved
        .as_ref()
        .map(|e| with_text(e.title.clone(), &d_title))
        .unwrap_or(Element {
        text: d.title,
        x_pct: 0.5,
        y_pct: 0.62,
        w_pct: 0.80,
        font_pct: d.title_size / CANVAS_H,
        fill: d.title_color,
        font_family: d.serif,
        font_style: "bold".into(),
        ..Default::default()
    });
    let sub_el = saved
        .as_ref()
        .map(|e| with_text(e.subtitle.clone(), &d_sub))
        .unwrap_or(Element {
        text: d.sub,
        x_pct: 0.5,
        y_pct: 0.76,
        w_pct: 0.85,
        font_pct: 48.0 / CANVAS_H,
        fill: d.sub_color,
        font_family: d.sub_font,
        font_style: d.sub_style,
        ..Default::default()
    });
    let author_el = saved
        .as_ref()
        .map(|e| with_text(e.author.clone(), &d_author))
        .unwrap_or(Element {
        text: d.author,
        x_pct: 0.5,
        y_pct: 0.93,
        w_pct: 0.80,
        font_pct: 42.0 / CANVAS_H,
        fill: d.author_color,
        font_family: "Montserrat".into(),
        font_style: "normal".into(),
        ..Default::default()
    });
    // Wrap back-panel layout: seed the editor's drag handles from the saved
    // `[cover.<lang>.wrap]` when present, else from defaults matching the renderer's
    // `back_absolute` positions (fractions of the back panel). The back panel is
    // `bleed + trim_w` wide by `trim_h + 2·bleed` tall (× 96 px/in); fractions are
    // resolution-free, so we work in px only to derive w/font fractions.
    const DPI: f64 = 96.0;
    let back_w_px = (d.bleed + d.trim_w) * DPI;
    let fh_px = (d.trim_h + 2.0 * d.bleed) * DPI;
    let bcontent_w = back_w_px - 2.0 * 0.55 * DPI; // matches wrap_svg bpad_x
    let w_frac = (bcontent_w / back_w_px).max(0.05);
    let saved_wrap = read_saved_wrap(&book_doc, lang);
    let wrap_blurb = saved_wrap.as_ref().and_then(|w| w.blurb.clone()).map(|e| with_text(e, &blurb)).unwrap_or(Element {
        text: blurb.clone(),
        x_pct: 0.5,
        y_pct: 0.42,
        w_pct: w_frac,
        font_pct: 21.0 / fh_px,
        fill: wrap_blurb_color.clone(),
        font_family: wrap_serif.clone(),
        font_style: "normal".into(),
        ..Default::default()
    });
    let wrap_badge = saved_wrap.as_ref().and_then(|w| w.badge.clone()).map(|e| with_text(e, &badge)).unwrap_or(Element {
        text: badge.clone(),
        x_pct: 0.5,
        y_pct: 0.085,
        w_pct: w_frac,
        font_pct: 13.0 / fh_px,
        fill: badge_color.clone(),
        font_family: "Montserrat".into(),
        font_style: "normal".into(),
        ..Default::default()
    });
    let wrap_author = saved_wrap.as_ref().and_then(|w| w.author.clone()).map(|e| with_text(e, &author_el.text.clone())).unwrap_or(Element {
        text: author_el.text.clone(),
        x_pct: 0.5,
        y_pct: 0.94,
        w_pct: w_frac,
        font_pct: 13.0 / fh_px,
        fill: author_el.fill.clone(),
        font_family: "Montserrat".into(),
        font_style: "normal".into(),
        ..Default::default()
    });

    let bgcolor = d.bgcolor;

    let protected = is_protected(&book.slug);
    let rendered = best_rendered_path(repo, book, lang);

    Ok(CoverResponse {
        slug: book.slug.clone(),
        lang: lang.to_string(),
        canvas: Canvas { w: CANVAS_W, h: CANVAS_H },
        bgcolor,
        bg_url: format!("/api/asset/{}/{}/bg", book.slug, lang),
        rendered_url: rendered.map(|_| format!("/api/asset/{}/{}/rendered", book.slug, lang)),
        protected,
        elements: Elements { title: title_el, subtitle: sub_el, author: author_el },
        layout_saved: saved.is_some(),
        badge,
        badge_color,
        accent,
        title_mt,
        author_mt,
        blurb,
        wrap: WrapLayoutResponse { blurb: wrap_blurb, badge: wrap_badge, author: wrap_author },
        wrap_saved: saved_wrap.is_some(),
        // Safe default; the axum handler overrides this from the book's editions.
        edit_mode: "wrap".to_string(),
    })
}

// --------------------------------------------------------------------------
// Save
// --------------------------------------------------------------------------

/// Persist the edited layout into the book's `bookmill.toml`:
///  * `[cover.<lang>.layout]` — full normalized fractions/styles (canonical).
///  * renderer-honored fields so `bookmill covers` reflects the edit today:
///    `[cover].{title_color,sub_color,author_color,bgcolor}`,
///    `[cover].title_size` (from the title font fraction),
///    `[cover.<lang>].{title,sub}` when the text was changed.
///
/// NOTE: bookmill's renderer does not yet read per-element x/y positions, so
/// re-rendered covers reflect color/size/text edits immediately; absolute
/// repositioning is persisted and awaits crate-side layout support (design doc
/// §A.3). Returns the path that was written.
pub fn save_cover(book: &BookSummary, lang: &str, els: &Elements, bgcolor: &str) -> Result<PathBuf> {
    let path = book.dir.join("bookmill.toml");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let mut doc: DocumentMut = text.parse().context("parsing book bookmill.toml")?;

    // ensure [cover] table exists
    if doc.get("cover").and_then(|i| i.as_table()).is_none() {
        doc["cover"] = Item::Table(Table::new());
    }

    // renderer-honored style fields on [cover]
    doc["cover"]["title_color"] = value(els.title.fill.clone());
    doc["cover"]["sub_color"] = value(els.subtitle.fill.clone());
    doc["cover"]["author_color"] = value(els.author.fill.clone());
    doc["cover"]["bgcolor"] = value(bgcolor);
    let title_size = (els.title.font_pct * CANVAS_H).round();
    doc["cover"]["title_size"] = value(title_size);

    // ensure [cover.<lang>] subtable
    {
        let cover = doc["cover"].as_table_mut().unwrap();
        if cover.get(lang).and_then(|i| i.as_table()).is_none() {
            let mut t = Table::new();
            t.set_implicit(false);
            cover.insert(lang, Item::Table(t));
        }
    }

    // per-language text overrides (only if non-empty)
    if !els.title.text.trim().is_empty() {
        doc["cover"][lang]["title"] = value(els.title.text.clone());
    }
    if !els.subtitle.text.trim().is_empty() {
        doc["cover"][lang]["sub"] = value(els.subtitle.text.clone());
    }

    // ONE design for every language: the geometry + style go to the SHARED
    // [cover.layout], and only the text (with its styled runs) goes to
    // [cover.<lang>.layout], which overlays the shared block field-by-field. So
    // moving a block while editing EN moves it on ES too — that is the point —
    // while each language keeps its own words. Hand-edit [cover.<lang>.layout] to
    // add a per-language tweak (e.g. a smaller title because the words are longer).
    let mut layout = Table::new();
    layout.set_implicit(false);
    layout.insert("title", Item::Value(element_inline(&geometry_of(&els.title))));
    layout.insert("subtitle", Item::Value(element_inline(&geometry_of(&els.subtitle))));
    layout.insert("author", Item::Value(element_inline(&geometry_of(&els.author))));
    doc["cover"]["layout"] = Item::Table(layout);

    let mut lang_layout = Table::new();
    lang_layout.set_implicit(false);
    // Only blocks that actually carry text — an empty one would just be noise (the
    // renderer already falls back to [title.<lang>] / [subtitle.<lang>]).
    for (key, el) in [("title", &els.title), ("subtitle", &els.subtitle)] {
        if let Some(v) = text_only_inline(el) {
            lang_layout.insert(key, Item::Value(v));
        }
    }
    doc["cover"][lang]["layout"] = Item::Table(lang_layout);

    std::fs::write(&path, doc.to_string()).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// The editable back-panel text blocks of the paperback wrap. Each is optional so
/// the editor can persist only what it exposes (blurb today; badge/author when
/// dragged). Mirrors [`Elements`] but for the wrap's BACK panel; coordinates are
/// fractions of the back panel (see `CoverWrapLayout` in the main crate).
#[derive(Deserialize, Default)]
pub struct WrapElements {
    #[serde(default)]
    pub blurb: Option<Element>,
    #[serde(default)]
    pub badge: Option<Element>,
    #[serde(default)]
    pub author: Option<Element>,
}

impl WrapElements {
    fn is_empty(&self) -> bool {
        self.blurb.is_none() && self.badge.is_none() && self.author.is_none()
    }
}

/// Persist the paperback-wrap back-panel layout into `[cover.<lang>.wrap]` in the
/// book's `bookmill.toml`, preserving comments/formatting. Mirrors `save_cover`'s
/// `[cover.<lang>.layout]` writer: one inline sub-table per present block
/// (`blurb`/`badge`/`author`) with `{ xPct, yPct, wPct, fontPct, fill, fontFamily,
/// fontStyle, text }`. The wrap renderer (`cover_svg::wrap_svg`) reads it to place
/// the back panel absolutely; absent blocks fall back to the flex defaults.
/// Returns the path that was written. A no-op (all-empty) returns without writing.
pub fn save_wrap_layout(book: &BookSummary, lang: &str, els: &WrapElements) -> Result<PathBuf> {
    let path = book.dir.join("bookmill.toml");
    if els.is_empty() {
        return Ok(path);
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let mut doc: DocumentMut = text.parse().context("parsing book bookmill.toml")?;

    // ensure [cover] then [cover.<lang>] exist
    if doc.get("cover").and_then(|i| i.as_table()).is_none() {
        doc["cover"] = Item::Table(Table::new());
    }
    {
        let cover = doc["cover"].as_table_mut().unwrap();
        if cover.get(lang).and_then(|i| i.as_table()).is_none() {
            let mut t = Table::new();
            t.set_implicit(false);
            cover.insert(lang, Item::Table(t));
        }
    }

    // Shared [cover.wrap] — geometry + style only, so the back cover is one design
    // in every language. The blurb/badge text resolves per-language on its own
    // ([cover.<lang>].blurb and the localized series badge), so nothing text-shaped
    // needs to be duplicated here. Present blocks merge over any prior saved layout,
    // so dragging only the blurb doesn't drop a previously-placed badge/author.
    if doc["cover"].get("wrap").and_then(|i| i.as_table()).is_none() {
        let mut t = Table::new();
        t.set_implicit(false);
        doc["cover"]["wrap"] = Item::Table(t);
    }
    if let Some(e) = &els.blurb {
        doc["cover"]["wrap"]["blurb"] = Item::Value(element_inline(&geometry_of(e)));
    }
    if let Some(e) = &els.badge {
        doc["cover"]["wrap"]["badge"] = Item::Value(element_inline(&geometry_of(e)));
    }
    if let Some(e) = &els.author {
        doc["cover"]["wrap"]["author"] = Item::Value(element_inline(&geometry_of(e)));
    }

    std::fs::write(&path, doc.to_string()).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Persist the paperback-wrap back-cover blurb into `[cover.<lang>].blurb` in the
/// book's `bookmill.toml`, preserving comments/formatting. Written by the cover
/// editor's "Paperback wrap" mode; the wrap renderer (`cover_tmpl::wrap_html`)
/// reads it for the back panel. Returns the path that was written.
pub fn save_blurb(book: &BookSummary, lang: &str, blurb: &str) -> Result<PathBuf> {
    let path = book.dir.join("bookmill.toml");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let mut doc: DocumentMut = text.parse().context("parsing book bookmill.toml")?;

    // ensure [cover] then [cover.<lang>] exist
    if doc.get("cover").and_then(|i| i.as_table()).is_none() {
        doc["cover"] = Item::Table(Table::new());
    }
    {
        let cover = doc["cover"].as_table_mut().unwrap();
        if cover.get(lang).and_then(|i| i.as_table()).is_none() {
            let mut t = Table::new();
            t.set_implicit(false);
            cover.insert(lang, Item::Table(t));
        }
    }
    doc["cover"][lang]["blurb"] = value(blurb);

    std::fs::write(&path, doc.to_string()).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Absolute path of the square audiobook cover for a book/lang. Lives alongside
/// the other cover assets (`libros/<slug>/cover/audiobook-<lang>.png`), derived by
/// the web editor from the front/wrap via a center-square crop.
pub fn audiobook_cover_path(book: &BookSummary, lang: &str) -> PathBuf {
    book.dir.join("cover").join(format!("audiobook-{lang}.png"))
}

fn element_inline(e: &Element) -> Value {
    let mut t = toml_edit::InlineTable::new();
    t.insert("xPct", round4(e.x_pct).into());
    t.insert("yPct", round4(e.y_pct).into());
    t.insert("wPct", round4(e.w_pct).into());
    t.insert("fontPct", round6(e.font_pct).into());
    t.insert("fill", e.fill.clone().into());
    t.insert("fontFamily", e.font_family.clone().into());
    t.insert("fontStyle", e.font_style.clone().into());
    // Styled runs win over the flat string; a block with no per-run styling keeps
    // writing `text = "…"` so existing configs stay readable and diffable. An empty
    // text is omitted entirely — that's the shared [cover.layout], whose text comes
    // from the per-language block.
    match &e.runs {
        Some(rs) if rs.iter().any(|r| r.styled()) => {
            t.insert("text", runs_value(rs));
        }
        _ if !e.text.is_empty() => {
            t.insert("text", e.text.clone().into());
        }
        _ => {}
    };
    // Optional formatting — only written when set, so untouched blocks stay terse.
    if let Some(v) = &e.align {
        t.insert("align", v.clone().into());
    }
    if let Some(v) = e.line_height {
        t.insert("lineHeight", round4(v).into());
    }
    if let Some(v) = e.letter_spacing {
        t.insert("letterSpacing", round4(v).into());
    }
    if let Some(v) = &e.text_transform {
        t.insert("textTransform", v.clone().into());
    }
    if let Some(v) = &e.stroke {
        t.insert("stroke", v.clone().into());
    }
    if let Some(v) = e.shadow {
        t.insert("shadow", v.into());
    }
    if let Some(v) = e.opacity {
        t.insert("opacity", round4(v).into());
    }
    Value::InlineTable(t)
}

/// The saved front layout, resolved the way the RENDERER resolves it: the shared
/// `[cover.layout]` is the base and `[cover.<lang>.layout]` overlays it field by
/// field. The editor must do the same merge or it would read a text-only language
/// block, see no geometry, fall back to the flex defaults — and then save those
/// defaults over the shared design.
fn read_saved_layout(doc: &DocumentMut, lang: &str) -> Option<Elements> {
    let cover = doc.get("cover")?;
    let shared = cover.get("layout");
    let langed = cover.get(lang).and_then(|l| l.get("layout"));
    if shared.is_none() && langed.is_none() {
        return None;
    }
    let get = |key: &str| -> Option<Element> {
        merge_el(
            shared.and_then(|l| l.get(key)).and_then(read_element_partial),
            langed.and_then(|l| l.get(key)).and_then(read_element_partial),
        )
    };
    Some(Elements {
        title: get("title")?,
        subtitle: get("subtitle")?,
        author: get("author")?,
    })
}

/// Overlay a per-language block onto the shared one. Only a block that ends up
/// with real geometry (x/y/size) is usable by the editor; anything less means the
/// book has no saved layout for that element and the flex default should seed it.
fn merge_el(base: Option<PartialElement>, over: Option<PartialElement>) -> Option<Element> {
    let (b, o) = (base.unwrap_or_default(), over.unwrap_or_default());
    let pick = |a: Option<f64>, c: Option<f64>| c.or(a);
    let picks = |a: Option<String>, c: Option<String>| c.or(a);
    Some(Element {
        x_pct: pick(b.x_pct, o.x_pct)?,
        y_pct: pick(b.y_pct, o.y_pct)?,
        w_pct: pick(b.w_pct, o.w_pct).unwrap_or(0.8),
        font_pct: pick(b.font_pct, o.font_pct)?,
        fill: picks(b.fill, o.fill).unwrap_or_else(|| "#FFFFFF".into()),
        font_family: picks(b.font_family, o.font_family).unwrap_or_else(|| "Playfair Display".into()),
        font_style: picks(b.font_style, o.font_style).unwrap_or_else(|| "normal".into()),
        text: o.text.clone().or(b.text).unwrap_or_default(),
        runs: o.runs.or(b.runs),
        align: picks(b.align, o.align),
        line_height: pick(b.line_height, o.line_height),
        letter_spacing: pick(b.letter_spacing, o.letter_spacing),
        text_transform: picks(b.text_transform, o.text_transform),
        stroke: picks(b.stroke, o.stroke),
        shadow: o.shadow.or(b.shadow),
        opacity: pick(b.opacity, o.opacity),
    })
}

/// Every field optional — a `[cover.<lang>.layout]` block legitimately carries only
/// `text`, and the shared block carries only geometry.
#[derive(Default, Clone)]
struct PartialElement {
    x_pct: Option<f64>,
    y_pct: Option<f64>,
    w_pct: Option<f64>,
    font_pct: Option<f64>,
    fill: Option<String>,
    font_family: Option<String>,
    font_style: Option<String>,
    text: Option<String>,
    runs: Option<Vec<TextRun>>,
    align: Option<String>,
    line_height: Option<f64>,
    letter_spacing: Option<f64>,
    text_transform: Option<String>,
    stroke: Option<String>,
    shadow: Option<bool>,
    opacity: Option<f64>,
}

fn read_element_partial(item: &Item) -> Option<PartialElement> {
    let t = item.as_inline_table()?;
    let f = |k: &str| t.get(k).and_then(|v| v.as_float());
    let s = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let runs = read_runs(t.get("text"));
    let text = match (&runs, s("text")) {
        (Some(rs), _) => Some(rs.iter().map(|r| r.t.as_str()).collect::<String>()),
        (None, p) => p,
    };
    Some(PartialElement {
        x_pct: f("xPct"),
        y_pct: f("yPct"),
        w_pct: f("wPct"),
        font_pct: f("fontPct"),
        fill: s("fill"),
        font_family: s("fontFamily"),
        font_style: s("fontStyle"),
        text,
        runs,
        align: s("align"),
        line_height: f("lineHeight"),
        letter_spacing: f("letterSpacing"),
        text_transform: s("textTransform"),
        stroke: s("stroke"),
        shadow: t.get("shadow").and_then(|v| v.as_bool()),
        opacity: f("opacity"),
    })
}

/// Saved `[cover.<lang>.wrap]` back-panel blocks, each optional (the editor may
/// have placed only some). None only when the `wrap` table itself is absent.
struct SavedWrap {
    blurb: Option<Element>,
    badge: Option<Element>,
    author: Option<Element>,
}

/// Same shared-over-language merge as [`read_saved_layout`], for the back panel:
/// `[cover.wrap]` is the shared design, `[cover.<lang>.wrap]` overlays it.
fn read_saved_wrap(doc: &DocumentMut, lang: &str) -> Option<SavedWrap> {
    let cover = doc.get("cover")?;
    let shared = cover.get("wrap");
    let langed = cover.get(lang).and_then(|l| l.get("wrap"));
    if shared.is_none() && langed.is_none() {
        return None;
    }
    let get = |key: &str| -> Option<Element> {
        merge_el(
            shared.and_then(|w| w.get(key)).and_then(read_element_partial),
            langed.and_then(|w| w.get(key)).and_then(read_element_partial),
        )
    };
    Some(SavedWrap {
        blurb: get("blurb"),
        badge: get("badge"),
        author: get("author"),
    })
}

fn read_element(item: &Item) -> Option<Element> {
    let t = item.as_inline_table()?;
    let f = |k: &str| t.get(k).and_then(|v| v.as_float());
    let s = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let b = |k: &str| t.get(k).and_then(|v| v.as_bool());
    // `text` is either a plain string or an array of styled runs (per-character
    // styling). Keep both: `runs` drives the render, `text` is the flat string.
    let runs = read_runs(t.get("text"));
    let text = match (&runs, s("text")) {
        (Some(rs), _) => rs.iter().map(|r| r.t.as_str()).collect::<String>(),
        (None, Some(plain)) => plain,
        (None, None) => String::new(),
    };
    Some(Element {
        text,
        runs,
        x_pct: f("xPct")?,
        y_pct: f("yPct")?,
        w_pct: f("wPct").unwrap_or(0.8),
        font_pct: f("fontPct")?,
        fill: s("fill").unwrap_or_else(|| "#FFFFFF".into()),
        font_family: s("fontFamily").unwrap_or_else(|| "Playfair Display".into()),
        font_style: s("fontStyle").unwrap_or_else(|| "normal".into()),
        align: s("align"),
        line_height: f("lineHeight"),
        letter_spacing: f("letterSpacing"),
        text_transform: s("textTransform"),
        stroke: s("stroke"),
        shadow: b("shadow"),
        opacity: f("opacity"),
    })
}

/// Parse `text = [{ t = "…", bold = true, fill = "#D4A937" }, …]` into runs.
/// Returns `None` when `text` is a plain string (or absent).
fn read_runs(item: Option<&toml_edit::Value>) -> Option<Vec<TextRun>> {
    let arr = item?.as_array()?;
    let mut out = Vec::new();
    for v in arr.iter() {
        let t = v.as_inline_table()?;
        out.push(TextRun {
            t: t.get("t").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            bold: t.get("bold").and_then(|v| v.as_bool()),
            italic: t.get("italic").and_then(|v| v.as_bool()),
            fill: t.get("fill").and_then(|v| v.as_str()).map(str::to_string),
            family: t.get("family").and_then(|v| v.as_str()).map(str::to_string),
            size: t.get("size").and_then(|v| v.as_float()),
        });
    }
    (!out.is_empty()).then_some(out)
}

/// The block minus its text — what goes into the language-neutral `[cover.layout]`.
fn geometry_of(e: &Element) -> Element {
    Element { text: String::new(), runs: None, ..e.clone() }
}

/// Only the block's text (+ styled runs) — what goes into `[cover.<lang>.layout]`,
/// overlaying the shared block. Every geometry field is `Option` on the config side,
/// so this really is text-only. `None` when the block has no text at all.
fn text_only_inline(e: &Element) -> Option<Value> {
    let mut t = toml_edit::InlineTable::new();
    match &e.runs {
        Some(rs) if rs.iter().any(|r| r.styled()) => {
            t.insert("text", runs_value(rs));
        }
        _ if !e.text.trim().is_empty() => {
            t.insert("text", e.text.clone().into());
        }
        _ => return None,
    }
    Some(Value::InlineTable(t))
}

/// Serialize styled runs back to a TOML array of inline tables. Only set style
/// keys are written, so an unstyled run stays `{ t = "…" }`.
fn runs_value(runs: &[TextRun]) -> toml_edit::Value {
    let mut arr = toml_edit::Array::new();
    for r in runs {
        let mut it = toml_edit::InlineTable::new();
        it.insert("t", r.t.clone().into());
        if let Some(v) = r.bold {
            it.insert("bold", v.into());
        }
        if let Some(v) = r.italic {
            it.insert("italic", v.into());
        }
        if let Some(v) = &r.fill {
            it.insert("fill", v.clone().into());
        }
        if let Some(v) = &r.family {
            it.insert("family", v.clone().into());
        }
        if let Some(v) = r.size {
            it.insert("size", v.into());
        }
        arr.push(toml_edit::Value::InlineTable(it));
    }
    toml_edit::Value::Array(arr)
}

// --------------------------------------------------------------------------
// Asset path resolution
// --------------------------------------------------------------------------

/// Books whose tracked cover/ assets bookmill will NOT overwrite (mirrors
/// covers.rs::PROTECTED) — their re-render lands at a comparison path. Empty now
/// that the two formerly-protected covers render faithfully via native resvg
/// in place; kept so a book can be re-protected with one edit.
const PROTECTED: &[&str] = &[];

pub fn is_protected(slug: &str) -> bool {
    PROTECTED.contains(&slug)
}

/// Resolve the background image file on disk (cover/<bg>, default bg.jpg).
pub fn bg_path(repo: &Repo, book: &BookSummary, _lang: &str) -> Option<PathBuf> {
    let doc = read_book_doc(book).ok()?;
    let bg = cover_str(&doc, &repo.repo_doc, "bg").unwrap_or_else(|| "bg.jpg".into());
    let p = book.dir.join("cover").join(&bg);
    if p.exists() {
        Some(p)
    } else {
        let fallback = book.dir.join("cover").join("bg.jpg");
        fallback.exists().then_some(fallback)
    }
}

/// Best available rendered cover PNG: comparison path (protected), then build
/// bundle, then in-place front PNG. Returns the newest that exists.
pub fn best_rendered_path(repo: &Repo, book: &BookSummary, lang: &str) -> Option<PathBuf> {
    let slug = &book.slug;
    let odir = repo.root.join("output").join(slug).join(lang);
    let candidates = [
        odir.join(format!("{slug}-{lang}-cover-resvg.png")),
        odir.join(format!("{slug}-{lang}-cover.png")),
        book.dir.join("cover").join(format!("front-{lang}.png")),
    ];
    candidates
        .into_iter()
        .filter(|p| p.exists())
        .max_by_key(|p| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .ok()
        })
}

// --------------------------------------------------------------------------
// toml_edit helpers
// --------------------------------------------------------------------------

fn read_book_doc(book: &BookSummary) -> Result<DocumentMut> {
    let path = book.dir.join("bookmill.toml");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    text.parse().context("parsing book bookmill.toml")
}

fn str_array(item: Option<&Item>) -> Vec<String> {
    item.and_then(|i| i.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// book [cover].<key> over repo [cover].<key> as string. Used for the `bg`
/// filename lookup and the badge/accent/margin style strings the editor seeds;
/// numeric style resolution (title_size, …) goes through the shared resolver (M8).
fn cover_str(book: &DocumentMut, repo: &DocumentMut, key: &str) -> Option<String> {
    book.get("cover")
        .and_then(|c| c.get(key))
        .and_then(|i| i.as_str())
        .or_else(|| repo.get("cover").and_then(|c| c.get(key)).and_then(|i| i.as_str()))
        .map(str::to_string)
}

/// Read a string from a per-language subtable: `[<section>.<lang>].<key>`
/// (e.g. `[cover.es].blurb` or `[listing.en].blurb`). None if absent.
fn lang_str(book: &DocumentMut, section: &str, lang: &str, key: &str) -> Option<String> {
    book.get(section)
        .and_then(|s| s.get(lang))
        .and_then(|l| l.get(key))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn round4(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
}
fn round6(x: f64) -> f64 {
    (x * 1_000_000.0).round() / 1_000_000.0
}
