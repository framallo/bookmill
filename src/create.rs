//! `bookmill create` — scaffold a project or a new book (convention over
//! configuration). Two modes, auto-detected:
//!
//!   * **add-book** — run inside an existing bookmill repo (a `bookmill.toml`
//!     without a `slug` exists up-tree): scaffold one new book into it.
//!   * **init** — no repo found: scaffold a brand-new project (repo `bookmill.toml`
//!     + first book). If a git repo is detected we confirm before initializing.
//!
//! Conventions come from `templates/archetypes.toml` (bundled, overridable by a
//! project-local copy) — pick an archetype for the scope (single/series) and
//! default languages; every field is still overridable by a flag or an
//! interactive answer. Works fully from flags (scriptable) or `--interactive`.

use crate::discover::{Repo, CONFIG_NAME};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The bundled archetype definitions (a project-local copy overrides this).
const BUNDLED_ARCHETYPES: &str = include_str!("../templates/archetypes.toml");

/// Shared build assets written into a new repo on `init` so it builds out of the
/// box — the EPUB template/CSS and Lua filter that `build.rs` references from the
/// repo root. PDFs render via the native Typst engine, which needs no scaffolded
/// assets. `(relative path, contents)`.
const SCAFFOLD_ASSETS: &[(&str, &str)] = &[
    ("templates/epub.html", include_str!("../templates/scaffold/templates/epub.html")),
    ("css/epub.css", include_str!("../templates/scaffold/css/epub.css")),
    ("scripts/drop-spot-epub.lua", include_str!("../templates/scaffold/scripts/drop-spot-epub.lua")),
];

/// Write the shared build assets into `root` (skip any that already exist, so a
/// customized file is never clobbered).
fn write_scaffold_assets(root: &Path) -> Result<()> {
    for (rel, contents) in SCAFFOLD_ASSETS {
        let p = root.join(rel);
        if p.exists() {
            continue;
        }
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, contents).with_context(|| format!("writing {}", p.display()))?;
    }
    Ok(())
}

// ---------- CLI args (mirrored from main.rs Cmd::Create) ----------
pub struct Args {
    pub slug: Option<String>,
    /// `lang=Title` entries, e.g. ["es=Mi libro", "en=My Book"]
    pub titles: Vec<String>,
    pub author: Option<String>,
    /// languages (repeatable or comma-joined)
    pub langs: Vec<String>,
    pub archetype: Option<String>,
    pub scope: Option<String>,
    pub series: Option<String>,
    pub books_dir: Option<String>,
    pub trim: Option<String>,
    pub dir: Option<PathBuf>,
    pub interactive: bool,
    /// with --interactive, prompt for ALL fields (not just essentials)
    pub all: bool,
    /// accept defaults / confirmations without prompting
    pub yes: bool,
}

// ---------- archetypes.toml model ----------
#[derive(Debug, Deserialize)]
struct Archetypes {
    default: String,
    #[serde(default)]
    archetype: BTreeMap<String, Archetype>,
    #[serde(default)]
    lang: BTreeMap<String, LangConv>,
}

#[derive(Debug, Deserialize, Clone)]
struct Archetype {
    #[allow(dead_code)]
    description: String,
    scope: String,
    languages: Vec<String>,
    editions: Vec<String>,
    trim: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct LangConv {
    chapter_stem: String,
    #[allow(dead_code)]
    voice: String,
    #[allow(dead_code)]
    code: String,
}

impl Archetypes {
    /// Load the project-local `templates/archetypes.toml` if present near
    /// `target` (or its repo root), else the bundled defaults.
    fn load(target: &Path) -> Result<Archetypes> {
        for base in [target, target.parent().unwrap_or(target)] {
            let p = base.join("templates").join("archetypes.toml");
            if p.exists() {
                let s = std::fs::read_to_string(&p)
                    .with_context(|| format!("reading {}", p.display()))?;
                return toml::from_str(&s).with_context(|| format!("parsing {}", p.display()));
            }
        }
        toml::from_str(BUNDLED_ARCHETYPES).context("parsing bundled archetypes.toml")
    }

