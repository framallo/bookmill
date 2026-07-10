//! Post-build book documents generated into `output/`:
//!   * `<slug>-<lang>.md` — the combined manuscript (front matter + all chapters);
//!   * `<slug>-<lang>-sample.{epub,pdf}` — a free sample of the opening chapters;
//!   * `README.md` (per book) — title/author/series, per-language word & page
//!     counts, KDP listing (keywords/BISAC/blurb/reading age), status/ASINs, and
//!     the list of files built.
//!
//! These are side-effects of a normal build, not queue jobs and not edition-
//! specific: after the interior queue finishes, the CLI regenerates them for
//! every book+language it touched. All best-effort — a failure here warns but
//! never fails an otherwise-successful build.

use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::build::{resolve_book_meta, resolve_geometry, BookMeta, Job, Out};
use crate::config::{self, BookConfig};
use crate::discover::Repo;

/// Regenerate the markdown export + free sample (per built language) and the
/// README (per book) for every book the job list touched. Best-effort: prints a
/// warning per book on failure and never returns an error.
pub fn generate_from_jobs(repo: &Repo, jobs: &[Job]) {
    // slug -> (book dir, languages built)
    let mut books: BTreeMap<String, (PathBuf, BTreeSet<String>)> = BTreeMap::new();
    for j in jobs {
        let e = books
            .entry(j.slug.clone())
            .or_insert_with(|| (j.dir.clone(), BTreeSet::new()));
        e.1.insert(j.lang.clone());
    }
    for (slug, (dir, langs)) in &books {
        if let Err(e) = generate_one(repo, dir, langs) {
            eprintln!("  ! book docs for {slug}: {e:#}");
        }
    }
}

fn generate_one(repo: &Repo, dir: &Path, langs: &BTreeSet<String>) -> Result<()> {
    let (book, _) = repo.load_book_at(dir)?;
    let mut made: Vec<String> = Vec::new();
    for lang in langs {
        let Some(content) = book.content.get(lang) else {
            continue;
        };
        let chaps = content.resolve(dir)?;
        if chaps.is_empty() {
            continue;
        }
        let md = write_markdown(repo, &book, lang, &chaps)?;
        made.push(rel(repo, &md));
        if config::sample_enabled(&book) {
            match build_sample(repo, &book, dir, lang, &chaps) {
                Ok(files) => made.extend(files.iter().map(|p| rel(repo, p))),
                Err(e) => eprintln!("  ! sample for {}/{lang}: {e:#}", book.slug),
            }
        }
    }
    let readme = write_readme(repo, &book, dir)?;
    made.push(rel(repo, &readme));
    if !made.is_empty() {
        println!("  + book docs: {}", made.join(", "));
    }
    Ok(())
}

// ---------- 1. combined markdown ----------

fn write_markdown(
    repo: &Repo,
    book: &BookConfig,
    lang: &str,
    chaps: &[PathBuf],
) -> Result<PathBuf> {
    let m = resolve_book_meta(repo, book, lang)?;
    let odir = out_dir(repo, &book.slug).join(lang);
    std::fs::create_dir_all(&odir)?;
    let out = odir.join(format!("{}-{lang}.md", book.slug));

    let mut s = String::new();
    s.push_str("---\n");
    s.push_str(&format!("title: {}\n", yaml_str(&m.title)));
    if let Some(sub) = &m.subtitle {
        s.push_str(&format!("subtitle: {}\n", yaml_str(sub)));
    }
    s.push_str(&format!("author: {}\n", yaml_str(&m.author)));
    s.push_str(&format!("language: {lang}\n"));
    s.push_str(&format!("rights: {}\n", yaml_str(&m.rights)));
    s.push_str("---\n\n");
    s.push_str(&format!("# {}\n\n", m.title));
    if let Some(sub) = &m.subtitle {
        s.push_str(&format!("*{sub}*\n\n"));
    }
    for (i, ch) in chaps.iter().enumerate() {
        let body =
            std::fs::read_to_string(ch).with_context(|| format!("reading {}", ch.display()))?;
        if i > 0 {
            s.push_str("\n\n");
        }
        s.push_str(body.trim_end());
        s.push('\n');
    }
    std::fs::write(&out, s).with_context(|| format!("writing {}", out.display()))?;
    Ok(out)
}

// ---------- 2. free sample ----------

