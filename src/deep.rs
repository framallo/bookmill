//! Deep validation (`bookmill validate --deep`).
//!
//! On top of the fast config/listing/house-rule checks (`config::validate_book`),
//! this verifies the *built artifacts* per book × edition × language:
//!   1. EPUB   — build (or reuse) the EPUB and run **epubcheck** on it.
//!   2. PDF    — build (or reuse) the print/digital PDF and verify, natively via
//!               `lopdf` (no poppler), that the page size equals the edition's
//!               expected trim+bleed (e.g. kdp-paperback picture book =
//!               6.125×9.25in = 441×666pt; 6×9 text = 432×648pt). For the KDP
//!               print PDF it also audits interior image DPI natively
//!               (`pdfmeta::placed_images` interprets the content stream's CTM to
//!               get each image's placed effective resolution at print size — the
//!               same number poppler's `pdfimages -list` reported), warning on any
//!               image below ~300dpi.
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
/// KDP's recommended minimum effective resolution for print images, in DPI.
const MIN_PRINT_DPI: u32 = 300;
/// Ignore tiny placed images (icons, hairline rules) below this pixel size.
const MIN_IMG_PX: u32 = 50;

/// Tally of check outcomes across the run, plus a structured issue log used by
/// the `--json` output (the data source for the web previewer's warnings panel).
struct Report {
    pass: u32,
    warn: u32,
    err: u32,
    /// suppress human-readable prints (JSON mode emits a single object instead).
    json: bool,
    /// current book/lang context, stamped onto each emitted issue.
    cur_book: String,
    cur_lang: Option<String>,
    issues: Vec<serde_json::Value>,
}

impl Report {
    fn new(json: bool) -> Self {
        Self {
            pass: 0,
            warn: 0,
            err: 0,
            json,
            cur_book: String::new(),
            cur_lang: None,
            issues: Vec::new(),
        }
    }
    fn push(&mut self, level: &str, m: &str, page: Option<u32>) {
        self.issues.push(serde_json::json!({
            "level": level,
            "book": self.cur_book,
            "lang": self.cur_lang,
            "page": page,
            "message": m,
        }));
    }
    fn ok(&mut self, m: String) {
        self.pass += 1;
        self.push("ok", &m, None);
        if !self.json {
            println!("  \u{2713} {m}");
        }
    }
    fn warn(&mut self, m: String) {
        self.warn += 1;
        self.push("warn", &m, None);
        if !self.json {
            println!("  ! {m}");
        }
    }
    /// A warning carrying a page number (e.g. the image-DPI audit).
    fn warn_page(&mut self, m: String, page: u32) {
        self.warn += 1;
        self.push("warn", &m, Some(page));
        if !self.json {
            println!("  ! {m}");
        }
    }
    fn bad(&mut self, m: String) {
        self.err += 1;
        self.push("error", &m, None);
        if !self.json {
            println!("  \u{2717} {m}");
        }
    }
    fn say(&self, m: &str) {
        if !self.json {
            println!("{m}");
        }
    }
}

