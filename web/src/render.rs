//! Authoritative cover re-render: shell out to `bookmill covers <slug> --lang
//! <lang>`. We never rasterize in the web server — the CLI (resvg) is the single
//! source of truth for what ships.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// Locate the bookmill binary: $BOOKMILL_BIN, then — when this process *is*
/// bookmill (the folded `bookmill web` server) — the current exe, then sibling
/// target dirs, then PATH. The current-exe check lets the in-binary server call
/// itself regardless of cwd, while the standalone `bookmill-web` (whose exe stem
/// is not "bookmill") falls through to locate the real bookmill binary.
pub fn bookmill_bin() -> String {
    if let Ok(b) = std::env::var("BOOKMILL_BIN") {
        return b;
    }
    if let Ok(exe) = std::env::current_exe() {
        if exe.file_stem().and_then(|s| s.to_str()) == Some("bookmill") {
            return exe.display().to_string();
        }
    }
    // web/ lives inside the bookmill crate; the binary builds to ../target/*.
    for rel in [
        "../target/release/bookmill",
        "../target/debug/bookmill",
        "target/release/bookmill",
        "target/debug/bookmill",
    ] {
        if Path::new(rel).exists() {
            return rel.to_string();
        }
    }
    "bookmill".to_string()
}

/// Run `bookmill build cover` for one book+lang. `pages` is passed when the print
/// interior PDF is absent (the resvg path needs a spine page count up front);
/// the front PNG — what the editor shows — is unaffected by the value.
pub fn render_cover(repo_root: &Path, slug: &str, lang: &str, pages: Option<u32>) -> Result<String> {
    let bin = bookmill_bin();
    let mut cmd = Command::new(&bin);
    // `--repo` is a global arg (must precede the subcommand); `build cover` is the
    // nested cover subcommand (renamed from the old top-level `covers`).
    cmd.arg("--repo")
        .arg(repo_root)
        .arg("build")
        .arg("cover")
        .arg(slug)
        .arg("--lang")
        .arg(lang);
    if let Some(p) = pages {
        cmd.arg("--pages").arg(p.to_string());
    }
    let out = cmd
        .output()
        .with_context(|| format!("running {bin} build cover {slug} --lang {lang}"))?;
    let mut log = String::new();
    log.push_str(&String::from_utf8_lossy(&out.stdout));
    log.push_str(&String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        bail!("bookmill covers failed:\n{log}");
    }
    Ok(log)
}
