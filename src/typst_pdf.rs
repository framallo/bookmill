//! Native Typst PDF engine (opt-in via `--engine typst`).
//!
//! Converts the resolved chapter markdown into a single Typst document and
//! compiles it with the `typst` CLI — a step toward dropping the external
//! pandoc/xelatex toolchain. The default PDF path stays on pandoc; this engine
//! is selected only when the user passes `--engine typst`.
//!
//! Reproduced interior conventions:
//!   * page geometry (paper size incl. bleed + margins) from `PageGeometry`;
//!   * chapters open on a recto page when `openright`;
//!   * auto-numbered chapter headings ("Capítulo N" / "Chapter N", terracotta),
//!     with `{.unnumbered}` headings (the epilogue) showing only the title;
//!   * full-page chapter-opening plates that bleed to all four edges;
//!   * small centered `.spot` tailpiece images (~2.4in);
//!   * a title page, a copyright page, and a table of contents.
//!
//! The Markdown -> Typst converter is intentionally small: headings (with the
//! `{.unnumbered}`/`{.spot}` attributes), paragraphs, `**bold**`/`*italic*`,
//! links, standalone images, `---` scene breaks, GFM pipe tables, and inline
//! `^[...]` footnotes. Anything else falls through as escaped literal text.
//! Reference-style GFM footnotes (`[^id]` + `[^id]:` definitions) are deferred:
//! they need a two-pass collect across blocks and no book in the series uses
//! them; inline `^[...]` covers the footnote need for now.

use crate::build::{BookMeta, PageGeometry};
use crate::discover::Repo;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Render a book (one language / one PDF flavor) to `out` via the `typst` CLI.
#[allow(clippy::too_many_arguments)]
pub fn run(
    repo: &Repo,
    meta: &BookMeta,
    cpdf: Option<&Path>,
    chaps: &[PathBuf],
    openright: bool,
    retail: bool,
    cover: Option<&Path>,
    geometry: Option<PageGeometry>,
    lang: &str,
    out: &Path,
) -> Result<()> {
    let doc = build_doc(repo, meta, cpdf, chaps, openright, retail, cover, geometry, lang)?;
    let odir = out.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(odir)?;
    let typ = odir.join(".typst-build.typ");
    std::fs::write(&typ, doc).with_context(|| format!("writing {}", typ.display()))?;

    let mut c = Command::new("typst");
    c.arg("compile")
        .arg("--root")
        .arg(&repo.root)
        .arg(&typ)
        .arg(out);
    let st = c
        .status()
        .with_context(|| "running typst (is the `typst` CLI installed?)")?;
    if !st.success() {
        bail!("typst compile failed for {}", out.display());
    }
    Ok(())
}

// ---------- document assembly ----------

