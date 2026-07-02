//! Build orchestrator: drive the rendering toolchains from the resolved config.
//! Supports --format builds and edition/target-driven builds.
//!
//! Every output is produced by native Rust — there is NO pandoc dependency:
//! PDFs (retail + KDP print) render through the native Typst engine
//! (`typst_pdf`), EPUB through `epub_native` (epub-builder + comrak), and the
//! editor `.docx` review doc through `docx_native` (docx-rs). A build request is
//! expanded up front into a flat list of JOBS (book × edition-or-format × lang ×
//! output), then run through a queue with per-job progress + ETA.

use crate::config::BookConfig;
use crate::discover::Repo;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DEFAULT_EPUB_PX: u32 = 1200;

#[derive(Clone, Copy, PartialEq)]
pub enum Out {
    RetailEpub,
    KdpEpub,
    RetailPdf,
    KdpPdf,
    /// Editor review document (`.docx`, native via docx-rs). Not a publish format.
    Docx,
}

impl Out {
    pub fn name(self) -> &'static str {
        match self {
            Out::RetailEpub => "RetailEpub",
            Out::KdpEpub => "KdpEpub",
            Out::RetailPdf => "RetailPdf",
            Out::KdpPdf => "KdpPdf",
            Out::Docx => "Docx",
        }
    }
}

/// One unit of work in the build queue.
#[derive(Clone)]
pub struct Job {
    pub slug: String,
    pub dir: PathBuf,
    pub lang: String,
    /// edition name when edition-driven; None for --format builds
    pub edition: Option<String>,
    /// edition target or format name — used for the label's middle segment
    pub target: String,
    pub out: Out,
    pub epub_px: u32,
    /// Full PDF page geometry (paper size + margins) in inches, resolved from
    /// config (edition trim+bleed + book/repo margins). None for EPUB outputs.
    pub geometry: Option<PageGeometry>,
}

/// Full print page geometry, in inches. Paper size comes from edition trim+bleed;
/// margins from book [pdf.margins] / repo [defaults.margins]. Passed to the
/// native Typst engine, which sets the page size and margins directly.
#[derive(Clone, Copy, Debug)]
pub struct PageGeometry {
    pub pw: f64,
    pub ph: f64,
    pub top: f64,
    pub bottom: f64,
    pub inner: f64,
    pub outer: f64,
    pub bindingoffset: f64,
}

impl Job {
    /// e.g. "la-riqueza-de-la-isla es · kdp-paperback · KdpPdf"
    pub fn label(&self) -> String {
        let mid = self.edition.clone().unwrap_or_else(|| self.target.clone());
        format!("{} {} · {} · {}", self.slug, self.lang, mid, self.out.name())
    }
}

/// Events emitted by the queue runner. Owned data so they can cross a channel
/// (the TUI runs the queue in a scoped thread and renders from these).
pub enum QueueEvent {
    Start {
        idx: usize,
        total: usize,
        label: String,
        elapsed: Duration,
        eta: Option<Duration>,
    },
    Done {
        idx: usize,
        label: String,
        dur: Duration,
        err: Option<String>,
    },
    Summary {
        total: usize,
        failures: usize,
        elapsed: Duration,
    },
}

// ---------- job planning ----------

fn outputs_for_format(fmt: &str) -> Vec<Out> {
    match fmt {
        "epub" => vec![Out::RetailEpub],
        "pdf" => vec![Out::RetailPdf],
        "kdp" => vec![Out::KdpEpub, Out::KdpPdf],
        "print" => vec![Out::KdpPdf],
        "docx" => vec![Out::Docx],
        _ => vec![Out::RetailEpub, Out::KdpEpub, Out::RetailPdf, Out::KdpPdf],
    }
}