/// `bookmill validate [book] --deep [--json]`
pub fn run(repo: &Repo, book: Option<String>, json: bool) -> Result<()> {
    let dirs = match &book {
        Some(s) => vec![repo.find_book(s)?.1],
        None => repo.book_dirs()?,
    };
    let mut rep = Report::new(json);
    for dir in dirs {
        let (b, _) = repo.load_book_at(&dir)?;
        rep.cur_book = b.slug.clone();
        rep.cur_lang = None;
        rep.say(&format!("\n=== {} [{}] ===", b.slug, b.languages.join(",")));

        // 1) config / KDP / house rules (same as the fast path).
        config_checks(&b, repo, &mut rep);

        // 2) deep artifact checks per edition output (EPUB → epubcheck;
        //    PDF → page-geometry). plan_editions yields exactly the publishable
        //    outputs for the book's editions, with resolved geometry per job.
        let jobs = build::plan_editions(repo, &Some(b.slug.clone()), &None, &None)?;
        for job in &jobs {
            rep.cur_lang = Some(job.lang.clone());
            match job.out {
                Out::RetailEpub | Out::KdpEpub => {
                    epub_check(repo, job, &mut rep);
                    // Accessibility audit of the built EPUB (alt text, a11y metadata,
                    // languages, nav) + DAISY Ace, when it's installed.
                    epub_a11y_check(repo, job, &mut rep);
                    ace_check(repo, job, &mut rep);
                }
                Out::RetailPdf => {
                    pdf_geometry_check(repo, job, &mut rep);
                    pdf_a11y_check(repo, job, &mut rep);
                }
                Out::KdpPdf => {
                    pdf_geometry_check(repo, job, &mut rep);
                    pdf_a11y_check(repo, job, &mut rep);
                    // Audit interior image DPI on the KDP print PDF only.
                    image_dpi_check(repo, job, &mut rep);
                    // Catch "insufficient bleed" (a full-page plate that leaves a
                    // white margin) before KDP rejects it.
                    bleed_coverage_check(repo, job, &mut rep);
                    // Spine-text eligibility (KDP allows spine text only ≥100pp).
                    spine_eligibility_check(repo, job, &mut rep);
                }
                // Editor .docx is not an edition output and not a publish artifact.
                Out::Docx => {}
            }
        }

        // 3) cover resolution per language.
        for lang in &b.languages {
            rep.cur_lang = Some(lang.clone());
            cover_checks(&b.slug, &dir, lang, &mut rep);
        }
    }

    if json {
        let out = serde_json::json!({
            "summary": { "ok": rep.pass, "warn": rep.warn, "error": rep.err },
            "issues": rep.issues,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        // JSON mode never sets a non-zero exit (the caller — the web previewer —
        // wants the issue list regardless of severity).
        return Ok(());
    }

    // Overall verdict (mirrors the web book-detail "Publish readiness" panel).
    let verdict = if rep.err > 0 {
        "\u{2717} NOT READY"
    } else if rep.warn > 0 {
        "\u{26a0} READY WITH WARNINGS"
    } else {
        "\u{2713} READY"
    };
    println!(
        "\n{verdict} — {} ok · {} warning(s) · {} error(s)",
        rep.pass, rep.warn, rep.err
    );
    if rep.err > 0 {
        anyhow::bail!("{} deep-validation error(s)", rep.err);
    }
    Ok(())
}

fn config_checks(b: &BookConfig, repo: &Repo, rep: &mut Report) {
    let mut issues = config::validate_book(b);
    issues.extend(config::validate_book_editions(b, &repo.config));
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
        rep.say(&format!("    (building {} …)", path.display()));
        if let Err(e) = build::build_job(repo, job) {
            rep.bad(format!("EPUB {label}: build failed: {e:#}"));
            return;
        }
    }
    match run_epubcheck(&path, rep.json) {
        Ok((warns, true)) if warns == 0 => rep.ok(format!("EPUB {label}: epubcheck clean")),
        Ok((warns, true)) => rep.warn(format!("EPUB {label}: epubcheck OK, {warns} warning(s)")),
        Ok((_, false)) => rep.bad(format!("EPUB {label}: epubcheck reported errors (see above)")),
        Err(e) => rep.warn(format!("EPUB {label}: epubcheck not run: {e:#}")),
    }
}

/// Run epubcheck on `epub`, echoing ERROR/FATAL/WARNING lines. Returns
/// (warning_count, success). `epubcheck` is the same binary the legacy Makefile
/// used (`make validate_*` → `@epubcheck <file>`); found on `$PATH`.
fn run_epubcheck(epub: &Path, quiet: bool) -> Result<(usize, bool)> {
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
            if !quiet {
                println!("      {l}");
            }
        } else if l.starts_with("WARNING") {
            warns += 1;
            if !quiet {
                println!("      {l}");
            }
        }
    }
    Ok((warns, out.status.success()))
}

// ---------- Accessibility ----------

/// Accessibility audit of a built EPUB (see [`crate::a11y`]): schema.org metadata,
/// `dc:language`, a language on every content document, a nav TOC — and, above all,
/// **alt text on every image**. Missing metadata / alt text / language is an error;
/// the merely *recommended* landmarks and page-list warn.
fn epub_a11y_check(repo: &Repo, job: &Job, rep: &mut Report) {
    let path = build::job_output_path(repo, job);
    let label = format!("{} {} · {}", job.slug, job.lang, job.out.name());
    if !path.exists() {
        rep.warn(format!("a11y {label}: EPUB not built (skipped)"));
        return;
    }
    match crate::a11y::audit_epub(&path) {
        Ok(findings) => emit(findings, &label, rep),
        Err(e) => rep.warn(format!("a11y {label}: audit failed: {e:#}")),
    }
}

