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
#[derive(Serialize, Deserialize, Clone)]
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

    // Existing saved layout, if any, wins over computed defaults.
    let saved = read_saved_layout(&book_doc, lang);

    let title_el = saved.as_ref().map(|e| e.title.clone()).unwrap_or(Element {
        text: d.title,
        x_pct: 0.5,
        y_pct: 0.62,
        w_pct: 0.80,
        font_pct: d.title_size / CANVAS_H,
        fill: d.title_color,
        font_family: d.serif,
        font_style: "bold".into(),
    });
    let sub_el = saved.as_ref().map(|e| e.subtitle.clone()).unwrap_or(Element {
        text: d.sub,
        x_pct: 0.5,
        y_pct: 0.76,
        w_pct: 0.85,
        font_pct: 48.0 / CANVAS_H,
        fill: d.sub_color,
        font_family: d.sub_font,
        font_style: d.sub_style,
    });
    let author_el = saved.as_ref().map(|e| e.author.clone()).unwrap_or(Element {
        text: d.author,
        x_pct: 0.5,
        y_pct: 0.93,
        w_pct: 0.80,
        font_pct: 42.0 / CANVAS_H,
        fill: d.author_color,
        font_family: "Montserrat".into(),
        font_style: "normal".into(),
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

    // [cover.<lang>.layout] — canonical normalized layout
    let mut layout = Table::new();
    layout.set_implicit(false);
    layout.insert("title", Item::Value(element_inline(&els.title)));
    layout.insert("subtitle", Item::Value(element_inline(&els.subtitle)));
    layout.insert("author", Item::Value(element_inline(&els.author)));
    doc["cover"][lang]["layout"] = Item::Table(layout);

    std::fs::write(&path, doc.to_string()).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
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
    t.insert("text", e.text.clone().into());
    Value::InlineTable(t)
}

fn read_saved_layout(doc: &DocumentMut, lang: &str) -> Option<Elements> {
    let layout = doc.get("cover")?.get(lang)?.get("layout")?;
    Some(Elements {
        title: read_element(layout.get("title")?)?,
        subtitle: read_element(layout.get("subtitle")?)?,
        author: read_element(layout.get("author")?)?,
    })
}

fn read_element(item: &Item) -> Option<Element> {
    let t = item.as_inline_table()?;
    let f = |k: &str| t.get(k).and_then(|v| v.as_float());
    let s = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_string);
    Some(Element {
        text: s("text").unwrap_or_default(),
        x_pct: f("xPct")?,
        y_pct: f("yPct")?,
        w_pct: f("wPct").unwrap_or(0.8),
        font_pct: f("fontPct")?,
        fill: s("fill").unwrap_or_else(|| "#FFFFFF".into()),
        font_family: s("fontFamily").unwrap_or_else(|| "Playfair Display".into()),
        font_style: s("fontStyle").unwrap_or_else(|| "normal".into()),
    })
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

fn round4(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
}
fn round6(x: f64) -> f64 {
    (x * 1_000_000.0).round() / 1_000_000.0
}
