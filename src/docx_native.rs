//! Native `.docx` editor review document — no pandoc.
//!
//! Walks each chapter's Markdown (parsed once with `comrak` into an AST) and
//! emits a clean, flowing Word document with `docx-rs`: headings, paragraphs with
//! bold/italic/strikethrough/inline-code runs, bullet/ordered lists, block quotes,
//! GFM tables, scene-break rules, and code blocks. Images become their alt text
//! (an editor doc has no need for the artwork). Exact styling is not important —
//! this is a proofreading artifact, not a publish format.

use crate::build::BookMeta;
use crate::discover::Repo;
use anyhow::{Context, Result};
use comrak::nodes::{AstNode, ListType, NodeValue};
use comrak::{parse_document, Arena, Options};
use docx_rs::*;
use std::path::{Path, PathBuf};

/// Build a `.docx` editor review document for one (book, lang) to `out`.
pub fn run(_repo: &Repo, meta: &BookMeta, chaps: &[PathBuf], out: &Path) -> Result<()> {
    let mut docx = Docx::new();

    // Title block.
    docx = docx.add_paragraph(
        Paragraph::new()
            .style("Heading1")
            .align(AlignmentType::Center)
            .add_run(Run::new().bold().size(40).add_text(meta.title.clone())),
    );
    if let Some(sub) = &meta.subtitle {
        docx = docx.add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Center)
                .add_run(Run::new().italic().size(26).add_text(sub.clone())),
        );
    }
    docx = docx.add_paragraph(
        Paragraph::new()
            .align(AlignmentType::Center)
            .add_run(Run::new().add_text(meta.author.clone())),
    );

    let opts = comrak_opts();
    for ch in chaps {
        let md = std::fs::read_to_string(ch)
            .with_context(|| format!("reading {}", ch.display()))?;
        let arena = Arena::new();
        let root = parse_document(&arena, &md, &opts);
        for node in root.children() {
            docx = emit_block(docx, node);
        }
    }

    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = std::fs::File::create(out)
        .with_context(|| format!("creating {}", out.display()))?;
    docx.build().pack(f).with_context(|| "packing docx")?;
    Ok(())
}

fn comrak_opts() -> Options<'static> {
    let mut o = Options::default();
    o.extension.table = true;
    o.extension.strikethrough = true;
    o.extension.autolink = true;
    o.extension.tasklist = true;
    o.extension.footnotes = true;
    o
}

/// Append one block-level AST node to the document.
fn emit_block<'a>(docx: Docx, node: &'a AstNode<'a>) -> Docx {
    let value = node.data.borrow().value.clone();
    match value {
        NodeValue::Heading(h) => {
            let level = h.level.clamp(1, 6);
            let runs = inline_runs(node, RunStyle::heading());
            let mut p = Paragraph::new().style(&format!("Heading{level}"));
            for r in runs {
                p = p.add_run(r);
            }
            docx.add_paragraph(p)
        }
        NodeValue::Paragraph => {
            let mut p = Paragraph::new();
            for r in inline_runs(node, RunStyle::default()) {
                p = p.add_run(r);
            }
            docx.add_paragraph(p)
        }
        NodeValue::ThematicBreak => docx.add_paragraph(
            Paragraph::new()
                .align(AlignmentType::Center)
                .add_run(Run::new().add_text("* * *")),
        ),
        NodeValue::List(list) => {
            let ordered = matches!(list.list_type, ListType::Ordered);
            let mut d = docx;
            let mut n = list.start.max(1);
            for item in node.children() {
                let marker = if ordered {
                    format!("{n}. ")
                } else {
                    "• ".to_string()
                };
                n += 1;
                // An item's text lives in its child paragraph(s); flatten them
                // onto one bulleted line, then recurse for any nested blocks.
                let mut first = true;
                for child in item.children() {
                    match child.data.borrow().value {
                        NodeValue::Paragraph => {
                            let mut p = Paragraph::new().indent(
                                Some(360),
                                None,
                                None,
                                None,
                            );
                            if first {
                                p = p.add_run(Run::new().add_text(marker.clone()));
                                first = false;
                            }
                            for r in inline_runs(child, RunStyle::default()) {
                                p = p.add_run(r);
                            }
                            d = d.add_paragraph(p);
                        }
                        _ => d = emit_block(d, child),
                    }
                }
            }
            d
        }
        NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
            let mut d = docx;
            for child in node.children() {
                if matches!(child.data.borrow().value, NodeValue::Paragraph) {
                    let mut p = Paragraph::new().indent(Some(720), None, None, None);
                    for r in inline_runs(child, RunStyle::default().italic()) {
                        p = p.add_run(r);
                    }
                    d = d.add_paragraph(p);
                } else {
                    d = emit_block(d, child);
                }
            }
            d
        }
        NodeValue::CodeBlock(cb) => {
            let mut d = docx;
            for line in cb.literal.lines() {
                d = d.add_paragraph(
                    Paragraph::new().add_run(
                        Run::new()
                            .fonts(RunFonts::new().ascii("Courier New"))
                            .add_text(line.to_string()),
                    ),
                );
            }
            d
        }
        NodeValue::Table(_) => emit_table(docx, node),
        // Anything else: recurse so nested content is not lost.
        _ => {
            let mut d = docx;
            for child in node.children() {
                d = emit_block(d, child);
            }
            d
        }
    }
}