/// Accessibility audit of a built PDF: a title and a language (an untitled,
/// language-less PDF fails every checker) and whether it is tagged.
fn pdf_a11y_check(repo: &Repo, job: &Job, rep: &mut Report) {
    let path = build::job_output_path(repo, job);
    let ed = job.edition.clone().unwrap_or_else(|| job.target.clone());
    let label = format!("{} {} · {} · {}", job.slug, job.lang, ed, job.out.name());
    if !path.exists() {
        return; // the geometry check already noted a missing artifact
    }
    emit(crate::a11y::audit_pdf(&path), &label, rep);
}

/// Map the audit's findings onto the report's ✓ / ! / ✗ tally.
fn emit(findings: Vec<crate::a11y::Finding>, label: &str, rep: &mut Report) {
    use crate::a11y::Level;
    for f in findings {
        let m = f.msg.replacen("a11y ", &format!("a11y {label} "), 1);
        match f.level {
            Level::Ok => rep.ok(m),
            Level::Warn => rep.warn(m),
            Level::Err => rep.bad(m),
        }
    }
}

/// Run **DAISY Ace** on the built EPUB when it is installed, surfacing its
/// violations. Optional, like `epubcheck`: absent from `$PATH` → skipped with a
/// note, never a hard dependency.
fn ace_check(repo: &Repo, job: &Job, rep: &mut Report) {
    let path = build::job_output_path(repo, job);
    let label = format!("{} {} · {}", job.slug, job.lang, job.out.name());
    if !path.exists() {
        return; // already noted
    }
    let outdir = std::env::temp_dir().join(format!("bookmill-ace-{}-{}", job.slug, job.lang));
    let out = match Command::new("ace")
        .arg("-o")
        .arg(&outdir)
        .arg("-f") // overwrite a previous report dir
        .arg(&path)
        .output()
    {
        Ok(o) => o,
        Err(_) => {
            rep.say(&format!("    (ace not on $PATH — accessibility checker skipped for {label})"));
            return;
        }
    };
    // Ace writes its findings to <outdir>/report.json; `assertions` is empty when clean.
    let report = outdir.join("report.json");
    let violations = std::fs::read_to_string(&report)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .map(|j| ace_violations(&j));
    let _ = std::fs::remove_dir_all(&outdir);
    match violations {
        Some(v) if v.is_empty() => rep.ok(format!("a11y {label}: DAISY Ace clean")),
        Some(v) => {
            for m in v {
                rep.warn(format!("a11y {label}: ace: {m}"));
            }
        }
        None => {
            let text = String::from_utf8_lossy(&out.stderr);
            rep.warn(format!(
                "a11y {label}: ace produced no report{}",
                if text.trim().is_empty() { String::new() } else { format!(": {}", text.trim()) }
            ))
        }
    }
}

/// Pull the human-readable violations out of an Ace `report.json`
/// (`assertions[].assertions[].earl:test.dct:title` + the failure message).
fn ace_violations(j: &serde_json::Value) -> Vec<String> {
    let mut v = Vec::new();
    let Some(outer) = j.get("assertions").and_then(|a| a.as_array()) else {
        return v;
    };
    for o in outer {
        let Some(inner) = o.get("assertions").and_then(|a| a.as_array()) else {
            continue;
        };
        for a in inner {
            let rule = a
                .get("earl:test")
                .and_then(|t| t.get("dct:title"))
                .and_then(|t| t.as_str())
                .unwrap_or("(rule)");
            let impact = a
                .get("earl:test")
                .and_then(|t| t.get("earl:impact"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let detail = a
                .get("earl:result")
                .and_then(|r| r.get("dct:description"))
                .and_then(|d| d.as_str())
                .unwrap_or("");
            v.push(format!("{rule} [{impact}] {detail}").trim().to_string());
        }
    }
    v
}

// ---------- PDF geometry ----------

fn pdf_geometry_check(repo: &Repo, job: &Job, rep: &mut Report) {
    let path = build::job_output_path(repo, job);
    let ed = job.edition.clone().unwrap_or_else(|| job.target.clone());
    let label = format!("{} {} · {} · {}", job.slug, job.lang, ed, job.out.name());
    if !path.exists() {
        rep.say(&format!("    (building {} …)", path.display()));
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
        Err(e) => rep.warn(format!("PDF {label}: page-size read failed: {e:#}")),
    }
}

/// First-page size in points (width, height), native via `lopdf` (first page's
/// `MediaBox`). Replaces parsing poppler `pdfinfo`'s `Page size:` line.
fn pdf_page_size(pdf: &Path) -> Result<(f64, f64)> {
    crate::pdfmeta::page_size_pt(pdf)
        .with_context(|| format!("could not read page size from {}", pdf.display()))
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
            Err(e) => rep.warn(format!("cover {slug} {lang}: wrap page-size: {e:#}")),
        }
    }

}