fn outputs_for_target(target: &str) -> Vec<Out> {
    match target {
        "kdp-paperback" => vec![Out::KdpPdf],
        "kdp-hardcover" => vec![Out::KdpPdf], // same bleed interior; differs only in (case-laminate) cover
        "kdp-epub" => vec![Out::KdpEpub],
        "kdp" => vec![Out::KdpEpub, Out::KdpPdf],
        "gumroad" => vec![Out::RetailEpub, Out::RetailPdf],
        "bubok" => vec![Out::KdpPdf], // POD print interior (bleed); per-region geometry via edition trim
        _ => vec![Out::RetailEpub, Out::RetailPdf],
    }
}

// ---------- geometry ----------

/// Parse a trim like "6x9", "6x9in", "5.83x8.27in" into (width, height) inches.
fn parse_trim(s: &str) -> Option<(f64, f64)> {
    let s = s.trim().trim_end_matches("in");
    let (w, h) = s.split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// Parse a length like "0.125in" or "0" into inches.
fn parse_len(s: &str) -> Option<f64> {
    s.trim().trim_end_matches("in").trim().parse().ok()
}

/// Resolve the PDF page geometry for one output flavor of an edition.
/// Print PDFs (`KdpPdf`) add bleed; digital PDFs (`RetailPdf`, gumroad) do not.
/// Returns None for EPUB outputs (no page geometry).
fn resolve_paper_dims(
    repo: &Repo,
    book: &BookConfig,
    edition: Option<&crate::config::Edition>,
    out: Out,
) -> Option<(f64, f64)> {
    let (tw, th) = {
        let trim = edition
            .and_then(|e| e.trim.clone())
            .or_else(|| book.pdf.trim.clone())
            .or_else(|| repo.config.defaults.trim.clone())
            .unwrap_or_else(|| "6x9".into());
        parse_trim(&trim)?
    };
    match out {
        // print interior: add KDP bleed (outer edge + top/bottom)
        Out::KdpPdf => {
            let bleed = edition
                .and_then(|e| e.bleed.clone())
                .or_else(|| book.pdf.bleed.clone())
                .or_else(|| repo.config.defaults.bleed.clone())
                .and_then(|b| parse_len(&b))
                .unwrap_or(0.125);
            Some((tw + bleed, th + 2.0 * bleed))
        }
        // digital PDF (gumroad): trim size, no bleed
        Out::RetailPdf => Some((tw, th)),
        // EPUB: no geometry
        _ => None,
    }
}

/// Resolve the FULL page geometry (paper size + margins) for a PDF output.
/// Returns None for EPUB outputs. Margins resolve per field:
/// book `[pdf.margins]` -> repo `[defaults.margins]` -> 6x9 text fallback.
fn resolve_geometry(
    repo: &Repo,
    book: &BookConfig,
    edition: Option<&crate::config::Edition>,
    out: Out,
) -> Option<PageGeometry> {
    let (pw, ph) = resolve_paper_dims(repo, book, edition, out)?;
    let bm = book.pdf.margins.as_ref();
    let dm = repo.config.defaults.margins.as_ref();
    let pick = |get: &dyn Fn(&crate::config::Margins) -> Option<f64>, fallback: f64| -> f64 {
        bm.and_then(get)
            .or_else(|| dm.and_then(get))
            .unwrap_or(fallback)
    };
    Some(PageGeometry {
        pw,
        ph,
        top: pick(&|m| m.top, 0.75),
        bottom: pick(&|m| m.bottom, 0.75),
        inner: pick(&|m| m.inner, 0.75),
        outer: pick(&|m| m.outer, 0.6),
        bindingoffset: pick(&|m| m.bindingoffset, 0.375),
    })
}

/// KDP print interior geometry for the web previewer.
///
/// Resolves the trim + bleed (inches) and the full page geometry (paper incl.
/// bleed + margins) of the book's KDP **print** interior — the edition whose
/// `KdpPdf` output lands at the canonical `<slug>-<lang>-kdp.pdf` path (i.e. a
/// `kdp-*` target, not the edition-suffixed `bubok` POD). Falls back to book /
/// repo trim+bleed defaults. Returns `None` if the book has no print interior.
pub struct PrintGeometry {
    /// trim width / height (inches), before bleed
    pub trim_w: f64,
    pub trim_h: f64,
    /// per-side bleed added on the KDP print page (inches)
    pub bleed: f64,
    /// full page geometry: paper (= trim + bleed) + margins
    pub geom: PageGeometry,
}

pub fn kdp_print_geometry(repo: &Repo, book: &BookConfig) -> Option<PrintGeometry> {
    // Pick the edition that produces the canonical `-kdp.pdf` (KdpPdf target,
    // excluding bubok, whose output is edition-suffixed).
    let ed = book.editions.iter().find_map(|name| {
        let e = repo.config.editions.get(name)?;
        let target = e.target.clone().unwrap_or_else(|| "retail".into());
        (outputs_for_target(&target).contains(&Out::KdpPdf) && target != "bubok").then_some(e)
    });
    let trim = ed
        .and_then(|e| e.trim.clone())
        .or_else(|| book.pdf.trim.clone())
        .or_else(|| repo.config.defaults.trim.clone())
        .unwrap_or_else(|| "6x9".into());
    let (trim_w, trim_h) = parse_trim(&trim)?;
    let bleed = ed
        .and_then(|e| e.bleed.clone())
        .or_else(|| book.pdf.bleed.clone())
        .or_else(|| repo.config.defaults.bleed.clone())
        .and_then(|b| parse_len(&b))
        .unwrap_or(0.125);
    let geom = resolve_geometry(repo, book, ed, Out::KdpPdf)?;
    Some(PrintGeometry { trim_w, trim_h, bleed, geom })
}

fn langs_for(book: &BookConfig, lang_filter: &Option<String>) -> Vec<String> {
    match lang_filter {
        Some(l) if l != "all" => vec![l.clone()],
        _ => book.languages.clone(),
    }
}

/// Expand a --format request into jobs.
pub fn plan_format(
    repo: &Repo,
    book_slug: &Option<String>,
    format: &Option<String>,
    lang_filter: &Option<String>,
) -> Result<Vec<Job>> {
    let fmt = format.clone().unwrap_or_else(|| "all".into());
    let outs = outputs_for_format(&fmt);
    let mut jobs = Vec::new();
    for (book, dir) in resolve_books(repo, book_slug)? {
        for lang in langs_for(&book, lang_filter) {
            for out in &outs {
                jobs.push(Job {
                    slug: book.slug.clone(),
                    dir: dir.clone(),
                    lang: lang.clone(),
                    edition: None,
                    target: fmt.clone(),
                    out: *out,
                    epub_px: DEFAULT_EPUB_PX,
                    geometry: resolve_geometry(repo, &book, None, *out),
                });
            }
        }
    }
    Ok(jobs)
}

/// Expand an edition-driven request into jobs.
pub fn plan_editions(
    repo: &Repo,
    book_slug: &Option<String>,
    lang_filter: &Option<String>,
    edition_filter: &Option<String>,
) -> Result<Vec<Job>> {
    // Static edition-level checks (paper/ink/finish/ISBN). Errors block the plan;
    // warnings are surfaced but non-fatal.
    let mut errors: Vec<String> = Vec::new();
    for issue in crate::config::validate_editions(&repo.config) {
        if issue.level == "error" {
            errors.push(issue.msg);
        } else {
            eprintln!("  ! {}", issue.msg);
        }
    }

    let mut jobs = Vec::new();
    for (book, dir) in resolve_books(repo, book_slug)? {
        let eds: Vec<String> = match edition_filter {
            Some(e) if e != "all" => vec![e.clone()],
            _ => book.editions.clone(),
        };
        for ed_name in &eds {
            // A book (or --edition) may only name an edition defined in the repo
            // `[editions]` table. Without this guard an unknown/typo'd edition
            // silently fell through to a "retail" build instead of the intended
            // KDP interior (M1). Error clearly instead.
            let ed = match repo.config.editions.get(ed_name) {
                Some(e) => Some(e),
                None => {
                    let known: Vec<&str> =
                        repo.config.editions.keys().map(String::as_str).collect();
                    errors.push(format!(
                        "{}: unknown edition {ed_name:?} (not in [editions]; known: {})",
                        book.slug,
                        if known.is_empty() { "none".into() } else { known.join(", ") }
                    ));
                    continue;
                }
            };
            let target = ed
                .and_then(|e| e.target.clone())
                .unwrap_or_else(|| "retail".into());
            let img_px = ed.and_then(|e| e.epub_image_px).unwrap_or(DEFAULT_EPUB_PX);
            let outs = outputs_for_target(&target);

            // Resolve the per-(edition,book) color choice and validate it.
            let paper = crate::config::resolve_paper(ed, &book, &repo.config);
            let ink = crate::config::resolve_ink(ed, &book, &repo.config);
            for issue in
                crate::config::validate_color_compat(&book.slug, ed_name, &paper, &ink, &target)
            {
                if issue.level == "error" {
                    errors.push(issue.msg);
                } else {
                    eprintln!("  ! {}", issue.msg);
                }
            }

            for lang in langs_for(&book, lang_filter) {
                // Page-range / hardcover eligibility gate. Needs the built interior:
                // warn pre-build, fail post-build when out of range. The hardcover
                // ≥75pp minimum is the activation gate (e.g. la-riqueza at 60pp).
                if matches!(target.as_str(), "kdp-paperback" | "kdp-hardcover") {
                    let (min, max) = crate::config::page_range(&target, &ink);
                    let kdp_pdf = repo
                        .root
                        .join("output")
                        .join(&book.slug)
                        .join(&lang)
                        .join(format!("{}-{}-kdp.pdf", book.slug, lang));
                    match pdf_page_count(&kdp_pdf) {
                        Some(pp) if pp < min || pp > max => errors.push(format!(
                            "{}/{ed_name} [{lang}]: interior is {pp}pp, outside {target} range {min}–{max}pp",
                            book.slug
                        )),
                        Some(_) => {}
                        None if target == "kdp-hardcover" => eprintln!(
                            "  ! {}/{ed_name} [{lang}]: hardcover needs ≥{min}pp — build the interior to verify (missing {})",
                            book.slug,
                            kdp_pdf.display()
                        ),
                        None => {}
                    }
                }

                for out in &outs {
                    jobs.push(Job {
                        slug: book.slug.clone(),
                        dir: dir.clone(),
                        lang: lang.clone(),
                        edition: Some(ed_name.clone()),
                        target: target.clone(),
                        out: *out,
                        epub_px: img_px,
                        geometry: resolve_geometry(repo, &book, ed, *out),
                    });
                }
            }
        }
    }
    if !errors.is_empty() {
        bail!("edition config invalid:\n  - {}", errors.join("\n  - "));
    }
    Ok(jobs)
}

/// Page count of a PDF (native, via `lopdf`). None if the file is missing or
/// unreadable (so callers can treat "not built yet" distinctly from an
/// out-of-range count).
fn pdf_page_count(pdf: &Path) -> Option<u32> {
    crate::pdfmeta::page_count(pdf)
}

// ---------- entry points ----------

/// --format based build (optionally limited to one language).
pub fn run(
    repo: &Repo,
    book_slug: Option<String>,
    format: Option<String>,
    lang_filter: Option<String>,
) -> Result<()> {
    let jobs = plan_format(repo, &book_slug, &format, &lang_filter)?;
    run_queue_stdout(repo, &jobs)
}

/// Edition/target driven build.
pub fn run_editions(
    repo: &Repo,
    book_slug: Option<String>,
    lang_filter: Option<String>,
    edition_filter: Option<String>,
) -> Result<()> {
    let jobs = plan_editions(repo, &book_slug, &lang_filter, &edition_filter)?;
    run_queue_stdout(repo, &jobs)
}

// ---------- queue runner ----------

/// Run a list of jobs sequentially, calling `cb` with progress events. Does no
/// printing itself — the caller (CLI stdout or TUI) decides how to surface
/// progress. Returns Err if any job failed (after running them all).
pub fn run_queue(repo: &Repo, jobs: &[Job], cb: &mut dyn FnMut(QueueEvent)) -> Result<()> {
    let n = jobs.len();
    let start = Instant::now();
    let mut durs: Vec<Duration> = Vec::new();
    let mut failures = 0usize;

    for (i, job) in jobs.iter().enumerate() {
        let idx = i + 1;
        let elapsed = start.elapsed();
        let eta = if durs.is_empty() {
            None
        } else {
            let avg = durs.iter().sum::<Duration>() / durs.len() as u32;
            Some(avg * (n - i) as u32)
        };
        cb(QueueEvent::Start {
            idx,
            total: n,
            label: job.label(),
            elapsed,
            eta,
        });

        let t = Instant::now();
        let res = build_job(repo, job);
        let dur = t.elapsed();
        durs.push(dur);

        let err = match res {
            Ok(()) => None,
            Err(e) => {
                failures += 1;
                Some(format!("{e:#}"))
            }
        };
        cb(QueueEvent::Done {
            idx,
            label: job.label(),
            dur,
            err,
        });
    }

    cb(QueueEvent::Summary {
        total: n,
        failures,
        elapsed: start.elapsed(),
    });
    if failures > 0 {
        bail!("{failures} of {n} job(s) failed");
    }
    Ok(())
}

/// Plain stdout progress (headless CLI).
fn run_queue_stdout(repo: &Repo, jobs: &[Job]) -> Result<()> {
    if jobs.is_empty() {
        println!("(no jobs)");
        return Ok(());
    }
    let mut cb = |e: QueueEvent| match e {
        QueueEvent::Start {
            idx,
            total,
            label,
            elapsed,
            eta,
        } => {
            let eta_s = match eta {
                Some(d) => format!(", ~{} left", fmt_dur(d)),
                None => String::new(),
            };
            println!(
                "[{idx}/{total}] {label}  (elapsed {}{})",
                fmt_dur(elapsed),
                eta_s
            );
        }
        QueueEvent::Done { label, dur, err, .. } => {
            if let Some(e) = err {
                println!("  \u{2717} {label}: {e}");
            } else {
                println!("  \u{2713} {} ({})", label, fmt_dur(dur));
            }
        }
        QueueEvent::Summary {
            total,
            failures,
            elapsed,
        } => {
            if failures > 0 {
                println!(
                    "\nDone: {} job(s) in {} — {} failed",
                    total,
                    fmt_dur(elapsed),
                    failures
                );
            } else {
                println!("\nDone: {} job(s) in {}", total, fmt_dur(elapsed));
            }
        }
    };
    run_queue(repo, jobs, &mut cb)
}

/// Compact duration: "42s", "3m10s", "1h02m".
pub fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    }
}