#[allow(clippy::too_many_arguments)]
fn build_doc(
    repo: &Repo,
    meta: &BookMeta,
    cpdf: Option<&Path>,
    chaps: &[PathBuf],
    openright: bool,
    retail: bool,
    cover: Option<&Path>,
    geometry: Option<PageGeometry>,
    lang: &str,
) -> Result<String> {
    let g = geometry.unwrap_or(PageGeometry {
        pw: 6.0,
        ph: 9.0,
        top: 0.75,
        bottom: 0.75,
        inner: 0.75,
        outer: 0.6,
        bindingoffset: 0.375,
    });
    let inside = g.inner + g.bindingoffset;
    let chlabel = if lang == "es" { "Capítulo" } else { "Chapter" };
    let toc_title = if lang == "es" { "Índice" } else { "Contents" };
    // recto-open break used before each chapter heading
    let recto = if openright {
        "pagebreak(to: \"odd\", weak: true)"
    } else {
        "pagebreak(weak: true)"
    };

    let mut s = String::new();
    // ---- preamble ----
    s.push_str(&format!(
        "#set document(title: {t}, author: {a})\n",
        t = ty_str(&meta.title),
        a = ty_str(&meta.author),
    ));
    s.push_str(&format!(
        "#set page(width: {pw:.4}in, height: {ph:.4}in, margin: (top: {top:.4}in, \
bottom: {bottom:.4}in, inside: {inside:.4}in, outside: {outside:.4}in))\n",
        pw = g.pw,
        ph = g.ph,
        top = g.top,
        bottom = g.bottom,
        inside = inside,
        outside = g.outer,
    ));
    // Retail-only paper-texture page background (mirrors the pandoc retail path's
    // `pdf/texture-bg.tex`): scale `<repo>/images/paper-texture.jpg` to the full
    // page behind every page. KDP print PDFs must NOT get it. Skip silently if the
    // image is absent.
    if retail && repo.root.join("images/paper-texture.jpg").exists() {
        s.push_str(
            "#set page(background: image(\"/images/paper-texture.jpg\", width: 100%, height: 100%))\n",
        );
    }
    s.push_str(&format!("#set text(size: 11pt, lang: {})\n", ty_str(lang)));
    s.push_str("#set par(justify: true, leading: 0.72em, first-line-indent: 1.2em)\n");
    s.push_str("#set heading(numbering: \"1\")\n");
    s.push_str("#let islatitle = rgb(\"#C2571C\")\n");
    s.push_str(&format!("#let chlabel = {}\n", ty_str(chlabel)));
    // Full-bleed chapter plate on the verso (left) page, facing the chapter
    // opener on the recto — mirrors the LaTeX `\cleartoverso` + `\AddThisPageImage`.
    // The image page has zero margin (fills the paper edge-to-edge incl. bleed);
    // the scoped `set page` reverts to the body geometry on the following page.
    s.push_str(
        "#let plate(p) = [\n  #pagebreak(to: \"even\", weak: true)\n  \
#set page(margin: 0pt, header: none, footer: none)\n  \
#image(p, width: 100%, height: 100%, fit: \"cover\")\n  #pagebreak()\n]\n",
    );
    // small centered tailpiece (spot)
    s.push_str(
        // Fit inside a 2.4in box (preserve aspect), mirroring pandoc's
        // keepaspectratio — a tall vignette stays narrow so it keeps ≥300dpi.
        "#let spot(p) = { v(1.5em); align(center, image(p, width: 2.4in, height: 2.4in, fit: \"contain\")); v(1em) }\n",
    );
    // centered scene break
    s.push_str(
        "#let scenebreak = { v(0.6em); align(center)[#sym.dot.c#h(0.6em)#sym.dot.c#h(0.6em)#sym.dot.c]; v(0.6em) }\n",
    );
    // chapter heading: terracotta "Capítulo N" label line + centered title
    s.push_str(&format!(
        "#show heading.where(level: 1): it => {{\n  {recto}\n  \
block(width: 100%, above: 20pt, below: 40pt, {{\n    set align(center)\n    \
if it.numbering != none {{\n      \
text(fill: islatitle, size: 14pt, smallcaps[#chlabel #context counter(heading).display(\"1\")])\n      \
linebreak()\n      v(12pt)\n    }}\n    \
text(fill: islatitle, weight: \"bold\", size: 22pt, it.body)\n  }})\n}}\n",
    ));
    s.push_str(
        "#show heading.where(level: 2): it => block(above: 1.2em, below: 0.6em, \
text(weight: \"bold\", size: 13pt, it.body))\n",
    );
    s.push_str("\n");

    // Front matter (copyright page, TOC) is numbered in lowercase roman; the body
    // restarts at arabic 1 at the first chapter (see the `numbering: "1"` +
    // counter reset below). Title/cover pages pass `footer: none`, so no visible
    // number there, but the counter still advances like a real book's front matter.
    s.push_str("#set page(numbering: \"i\")\n");

    // ---- front matter ----
    // optional retail cover plate (page 1)
    if retail {
        if let Some(cv) = cover {
            let p = typst_img_path(&repo.root, &cv.display().to_string());
            s.push_str(&format!(
                "#page(margin: 0pt, footer: none, header: none)[#image({}, width: 100%, height: 100%, fit: \"cover\")]\n",
                ty_str(&p)
            ));
        }
    }
    // title page
    s.push_str("#page(footer: none)[\n  #v(1fr)\n  #align(center)[\n");
    s.push_str(&format!(
        "    #text(size: 28pt, weight: \"bold\")[{}]\n",
        inline(&meta.title)
    ));
    if let Some(sub) = &meta.subtitle {
        s.push_str(&format!(
            "    #v(10pt)\n    #text(size: 15pt, style: \"italic\")[{}]\n",
            inline(sub)
        ));
    }
    s.push_str(&format!(
        "    #v(28pt)\n    #text(size: 13pt)[{}]\n",
        inline(&meta.author)
    ));
    s.push_str("  ]\n  #v(1fr)\n]\n");

    // copyright page + table of contents (only when the book ships a copyright)
    if let Some(cp) = cpdf {
        let txt = std::fs::read_to_string(cp)
            .with_context(|| format!("reading {}", cp.display()))?;
        let blocks = parse_blocks(&txt);
        s.push_str("#page(footer: none)[\n  #v(1fr)\n  #set align(center)\n  #set text(size: 10pt)\n");
        for b in &blocks {
            if let Block::Para(t) = b {
                s.push_str(&format!("  {}\n\n", inline(t)));
            }
        }
        s.push_str("  #v(1fr)\n]\n");
        s.push_str(&format!(
            "#outline(title: [{}], depth: 1, indent: auto)\n",
            inline(toc_title)
        ));
    }

    // start body page numbering at 1
    s.push_str("#set page(numbering: \"1\")\n#counter(page).update(1)\n\n");

    // ---- chapters ----
    for ch in chaps {
        let txt = std::fs::read_to_string(ch)
            .with_context(|| format!("reading {}", ch.display()))?;
        let blocks = parse_blocks(&txt);
        emit_blocks(&mut s, &blocks, &repo.root);
        s.push('\n');
    }

    Ok(s)
}

