//! Authoritative cover re-render: shell out to `bookmill covers <slug> --lang
//! <lang>`. We never rasterize in the web server — the CLI (resvg) is the single
//! source of truth for what ships.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// Locate the bookmill binary: $BOOKMILL_BIN, then sibling target dirs, then PATH.
pub fn bookmill_bin() -> String {
    if let Ok(b) = std::env::var("BOOKMILL_BIN") {
        return b;
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

/// Run `bookmill covers` for one book+lang. `pages` is passed when the print
/// interior PDF is absent (the resvg path needs a spine page count up front);
/// the front PNG — what the editor shows — is unaffected by the value.
pub fn render_cover(repo_root: &Path, slug: &str, lang: &str, pages: Option<u32>) -> Result<String> {
    let bin = bookmill_bin();
    let mut cmd = Command::new(&bin);
    cmd.arg("--repo")
        .arg(repo_root)
        .arg("covers")
        .arg(slug)
        .arg("--lang")
        .arg(lang);
    if let Some(p) = pages {
        cmd.arg("--pages").arg(p.to_string());
    }
    let out = cmd
        .output()
        .with_context(|| format!("running {bin} covers {slug} --lang {lang}"))?;
    let mut log = String::new();
    log.push_str(&String::from_utf8_lossy(&out.stdout));
    log.push_str(&String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        bail!("bookmill covers failed:\n{log}");
    }
    Ok(log)
}
