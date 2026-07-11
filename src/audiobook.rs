//! Audiobook engine (v1): drive the proven `kab` (Kokoro on the Apple Neural
//! Engine) converter from the resolved config — `kab` is the one remaining
//! external renderer (interiors are all native Rust now: Typst PDF, epub-builder
//! EPUB, docx-rs DOCX).
//!
//! A render request expands to one [`AudioJob`] per (book × language). Each job
//! resolves its chapter list through the shared `[content.<lang>]` selection
//! (prepend + glob + files + append, in order), stages those files into a numbered
//! scratch dir (repo-local `.bookmill-tmp/`, OUTSIDE `output/`) so the lexical glob
//! `kab convert` runs preserves bookmill's exact order (including appended epilogues
//! a bare `capitulo-*.md` glob would miss), and feeds them to the [`TtsEngine`].
//! Output is `<slug>-<lang>.m4b`. The transient staging copy and the persistent
//! per-chapter WAV cache both live under the scratch dir — never in `output/`.
//!
//! The trait boundary is deliberate: the content-hash AST *segment cache* (so
//! fixing one line re-renders only that clip) slots in later as a caching engine
//! that wraps [`KabEngine`] — it needs per-segment synth, which `kab convert` does
//! not expose, so v1 renders the whole book each call. See BOOKMILL-PLAN.md.

use crate::config::{default_ane_code, default_voice, Audiobook, AudiobookLang, BookConfig};
use crate::discover::Repo;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
    /// per-job scratch/cache base, OUTSIDE `output/` (staging + WAV cache live
    /// here so they never clutter the deliverable folder). See [`audio_tmp_dir`].
    pub tmp: PathBuf,
}

/// The audiobook scratch/cache base for a book+language: a repo-local hidden dir
/// **outside** `output/` (`<repo>/.bookmill-tmp/audio/<slug>/<lang>/`). Holds the
/// transient `stage/` copy of the chapters and the persistent per-chapter `cache/`
/// of WAVs. Kept local to the repo (not the OS temp dir) so the cache survives
/// reboots and stays tied to this project's content — that's what makes the
/// incremental re-render skip correct across sessions.
pub fn audio_tmp_dir(root: &Path, slug: &str, lang: &str) -> PathBuf {
    root.join(".bookmill-tmp").join("audio").join(slug).join(lang)
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

    Resolved { voice, code, speed, speak_titles, artist, engine }
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
            let tmp = audio_tmp_dir(&repo.root, &book.slug, &lang);
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
                tmp,
            });
        }
    }
    Ok(jobs)
}

// ---------- clean (temp / cache housekeeping) ----------

/// Total bytes under a path (recursively). 0 if it can't be stat'd/read.
fn dir_size(p: &Path) -> u64 {
    let md = match std::fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    if md.is_file() {
        return md.len();
    }
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(p) {
        for e in entries.flatten() {
            total += dir_size(&e.path());
        }
    }
    total
}

/// Remove the audiobook temp/cache artifacts for a book/language — the repo-local
/// scratch base `.bookmill-tmp/audio/<slug>/<lang>/` (its `stage/` + `cache/`), and
/// (with `drop_manifest`) the `.…audiomanifest.json` that sits next to the `.m4b`.
/// Also sweeps the *legacy* in-`output/` `.audiostage`/`.audiocache` dirs left by
/// older bookmill versions. Scoped to one book / language or all of them. The
/// rendered `.m4b` is never touched. Reports how many items were removed and how
/// much disk was freed.
pub fn clean(
    repo: &Repo,
    book_slug: Option<String>,
    lang_filter: Option<String>,
    drop_manifest: bool,
) -> Result<()> {
    let books = match &book_slug {
        Some(s) => vec![repo.find_book(s)?],
        None => {
            let mut v = Vec::new();
            for d in repo.book_dirs()? {
                v.push(repo.load_book_at(&d)?);
            }
            v
        }
    };

    let mut freed: u64 = 0;
    let mut removed = 0usize;
    for (book, _dir) in &books {
        for lang in langs_for(book, &lang_filter) {
            let odir = repo.root.join("output").join(&book.slug).join(&lang);
            // New location (repo-local scratch, outside output/) + legacy
            // in-output dirs from older versions.
            let mut targets = vec![
                audio_tmp_dir(&repo.root, &book.slug, &lang),
                odir.join(".audiostage"),
                odir.join(".audiocache"),
            ];
            if drop_manifest {
                targets.push(odir.join(format!(".{}-{}.audiomanifest.json", book.slug, lang)));
            }
            for t in targets {
                if !t.exists() {
                    continue;
                }
                let sz = dir_size(&t);
                let res = if t.is_dir() {
                    std::fs::remove_dir_all(&t)
                } else {
                    std::fs::remove_file(&t)
                };
                match res {
                    Ok(()) => {
                        freed += sz;
                        removed += 1;
                        println!("  removed {} ({:.1} MB)", t.display(), sz as f64 / 1e6);
                    }
                    Err(e) => println!("  (warning: could not remove {}: {e})", t.display()),
                }
            }
        }
    }

    if removed == 0 {
        println!("nothing to clean (no audiobook temp/cache files found)");
    } else {
        println!("\nCleaned {removed} item(s), freed {:.1} MB", freed as f64 / 1e6);
    }
    Ok(())
}

