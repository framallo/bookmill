//! Audiobook engine (v1): drive the proven `kab` (Kokoro on the Apple Neural
//! Engine) converter from the resolved config — the same "shell out to the proven
//! toolchain in v1, go native later" approach `build.rs` takes with pandoc/xelatex.
//!
//! A render request expands to one [`AudioJob`] per (book × language). Each job
//! resolves its chapter list through the shared `[content.<lang>]` selection
//! (prepend + glob + files + append, in order), stages those files into a numbered
//! temp dir so the lexical glob `kab convert` runs preserves bookmill's exact
//! order (including appended epilogues a bare `capitulo-*.md` glob would miss),
//! and feeds them to the [`TtsEngine`]. Output is `<slug>-<lang>.m4b`.
//!
//! The trait boundary is deliberate: the content-hash AST *segment cache* (so
//! fixing one line re-renders only that clip) slots in later as a caching engine
//! that wraps [`KabEngine`] — it needs per-segment synth, which `kab convert` does
//! not expose, so v1 renders the whole book each call. See BOOKMILL-PLAN.md.

use crate::config::{default_ane_code, default_voice, Audiobook, AudiobookLang, BookConfig};
use crate::discover::Repo;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// A pluggable text-to-speech backend. `kab` is the only built-in today.
pub trait TtsEngine {
    fn name(&self) -> &str;
    /// Render one job to its `out` .m4b path.
    fn render(&self, job: &AudioJob) -> Result<()>;
}

/// One audiobook to render: a resolved chapter list + voice/metadata + output path.
pub struct AudioJob {
    pub slug: String,
    pub lang: String,
    /// album/title metadata (the book's title in this language)
    pub title: String,
    pub artist: String,
    pub voice: String,
    /// ane_book language code (a/e/f/i/p)
    pub code: String,
    pub speed: f64,
    /// speak the chapter heading aloud (false => chapter markers only)
    pub speak_titles: bool,
    /// ordered chapter files (prepend + glob + files + append), absolute paths
    pub chapters: Vec<PathBuf>,
    pub out: PathBuf,
}

// ---------- config resolution ----------

/// The resolved per-language audiobook settings (repo defaults <- book overrides
/// <- per-language subtable <- built-in defaults).
struct Resolved {
    voice: String,
    code: String,
    speed: f64,
    speak_titles: bool,
    artist: String,
    engine: String,
    kab_bin: PathBuf,
}

/// Merge repo + book `[audiobook]` and a language into final settings.
fn resolve(repo: &Repo, book: &BookConfig, lang: &str) -> Resolved {
    let rep = repo.config.audiobook.as_ref();
    let bok = book.audiobook.as_ref();
    let rl: Option<&AudiobookLang> = rep.and_then(|a| a.lang.get(lang));
    let bl: Option<&AudiobookLang> = bok.and_then(|a| a.lang.get(lang));

    // book lang -> repo lang -> built-in default
    let voice = bl
        .and_then(|l| l.voice.clone())
        .or_else(|| rl.and_then(|l| l.voice.clone()))
        .unwrap_or_else(|| default_voice(lang).to_string());
    let code = bl
        .and_then(|l| l.code.clone())
        .or_else(|| rl.and_then(|l| l.code.clone()))
        .unwrap_or_else(|| default_ane_code(lang).to_string());
    // per-lang speed -> book top speed -> repo lang speed -> repo top speed -> 1.0
    let speed = bl
        .and_then(|l| l.speed)
        .or_else(|| bok.and_then(|a| a.speed))
        .or_else(|| rl.and_then(|l| l.speed))
        .or_else(|| rep.and_then(|a| a.speed))
        .unwrap_or(1.0);
    let speak_titles = bl
        .and_then(|l| l.speak_titles)
        .or_else(|| bok.and_then(|a| a.speak_titles))
        .or_else(|| rl.and_then(|l| l.speak_titles))
        .or_else(|| rep.and_then(|a| a.speak_titles))
        .unwrap_or(true);
    let artist = bok
        .and_then(|a| a.artist.clone())
        .or_else(|| rep.and_then(|a| a.artist.clone()))
        .or_else(|| book.meta.author.clone())
        .or_else(|| repo.config.author.clone())
        .unwrap_or_else(|| "Federico Ramallo".into());
    let engine = bok
        .and_then(|a| a.engine.clone())
        .or_else(|| rep.and_then(|a| a.engine.clone()))
        .unwrap_or_else(|| "kab".into());
    let kab_bin = resolve_kab_bin(rep, bok);

    Resolved { voice, code, speed, speak_titles, artist, engine, kab_bin }
}

/// Locate the kab binary: env `KAB`/`BOOKMILL_KAB` -> book/repo `kab_bin` ->
/// the known build path -> bare `kab` (resolved on PATH at spawn time).
fn resolve_kab_bin(rep: Option<&Audiobook>, bok: Option<&Audiobook>) -> PathBuf {
    if let Some(p) = std::env::var_os("KAB").or_else(|| std::env::var_os("BOOKMILL_KAB")) {
        return PathBuf::from(p);
    }
    if let Some(p) = bok.and_then(|a| a.kab_bin.clone()).or_else(|| rep.and_then(|a| a.kab_bin.clone())) {
        return PathBuf::from(p);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let built = Path::new(&home).join("work/libs/kokoro-audiobook-mcp/.build/release/kab");
        if built.exists() {
            return built;
        }
    }
    PathBuf::from("kab")
}

// ---------- job planning ----------

fn langs_for(book: &BookConfig, lang_filter: &Option<String>) -> Vec<String> {
    match lang_filter {
        Some(l) if l != "all" => vec![l.clone()],
        _ => book.languages.clone(),
    }
}

