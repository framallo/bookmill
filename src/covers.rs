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
use std::io::{IsTerminal, Write};
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
    assume_yes: bool,
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
        // A book with no background art is no longer skipped: it renders on the
        // solid `[cover].bgcolor`. That's a visible product decision, so ask
        // before doing it (once per book — the art is shared across languages).
        let cover_dir = dir.join("cover");
        let no_art = langs
            .iter()
            .all(|l| crate::cover_svg::art_missing(&repo.config, book, l, &cover_dir));
        if no_art {
            let color = langs
                .first()
                .map(|l| crate::cover_svg::bgcolor_of(&repo.config, book, l))
                .unwrap_or_else(|| "#000000".to_string());
            if !confirm_solid(&book.slug, &color, assume_yes)? {
                println!("  \u{2014} {} skipped (no cover art)", book.slug);
                skipped += langs.len();
                continue;
            }
            std::fs::create_dir_all(&cover_dir)
                .with_context(|| format!("creating {}", cover_dir.display()))?;
        }
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
    // No `cover/` dir is fine: a book with no art renders on the solid bgcolor
    // (the caller has already asked and created the dir). Only a *file* the
    // config explicitly points at and that is missing is an error.
    std::fs::create_dir_all(&cover_dir)
        .with_context(|| format!("creating {}", cover_dir.display()))?;
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

/// Ask whether to render a colour-only cover for a book that has no art.
///
/// Interactive shells get a y/N prompt; `--yes` and non-interactive shells (CI,
/// pipes) accept automatically so a scripted `bookmill build cover` never hangs.
/// The colour itself comes from `[cover].bgcolor` — config stays the single
/// source of truth, so recolouring means editing the book's `bookmill.toml`.
fn confirm_solid(slug: &str, color: &str, assume_yes: bool) -> Result<bool> {
    println!("  ! {slug}: no cover art (no cover/bg.jpg)");
    println!("    A cover can still be rendered on the solid [cover].bgcolor {color}.");
    if assume_yes || !std::io::stdin().is_terminal() {
        println!("    -> generating (non-interactive; pass --yes explicitly to silence)");
        return Ok(true);
    }
    print!("    Generate a solid-colour cover on {color}? [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        return Ok(false); // EOF
    }
    let ans = line.trim().to_ascii_lowercase();
    if matches!(ans.as_str(), "y" | "yes") {
        Ok(true)
    } else {
        println!("    (to change the colour, set bgcolor under [cover] in the book's bookmill.toml)");
        Ok(false)
    }
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

