//! `bookmill tauri` / `bookmill desktop` — launch the native desktop window.
//!
//! The desktop UI is a separate Tauri v2 crate (`desktop/`, sibling to `web/`),
//! so the main binary stays lean. This launcher locates the built/installed
//! `bookmill-desktop` executable and runs it, passing the discovered repo root
//! so the window opens on the same books `bookmill web` would show. If the app
//! isn't built yet, it explains how to build it (and can fall back to the plain
//! `bookmill web` server so the user still gets the UI in their browser).

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Launch the desktop window for the repo rooted at `repo_root`.
pub fn run(repo_root: &Path, port: u16, pages: u32) -> Result<()> {
    match find_desktop_bin(repo_root) {
        Some(bin) => {
            println!("bookmill desktop: launching {}", bin.display());
            let status = Command::new(&bin)
                .arg("--repo")
                .arg(repo_root)
                .status()
                .with_context(|| format!("launching desktop app {}", bin.display()))?;
            if !status.success() {
                anyhow::bail!("desktop app exited with {status}");
            }
            Ok(())
        }
        None => {
            eprintln!(
                "The native desktop app (`bookmill-desktop`) is not built or installed.\n\
                 \n\
                 Build it once with:\n\
                 \x20   cd desktop/src-tauri && cargo tauri build\n\
                 or run it in dev mode:\n\
                 \x20   cd desktop/src-tauri && cargo tauri dev -- -- --repo {repo}\n\
                 \n\
                 Set BOOKMILL_DESKTOP_BIN to point at the built binary to skip discovery.\n\
                 \n\
                 Falling back to `bookmill web` (open the printed URL in your browser)…\n",
                repo = repo_root.display()
            );
            crate::web::run(repo_root, port, pages)
        }
    }
}

/// Locate the `bookmill-desktop` executable. Order: env override, the installed
/// macOS `.app` bundle, a local dev build under `desktop/…/target`, then PATH.
fn find_desktop_bin(repo_root: &Path) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("BOOKMILL_DESKTOP_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }

    // Installed macOS app bundle (e.g. via `brew install --cask bookmill`).
    let app_bins = [
        "/Applications/bookmill.app/Contents/MacOS/bookmill-desktop",
        "/Applications/bookmill.app/Contents/MacOS/bookmill",
    ];
    for p in app_bins {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        for name in ["bookmill-desktop", "bookmill"] {
            let p = Path::new(&home)
                .join("Applications/bookmill.app/Contents/MacOS")
                .join(name);
            if p.exists() {
                return Some(p);
            }
        }
    }

    // Local dev build (running from a bookmill checkout).
    for base in [repo_root, Path::new(env!("CARGO_MANIFEST_DIR"))] {
        for profile in ["release", "debug"] {
            let p = base
                .join("desktop/src-tauri/target")
                .join(profile)
                .join("bookmill-desktop");
            if p.exists() {
                return Some(p);
            }
        }
    }

    which_on_path("bookmill-desktop")
}

fn which_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}
