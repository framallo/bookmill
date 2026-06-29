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