// ---------- per-job execution ----------

/// The output file path a job writes to (mirrors `build_one`'s filename logic).
/// Used by deep validation to reuse an already-built artifact instead of
/// rebuilding it.
pub fn job_output_path(repo: &Repo, job: &Job) -> PathBuf {
    let odir = repo.root.join("output").join(&job.slug).join(&job.lang);
    let base = format!("{}-{}", job.slug, job.lang);
    let fname = match job.out {
        Out::RetailEpub => format!("{base}.epub"),
        Out::KdpEpub => format!("{base}-kdp.epub"),
        Out::RetailPdf => format!("{base}.pdf"),
        Out::KdpPdf => match (job.target.as_str(), job.edition.as_deref()) {
            ("bubok", Some(ed)) => format!("{base}-{ed}.pdf"),
            _ => format!("{base}-kdp.pdf"),
        },
        Out::Docx => format!("{base}.docx"),
    };
    odir.join(fname)
}

pub fn build_job(repo: &Repo, job: &Job) -> Result<()> {
    let (book, dir) = repo.load_book_at(&job.dir)?;
    build_one(
        repo,
        &book,
        &dir,
        &job.lang,
        job.out,
        job.epub_px,
        job.geometry,
        job.edition.as_deref(),
        &job.target,
    )
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

#[allow(clippy::too_many_arguments)]
fn build_one(
    repo: &Repo,
    book: &BookConfig,
    dir: &Path,
    lang: &str,
    out: Out,
    epub_px: u32,
    geometry: Option<PageGeometry>,
    edition: Option<&str>,
    target: &str,
) -> Result<()> {
    let slug = &book.slug;
    let content = book
        .content
        .get(lang)
        .with_context(|| format!("no [content.{lang}] for {slug}"))?;
    let chaps = content.resolve(dir)?;
    // copyright pages are optional (the new es-only fables ship without them)
    let cpdf = some_if_exists(dir.join(lang).join("copyright.md"));
    let cepub = some_if_exists(dir.join(lang).join("copyright-epub.md"));
    // cover art is optional (the new es-only fables have no cover/ assets yet)
    let cover = some_if_exists(dir.join("cover").join(format!("front-{lang}.png")));
    let odir = repo.root.join("output").join(slug).join(lang);
    std::fs::create_dir_all(&odir)?;
    // Front-matter values (title/author/lang/rights) resolved from config; shared
    // by every native engine (Typst, EPUB, DOCX).
    let m = resolve_book_meta(repo, book, lang)?;
    let base = format!("{slug}-{lang}");
    let openright = book.pdf.chapter_opens.as_deref() == Some("recto");

    match out {
        Out::RetailEpub => {
            let o = odir.join(format!("{base}.epub"));
            crate::epub_native::run(repo, &m, cepub.as_deref(), &chaps, cover.as_deref(), lang, true, &o)?;
            shrink_epub(repo, &o, epub_px)?;
            if let Some(c) = &cover {
                emit_cover_jpg(c, &odir.join(format!("{base}-cover.jpg")))?;
            }
        }
        Out::KdpEpub => {
            let o = odir.join(format!("{base}-kdp.epub"));
            crate::epub_native::run(repo, &m, cepub.as_deref(), &chaps, cover.as_deref(), lang, false, &o)?;
            shrink_epub(repo, &o, epub_px)?;
            if let Some(c) = &cover {
                emit_cover_jpg(c, &odir.join(format!("{base}-cover.jpg")))?;
            }
        }
        Out::RetailPdf => {
            let pdf = odir.join(format!("{base}.pdf"));
            crate::typst_pdf::run(
                repo,
                &m,
                cpdf.as_deref(),
                &chaps,
                openright,
                true,
                cover.as_deref(),
                geometry,
                lang,
                &pdf,
            )?;
            crate::pages::write_sidecar(&pdf, &chaps);
        }
        Out::KdpPdf => {
            // Default KDP print interior is "{base}-kdp.pdf". Regional POD print
            // editions (bubok) get their own filename so they don't clobber it.
            let fname = match (target, edition) {
                ("bubok", Some(ed)) => format!("{base}-{ed}.pdf"),
                _ => format!("{base}-kdp.pdf"),
            };
            let pdf = odir.join(fname);
            crate::typst_pdf::run(
                repo,
                &m,
                cpdf.as_deref(),
                &chaps,
                openright,
                false,
                None,
                geometry,
                lang,
                &pdf,
            )?;
            crate::pages::write_sidecar(&pdf, &chaps);
        }
        Out::Docx => {
            // Editor review doc — clean Word document via the native docx-rs engine.
            crate::docx_native::run(repo, &m, &chaps, &odir.join(format!("{base}.docx")))?;
        }
    }
    Ok(())
}

/// Resolved per-(book, lang) front-matter values, shared by every native engine
/// (Typst PDF, EPUB, DOCX).
pub struct BookMeta {
    pub title: String,
    pub subtitle: Option<String>,
    pub author: String,
    pub rights: String,
}

/// Resolve title/subtitle/author/rights for a (book, lang) from config.
pub fn resolve_book_meta(repo: &Repo, book: &BookConfig, lang: &str) -> Result<BookMeta> {
    let title = book
        .title
        .get(lang)
        .with_context(|| format!("no [title.{lang}] for {}", book.slug))?
        .clone();
    let subtitle = book.subtitle.get(lang).cloned();
    let author = book
        .meta
        .author
        .clone()
        .or_else(|| repo.config.author.clone())
        .unwrap_or_else(|| "Federico Ramallo".into());
    let date = book.meta.date.clone().or_else(|| repo.config.date.clone());
    let year = date.as_ref().map(date_year).unwrap_or_else(|| "2026".into());
    let rights = book
        .meta
        .rights
        .clone()
        .unwrap_or_else(|| localized_rights(lang, &author, &year));
    Ok(BookMeta { title, subtitle, author, rights })
}

/// Localized "all rights reserved" line, matching the old meta.md wording.
fn localized_rights(lang: &str, author: &str, year: &str) -> String {
    match lang {
        "es" => format!("© {year} {author}. Todos los derechos reservados."),
        _ => format!("© {year} {author}. All rights reserved."),
    }
}

/// Render a TOML date value (integer year, string, or datetime) as a year string.
fn date_year(d: &toml::Value) -> String {
    match d {
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::String(s) => s.clone(),
        toml::Value::Datetime(dt) => dt.to_string(),
        other => other.to_string(),
    }
}

/// Emit the KDP eBook cover JPG (RGB, sRGB) from the front PNG — pure Rust.
/// Errors propagate so a failed emit fails the job instead of silently passing (M4).
pub fn emit_cover_jpg(cover_png: &Path, out: &Path) -> Result<()> {
    if !cover_png.exists() {
        return Ok(());
    }
    let img = image::open(cover_png)
        .with_context(|| format!("cover jpg: reading {}", cover_png.display()))?;
    let rgb = img.to_rgb8(); // drop alpha (front cover is full-bleed/opaque)
    let f = std::fs::File::create(out)
        .with_context(|| format!("cover jpg: creating {}", out.display()))?;
    let mut w = std::io::BufWriter::new(f);
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, 90);
    enc.encode_image(&rgb)
        .with_context(|| format!("cover jpg: encoding {}", out.display()))?;
    Ok(())
}

