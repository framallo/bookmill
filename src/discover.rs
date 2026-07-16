//! Repo + book discovery (convention over configuration).
//! Config files are all named `bookmill.toml`. A *repo* config sits at the repo
//! root; a *book* config sits in each book dir and is distinguished by having a
//! top-level `slug` key.

use crate::config::{load_book, load_repo, BookConfig, RepoConfig};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

pub const CONFIG_NAME: &str = "bookmill.toml";

pub struct Repo {
    pub root: PathBuf,
    pub config: RepoConfig,
}

/// True if a `bookmill.toml` is a book config (has a top-level `slug`).
fn is_book_config(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.parse::<toml::Table>().ok())
        .map(|t| t.contains_key("slug"))
        .unwrap_or(false)
}

impl Repo {
    /// Find the repo by walking up from `start` for a *repo* `bookmill.toml`
    /// (skips book-level configs that share the filename).
    pub fn find(start: &Path) -> Result<Repo> {
        let mut dir = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
        loop {
            let cfg = dir.join(CONFIG_NAME);
            if cfg.exists() && !is_book_config(&cfg) {
                let config = load_repo(&cfg)?;
                return Ok(Repo { root: dir, config });
            }
            if !dir.pop() {
                bail!("no repo {CONFIG_NAME} found (run inside a books repo)");
            }
        }
    }

    pub fn books_dir(&self) -> PathBuf {
        self.root.join(&self.config.books_dir)
    }

    // ---- configurable path layout ([paths]) — see config::Paths ----

    /// Build output root (repo-root-relative; `[paths].output_dir`, default "output").
    pub fn output_dir(&self) -> PathBuf {
        self.root.join(&self.config.paths.output_dir)
    }

    /// Per-book cover asset dir (`[paths].cover_dir`, default "cover").
    pub fn cover_dir(&self, book_dir: &Path) -> PathBuf {
        book_dir.join(&self.config.paths.cover_dir)
    }

    /// Project-local template override dir (`[paths].templates_dir`, default "templates").
    pub fn templates_dir(&self) -> PathBuf {
        self.root.join(&self.config.paths.templates_dir)
    }

    /// EPUB stylesheet (`[paths].epub_css`, default "css/epub.css").
    pub fn epub_css(&self) -> PathBuf {
        self.root.join(&self.config.paths.epub_css)
    }

    /// Retail-PDF page-background texture (`[paths].paper_texture`).
    pub fn paper_texture(&self) -> PathBuf {
        self.root.join(&self.config.paths.paper_texture)
    }

    /// Audiobook scratch/cache dir (`[paths].tmp_dir`, default ".bookmill-tmp").
    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join(&self.config.paths.tmp_dir)
    }

    /// eBook front-cover filename for `lang` (`[paths].front_cover`).
    pub fn front_cover_name(&self, lang: &str) -> String {
        self.config.paths.front_cover.replace("{lang}", lang)
    }

    /// Paperback wrap-cover filename for `lang` (`[paths].wrap_cover`).
    pub fn wrap_cover_name(&self, lang: &str) -> String {
        self.config.paths.wrap_cover.replace("{lang}", lang)
    }

    /// Full path to a book's eBook front cover for `lang`.
    pub fn front_cover(&self, book_dir: &Path, lang: &str) -> PathBuf {
        self.cover_dir(book_dir).join(self.front_cover_name(lang))
    }

    /// Full path to a book's paperback wrap cover for `lang`.
    pub fn wrap_cover(&self, book_dir: &Path, lang: &str) -> PathBuf {
        self.cover_dir(book_dir).join(self.wrap_cover_name(lang))
    }

    /// Full path to a book's PDF copyright page for `lang` (`[paths].copyright_pdf`).
    pub fn copyright_pdf(&self, book_dir: &Path, lang: &str) -> PathBuf {
        book_dir.join(self.config.paths.copyright_pdf.replace("{lang}", lang))
    }

    /// Full path to a book's EPUB copyright page for `lang` (`[paths].copyright_epub`).
    pub fn copyright_epub(&self, book_dir: &Path, lang: &str) -> PathBuf {
        book_dir.join(self.config.paths.copyright_epub.replace("{lang}", lang))
    }

    /// List discovered book directories (those with a book-level `bookmill.toml`).
    pub fn book_dirs(&self) -> Result<Vec<PathBuf>> {
        let bd = self.books_dir();
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&bd)
            .with_context(|| format!("reading books dir {}", bd.display()))?
        {
            let p = entry?.path();
            let cfg = p.join(CONFIG_NAME);
            if cfg.exists() && is_book_config(&cfg) {
                out.push(p);
            }
        }
        out.sort();
        Ok(out)
    }

    pub fn load_book_at(&self, dir: &Path) -> Result<(BookConfig, PathBuf)> {
        Ok((load_book(&dir.join(CONFIG_NAME))?, dir.to_path_buf()))
    }

    /// Resolve a book by slug (or directory name).
    pub fn find_book(&self, slug: &str) -> Result<(BookConfig, PathBuf)> {
        for dir in self.book_dirs()? {
            let (b, d) = self.load_book_at(&dir)?;
            if b.slug == slug || dir.file_name().map(|n| n == slug).unwrap_or(false) {
                return Ok((b, d));
            }
        }
        bail!("book not found: {slug}")
    }
}
