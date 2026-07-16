//! Native EPUB3 builder — the sole EPUB backend (no pandoc).
//!
//! Converts each chapter's Markdown to XHTML with `comrak` (GFM: tables,
//! footnotes, strikethrough, …) and assembles a valid EPUB3 with `epub-builder`:
//! metadata (title/author/lang/rights), the shared `css/epub.css` stylesheet, an
//! embedded cover image, a generated title page + copyright page, one content
//! document per source chapter, and a depth-1 nav/TOC (one entry per chapter).
//!
//! Reproduced conventions from the old pandoc `--to=epub3` path:
//!   * `{.spot}` tailpiece images are dropped (they are print-only — this mirrors
//!     `scripts/drop-spot-epub.lua`);
//!   * pandoc attribute blocks (`{.unnumbered}`, `{.unlisted}`, `{width=80%}`) are
//!     stripped from headings/images (comrak does not parse them);
//!   * chapters start on a recto page via the EPUB CSS (`break-before: right`);
//!   * the cover image is embedded as the EPUB cover (`front-<lang>.png`);
//!   * an inline contents page is emitted for the retail edition (`toc = true`),
//!     matching the old `--toc --toc-depth=1`.
//!
//! After this builds the EPUB, `build.rs` runs the native `shrink_epub` pass.

use crate::build::BookMeta;
use crate::discover::Repo;