fn build_sample(
    repo: &Repo,
    book: &BookConfig,
    dir: &Path,
    lang: &str,
    chaps: &[PathBuf],
) -> Result<Vec<PathBuf>> {
    let n = config::sample_count(book, chaps.len());
    if n == 0 {
        return Ok(vec![]);
    }
    let odir = out_dir(repo, &book.slug).join(lang);
    std::fs::create_dir_all(&odir)?;

    // Sample metadata: mark the title so a reader's library shows it's a preview.
    let base = resolve_book_meta(repo, book, lang)?;
    let suffix = if lang == "es" { "Muestra" } else { "Sample" };
    let m = BookMeta {
        title: format!("{} ({suffix})", base.title),
        subtitle: base.subtitle.clone(),
        author: base.author.clone(),
        rights: base.rights.clone(),
    };

    // Opening chapters + a localized "end of the sample" closing note.
    let note_path = odir.join(format!(".sample-note-{lang}.md"));
    std::fs::write(&note_path, sample_note(lang, &base.title))
        .with_context(|| format!("writing {}", note_path.display()))?;
    let mut sample_chaps: Vec<PathBuf> = chaps.iter().take(n).cloned().collect();
    sample_chaps.push(note_path.clone());

    let cepub = some(dir.join(lang).join("copyright-epub.md"));
    let cpdf = some(dir.join(lang).join("copyright.md"));
    let cover = some(dir.join("cover").join(format!("front-{lang}.png")));
    let openright = book.pdf.chapter_opens.as_deref() == Some("recto");
    let plate_framed = book.pdf.plate_style.as_deref() == Some("framed");
    let plate_width = book.pdf.plate_width.unwrap_or(0.78);
    let captions = config::resolve_captions(None, book);
    // Digital geometry (bare trim, no bleed) — the sample is a reading preview.
    let geometry = resolve_geometry(repo, book, None, Out::RetailPdf);

    let stem = format!("{}-{lang}-sample", book.slug);
    let mut out = Vec::new();

    let epub = odir.join(format!("{stem}.epub"));
    crate::epub_native::run(
        repo,
        &m,
        cepub.as_deref(),
        &sample_chaps,
        cover.as_deref(),
        lang,
        true,
        captions,
        &epub,
    )
    .with_context(|| "building sample epub")?;
    // Shrink the sample EPUB's images just like the retail EPUB, so a picture-book
    // preview isn't tens of MB. Best-effort (native, no external dep).
    let _ = crate::epub_shrink::shrink_epub(&epub, 1200);
    out.push(epub);

    let pdf = odir.join(format!("{stem}.pdf"));
    crate::typst_pdf::run(
        repo,
        &m,
        cpdf.as_deref(),
        &sample_chaps,
        openright,
        plate_framed,
        plate_width,
        captions,
        true,
        false,
        false,
        cover.as_deref(),
        geometry,
        lang,
        &pdf,
    )
    .with_context(|| "building sample pdf")?;
    // A shared sample should stay light: honor the book's digital-PDF DPI (same
    // policy as the retail PDF). Best-effort — a missing Ghostscript just leaves
    // the full-res sample.
    if let Some(dpi) = config::resolve_digital_pdf_dpi(None, book) {
        let _ = crate::pdf_shrink::shrink_pdf(&pdf, dpi);
    }
    out.push(pdf);

    let _ = std::fs::remove_file(&note_path);
    Ok(out)
}

fn sample_note(lang: &str, title: &str) -> String {
    match lang {
        "es" => format!(
            "# Fin de la muestra\n\nGracias por leer esta muestra de *{title}*. \
             El libro completo está disponible en tu tienda.\n"
        ),
        _ => format!(
            "# End of the sample\n\nThank you for reading this sample of *{title}*. \
             The complete book is available at your store.\n"
        ),
    }
}

// ---------- 3. README ----------