/// Emit a chapter's blocks, applying the heading+plate reorder: a chapter that
/// opens with a standalone (non-spot) image renders the image as a full-page
/// verso plate BEFORE its heading, so heading+body open together on the recto.
fn emit_blocks(s: &mut String, blocks: &[Block], root: &Path) {
    let mut i = 0;
    while i < blocks.len() {
        match &blocks[i] {
            Block::Heading { level, text, unnumbered } if *level == 1 => {
                // look ahead for an opening plate image
                if let Some(Block::Image { src, spot: false, .. }) = blocks.get(i + 1) {
                    let p = typst_img_path(root, src);
                    s.push_str(&format!("#plate({})\n", ty_str(&p)));
                    emit_heading(s, *level, text, *unnumbered);
                    i += 2;
                    continue;
                }
                emit_heading(s, *level, text, *unnumbered);
                i += 1;
            }
            Block::Heading { level, text, unnumbered } => {
                emit_heading(s, *level, text, *unnumbered);
                i += 1;
            }
            Block::Image { src, spot, width } => {
                let p = typst_img_path(root, src);
                if *spot {
                    s.push_str(&format!("#spot({})\n", ty_str(&p)));
                } else {
                    let w = width.unwrap_or(100.0);
                    s.push_str(&format!(
                        "#align(center, image({}, width: {w}%))\n",
                        ty_str(&p)
                    ));
                }
                i += 1;
            }
            Block::Table { header, rows } => {
                emit_table(s, header, rows);
                i += 1;
            }
            Block::Rule => {
                s.push_str("#scenebreak\n");
                i += 1;
            }
            Block::Para(t) => {
                s.push_str(&inline(t));
                s.push_str("\n\n");
                i += 1;
            }
        }
    }
}

