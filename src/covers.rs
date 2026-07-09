//! Cover rendering: bookmill builds the cover as SVG from the `[cover]` TOML
//! config (see `cover_svg` / `cover_tmpl::resolve`) and rasterizes it natively
//! with resvg (front PNG) + svg2pdf (wrap PDF) — no Chrome, no HTML. Per book ×
//! language it produces:
//!   libros/<slug>/cover/front-<lang>.png   (eBook front 1600x2560, git-tracked)
//!   libros/<slug>/cover/wrap-<lang>-KDP.pdf (paperback wrap, KDP upload)
//! then copies them into the build bundle as
//!   output/<slug>/<lang>/<slug>-<lang>-cover.png
//!   output/<slug>/<lang>/<slug>-<lang>-cover-wrap-kdp.pdf
//! and emits the eBook cover JPG via the native `image`-crate path in build.rs.

use crate::build::emit_cover_jpg;
use crate::config::BookConfig;
use crate::cover_svg::CoverRenderer;
use crate::discover::Repo;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Books whose tracked `cover/` image assets must NOT be overwritten by the
/// native resvg render: they render to a side-by-side comparison path under
/// `output/` instead of in place. Empty today (every book renders faithfully in
/// place); kept so a future book can be protected again with one edit.
const PROTECTED: &[&str] = &[];

/// `bookmill covers [book] [--lang]` — native resvg render for every (book, lang).
pub fn run(
    repo: &Repo,
    book_slug: Option<String>,
    lang_filter: Option<String>,
    pages_override: Option<u32>,
) -> Result<()> {
    let books = resolve_books(repo, &book_slug)?;
    let renderer = CoverRenderer::new();
    let mut made = 0usize;
    let mut skipped = 0usize;
    for (book, dir) in &books {
        let langs: Vec<String> = match &lang_filter {
            Some(l) if l != "all" => vec![l.clone()],
            _ => book.languages.clone(),
        };
        for lang in &langs {
            match cover_one_resvg(repo, book, dir, lang, &renderer, pages_override) {
                Ok(()) => made += 1,
                Err(e) => {
                    eprintln!("  \u{2717} {} {}: {e:#}", book.slug, lang);
                    skipped += 1;
                }
            }
        }
    }
    println!("\nCovers: {made} rendered, {skipped} skipped");
    if made == 0 && skipped > 0 {
        bail!("no covers rendered");
    }
    Ok(())
}

/// Resolve the spine page count: explicit override, else the built KDP interior.
fn resolve_pages(odir: &Path, slug: &str, lang: &str, pages_override: Option<u32>) -> Result<u32> {
    if let Some(p) = pages_override {
        return Ok(p);
    }
    let kdp_pdf = odir.join(format!("{slug}-{lang}-kdp.pdf"));
    page_count(&kdp_pdf).with_context(|| {
        format!(
            "need the print interior for spine width — build it first:\n      \
             bookmill build {slug} --format print --lang {lang}\n      (missing {})",
            kdp_pdf.display()
        )
    })
}

/// Native resvg path: build SVG from `[cover]` config and rasterize with
/// resvg (front PNG) + svg2pdf (wrap PDF). No Chrome, no HTML.
fn cover_one_resvg(
    repo: &Repo,
    book: &BookConfig,
    dir: &Path,
    lang: &str,
    renderer: &CoverRenderer,
    pages_override: Option<u32>,
) -> Result<()> {
    let slug = &book.slug;
    let cover_dir = dir.join("cover");
    if !cover_dir.exists() {
        bail!("no cover dir at {}", cover_dir.display());
    }
    let odir = repo.root.join("output").join(slug).join(lang);
    std::fs::create_dir_all(&odir)?;
    let pages = resolve_pages(&odir, slug, lang, pages_override)?;

    if PROTECTED.contains(&slug.as_str()) {
        // Render to a side-by-side comparison path; never touch tracked cover/ assets.
        let cmp_png = odir.join(format!("{slug}-{lang}-cover-resvg.png"));
        let cmp_pdf = odir.join(format!("{slug}-{lang}-cover-wrap-resvg.pdf"));
        renderer.render_front_png(&repo.config, book, lang, &cover_dir, &cmp_png)?;
        renderer.render_wrap_pdf(&repo.config, book, lang, &cover_dir, pages, &cmp_pdf)?;
        println!(
            "  \u{2713} {slug} {lang}: {pages}pp  (resvg COMPARISON only — protected, not in place)\n        front: {}\n        wrap:  {}",
            cmp_png.display(),
            cmp_pdf.display()
        );
        return Ok(());
    }

    // Non-protected books: render resvg in place (same paths as the Chrome path).
    let front_png = cover_dir.join(format!("front-{lang}.png"));
    let wrap_pdf = cover_dir.join(format!("wrap-{lang}-KDP.pdf"));
    renderer.render_front_png(&repo.config, book, lang, &cover_dir, &front_png)?;
    renderer.render_wrap_pdf(&repo.config, book, lang, &cover_dir, pages, &wrap_pdf)?;

    let bundle_png = odir.join(format!("{slug}-{lang}-cover.png"));
    let bundle_wrap = odir.join(format!("{slug}-{lang}-cover-wrap-kdp.pdf"));
    std::fs::copy(&front_png, &bundle_png)
        .with_context(|| format!("copy {} -> {}", front_png.display(), bundle_png.display()))?;
    std::fs::copy(&wrap_pdf, &bundle_wrap)
        .with_context(|| format!("copy {} -> {}", wrap_pdf.display(), bundle_wrap.display()))?;
    emit_cover_jpg(&front_png, &odir.join(format!("{slug}-{lang}-cover.jpg")))?;

    println!("  \u{2713} {slug} {lang}: {pages}pp  (resvg: front PNG + wrap PDF + JPG)");
    Ok(())
}

/// Page count of a PDF (native, via `lopdf`).
fn page_count(pdf: &Path) -> Result<u32> {
    if !pdf.exists() {
        bail!("not found: {}", pdf.display());
    }
    crate::pdfmeta::page_count(pdf)
        .with_context(|| format!("could not read page count from {}", pdf.display()))
}

fn resolve_books(repo: &Repo, slug: &Option<String>) -> Result<Vec<(BookConfig, PathBuf)>> {
    match slug {
        Some(s) => Ok(vec![repo.find_book(s)?]),
        None => {
            let mut v = Vec::new();
            for d in repo.book_dirs()? {
                v.push(repo.load_book_at(&d)?);
            }
            Ok(v)
        }
    }
}

