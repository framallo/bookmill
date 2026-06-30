//! Native PDF metadata — page count and first-page size — via the `lopdf` crate.
//!
//! Replaces shelling out to poppler's `pdfinfo`. Pure Rust, no external binary on
//! `$PATH`. Both helpers return `None` on a missing file or parse error, so callers
//! can treat that the same as "not built yet / unreadable" (the old `pdfinfo`
//! wrappers did the same).
//!
//! Note: the interior image-DPI / bleed-coverage audits in `deep.rs` still use
//! poppler's `pdfimages -list` — placed-image effective ppi isn't something lopdf
//! surfaces cheaply, and `validate --deep` already needs epubcheck/Java, so it is
//! not self-contained regardless. Only the page-count / page-size reads moved here.

use lopdf::{Document, Object};
use std::path::Path;

/// Number of pages in a PDF. `None` if the file is missing or can't be parsed.
pub fn page_count(pdf: &Path) -> Option<u32> {
    if !pdf.exists() {
        return None;
    }
    let doc = Document::load(pdf).ok()?;
    Some(doc.get_pages().len() as u32)
}

/// First page's size in points `(width, height)` from its `MediaBox`
/// (`[x0 y0 x1 y1]` → `x1-x0`, `y1-y0`). The `MediaBox` may be inherited from an
/// ancestor in the page tree, so we walk up the tree when the page dict lacks it.
/// `None` on a missing file, parse error, or absent/malformed `MediaBox`.
pub fn page_size_pt(pdf: &Path) -> Option<(f64, f64)> {
    if !pdf.exists() {
        return None;
    }
    let doc = Document::load(pdf).ok()?;
    let (_, page_id) = doc.get_pages().into_iter().next()?;
    let media_box = inherited_media_box(&doc, page_id)?;
    rect_size(&doc, &media_box)
}

/// Resolve `MediaBox` for a page, walking up `/Parent` links for an inherited one.
fn inherited_media_box(doc: &Document, page_id: lopdf::ObjectId) -> Option<Vec<Object>> {
    let mut id = page_id;
    // Bound the climb defensively (page trees are shallow; avoid a cycle hang).
    for _ in 0..64 {
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(mb) = dict.get(b"MediaBox") {
            let arr = doc.dereference(mb).ok()?.1.as_array().ok()?;
            return Some(arr.clone());
        }
        match dict.get(b"Parent") {
            Ok(parent) => id = parent.as_reference().ok()?,
            Err(_) => return None,
        }
    }
    None
}

/// Convert a `MediaBox`/rect array `[x0 y0 x1 y1]` into `(width, height)` in points.
fn rect_size(doc: &Document, arr: &[Object]) -> Option<(f64, f64)> {
    if arr.len() < 4 {
        return None;
    }
    let n = |o: &Object| -> Option<f64> {
        // Coordinates may be references to numbers; deref then read int/real.
        let o = doc.dereference(o).ok()?.1;
        match o {
            Object::Integer(i) => Some(*i as f64),
            Object::Real(r) => Some(*r as f64),
            _ => None,
        }
    };
    let x0 = n(&arr[0])?;
    let y0 = n(&arr[1])?;
    let x1 = n(&arr[2])?;
    let y1 = n(&arr[3])?;
    Some(((x1 - x0).abs(), (y1 - y0).abs()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    /// Build a valid one-page PDF (with xref, so `Document::load` accepts it) whose
    /// MediaBox is `[0 0 w h]`. If `inherit`, the MediaBox lives on the `Pages`
    /// parent (testing inheritance) rather than the leaf `Page`.
    fn make_pdf(name: &str, w: i64, h: i64, inherit: bool) -> std::path::PathBuf {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let media = vec![0.into(), 0.into(), w.into(), h.into()];
        let mut page = dictionary! { "Type" => "Page", "Parent" => pages_id };
        if !inherit {
            page.set("MediaBox", media.clone());
        }
        let page_id = doc.add_object(page);
        let mut pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        };
        if inherit {
            pages.set("MediaBox", media);
        }
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);

        let mut p = std::env::temp_dir();
        p.push(format!("bookmill-pdfmeta-{name}-{}.pdf", std::process::id()));
        doc.save(&p).unwrap();
        p
    }

    #[test]
    fn missing_file_is_none() {
        let p = Path::new("/nonexistent/definitely-not-a.pdf");
        assert_eq!(page_count(p), None);
        assert_eq!(page_size_pt(p), None);
    }

    #[test]
    fn parses_count_and_size() {
        let p = make_pdf("letter", 612, 792, false);
        assert_eq!(page_count(&p), Some(1));
        let (w, h) = page_size_pt(&p).expect("size");
        assert!((w - 612.0).abs() < 0.01, "w={w}");
        assert!((h - 792.0).abs() < 0.01, "h={h}");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn inherited_media_box_from_parent() {
        // Page dict has no MediaBox; it lives on the Pages parent (inheritance).
        // 441×666pt = the la-riqueza KDP picture-book trim+bleed.
        let p = make_pdf("inherited", 441, 666, true);
        let (w, h) = page_size_pt(&p).expect("inherited size");
        assert!((w - 441.0).abs() < 0.01, "w={w}");
        assert!((h - 666.0).abs() < 0.01, "h={h}");
        let _ = std::fs::remove_file(&p);
    }

    /// The `rect_size` parse helper handles a non-zero origin (size = x1-x0).
    #[test]
    fn rect_size_helper() {
        let doc = Document::new();
        let arr = vec![
            Object::Integer(10),
            Object::Integer(20),
            Object::Integer(622),
            Object::Integer(812),
        ];
        assert_eq!(rect_size(&doc, &arr), Some((612.0, 792.0)));
    }
}