/// Emit a GFM table as a Typst `#table(...)` with bold header cells. Column count
/// is taken from the header; short body rows are padded so the grid stays valid.
fn emit_table(s: &mut String, header: &[String], rows: &[Vec<String>]) {
    let cols = header.len().max(1);
    s.push_str(&format!(
        "#table(\n  columns: {cols},\n  table.header({}),\n",
        header
            .iter()
            .map(|c| format!("[*{}*]", inline(c)))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    for row in rows {
        let mut cells: Vec<String> =
            row.iter().map(|c| format!("[{}]", inline(c))).collect();
        while cells.len() < cols {
            cells.push("[]".to_string());
        }
        cells.truncate(cols);
        s.push_str(&format!("  {},\n", cells.join(", ")));
    }
    s.push_str(")\n\n");
}

fn emit_heading(s: &mut String, level: usize, text: &str, unnumbered: bool) {
    let body = inline(text);
    if level == 1 {
        if unnumbered {
            s.push_str(&format!("#heading(level: 1, numbering: none)[{body}]\n"));
        } else {
            s.push_str(&format!("= {body}\n"));
        }
    } else {
        s.push_str(&format!("{} {body}\n", "=".repeat(level)));
    }
}

// ---------- block parsing ----------

enum Block {
    Heading { level: usize, text: String, unnumbered: bool },
    Para(String),
    Image { src: String, spot: bool, width: Option<f64> },
    Rule,
    /// GFM pipe table: a header row plus body rows, each a vector of raw cell
    /// strings (inline markdown, converted at emit time).
    Table { header: Vec<String>, rows: Vec<Vec<String>> },
}

/// Split markdown into blocks. Drops fenced code blocks (```; used by the
/// copyright pages to carry raw LaTeX, which Typst must not see). Recognizes
/// ATX headings, `---`/`***` scene-break rules, standalone images, and
/// blank-line-delimited paragraphs.
fn parse_blocks(md: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut para: Vec<String> = Vec::new();
    let mut in_fence = false;

    let flush = |para: &mut Vec<String>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Para(para.join(" ")));
            para.clear();
        }
    };

    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end();
        let trimmed = line.trim();

        // fenced code block: drop everything inside (and the fences)
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            flush(&mut para, &mut out);
            in_fence = !in_fence;
            i += 1;
            continue;
        }
        if in_fence {
            i += 1;
            continue;
        }

        if trimmed.is_empty() {
            flush(&mut para, &mut out);
            i += 1;
            continue;
        }

        // GFM pipe table: a `| … |` header row immediately followed by a
        // `|---|---|` separator row. Consumes the whole contiguous table.
        if is_table_row(trimmed)
            && lines
                .get(i + 1)
                .map(|l| is_table_separator(l.trim()))
                .unwrap_or(false)
        {
            flush(&mut para, &mut out);
            let header = split_table_row(trimmed);
            let mut rows = Vec::new();
            let mut j = i + 2;
            while j < lines.len() {
                let t = lines[j].trim();
                if t.is_empty() || !is_table_row(t) {
                    break;
                }
                rows.push(split_table_row(t));
                j += 1;
            }
            out.push(Block::Table { header, rows });
            i = j;
            continue;
        }

        // scene-break rule: a line of only -, *, or _ (>=3)
        if is_rule(trimmed) {
            flush(&mut para, &mut out);
            out.push(Block::Rule);
            i += 1;
            continue;
        }

        // ATX heading
        if let Some(h) = parse_heading(trimmed) {
            flush(&mut para, &mut out);
            out.push(h);
            i += 1;
            continue;
        }

        // standalone image paragraph
        if let Some(img) = parse_image(trimmed) {
            flush(&mut para, &mut out);
            out.push(img);
            i += 1;
            continue;
        }

        para.push(line.to_string());
        i += 1;
    }
    flush(&mut para, &mut out);
    out
}

/// A line that looks like a GFM table row: contains a `|` and (after trimming a
/// single optional leading/trailing pipe) is non-empty.
fn is_table_row(s: &str) -> bool {
    let s = s.trim();
    s.contains('|') && s.trim_matches('|').contains(|c| c != '|')
}

/// A GFM header/body separator row, e.g. `|---|:--:|---:|`. Cells contain only
/// `-`, `:`, and spaces, and at least one `-`.
fn is_table_separator(s: &str) -> bool {
    if !s.contains('|') || !s.contains('-') {
        return false;
    }
    split_table_row(s)
        .iter()
        .all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' '))
}

/// Split a `| a | b | c |` row into trimmed cell strings, dropping the optional
/// leading/trailing pipe. Does not handle escaped `\|` inside cells (unused here).
fn split_table_row(s: &str) -> Vec<String> {
    let s = s.trim();
    let s = s.strip_prefix('|').unwrap_or(s);
    let s = s.strip_suffix('|').unwrap_or(s);
    s.split('|').map(|c| c.trim().to_string()).collect()
}

fn is_rule(s: &str) -> bool {
    let s = s.replace(' ', "");
    s.len() >= 3
        && (s.chars().all(|c| c == '-') || s.chars().all(|c| c == '*') || s.chars().all(|c| c == '_'))
}