    fn pick(&self, name: Option<&str>) -> Result<(String, Archetype)> {
        let key = name.map(str::to_string).unwrap_or_else(|| self.default.clone());
        let a = self
            .archetype
            .get(&key)
            .cloned()
            .with_context(|| format!("unknown archetype {key:?}; known: {:?}", self.archetype.keys().collect::<Vec<_>>()))?;
        Ok((key, a))
    }

    /// Chapter-file stem for a language (es -> "capitulo", default -> "chapter").
    fn chapter_stem(&self, lang: &str) -> String {
        self.lang
            .get(lang)
            .or_else(|| self.lang.get("default"))
            .map(|c| c.chapter_stem.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "chapter".into())
    }
}

// ---------- entry point ----------

pub fn run(start: &Path, args: Args) -> Result<()> {
    let target = args
        .dir
        .clone()
        .unwrap_or_else(|| start.to_path_buf());
    std::fs::create_dir_all(&target)
        .with_context(|| format!("creating target dir {}", target.display()))?;

    let archetypes = Archetypes::load(&target)?;
    let (arch_name, arch) = archetypes.pick(args.archetype.as_deref())?;

    // Detect an existing repo (add-book) vs init.
    match Repo::find(&target) {
        Ok(repo) => add_book(&repo, &archetypes, &arch, &args),
        Err(_) => init_project(&target, &archetypes, &arch_name, &arch, &args),
    }
}

// ---------- add-book mode ----------

fn add_book(repo: &Repo, archetypes: &Archetypes, arch: &Archetype, args: &Args) -> Result<()> {
    println!("Adding a book to existing repo at {}", repo.root.display());
    let languages = resolve_langs(args, repo_langs(repo).unwrap_or(arch.languages.clone()));
    let slug = resolve_slug(args)?;
    let titles = resolve_titles(args, &languages, &slug);
    let author = args.author.clone(); // None => inherit repo author
    let editions = arch.editions.clone();

    let books_dir = repo.books_dir();
    write_book(&books_dir, &slug, &languages, &titles, author.as_deref(), &editions, archetypes)?;
    summary_book(&repo.root, &repo.config.books_dir, &slug, &languages);
    Ok(())
}

fn repo_langs(repo: &Repo) -> Option<Vec<String>> {
    (!repo.config.languages.is_empty()).then(|| repo.config.languages.clone())
}

// ---------- init mode ----------

#[allow(clippy::too_many_arguments)]
fn init_project(
    target: &Path,
    archetypes: &Archetypes,
    arch_name: &str,
    arch: &Archetype,
    args: &Args,
) -> Result<()> {
    let in_git = has_git(target);
    if !args.yes {
        let git_note = if in_git { " (existing git repo detected)" } else { "" };
        let ok = confirm(
            args,
            &format!(
                "No bookmill repo found. Initialize a new bookmill project in {}{}?",
                target.display(),
                git_note
            ),
            true,
        );
        if !ok {
            bail!("aborted (no project created)");
        }
    }

    let scope = args.scope.clone().unwrap_or_else(|| arch.scope.clone());
    let languages = resolve_langs(args, arch.languages.clone());
    let author = resolve_author(args, "Author Name");
    let books_dir = resolve_books_dir(args, "books");
    let trim = args.trim.clone().unwrap_or_else(|| arch.trim.clone());
    let series = if scope == "series" {
        Some(resolve_series(args, &author))
    } else {
        None
    };
    let slug = resolve_slug(args)?;
    let titles = resolve_titles(args, &languages, &slug);

    println!(
        "Initializing {scope} project (archetype {arch_name}) in {}",
        target.display()
    );

    // Repo config.
    let repo_cfg = target.join(CONFIG_NAME);
    if repo_cfg.exists() {
        bail!("{} already exists — refusing to overwrite", repo_cfg.display());
    }
    std::fs::write(
        &repo_cfg,
        repo_config_toml(&author, series.as_deref(), &languages, &books_dir, &arch.editions, &trim, archetypes),
    )
    .with_context(|| format!("writing {}", repo_cfg.display()))?;

    // .gitignore (create or extend).
    ensure_gitignore(target)?;

    // Shared build assets (LaTeX headers, EPUB template/CSS, Lua filters) so the
    // new project builds without hunting them down.
    write_scaffold_assets(target)?;

    // First book.
    write_book(
        &target.join(&books_dir),
        &slug,
        &languages,
        &titles,
        None, // inherit repo author
        &arch.editions,
        archetypes,
    )?;

    summary_init(target, &books_dir, &slug, &languages, &repo_cfg);
    Ok(())
}

fn has_git(dir: &Path) -> bool {
    let mut d = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    loop {
        if d.join(".git").exists() {
            return true;
        }
        if !d.pop() {
            return false;
        }
    }
}

// ---------- field resolution (flag -> prompt -> default) ----------

fn resolve_slug(args: &Args) -> Result<String> {
    let slug = match &args.slug {
        Some(s) => s.clone(),
        None if args.interactive => prompt("Book slug (kebab-case, e.g. my-book)", ""),
        None => bail!("--slug is required (or use --interactive)"),
    };
    validate_slug(&slug)?;
    Ok(slug)
}

fn resolve_langs(args: &Args, default: Vec<String>) -> Vec<String> {
    if !args.langs.is_empty() {
        return split_commas(&args.langs);
    }
    if args.interactive && args.all {
        let ans = prompt("Languages (comma-separated)", &default.join(","));
        let v = split_commas(&[ans]);
        if !v.is_empty() {
            return v;
        }
    }
    default
}

fn resolve_titles(args: &Args, languages: &[String], slug: &str) -> BTreeMap<String, String> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for entry in &args.titles {
        if let Some((lang, title)) = entry.split_once('=') {
            map.insert(lang.trim().to_string(), title.trim().to_string());
        }
    }
    let humanized = humanize(slug);
    for lang in languages {
        if !map.contains_key(lang) {
            let title = if args.interactive {
                prompt(&format!("Title [{lang}]"), &humanized)
            } else {
                humanized.clone()
            };
            map.insert(lang.clone(), title);
        }
    }
    map
}