// ---------- Interior image DPI ----------

/// One audited raster image and its effective resolution at print size.
struct DpiRow {
    page: u32,
    w: u32,
    h: u32,
    ppi: u32,
}

/// Audit interior image DPI on the (already-built) KDP print PDF.
///
/// [`crate::pdfmeta::placed_images`] reports, per embedded image, the placed
/// effective resolution at print size (it interprets the content-stream CTM —
/// the same number poppler's `pdfimages -list` used to print). Anything below
/// [`MIN_PRINT_DPI`] is flagged as a `warn` (not an error): some decorative or
/// full-bleed art may be intentionally lower, so a warning is the right level.
/// Mirrors the cover check: if the artifact isn't built, note it and skip — we
/// never rebuild here (the geometry check that runs first handles building).
fn image_dpi_check(repo: &Repo, job: &Job, rep: &mut Report) {
    let path = build::job_output_path(repo, job);
    let ed = job.edition.clone().unwrap_or_else(|| job.target.clone());
    let label = format!("{} {} · {} · {}", job.slug, job.lang, ed, job.out.name());
    if !path.exists() {
        rep.warn(format!("image DPI {label}: PDF not built (skipped)"));
        return;
    }
    let images = match crate::pdfmeta::placed_images(&path) {
        Some(imgs) => imgs,
        None => {
            rep.warn(format!("image DPI {label}: could not read PDF images"));
            return;
        }
    };
    let (checked, min_seen, low) = audit_dpi_rows(&images, MIN_PRINT_DPI, MIN_IMG_PX);
    match (checked, min_seen) {
        (0, _) | (_, None) => rep.ok(format!("image DPI {label}: no raster images to audit")),
        (_, Some(min)) if low.is_empty() => rep.ok(format!(
            "image DPI {label}: {checked} image(s) ≥{MIN_PRINT_DPI}dpi (min {min})"
        )),
        (_, Some(min)) => {
            // One page-stamped warning per low-DPI image (the previewer keys off
            // the page number to flag the offending spread).
            for r in &low {
                rep.warn_page(
                    format!(
                        "image DPI {label}: p{} {}×{}px @ {}dpi (below {MIN_PRINT_DPI}dpi; min {min})",
                        r.page, r.w, r.h, r.ppi
                    ),
                    r.page,
                );
            }
        }
    }
}

/// Audit a list of placed images for effective DPI.
///
/// We audit every placed raster image, skipping tiny ones (< `min_px` on either
/// pixel side — icons, hairline rules). The effective DPI is the smaller of the
/// two axes (matching what poppler's `pdfimages -list` reported as
/// `min(x-ppi,y-ppi)`). Returns `(images_checked, min_dpi_seen, rows_below_min)`.
fn audit_dpi_rows(
    images: &[crate::pdfmeta::PlacedImage],
    min_dpi: u32,
    min_px: u32,
) -> (usize, Option<u32>, Vec<DpiRow>) {
    let mut checked = 0usize;
    let mut min_seen: Option<u32> = None;
    let mut low = Vec::new();
    for im in images {
        if im.px_w < min_px || im.px_h < min_px {
            continue;
        }
        // A degenerate placement (zero-area CTM) can't yield a meaningful ppi.
        if im.placed_w_pt <= 0.0 || im.placed_h_pt <= 0.0 {
            continue;
        }
        let ppi = im.dpi();
        checked += 1;
        min_seen = Some(min_seen.map_or(ppi, |m| m.min(ppi)));
        if ppi < min_dpi {
            low.push(DpiRow {
                page: im.page,
                w: im.px_w,
                h: im.px_h,
                ppi,
            });
        }
    }
    (checked, min_seen, low)
}

// ---------- Spine-text eligibility ----------