fn parse_heading(s: &str) -> Option<Block> {
    if !s.starts_with('#') {
        return None;
    }
    let level = s.chars().take_while(|&c| c == '#').count();
    let rest = s[level..].trim_start();
    if rest.is_empty() {
        return None;
    }
    // strip a trailing pandoc attribute block: `# Title {.unnumbered}`
    let (title, attrs) = split_attrs(rest);
    let unnumbered = attrs.contains(".unnumbered");
    Some(Block::Heading {
        level: level.min(6),
        text: title.trim().to_string(),
        unnumbered,
    })
}

/// Parse a standalone `![alt](src){attrs}` line. Returns None if the line is not
/// solely an image.
fn parse_image(s: &str) -> Option<Block> {
    if !s.starts_with("![") {
        return None;
    }
    let close_alt = s.find("](")?;
    let after = &s[close_alt + 2..];
    let close_paren = after.find(')')?;
    let src = after[..close_paren].trim().to_string();
    let tail = after[close_paren + 1..].trim();
    // must be only an image (optionally followed by an attribute block)
    if !tail.is_empty() && !tail.starts_with('{') {
        return None;
    }
    let (_, attrs) = split_attrs(s);
    let spot = attrs.contains(".spot");
    let width = parse_width(&attrs);
    Some(Block::Image { src, spot, width })
}

/// Split a trailing `{ ... }` pandoc attribute block off the end of a line.
/// Returns (content_without_attrs, attrs_inner).
fn split_attrs(s: &str) -> (String, String) {
    let s = s.trim();
    if s.ends_with('}') {
        if let Some(open) = s.rfind('{') {
            return (s[..open].trim().to_string(), s[open + 1..s.len() - 1].to_string());
        }
    }
    (s.to_string(), String::new())
}

/// Pull a percentage width out of an attribute string like `width=80%`.
fn parse_width(attrs: &str) -> Option<f64> {
    let idx = attrs.find("width=")?;
    let rest = &attrs[idx + 6..];
    let val: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    val.parse().ok()
}

// ---------- inline conversion ----------

/// Convert markdown inline text to Typst markup: `**bold**` -> `*bold*`,
/// `*italic*`/`_italic_` -> `_italic_`, `[t](u)` -> `#link("u")[t]`, everything
/// else escaped as literal text.
fn inline(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // inline footnote ^[text]  ->  #footnote[text]
        // (pandoc-style inline footnote; reference-style GFM `[^id]` is deferred —
        // see module note. No book in the series uses footnotes yet.)
        if c == '^' && chars.get(i + 1) == Some(&'[') {
            if let Some(end) = find_char(&chars, i + 2, ']') {
                let inner: String = chars[i + 2..end].iter().collect();
                out.push_str("#footnote[");
                out.push_str(&inline(&inner));
                out.push(']');
                i = end + 1;
                continue;
            }
        }
        // bold **...**
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            if let Some(end) = find_seq(&chars, i + 2, &['*', '*']) {
                let inner: String = chars[i + 2..end].iter().collect();
                out.push('*');
                out.push_str(&inline(&inner));
                out.push('*');
                i = end + 2;
                continue;
            }
        }
        // italic *...* or _..._
        if c == '*' || c == '_' {
            if let Some(end) = find_char(&chars, i + 1, c) {
                let inner: String = chars[i + 1..end].iter().collect();
                if !inner.is_empty() {
                    out.push('_');
                    out.push_str(&inline(&inner));
                    out.push('_');
                    i = end + 1;
                    continue;
                }
            }
        }
        // link [text](url)
        if c == '[' {
            if let Some((text, url, next)) = parse_inline_link(&chars, i) {
                out.push_str(&format!(
                    "#link({})[{}]",
                    ty_str(&url),
                    inline(&text)
                ));
                i = next;
                continue;
            }
        }
        push_escaped(&mut out, c);
        i += 1;
    }
    out
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

/// Parse `[text](url)` starting at `start` (which must be `[`). Returns
/// (text, url, index_after).
fn parse_inline_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let close = find_char(chars, start + 1, ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let close_paren = find_char(chars, close + 2, ')')?;
    let text: String = chars[start + 1..close].iter().collect();
    let url: String = chars[close + 2..close_paren].iter().collect();
    Some((text, url, close_paren + 1))
}

