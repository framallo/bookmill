//! `bookmill lint` and `bookmill kdp` — thin wrappers over the repo's Python
//! helper scripts, mirroring the Makefile's `lint_book_%` / `kdp_book_%` targets.
//!
//! These are NON-build, review-time actions (like `validate --deep` → epubcheck
//! or `audiobook` → kab), so shelling out to one external tool is consistent
//! with the rest of bookmill. Both scripts live in `<repo>/scripts/` and read
//! the book repo's own config; bookmill just resolves book→slug/lang and streams
//! the tool's output.
//!
//! - **lint** → `scripts/lint-prose.py <slug> <lang>` (LanguageTool: grammar +
//!   Spanish tildes). Requires `pip install language-tool-python` (offline needs
//!   a JRE; else it falls back to the public LanguageTool HTTP API).
//! - **kdp** → `scripts/kdp-metadata.py {scaffold|check} <slug>` (KDP listing:
//!   title/subtitle, 7 keywords, BISAC, blurb, reading age). Scaffolds
//!   `kdp/<slug>.md` when missing, else validates it against KDP limits.
//!
//! Native `bookmill validate` already checks the listing *limits* (keyword count
//! /length, blurb length, BISAC count, forbidden phrases — see
//! `config::validate_book`), but it does not scaffold or parse the standalone
//! `kdp/<slug>.md` worksheet, so `kdp` remains a distinct scaffold/check action.

use crate::config::BookConfig;
use crate::discover::Repo;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Languages to act on for a book, honoring `--lang` (else all).
fn langs_for(book: &BookConfig, lang: &Option<String>) -> Vec<String> {
    match lang {
        Some(l) if l != "all" => vec![l.clone()],
        _ => book.languages.clone(),
    }
}

/// Resolve the requested books (one slug or all discovered).
fn books(repo: &Repo, slug: &Option<String>) -> Result<Vec<(BookConfig, PathBuf)>> {
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

/// Locate a repo helper script, erroring clearly if the repo doesn't ship it.
fn script(repo: &Repo, name: &str) -> Result<PathBuf> {
    let p = repo.root.join("scripts").join(name);
    if !p.exists() {
        bail!(
            "this repo has no scripts/{name} — bookmill's `{}` shells out to it \
             (see the Isla de la Libertad repo for the reference implementation)",
            name.trim_end_matches(".py")
        );
    }
    Ok(p)
}

/// Run a Python helper script from the repo root, streaming its output.
/// Returns Ok even on a non-zero exit (linters flag issues via exit code); the
/// caller keeps going across books, matching the Makefile's `|| true`.
fn run_py(root: &Path, script: &Path, args: &[&str]) -> Result<()> {
    println!("$ python3 {} {}", script.display(), args.join(" "));
    let status = Command::new("python3")
        .arg(script)
        .args(args)
        .current_dir(root)
        .status()
        .with_context(|| format!("running python3 {}", script.display()))?;
    if !status.success() {
        eprintln!(
            "  (script exited {})",
            status.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())
        );
    }
    Ok(())
}

/// `bookmill lint [book] [--lang]` — LanguageTool prose lint per book/lang.
pub fn lint(repo: &Repo, book: Option<String>, lang: Option<String>) -> Result<()> {
    let script = script(repo, "lint-prose.py")?;
    for (b, _dir) in books(repo, &book)? {
        for l in langs_for(&b, &lang) {
            run_py(&repo.root, &script, &[&b.slug, &l])?;
        }
    }
    Ok(())
}

/// `bookmill kdp [book]` — scaffold `kdp/<slug>.md` when missing, else check it
/// against KDP limits (mirrors `kdp_book_%`).
pub fn kdp(repo: &Repo, book: Option<String>) -> Result<()> {
    let script = script(repo, "kdp-metadata.py")?;
    match book {
        // `all`: scaffold every missing worksheet, then check them all.
        None => {
            run_py(&repo.root, &script, &["scaffold", "all"])?;
            run_py(&repo.root, &script, &["check", "all"])?;
        }
        Some(ref s) if s == "all" => {
            run_py(&repo.root, &script, &["scaffold", "all"])?;
            run_py(&repo.root, &script, &["check", "all"])?;
        }
        Some(s) => {
            let (b, _dir) = repo.find_book(&s)?;
            let worksheet = repo.root.join("kdp").join(format!("{}.md", b.slug));
            let sub = if worksheet.exists() { "check" } else { "scaffold" };
            run_py(&repo.root, &script, &[sub, &b.slug])?;
        }
    }
    Ok(())
}