fn resolve_author(args: &Args, default: &str) -> String {
    match &args.author {
        Some(a) => a.clone(),
        None if args.interactive => prompt("Author", default),
        None => default.to_string(),
    }
}

fn resolve_series(args: &Args, default_author: &str) -> String {
    match &args.series {
        Some(s) => s.clone(),
        None if args.interactive => prompt("Series name", &format!("{default_author}'s Series")),
        None => format!("{default_author}'s Series"),
    }
}

fn resolve_books_dir(args: &Args, default: &str) -> String {
    match &args.books_dir {
        Some(b) => b.clone(),
        None if args.interactive && args.all => prompt("Books directory", default),
        None => default.to_string(),
    }
}

// ---------- file generation ----------

fn write_book(
    books_dir: &Path,
    slug: &str,
    languages: &[String],
    titles: &BTreeMap<String, String>,
    author: Option<&str>,
    editions: &[String],
    archetypes: &Archetypes,
) -> Result<()> {
    let book_dir = books_dir.join(slug);
    if book_dir.join(CONFIG_NAME).exists() {
        bail!("book already exists: {}", book_dir.join(CONFIG_NAME).display());
    }
    std::fs::create_dir_all(&book_dir)
        .with_context(|| format!("creating {}", book_dir.display()))?;

    // book bookmill.toml
    std::fs::write(
        book_dir.join(CONFIG_NAME),
        book_config_toml(slug, languages, titles, author, editions, archetypes),
    )?;

    // per-language dirs + a starter chapter
    for lang in languages {
        let ldir = book_dir.join(lang);
        std::fs::create_dir_all(&ldir)?;
        let stem = archetypes.chapter_stem(lang);
        let chap = ldir.join(format!("{stem}-01.md"));
        std::fs::write(&chap, starter_chapter(lang))?;
    }
    Ok(())
}