/// Shrink EPUB images in place (native, Python-free). Errors propagate so a failed
/// shrink (which would ship oversized Kindle-delivery images) fails the job (M4).
fn shrink_epub(_repo: &Repo, epub: &Path, px: u32) -> Result<()> {
    let (before, after) = crate::epub_shrink::shrink_epub(epub, px)
        .with_context(|| format!("shrinking {}", epub.display()))?;
    println!(
        "  {}: {:.1}MB -> {:.1}MB",
        epub.file_name().and_then(|n| n.to_str()).unwrap_or(""),
        before as f64 / 1e6,
        after as f64 / 1e6
    );
    Ok(())
}

fn some_if_exists(p: PathBuf) -> Option<PathBuf> {
    p.exists().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Edition;

    fn repo(s: &str) -> Repo {
        Repo {
            root: PathBuf::from("/tmp/bookmill-test"),
            config: toml::from_str(s).unwrap(),
        }
    }
    fn book(s: &str) -> BookConfig {
        toml::from_str(s).unwrap()
    }

    #[test]
    fn parse_trim_forms() {
        assert_eq!(parse_trim("6x9"), Some((6.0, 9.0)));
        assert_eq!(parse_trim("6x9in"), Some((6.0, 9.0)));
        assert_eq!(parse_trim("5.83x8.27in"), Some((5.83, 8.27)));
        assert_eq!(parse_trim("garbage"), None);
    }

    #[test]
    fn parse_len_forms() {
        assert_eq!(parse_len("0.125in"), Some(0.125));
        assert_eq!(parse_len("0"), Some(0.0));
        assert_eq!(parse_len("x"), None);
    }

    #[test]
    fn kdp_pdf_adds_asymmetric_bleed() {
        // KDP print: width gets ONE bleed (outer edge only), height gets TWO
        // (top + bottom). This asymmetry is the risky rule the audit flagged.
        let r = repo("[defaults]\ntrim = '6x9'\nbleed = '0.125in'\n");
        let b = book("slug = 's'\n");
        let ed = Edition {
            trim: Some("6x9".into()),
            bleed: Some("0.125in".into()),
            ..Default::default()
        };
        let (w, h) = resolve_paper_dims(&r, &b, Some(&ed), Out::KdpPdf).unwrap();
        assert!((w - 6.125).abs() < 1e-9, "w = {w}");
        assert!((h - 9.25).abs() < 1e-9, "h = {h}");
    }

    #[test]
    fn kdp_pdf_bleed_defaults_to_eighth_inch() {
        // No bleed set anywhere → KDP default 0.125in.
        let r = repo("[defaults]\ntrim = '6x9'\n");
        let b = book("slug = 's'\n");
        let (w, h) = resolve_paper_dims(&r, &b, None, Out::KdpPdf).unwrap();
        assert!((w - 6.125).abs() < 1e-9, "w = {w}");
        assert!((h - 9.25).abs() < 1e-9, "h = {h}");
    }

    #[test]
    fn zero_bleed_yields_trim_size() {
        let r = repo("[defaults]\ntrim = '6x9'\nbleed = '0'\n");
        let b = book("slug = 's'\n");
        let (w, h) = resolve_paper_dims(&r, &b, None, Out::KdpPdf).unwrap();
        assert!((w - 6.0).abs() < 1e-9);
        assert!((h - 9.0).abs() < 1e-9);
    }

    #[test]
    fn retail_pdf_has_no_bleed_and_epub_none() {
        let r = repo("[defaults]\ntrim = '5x8'\nbleed = '0.125in'\n");
        let b = book("slug = 's'\n");
        // digital PDF: exactly the trim, no bleed added
        assert_eq!(resolve_paper_dims(&r, &b, None, Out::RetailPdf), Some((5.0, 8.0)));
        // EPUB outputs have no page geometry
        assert_eq!(resolve_paper_dims(&r, &b, None, Out::RetailEpub), None);
        assert_eq!(resolve_paper_dims(&r, &b, None, Out::KdpEpub), None);
    }

    #[test]
    fn trim_falls_back_edition_then_book_then_repo() {
        let r = repo("[defaults]\ntrim = '6x9'\n");
        // book [pdf].trim overrides repo defaults when no edition trim
        let b = book("slug = 's'\n[pdf]\ntrim = '5x8'\n");
        assert_eq!(resolve_paper_dims(&r, &b, None, Out::RetailPdf), Some((5.0, 8.0)));
        // edition trim wins over book
        let ed = Edition { trim: Some("8.5x11".into()), ..Default::default() };
        assert_eq!(
            resolve_paper_dims(&r, &b, Some(&ed), Out::RetailPdf),
            Some((8.5, 11.0))
        );
        // nothing set → built-in 6x9
        let bare_repo = repo("");
        let bare_book = book("slug = 's'\n");
        assert_eq!(
            resolve_paper_dims(&bare_repo, &bare_book, None, Out::RetailPdf),
            Some((6.0, 9.0))
        );
    }

    #[test]
    fn geometry_margins_precedence() {
        // book [pdf.margins] wins per field; else repo [defaults.margins]; else the
        // built-in 6x9 fallback (top/bottom/inner 0.75, outer 0.6, binding 0.375).
        let r = repo(
            "[defaults]\ntrim = '6x9'\nbleed = '0'\n[defaults.margins]\ntop = 1.0\nouter = 0.5\n",
        );
        let b = book("slug = 's'\n[pdf.margins]\ntop = 0.9\n");
        let g = resolve_geometry(&r, &b, None, Out::KdpPdf).unwrap();
        assert!((g.top - 0.9).abs() < 1e-9, "book margin wins: {}", g.top);
        assert!((g.outer - 0.5).abs() < 1e-9, "repo margin used: {}", g.outer);
        assert!((g.inner - 0.75).abs() < 1e-9, "built-in fallback: {}", g.inner);
        assert!((g.bindingoffset - 0.375).abs() < 1e-9);
        assert!((g.pw - 6.0).abs() < 1e-9 && (g.ph - 9.0).abs() < 1e-9);
    }
}
