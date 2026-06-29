//! Deep validation (`bookmill validate --deep`).
//!
//! On top of the fast config/listing/house-rule checks (`config::validate_book`),
//! this verifies the *built artifacts* per book × edition × language:
//!   1. EPUB   — build (or reuse) the EPUB and run **epubcheck** on it.
//!   2. PDF    — build (or reuse) the print/digital PDF and verify, via `pdfinfo`,
//!               that the page size equals the edition's expected trim+bleed
//!               (e.g. kdp-paperback picture book = 6.125×9.25in = 441×666pt;
//!               6×9 text = 432×648pt). For the KDP print PDF it also audits
//!               interior image DPI via `pdfimages -list` (effective resolution
//!               at print size), warning on any image below ~300dpi.
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
        config_checks(&b, &mut rep);

        // 2) deep artifact checks per edition output (EPUB → epubcheck;
        //    PDF → page-geometry). plan_editions yields exactly the publishable
        //    outputs for the book's editions, with resolved geometry per job.
        let jobs = build::plan_editions(repo, &Some(b.slug.clone()), &None, &None)?;
        for job in &jobs {
            rep.cur_lang = Some(job.lang.clone());
            match job.out {
                Out::RetailEpub | Out::KdpEpub => epub_check(repo, job, &mut rep),
                Out::RetailPdf => pdf_geometry_check(repo, job, &mut rep),
                Out::KdpPdf => {
                    pdf_geometry_check(repo, job, &mut rep);
                    // Audit interior image DPI on the KDP print PDF only.
                    image_dpi_check(repo, job, &mut rep);
                }
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
/// `pdfimages -list` reports, per embedded image, the placed `x-ppi`/`y-ppi` —
/// i.e. the *effective* resolution at print size. Anything below
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
    let list = match pdfimages_list(&path) {
        Ok(t) => t,
        Err(e) => {
            rep.warn(format!("image DPI {label}: pdfimages failed: {e:#}"));
            return;
        }
    };
    let (checked, min_seen, low) = audit_dpi_rows(&list, MIN_PRINT_DPI, MIN_IMG_PX);
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

/// Run `pdfimages -list` (poppler — same toolset as `pdfinfo`) and return stdout.
fn pdfimages_list(pdf: &Path) -> Result<String> {
    let out = Command::new("pdfimages")
        .arg("-list")
        .arg(pdf)
        .output()
        .context("running pdfimages -list (is poppler on $PATH?)")?;
    if !out.status.success() {
        anyhow::bail!("pdfimages failed on {}", pdf.display());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parse `pdfimages -list` output into audited image rows.
///
/// Columns are whitespace-separated: `page num type width height color comp bpc
/// enc interp object ID x-ppi y-ppi size ratio`. We only audit placed raster
/// images (`type == "image"`), skip the header/separator rows (non-numeric
/// `page`), skip vector/inline entries whose ppi is `-` (non-numeric), and skip
/// tiny images (< `min_px` on either side). The effective DPI is the smaller of
/// x-ppi/y-ppi. Returns `(images_checked, min_dpi_seen, rows_below_min)`.
fn audit_dpi_rows(list: &str, min_dpi: u32, min_px: u32) -> (usize, Option<u32>, Vec<DpiRow>) {
    let mut checked = 0usize;
    let mut min_seen: Option<u32> = None;
    let mut low = Vec::new();
    for line in list.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 16 || f[2] != "image" {
            continue;
        }
        let (page, w, h, xppi, yppi) = match (
            f[0].parse::<u32>(),
            f[3].parse::<u32>(),
            f[4].parse::<u32>(),
            f[12].parse::<u32>(),
            f[13].parse::<u32>(),
        ) {
            (Ok(p), Ok(w), Ok(h), Ok(x), Ok(y)) => (p, w, h, x, y),
            _ => continue, // header row or `-` ppi (vector/inline) → skip
        };
        if w < min_px || h < min_px {
            continue;
        }
        let ppi = xppi.min(yppi);
        checked += 1;
        min_seen = Some(min_seen.map_or(ppi, |m| m.min(ppi)));
        if ppi < min_dpi {
            low.push(DpiRow { page, w, h, ppi });
        }
    }
    (checked, min_seen, low)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
page   num  type   width height color comp bpc  enc interp  object ID x-ppi y-ppi size ratio
--------------------------------------------------------------------------------------------
   6     0 image    2048  3072  rgb     3   8  image  no        47  0   349   349 8980K  49%
   8     1 image     612   820  rgb     3   8  image  no        57  0   342   342  619K  42%
   8     2 smask     612   820  gray    1   8  image  no        57  0   342   342 4469B 0.9%
  10     3 image     800   600  rgb     3   8  image  no        66  0   150   150 4033K  22%
  12     4 image      40    40  rgb     3   8  image  no        70  0    72    72  100B  10%
  14     5 image     900   900  rgb     3   8  jpeg   no        80  0     -     - 1000B   1%";

    #[test]
    fn audits_placed_images_and_flags_low_dpi() {
        let (checked, min_seen, low) = audit_dpi_rows(SAMPLE, MIN_PRINT_DPI, MIN_IMG_PX);
        // 3 audited: the two 349/342 images and the 150-dpi one. The smask (not
        // type "image"), the 40px tiny image, and the `-` ppi vector are skipped.
        assert_eq!(checked, 3);
        assert_eq!(min_seen, Some(150));
        assert_eq!(low.len(), 1);
        assert_eq!(low[0].page, 10);
        assert_eq!(low[0].ppi, 150);
        assert_eq!((low[0].w, low[0].h), (800, 600));
    }

    #[test]
    fn empty_or_header_only_audits_nothing() {
        let header = "page num type width height color comp bpc enc interp object ID x-ppi y-ppi size ratio";
        let (checked, min_seen, low) = audit_dpi_rows(header, MIN_PRINT_DPI, MIN_IMG_PX);
        assert_eq!(checked, 0);
        assert_eq!(min_seen, None);
        assert!(low.is_empty());
    }
}