fn book_config_toml(
    slug: &str,
    languages: &[String],
    titles: &BTreeMap<String, String>,
    author: Option<&str>,
    editions: &[String],
    archetypes: &Archetypes,
) -> String {
    let mut s = String::new();
    s.push_str(&format!("slug      = {}\n", q(slug)));
    s.push_str(&format!("languages = [{}]\n", list(languages)));
    s.push_str(&format!("editions  = [{}]\n\n", list(editions)));

    s.push_str("[title]\n");
    for lang in languages {
        if let Some(t) = titles.get(lang) {
            s.push_str(&format!("{lang} = {}\n", q(t)));
        }
    }
    s.push('\n');

    if let Some(a) = author {
        s.push_str("[meta]\n");
        s.push_str(&format!("author = {}\n\n", q(a)));
    }

    for lang in languages {
        let stem = archetypes.chapter_stem(lang);
        s.push_str(&format!("[content.{lang}]\n"));
        s.push_str(&format!("glob = {}\n\n", q(&format!("{lang}/{stem}-*.md"))));
    }

    s.push_str("[pdf]\n");
    s.push_str("chapter_opens = \"recto\"   # chapters open on a right-hand page\n");
    s
}

fn starter_chapter(lang: &str) -> String {
    match lang {
        "es" => "# Primer capítulo\n\nEscribe aquí tu primer capítulo.\n".into(),
        _ => "# First chapter\n\nWrite your first chapter here.\n".into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn repo_config_toml(
    author: &str,
    series: Option<&str>,
    languages: &[String],
    books_dir: &str,
    editions: &[String],
    trim: &str,
    archetypes: &Archetypes,
) -> String {
    let mut s = String::new();
    s.push_str("# bookmill repo config (generated by `bookmill create`)\n");
    s.push_str(&format!("author    = {}\n", q(author)));
    if let Some(ser) = series {
        s.push_str(&format!("series    = {}\n", q(ser)));
    }
    s.push_str("date      = 2026\n");
    s.push_str(&format!("languages = [{}]\n", list(languages)));
    s.push_str(&format!("books_dir = {}\n\n", q(books_dir)));

    s.push_str("[defaults]\n");
    s.push_str(&format!("trim   = {}\n", q(trim)));
    s.push_str("bleed  = \"0\"          # text interiors: no full-bleed; picture books override\n");
    s.push_str("paper  = \"white\"      # white | cream | groundwood\n");
    s.push_str("ink    = \"black\"      # black | standard-color | premium-color\n");
    s.push_str("finish = \"matte\"      # matte | glossy (cover/listing only)\n\n");

    s.push_str("[defaults.margins]\n");
    s.push_str("top           = 0.75\nbottom        = 0.75\ninner         = 0.75\nouter         = 0.6\nbindingoffset = 0.375\n\n");

    for ed in editions {
        s.push_str(&edition_block(ed));
    }

    s.push_str("[build]\n");
    s.push_str("output_dir = \"output\"\n");
    s.push_str("formats    = [\"epub\", \"pdf\", \"kdp\"]\n\n");

    // Audiobook defaults from the language conventions.
    s.push_str("# Audiobook synthesis defaults (kab = Kokoro on the Apple Neural Engine).\n");
    s.push_str("[audiobook]\n");
    s.push_str("engine       = \"kab\"\n");
    s.push_str("speak_titles = true\n\n");
    for lang in languages {
        if let Some(c) = archetypes.lang.get(lang).filter(|c| !c.voice.is_empty()) {
            s.push_str(&format!("[audiobook.{lang}]\n"));
            s.push_str(&format!("voice = {}\n", q(&c.voice)));
            s.push_str(&format!("code  = {}\n\n", q(&c.code)));
        }
    }
    s
}

/// One `[editions.<name>]` block. Known KDP/Gumroad/Bubok channels get a sensible
/// preset; an unknown name gets a minimal block with a best-effort target.
fn edition_block(name: &str) -> String {
    match name {
        "kdp-paperback" => "[editions.kdp-paperback]\ntarget = \"kdp-paperback\"\nmarket = \"US\"\npaper  = \"white\"\nink    = \"black\"\nfinish = \"matte\"\nisbn   = \"free\"   # KDP-assigned (non-portable)\n\n".into(),
        "kdp-epub" => "[editions.kdp-epub]\ntarget        = \"kdp-epub\"\nmarket        = \"US\"\nepub_image_px = 1000   # lower-res images = smaller Kindle delivery\n\n".into(),
        "kdp-hardcover" => "[editions.kdp-hardcover]\ntarget = \"kdp-hardcover\"\nmarket = \"US\"\npaper  = \"white\"\nink    = \"black\"\nfinish = \"matte\"   # needs >=75 pages\n\n".into(),
        "gumroad" => "[editions.gumroad]\ntarget = \"gumroad\"\nmarket = \"WW\"   # worldwide digital\n\n".into(),
        other => {
            let target = if other.starts_with("bubok") { "bubok" } else { other };
            format!("[editions.{other}]\ntarget = {}\nmarket = \"US\"\n\n", q(target))
        }
    }
}

fn ensure_gitignore(target: &Path) -> Result<()> {
    let p = target.join(".gitignore");
    let existing = std::fs::read_to_string(&p).unwrap_or_default();
    let mut add = Vec::new();
    for want in ["/output", "/target"] {
        if !existing.lines().any(|l| l.trim() == want) {
            add.push(want);
        }
    }
    if add.is_empty() {
        return Ok(());
    }
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for a in add {
        out.push_str(a);
        out.push('\n');
    }
    std::fs::write(&p, out).with_context(|| format!("writing {}", p.display()))?;
    Ok(())
}

// ---------- summaries ----------

fn summary_book(root: &Path, books_dir: &str, slug: &str, languages: &[String]) {
    println!("\nCreated book {slug} ({})", languages.join(", "));
    println!("  {}/{books_dir}/{slug}/{CONFIG_NAME}", root.display());
    next_steps(slug);
}

fn summary_init(target: &Path, books_dir: &str, slug: &str, languages: &[String], repo_cfg: &Path) {
    println!("\nInitialized project at {}", target.display());
    println!("  {}", repo_cfg.display());
    println!("  {}/{books_dir}/{slug}/  ({})", target.display(), languages.join(", "));
    next_steps(slug);
}

fn next_steps(slug: &str) {
    println!("\nNext:");
    println!("  bookmill list");
    println!("  bookmill validate {slug}");
    println!("  bookmill build {slug} --format pdf");
}

// ---------- small helpers ----------

fn validate_slug(slug: &str) -> Result<()> {
    if slug.is_empty() {
        bail!("slug cannot be empty");
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        bail!("slug {slug:?} must be kebab-case (lowercase letters, digits, hyphens)");
    }
    Ok(())
}

/// "no-hay-plata" -> "No Hay Plata".
fn humanize(slug: &str) -> String {
    slug.split('-')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(c) => c.to_uppercase().collect::<String>() + cs.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Split a Vec of possibly comma-joined entries into a flat, trimmed list.
fn split_commas(items: &[String]) -> Vec<String> {
    items
        .iter()
        .flat_map(|s| s.split(','))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// TOML double-quoted string.
fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Render a string slice list as quoted TOML array items.
fn list(items: &[String]) -> String {
    items.iter().map(|s| q(s)).collect::<Vec<_>>().join(", ")
}

/// Read a line from stdin, returning `default` on empty input.
fn prompt(question: &str, default: &str) -> String {
    if default.is_empty() {
        print!("? {question}: ");
    } else {
        print!("? {question} [{default}]: ");
    }
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return default.to_string();
    }
    let t = line.trim();
    if t.is_empty() {
        default.to_string()
    } else {
        t.to_string()
    }
}

/// Yes/no confirm. With `--yes` returns `default` without prompting.
fn confirm(args: &Args, question: &str, default: bool) -> bool {
    if args.yes {
        return default;
    }
    let d = if default { "Y/n" } else { "y/N" };
    print!("? {question} [{d}]: ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return default;
    }
    match line.trim().to_lowercase().as_str() {
        "" => default,
        "y" | "yes" => true,
        _ => false,
    }
}
