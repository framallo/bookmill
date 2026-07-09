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
use anyhow::{Context, Result};
use comrak::{markdown_to_html, Options};
use epub_builder::{EpubBuilder, EpubContent, EpubVersion, ReferenceType, ZipLibrary};
use std::collections::BTreeSet;
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

    // Shared stylesheet (css/epub.css). Optional — books without it still build.
    let css_path = repo.root.join("css/epub.css");
    if css_path.exists() {
        let css = std::fs::read(&css_path)
            .with_context(|| format!("reading {}", css_path.display()))?;
        b.stylesheet(&css[..]).map_err(anyhow::Error::msg)?;
    }

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

    // Generated title page (not listed in the nav).
    let title_xhtml = title_page(meta, lang);
    b.add_content(EpubContent::new("title.xhtml", title_xhtml.as_bytes()))
        .map_err(anyhow::Error::msg)?;

    // Copyright page from copyright-epub.md (not listed in the nav).
    if let Some(cp) = cepub {
        let md = std::fs::read_to_string(cp)
            .with_context(|| format!("reading {}", cp.display()))?;
        let (clean, _) = clean_chapter(&md);
        let body = markdown_to_html(&clean, &comrak_opts());
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

        let html = markdown_to_html(&clean, &comrak_opts());
        let body = if captions { figcaption_plates(&html) } else { html };
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

    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = std::fs::File::create(out)
        .with_context(|| format!("creating {}", out.display()))?;
    b.generate(f).map_err(anyhow::Error::msg)?;
    Ok(())
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
