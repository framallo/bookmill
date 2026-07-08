//! bookmill — convention-over-configuration book publishing pipeline.
//! v1: config model + discovery + validation + Ratatui TUI. Build/cover/audiobook
//! engines are scaffolded (orchestrator-first; native engines land next).

mod audiobook;
mod build;
mod config;
mod create;
mod cover_svg;
mod docx_native;
mod epub_native;
mod epub_shrink;
mod pages;
mod pdfmeta;
mod cover_tmpl;
mod covers;
mod deep;
mod discover;
mod lint;
mod scripts;
mod tui;
mod words;
mod typst_pdf;
mod web;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use discover::Repo;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "bookmill", version, about = "Book publishing pipeline (EPUB/PDF/KDP/covers/audiobook/words/lint)")]
struct Cli {
    /// Path to start repo discovery from (defaults to current dir)
    #[arg(long, global = true)]
    repo: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scaffold a new project (init) or add a book to an existing repo
    Create {
        /// book slug (kebab-case identifier + directory name)
        #[arg(long)]
        slug: Option<String>,
        /// title as `lang=Title` (repeatable), e.g. --title es="Mi libro" --title en="My Book"
        #[arg(long = "title")]
        titles: Vec<String>,
        /// author name (defaults to repo author / "Author Name")
        #[arg(long)]
        author: Option<String>,
        /// languages (repeatable or comma-joined), e.g. --lang es --lang en
        #[arg(long = "lang")]
        langs: Vec<String>,
        /// archetype preset (see templates/archetypes.toml)
        #[arg(long)]
        archetype: Option<String>,
        /// project scope when initializing: single | series
        #[arg(long)]
        scope: Option<String>,
        /// series name (series scope)
        #[arg(long)]
        series: Option<String>,
        /// books directory when initializing a new repo (default "books")
        #[arg(long)]
        books_dir: Option<String>,
        /// trim size, e.g. 6x9
        #[arg(long)]
        trim: Option<String>,
        /// target directory (defaults to current dir / --repo)
        #[arg(long)]
        dir: Option<PathBuf>,
        /// prompt for fields interactively
        #[arg(long, short)]
        interactive: bool,
        /// with --interactive, prompt for ALL fields (not just essentials)
        #[arg(long)]
        all: bool,
        /// accept defaults / confirmations without prompting
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// List discovered books
    List {
        #[arg(long)]
        json: bool,
    },
    /// Validate config + KDP/house rules
    Validate {
        /// book slug (omit = all)
        book: Option<String>,
        /// deep mode: also build/reuse outputs and run epubcheck + PDF-geometry +
        /// cover-resolution audits (slow). Default is the fast config-only check.
        #[arg(long)]
        deep: bool,
        /// with --deep, emit the issues as JSON (the data source for the web
        /// previewer's warnings panel) instead of human-readable lines.
        #[arg(long)]
        json: bool,
    },
    /// Print the resolved content file list for a book/lang
    Content { book: String, lang: String },
    /// Word + page counts per book/lang (native; matches the Makefile `words`)
    Words {
        /// book slug (omit or "all" = every book)
        book: Option<String>,
        /// limit to a language (es|en|all)
        #[arg(long)]
        lang: Option<String>,
    },
    /// Prose lint — native (missing tildes, spelling, canon/AI-isms); no Python.
    Lint {
        /// book slug (omit or "all" = every book)
        book: Option<String>,
        /// limit to a language (es|en|all)
        #[arg(long)]
        lang: Option<String>,
        /// deep mode: also run full grammar via scripts/lint-prose.py (LanguageTool)
        #[arg(long, visible_alias = "languagetool")]
        deep: bool,
    },
    /// Scaffold/check KDP listing metadata via scripts/kdp-metadata.py
    Kdp {
        /// book slug (omit or "all" = every book)
        book: Option<String>,
    },
    /// Interactive terminal UI
    Tui,
    /// Build outputs: interiors (default), covers, or shrink an EPUB
    #[command(args_conflicts_with_subcommands = true)]
    Build {
        /// cover / shrink subcommand; omit to build interiors (epub/pdf)
        #[command(subcommand)]
        what: Option<BuildSub>,
        /// interior build options (used when no subcommand is given)
        #[command(flatten)]
        interior: InteriorArgs,
    },
    /// Build audiobook (kab default engine), or `clean` its temp/cache files
    #[command(args_conflicts_with_subcommands = true)]
    Audiobook {
        /// `clean` subcommand; omit to render audiobooks
        #[command(subcommand)]
        what: Option<AudiobookSub>,
        /// render options (used when no subcommand is given)
        #[command(flatten)]
        render: AudiobookArgs,
    },
    /// Launch the cover-editor web UI (localhost)
    Web {
        /// listen port (localhost only)
        #[arg(long, default_value_t = 7777)]
        port: u16,
        /// spine page count used only when a book's print interior PDF is missing
        #[arg(long, default_value_t = 120)]
        pages: u32,
    },
}

/// `audiobook` render options (the default action when no subcommand is given).
#[derive(Args, Default)]
struct AudiobookArgs {
    book: Option<String>,
    #[arg(long)]
    lang: Option<String>,
    /// override the Kokoro voice (e.g. ef_dora, af_heart) for all jobs
    #[arg(long)]
    voice: Option<String>,
    /// override speech speed for all jobs (e.g. 1.0, 1.1)
    #[arg(long)]
    speed: Option<f64>,
    /// TTS engine id (default "kab"; reserved for future engines)
    #[arg(long)]
    engine: Option<String>,
    /// re-render even if the manifest shows nothing changed
    #[arg(long)]
    force: bool,
}

/// `audiobook` subcommands.
#[derive(Subcommand)]
enum AudiobookSub {
    /// Remove audiobook temp/cache files (.audiostage + .audiocache) for all books or one
    Clean {
        book: Option<String>,
        #[arg(long)]
        lang: Option<String>,
        /// also delete the render manifest (forces a full re-render next time)
        #[arg(long)]
        manifest: bool,
    },
}

/// `build` subcommands. Interior (epub/pdf) is the default when none is given.
#[derive(Subcommand)]
enum BuildSub {
    /// Build interiors (epub/pdf/kdp) — the default `build` action
    Interior(InteriorArgs),
    /// Render covers (front PNG + paperback wrap PDF + eBook JPG)
    Cover(CoverArgs),
    /// Shrink images inside an EPUB in place (native; Python-free)
    Shrink(ShrinkArgs),
}

/// Interior build options (also the default action for `build`).
#[derive(Args, Default)]
struct InteriorArgs {
    book: Option<String>,
    /// build a specific format (epub|pdf|kdp|print|all); else build by editions
    #[arg(long)]
    format: Option<String>,
    /// build a specific edition (e.g. kdp-paperback, gumroad); else all editions
    #[arg(long)]
    edition: Option<String>,
    /// limit to a language (es|en|all)
    #[arg(long)]
    lang: Option<String>,
}

#[derive(Args)]
struct CoverArgs {
    book: Option<String>,
    /// limit to a language (es|en|all)
    #[arg(long)]
    lang: Option<String>,
    /// only emit the cover HTML (skip headless Chrome rasterization);
    /// useful to verify the template/config without touching image assets
    /// (chrome engine only)
    #[arg(long)]
    html_only: bool,
    /// override the spine page count (else read from the built -kdp.pdf);
    /// lets covers render before the print interior exists
    #[arg(long)]
    pages: Option<u32>,
    /// rasterization engine: resvg (default, pure Rust, no Chrome) | chrome
    #[arg(long)]
    engine: Option<String>,
}

#[derive(Args)]
struct ShrinkArgs {
    /// path to the .epub
    epub: PathBuf,
    /// max image width in px (default 1200)
    #[arg(long, default_value_t = 1200)]
    px: u32,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let start = cli.repo.unwrap_or(std::env::current_dir()?);

