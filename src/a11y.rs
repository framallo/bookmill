//! Accessibility audit of the **built artifacts** (`bookmill validate --deep`).
//!
//! The build side derives its accessibility claims from the content
//! (`epub_native::a11y_metadata`, `typst_pdf::all_images_described`); this module is
//! the check that they hold, run against the finished EPUB/PDF rather than the
//! markdown — so a regression in the pipeline can't slip through silently.
//!
//! What is audited:
//!   * **EPUB** — the OPF's schema.org accessibility metadata, `dc:language`, a
//!     `lang`/`xml:lang` on every content document, a nav TOC with entries, and
//!     (the check that matters most) **every `<img>` carrying non-empty alt text**.
//!     `landmarks` / `page-list` are *recommended*, so their absence warns.
//!   * **PDF** — a document title and a language (an untitled, language-less PDF
//!     fails every accessibility checker), plus whether it is a tagged PDF.
//!
//! XHTML/OPF are scanned as strings rather than parsed: bookmill has no XML parser
//! in its dependency tree, the documents are its own output (well-formed by
//! construction — epubcheck proves it), and `epub_native` already reads attributes
//! this way.

use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;

/// Severity of one audit finding, mapped by the caller onto the ✓ / ! / ✗ report.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Level {
    Ok,
    Warn,
    Err,
}

/// One line of the accessibility report.
#[derive(Debug)]
pub struct Finding {
    pub level: Level,
    pub msg: String,
}

fn ok(msg: impl Into<String>) -> Finding {
    Finding { level: Level::Ok, msg: msg.into() }
}
fn warn(msg: impl Into<String>) -> Finding {
    Finding { level: Level::Warn, msg: msg.into() }
}
fn err(msg: impl Into<String>) -> Finding {
    Finding { level: Level::Err, msg: msg.into() }
}

/// The schema.org accessibility properties every EPUB must carry (EPUB
/// Accessibility 1.1; the EU Accessibility Act makes them mandatory for ebooks sold
/// in the EU). `epub_native::a11y_metadata` emits all five.
const REQUIRED_A11Y_PROPS: [&str; 5] = [
    "schema:accessMode",
    "schema:accessModeSufficient",
    "schema:accessibilityFeature",
    "schema:accessibilityHazard",
    "schema:accessibilitySummary",
];

/// Audit a built EPUB. Returns one [`Finding`] per check.
pub fn audit_epub(epub: &Path) -> Result<Vec<Finding>> {
    let bytes = std::fs::read(epub).with_context(|| format!("reading {}", epub.display()))?;
    let mut zin =
        zip::ZipArchive::new(std::io::Cursor::new(&bytes)).context("opening epub zip")?;

    // Read the text entries we audit (OPF + XHTML). Images etc. are skipped.
    let mut opf = String::new();
    let mut docs: Vec<(String, String)> = Vec::new(); // (name, xhtml)
    for i in 0..zin.len() {
        let mut f = zin.by_index(i)?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_string();
        let is_opf = name.ends_with(".opf");
        let is_xhtml = name.ends_with(".xhtml") || name.ends_with(".html");
        if !is_opf && !is_xhtml {
            continue;
        }
        let mut s = String::new();
        if f.read_to_string(&mut s).is_err() {
            continue; // not UTF-8 text; nothing to audit
        }
        if is_opf {
            opf = s;
        } else {
            docs.push((name, s));
        }
    }

    let mut f = Vec::new();
    if opf.is_empty() {
        f.push(err("a11y: no OPF package document found in the EPUB"));
        return Ok(f);
    }

    f.extend(audit_opf(&opf));
    f.extend(audit_img_alt(&docs));
    f.extend(audit_doc_langs(&docs));
    f.extend(audit_nav(&docs));
    f.extend(audit_headings(&docs));
    Ok(f)
}

