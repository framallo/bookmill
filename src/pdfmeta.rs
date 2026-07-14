//! Native PDF metadata — page count, first-page size, and placed-image
//! resolution — via the `lopdf` crate.
//!
//! Replaces shelling out to poppler (`pdfinfo`, `pdfimages -list`). Pure Rust, no
//! external binary on `$PATH`. The page-count / page-size helpers return `None` on
//! a missing file or parse error, so callers can treat that the same as "not built
//! yet / unreadable" (the old `pdfinfo` wrappers did the same).
//!
//! [`placed_images`] replaces poppler's `pdfimages -list`: it runs a tiny PDF
//! content-stream interpreter (tracking the CTM through `q`/`Q`/`cm`) so that for
//! every image drawn by a `Do` operator we know the placed size in points, and
//! hence the *effective* resolution at print size — the number `pdfimages` prints
//! in its `x-ppi`/`y-ppi` columns. See [`PlacedImage`].

use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::HashMap;
use std::path::Path;

/// Number of pages in a PDF. `None` if the file is missing or can't be parsed.
pub fn page_count(pdf: &Path) -> Option<u32> {
    if !pdf.exists() {
        return None;
    }
    let doc = Document::load(pdf).ok()?;
    Some(doc.get_pages().len() as u32)
}

/// The document title (`/Info /Title`, or the XMP `dc:title`). `None` if absent —
/// an untitled PDF fails every accessibility check, so the validator errors on it.
pub fn title(pdf: &Path) -> Option<String> {
    let doc = Document::load(pdf).ok()?;
    let info = doc.trailer.get(b"Info").ok()?;
    let dict = match info {
        Object::Reference(id) => doc.get_object(*id).ok()?.as_dict().ok()?,
        Object::Dictionary(d) => d,
        _ => return None,
    };
    let t = dict.get(b"Title").ok()?;
    let s = decode_text(t)?;
    (!s.trim().is_empty()).then_some(s)
}

/// The document's natural language (`/Root /Lang`). `None` if unset — a PDF with no
/// language makes a screen reader guess the pronunciation of every word.
pub fn catalog_lang(pdf: &Path) -> Option<String> {
    let doc = Document::load(pdf).ok()?;
    let root = doc.trailer.get(b"Root").ok()?.as_reference().ok()?;
    let cat = doc.get_object(root).ok()?.as_dict().ok()?;
    let s = decode_text(cat.get(b"Lang").ok()?)?;
    (!s.trim().is_empty()).then_some(s)
}

/// True when the PDF carries a structure tree (`/Root /StructTreeRoot`) — i.e. it is
/// a *tagged* PDF, the prerequisite for any real PDF accessibility.
pub fn is_tagged(pdf: &Path) -> bool {
    let Ok(doc) = Document::load(pdf) else { return false };
    let Ok(root) = doc.trailer.get(b"Root").and_then(|r| r.as_reference()) else {
        return false;
    };
    doc.get_object(root)
        .and_then(|o| o.as_dict())
        .map(|c| c.has(b"StructTreeRoot"))
        .unwrap_or(false)
}

/// Set `/Lang` on the document catalog, in place.
///
/// Used to restore the language after the Ghostscript digital-PDF shrink, which
/// rebuilds the file and drops both the structure tree and `/Lang` (the tag tree is
/// unrecoverable; `/Lang` is one catalog entry, and a language-less PDF fails every
/// accessibility check). Setting it states only what is true: the document's language.
pub fn set_catalog_lang(pdf: &Path, lang: &str) -> Option<()> {
    let mut doc = Document::load(pdf).ok()?;
    let root = doc.trailer.get(b"Root").ok()?.as_reference().ok()?;
    let cat = doc.get_object_mut(root).ok()?.as_dict_mut().ok()?;
    cat.set("Lang", Object::string_literal(lang));
    doc.save(pdf).ok()?;
    Some(())
}

