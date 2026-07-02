//! `bookmill words` — word + page counts, native Rust (no external tools).
//!
//! Words are counted over the same content files the build/audiobook engines
//! resolve (`[content.<lang>]`: prepend + glob + files + append), matching the
//! Makefile's `cat <chapters> | wc -w`. Pages come from the already-built print
//! interior PDF via [`crate::pdfmeta::page_count`] (`-kdp.pdf`, falling back to
//! the retail `-<lang>.pdf`); if neither exists we print "—" and hint to build.

use crate::config::BookConfig;
use crate::discover::Repo;
use crate::pdfmeta;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Count whitespace-separated tokens across the resolved content files, the same
/// unit `wc -w` uses (so the totals match the Makefile's `words_book_%`).
fn count_words(files: &[PathBuf]) -> usize {
    files
        .iter()
        .map(|f| {
            std::fs::read_to_string(f)
                .map(|s| s.split_whitespace().count())
                .unwrap_or(0)
        })
        .sum()
}

/// Resolve the interior PDF page count for a book/lang. Prefer the KDP print
/// interior (`-kdp.pdf`); fall back to the retail PDF. `None` if neither built.
fn pages_for(repo: &Repo, slug: &str, lang: &str) -> Option<u32> {
    let odir = repo.root.join("output").join(slug).join(lang);
    let base = format!("{slug}-{lang}");
    let kdp = odir.join(format!("{base}-kdp.pdf"));
    let retail = odir.join(format!("{base}.pdf"));
    pdfmeta::page_count(&kdp).or_else(|| pdfmeta::page_count(&retail))
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
            Ok(files) => count_words(&files),
            Err(e) => {
                println!("  {:<45} {:<3}  (content error: {})", book.slug, lang, e);
                continue;
            }
        };
        let pages = pages_for(repo, &book.slug, &lang)
            .map(|p| format!("{p:>4} pages"))
            .unwrap_or_else(|| "   — pages (build the PDF)".into());
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