/// Heading hierarchy: a document must not skip a level (an `<h1>` followed by an
/// `<h3>` leaves a screen-reader user with a hole in the outline, and it is a hard
/// **PDF/UA-1 failure** — Typst rejects the export, so such a book silently loses its
/// UA-1 claim).
///
/// Reported as a warning, not an error: the fix is an authoring decision in the
/// manuscript (`###` → `##`), which also changes how the heading renders, so it is
/// the author's call — not something a validator should fail the build over.
fn audit_headings(docs: &[(String, String)]) -> Vec<Finding> {
    let mut bad = Vec::new();
    for (name, html) in docs {
        let mut prev = 0usize;
        for lvl in heading_levels(html) {
            if prev > 0 && lvl > prev + 1 {
                bad.push(format!("{name}: h{prev} → h{lvl}"));
            }
            prev = lvl;
        }
    }
    if bad.is_empty() {
        return vec![ok("a11y EPUB: heading hierarchy has no skipped levels")];
    }
    vec![warn(format!(
        "a11y EPUB: heading level skipped (breaks the outline; blocks PDF/UA-1) — {}",
        bad.join("; ")
    ))]
}

/// The heading levels (`<h1>`…`<h6>`) of a document, in document order.
fn heading_levels(html: &str) -> Vec<usize> {
    let bytes = html.as_bytes();
    let mut v = Vec::new();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'<' && (bytes[i + 1] == b'h' || bytes[i + 1] == b'H') {
            let d = bytes[i + 2];
            // `<hN` followed by a tag terminator — not `<hr`, `<header`, `<html`
            if d.is_ascii_digit() && (b'1'..=b'6').contains(&d) {
                let after = bytes.get(i + 3).copied();
                if matches!(after, Some(c) if c == b'>' || c == b' ' || c == b'\t') {
                    v.push((d - b'0') as usize);
                }
            }
        }
        i += 1;
    }
    v
}

/// OPF checks: the schema.org accessibility metadata and `dc:language`.
fn audit_opf(opf: &str) -> Vec<Finding> {
    let mut f = Vec::new();
    let missing: Vec<&str> = REQUIRED_A11Y_PROPS
        .iter()
        .copied()
        .filter(|p| !opf.contains(&format!("property=\"{p}\"")))
        .collect();
    if missing.is_empty() {
        f.push(ok("a11y EPUB: accessibility metadata complete (accessMode, accessModeSufficient, accessibilityFeature, accessibilityHazard, accessibilitySummary)"));
    } else {
        f.push(err(format!(
            "a11y EPUB: OPF is missing accessibility metadata: {}",
            missing.join(", ")
        )));
    }

    if has_nonempty_element(opf, "dc:language") {
        f.push(ok("a11y EPUB: dc:language set"));
    } else {
        f.push(err("a11y EPUB: no dc:language in the OPF"));
    }

    // The cover image is exposed to readers via the manifest's `cover-image`
    // property (the EPUB3 mechanism); it is not a content document, so it gets no
    // landmark. Informational when absent — a book may legitimately ship no cover.
    if opf.contains("cover-image") {
        f.push(ok("a11y EPUB: cover image declared (manifest properties=\"cover-image\")"));
    } else {
        f.push(warn("a11y EPUB: no cover image declared in the manifest"));
    }
    f
}

/// **Every `<img>` must carry non-empty alt text.** The single most important
/// content check: an undescribed image is invisible to a screen-reader user, and it
/// also silently downgrades what the OPF may claim (see `epub_native::a11y_metadata`).
fn audit_img_alt(docs: &[(String, String)]) -> Vec<Finding> {
    let mut offenders: Vec<(String, String)> = Vec::new(); // (doc, src)
    let mut total = 0usize;
    for (name, html) in docs {
        for tag in img_tags(html) {
            total += 1;
            let alt = attr_value(&tag, "alt").unwrap_or_default();
            if alt.trim().is_empty() {
                let src = attr_value(&tag, "src").unwrap_or_else(|| "(no src)".into());
                offenders.push((name.clone(), src));
            }
        }
    }
    if offenders.is_empty() {
        return vec![ok(format!(
            "a11y EPUB: every image has alt text ({total} image(s))"
        ))];
    }
    offenders
        .iter()
        .map(|(doc, src)| err(format!("a11y EPUB: <img> without alt text — {doc}: {src}")))
        .collect()
}