/// Escape a single char that is special in Typst markup.
fn push_escaped(out: &mut String, c: char) {
    match c {
        '\\' | '#' | '$' | '*' | '_' | '`' | '<' | '>' | '@' | '[' | ']' | '~' | '^' => {
            out.push('\\');
            out.push(c);
        }
        _ => out.push(c),
    }
}

/// Quote a string as a Typst string literal.
fn ty_str(s: &str) -> String {
    let esc = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{esc}\"")
}

/// Map a markdown image src to a Typst (`--root`-relative) path. Markdown srcs
/// are relative to the repo root (the pandoc build runs there); absolute srcs
/// under the repo are made root-relative; URLs are left untouched.
fn typst_img_path(root: &Path, src: &str) -> String {
    if src.starts_with("http://") || src.starts_with("https://") {
        return src.to_string();
    }
    let p = Path::new(src);
    if p.is_absolute() {
        if let Ok(rel) = p.strip_prefix(root) {
            return format!("/{}", rel.display());
        }
        return src.to_string();
    }
    format!("/{src}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_emphasis_and_escape() {
        assert_eq!(inline("plain text"), "plain text");
        assert_eq!(inline("**bold**"), "*bold*");
        assert_eq!(inline("*italic*"), "_italic_");
        assert_eq!(inline("a #b [c]"), "a \\#b \\[c\\]");
    }

    #[test]
    fn inline_link() {
        assert_eq!(inline("[t](http://x)"), "#link(\"http://x\")[t]");
    }

    #[test]
    fn heading_unnumbered() {
        match parse_heading("# Epílogo {.unnumbered}").unwrap() {
            Block::Heading { level, text, unnumbered } => {
                assert_eq!(level, 1);
                assert_eq!(text, "Epílogo");
                assert!(unnumbered);
            }
            _ => panic!("not a heading"),
        }
    }

    #[test]
    fn image_spot_and_width() {
        match parse_image("![a](x.png){.spot}").unwrap() {
            Block::Image { src, spot, .. } => {
                assert_eq!(src, "x.png");
                assert!(spot);
            }
            _ => panic!("not an image"),
        }
        match parse_image("![a](y.png){width=80%}").unwrap() {
            Block::Image { width, spot, .. } => {
                assert_eq!(width, Some(80.0));
                assert!(!spot);
            }
            _ => panic!("not an image"),
        }
    }

    #[test]
    fn rule_detect() {
        assert!(is_rule("---"));
        assert!(is_rule("* * *"));
        assert!(!is_rule("-- a"));
    }

    #[test]
    fn inline_footnote() {
        assert_eq!(inline("a^[note]b"), "a#footnote[note]b");
        // a bare caret is escaped (Typst superscript), not treated as a footnote
        assert_eq!(inline("2^3"), "2\\^3");
    }

    #[test]
    fn table_parse_and_emit() {
        let md = "| A | B |\n|---|---|\n| 1 | two |\n| 3 | |\n";
        let blocks = parse_blocks(md);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            Block::Table { header, rows } => {
                assert_eq!(header, &vec!["A".to_string(), "B".to_string()]);
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0], vec!["1".to_string(), "two".to_string()]);
            }
            _ => panic!("not a table"),
        }
        let mut s = String::new();
        emit_blocks(&mut s, &blocks, Path::new("/repo"));
        assert!(s.contains("#table("));
        assert!(s.contains("columns: 2"));
        assert!(s.contains("table.header([*A*], [*B*])"));
        assert!(s.contains("[1], [two]"));
        // a separator line alone (no header above) must not become a table
        assert!(!s.contains("table.header([])"));
    }

    #[test]
    fn table_separator_detection() {
        assert!(is_table_separator("|---|---|"));
        assert!(is_table_separator("| :--- | ---: | :--: |"));
        assert!(!is_table_separator("| a | b |"));
        assert!(!is_table_separator("---")); // scene-break rule, not a table sep
    }

    #[test]
    fn img_path_rooting() {
        let root = Path::new("/repo");
        assert_eq!(typst_img_path(root, "libros/a/ch01.png"), "/libros/a/ch01.png");
        assert_eq!(typst_img_path(root, "/repo/libros/a/ch01.png"), "/libros/a/ch01.png");
    }
}