/// KDP allows printed **spine text** only at this page count or above (thinner
/// books ship a blank spine).
const SPINE_TEXT_MIN_PAGES: u32 = 100;

/// Report the interior page count and whether the cover may carry **spine text**
/// (KDP's ≥100-page rule). Informational — a blank spine on a thin book is correct,
/// not an error. Runs once per (book, lang) on the canonical kdp-paperback interior
/// (not the per-region bubok copies) to avoid duplicate lines.
fn spine_eligibility_check(repo: &Repo, job: &Job, rep: &mut Report) {
    if !matches!(job.target.as_str(), "kdp-paperback" | "kdp-hardcover") {
        return;
    }
    let path = build::job_output_path(repo, job);
    let ed = job.edition.clone().unwrap_or_else(|| job.target.clone());
    let label = format!("{} {} · {} · {}", job.slug, job.lang, ed, job.out.name());
    let Some(pages) = pdf_pages(&path) else { return }; // missing artifact already noted
    if pages >= SPINE_TEXT_MIN_PAGES {
        rep.ok(format!(
            "spine {label}: interior {pages}pp — spine text allowed (≥{SPINE_TEXT_MIN_PAGES}pp)"
        ));
    } else {
        rep.ok(format!(
            "spine {label}: interior {pages}pp — spine text NOT allowed (<{SPINE_TEXT_MIN_PAGES}pp); ship a blank spine"
        ));
    }
}

/// Page count of a PDF (native, via `lopdf`). None if missing/unreadable.
fn pdf_pages(pdf: &Path) -> Option<u32> {
    crate::pdfmeta::page_count(pdf)
}

// ---------- Interior bleed coverage ----------

/// A plate covering ≥ this fraction of the page *area* is treated as a full-page
/// image that is meant to bleed (vs. a small inset/vignette, which is not).
const FULLPAGE_AREA_FRAC: f64 = 0.70;
/// A bleeding full-page image should reach the page edge. If it falls short by
/// more than this (total, both sides of a dimension), it leaves a white margin —
/// KDP's "insufficient bleed". 1/16in is KDP's cut tolerance; a fit-to-trim plate
/// on a 0.125in-bleed page falls short by 0.25in, well past this.
const BLEED_GAP_TOL_IN: f64 = 0.0625;

/// A full-page image that doesn't reach the page edge (placed size vs. page size).
struct BleedRow {
    page: u32,
    placed_w: f64,
    placed_h: f64,
    gap_w: f64,
    gap_h: f64,
}

/// Flag full-page plates that leave a white margin (insufficient bleed) on the
/// KDP print PDF — the exact failure KDP rejects ("insufficient bleed, page N").
/// [`crate::pdfmeta::placed_images`] gives each image's pixel size and *placed*
/// size in inches (from the content-stream CTM); compared to the page size (from
/// the job's resolved geometry), a near-full-page image that stops short of the
/// edge is reported as a `warn` (heuristic — decorative bordered art could trip it).
fn bleed_coverage_check(repo: &Repo, job: &Job, rep: &mut Report) {
    let Some(g) = job.geometry else { return };
    let path = build::job_output_path(repo, job);
    let ed = job.edition.clone().unwrap_or_else(|| job.target.clone());
    let label = format!("{} {} · {} · {}", job.slug, job.lang, ed, job.out.name());
    if !path.exists() {
        return; // the DPI/geometry checks already note a missing artifact
    }
    let images = match crate::pdfmeta::placed_images(&path) {
        Some(imgs) => imgs,
        None => return, // image_dpi_check surfaces the read failure
    };
    let short = audit_bleed_rows(&images, g.pw, g.ph, MIN_IMG_PX);
    if short.is_empty() {
        rep.ok(format!("bleed {label}: full-page images reach the page edge"));
        return;
    }
    for r in &short {
        rep.warn_page(
            format!(
                "bleed {label}: p{} full-page image {:.2}×{:.2}in leaves a {:.2}×{:.2}in margin \
                 (does not reach the page edge — KDP insufficient bleed)",
                r.page, r.placed_w, r.placed_h, r.gap_w, r.gap_h
            ),
            r.page,
        );
    }
}