/// Bundled code-highlight stylesheet appended to every EPUB (styles fenced code +
/// the syntect class-mode token spans). See `templates/epub-code.css`.
const CODE_CSS: &str = include_str!("../templates/epub-code.css");
use anyhow::{Context, Result};
use comrak::plugins::syntect::SyntectAdapter;
use comrak::{markdown_to_html, markdown_to_html_with_plugins, Options, Plugins};
use epub_builder::{
    EpubBuilder, EpubContent, EpubVersion, MetadataOpfV3, ReferenceType, ZipLibrary,
};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Build one EPUB (one language / one retail-or-KDP flavor) to `out`.
#[allow(clippy::too_many_arguments)]
pub fn run(
    repo: &Repo,
    meta: &BookMeta,
    cepub: Option<&Path>,
    chaps: &[PathBuf],
    cover: Option<&Path>,
    lang: &str,
    toc: bool,
    captions: bool,
    out: &Path,
) -> Result<()> {
    let mut b = EpubBuilder::new(ZipLibrary::new().map_err(anyhow::Error::msg)?)
        .map_err(anyhow::Error::msg)?;
    b.epub_version(EpubVersion::V30);
    b.metadata("title", &meta.title).map_err(anyhow::Error::msg)?;
    b.metadata("author", &meta.author).map_err(anyhow::Error::msg)?;
    b.add_language(lang);
    b.metadata("license", &meta.rights).map_err(anyhow::Error::msg)?;
    b.metadata("generator", "bookmill").map_err(anyhow::Error::msg)?;
    b.metadata("toc_name", if lang == "es" { "Índice" } else { "Contents" })
        .map_err(anyhow::Error::msg)?;
    // Listing copy → `dc:description` / `dc:subject`. Retailers read both straight
    // off the OPF. Only emitted when the book actually has a `[listing.<lang>]`
    // blurb/keywords — never invented.
    if let Some(d) = &meta.description {
        b.metadata("description", d.as_str()).map_err(anyhow::Error::msg)?;
    }
    for kw in &meta.subjects {
        b.metadata("subject", kw.as_str()).map_err(anyhow::Error::msg)?;
    }

    // Stylesheet: the repo's css/epub.css (optional) followed by the bundled
    // code-highlight CSS, so fenced code is styled in every book with no per-repo
    // setup. Always emitted (even absent a repo stylesheet) so the linked
    // `stylesheet.css` always resolves.
    let mut css = String::new();
    let css_path = repo.epub_css();
    if css_path.exists() {
        css.push_str(
            &std::fs::read_to_string(&css_path)
                .with_context(|| format!("reading {}", css_path.display()))?,
        );
        css.push('\n');
    }
    css.push_str(CODE_CSS);
    b.stylesheet(css.as_bytes()).map_err(anyhow::Error::msg)?;

    // Cover image: embed front-<lang>.png as the EPUB cover.
    if let Some(cv) = cover {
        if cv.exists() {
            let bytes = std::fs::read(cv)
                .with_context(|| format!("reading {}", cv.display()))?;
            let ext = ext_of(cv);
            b.add_cover_image(format!("cover.{ext}"), &bytes[..], mime_for(&ext))
                .map_err(anyhow::Error::msg)?;
        }
    }

    // Syntax highlighter for fenced code blocks (```ruby …). Built once per EPUB.
    let hl = highlighter();

    // Generated title page (not listed in the nav).
    let title_xhtml = title_page(meta, lang);
    b.add_content(EpubContent::new("title.xhtml", title_xhtml.as_bytes()))
        .map_err(anyhow::Error::msg)?;

    // Copyright page from copyright-epub.md (not listed in the nav).
    if let Some(cp) = cepub {
        let md = std::fs::read_to_string(cp)
            .with_context(|| format!("reading {}", cp.display()))?;
        let (clean, _) = clean_chapter(&md);
        let body = render_md(&clean, &hl);
        let doc = xhtml_doc(lang, &meta.title, &body);
        b.add_content(
            EpubContent::new("copyright.xhtml", doc.as_bytes())
                .reftype(ReferenceType::Copyright),
        )
        .map_err(anyhow::Error::msg)?;
    }

    // Inline contents page for the retail edition (matches old `--toc`).
    if toc {
        b.inline_toc();
    }

    // Chapters: one content document each, depth-1 nav entry per chapter.
    let mut added_imgs: BTreeSet<String> = BTreeSet::new();
    // Accessibility metadata is DERIVED, never asserted: count the images actually
    // rendered and how many carry real alt text, so the OPF can't claim a feature the
    // book doesn't have. See `a11y_metadata`.
    let (mut imgs, mut imgs_with_alt) = (0usize, 0usize);
    for (i, ch) in chaps.iter().enumerate() {
        let md = std::fs::read_to_string(ch)
            .with_context(|| format!("reading {}", ch.display()))?;
        let (clean, nav_title) = clean_chapter(&md);

        // Register referenced (non-spot) images as EPUB resources, once each.
        for src in image_srcs(&clean) {
            if added_imgs.contains(&src) || is_remote(&src) {
                continue;
            }
            let abs = resolve_img(&repo.root, &src);
            match std::fs::read(&abs) {
                Ok(bytes) => {
                    let ext = ext_of(Path::new(&src));
                    b.add_resource(&src, &bytes[..], mime_for(&ext))
                        .map_err(anyhow::Error::msg)?;
                    added_imgs.insert(src);
                }
                Err(e) => eprintln!("  (epub: image {} skipped: {e})", abs.display()),
            }
        }

        let html = render_md(&clean, &hl);
        // per-image width/alignment from the markdown `{…}` attributes (stripped
        // before comrak) re-applied to the rendered <img> by matching src.
        let html = apply_img_styles(&html, &img_styles(&md));
        let body = if captions { figcaption_plates(&html) } else { html };
        let (n, with_alt) = count_imgs(&body);
        imgs += n;
        imgs_with_alt += with_alt;
        let title = nav_title.unwrap_or_else(|| chapter_fallback_title(ch, i));
        let doc = xhtml_doc(lang, &title, &body);
        let href = format!("chapter-{:03}.xhtml", i + 1);
        let mut content = EpubContent::new(href, doc.as_bytes()).title(title);
        if i == 0 {
            // first chapter = start of the body matter (a KDP-friendly landmark)
            content = content.reftype(ReferenceType::Text);
        }
        b.add_content(content).map_err(anyhow::Error::msg)?;
    }

    // Accessibility metadata (EPUB Accessibility 1.1 / schema.org). The EU
    // Accessibility Act makes this mandatory for ebooks sold in the EU from June
    // 2025, and retailers surface it in the listing. Emitted LAST because it is
    // computed from what the chapters actually contained.
    for (property, content) in a11y_metadata(lang, imgs, imgs_with_alt) {
        b.add_metadata_opf(Box::new(MetadataOpfV3::new(property, content)));
    }

    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = std::fs::File::create(out)
        .with_context(|| format!("creating {}", out.display()))?;
    b.generate(f).map_err(anyhow::Error::msg)?;

    // Accessibility fixes the builder's fixed nav template cannot express (see
    // `a11y_fixup`): a language on the generated nav/TOC documents, plus the
    // title-page / copyright-page landmarks.
    let mut marks: Vec<(&str, &str, String)> = vec![(
        "titlepage",
        "title.xhtml",
        if lang == "es" { "Portada" } else { "Title Page" }.to_string(),
    )];
    if cepub.is_some() {
        marks.push((
            "copyright-page",
            "copyright.xhtml",
            if lang == "es" { "Créditos" } else { "Copyright" }.to_string(),
        ));
    }
    a11y_fixup(out, lang, &marks)
        .with_context(|| format!("accessibility fixup of {}", out.display()))?;
    Ok(())
}

