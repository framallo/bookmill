//! `bookmill kdp` — native (Python-free) KDP listing worksheet tool.
//!
//! Scaffolds `<worksheet_dir>/<slug>.md` when missing, else validates it against
//! the KDP hard limits. A faithful port of the retired `scripts/kdp-metadata.py`,
//! driven by `bookmill.toml` (title/subtitle per language) instead of the removed
//! `books.yml`. Worksheet dir comes from `[build].worksheet_dir` (default `kdp`).
//!
//! KDP limits enforced here (mirroring the Python checker):
//!   - title + subtitle ≤ 200 chars (combined)
//!   - exactly 7 keyword slots, each ≤ 50 chars
//!   - 2–3 BISAC categories
//!   - no unfilled `TODO` placeholder
//!
//! (Native `validate` additionally checks the TOML `[listing]` limits — keyword
//! count/length, blurb ≤4000, BISAC ≤3, forbidden phrases — see
//! `config::validate_book`. This tool covers the standalone `kdp/<slug>.md`.)

use crate::config::BookConfig;
use crate::discover::Repo;
use anyhow::{bail, Result};
use std::path::PathBuf;

const LIM_TITLE_SUB: usize = 200;
const LIM_KEYWORD: usize = 50;
const LIM_KEYWORDS_COUNT: usize = 7;

/// Worksheet directory relative to the repo root (configurable, default `kdp`).
fn worksheet_dir(repo: &Repo) -> &str {
    repo.config.build.worksheet_dir.as_deref().unwrap_or("kdp")
}

fn worksheet_path(repo: &Repo, slug: &str) -> PathBuf {
    repo.root.join(worksheet_dir(repo)).join(format!("{slug}.md"))
}

fn template(slug: &str, books_dir: &str, es_title: &str, en_title: &str) -> String {
    let block = |lang: &str, title: &str| -> String {
        format!(
            "\
- **title:** {title}
- **subtitle:**
- **cover:** {books_dir}/{slug}/cover/front-{lang}.png (eBook) · cover/wrap-{lang}-KDP.pdf (paperback)
- **keywords:**
  1.
  2.
  3.
  4.
  5.
  6.
  7.
- **bisac_categories:**
  1.
  2.
- **audience / reading_age:**
- **blurb:** |
  TODO
"
        )
    };
    format!(
        "\
# KDP metadata — {slug}

> Listing data for Amazon KDP. Fill every field. Keep ES and EN in sync.
> Limits: title+subtitle ≤ 200 chars · 7 keywords (≤ 50 chars each) ·
> 2-3 BISAC categories · description ≤ 4000 chars.
> The **cover** field is the file you upload to KDP (eBook front PNG + paperback
> wrap PDF); the build embeds the eBook cover from the same cover/ dir.
> Validate with: bookmill kdp {slug}

## Spanish (es)

{es}
## English (en)

{en}",
        es = block("es", es_title),
        en = block("en", en_title),
    )
}

/// Create the worksheet if missing; leave an existing one untouched.
fn scaffold(repo: &Repo, b: &BookConfig) -> Result<()> {
    let dir = repo.root.join(worksheet_dir(repo));
    std::fs::create_dir_all(&dir)?;
    let out = worksheet_path(repo, &b.slug);
    let wd = worksheet_dir(repo);
    if out.exists() {
        println!("  exists  {wd}/{}.md (left untouched)", b.slug);
        return Ok(());
    }
    let es_title = b.title.get("es").map(String::as_str).unwrap_or("");
    let en_title = b.title.get("en").map(String::as_str).unwrap_or("");
    std::fs::write(&out, template(&b.slug, &repo.config.books_dir, es_title, en_title))?;
    println!("  created {wd}/{}.md  — fill in keywords, categories, blurb", b.slug);
    Ok(())
}

/// The block of text under a `## Spanish`/`## English` marker (until the next `## `).
fn section<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let idx = text.find(marker)?;
    let rest = &text[idx + marker.len()..];
    match rest.find("\n## ") {
        Some(n) => Some(&rest[..n]),
        None => Some(rest),
    }
}

