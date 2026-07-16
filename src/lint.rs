//! `bookmill lint` — prose linter, a thin driver over the `prose-lint` library.
//!
//! The linting engine (missing-tilde detection, voseo-aware spelling, canon/house
//! rules, and the offline LanguageTool-style grammar rule engine) lives in the
//! standalone, reusable [`prose_lint`] crate. bookmill resolves book → slug/lang →
//! chapter files and the effective allow-list / forbidden-terms / style from
//! config, then streams `prose_lint`'s findings.
//!
//! Default `bookmill lint` runs tildes + spelling + canon (fast, offline).
//! `bookmill lint --deep` (alias `--languagetool`) additionally runs the grammar
//! pass — the offline rule engine, plus Harper/nlprule if `prose-lint` was built
//! with those features.

use crate::config::{BookConfig, LintConfig, FORBIDDEN_PUBLIC};
use crate::discover::Repo;
use anyhow::{bail, Result};
use prose_lint::{lint_markdown, Kind, Lang, Opts};
use std::collections::HashSet;

/// Default forbidden terms the project always cares about (merged with the
/// `[lint].forbid` config + `FORBIDDEN_PUBLIC`). Matched case-insensitively.
const DEFAULT_FORBID: &[&str] = &["Pingüina", "Animal Farm", "Orwell", "Rebelión en la granja"];

/// Build the effective allow-list (lowercased), forbidden-term list, and
/// percentage-style flag for a book by merging repo-root `[lint]` with book `[lint]`.
fn merged_lint(repo_lint: &LintConfig, book_lint: &LintConfig) -> (HashSet<String>, Vec<String>, bool) {
    let mut ignore = HashSet::new();
    for w in repo_lint.ignore.iter().chain(book_lint.ignore.iter()) {
        let w = w.trim().to_lowercase();
        if !w.is_empty() {
            ignore.insert(w);
        }
    }
    let mut forbid: Vec<String> = Vec::new();
    for t in DEFAULT_FORBID
        .iter()
        .map(|s| s.to_string())
        .chain(FORBIDDEN_PUBLIC.iter().map(|s| s.to_string()))
        .chain(repo_lint.forbid.iter().cloned())
        .chain(book_lint.forbid.iter().cloned())
    {
        let t = t.trim().to_string();
        if !t.is_empty() && !forbid.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
            forbid.push(t);
        }
    }
    let style = book_lint.style.as_deref().or(repo_lint.style.as_deref());
    let allow_percentages = matches!(style, Some("business") | Some("technical"));
    (ignore, forbid, allow_percentages)
}

fn langs_for(book: &BookConfig, lang: &Option<String>) -> Vec<String> {
    match lang {
        Some(l) if l != "all" => vec![l.clone()],
        _ => book.languages.clone(),
    }
}

/// Native prose lint for one book (or all) and one language (or all). When
/// `grammar` is set, the grammar pass runs too (`--deep`). Non-zero exit (via
/// `bail`) when any issue remains, so it can gate a release.
pub fn run(repo: &Repo, book: Option<String>, lang: Option<String>, grammar: bool) -> Result<()> {
    let books = match &book {
        Some(s) => vec![repo.find_book(s)?],
        None => {
            let mut v = Vec::new();
            for d in repo.book_dirs()? {
                v.push(repo.load_book_at(&d)?);
            }
            v
        }
    };

    let mut grand_total = 0usize;
    for (b, dir) in books {
        let (ignore, forbid, allow_percentages) = merged_lint(&repo.config.lint, &b.lint);
        for l in langs_for(&b, &lang) {
            let Some(content) = b.content.get(&l) else {
                println!("  {} [{l}] — no [content.{l}]", b.slug);
                continue;
            };
            let files = match content.resolve(&dir) {
                Ok(f) => f,
                Err(e) => {
                    println!("  {} [{l}] — content error: {e}", b.slug);
                    continue;
                }
            };
            let opts = Opts {
                ignore: ignore.clone(),
                forbid: forbid.clone(),
                allow_percentages,
                grammar,
            };
            let lang_enum = Lang::from_code(&l);

            let mut notes = Vec::new();
            if lang_enum == Lang::Es {
                notes.push("voseo-aware".to_string());
            }
            if grammar {
                notes.push("grammar".to_string());
            }
            if !ignore.is_empty() {
                notes.push(format!("{} allow-listed", ignore.len()));
            }
            let note = if notes.is_empty() { String::new() } else { format!("; {}", notes.join(", ")) };
            println!("\nLinting {}/{l} — {} chapters (prose-lint{note})\n", b.slug, files.len());

            let mut total = 0usize;
            let mut accent_total = 0usize;
            let mut hidden_total = 0usize;
            for path in &files {
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let raw = std::fs::read_to_string(path).unwrap_or_default();
                let report = lint_markdown(&raw, lang_enum, &opts);
                let ptotal: usize = report.findings.iter().map(|f| f.count).sum();
                let paccent: usize = report.findings.iter().filter(|f| f.kind == Kind::Tilde).map(|f| f.count).sum();

                let flag = if paccent > 0 { " [accents!]" } else { "" };
                println!("  {name}: {ptotal} issues ({paccent} accent){flag}");
                for f in &report.findings {
                    let loc = if f.line > 0 { format!(":{}", f.line) } else { String::new() };
                    let rep = if f.suggestion.is_empty() { String::new() } else { format!(" -> {}", f.suggestion) };
                    let times = if f.count > 1 { format!(" (×{})", f.count) } else { String::new() };
                    println!("      [{}{}] {}{}{}", f.kind.tag(), loc, f.text, rep, times);
                    if !f.message.is_empty() {
                        println!("            {}", f.message);
                    }
                    println!("            {}", f.context);
                }
                println!();
                total += ptotal;
                accent_total += paccent;
                hidden_total += report.hidden;
            }
            let suppressed = if hidden_total > 0 { format!(", {hidden_total} allow-listed hidden") } else { String::new() };
            println!(
                "TOTAL: {total} issues across {} chapters ({accent_total} accent/tilde{suppressed}).",
                files.len()
            );
            grand_total += total;
        }
    }

    if grand_total > 0 {
        bail!("lint found {grand_total} issue(s)");
    }
    Ok(())
}
