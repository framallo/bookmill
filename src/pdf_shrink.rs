//! PDF image compressor — the PDF counterpart of `epub_shrink`.
//!
//! Downsamples the images embedded in a PDF so the *digital* (retail/gumroad)
//! download stays small, without touching the KDP/POD print interior (which
//! needs full-res images). A full-bleed color picture book exported for print is
//! ~190 MB; at 150 DPI it drops to a few MB with no visible loss on screen.
//!
//! PDFs can't be reflowed/repacked natively the way EPUB images can, so — in
//! keeping with bookmill's v1 orchestrator design (Typst for PDF, epubcheck for
//! validation) — this drives the proven **Ghostscript** engine. The binary is
//! exec'd directly (no shell), so an interactive alias like `gs=git status`
//! never applies; `$GHOSTSCRIPT`/`$GS` override the lookup.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// Locate the Ghostscript binary: honor `$GHOSTSCRIPT` / `$GS`, else try `gs` on
/// `PATH` and the common Homebrew/Unix install locations. Returns the first that
/// answers `--version`.
fn gs_binary() -> Option<String> {
    for var in ["GHOSTSCRIPT", "GS"] {
        if let Ok(p) = std::env::var(var) {
            if !p.is_empty() {
                return Some(p);
            }
        }
    }
    for cand in ["gs", "/opt/homebrew/bin/gs", "/usr/local/bin/gs", "/usr/bin/gs"] {
        let ok = Command::new(cand)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return Some(cand.to_string());
        }
    }
    None
}

/// Downsample the images in `pdf` in place to `dpi` (bicubic) via Ghostscript.
/// Returns `(before, after)` byte sizes. If the recompressed file is not smaller
/// (e.g. a text-only PDF with no images to downsample), the original is kept and
/// `after == before`.
pub fn shrink_pdf(pdf: &Path, dpi: u32) -> Result<(u64, u64)> {
    let gs = gs_binary().context(
        "Ghostscript (gs) not found — install it (brew install ghostscript) or set $GHOSTSCRIPT",
    )?;
    let before = std::fs::metadata(pdf)
        .map(|m| m.len())
        .with_context(|| format!("reading {}", pdf.display()))?;

    let dir = pdf.parent().unwrap_or_else(|| Path::new("."));
    let stem = pdf.file_name().and_then(|s| s.to_str()).unwrap_or("out");
    let tmp = dir.join(format!(".{stem}.gsshrink.pdf"));

    // Mono (1-bit) art degrades fast when downsampled, so keep it at a higher
    // floor than the color/gray target.
    let mono_dpi = dpi.max(300);
    let args: Vec<String> = vec![
        "-sDEVICE=pdfwrite".into(),
        "-dCompatibilityLevel=1.5".into(),
        "-dNOPAUSE".into(),
        "-dBATCH".into(),
        "-dQUIET".into(),
        "-dSAFER".into(),
        "-dDetectDuplicateImages=true".into(),
        "-dDownsampleColorImages=true".into(),
        "-dColorImageDownsampleType=/Bicubic".into(),
        format!("-dColorImageResolution={dpi}"),
        "-dDownsampleGrayImages=true".into(),
        "-dGrayImageDownsampleType=/Bicubic".into(),
        format!("-dGrayImageResolution={dpi}"),
        "-dDownsampleMonoImages=true".into(),
        "-dMonoImageDownsampleType=/Subsample".into(),
        format!("-dMonoImageResolution={mono_dpi}"),
        "-dCompressFonts=true".into(),
        "-dSubsetFonts=true".into(),
        format!("-sOutputFile={}", tmp.display()),
    ];
    let status = Command::new(&gs)
        .args(&args)
        .arg(pdf)
        .status()
        .with_context(|| format!("running ghostscript ({gs})"))?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        bail!("ghostscript failed on {}", pdf.display());
    }
    let after = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    if after == 0 {
        let _ = std::fs::remove_file(&tmp);
        bail!("ghostscript produced an empty file for {}", pdf.display());
    }
    // Only replace when we actually saved space — a lean text PDF can grow.
    if after < before {
        std::fs::rename(&tmp, pdf)
            .with_context(|| format!("replacing {}", pdf.display()))?;
        Ok((before, after))
    } else {
        let _ = std::fs::remove_file(&tmp);
        Ok((before, before))
    }
}
