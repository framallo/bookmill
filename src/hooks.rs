//! External command hooks. Bookmill stays agnostic about what a hook does: it
//! substitutes per-artifact placeholders into the configured argument list,
//! spawns the program directly (no shell), and reports success or failure.

use crate::config::Hook;
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// Substitute `{file}`, `{format}`, `{lang}`, `{slug}` in a single argument.
fn substitute(arg: &str, file: &str, format: &str, lang: &str, slug: &str) -> String {
    arg.replace("{file}", file)
        .replace("{format}", format)
        .replace("{lang}", lang)
        .replace("{slug}", slug)
}

/// Run every post-build hook that applies to this artifact. A hook whose
/// `formats` filter excludes `format` is skipped. A failing `required` hook
/// returns an error (failing the job); a non-required failure is a warning.
pub fn run_post_build(
    hooks: &[Hook],
    out_path: &Path,
    format: &str,
    lang: &str,
    slug: &str,
) -> Result<()> {
    let file = out_path.to_string_lossy();
    for hook in hooks {
        if !hook.formats.is_empty() && !hook.formats.iter().any(|f| f == format) {
            continue;
        }
        let label = hook.name.clone().unwrap_or_else(|| hook.command.clone());
        let args: Vec<String> = hook
            .args
            .iter()
            .map(|a| substitute(a, &file, format, lang, slug))
            .collect();

        let status = Command::new(&hook.command)
            .args(&args)
            .status()
            .with_context(|| format!("failed to spawn hook '{}' ({})", label, hook.command));

        match status {
            Ok(s) if s.success() => {
                println!("      \u{2713} hook {label}");
            }
            Ok(s) => {
                let msg = format!("hook '{label}' exited with status {s}");
                if hook.required {
                    bail!(msg);
                }
                eprintln!("      ! {msg} (non-required, continuing)");
            }
            Err(e) => {
                if hook.required {
                    return Err(e);
                }
                eprintln!("      ! {e:#} (non-required, continuing)");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Hook;

    fn hook(command: &str, args: &[&str], formats: &[&str], required: bool) -> Hook {
        Hook {
            command: command.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            formats: formats.iter().map(|s| s.to_string()).collect(),
            required,
            name: None,
        }
    }

    #[test]
    fn substitutes_all_placeholders() {
        let out = substitute(
            "clean:{file}:{format}:{lang}:{slug}",
            "/o/book.epub",
            "epub",
            "es",
            "mybook",
        );
        assert_eq!(out, "clean:/o/book.epub:epub:es:mybook");
    }

    #[test]
    fn format_filter_skips_non_matching() {
        // Command would fail if run, but the pdf filter excludes an epub artifact,
        // so it is never spawned and the run succeeds.
        let h = hook("this-command-does-not-exist", &["{file}"], &["pdf"], true);
        let r = run_post_build(&[h], Path::new("/tmp/x.epub"), "epub", "en", "s");
        assert!(r.is_ok());
    }

    #[test]
    fn required_hook_failure_is_an_error() {
        let h = hook("this-command-does-not-exist", &["{file}"], &[], true);
        let r = run_post_build(&[h], Path::new("/tmp/x.epub"), "epub", "en", "s");
        assert!(r.is_err());
    }

    #[test]
    fn non_required_hook_failure_is_tolerated() {
        let h = hook("this-command-does-not-exist", &["{file}"], &[], false);
        let r = run_post_build(&[h], Path::new("/tmp/x.epub"), "epub", "en", "s");
        assert!(r.is_ok());
    }

    #[test]
    fn successful_hook_runs_and_receives_substituted_path() {
        let dir = std::env::temp_dir().join(format!("bookmill_hook_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("touched.txt");
        let _ = std::fs::remove_file(&marker);
        // `cp {file} <marker>` copies the artifact to a marker path we then verify.
        let src = dir.join("book.epub");
        std::fs::write(&src, b"artifact-bytes").unwrap();
        let h = hook("cp", &["{file}", marker.to_str().unwrap()], &["epub"], true);
        let r = run_post_build(&[h], &src, "epub", "en", "book");
        assert!(r.is_ok(), "{r:?}");
        assert_eq!(std::fs::read(&marker).unwrap(), b"artifact-bytes");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
