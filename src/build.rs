//! Build orchestrator (v1): drive the proven pandoc/xelatex toolchain from the
//! resolved config. Supports --format builds and edition/target-driven builds.
//!
//! A build request is expanded up front into a flat list of JOBS
//! (book × edition-or-format × lang × output), then run through a queue with
//! per-job progress + ETA. Native Typst/comrak/epub-builder engines replace the
//! pandoc shell-outs later.

use crate::config::BookConfig;
use crate::discover::Repo;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const EPUB_FROM: &str = "gfm+raw_attribute+attributes+implicit_figures";
const PDF_FROM: &str = "gfm+raw_attribute+attributes-implicit_figures";
const DEFAULT_EPUB_PX: u32 = 1200;

#[derive(Clone, Copy, PartialEq)]
pub enum Out {
    RetailEpub,
    KdpEpub,
    RetailPdf,
    KdpPdf,
}

/// PDF rendering engine. `Pandoc` (default) shells out to `pandoc --pdf-engine=
/// xelatex`; `Typst` renders a native `.typ` document via the `typst` CLI
/// (opt-in via `--engine typst`). Only affects PDF outputs; EPUB is unchanged.
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub enum Engine {
    #[default]
    Pandoc,
    Typst,
}

impl Engine {
    /// Parse `--engine` (case-insensitive). None / "pandoc" => Pandoc; "typst" => Typst.
    pub fn parse(s: Option<&str>) -> Result<Engine> {
        match s.map(|x| x.to_ascii_lowercase()).as_deref() {
            None | Some("pandoc") | Some("xelatex") => Ok(Engine::Pandoc),
            Some("typst") => Ok(Engine::Typst),
            Some(other) => bail!("unknown --engine {other:?} (expected \"pandoc\" or \"typst\")"),
        }
    }
}