    // `create` runs before repo discovery — it may scaffold a brand-new repo
    // where none exists yet (init mode) or add a book to one it finds.
    if let Cmd::Create {
        slug,
        titles,
        author,
        langs,
        archetype,
        scope,
        series,
        books_dir,
        trim,
        dir,
        interactive,
        all,
        yes,
    } = cli.cmd
    {
        return create::run(
            &start,
            create::Args {
                slug,
                titles,
                author,
                langs,
                archetype,
                scope,
                series,
                books_dir,
                trim,
                dir,
                interactive,
                all,
                yes,
            },
        );
    }

    let repo = Repo::find(&start)?;

    match cli.cmd {
        Cmd::Create { .. } => unreachable!("handled above"),
        Cmd::List { json } => cmd_list(&repo, json)?,
        Cmd::Validate { book, deep, json } => {
            if deep {
                deep::run(&repo, book, json)?
            } else {
                cmd_validate(&repo, book)?
            }
        }
        Cmd::Content { book, lang } => cmd_content(&repo, &book, &lang)?,
        Cmd::Words { book, lang } => words::run(&repo, book, lang)?,
        Cmd::Lint { book, lang, deep } => {
            if deep {
                scripts::lint(&repo, book, lang)?
            } else {
                lint::run(&repo, book, lang)?
            }
        }
        Cmd::Kdp { book } => scripts::kdp(&repo, book)?,
        Cmd::Tui => match tui::run(&repo)? {
            Some(action) => {
                // Show the exact command about to run, before running it.
                let cmd = action.command();
                println!("\n\u{25b6} running: {cmd}\n");
                match action {
                    tui::Action::Build(req) => {
                        let jobs = tui::jobs_for_req(&repo, &req)?;
                        tui::run_queue_ui(&repo, &jobs, &cmd)?;
                    }
                    tui::Action::Release(req) => {
                        // complete build: all editions -> covers -> deep validate
                        let lang = (req.lang != "all").then_some(req.lang.clone());
                        let all = tui::BuildReq {
                            book: req.book.clone(),
                            lang: req.lang.clone(),
                            edition: "all".into(),
                            format: "edition".into(),
                        };
                        let jobs = tui::jobs_for_req(&repo, &all)?;
                        tui::run_queue_ui(&repo, &jobs, &cmd)?;
                        covers::run(&repo, Some(req.book.clone()), lang, false, None, covers::Engine::Resvg)?;
                        deep::run(&repo, Some(req.book), false)?;
                    }
                    tui::Action::Validate { book } => deep::run(&repo, Some(book), false)?,
                    tui::Action::Covers { book, lang } => {
                        let lang = (lang != "all").then_some(lang);
                        covers::run(&repo, Some(book), lang, false, None, covers::Engine::Resvg)?;
                    }
                    tui::Action::Audiobook { book, lang } => {
                        let lang = (lang != "all").then_some(lang);
                        audiobook::run(&repo, Some(book), lang, None, None, false)?;
                    }
                    tui::Action::Words { book, lang } => {
                        let lang = (lang != "all").then_some(lang);
                        words::run(&repo, Some(book), lang)?;
                    }
                    tui::Action::Lint { book, lang } => {
                        let lang = (lang != "all").then_some(lang);
                        // native lint by default (Python-free); `--deep` on the CLI
                        // still routes to scripts/lint-prose.py for full grammar.
                        // Issues make `run` return Err (non-zero exit on the CLI);
                        // in the TUI we only want the report, so don't abort.
                        let _ = lint::run(&repo, Some(book), lang);
                    }
                    tui::Action::Kdp { book } => scripts::kdp(&repo, Some(book))?,
                }
                // Bare, copy-pasteable command on its own line (no prefix, no
                // emoji) so a terminal selection re-runs this exact task verbatim.
                println!("\nre-run this task:\n{cmd}");
            }
            None => println!("(nothing selected)"),
        },
        Cmd::Build { what, interior } => {
            match what.unwrap_or(BuildSub::Interior(interior)) {
                BuildSub::Interior(a) => {
                    // All outputs are native Rust: PDFs via Typst, EPUB via
                    // epub-builder/comrak, DOCX via docx-rs. No pandoc.
                    if a.format.is_some() {
                        build::run(&repo, a.book, a.format, a.lang)?;
                    } else {
                        build::run_editions(&repo, a.book, a.lang, a.edition)?;
                    }
                }
                BuildSub::Cover(a) => {
                    let engine = covers::Engine::parse(a.engine.as_deref())?;
                    covers::run(&repo, a.book, a.lang, a.html_only, a.pages, engine)?
                }
                BuildSub::Shrink(a) => {
                    let (before, after) = epub_shrink::shrink_epub(&a.epub, a.px)?;
                    println!(
                        "{}: {:.1}MB -> {:.1}MB",
                        a.epub.display(),
                        before as f64 / 1e6,
                        after as f64 / 1e6
                    );
                }
            }
        }
        Cmd::Audiobook { what, render } => match what {
            Some(AudiobookSub::Clean { book, lang, manifest }) => {
                audiobook::clean(&repo, book, lang, manifest)?;
            }
            None => {
                if let Some(e) = &render.engine {
                    if e != "kab" {
                        anyhow::bail!("unsupported audiobook engine {e:?} (only \"kab\")");
                    }
                }
                audiobook::run(&repo, render.book, render.lang, render.voice, render.speed, render.force)?;
            }
        },
        Cmd::Web { port, pages } => web::run(&repo.root, port, pages)?,
    }
    Ok(())
}

fn cmd_list(repo: &Repo, json: bool) -> Result<()> {
    let mut books = Vec::new();
    for dir in repo.book_dirs()? {
        if let Ok((b, _)) = repo.load_book_at(&dir) {
            books.push(b);
        }
    }
    if json {
        let v: Vec<_> = books
            .iter()
            .map(|b| {
                serde_json::json!({
                    "slug": b.slug,
                    "languages": b.languages,
                    "editions": b.editions,
                    "title": b.title,
                    "status": b.status.ribbon(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!("{} book(s) in {}", books.len(), repo.books_dir().display());
        for b in &books {
            let title = b.title.values().next().cloned().unwrap_or_default();
            let tag = match b.status.ribbon() {
                "live" => "LIVE",
                "in-review" => "REVIEW",
                "blocked" => "BLOCKED",
                _ => "DRAFT",
            };
            println!(
                "  {:>7}  {:30} {:36} [{}] editions: {}",
                tag,
                b.slug,
                title,
                b.languages.join(","),
                if b.editions.is_empty() { "—".into() } else { b.editions.join(",") }
            );
        }
    }
    Ok(())
}

fn cmd_validate(repo: &Repo, book: Option<String>) -> Result<()> {
    let dirs = match book {
        Some(s) => vec![repo.find_book(&s)?.1],
        None => repo.book_dirs()?,
    };
    let mut total = 0usize;
    for dir in dirs {
        let (b, _) = repo.load_book_at(&dir)?;
        let mut issues = config::validate_book(&b);
        issues.extend(config::validate_book_editions(&b, &repo.config));
        if issues.is_empty() {
            println!("\u{2713} {} — ok", b.slug);
        } else {
            for i in &issues {
                println!("{} {}: {}", if i.level == "error" { "\u{2717}" } else { "!" }, b.slug, i.msg);
            }
            total += issues.iter().filter(|i| i.level == "error").count();
        }
    }
    if total > 0 {
        anyhow::bail!("{total} error(s)");
    }
    Ok(())
}

fn cmd_content(repo: &Repo, book: &str, lang: &str) -> Result<()> {
    let (b, dir) = repo.find_book(book)?;
    let c = b
        .content
        .get(lang)
        .ok_or_else(|| anyhow::anyhow!("no [content.{lang}] for {book}"))?;
    for f in c.resolve(&dir)? {
        println!("{}", f.display());
    }
    Ok(())
}