/// Wipe the whole audiobook tmp tree (`<repo>/.bookmill-tmp/`) in one sweep — the
/// blunt, book-agnostic counterpart to [`clean`]. Removes every book/language's
/// `stage/` + `cache/` at once; never touches the rendered `.m4b` files or their
/// manifests (which live under `output/`). Reports the disk freed.
pub fn clear(repo: &Repo) -> Result<()> {
    let tmp = repo.root.join(".bookmill-tmp");
    if !tmp.exists() {
        println!("nothing to clear (no {} dir)", tmp.display());
        return Ok(());
    }
    let sz = dir_size(&tmp);
    std::fs::remove_dir_all(&tmp)
        .with_context(|| format!("removing {}", tmp.display()))?;
    println!("Cleared {} — freed {:.1} MB", tmp.display(), sz as f64 / 1e6);
    Ok(())
}

// ---------- render manifest (incremental cache + change report) ----------

/// What a rendered `.m4b` was made from: the engine/voice/speed and a content
/// hash per chapter. Persisted next to the `.m4b` so the next run can (a) skip
/// the render when nothing changed and (b) report exactly which chapters changed
/// — the signal for Federico's proofread-by-listening loop.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Manifest {
    engine: String,
    voice: String,
    code: String,
    /// formatted to a fixed precision so float round-trips compare cleanly
    speed: String,
    speak_titles: bool,
    chapters: Vec<ChapterEntry>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct ChapterEntry {
    /// chapter file name (stem shown in change reports)
    file: String,
    /// sha256 of the chapter's bytes
    sha: String,
}

impl Manifest {
    /// Build the manifest for a job by hashing each chapter file.
    fn build(job: &AudioJob, engine: &str) -> Result<Manifest> {
        let mut chapters = Vec::with_capacity(job.chapters.len());
        for p in &job.chapters {
            let bytes = std::fs::read(p)
                .with_context(|| format!("hashing chapter {}", p.display()))?;
            let sha = format!("{:x}", Sha256::digest(&bytes));
            let file = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("chapter.md")
                .to_string();
            chapters.push(ChapterEntry { file, sha });
        }
        Ok(Manifest {
            engine: engine.to_string(),
            voice: job.voice.clone(),
            code: job.code.clone(),
            speed: format!("{:.4}", job.speed),
            speak_titles: job.speak_titles,
            chapters,
        })
    }

    /// True if the engine/voice/speed/titles settings differ (forces a full
    /// re-render regardless of chapter content).
    fn settings_differ(&self, other: &Manifest) -> bool {
        self.engine != other.engine
            || self.voice != other.voice
            || self.code != other.code
            || self.speed != other.speed
            || self.speak_titles != other.speak_titles
    }
}

/// Where a job's manifest is stored (hidden, next to the `.m4b`).
fn manifest_path(job: &AudioJob) -> PathBuf {
    let dir = job.out.parent().unwrap_or_else(|| Path::new("."));
    dir.join(format!(".{}-{}.audiomanifest.json", job.slug, job.lang))
}