/// Every content document needs a language (WCAG 3.1.1). Chapters get one from
/// `epub_native::xhtml_doc`; the generated nav/TOC get theirs from the a11y fixup.
fn audit_doc_langs(docs: &[(String, String)]) -> Vec<Finding> {
    let missing: Vec<&str> = docs
        .iter()
        .filter(|(_, html)| !has_html_lang(html))
        .map(|(n, _)| n.as_str())
        .collect();
    if missing.is_empty() {
        return vec![ok(format!(
            "a11y EPUB: lang/xml:lang on all {} content document(s)",
            docs.len()
        ))];
    }
    vec![err(format!(
        "a11y EPUB: no lang/xml:lang on: {}",
        missing.join(", ")
    ))]
}

/// The nav document: a `toc` nav with entries (required), plus the *recommended*
/// `landmarks` and `page-list` navs (absence warns, never errors).
fn audit_nav(docs: &[(String, String)]) -> Vec<Finding> {
    let Some((_, nav)) = docs.iter().find(|(n, html)| {
        n.ends_with("nav.xhtml") || html.contains("epub:type=\"toc\"") || html.contains("epub:type = \"toc\"")
    }) else {
        return vec![err("a11y EPUB: no nav document with a toc")];
    };
    let mut f = Vec::new();

    let entries = nav.matches("<li>").count();
    if has_nav(nav, "toc") && entries > 0 {
        f.push(ok(format!("a11y EPUB: nav TOC present ({entries} entries)")));
    } else {
        f.push(err("a11y EPUB: nav TOC missing or empty"));
    }

    if has_nav(nav, "landmarks") {
        f.push(ok("a11y EPUB: landmarks nav present"));
    } else {
        f.push(warn("a11y EPUB: no landmarks nav (recommended, not required)"));
    }

    // A page-list may only exist when the EPUB carries real print-page boundaries
    // (`epub:type="pagebreak"` anchors). bookmill does not emit them — the print PDF
    // is a separate Typst layout, so there is no honest EPUB→print page mapping to
    // publish. Warn, never fabricate.
    if has_nav(nav, "page-list") {
        f.push(ok("a11y EPUB: page-list nav present"));
    } else {
        f.push(warn(
            "a11y EPUB: no page-list nav (recommended; needs real print-page anchors, which bookmill does not emit)",
        ));
    }
    f
}

/// Audit a built PDF: title, language, tagging.
pub fn audit_pdf(pdf: &Path) -> Vec<Finding> {
    let mut f = Vec::new();
    match crate::pdfmeta::title(pdf) {
        Some(t) => f.push(ok(format!("a11y PDF: title set (\"{t}\")"))),
        None => f.push(err("a11y PDF: no document title (fails every accessibility check)")),
    }
    match crate::pdfmeta::catalog_lang(pdf) {
        Some(l) => f.push(ok(format!("a11y PDF: language set (/Lang {l})"))),
        None => f.push(err("a11y PDF: no document language (/Lang)")),
    }
    // Tagging is what makes a PDF navigable by a screen reader. Typst tags every PDF
    // it exports; the Ghostscript digital-PDF shrink rebuilds the file and drops the
    // structure tree (it cannot carry it), so that one output is untagged — a real
    // limitation, reported rather than papered over.
    if crate::pdfmeta::is_tagged(pdf) {
        f.push(ok("a11y PDF: tagged (structure tree present)"));
    } else {
        f.push(warn(
            "a11y PDF: not tagged (no structure tree) — the Ghostscript digital-PDF compression drops it",
        ));
    }
    f
}

// ---------- string scanning ----------

/// Every `<img …>` tag in a document.
fn img_tags(html: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("<img") {
        // only a real tag: `<img` followed by whitespace or `/` or `>`
        let after = rest[pos + 4..].chars().next();
        let Some(end_rel) = rest[pos..].find('>') else { break };
        if matches!(after, Some(c) if c.is_whitespace() || c == '/' || c == '>') {
            v.push(rest[pos..pos + end_rel].to_string());
        }
        rest = &rest[pos + end_rel..];
    }
    v
}

