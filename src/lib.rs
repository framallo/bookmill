//! bookmill library surface.
//!
//! Deliberately tiny: it re-exports the layered config model and the cover
//! resolver so the **web cover editor** resolves its initial display values with
//! the exact same code path the **build** uses. Before this, `web/src/cover.rs`
//! hand-rolled a partial `toml_edit` resolution (hardcoded author/title-size,
//! book-over-repo only) that could drift from what `build cover` actually
//! rendered (M8). Both the folded `bookmill web` server (which references this as
//! `bookmill::…` from its own package) and the standalone `bookmill-web` crate
//! (which depends on it by path) now call [`resolve_editor_cover`].
//!
//! `dead_code` is allowed crate-wide here because the library only needs a slice
//! of the config/cover_tmpl API; the rest is exercised by the binary, whose own
//! compilation keeps the full warning set honest.
#![allow(dead_code)]

pub mod config;
pub mod cover_svg;
pub mod cover_tmpl;

use anyhow::Result;
use std::path::Path;

/// The exact SVG the build rasterizes for a book's cover — front (1600x2560) or
/// the full paperback wrap (back + spine + front). The web editor renders this
/// directly in the browser so its canvas *is* the authoritative output (no
/// re-implementation, no drift). `pages` drives the wrap spine width; ignored for
/// the front.
pub fn editor_cover_svg(
    repo_root: &Path,
    book_dir: &Path,
    lang: &str,
    wrap: bool,
    pages: u32,
) -> Result<String> {
    let repo = config::load_repo(&repo_root.join("bookmill.toml"))?;
    let book = config::load_book(&book_dir.join("bookmill.toml"))?;
    let cover_dir = book_dir.join("cover");
    let r = cover_svg::CoverRenderer::new();
    if wrap {
        r.wrap_svg_string(&repo, &book, lang, &cover_dir, pages)
    } else {
        r.front_svg_string(&repo, &book, lang, &cover_dir)
    }
}

/// The subset of resolved cover values the web editor needs to seed its canvas
/// (text, colors, fonts, title size) — produced by the same layered resolver
/// (`cover_tmpl::resolve`) the build uses.
pub struct EditorCover {
    pub author: String,
    pub title: String,
    pub sub: String,
    pub title_color: String,
    pub sub_color: String,
    pub author_color: String,
    pub bgcolor: String,
    pub serif: String,
    pub sub_font: String,
    /// "italic" or "normal" (matches how the build renders the subtitle).
    pub sub_style: String,
    pub title_size: f64,
    /// Back-cover blurb text (for the wrap back panel), resolved the same way the
    /// build does (`[cover.<lang>].blurb` → `[listing.<lang>].blurb` → `[cover].blurb`).
    pub blurb: String,
    /// Back-panel geometry so the wrap editor can seed its handles in the exact
    /// fraction space the renderer uses. Widths/heights are in inches; the
    /// back panel spans `bleed + trim_w` wide by `trim_h + 2·bleed` tall.
    pub trim_w: f64,
    pub trim_h: f64,
    pub bleed: f64,
    pub blurb_color: String,
    pub badge_color: String,
}

/// Resolve the editor's cover defaults for `(repo_root, book_dir, lang)` through
/// the build's own config model, so the editor never diverges from `build cover`.
pub fn resolve_editor_cover(repo_root: &Path, book_dir: &Path, lang: &str) -> Result<EditorCover> {
    let repo = config::load_repo(&repo_root.join("bookmill.toml"))?;
    let book = config::load_book(&book_dir.join("bookmill.toml"))?;
    let r = cover_tmpl::resolve(&repo, &book, lang);
    Ok(EditorCover {
        author: r.author,
        title: r.title,
        sub: r.sub,
        title_color: r.title_color,
        sub_color: r.sub_color,
        author_color: r.author_color,
        bgcolor: r.bgcolor,
        serif: r.serif,
        sub_font: r.sub_font,
        sub_style: r.sub_italic,
        title_size: r.title_size,
        blurb: r.blurb,
        trim_w: r.trim_w,
        trim_h: r.trim_h,
        bleed: r.bleed,
        blurb_color: r.blurb_color,
        badge_color: r.badge_color,
    })
}