/// Post-pass over the generated EPUB zip, applying the two accessibility fixes the
/// `epub-builder` nav template cannot express:
///
///   * **a language on `nav.xhtml` / `toc.xhtml`** — the crate's template hardcodes a
///     bare `<html xmlns=…>`, so those two generated documents were the only ones in
///     the book with no `lang`/`xml:lang` (every chapter already gets one from
///     [`xhtml_doc`]).
///   * **extra `landmarks` entries.** `epub-builder` only emits a landmark for a
///     document carrying *both* a `ReferenceType` and a nav title — but giving the
///     title page a nav title would also push it into the visible TOC (`add_content`
///     adds any titled document to the TOC). We want the landmark *without* the TOC
///     entry, so the title/copyright landmarks are injected here instead.
///
/// Rezips mimetype-first/STORED, mirroring [`crate::epub_shrink`] (which runs after
/// this and preserves what we write).
///
/// Note: no `cover` landmark is emitted — a landmark must point at a content
/// document, and the cover is embedded as an image resource (declared in the OPF with
/// `properties="cover-image"`, which is how EPUB3 readers surface it), not as a page.
/// Synthesising a cover *page* just to satisfy the landmark would change the reading
/// order of every book, so we don't.
fn a11y_fixup(epub: &Path, lang: &str, marks: &[(&str, &str, String)]) -> Result<()> {
    let bytes = std::fs::read(epub).with_context(|| format!("reading {}", epub.display()))?;
    let mut zin = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).context("opening epub zip")?;

    struct Entry {
        name: String,
        is_dir: bool,
        data: Vec<u8>,
    }
    let mut entries: Vec<Entry> = Vec::with_capacity(zin.len());
    for i in 0..zin.len() {
        let mut f = zin.by_index(i)?;
        let (name, is_dir) = (f.name().to_string(), f.is_dir());
        let mut data = Vec::new();
        if !is_dir {
            f.read_to_end(&mut data)?;
        }
        entries.push(Entry { name, is_dir, data });
    }

    for e in entries.iter_mut() {
        let base = e.name.rsplit('/').next().unwrap_or(&e.name);
        if !matches!(base, "nav.xhtml" | "toc.xhtml") {
            continue;
        }
        let Ok(s) = std::str::from_utf8(&e.data) else { continue };
        let mut html = html_lang(s, lang);
        if base == "nav.xhtml" {
            html = inject_landmarks(&html, marks);
        }
        e.data = html.into_bytes();
    }

    // Rezip: mimetype first + STORED, the rest DEFLATED, original order.
    let tmp = epub.with_extension("epub.a11y");
    {
        let file = std::fs::File::create(&tmp)
            .with_context(|| format!("creating {}", tmp.display()))?;
        let mut zout = zip::ZipWriter::new(std::io::BufWriter::new(file));
        let stored =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        if let Some(mt) = entries.iter().find(|e| e.name == "mimetype") {
            zout.start_file("mimetype", stored)?;
            zout.write_all(&mt.data)?;
        }
        for e in &entries {
            if e.name == "mimetype" {
                continue;
            }
            if e.is_dir {
                zout.add_directory(e.name.trim_end_matches('/'), deflated)?;
            } else {
                zout.start_file(&e.name, deflated)?;
                zout.write_all(&e.data)?;
            }
        }
        zout.finish()?;
    }
    std::fs::rename(&tmp, epub).with_context(|| format!("replacing {}", epub.display()))?;
    Ok(())
}

/// Add `lang`/`xml:lang` to a document's `<html>` tag. A tag that already declares a
/// language is left alone (we never overwrite an explicit one).
fn html_lang(html: &str, lang: &str) -> String {
    let Some(start) = html.find("<html") else { return html.to_string() };
    let Some(end_rel) = html[start..].find('>') else { return html.to_string() };
    let end = start + end_rel;
    let tag = &html[start..end];
    if tag.contains("lang=") {
        return html.to_string();
    }
    let lang = xml_escape(lang);
    format!(
        "{}{} lang=\"{lang}\" xml:lang=\"{lang}\"{}",
        &html[..start],
        tag,
        &html[end..]
    )
}

/// Insert `<li>` landmark entries at the head of the `landmarks` nav's `<ol>`.
/// A nav with no `<ol>` (no landmarks at all) is returned unchanged — we add to an
/// existing list rather than inventing the structure.
fn inject_landmarks(nav: &str, marks: &[(&str, &str, String)]) -> String {
    if marks.is_empty() {
        return nav.to_string();
    }
    let Some(lm) = nav.find("epub:type = \"landmarks\"").or_else(|| nav.find("epub:type=\"landmarks\"")) else {
        return nav.to_string();
    };
    let Some(ol_rel) = nav[lm..].find("<ol>") else { return nav.to_string() };
    let at = lm + ol_rel + "<ol>".len();
    let mut items = String::new();
    for (etype, href, label) in marks {
        // Don't duplicate a landmark the builder already emitted for this document.
        if nav.contains(&format!("href=\"{href}\"")) && nav[lm..].contains(&format!("href=\"{href}\"")) {
            continue;
        }
        items.push_str(&format!(
            "\n      <li><a epub:type=\"{}\" href=\"{}\">{}</a></li>",
            xml_escape(etype),
            xml_escape(href),
            xml_escape(label),
        ));
    }
    format!("{}{}{}", &nav[..at], items, &nav[at..])
}

