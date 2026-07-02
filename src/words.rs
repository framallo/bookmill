//! `bookmill words` — word + page counts, native Rust (no external tools).
//!
//! Words are counted over the same content files the build/audiobook engines
//! resolve (`[content.<lang>]`: prepend + glob + files + append), matching the
//! Makefile's `cat <chapters> | wc -w`. Pages are **source-derived**: each print
//! build drops a `(words, pages)` sidecar next to its PDF, and the count here
//! rescales that book's own layout density to the *current* word count (see
//! [`crate::pages`]) — so an edited chapter shows a live `~estimate` instead of
//! the last build's stale page count. It falls back to the raw PDF page count
//! when no sidecar exists yet, and prints "—" when nothing is built.

use crate::config::BookConfig;
use crate::discover::Repo;
use crate::pages::{self, PageCount};
use anyhow::Result;
use std::path::Path;

/// Resolve the display page count for a book/lang from the current source word
/// count. Prefer the KDP print interior (`-kdp.pdf`); fall back to the retail
/// PDF. Uses the build sidecar to rescale to edited source when present.
fn pages_for(repo: &Repo, slug: &str, lang: &str, words: usize) -> PageCount {
    let odir = repo.root.join("output").join(slug).join(lang);
    let base = format!("{slug}-{lang}");
    let kdp = odir.join(format!("{base}-kdp.pdf"));
    match pages::resolve(&kdp, words) {
        PageCount::Unknown => pages::resolve(&odir.join(format!("{base}.pdf")), words),
        found => found,
    }
}

/// Languages to report for a book, honoring the `--lang` filter (else all).
fn langs_for(book: &BookConfig, lang: &Option<String>) -> Vec<String> {
    match lang {
        Some(l) if l != "all" => vec![l.clone()],
        _ => book.languages.clone(),
    }
}

fn report_book(repo: &Repo, book: &BookConfig, dir: &Path, lang_filter: &Option<String>) {
    for lang in langs_for(book, lang_filter) {
        let Some(content) = book.content.get(&lang) else {
            println!("  {:<45} {:<3}  (no [content.{}])", book.slug, lang, lang);
            continue;
        };
        let words = match content.resolve(dir) {
            Ok(files) => pages::count_words(&files),
            Err(e) => {
                println!("  {:<45} {:<3}  (content error: {})", book.slug, lang, e);
                continue;
            }
        };
        let pages = match pages_for(repo, &book.slug, &lang, words) {
            PageCount::Exact(p) => format!("{p:>4} pages"),
            PageCount::Estimate(p) => format!("{p:>4} pages ~"),
            PageCount::Unknown => "   — pages (build the PDF)".into(),
        };
        println!("  {:<45} {:<3}  {:>6} words   {}", book.slug, lang, words, pages);
    }
}

/// Print word + page counts for one book (or all) and one language (or all).
pub fn run(repo: &Repo, book: Option<String>, lang: Option<String>) -> Result<()> {
    match book {
        Some(s) => {
            let (b, dir) = repo.find_book(&s)?;
            report_book(repo, &b, &dir, &lang);
        }
        None => {
            for dir in repo.book_dirs()? {
                if let Ok((b, d)) = repo.load_book_at(&dir) {
                    report_book(repo, &b, &d, &lang);
                }
            }
        }
    }
    Ok(())
}