/// Decode a PDF text object (literal string, possibly UTF-16BE with a BOM).
fn decode_text(o: &Object) -> Option<String> {
    let b = o.as_str().ok()?;
    if b.starts_with(&[0xFE, 0xFF]) {
        let u16s: Vec<u16> = b[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16(&u16s).ok();
    }
    Some(String::from_utf8_lossy(b).into_owned())
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

// ---------- Placed-image resolution (native `pdfimages -list`) ----------

/// One raster image as it is *placed* on a page: its pixel dimensions (from the
/// image XObject's `/Width`,`/Height`) and its placed size in PDF points (the
/// length of the CTM's column vectors at the `Do` that drew it). An image XObject
/// is painted into the unit square `[0,1]×[0,1]`, so the placed width is the
/// length of the CTM vector `(a,b)` and the placed height the length of `(c,d)`.
///
/// The *effective* resolution is `pixels / (placed_pt / 72)` — exactly what
/// poppler's `pdfimages -list` reports as `x-ppi`/`y-ppi`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedImage {
    /// 1-based page number.
    pub page: u32,
    /// Image pixel width (`/Width`).
    pub px_w: u32,
    /// Image pixel height (`/Height`).
    pub px_h: u32,
    /// Placed width in PDF points (= length of CTM column `(a,b)`).
    pub placed_w_pt: f64,
    /// Placed height in PDF points (= length of CTM column `(c,d)`).
    pub placed_h_pt: f64,
}

impl PlacedImage {
    /// Effective horizontal resolution in dpi (`px_w / placed_inches`).
    pub fn dpi_x(&self) -> f64 {
        if self.placed_w_pt <= 0.0 {
            0.0
        } else {
            self.px_w as f64 / (self.placed_w_pt / 72.0)
        }
    }
    /// Effective vertical resolution in dpi.
    pub fn dpi_y(&self) -> f64 {
        if self.placed_h_pt <= 0.0 {
            0.0
        } else {
            self.px_h as f64 / (self.placed_h_pt / 72.0)
        }
    }
    /// Effective resolution = the smaller of the two axes, rounded to the nearest
    /// integer (matching how `pdfimages -list` prints whole ppi numbers).
    pub fn dpi(&self) -> u32 {
        self.dpi_x().min(self.dpi_y()).round().max(0.0) as u32
    }
    /// Placed width in inches.
    pub fn placed_w_in(&self) -> f64 {
        self.placed_w_pt / 72.0
    }
    /// Placed height in inches.
    pub fn placed_h_in(&self) -> f64 {
        self.placed_h_pt / 72.0
    }
}

/// A 2×3 affine matrix `[a b c d e f]` (PDF user-space transform). Rows are
/// `(a,b)`, `(c,d)`, translation `(e,f)`.
#[derive(Debug, Clone, Copy)]
struct Mat {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Mat {
    const IDENTITY: Mat = Mat { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    /// `self ∘ other`: apply `self` first, then `other` — i.e. the 3×3 product
    /// `self × other`. PDF's `cm` updates the CTM to `cm_matrix × CTM`.
    fn then(self, o: Mat) -> Mat {
        Mat {
            a: self.a * o.a + self.b * o.c,
            b: self.a * o.b + self.b * o.d,
            c: self.c * o.a + self.d * o.c,
            d: self.c * o.b + self.d * o.d,
            e: self.e * o.a + self.f * o.c + o.e,
            f: self.e * o.b + self.f * o.d + o.f,
        }
    }
}

/// Parse the six operands of a `cm`/form `/Matrix` into a [`Mat`].
fn mat_from_operands(ops: &[Object]) -> Option<Mat> {
    if ops.len() < 6 {
        return None;
    }
    let n = |i: usize| ops[i].as_float().ok().map(|v| v as f64);
    Some(Mat {
        a: n(0)?,
        b: n(1)?,
        c: n(2)?,
        d: n(3)?,
        e: n(4)?,
        f: n(5)?,
    })
}

/// Every raster image placed on a page of `pdf`, with its effective resolution.
///
/// This is the native replacement for `pdfimages -list`. It interprets each
/// page's content stream — maintaining a CTM stack (`q` push, `Q` pop, `cm`
/// concatenate) — and on every `Do` that resolves to an image XObject records the
/// placement. `Do`s onto **form** XObjects are recursed into (applying the form's
/// `/Matrix` and resources), so images nested one or more forms deep are still
/// found. Inline images (`BI…ID…EI`) are not handled — typst (this repo's PDF
/// engine) never emits them; if a future engine does, those images are simply not
/// audited (reported as fewer images, never miscounted).
///
/// `None` on a missing file or a parse failure of the whole document.
pub fn placed_images(pdf: &Path) -> Option<Vec<PlacedImage>> {
    if !pdf.exists() {
        return None;
    }
    let doc = Document::load(pdf).ok()?;
    let mut out = Vec::new();
    for (page_no, page_id) in doc.get_pages() {
        let res = page_resource_dicts(&doc, page_id);
        if let Ok(content) = doc.get_and_decode_page_content(page_id) {
            interpret(&doc, &content, &res, Mat::IDENTITY, page_no, &mut out, 0);
        }
    }
    Some(out)
}

/// Resolve a page's resource dictionaries (its own plus any inherited up the page
/// tree). Returns owned clones so the borrow on `doc` doesn't outlive recursion.
fn page_resource_dicts(doc: &Document, page_id: ObjectId) -> Vec<Dictionary> {
    let mut dicts = Vec::new();
    if let Ok((own, ids)) = doc.get_page_resources(page_id) {
        if let Some(d) = own {
            dicts.push(d.clone());
        }
        for id in ids {
            if let Ok(d) = doc.get_dictionary(id) {
                dicts.push(d.clone());
            }
        }
    }
    dicts
}

/// Build `name → object-id` for the `/XObject` entries across resource dicts.
fn xobject_ids(doc: &Document, res: &[Dictionary]) -> HashMap<Vec<u8>, ObjectId> {
    let mut map = HashMap::new();
    for r in res {
        let Ok(xo) = r.get(b"XObject") else { continue };
        let Ok((_, xo)) = doc.dereference(xo) else { continue };
        let Ok(xo) = xo.as_dict() else { continue };
        for (name, val) in xo.iter() {
            if let Ok(id) = val.as_reference() {
                map.entry(name.clone()).or_insert(id);
            }
        }
    }
    map
}

/// Interpret a decoded content stream, recording every image `Do` with its CTM.
fn interpret(
    doc: &Document,
    content: &Content,
    res: &[Dictionary],
    start: Mat,
    page: u32,
    out: &mut Vec<PlacedImage>,
    depth: u8,
) {
    if depth > 8 {
        return; // guard against pathological / cyclic form nesting
    }
    let xobjs = xobject_ids(doc, res);
    let mut ctm = start;
    let mut stack: Vec<Mat> = Vec::new();
    for op in &content.operations {
        match op.operator.as_str() {
            "q" => stack.push(ctm),
            "Q" => {
                if let Some(m) = stack.pop() {
                    ctm = m;
                }
            }
            "cm" => {
                if let Some(m) = mat_from_operands(&op.operands) {
                    ctm = m.then(ctm);
                }
            }
            "Do" => {
                let Some(name) = op.operands.first().and_then(|o| o.as_name().ok()) else {
                    continue;
                };
                let Some(&id) = xobjs.get(name) else { continue };
                let Ok(stream) = doc.get_object(id).and_then(|o| o.as_stream()) else {
                    continue;
                };
                let subtype = stream.dict.get(b"Subtype").and_then(|o| o.as_name()).ok();
                match subtype {
                    Some(b"Image") => {
                        let (Ok(w), Ok(h)) = (
                            stream.dict.get(b"Width").and_then(|o| o.as_float()),
                            stream.dict.get(b"Height").and_then(|o| o.as_float()),
                        ) else {
                            continue;
                        };
                        out.push(PlacedImage {
                            page,
                            px_w: w.round() as u32,
                            px_h: h.round() as u32,
                            placed_w_pt: ctm.a.hypot(ctm.b),
                            placed_h_pt: ctm.c.hypot(ctm.d),
                        });
                    }
                    Some(b"Form") => {
                        // Recurse: form contents are drawn under form.Matrix × CTM,
                        // using the form's own /Resources (falling back to the
                        // current ones if it has none).
                        let form_mat = stream
                            .dict
                            .get(b"Matrix")
                            .ok()
                            .and_then(|o| o.as_array().ok())
                            .and_then(|a| mat_from_operands(a))
                            .unwrap_or(Mat::IDENTITY);
                        let form_res = stream
                            .dict
                            .get(b"Resources")
                            .ok()
                            .and_then(|o| doc.dereference(o).ok())
                            .and_then(|(_, o)| o.as_dict().ok())
                            .map(|d| vec![d.clone()])
                            .unwrap_or_else(|| res.to_vec());
                        if let Ok(inner) = stream
                            .decompressed_content()
                            .and_then(|data| Content::decode(&data))
                        {
                            interpret(
                                doc,
                                &inner,
                                &form_res,
                                form_mat.then(ctm),
                                page,
                                out,
                                depth + 1,
                            );
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
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

    /// Matrix concatenation order matches PDF's `cm` semantics: nesting an inner
    /// `cm` under an outer one composes the scales.
    #[test]
    fn matrix_concatenation_composes() {
        // outer scale 2× then inner scale 3× → effective 6× on the unit square.
        let outer = Mat { a: 2.0, b: 0.0, c: 0.0, d: 2.0, e: 0.0, f: 0.0 };
        let inner = Mat { a: 3.0, b: 0.0, c: 0.0, d: 3.0, e: 0.0, f: 0.0 };
        let ctm = inner.then(outer);
        assert!((ctm.a - 6.0).abs() < 1e-9 && (ctm.d - 6.0).abs() < 1e-9);
    }

    /// Build a one-page PDF that paints an image XObject `/Im0` (px_w×px_h pixels)
    /// at the given content-stream `cm` matrix, then assert `placed_images` recovers
    /// the placement (and thus the effective dpi). `nest_in_form`: draw the image
    /// from inside a Form XObject to exercise the recursion path.
    fn make_image_pdf(
        name: &str,
        px_w: i64,
        px_h: i64,
        cm: [f64; 6],
        nest_in_form: bool,
    ) -> std::path::PathBuf {
        use lopdf::Stream;
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();

        // The image XObject.
        let img = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => px_w,
                "Height" => px_h,
                "ColorSpace" => "DeviceRGB",
                "BitsPerComponent" => 8,
            },
            vec![0u8; (px_w * px_h * 3) as usize],
        );
        let img_id = doc.add_object(img);

        let cm_str = format!(
            "{} {} {} {} {} {}",
            cm[0], cm[1], cm[2], cm[3], cm[4], cm[5]
        );

        let (content_bytes, resources) = if nest_in_form {
            // Form XObject draws the image under an identity Matrix; the page draws
            // the form under `cm` — so the effective CTM at the image is `cm`.
            let form = Stream::new(
                dictionary! {
                    "Type" => "XObject",
                    "Subtype" => "Form",
                    "BBox" => vec![0.into(), 0.into(), 1.into(), 1.into()],
                    "Resources" => dictionary! {
                        "XObject" => dictionary! { "Im0" => img_id },
                    },
                },
                b"q 1 0 0 1 0 0 cm /Im0 Do Q".to_vec(),
            );
            let form_id = doc.add_object(form);
            (
                format!("q {cm_str} cm /Fm0 Do Q").into_bytes(),
                dictionary! { "XObject" => dictionary! { "Fm0" => form_id } },
            )
        } else {
            (
                format!("q {cm_str} cm /Im0 Do Q").into_bytes(),
                dictionary! { "XObject" => dictionary! { "Im0" => img_id } },
            )
        };

        let content_id = doc.add_object(Stream::new(Dictionary::new(), content_bytes));
        let page = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 441.into(), 666.into()],
            "Contents" => content_id,
            "Resources" => resources,
        };
        let page_id = doc.add_object(page);
        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);

        let mut p = std::env::temp_dir();
        p.push(format!("bookmill-pdfimg-{name}-{}.pdf", std::process::id()));
        doc.save(&p).unwrap();
        p
    }

    #[test]
    fn placed_image_dpi_from_ctm() {
        // 200×400 px image placed at 72×144 pt (= 1×2 in) → 200 dpi on both axes.
        let p = make_image_pdf("dpi", 200, 400, [72.0, 0.0, 0.0, 144.0, 0.0, 0.0], false);
        let imgs = placed_images(&p).expect("placed images");
        assert_eq!(imgs.len(), 1);
        let im = imgs[0];
        assert_eq!((im.page, im.px_w, im.px_h), (1, 200, 400));
        assert!((im.placed_w_pt - 72.0).abs() < 1e-6, "w={}", im.placed_w_pt);
        assert!((im.placed_h_pt - 144.0).abs() < 1e-6, "h={}", im.placed_h_pt);
        assert_eq!(im.dpi(), 200);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn placed_image_matches_riqueza_full_bleed_ctm() {
        // The real la-riqueza p4 plate: 2048×3072 px at cm (444,0,0,666) → 332 dpi,
        // matching `pdfimages -list`.
        let p = make_image_pdf("riq", 2048, 3072, [444.0, 0.0, 0.0, 666.0, 0.0, 0.0], false);
        let imgs = placed_images(&p).expect("placed images");
        assert_eq!(imgs.len(), 1);
        assert_eq!(imgs[0].dpi(), 332);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn placed_image_found_inside_form_xobject() {
        // Same placement, but the image is drawn from inside a Form XObject — the
        // interpreter must recurse and still report 200 dpi at the page CTM.
        let p = make_image_pdf("form", 200, 400, [72.0, 0.0, 0.0, 144.0, 0.0, 0.0], true);
        let imgs = placed_images(&p).expect("placed images");
        assert_eq!(imgs.len(), 1, "image nested in form not found");
        assert_eq!(imgs[0].dpi(), 200);
        let _ = std::fs::remove_file(&p);
    }
}
