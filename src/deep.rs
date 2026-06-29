//! Deep validation (`bookmill validate --deep`).
//!
//! On top of the fast config/listing/house-rule checks (`config::validate_book`),
//! this verifies the *built artifacts* per book × edition × language:
//!   1. EPUB   — build (or reuse) the EPUB and run **epubcheck** on it.
//!   2. PDF    — build (or reuse) the print/digital PDF and verify, via `pdfinfo`,
//!               that the page size equals the edition's expected trim+bleed
//!               (e.g. kdp-paperback picture book = 6.125×9.25in = 441×666pt;
//!               6×9 text = 432×648pt).
//!   3. Cover  — front PNG ≥ 1600×2560, wrap PDF page size sane (else low-res warn).
//!
//! Reuses already-built outputs under `output/` when present (safe: gitignored,
//! user-backed-up); only builds the missing ones. Errors set a non-zero exit;
//! warnings (`!`) do not. The default `bookmill validate` (no `--deep`) stays
//! fast and config-only.

use crate::build::{self, Job, Out};
use crate::config::{self, BookConfig};
use crate::discover::Repo;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Minimum eBook front-cover resolution (KDP recommends ≥ 1600×2560).
const FRONT_MIN_W: u32 = 1600;
const FRONT_MIN_H: u32 = 2560;
/// PDF page-size tolerance, in points (rounding in the geometry → LaTeX path).
const PT_TOL: f64 = 1.5;

/// Tally of check outcomes across the run.
struct Report {
    pass: u32,
    warn: u32,
    err: u32,
}

impl Report {
    fn new() -> Self {
        Self { pass: 0, warn: 0, err: 0 }
    }
    fn ok(&mut self, m: String) {
        self.pass += 1;
        println!("  \u{2713} {m}");
    }
    fn warn(&mut self, m: String) {
        self.warn += 1;
        println!("  ! {m}");
    }
    fn bad(&mut self, m: String) {
        self.err += 1;
        println!("  \u{2717} {m}");
    }
}

/// `bookmill validate [book] --deep`
pub fn run(repo: &Repo, book: Option<String>) -> Result<()> {
    let dirs = match &book {
        Some(s) => vec![repo.find_book(s)?.1],
        None => repo.book_dirs()?,
    };
    let mut rep = Report::new();
    for dir in dirs {
        let (b, _) = repo.load_book_at(&dir)?;
        println!("\n=== {} [{}] ===", b.slug, b.languages.join(","));

        // 1) config / KDP / house rules (same as the fast path).
        config_checks(&b, &mut rep);

        // 2) deep artifact checks per edition output (EPUB → epubcheck;
        //    PDF → page-geometry). plan_editions yields exactly the publishable
        //    outputs for the book's editions, with resolved geometry per job.
        let jobs = build::plan_editions(repo, &Some(b.slug.clone()), &None, &None)?;
        for job in &jobs {
            match job.out {
                Out::RetailEpub | Out::KdpEpub => epub_check(repo, job, &mut rep),
                Out::RetailPdf | Out::KdpPdf => pdf_geometry_check(repo, job, &mut rep),
            }
        }

        // 3) cover resolution per language.
        for lang in &b.languages {
            cover_checks(&b.slug, &dir, lang, &mut rep);
        }
    }

    println!(
        "\n{} ok · {} warning(s) · {} error(s)",
        rep.pass, rep.warn, rep.err
    );
    if rep.err > 0 {
        anyhow::bail!("{} deep-validation error(s)", rep.err);
    }
    Ok(())
}

fn config_checks(b: &BookConfig, rep: &mut Report) {
    let issues = config::validate_book(b);
    if issues.is_empty() {
        rep.ok(format!("config/KDP rules ({} lang)", b.languages.len()));
        return;
    }
    for i in issues {
        if i.level == "error" {
            rep.bad(format!("config: {}", i.msg));
        } else {
            rep.warn(format!("config: {}", i.msg));
        }
    }
}

// ---------- EPUB (epubcheck) ----------

fn epub_check(repo: &Repo, job: &Job, rep: &mut Report) {
    let path = build::job_output_path(repo, job);
    let label = format!("{} {} · {}", job.slug, job.lang, job.out.name());
    if !path.exists() {
        println!("    (building {} …)", path.display());
        if let Err(e) = build::build_job(repo, job) {
            rep.bad(format!("EPUB {label}: build failed: {e:#}"));
            return;
        }
    }
    match run_epubcheck(&path) {
        Ok((warns, true)) if warns == 0 => rep.ok(format!("EPUB {label}: epubcheck clean")),
        Ok((warns, true)) => rep.warn(format!("EPUB {label}: epubcheck OK, {warns} warning(s)")),
        Ok((_, false)) => rep.bad(format!("EPUB {label}: epubcheck reported errors (see above)")),
        Err(e) => rep.warn(format!("EPUB {label}: epubcheck not run: {e:#}")),
    }
}