/// comrak options: GFM features used by the books (tables, footnotes, …) with
/// raw HTML escaped so the output stays well-formed XHTML for epubcheck.
fn comrak_opts() -> Options<'static> {
    let mut o = Options::default();
    o.extension.table = true;
    o.extension.strikethrough = true;
    o.extension.autolink = true;
    o.extension.tasklist = true;
    o.extension.footnotes = true;
    // render.unsafe_ stays false: raw HTML is escaped/omitted, never injected, so
    // every content document is valid XML.
    o
}

/// Build the fenced-code syntax highlighter in **CSS-class mode**: comrak emits
/// `<pre class="syntax-highlighting"><code>…<span class="keyword …">` scope-class
/// spans (no inline colors), styled by `css/epub.css`. This keeps the EPUB
/// self-contained and lets one stylesheet theme both light and dark readers.
/// A fenced block's info string (```ruby) selects the syntect syntax; an unknown
/// or missing language falls back to plain, escaped text (never an error).
fn highlighter() -> SyntectAdapter {
    comrak::plugins::syntect::SyntectAdapterBuilder::new().css().build()
}

/// Markdown → XHTML with fenced-code highlighting via `hl`. Same GFM options as
/// [`comrak_opts`]; the plugin output is written pre-escaped so `unsafe_` stays
/// false and the document remains epubcheck-valid.
fn render_md(md: &str, hl: &SyntectAdapter) -> String {
    let mut plugins = Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(hl);
    markdown_to_html_with_plugins(md, &comrak_opts(), &plugins)
}