/// Build the [`AudioJob`]s for a render request.
fn plan(
    repo: &Repo,
    book_slug: &Option<String>,
    lang_filter: &Option<String>,
    voice_override: &Option<String>,
    speed_override: Option<f64>,
) -> Result<Vec<AudioJob>> {
    let books = match book_slug {
        Some(s) => vec![repo.find_book(s)?],
        None => {
            let mut v = Vec::new();
            for d in repo.book_dirs()? {
                v.push(repo.load_book_at(&d)?);
            }
            v
        }
    };

    let mut jobs = Vec::new();
    for (book, dir) in books {
        for lang in langs_for(&book, lang_filter) {
            let content = book
                .content
                .get(&lang)
                .with_context(|| format!("no [content.{lang}] for {}", book.slug))?;
            let chapters = content
                .resolve(&dir)
                .with_context(|| format!("resolving chapters for {} [{lang}]", book.slug))?;
            let title = book
                .title
                .get(&lang)
                .cloned()
                .unwrap_or_else(|| book.slug.clone());
            let r = resolve(repo, &book, &lang);
            if r.engine != "kab" {
                bail!(
                    "{} [{lang}]: audiobook engine {:?} not supported (only \"kab\")",
                    book.slug,
                    r.engine
                );
            }
            let out = repo
                .root
                .join("output")
                .join(&book.slug)
                .join(&lang)
                .join(format!("{}-{}.m4b", book.slug, lang));
            jobs.push(AudioJob {
                slug: book.slug.clone(),
                lang: lang.clone(),
                title,
                artist: r.artist,
                voice: voice_override.clone().unwrap_or(r.voice),
                code: r.code,
                speed: speed_override.unwrap_or(r.speed),
                speak_titles: r.speak_titles,
                chapters,
                out,
            });
        }
    }
    Ok(jobs)
}

// ---------- entry point ----------

/// Render audiobooks for a request (optionally one book / one language), with
/// optional voice/speed overrides (mirrors the Makefile's `VOICE=` knob).
pub fn run(
    repo: &Repo,
    book_slug: Option<String>,
    lang_filter: Option<String>,
    voice_override: Option<String>,
    speed_override: Option<f64>,
) -> Result<()> {
    let jobs = plan(repo, &book_slug, &lang_filter, &voice_override, speed_override)?;
    if jobs.is_empty() {
        println!("(no audiobook jobs)");
        return Ok(());
    }
    let engine = KabEngine { bin: resolve_kab_bin(repo.config.audiobook.as_ref(), None) };
    let n = jobs.len();
    let mut failures = 0;
    for (i, job) in jobs.iter().enumerate() {
        println!(
            "[{}/{n}] audiobook {} {} · {} ({} chapters, voice {})",
            i + 1,
            job.slug,
            job.lang,
            engine.name(),
            job.chapters.len(),
            job.voice
        );
        match engine.render(job) {
            Ok(()) => println!("  \u{2713} {}", job.out.display()),
            Err(e) => {
                failures += 1;
                println!("  \u{2717} {} {}: {e:#}", job.slug, job.lang);
            }
        }
    }
    if failures > 0 {
        bail!("{failures} of {n} audiobook(s) failed");
    }
    println!("\nDone: {n} audiobook(s)");
    Ok(())
}

// ---------- kab engine ----------

/// The default engine: shells out to `kab convert` (Kokoro on the ANE).
pub struct KabEngine {
    pub bin: PathBuf,
}

impl TtsEngine for KabEngine {
    fn name(&self) -> &str {
        "kab"
    }

    fn render(&self, job: &AudioJob) -> Result<()> {
        if job.chapters.is_empty() {
            bail!("no chapters to render for {} [{}]", job.slug, job.lang);
        }
        if let Some(parent) = job.out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Stage the resolved chapter list with zero-padded numeric prefixes so
        // kab's lexical `sorted(glob(...))` reproduces bookmill's exact order.
        let stage = stage_chapters(job)?;

        let mut c = Command::new(&self.bin);
        c.arg("convert")
            .arg("--chapters-dir")
            .arg(&stage)
            .args(["--glob", "*.md"])
            .args(["--voice", &job.voice])
            .args(["--lang", &job.code])
            .args(["--title", &job.title])
            .args(["--artist", &job.artist])
            .arg("--out")
            .arg(&job.out)
            .args(["--speed", &format!("{}", job.speed)]);
        if !job.speak_titles {
            c.arg("--drop-title");
        }

        let status = c
            .status()
            .with_context(|| format!("running {} convert", self.bin.display()))?;
        // Clean the staging dir regardless of outcome.
        let _ = std::fs::remove_dir_all(&stage);
        if !status.success() {
            bail!("kab convert failed for {} [{}]", job.slug, job.lang);
        }
        if !job.out.exists() {
            bail!("kab reported success but no file at {}", job.out.display());
        }
        Ok(())
    }
}

/// Copy a job's ordered chapter files into a fresh `.audiostage` dir under the
/// output folder, renamed `NNNN-<original>` to lock in reading order for kab's
/// glob. Returns the staging dir.
fn stage_chapters(job: &AudioJob) -> Result<PathBuf> {
    let stage = job
        .out
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".audiostage");
    // Start clean so a previous run's files can't leak into this one.
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)
        .with_context(|| format!("creating staging dir {}", stage.display()))?;
    for (i, src) in job.chapters.iter().enumerate() {
        let name = src
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("chapter.md");
        let dst = stage.join(format!("{:04}-{}", i, name));
        std::fs::copy(src, &dst)
            .with_context(|| format!("staging {} -> {}", src.display(), dst.display()))?;
    }
    Ok(stage)
}