/// The value of a `- **name:**` field within a block.
fn field(block: &str, name: &str) -> Option<String> {
    let needle = format!("- **{name}:**");
    for line in block.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(&needle) {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// The numbered list items under a `- **name:**` field (lines like `  1. value`).
fn numbered(block: &str, name: &str) -> Vec<String> {
    let needle = format!("- **{name}:**");
    let mut out = Vec::new();
    let mut capture = false;
    for line in block.lines() {
        let s = line.trim();
        if s.starts_with(&needle) {
            capture = true;
            continue;
        }
        if capture {
            let first = s.chars().next();
            if first.map(|c| c.is_ascii_digit()).unwrap_or(false) && s.get(..3).map(|p| p.contains('.')).unwrap_or(false) {
                if let Some((_, v)) = s.split_once('.') {
                    out.push(v.trim().to_string());
                }
            } else if s.starts_with("- **") {
                break;
            }
        }
    }
    out
}

/// Validate an existing worksheet. Returns true when it passes.
fn check(repo: &Repo, slug: &str) -> bool {
    let wd = worksheet_dir(repo);
    let path = worksheet_path(repo, slug);
    if !path.exists() {
        println!("  MISSING {wd}/{slug}.md  (run: bookmill kdp {slug})");
        return false;
    }
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut problems: Vec<String> = Vec::new();

    if text.contains("TODO") {
        problems.push("contains unfilled TODO placeholder(s)".into());
    }
    for (label, marker) in [("es", "## Spanish"), ("en", "## English")] {
        let Some(block) = section(&text, marker) else {
            problems.push(format!("[{label}] section missing"));
            continue;
        };
        let title = field(block, "title");
        let subtitle = field(block, "subtitle");
        if let (Some(t), Some(s)) = (&title, &subtitle) {
            let total = t.chars().count() + s.chars().count();
            if total > LIM_TITLE_SUB {
                problems.push(format!("[{label}] title+subtitle {total} > {LIM_TITLE_SUB} chars"));
            }
        }
        let filled: Vec<String> = numbered(block, "keywords").into_iter().filter(|k| !k.trim().is_empty()).collect();
        if filled.len() != LIM_KEYWORDS_COUNT {
            problems.push(format!("[{label}] {} keywords (need exactly {LIM_KEYWORDS_COUNT})", filled.len()));
        }
        for k in &filled {
            let n = k.chars().count();
            if n > LIM_KEYWORD {
                let preview: String = k.chars().take(30).collect();
                problems.push(format!("[{label}] keyword too long ({n} chars): {preview}…"));
            }
        }
        let cats: Vec<String> = numbered(block, "bisac_categories").into_iter().filter(|c| !c.trim().is_empty()).collect();
        if cats.len() < 2 {
            problems.push(format!("[{label}] {} BISAC categories (need 2-3)", cats.len()));
        }
    }

    if !problems.is_empty() {
        println!("  FAIL    {wd}/{slug}.md");
        for p in &problems {
            println!("            - {p}");
        }
        return false;
    }
    println!("  OK      {wd}/{slug}.md");
    true
}

fn all_books(repo: &Repo) -> Result<Vec<BookConfig>> {
    let mut v = Vec::new();
    for d in repo.book_dirs()? {
        v.push(repo.load_book_at(&d)?.0);
    }
    Ok(v)
}

/// `bookmill kdp [book]` — scaffold missing worksheets + check them.
pub fn run(repo: &Repo, book: Option<String>) -> Result<()> {
    match book.as_deref() {
        None | Some("all") => {
            let books = all_books(repo)?;
            for b in &books {
                scaffold(repo, b)?;
            }
            let mut ok = true;
            for b in &books {
                ok = check(repo, &b.slug) && ok;
            }
            if !ok {
                bail!("one or more KDP worksheets failed validation");
            }
        }
        Some(slug) => {
            let (b, _dir) = repo.find_book(slug)?;
            if worksheet_path(repo, &b.slug).exists() {
                if !check(repo, &b.slug) {
                    bail!("KDP worksheet failed validation");
                }
            } else {
                scaffold(repo, &b)?;
            }
        }
    }
    Ok(())
}