fn load_manifest(path: &Path) -> Option<Manifest> {
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

fn save_manifest(path: &Path, m: &Manifest) -> Result<()> {
    let s = serde_json::to_string_pretty(m)?;
    std::fs::write(path, s).with_context(|| format!("writing {}", path.display()))
}

/// Stem of a chapter file name for compact change reports ("capitulo-03").
fn stem(file: &str) -> &str {
    file.strip_suffix(".md").unwrap_or(file)
}

/// Print which chapters changed vs. the previous render (added / removed /
/// edited), so the proofreader knows exactly what to re-listen to.
fn report_changes(old: Option<&Manifest>, new: &Manifest) {
    let Some(old) = old else {
        println!("  first render — {} chapters", new.chapters.len());
        return;
    };
    if old.settings_differ(new) {
        println!("  voice/speed/engine changed → full re-render");
        return;
    }
    let old_by: std::collections::BTreeMap<&str, &str> =
        old.chapters.iter().map(|c| (c.file.as_str(), c.sha.as_str())).collect();
    let new_files: std::collections::BTreeSet<&str> =
        new.chapters.iter().map(|c| c.file.as_str()).collect();

    let mut edited = Vec::new();
    let mut added = Vec::new();
    for c in &new.chapters {
        match old_by.get(c.file.as_str()) {
            Some(&sha) if sha != c.sha => edited.push(stem(&c.file)),
            None => added.push(stem(&c.file)),
            _ => {}
        }
    }
    let removed: Vec<&str> = old
        .chapters
        .iter()
        .filter(|c| !new_files.contains(c.file.as_str()))
        .map(|c| stem(&c.file))
        .collect();

    if edited.is_empty() && added.is_empty() && removed.is_empty() {
        // Content identical but the .m4b was missing — re-rendering anyway.
        println!("  re-rendering (audio missing) — {} chapters", new.chapters.len());
        return;
    }
    let mut parts = Vec::new();
    if !edited.is_empty() {
        parts.push(format!("edited [{}]", edited.join(", ")));
    }
    if !added.is_empty() {
        parts.push(format!("added [{}]", added.join(", ")));
    }
    if !removed.is_empty() {
        parts.push(format!("removed [{}]", removed.join(", ")));
    }
    println!("  changed: {} → re-listen to the edited chapters", parts.join("; "));
}

// ---------- entry point ----------

/// Render audiobooks for a request (optionally one book / one language), with
/// optional voice/speed overrides (mirrors the Makefile's `VOICE=` knob).
///
/// Incremental: each job is hashed into a manifest stored next to its `.m4b`. If
/// nothing changed since the last render (same chapters + voice/speed) and the
/// `.m4b` exists, the job is skipped unless `force` is set. When something did
/// change, the changed chapters are reported before re-rendering.
pub fn run(
    repo: &Repo,
    book_slug: Option<String>,
    lang_filter: Option<String>,
    voice_override: Option<String>,
    speed_override: Option<f64>,
    force: bool,
) -> Result<()> {
    let jobs = plan(repo, &book_slug, &lang_filter, &voice_override, speed_override)?;
    if jobs.is_empty() {
        println!("(no audiobook jobs)");
        return Ok(());
    }
    let engine = KabEngine { bin: resolve_kab_bin(repo.config.audiobook.as_ref(), None) };
    let n = jobs.len();
    let mut failures = 0;
    let mut skipped = 0;
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
        let new_manifest = match Manifest::build(job, engine.name()) {
            Ok(m) => m,
            Err(e) => {
                failures += 1;
                println!("  \u{2717} {} {}: {e:#}", job.slug, job.lang);
                continue;
            }
        };
        let mpath = manifest_path(job);
        let old = load_manifest(&mpath);
        if !force && job.out.exists() && old.as_ref() == Some(&new_manifest) {
            skipped += 1;
            println!(
                "  \u{2713} up to date — {} chapters unchanged (skipped; --force to re-render)",
                new_manifest.chapters.len()
            );
            continue;
        }
        if force {
            println!("  forced re-render — {} chapters", new_manifest.chapters.len());
        } else {
            report_changes(old.as_ref(), &new_manifest);
        }
        match engine.render(job) {
            Ok(()) => {
                if let Err(e) = save_manifest(&mpath, &new_manifest) {
                    println!("  (warning: could not write render manifest: {e:#})");
                }
                println!("  \u{2713} {}", job.out.display());
            }
            Err(e) => {
                failures += 1;
                println!("  \u{2717} {} {}: {e:#}", job.slug, job.lang);
            }
        }
    }
    if failures > 0 {
        bail!("{failures} of {n} audiobook(s) failed");
    }
    let rendered = n - skipped;
    println!("\nDone: {rendered} rendered, {skipped} up to date ({n} total)");
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

        // Persistent per-chapter audio cache, in the repo-local scratch dir
        // (OUTSIDE `output/`). ane_book.py keys each chapter's WAV by a hash of
        // the spoken text (+ voice/lang/speed/model) so a one-line edit
        // re-renders only that chapter on the next run; the cache survives runs.
        let cache_dir = job.tmp.join("cache");
        std::fs::create_dir_all(&cache_dir)
            .with_context(|| format!("creating audio cache dir {}", cache_dir.display()))?;

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
            .arg("--cache-dir")
            .arg(&cache_dir)
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

/// Copy a job's ordered chapter files into a fresh `stage/` dir in the repo-local
/// scratch base (OUTSIDE `output/`), renamed `NNNN-<original>` to lock in reading
/// order for kab's glob. Returns the staging dir.
fn stage_chapters(job: &AudioJob) -> Result<PathBuf> {
    let stage = job.tmp.join("stage");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(file: &str, sha: &str) -> ChapterEntry {
        ChapterEntry { file: file.into(), sha: sha.into() }
    }

    fn manifest(chapters: Vec<ChapterEntry>) -> Manifest {
        Manifest {
            engine: "kab".into(),
            voice: "ef_dora".into(),
            code: "e".into(),
            speed: "1.0000".into(),
            speak_titles: true,
            chapters,
        }
    }

    #[test]
    fn unchanged_manifests_are_equal_so_render_is_skipped() {
        let a = manifest(vec![ch("c01.md", "aaa"), ch("c02.md", "bbb")]);
        let b = manifest(vec![ch("c01.md", "aaa"), ch("c02.md", "bbb")]);
        assert_eq!(a, b, "identical content+settings must compare equal (skip)");
    }

    #[test]
    fn an_edited_chapter_breaks_equality() {
        let a = manifest(vec![ch("c01.md", "aaa"), ch("c02.md", "bbb")]);
        let b = manifest(vec![ch("c01.md", "aaa"), ch("c02.md", "ZZZ")]);
        assert_ne!(a, b, "a changed chapter hash must force a re-render");
    }

    #[test]
    fn settings_change_forces_rerender() {
        let base = manifest(vec![ch("c01.md", "aaa")]);
        let mut diff_voice = manifest(vec![ch("c01.md", "aaa")]);
        diff_voice.voice = "af_heart".into();
        assert!(base.settings_differ(&diff_voice));
        assert_ne!(base, diff_voice);

        let mut diff_speed = manifest(vec![ch("c01.md", "aaa")]);
        diff_speed.speed = "1.1000".into();
        assert!(base.settings_differ(&diff_speed));

        let same = manifest(vec![ch("c01.md", "aaa")]);
        assert!(!base.settings_differ(&same));
    }

    #[test]
    fn stem_strips_md() {
        assert_eq!(stem("capitulo-03.md"), "capitulo-03");
        assert_eq!(stem("epilogo"), "epilogo");
    }

    /// The KabEngine must pass `--cache-dir <job.tmp>/cache` (and create it) so
    /// ane_book.py can reuse unchanged chapters across runs — and that scratch dir
    /// must live OUTSIDE the output folder (next to neither the .m4b). Verified
    /// with a fake `kab` that records its argv and produces the expected .m4b.
    #[test]
    fn render_passes_cache_dir_and_creates_it() {
        use std::io::Write;

        let tmp = std::env::temp_dir().join(format!("bookmill_audio_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // A real chapter file to stage.
        let chap = tmp.join("capitulo-01.md");
        std::fs::write(&chap, "# Capítulo uno\n\nHola.\n").unwrap();

        let out = tmp.join("out").join("slug-es.m4b");
        let scratch = tmp.join("scratch");
        let arglog = tmp.join("argv.txt");

        // Fake kab: dump argv to a file, then create the requested --out file so
        // render()'s post-condition (out exists) passes.
        let fake = tmp.join("fake-kab.sh");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nout=''\nwhile [ $# -gt 0 ]; do if [ \"$1\" = \"--out\" ]; then out=\"$2\"; fi; shift; done\nmkdir -p \"$(dirname \"$out\")\"\necho fake > \"$out\"\nexit 0\n",
            arglog.display()
        );
        let mut f = std::fs::File::create(&fake).unwrap();
        f.write_all(script.as_bytes()).unwrap();
        drop(f);
        let mut perm = std::fs::metadata(&fake).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
        std::fs::set_permissions(&fake, perm).unwrap();

        let job = AudioJob {
            slug: "slug".into(),
            lang: "es".into(),
            title: "T".into(),
            artist: "A".into(),
            voice: "ef_dora".into(),
            code: "e".into(),
            speed: 1.0,
            speak_titles: true,
            chapters: vec![chap],
            out: out.clone(),
            tmp: scratch.clone(),
        };

        let engine = KabEngine { bin: fake };
        engine.render(&job).expect("render should succeed with fake kab");

        let expected_cache = scratch.join("cache");
        assert!(expected_cache.is_dir(), "cache dir must be created");
        assert!(
            !out.parent().unwrap().join(".audiocache").exists(),
            "cache must NOT be created inside the output folder"
        );

        let argv = std::fs::read_to_string(&arglog).unwrap();
        let lines: Vec<&str> = argv.lines().collect();
        let i = lines
            .iter()
            .position(|a| *a == "--cache-dir")
            .expect("--cache-dir must be passed to kab");
        assert_eq!(
            lines[i + 1],
            expected_cache.to_str().unwrap(),
            "--cache-dir must point at <job.tmp>/cache"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