/// Scan placed images for full-page plates whose placed size (in inches) falls
/// short of the page size. Skips small insets (< [`FULLPAGE_AREA_FRAC`] of the
/// page area — vignettes are meant to float, not bleed) and tiny images.
fn audit_bleed_rows(
    images: &[crate::pdfmeta::PlacedImage],
    page_w: f64,
    page_h: f64,
    min_px: u32,
) -> Vec<BleedRow> {
    let page_area = page_w * page_h;
    let mut out = Vec::new();
    for im in images {
        if im.px_w < min_px || im.px_h < min_px {
            continue;
        }
        let placed_w = im.placed_w_in();
        let placed_h = im.placed_h_in();
        if placed_w <= 0.0 || placed_h <= 0.0 {
            continue;
        }
        // Only consider images large enough to be "meant to be full-page".
        if placed_w * placed_h < FULLPAGE_AREA_FRAC * page_area {
            continue;
        }
        let gap_w = (page_w - placed_w).max(0.0);
        let gap_h = (page_h - placed_h).max(0.0);
        if gap_w > BLEED_GAP_TOL_IN || gap_h > BLEED_GAP_TOL_IN {
            out.push(BleedRow {
                page: im.page,
                placed_w,
                placed_h,
                gap_w,
                gap_h,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdfmeta::PlacedImage;

    /// Build a placed image of `w×h` px at a known effective `dpi` (square pixels),
    /// i.e. placed size = `px / dpi` inches → `px / dpi * 72` pt.
    fn placed(page: u32, w: u32, h: u32, dpi: f64) -> PlacedImage {
        PlacedImage {
            page,
            px_w: w,
            px_h: h,
            placed_w_pt: w as f64 / dpi * 72.0,
            placed_h_pt: h as f64 / dpi * 72.0,
        }
    }

    // Mirrors the old `pdfimages -list` sample: two ≥300dpi plates, one 150dpi
    // plate, a tiny 40px icon (skipped), and a sub-min-px placement.
    fn sample() -> Vec<PlacedImage> {
        vec![
            placed(6, 2048, 3072, 349.0),
            placed(8, 612, 820, 342.0),
            placed(10, 800, 600, 150.0),
            placed(12, 40, 40, 72.0), // tiny → skipped by MIN_IMG_PX
        ]
    }

    #[test]
    fn audits_placed_images_and_flags_low_dpi() {
        let (checked, min_seen, low) = audit_dpi_rows(&sample(), MIN_PRINT_DPI, MIN_IMG_PX);
        // 3 audited: the two 349/342 images and the 150-dpi one. The 40px tiny
        // image is skipped.
        assert_eq!(checked, 3);
        assert_eq!(min_seen, Some(150));
        assert_eq!(low.len(), 1);
        assert_eq!(low[0].page, 10);
        assert_eq!(low[0].ppi, 150);
        assert_eq!((low[0].w, low[0].h), (800, 600));
    }

    #[test]
    fn empty_audits_nothing() {
        let (checked, min_seen, low) = audit_dpi_rows(&[], MIN_PRINT_DPI, MIN_IMG_PX);
        assert_eq!(checked, 0);
        assert_eq!(min_seen, None);
        assert!(low.is_empty());
    }

    // page 6.125 x 9.25 (6x9 trim + 0.125 bleed):
    //   p6  fit-to-trim plate 2048x3072 @ 349dpi  -> placed 5.87x8.80in (white border) -> FLAG
    //   p30 full-bleed plate  2458x3712 @ 401dpi  -> placed 6.13x9.26in (reaches edge)  -> ok
    //   p8  vignette          612x820   @ 342dpi  -> placed 1.79x2.40in (small)          -> skip
    //   p12 tiny icon         40x40     @ 72dpi                                          -> skip
    fn bleed_sample() -> Vec<PlacedImage> {
        vec![
            placed(6, 2048, 3072, 349.0),
            placed(8, 612, 820, 342.0),
            placed(12, 40, 40, 72.0),
            placed(30, 2458, 3712, 401.0),
        ]
    }

    #[test]
    fn flags_only_fit_to_trim_fullpage_plate() {
        let short = audit_bleed_rows(&bleed_sample(), 6.125, 9.25, MIN_IMG_PX);
        // only the fit-to-trim plate on page 6 leaves a margin; the full-bleed
        // plate (p30), the vignette (p8, too small) and the tiny icon are not flagged.
        assert_eq!(short.len(), 1);
        assert_eq!(short[0].page, 6);
        assert!(short[0].gap_w > 0.0625 && short[0].gap_h > 0.0625);
    }
}