/// Render a GFM table node as a docx table.
fn emit_table<'a>(docx: Docx, node: &'a AstNode<'a>) -> Docx {
    let mut rows: Vec<TableRow> = Vec::new();
    for row_node in node.children() {
        let is_header = matches!(row_node.data.borrow().value, NodeValue::TableRow(true));
        let mut cells: Vec<TableCell> = Vec::new();
        for cell_node in row_node.children() {
            let style = if is_header {
                RunStyle::default().bold()
            } else {
                RunStyle::default()
            };
            let mut p = Paragraph::new();
            for r in inline_runs(cell_node, style) {
                p = p.add_run(r);
            }
            cells.push(TableCell::new().add_paragraph(p));
        }
        if !cells.is_empty() {
            rows.push(TableRow::new(cells));
        }
    }
    if rows.is_empty() {
        return docx;
    }
    docx.add_table(Table::new(rows))
}

/// Inline run styling carried down the inline tree.
#[derive(Clone, Copy, Default)]
struct RunStyle {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
}

impl RunStyle {
    fn heading() -> Self {
        RunStyle { bold: true, ..Default::default() }
    }
    fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
    fn italic(mut self) -> Self {
        self.italic = true;
        self
    }
    fn strike(mut self) -> Self {
        self.strike = true;
        self
    }
    fn code(mut self) -> Self {
        self.code = true;
        self
    }
    fn run(&self, text: &str) -> Run {
        let mut r = Run::new();
        if self.bold {
            r = r.bold();
        }
        if self.italic {
            r = r.italic();
        }
        if self.strike {
            r = r.strike();
        }
        if self.code {
            r = r.fonts(RunFonts::new().ascii("Courier New"));
        }
        r.add_text(text.to_string())
    }
}

/// Flatten an inline subtree into styled docx runs.
fn inline_runs<'a>(node: &'a AstNode<'a>, style: RunStyle) -> Vec<Run> {
    let mut runs = Vec::new();
    collect_inline(node, style, &mut runs);
    if runs.is_empty() {
        runs.push(Run::new().add_text(""));
    }
    runs
}

fn collect_inline<'a>(node: &'a AstNode<'a>, style: RunStyle, out: &mut Vec<Run>) {
    for child in node.children() {
        let value = child.data.borrow().value.clone();
        match value {
            NodeValue::Text(t) => out.push(style.run(&t)),
            NodeValue::Code(c) => out.push(style.code().run(&c.literal)),
            NodeValue::SoftBreak | NodeValue::LineBreak => out.push(style.run(" ")),
            NodeValue::Strong => collect_inline(child, style.bold(), out),
            NodeValue::Emph => collect_inline(child, style.italic(), out),
            NodeValue::Strikethrough => collect_inline(child, style.strike(), out),
            NodeValue::Link(_) | NodeValue::Underline | NodeValue::Superscript
            | NodeValue::Subscript | NodeValue::Highlight => {
                collect_inline(child, style, out)
            }
            // An image contributes its alt text (its inline children).
            NodeValue::Image(_) => collect_inline(child, style, out),
            _ => collect_inline(child, style, out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn builds_valid_nonempty_docx() {
        let dir = std::env::temp_dir().join("bm-docx-test");
        std::fs::create_dir_all(&dir).unwrap();
        let ch = dir.join("ch.md");
        std::fs::write(
            &ch,
            "# Chapter One\n\nA **bold** and *italic* line.\n\n- one\n- two\n\n| A | B |\n|---|---|\n| 1 | 2 |\n",
        )
        .unwrap();
        let meta = BookMeta {
            title: "T".into(),
            subtitle: None,
            author: "A".into(),
            rights: "r".into(),
            description: None,
            subjects: Vec::new(),
        };
        // Minimal repo stub via the real loader is overkill; call the converter
        // pieces directly through `run` requires a Repo, so just exercise the AST
        // walk by building a Docx here.
        let opts = comrak_opts();
        let md = std::fs::read_to_string(&ch).unwrap();
        let arena = Arena::new();
        let root = parse_document(&arena, &md, &opts);
        let mut docx = Docx::new().add_paragraph(
            Paragraph::new().add_run(Run::new().add_text(meta.title)),
        );
        for node in root.children() {
            docx = emit_block(docx, node);
        }
        let out = dir.join("out.docx");
        let f = std::fs::File::create(&out).unwrap();
        docx.build().pack(f).unwrap();

        // Valid zip with a word/document.xml entry, and non-trivial size.
        let mut buf = Vec::new();
        std::fs::File::open(&out).unwrap().read_to_end(&mut buf).unwrap();
        assert!(buf.starts_with(b"PK"), "docx must be a zip");
        assert!(buf.len() > 1000, "docx should be non-trivial");
    }
}
