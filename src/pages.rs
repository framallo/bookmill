//! Source-derived page counts for `bookmill words`.
//!
//! Exact pagination only exists after Typst lays the book out, so the page count
//! is inherently a property of the *built* PDF. But that number goes stale the
//! moment a chapter is edited — `bookmill words` would keep reporting the old
//! PDF's page count until the next rebuild (e.g. `menger-a-milei` showing 37pp
//! for a 50k-word book).
//!
//! To keep the reported page count tracking the *source*, every print-PDF build
//! drops a tiny sidecar next to the PDF recording the `(words, pages)` pair it
//! was laid out from ([`write_sidecar`]). `bookmill words` then reads that
//! book's own layout density (`pages / words`) and rescales it to the *current*
//! source word count ([`resolve`]): identical words → the exact built page
//! count; edited source → a live estimate, flagged, so the number moves with the
//! prose instead of lying until the next build.

use crate::pdfmeta;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The `(words, pages)` a built PDF was laid out from. Serialized as JSON next
/// to the PDF (`<pdf-stem>.pages.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PagesRecord {
    pub words: usize,
    pub pages: u32,
}

/// Sidecar path for a PDF: `foo-kdp.pdf` → `foo-kdp.pages.json`.
pub fn sidecar_path(pdf: &Path) -> PathBuf {
    pdf.with_extension("pages.json")
}

/// Count whitespace-separated tokens across the resolved content files — the
/// same unit `wc -w` uses, so totals match `bookmill words`.
pub fn count_words(files: &[PathBuf]) -> usize {
    files
        .iter()
        .map(|f| {
            std::fs::read_to_string(f)
                .map(|s| s.split_whitespace().count())
                .unwrap_or(0)
        })
        .sum()
}

/// After a print PDF is written, record the `(words, pages)` it was laid out
/// from so `bookmill words` can rescale to edited source. Best-effort: a
/// missing/unreadable PDF or a write error is silently skipped (the count just
/// falls back to reading the PDF directly).
pub fn write_sidecar(pdf: &Path, chaps: &[PathBuf]) {
    let Some(pages) = pdfmeta::page_count(pdf) else { return };
    let words = count_words(chaps);
    let rec = PagesRecord { words, pages };
    if let Ok(json) = serde_json::to_string(&rec) {
        let _ = std::fs::write(sidecar_path(pdf), json);
    }
}

/// Read a PDF's sidecar record, if present and parseable.
pub fn read_sidecar(pdf: &Path) -> Option<PagesRecord> {
    let json = std::fs::read_to_string(sidecar_path(pdf)).ok()?;
    serde_json::from_str(&json).ok()
}

/// A page count resolved for display: either exact (source matches the built
/// layout) or an estimate rescaled from the book's own layout density.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PageCount {
    /// Source word count matches the built PDF — the page count is authoritative.
    Exact(u32),
    /// Source changed since the build — pages rescaled from the book's density.
    Estimate(u32),
    /// Never built (no sidecar and no PDF) — nothing to report.
    Unknown,
}

/// Resolve a page count from a built PDF + the *current* source word count.
///
/// Preference order:
/// 1. Sidecar present → rescale its `pages/words` density to `current_words`;
///    exact when the word counts match, otherwise a flagged estimate.
/// 2. No sidecar but the PDF exists → its raw page count (may be stale; the next
///    build writes a sidecar and upgrades this to source-tracked).
/// 3. Neither → `Unknown`.
pub fn resolve(pdf: &Path, current_words: usize) -> PageCount {
    if let Some(rec) = read_sidecar(pdf) {
        if rec.words == 0 {
            return PageCount::Unknown;
        }
        if rec.words == current_words {
            return PageCount::Exact(rec.pages);
        }
        // Rescale by this book's own layout density (pages per word), rounded.
        let density = rec.pages as f64 / rec.words as f64;
        let est = (current_words as f64 * density).round().max(1.0) as u32;
        return PageCount::Estimate(est);
    }
    match pdfmeta::page_count(pdf) {
        Some(p) => PageCount::Exact(p),
        None => PageCount::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_path_swaps_extension() {
        let p = Path::new("output/x/es/x-es-kdp.pdf");
        assert_eq!(sidecar_path(p), Path::new("output/x/es/x-es-kdp.pages.json"));
    }

    fn resolve_with(rec: PagesRecord, current_words: usize) -> PageCount {
        // Exercise the density logic directly (no PDF/sidecar on disk).
        if rec.words == 0 {
            return PageCount::Unknown;
        }
        if rec.words == current_words {
            return PageCount::Exact(rec.pages);
        }
        let density = rec.pages as f64 / rec.words as f64;
        let est = (current_words as f64 * density).round().max(1.0) as u32;
        PageCount::Estimate(est)
    }

    #[test]
    fn matching_words_are_exact() {
        let rec = PagesRecord { words: 49596, pages: 140 };
        assert_eq!(resolve_with(rec, 49596), PageCount::Exact(140));
    }

    #[test]
    fn edited_source_rescales_by_density() {
        // The bug: a stale 11k-word / 37pp build against 49596 current words.
        let rec = PagesRecord { words: 11000, pages: 37 };
        assert_eq!(resolve_with(rec, 49596), PageCount::Estimate(167));
    }

    #[test]
    fn zero_word_record_is_unknown() {
        let rec = PagesRecord { words: 0, pages: 0 };
        assert_eq!(resolve_with(rec, 100), PageCount::Unknown);
    }
}