impl Out {
    pub fn name(self) -> &'static str {
        match self {
            Out::RetailEpub => "RetailEpub",
            Out::KdpEpub => "KdpEpub",
            Out::RetailPdf => "RetailPdf",
            Out::KdpPdf => "KdpPdf",
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
    /// PDF engine (pandoc default; typst opt-in). Ignored for EPUB outputs.
    pub engine: Engine,
}

/// Full print page geometry, in inches. Paper size comes from edition trim+bleed;
/// margins from book [pdf.margins] / repo [defaults.margins]. Emitted into the
/// generated `.geometry.tex` header (replaces the old meta.md `geometry:` block).
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
        total: usize,
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
                    engine: Engine::default(),
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
            let ed = repo.config.editions.get(ed_name);
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
                        engine: Engine::default(),
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

/// Page count of a PDF via `pdfinfo`. None if the file is missing or unreadable
/// (so callers can treat "not built yet" distinctly from an out-of-range count).
fn pdf_page_count(pdf: &Path) -> Option<u32> {
    if !pdf.exists() {
        return None;
    }
    let out = Command::new("pdfinfo").arg(pdf).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find_map(|l| l.strip_prefix("Pages:").and_then(|r| r.trim().parse::<u32>().ok()))
}

// ---------- entry points ----------

/// --format based build (optionally limited to one language).
pub fn run(
    repo: &Repo,
    book_slug: Option<String>,
    format: Option<String>,
    lang_filter: Option<String>,
    engine: Engine,
) -> Result<()> {
    let mut jobs = plan_format(repo, &book_slug, &format, &lang_filter)?;
    for j in &mut jobs {
        j.engine = engine;
    }
    run_queue_stdout(repo, &jobs)
}

/// Edition/target driven build.
pub fn run_editions(
    repo: &Repo,
    book_slug: Option<String>,
    lang_filter: Option<String>,
    edition_filter: Option<String>,
    engine: Engine,
) -> Result<()> {
    let mut jobs = plan_editions(repo, &book_slug, &lang_filter, &edition_filter)?;
    for j in &mut jobs {
        j.engine = engine;
    }
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
            total: n,
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
        job.engine,
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
    engine: Engine,
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
    // Pandoc metadata is generated from config (no per-book/lang meta.md).
    let meta = write_metadata(repo, book, lang, &odir)
        .with_context(|| format!("generating metadata for {slug} [{lang}]"))?;
    let base = format!("{slug}-{lang}");
    let openright = book.pdf.chapter_opens.as_deref() == Some("recto");

    match out {
        Out::RetailEpub => {
            let o = odir.join(format!("{base}.epub"));
            run_epub(repo, &meta, cepub.as_deref(), &chaps, cover.as_deref(), lang, true, &o)?;
            shrink_epub(repo, &o, epub_px);
            if let Some(c) = &cover {
                emit_cover_jpg(c, &odir.join(format!("{base}-cover.jpg")));
            }
        }
        Out::KdpEpub => {
            let o = odir.join(format!("{base}-kdp.epub"));
            run_epub(repo, &meta, cepub.as_deref(), &chaps, cover.as_deref(), lang, false, &o)?;
            shrink_epub(repo, &o, epub_px);
            if let Some(c) = &cover {
                emit_cover_jpg(c, &odir.join(format!("{base}-cover.jpg")));
            }
        }
        Out::RetailPdf if engine == Engine::Typst => {
            let m = resolve_book_meta(repo, book, lang)?;
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
                &odir.join(format!("{base}.pdf")),
            )?;
        }
        Out::KdpPdf if engine == Engine::Typst => {
            let fname = match (target, edition) {
                ("bubok", Some(ed)) => format!("{base}-{ed}.pdf"),
                _ => format!("{base}-kdp.pdf"),
            };
            let m = resolve_book_meta(repo, book, lang)?;
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
                &odir.join(fname),
            )?;
        }
        Out::RetailPdf => {
            // only stamp a cover page when the book has cover art
            let cover_tex = cover.as_ref().map(|c| {
                let ct = odir.join(".cover.tex");
                let _ = std::fs::write(
                    &ct,
                    format!(
                        "\\def\\coverimagepath{{{}}}\n",
                        c.canonicalize().unwrap_or_else(|_| c.clone()).display()
                    ),
                );
                ct
            });
            run_pdf(
                repo,
                &meta,
                cpdf.as_deref(),
                &chaps,
                openright,
                true,
                cover_tex.as_deref(),
                geometry,
                &odir.join(format!("{base}.pdf")),
            )?;
        }
        Out::KdpPdf => {
            // Default KDP print interior is "{base}-kdp.pdf". Regional POD print
            // editions (bubok) get their own filename so they don't clobber it.
            let fname = match (target, edition) {
                ("bubok", Some(ed)) => format!("{base}-{ed}.pdf"),
                _ => format!("{base}-kdp.pdf"),
            };
            run_pdf(
                repo,
                &meta,
                cpdf.as_deref(),
                &chaps,
                openright,
                false,
                None,
                geometry,
                &odir.join(fname),
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_epub(
    repo: &Repo,
    meta: &Path,
    cepub: Option<&Path>,
    chaps: &[PathBuf],
    cover: Option<&Path>,
    lang: &str,
    toc: bool,
    out: &Path,
) -> Result<()> {
    let mut c = Command::new("pandoc");
    c.current_dir(&repo.root)
        .args(["-f", EPUB_FROM, "--to=epub3", "--top-level-division=chapter"])
        .arg("--template=templates/epub.html")
        .arg("--lua-filter=scripts/drop-spot-epub.lua")
        .arg("--css=css/epub.css")
        .args(["--metadata", &format!("lang={lang}")]);
    if let Some(cv) = cover {
        c.arg(format!("--epub-cover-image={}", cv.display()));
    }
    if toc {
        c.args(["--toc", "--toc-depth=1"]);
    }
    c.arg("-o").arg(out).arg(meta);
    if let Some(cp) = cepub {
        c.arg(cp);
    }
    c.args(chaps);
    sh(c, "pandoc epub")
}

#[allow(clippy::too_many_arguments)]
fn run_pdf(
    repo: &Repo,
    meta: &Path,
    cpdf: Option<&Path>,
    chaps: &[PathBuf],
    openright: bool,
    retail: bool,
    cover_tex: Option<&Path>,
    geometry: Option<PageGeometry>,
    out: &Path,
) -> Result<()> {
    let classopt = format!("twoside,{}", if openright { "openright" } else { "openany" });
    let mut c = Command::new("pandoc");
    c.current_dir(&repo.root)
        .args(["-f", PDF_FROM, "--pdf-engine=xelatex", "--to=latex"])
        .args(["-V", &format!("classoption={classopt}")])
        .arg("--top-level-division=chapter");
    // Full page geometry from config (paper size + margins). Loads the geometry
    // package and sets every dimension here, so layout no longer needs meta.md's
    // `geometry:` block. Emitted before the other header includes.
    if let Some(g) = geometry {
        let geo = out.parent().unwrap_or_else(|| Path::new(".")).join(".geometry.tex");
        std::fs::write(
            &geo,
            format!(
                "\\usepackage{{geometry}}\n\\geometry{{paperwidth={:.4}in,paperheight={:.4}in,\
top={:.4}in,bottom={:.4}in,inner={:.4}in,outer={:.4}in,bindingoffset={:.4}in}}\n",
                g.pw, g.ph, g.top, g.bottom, g.inner, g.outer, g.bindingoffset
            ),
        )?;
        c.arg(format!("--include-in-header={}", geo.display()));
    }
    c.arg("--include-in-header=pdf/float-here.tex")
        .arg("--include-in-header=pdf/facing-plate.tex")
        .arg("--include-in-header=pdf/chapter-title.tex")
        .arg("--lua-filter=pdf/facing-plate.lua");
    if retail {
        c.arg("--include-in-header=pdf/texture-bg.tex");
        // only stamp the cover page when the book actually has cover art
        if let Some(ct) = cover_tex {
            c.arg(format!("--include-in-header={}", ct.display()));
            c.arg("--include-in-header=pdf/cover-page.tex");
        }
    }
    c.arg("-o").arg(out).arg(meta);
    if let Some(cp) = cpdf {
        c.arg(cp);
    }
    c.args(chaps);
    sh(c, "pandoc pdf")
}

/// Generate the pandoc metadata file for one (book, lang) from config, replacing
/// the old per-book/lang `meta.md`. Writes a `.gen-meta.md` YAML block into the
/// output dir and returns its path. Carries title/subtitle/author/date/lang/
/// language/documentclass/rights; page geometry is handled separately (the PDF
/// `.geometry.tex` header), so it is intentionally NOT emitted here.
/// Resolved per-(book, lang) front-matter values, shared by the pandoc metadata
/// file and the native Typst document.
pub struct BookMeta {
    pub title: String,
    pub subtitle: Option<String>,
    pub author: String,
    pub year: String,
    pub rights: String,
}

/// Resolve title/subtitle/author/year/rights for a (book, lang) from config,
/// applying the same precedence the pandoc metadata uses.
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
    Ok(BookMeta { title, subtitle, author, year, rights })
}

fn write_metadata(repo: &Repo, book: &BookConfig, lang: &str, odir: &Path) -> Result<PathBuf> {
    let m = resolve_book_meta(repo, book, lang)?;
    let (title, subtitle, author, year, rights) =
        (&m.title, m.subtitle.as_ref(), &m.author, &m.year, &m.rights);

    let mut y = String::from("---\n");
    y.push_str(&format!("title: {}\n", yaml_str(title)));
    if let Some(s) = subtitle {
        y.push_str(&format!("subtitle: {}\n", yaml_str(s)));
    }
    y.push_str(&format!("author: {}\n", yaml_str(author)));
    y.push_str(&format!("date: {}\n", yaml_str(year)));
    y.push_str(&format!("lang: {lang}\n"));
    y.push_str(&format!("language: {lang}\n"));
    y.push_str("documentclass: book\n");
    y.push_str(&format!("rights: {}\n", yaml_str(rights)));
    y.push_str("---\n");

    let p = odir.join(".gen-meta.md");
    std::fs::write(&p, y).with_context(|| format!("writing {}", p.display()))?;
    Ok(p)
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

/// Quote a string as a YAML double-quoted scalar (escapes `\` and `"`).
fn yaml_str(s: &str) -> String {
    let esc = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{esc}\"")
}

/// Emit the KDP eBook cover JPG (RGB, sRGB) from the front PNG — pure Rust.
pub fn emit_cover_jpg(cover_png: &Path, out: &Path) {
    if !cover_png.exists() {
        return;
    }
    match image::open(cover_png) {
        Ok(img) => {
            let rgb = img.to_rgb8(); // drop alpha (front cover is full-bleed/opaque)
            match std::fs::File::create(out) {
                Ok(f) => {
                    let mut w = std::io::BufWriter::new(f);
                    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, 90);
                    if let Err(e) = enc.encode_image(&rgb) {
                        eprintln!("  (cover jpg encode failed: {e})");
                    }
                }
                Err(e) => eprintln!("  (cover jpg create failed: {e})"),
            }
        }
        Err(e) => eprintln!("  (cover jpg skipped, can't read {}: {e})", cover_png.display()),
    }
}

fn shrink_epub(_repo: &Repo, epub: &Path, px: u32) {
    // Native (Python-free) image shrink; mirrors scripts/shrink-epub-images.py.
    match crate::epub_shrink::shrink_epub(epub, px) {
        Ok((before, after)) => println!(
            "  {}: {:.1}MB -> {:.1}MB",
            epub.file_name().and_then(|n| n.to_str()).unwrap_or(""),
            before as f64 / 1e6,
            after as f64 / 1e6
        ),
        Err(e) => eprintln!("  (epub shrink failed for {}: {e:#})", epub.display()),
    }
}

fn some_if_exists(p: PathBuf) -> Option<PathBuf> {
    p.exists().then_some(p)
}

fn sh(mut c: Command, what: &str) -> Result<()> {
    let st = c.status().with_context(|| format!("running {what}"))?;
    if !st.success() {
        bail!("{what} failed");
    }
    Ok(())
}