/// Strip pandoc attribute blocks and drop `{.spot}` images from a chapter's
/// Markdown, returning the cleaned Markdown plus the first level-1 heading text
/// (used as the nav/TOC title). `.unlisted` is irrelevant here because the title
/// and copyright pages are added without a nav entry regardless.
fn clean_chapter(md: &str) -> (String, Option<String>) {
    let md = strip_html_comments(md);
    let mut out = String::with_capacity(md.len());
    let mut nav_title: Option<String> = None;
    for line in md.lines() {
        let t = line.trim_start();

        // Drop a standalone `.spot` tailpiece image line entirely (print-only).
        if t.starts_with("![") && is_spot_image(t) {
            continue;
        }

        // Headings: strip a trailing `{…}` attribute block; capture the first H1.
        if t.starts_with('#') {
            let stripped = strip_attr_block(line);
            let st = stripped.trim_start();
            let level = st.chars().take_while(|&c| c == '#').count();
            if level == 1 && nav_title.is_none() {
                let text = st[level..].trim().to_string();
                if !text.is_empty() {
                    nav_title = Some(text);
                }
            }
            out.push_str(&stripped);
            out.push('\n');
            continue;
        }

        // Image lines: strip a trailing `{width=…}` etc. attribute block.
        if t.starts_with("![") {
            out.push_str(&strip_attr_block(line));
            out.push('\n');
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }
    (out, nav_title)
}

/// Images in the rendered body, and how many carry non-empty alt text.
fn count_imgs(html: &str) -> (usize, usize) {
    let (mut n, mut with_alt) = (0, 0);
    let mut rest = html;
    while let Some(pos) = rest.find("<img ") {
        let Some(end_rel) = rest[pos..].find('>') else { break };
        let tag = &rest[pos..pos + end_rel];
        n += 1;
        if attr_value(tag, "alt").is_some_and(|a| !a.trim().is_empty()) {
            with_alt += 1;
        }
        rest = &rest[pos + end_rel..];
    }
    (n, with_alt)
}

/// Accessibility metadata for the OPF, **derived from the book's actual content**.
///
/// The rule that matters: never claim a feature the book doesn't have. A text-only
/// novel is `textual` and needs no more. A book with pictures is also `visual`, and
/// may only advertise `alternativeText` — and a text-only reading path
/// (`accessModeSufficient=textual`) — when *every* image really carries alt text.
///
/// Deliberately omitted: `dcterms:conformsTo` (a WCAG conformance claim) and
/// `a11y:certifiedBy`. Those assert an audit that nobody has performed; publishing
/// them unearned would be a false accessibility claim.
fn a11y_metadata(lang: &str, imgs: usize, imgs_with_alt: usize) -> Vec<(String, String)> {
    let all_described = imgs == 0 || imgs == imgs_with_alt;
    let mut m: Vec<(String, String)> = Vec::new();
    let mut push = |p: &str, v: &str| m.push((p.to_string(), v.to_string()));

    push("schema:accessMode", "textual");
    if imgs > 0 {
        push("schema:accessMode", "visual");
    }
    // How the content can be consumed END TO END. Text alone suffices when there are
    // no images, or when every image is described.
    if all_described {
        push("schema:accessModeSufficient", "textual");
    }
    if imgs > 0 {
        push("schema:accessModeSufficient", "textual,visual");
    }

    push("schema:accessibilityFeature", "structuralNavigation");
    push("schema:accessibilityFeature", "tableOfContents");
    push("schema:accessibilityFeature", "readingOrder");
    if imgs > 0 && all_described {
        push("schema:accessibilityFeature", "alternativeText");
    }
    // Reflowable text, no flashing, no motion, no audio.
    push("schema:accessibilityHazard", "none");

    let summary = match (lang, imgs, all_described) {
        ("es", 0, _) => "Publicación de texto reflujable, sin imágenes. Estructura de \
             encabezados y tabla de contenidos navegable; compatible con lectores de \
             pantalla y con la ampliación o el cambio de tipografía. Sin riesgos de \
             destellos ni movimiento."
            .to_string(),
        ("es", _, true) => "Publicación de texto reflujable. Todas las ilustraciones \
             llevan texto alternativo, por lo que el libro puede leerse solo con texto. \
             Estructura de encabezados y tabla de contenidos navegable; compatible con \
             lectores de pantalla. Sin riesgos de destellos ni movimiento."
            .to_string(),
        ("es", _, false) => "Publicación de texto reflujable con ilustraciones. Algunas \
             imágenes no tienen texto alternativo. Estructura de encabezados y tabla de \
             contenidos navegable. Sin riesgos de destellos ni movimiento."
            .to_string(),
        (_, 0, _) => "Reflowable text publication with no images. Navigable heading \
             structure and table of contents; works with screen readers and with \
             resized or substituted fonts. No flashing or motion hazards."
            .to_string(),
        (_, _, true) => "Reflowable text publication. Every illustration carries \
             alternative text, so the book can be read as text alone. Navigable heading \
             structure and table of contents; works with screen readers. No flashing or \
             motion hazards."
            .to_string(),
        (_, _, false) => "Reflowable text publication with illustrations. Some images \
             lack alternative text. Navigable heading structure and table of contents. \
             No flashing or motion hazards."
            .to_string(),
    };
    m.push(("schema:accessibilitySummary".to_string(), summary));
    m
}

/// Turn each standalone plate-image paragraph (`<p><img … alt="…"/></p>`) into a
/// `<figure class="plate">` with a visible `<figcaption>` carrying the alt text, so
/// the caption shows in the reader (not just as accessibility metadata). Images with
/// an empty alt are left untouched. Non-matching HTML passes through verbatim.
fn figcaption_plates(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    let mut rest = html;
    while let Some(pos) = rest.find("<p><img ") {
        let Some(end_rel) = rest[pos..].find("</p>") else { break };
        let end = pos + end_rel + 4;
        let inner = &rest[pos + 3..end - 4]; // the <img …/> between <p> and </p>
        out.push_str(&rest[..pos]);
        match attr_value(inner, "alt") {
            Some(alt) if !alt.trim().is_empty() => {
                out.push_str("<figure class=\"plate\">");
                out.push_str(inner);
                out.push_str("<figcaption>");
                out.push_str(&alt);
                out.push_str("</figcaption></figure>");
            }
            _ => out.push_str(&rest[pos..end]),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Read a double-quoted HTML attribute value (already entity-escaped by the
/// renderer) out of a tag string. `None` if the attribute is absent.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let pat = format!("{name}=\"");
    let i = tag.find(&pat)? + pat.len();
    let j = tag[i..].find('"')? + i;
    Some(tag[i..j].to_string())
}

/// True if a standalone image line carries a `{… .spot …}` attribute block.
fn is_spot_image(line: &str) -> bool {
    attr_block(line).map(|a| a.contains(".spot")).unwrap_or(false)
}

/// Remove HTML comments `<!-- ... -->` (including multi-line ones) from Markdown
/// before cleaning, so pipeline notes carried in frontmatter/chapters (e.g. the
/// copyright page's editing note) never leak into the rendered XHTML.
fn strip_html_comments(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start + 4..].find("-->") {
            Some(end) => rest = &rest[start + 4 + end + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Return the inner text of a trailing `{ … }` attribute block, if the line ends
/// with one.
fn attr_block(line: &str) -> Option<String> {
    let s = line.trim_end();
    if !s.ends_with('}') {
        return None;
    }
    let open = s.rfind('{')?;
    Some(s[open + 1..s.len() - 1].to_string())
}

/// Drop a trailing `{ … }` attribute block from a line, preserving indentation.
fn strip_attr_block(line: &str) -> String {
    let s = line.trim_end();
    if s.ends_with('}') {
        if let Some(open) = s.rfind('{') {
            return s[..open].trim_end().to_string();
        }
    }
    s.to_string()
}

/// Raw value token for `key=…` in a pandoc attribute string (stops at whitespace).
fn pandoc_attr(attrs: &str, key: &str) -> Option<String> {
    let pat = format!("{key}=");
    let idx = attrs.find(&pat)?;
    let val: String = attrs[idx + pat.len()..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    let val = val.trim_matches(|c| c == '"' || c == '\'');
    (!val.is_empty()).then(|| val.to_string())
}

/// The `src` of a standalone `![alt](src){…}` image line.
fn image_src_of(line: &str) -> Option<String> {
    let s = line.trim_start();
    if !s.starts_with("![") {
        return None;
    }
    let close_alt = s.find("](")?;
    let after = &s[close_alt + 2..];
    let close_paren = after.find(')')?;
    let src = after[..close_paren].trim();
    (!src.is_empty()).then(|| src.to_string())
}

/// Build (src → inline CSS) for image lines carrying `{width=… align=…}`. In a
/// reflowable EPUB only width and alignment are meaningful; height/fit/border are
/// print-layout concepts and stay PDF-only. `.spot` images are skipped (dropped).
fn img_styles(md: &str) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for line in md.lines() {
        let t = line.trim_start();
        if !t.starts_with("![") {
            continue;
        }
        let Some(attrs) = attr_block(t) else { continue };
        if attrs.contains(".spot") {
            continue;
        }
        let Some(src) = image_src_of(t) else { continue };
        let mut style = String::new();
        if let Some(w) = pandoc_attr(&attrs, "width") {
            let w = if !w.is_empty() && w.chars().all(|c| c.is_ascii_digit() || c == '.') {
                format!("{w}%")
            } else {
                w
            };
            style.push_str(&format!("width:{w};"));
        }
        match pandoc_attr(&attrs, "align").as_deref() {
            Some("left") => style.push_str("display:block;margin-left:0;margin-right:auto;"),
            Some("right") => style.push_str("display:block;margin-left:auto;margin-right:0;"),
            Some("center") => style.push_str("display:block;margin-left:auto;margin-right:auto;"),
            _ => {}
        }
        if !style.is_empty() {
            v.push((src, style));
        }
    }
    v
}

/// Inject `style="…"` into each `<img>` whose `src` matches an entry in `styles`,
/// so the rendered EPUB image mirrors the markdown width/alignment. Comrak runs with
/// raw HTML off, so this string post-pass is how per-image CSS reaches the output.
fn apply_img_styles(html: &str, styles: &[(String, String)]) -> String {
    if styles.is_empty() {
        return html.to_string();
    }
    let mut out = String::with_capacity(html.len() + 64);
    let mut rest = html;
    while let Some(pos) = rest.find("<img ") {
        let Some(end_rel) = rest[pos..].find('>') else { break };
        let end = pos + end_rel + 1;
        let tag = &rest[pos..end];
        out.push_str(&rest[..pos]);
        let style = styles
            .iter()
            .find(|(src, _)| tag.contains(&format!("src=\"{src}\"")))
            .map(|(_, s)| s);
        match style {
            Some(style) if !tag.contains("style=") => {
                // normalise the tag close to ` />` and insert the style before it
                let core = tag.trim_end_matches('>').trim_end_matches('/').trim_end();
                out.push_str(core);
                out.push_str(&format!(" style=\"{style}\" />"));
            }
            _ => out.push_str(tag),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Extract every image source (`![alt](src)`) from cleaned Markdown.
fn image_srcs(md: &str) -> Vec<String> {
    let mut v = Vec::new();
    let bytes: Vec<char> = md.chars().collect();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == '!' && bytes[i + 1] == '[' {
            // find "](" then ")"
            if let Some(close_alt) = find_seq(&bytes, i + 2, &[']', '(']) {
                if let Some(close) = find_char(&bytes, close_alt + 2, ')') {
                    let src: String = bytes[close_alt + 2..close].iter().collect();
                    let src = src.trim().to_string();
                    if !src.is_empty() {
                        v.push(src);
                    }
                    i = close + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    v
}

fn find_seq(chars: &[char], from: usize, seq: &[char]) -> Option<usize> {
    let mut i = from;
    while i + seq.len() <= chars.len() {
        if chars[i..i + seq.len()] == *seq {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_char(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&i| chars[i] == target)
}

fn is_remote(src: &str) -> bool {
    src.starts_with("http://") || src.starts_with("https://")
}

/// Resolve a Markdown image src (relative to the repo root, as the build runs
/// there) to an absolute path on disk.
fn resolve_img(root: &Path, src: &str) -> PathBuf {
    let p = Path::new(src);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.join(src)
    }
}

/// The generated title page (centered title / subtitle / author).
fn title_page(meta: &BookMeta, lang: &str) -> String {
    let mut body = String::new();
    body.push_str("<section epub:type=\"titlepage\" class=\"titlepage\">\n");
    body.push_str(&format!("<h1 class=\"title\">{}</h1>\n", xml_escape(&meta.title)));
    if let Some(sub) = &meta.subtitle {
        body.push_str(&format!("<p class=\"subtitle\">{}</p>\n", xml_escape(sub)));
    }
    body.push_str(&format!("<p class=\"author\">{}</p>\n", xml_escape(&meta.author)));
    body.push_str("</section>\n");
    xhtml_doc(lang, &meta.title, &body)
}

/// Fallback nav title for a chapter with no level-1 heading.
fn chapter_fallback_title(ch: &Path, idx: usize) -> String {
    ch.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("Chapter {}", idx + 1))
}

/// Wrap an XHTML body fragment in a complete XHTML5 content document that links
/// the shared stylesheet.
fn xhtml_doc(lang: &str, title: &str, body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<!DOCTYPE html>\n\
<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" \
lang=\"{lang}\" xml:lang=\"{lang}\">\n\
<head>\n\
<meta charset=\"utf-8\"/>\n\
<title>{title}</title>\n\
<link rel=\"stylesheet\" type=\"text/css\" href=\"stylesheet.css\"/>\n\
</head>\n\
<body>\n{body}</body>\n\
</html>\n",
        lang = xml_escape(lang),
        title = xml_escape(title),
        body = body,
    )
}

/// Escape text for XML/XHTML (`& < > " '`).
fn xml_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            _ => o.push(c),
        }
    }
    o
}

/// Lowercase file extension of a path (no dot), defaulting to "png".
fn ext_of(p: &Path) -> String {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| "png".into())
}

/// MIME type for a (lowercase, dotless) image extension.
fn mime_for(ext: &str) -> &'static str {
    match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(m: &[(String, String)], key: &str) -> Vec<String> {
        m.iter()
            .filter(|(p, _)| p == key)
            .map(|(_, v)| v.clone())
            .collect()
    }

    #[test]
    fn a11y_text_only_book_is_textual_and_claims_no_images() {
        let m = a11y_metadata("es", 0, 0);
        assert_eq!(props(&m, "schema:accessMode"), ["textual"]);
        assert_eq!(props(&m, "schema:accessModeSufficient"), ["textual"]);
        assert!(!props(&m, "schema:accessibilityFeature").contains(&"alternativeText".into()));
        assert_eq!(props(&m, "schema:accessibilityHazard"), ["none"]);
    }

    #[test]
    fn a11y_illustrated_book_is_visual_and_claims_alt_text_when_every_image_has_it() {
        let m = a11y_metadata("en", 12, 12);
        assert_eq!(props(&m, "schema:accessMode"), ["textual", "visual"]);
        // text alone is enough to read it, because every image is described
        assert!(props(&m, "schema:accessModeSufficient").contains(&"textual".into()));
        assert!(props(&m, "schema:accessibilityFeature").contains(&"alternativeText".into()));
    }

    #[test]
    fn a11y_never_claims_alt_text_a_book_does_not_have() {
        // One undescribed image is enough to lose BOTH the alternativeText feature
        // and the text-only reading path. Claiming either would be a false
        // accessibility claim, which is worse than declaring the gap honestly.
        let m = a11y_metadata("en", 12, 11);
        assert!(!props(&m, "schema:accessibilityFeature").contains(&"alternativeText".into()));
        assert_eq!(props(&m, "schema:accessModeSufficient"), ["textual,visual"]);
        assert!(m
            .iter()
            .any(|(p, v)| p == "schema:accessibilitySummary" && v.contains("lack alternative text")));
    }

    #[test]
    fn adds_lang_to_the_generated_nav_which_the_builder_template_omits() {
        // epub-builder's nav/toc template hardcodes a bare <html>, so those two were
        // the only documents in the book with no language.
        let nav = "<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"x\">\n<body/>\n</html>";
        let out = html_lang(nav, "es");
        assert!(out.contains("lang=\"es\" xml:lang=\"es\""));
    }

    #[test]
    fn never_overwrites_a_language_that_is_already_declared() {
        let doc = "<html lang=\"en\" xml:lang=\"en\"><body/></html>";
        assert_eq!(html_lang(doc, "es"), doc);
    }

    #[test]
    fn injects_titlepage_and_copyright_landmarks_without_touching_the_toc() {
        let nav = "<nav epub:type = \"toc\" id=\"toc\">\n<ol>\n<li><a href=\"chapter-001.xhtml\">Uno</a></li>\n</ol>\n</nav>\n\
<nav epub:type = \"landmarks\">\n    <ol>\n      <li><a epub:type=\"bodymatter\" href=\"chapter-001.xhtml\">Uno</a></li>\n    </ol>\n  </nav>";
        let marks = vec![
            ("titlepage", "title.xhtml", "Portada".to_string()),
            ("copyright-page", "copyright.xhtml", "Créditos".to_string()),
        ];
        let out = inject_landmarks(nav, &marks);
        assert!(out.contains("<a epub:type=\"titlepage\" href=\"title.xhtml\">Portada</a>"));
        assert!(out.contains("<a epub:type=\"copyright-page\" href=\"copyright.xhtml\">Créditos</a>"));
        // the existing bodymatter landmark survives ...
        assert!(out.contains("epub:type=\"bodymatter\""));
        // ... and the TOC gains no entries (that is the whole reason we inject here
        // rather than titling the documents, which would list them in the contents).
        let toc = &out[..out.find("landmarks").unwrap()];
        assert!(!toc.contains("title.xhtml") && !toc.contains("copyright.xhtml"));
    }

    #[test]
    fn landmark_injection_is_a_no_op_without_a_landmarks_nav() {
        let nav = "<nav epub:type = \"toc\"><ol><li><a href=\"c1.xhtml\">Uno</a></li></ol></nav>";
        let marks = vec![("titlepage", "title.xhtml", "Portada".to_string())];
        assert_eq!(inject_landmarks(nav, &marks), nav);
    }

    #[test]
    fn strips_heading_attrs_and_captures_title() {
        let md = "# El Tortugo recuerda {.unnumbered}\n\nUn párrafo.\n";
        let (clean, title) = clean_chapter(md);
        assert_eq!(title.as_deref(), Some("El Tortugo recuerda"));
        assert!(clean.contains("# El Tortugo recuerda\n"));
        assert!(!clean.contains("{.unnumbered}"));
    }

    #[test]
    fn drops_spot_images_keeps_width_images() {
        let md = "![big](libros/x/ch01.png){width=80%}\n\n![vig](libros/x/vig.png){.spot}\n";
        let (clean, _) = clean_chapter(md);
        assert!(clean.contains("![big](libros/x/ch01.png)"));
        assert!(!clean.contains("{width=80%}"));
        assert!(!clean.contains("vig.png"));
    }

    #[test]
    fn img_width_align_reach_epub_html() {
        // width + align on a plate image should land as inline CSS on the <img>,
        // matched by src; a plain image (no attrs) is untouched.
        let md = "![p](libros/x/ch01.png){width=55% align=left}\n\n![q](images/y.jpg)\n";
        let html = markdown_to_html(&clean_chapter(md).0, &comrak_opts());
        let styled = apply_img_styles(&html, &img_styles(md));
        assert!(styled.contains("src=\"libros/x/ch01.png\""));
        assert!(styled.contains("width:55%;"));
        assert!(styled.contains("margin-left:0;margin-right:auto;"));
        // the un-attributed image gets no style attribute
        let q_tag = &styled[styled.find("src=\"images/y.jpg\"").unwrap()..];
        let q_tag = &q_tag[..q_tag.find('>').unwrap()];
        assert!(!q_tag.contains("style="));
    }

    #[test]
    fn bare_number_width_becomes_percent_in_epub() {
        let md = "![p](x.png){width=40}\n";
        let styled = apply_img_styles(
            &markdown_to_html(&clean_chapter(md).0, &comrak_opts()),
            &img_styles(md),
        );
        assert!(styled.contains("width:40%;"));
    }

    #[test]
    fn extracts_only_real_image_srcs() {
        let md = "![big](libros/x/ch01.png)\n\ntext\n\n![two](images/y.jpg)\n";
        let srcs = image_srcs(md);
        assert_eq!(srcs, vec!["libros/x/ch01.png", "images/y.jpg"]);
    }

    #[test]
    fn comrak_emits_well_formed_void_elements() {
        let html = markdown_to_html("![a](x.png)\n\n---\n\n| A | B |\n|---|---|\n| 1 | 2 |\n", &comrak_opts());
        assert!(html.contains("<img src=\"x.png\" alt=\"a\" />"));
        assert!(html.contains("<hr />"));
        assert!(html.contains("<table>"));
    }

    #[test]
    fn xhtml_doc_is_xml_prologued_and_escapes_title() {
        let doc = xhtml_doc("es", "Q & A", "<p>hi</p>\n");
        assert!(doc.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>"));
        assert!(doc.contains("<title>Q &amp; A</title>"));
        assert!(doc.contains("href=\"stylesheet.css\""));
        assert!(doc.contains("lang=\"es\""));
    }
}