fn write_readme(repo: &Repo, book: &BookConfig, dir: &Path) -> Result<PathBuf> {
    let odir = out_dir(repo, &book.slug);
    std::fs::create_dir_all(&odir)?;
    let out = odir.join("README.md");

    // Header metadata (author/series/date resolve like the front matter).
    let author = book
        .meta
        .author
        .clone()
        .or_else(|| repo.config.author.clone())
        .unwrap_or_else(|| "Federico Ramallo".into());
    let series = book
        .meta
        .series
        .clone()
        .or_else(|| repo.config.series.clone());
    let year = book
        .meta
        .date
        .clone()
        .or_else(|| repo.config.date.clone())
        .map(|d| d.to_string().trim_matches('"').to_string());

    // Primary display title (first available language).
    let primary = book
        .languages
        .first()
        .and_then(|l| book.title.get(l).cloned())
        .or_else(|| book.title.values().next().cloned())
        .unwrap_or_else(|| book.slug.clone());

    let mut s = String::new();
    s.push_str(&format!("# {primary}\n\n"));
    s.push_str("> Auto-generated by `bookmill` on build. Describes this book and everything under `output/{slug}/`.\n\n".replace("{slug}", &book.slug).as_str());
    s.push_str(&format!("- **Slug:** `{}`\n", book.slug));
    s.push_str(&format!("- **Author:** {author}\n"));
    if let Some(series) = &series {
        s.push_str(&format!("- **Series:** {series}\n"));
    }
    if let Some(year) = &year {
        s.push_str(&format!("- **Date:** {year}\n"));
    }
    if !book.languages.is_empty() {
        s.push_str(&format!("- **Languages:** {}\n", book.languages.join(", ")));
    }
    if !book.editions.is_empty() {
        s.push_str(&format!("- **Editions:** {}\n", book.editions.join(", ")));
    }
    s.push_str(&format!("- **Status:** {}\n", status_line(book)));
    s.push('\n');

    // Per-language details.
    for lang in &book.languages {
        let Some(title) = book.title.get(lang) else {
            continue;
        };
        s.push_str(&format!("## {}\n\n", lang_name(lang)));
        s.push_str(&format!("- **Title:** {title}\n"));
        if let Some(sub) = book.subtitle.get(lang) {
            s.push_str(&format!("- **Subtitle:** {sub}\n"));
        }
        // word + page counts from the resolved content / built print PDF.
        if let Some(content) = book.content.get(lang) {
            if let Ok(chaps) = content.resolve(dir) {
                let words = crate::pages::count_words(&chaps);
                s.push_str(&format!(
                    "- **Words:** {} ({} chapter files)\n",
                    thousands(words),
                    chaps.len()
                ));
                let kdp_pdf = out_dir(repo, &book.slug)
                    .join(lang)
                    .join(format!("{}-{lang}-kdp.pdf", book.slug));
                s.push_str(&format!(
                    "- **Print pages:** {}\n",
                    page_str(&kdp_pdf, words)
                ));
                let n = config::sample_count(book, chaps.len());
                if config::sample_enabled(book) {
                    s.push_str(&format!("- **Free sample:** first {n} chapter file(s)\n"));
                }
            }
        }
        if let Some(listing) = book.listing.get(lang) {
            if let Some(age) = &listing.reading_age {
                s.push_str(&format!("- **Reading age:** {age}\n"));
            }
            if !listing.keywords.is_empty() {
                s.push_str(&format!("- **Keywords:** {}\n", listing.keywords.join(", ")));
            }
            if !listing.bisac.is_empty() {
                s.push_str(&format!("- **BISAC:** {}\n", listing.bisac.join(", ")));
            }
            if let Some(blurb) = &listing.blurb {
                s.push_str("- **Blurb:**\n\n");
                for line in blurb.trim().lines() {
                    s.push_str(&format!("  > {}\n", line.trim()));
                }
            }
        }
        // Files built for this language.
        let files = list_files(&out_dir(repo, &book.slug).join(lang));
        if !files.is_empty() {
            s.push_str("- **Files:**\n");
            for f in files {
                s.push_str(&format!("  - `{f}`\n"));
            }
        }
        s.push('\n');
    }

    // ASINs, if the book records any.
    if !book.status.asin.is_empty() {
        s.push_str("## ASINs\n\n");
        for (k, v) in &book.status.asin {
            s.push_str(&format!("- **{k}:** {v}\n"));
        }
        s.push('\n');
    }

    std::fs::write(&out, s).with_context(|| format!("writing {}", out.display()))?;
    Ok(out)
}

fn status_line(book: &BookConfig) -> String {
    let st = &book.status;
    let mut parts: Vec<String> = Vec::new();
    let mut push = |k: &str, v: &Option<String>| {
        if let Some(v) = v {
            parts.push(format!("{k}={v}"));
        }
    };
    push("overall", &st.overall);
    push("kdp_paperback", &st.kdp_paperback);
    push("kdp_kindle", &st.kdp_kindle);
    push("kdp_hardcover", &st.kdp_hardcover);
    if parts.is_empty() {
        st.ribbon().to_string()
    } else {
        format!("{} ({})", st.ribbon(), parts.join(", "))
    }
}

// ---------- helpers ----------

/// `output/<slug>/`
fn out_dir(repo: &Repo, slug: &str) -> PathBuf {
    repo.root.join("output").join(slug)
}

fn some(p: PathBuf) -> Option<PathBuf> {
    p.exists().then_some(p)
}

fn rel(repo: &Repo, p: &Path) -> String {
    p.strip_prefix(&repo.root)
        .unwrap_or(p)
        .to_string_lossy()
        .into_owned()
}

/// The publishable outputs in a language dir (skip the `.md`/sidecar/hidden bits
/// are kept — the manifest is meant to be complete). Sorted for stable README.
fn list_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name.ends_with(".pages.json") {
                continue;
            }
            names.push(name);
        }
    }
    names.sort();
    names
}

fn page_str(kdp_pdf: &Path, words: usize) -> String {
    match crate::pages::resolve(kdp_pdf, words) {
        crate::pages::PageCount::Exact(p) => p.to_string(),
        crate::pages::PageCount::Estimate(p) => format!("~{p} (estimate)"),
        crate::pages::PageCount::Unknown => "— (print interior not built)".into(),
    }
}

fn lang_name(lang: &str) -> String {
    match lang {
        "es" => "Español (es)".into(),
        "en" => "English (en)".into(),
        other => other.to_string(),
    }
}

fn thousands(n: usize) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::new();
    for (i, c) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

fn yaml_str(s: &str) -> String {
    // Quote and escape for a YAML scalar (safe for titles with colons/quotes).
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