/// Read a double-quoted attribute value out of a tag string.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let pat = format!("{name}=\"");
    let i = tag.find(&pat)? + pat.len();
    let j = tag[i..].find('"')? + i;
    Some(tag[i..j].to_string())
}

/// True if the document's `<html>` tag declares a language.
fn has_html_lang(html: &str) -> bool {
    let Some(start) = html.find("<html") else { return false };
    let Some(end_rel) = html[start..].find('>') else { return false };
    let tag = &html[start..start + end_rel];
    tag.contains("lang=\"") && attr_value(tag, "lang").is_some_and(|l| !l.trim().is_empty())
}

/// True if the nav document contains `<nav epub:type="<kind>">` (the builder emits
/// the attribute with spaces around `=`, so both spellings are accepted).
fn has_nav(nav: &str, kind: &str) -> bool {
    nav.contains(&format!("epub:type=\"{kind}\"")) || nav.contains(&format!("epub:type = \"{kind}\""))
}

/// True if the OPF has a `<tag>…</tag>` with non-empty text.
fn has_nonempty_element(xml: &str, tag: &str) -> bool {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let Some(i) = xml.find(&open) else { return false };
    let Some(gt) = xml[i..].find('>').map(|r| i + r + 1) else { return false };
    let Some(j) = xml[gt..].find(&close).map(|r| gt + r) else { return false };
    !xml[gt..j].trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(f: &[Finding]) -> Vec<Level> {
        f.iter().map(|x| x.level).collect()
    }

    #[test]
    fn flags_every_image_missing_alt_with_file_and_src() {
        let docs = vec![
            (
                "OEBPS/chapter-001.xhtml".to_string(),
                r#"<p><img src="a.png" alt="Un león" /></p><p><img src="b.png" alt="" /></p>"#
                    .to_string(),
            ),
            (
                "OEBPS/chapter-002.xhtml".to_string(),
                r#"<p><img src="c.png" /></p>"#.to_string(),
            ),
        ];
        let f = audit_img_alt(&docs);
        assert_eq!(f.len(), 2, "one error per undescribed image");
        assert!(f.iter().all(|x| x.level == Level::Err));
        // the offending file AND src are named, so the fix is obvious
        assert!(f[0].msg.contains("chapter-001.xhtml") && f[0].msg.contains("b.png"));
        assert!(f[1].msg.contains("chapter-002.xhtml") && f[1].msg.contains("c.png"));
    }

    #[test]
    fn passes_when_every_image_is_described() {
        let docs = vec![(
            "OEBPS/chapter-001.xhtml".to_string(),
            r#"<img src="a.png" alt="Un león"/><img src="b.png" alt="Una rata"/>"#.to_string(),
        )];
        let f = audit_img_alt(&docs);
        assert_eq!(levels(&f), [Level::Ok]);
        assert!(f[0].msg.contains("2 image(s)"));
    }

    #[test]
    fn opf_missing_a11y_metadata_is_an_error() {
        let opf = r#"<metadata><dc:language>es</dc:language>
            <meta property="schema:accessMode">textual</meta></metadata>"#;
        let f = audit_opf(opf);
        // accessMode is there; the other four are not -> one error naming them
        let e = f.iter().find(|x| x.level == Level::Err).expect("an error");
        assert!(e.msg.contains("schema:accessibilitySummary"));
        assert!(e.msg.contains("schema:accessibilityHazard"));
        assert!(!e.msg.contains("property"), "only the missing names are listed");
    }

    #[test]
    fn opf_with_full_a11y_metadata_and_language_passes() {
        let mut opf = String::from("<metadata><dc:language>es</dc:language>");
        for p in REQUIRED_A11Y_PROPS {
            opf.push_str(&format!("<meta property=\"{p}\">x</meta>"));
        }
        opf.push_str("<item properties=\"cover-image\"/></metadata>");
        let f = audit_opf(&opf);
        assert!(f.iter().all(|x| x.level == Level::Ok), "{f:?}");
    }

    #[test]
    fn empty_dc_language_is_an_error() {
        let opf = "<metadata><dc:language></dc:language></metadata>";
        assert!(audit_opf(opf)
            .iter()
            .any(|x| x.level == Level::Err && x.msg.contains("dc:language")));
    }

    #[test]
    fn content_document_without_lang_is_an_error() {
        let docs = vec![
            ("a.xhtml".to_string(), r#"<html lang="es" xml:lang="es"><body/></html>"#.to_string()),
            ("nav.xhtml".to_string(), r#"<html xmlns="x"><body/></html>"#.to_string()),
        ];
        let f = audit_doc_langs(&docs);
        assert_eq!(levels(&f), [Level::Err]);
        assert!(f[0].msg.contains("nav.xhtml") && !f[0].msg.contains("a.xhtml"));
    }

    #[test]
    fn nav_toc_required_landmarks_and_pagelist_only_warn() {
        // a nav with a toc + landmarks, but no page-list (bookmill's real output)
        let nav = r#"<html lang="es"><nav epub:type = "toc"><ol><li><a href="c1.xhtml">Uno</a></li></ol></nav>
            <nav epub:type = "landmarks"><ol><li><a epub:type="bodymatter" href="c1.xhtml">Uno</a></li></ol></nav></html>"#;
        let docs = vec![("OEBPS/nav.xhtml".to_string(), nav.to_string())];
        let f = audit_nav(&docs);
        assert_eq!(levels(&f), [Level::Ok, Level::Ok, Level::Warn]);
        // the page-list gap is a warning — never an error, and never faked
        assert!(f[2].msg.contains("page-list"));
    }

    #[test]
    fn nav_without_landmarks_warns_but_does_not_error() {
        let nav = r#"<html lang="es"><nav epub:type="toc"><ol><li><a href="c1.xhtml">Uno</a></li></ol></nav></html>"#;
        let f = audit_nav(&[("OEBPS/nav.xhtml".to_string(), nav.to_string())]);
        assert!(!f.iter().any(|x| x.level == Level::Err));
        assert_eq!(f.iter().filter(|x| x.level == Level::Warn).count(), 2); // landmarks + page-list
    }

    #[test]
    fn flags_a_skipped_heading_level_but_only_as_a_warning() {
        // the real menger-a-milei shape: chapter h1, then straight to h3
        let docs = vec![(
            "OEBPS/chapter-001.xhtml".to_string(),
            "<h1>Capítulo</h1><h3>Un mapa mejor</h3>".to_string(),
        )];
        let f = audit_headings(&docs);
        assert_eq!(levels(&f), [Level::Warn]);
        assert!(f[0].msg.contains("h1 → h3") && f[0].msg.contains("chapter-001.xhtml"));
    }

    #[test]
    fn well_nested_headings_pass() {
        let docs = vec![(
            "a.xhtml".to_string(),
            "<h1>T</h1><h2>S</h2><h3>Sub</h3><h2>S2</h2>".to_string(),
        )];
        assert_eq!(levels(&audit_headings(&docs)), [Level::Ok]);
    }

    #[test]
    fn heading_scan_ignores_hr_header_and_html_tags() {
        // `<hr/>`, `<header>` and `<html>` must not be read as headings
        assert_eq!(heading_levels("<html><hr/><header>x</header><h2 class=\"a\">Y</h2>"), [2]);
    }

    #[test]
    fn img_tag_scan_ignores_lookalikes() {
        // `<image>` (SVG) and text mentioning "img" must not be picked up as <img>
        let html = r#"<image href="x.svg"/> img src=nope <img src="a.png" alt="ok"/>"#;
        let tags = img_tags(html);
        assert_eq!(tags.len(), 1);
        assert_eq!(attr_value(&tags[0], "src").as_deref(), Some("a.png"));
    }
}