/// Run epubcheck on `epub`, echoing ERROR/FATAL/WARNING lines. Returns
/// (warning_count, success). `epubcheck` is the same binary the legacy Makefile
/// used (`make validate_*` → `@epubcheck <file>`); found on `$PATH`.
fn run_epubcheck(epub: &Path) -> Result<(usize, bool)> {
    let out = Command::new("epubcheck")
        .arg(epub)
        .output()
        .context("running epubcheck (is it on $PATH?)")?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let mut warns = 0usize;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("ERROR") || l.starts_with("FATAL") {
            println!("      {l}");
        } else if l.starts_with("WARNING") {
            warns += 1;
            println!("      {l}");
        }
    }
    Ok((warns, out.status.success()))
}

// ---------- PDF geometry ----------

fn pdf_geometry_check(repo: &Repo, job: &Job, rep: &mut Report) {
    let path = build::job_output_path(repo, job);
    let ed = job.edition.clone().unwrap_or_else(|| job.target.clone());
    let label = format!("{} {} · {} · {}", job.slug, job.lang, ed, job.out.name());
    if !path.exists() {
        println!("    (building {} …)", path.display());
        if let Err(e) = build::build_job(repo, job) {
            rep.bad(format!("PDF {label}: build failed: {e:#}"));
            return;
        }
    }
    let g = match job.geometry {
        Some(g) => g,
        None => return, // EPUB jobs never reach here; defensively skip.
    };
    let exp_w = g.pw * 72.0;
    let exp_h = g.ph * 72.0;
    match pdf_page_size(&path) {
        Ok((w, h)) => {
            if (w - exp_w).abs() <= PT_TOL && (h - exp_h).abs() <= PT_TOL {
                rep.ok(format!(
                    "PDF {label}: {w:.0}×{h:.0}pt (= {:.3}×{:.3}in)",
                    g.pw, g.ph
                ));
            } else {
                rep.bad(format!(
                    "PDF {label}: page {w:.1}×{h:.1}pt ≠ expected {exp_w:.0}×{exp_h:.0}pt ({:.3}×{:.3}in)",
                    g.pw, g.ph
                ));
            }
        }
        Err(e) => rep.warn(format!("PDF {label}: pdfinfo failed: {e:#}")),
    }
}

/// First-page size in points, via `pdfinfo` (the same tool covers.rs uses for
/// page counts). Parses the `Page size:  W x H pts [...]` line.
fn pdf_page_size(pdf: &Path) -> Result<(f64, f64)> {
    let out = Command::new("pdfinfo")
        .arg(pdf)
        .output()
        .context("running pdfinfo")?;
    if !out.status.success() {
        anyhow::bail!("pdfinfo failed on {}", pdf.display());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Page size:") {
            let nums: Vec<f64> = rest
                .split_whitespace()
                .filter_map(|t| t.trim_end_matches("pts").parse::<f64>().ok())
                .collect();
            if nums.len() >= 2 {
                return Ok((nums[0], nums[1]));
            }
        }
    }
    anyhow::bail!("could not parse 'Page size' from pdfinfo on {}", pdf.display())
}

// ---------- Covers ----------

fn cover_checks(slug: &str, dir: &Path, lang: &str, rep: &mut Report) {
    let front = dir.join("cover").join(format!("front-{lang}.png"));
    let wrap = dir.join("cover").join(format!("wrap-{lang}-KDP.pdf"));
    if !front.exists() && !wrap.exists() {
        rep.warn(format!("cover {slug} {lang}: no cover assets (skipped)"));
        return;
    }

    // Front eBook PNG resolution (header read only — cheap).
    if front.exists() {
        match image::image_dimensions(&front) {
            Ok((w, h)) => {
                if w >= FRONT_MIN_W && h >= FRONT_MIN_H {
                    rep.ok(format!(
                        "cover {slug} {lang}: front {w}×{h} (≥{FRONT_MIN_W}×{FRONT_MIN_H})"
                    ));
                } else {
                    rep.warn(format!(
                        "cover {slug} {lang}: front {w}×{h} below {FRONT_MIN_W}×{FRONT_MIN_H} (low-res)"
                    ));
                }
            }
            Err(e) => rep.warn(format!("cover {slug} {lang}: can't read front PNG: {e}")),
        }
    } else {
        rep.warn(format!("cover {slug} {lang}: no front-{lang}.png"));
    }

    // Wrap PDF sanity: a paperback wrap is landscape (back+spine+front) and well
    // larger than a single trim page.
    if wrap.exists() {
        match pdf_page_size(&wrap) {
            Ok((w, h)) => {
                if w > h && w > 400.0 && h > 400.0 {
                    rep.ok(format!("cover {slug} {lang}: wrap {w:.0}×{h:.0}pt"));
                } else {
                    rep.warn(format!(
                        "cover {slug} {lang}: wrap {w:.0}×{h:.0}pt looks off (expect landscape ≥400pt)"
                    ));
                }
            }
            Err(e) => rep.warn(format!("cover {slug} {lang}: wrap pdfinfo: {e:#}")),
        }
    }

    // TODO(image-dpi): auditing effective image DPI at print size would require
    // parsing each PDF image's placed dimensions vs. its pixel size — not cheap
    // or reliable via pdfinfo/pdfimages alone. Left out deliberately rather than
    // shipping a flaky check; revisit with a native Typst/PDF page model.
}
