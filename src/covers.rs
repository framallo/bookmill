//! Cover rendering: bookmill emits the cover HTML itself from the `[cover]` TOML
//! config + the bundled templates (`templates/cover/{front,wrap}.html.tmpl`,
//! see `cover_tmpl`), then rasterizes via headless Chrome (mirroring the old
//! `scripts/render-covers.sh`) to produce, per book × language:
//!   libros/<slug>/cover/front-<lang>.png   (eBook front 1600x2560, git-tracked)
//!   libros/<slug>/cover/wrap-<lang>-KDP.pdf (paperback wrap, KDP upload)
//! then copies them into the build bundle as
//!   output/<slug>/<lang>/<slug>-<lang>-cover.png
//!   output/<slug>/<lang>/<slug>-<lang>-cover-wrap-kdp.pdf
//! and emits the eBook cover JPG via the native `image`-crate path in build.rs.
//!
//! The native HTML matches the legacy `make-covers.py` output byte-for-byte, so
//! covers stay identical. `make-covers.py` is kept for coexistence; native
//! SVG→resvg rasterization replaces headless Chrome in a later phase.

use crate::build::emit_cover_jpg;
use crate::config::BookConfig;
use crate::cover_svg::CoverRenderer;
use crate::cover_tmpl;
use crate::discover::Repo;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Cover rasterization engine. `resvg` (pure Rust, default) needs no Chrome;
/// `chrome` is the legacy HTML/headless-Chrome path, kept as a fallback.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Resvg,
    Chrome,
}

impl Engine {
    pub fn parse(s: Option<&str>) -> Result<Engine> {
        match s.map(|s| s.to_ascii_lowercase()).as_deref() {
            None | Some("resvg") => Ok(Engine::Resvg),
            Some("chrome") => Ok(Engine::Chrome),
            Some(other) => bail!("unknown --engine {other:?} (use resvg|chrome)"),
        }
    }
}

/// Submitted / separately-managed books whose tracked `cover/` image assets must
/// NOT be overwritten. With the resvg engine these render to a side-by-side
/// comparison path under `output/` instead of in place.
const PROTECTED: &[&str] = &[
    "la-riqueza-de-la-isla",                  // SUBMITTED to KDP
    "no-hay-plata-en-la-isla-de-las-ratas",   // new cover wired separately
];

fn chrome_bin() -> String {
    std::env::var("CHROME")
        .unwrap_or_else(|_| "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into())
}

/// `bookmill covers [book] [--lang] [--engine resvg|chrome] [--html-only]`
pub fn run(
    repo: &Repo,
    book_slug: Option<String>,
    lang_filter: Option<String>,
    html_only: bool,
    pages_override: Option<u32>,
    engine: Engine,
) -> Result<()> {
    let books = resolve_books(repo, &book_slug)?;
    let chrome = chrome_bin();
    if engine == Engine::Chrome && !html_only && !Path::new(&chrome).exists() {
        bail!("headless Chrome not found at {chrome:?} (set $CHROME)");
    }
    let renderer = (engine == Engine::Resvg).then(CoverRenderer::new);
    let mut made = 0usize;
    let mut skipped = 0usize;
    for (book, dir) in &books {
        let langs: Vec<String> = match &lang_filter {
            Some(l) if l != "all" => vec![l.clone()],
            _ => book.languages.clone(),
        };
        for lang in &langs {
            let res = match engine {
                Engine::Chrome => cover_one_chrome(repo, book, dir, lang, &chrome, html_only, pages_override),
                Engine::Resvg => {
                    cover_one_resvg(repo, book, dir, lang, renderer.as_ref().unwrap(), pages_override)
                }
            };
            match res {
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
    emit_cover_jpg(&front_png, &odir.join(format!("{slug}-{lang}-cover.jpg")));

    println!("  \u{2713} {slug} {lang}: {pages}pp  (resvg: front PNG + wrap PDF + JPG)");
    Ok(())
}

fn cover_one_chrome(
    repo: &Repo,
    book: &BookConfig,
    dir: &Path,
    lang: &str,
    chrome: &str,
    html_only: bool,
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

    let front_html = cover_dir.join(format!("front-{lang}.html"));
    let wrap_html = cover_dir.join(format!("wrap-{lang}.html"));
    let front_png = cover_dir.join(format!("front-{lang}.png"));
    let wrap_pdf = cover_dir.join(format!("wrap-{lang}-KDP.pdf"));

    // 1) emit cover HTML from [cover] config + bundled templates (native; matches
    //    the legacy make-covers.py output byte-for-byte).
    std::fs::write(&front_html, cover_tmpl::front_html(&repo.config, book, lang))
        .with_context(|| format!("writing {}", front_html.display()))?;
    std::fs::write(&wrap_html, cover_tmpl::wrap_html(&repo.config, book, lang, pages))
        .with_context(|| format!("writing {}", wrap_html.display()))?;

    if html_only {
        println!("  \u{2713} {slug} {lang}: {pages}pp  (HTML only)");
        return Ok(());
    }

    // 2) render eBook front PNG (1600x2560) — mirrors render-covers.sh render_front.
    let st = Command::new(chrome)
        .args([
            "--headless",
            "--disable-gpu",
            "--hide-scrollbars",
            "--force-device-scale-factor=1",
            "--window-size=1600,2560",
            "--virtual-time-budget=20000",
        ])
        .arg(format!("--screenshot={}", front_png.display()))
        .arg(format!("file://{}", front_html.display()))
        .status()
        .context("chrome --screenshot (front)")?;
    if !st.success() {
        bail!("chrome front render failed");
    }

    // 3) render paperback wrap PDF — mirrors render-covers.sh render_wrap.
    let st = Command::new(chrome)
        .args([
            "--headless",
            "--disable-gpu",
            "--no-pdf-header-footer",
            "--virtual-time-budget=20000",
        ])
        .arg(format!("--print-to-pdf={}", wrap_pdf.display()))
        .arg(format!("file://{}", wrap_html.display()))
        .status()
        .context("chrome --print-to-pdf (wrap)")?;
    if !st.success() {
        bail!("chrome wrap render failed");
    }

    // 4) copy into the build bundle (lang-carrying names) + emit eBook cover JPG.
    let bundle_png = odir.join(format!("{slug}-{lang}-cover.png"));
    let bundle_wrap = odir.join(format!("{slug}-{lang}-cover-wrap-kdp.pdf"));
    std::fs::copy(&front_png, &bundle_png)
        .with_context(|| format!("copy {} -> {}", front_png.display(), bundle_png.display()))?;
    std::fs::copy(&wrap_pdf, &bundle_wrap)
        .with_context(|| format!("copy {} -> {}", wrap_pdf.display(), bundle_wrap.display()))?;
    emit_cover_jpg(&front_png, &odir.join(format!("{slug}-{lang}-cover.jpg")));

    println!("  \u{2713} {slug} {lang}: {pages}pp  (chrome: front PNG + wrap PDF + JPG)");
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

