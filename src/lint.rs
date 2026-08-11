//! `bookmill lint` — prose linter, a thin driver over the `prose-lint` library.
//!
//! The linting engine (missing-tilde detection, voseo-aware spelling, canon/house
//! rules, and the offline LanguageTool-style grammar rule engine) lives in the
//! standalone, reusable [`prose_lint`] crate. bookmill resolves book → slug/lang →
//! chapter files and the effective allow-list / forbidden-terms / style from
//! config, then streams `prose_lint`'s findings.
//!
//! Default `bookmill lint` runs tildes + spelling + canon (fast, offline).
//! `bookmill lint --deep` (alias `--languagetool`) additionally runs the grammar
//! pass — the offline rule engine, plus Harper/nlprule if `prose-lint` was built
//! with those features.

use crate::config::{BookConfig, LintConfig, FORBIDDEN_PUBLIC};
use crate::discover::Repo;
use anyhow::{bail, Result};
use prose_lint::{lint_markdown, Kind, Lang, Opts, Variant};
use std::collections::HashSet;

/// Default forbidden terms the project always cares about (merged with the
/// `[lint].forbid` config + `FORBIDDEN_PUBLIC`). Matched case-insensitively.
const DEFAULT_FORBID: &[&str] = &["Pingüina", "Animal Farm", "Orwell", "Rebelión en la granja"];

/// Effective lint settings for a book, merged repo-root `[lint]` → book `[lint]`.
struct Merged {
    ignore: HashSet<String>,
    forbid: Vec<String>,
    allow_percentages: bool,
    /// Lowercased grammar rule ids / categories to suppress (LanguageTool backend).
    disable_rules: HashSet<String>,
    /// Spanish register (voseo vs. tuteo).
    variant: Variant,
}

/// Build the effective lint settings by merging repo-root `[lint]` with book `[lint]`.
fn merged_lint(repo_lint: &LintConfig, book_lint: &LintConfig) -> Merged {
    let mut ignore = HashSet::new();
    for w in repo_lint.ignore.iter().chain(book_lint.ignore.iter()) {
        let w = w.trim().to_lowercase();
        if !w.is_empty() {
            ignore.insert(w);
        }
    }
    let mut forbid: Vec<String> = Vec::new();
    for t in DEFAULT_FORBID
        .iter()
        .map(|s| s.to_string())
        .chain(FORBIDDEN_PUBLIC.iter().map(|s| s.to_string()))
        .chain(repo_lint.forbid.iter().cloned())
        .chain(book_lint.forbid.iter().cloned())
    {
        let t = t.trim().to_string();
        if !t.is_empty() && !forbid.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
            forbid.push(t);
        }
    }
    let style = book_lint.style.as_deref().or(repo_lint.style.as_deref());
    let allow_percentages = matches!(style, Some("business") | Some("technical"));

    // Grammar rule overrides — repo + book merged, lowercased for case-insensitive
    // matching against a finding's `/`-separated rule segments.
    let mut disable_rules = HashSet::new();
    for r in repo_lint.disable_rules.iter().chain(book_lint.disable_rules.iter()) {
        let r = r.trim().to_lowercase();
        if !r.is_empty() {
            disable_rules.insert(r);
        }
    }
    // Register — book overrides repo; absent → Rioplatense (voseo accepted).
    let variant = book_lint
        .variant
        .as_deref()
        .or(repo_lint.variant.as_deref())
        .map(Variant::from_name)
        .unwrap_or_default();

    Merged { ignore, forbid, allow_percentages, disable_rules, variant }
}

fn langs_for(book: &BookConfig, lang: &Option<String>) -> Vec<String> {
    match lang {
        Some(l) if l != "all" => vec![l.clone()],
        _ => book.languages.clone(),
    }
}

/// Native prose lint for one book (or all) and one language (or all). When
/// `grammar` is set, the grammar pass runs too (`--deep`). Non-zero exit (via
/// `bail`) when any issue remains, so it can gate a release.
/// Reference-style footnote defects in one chapter, as (line, message):
///   * a `[^id]` reference with no `[^id]:` definition in the file;
///   * a definition-looking line `[^id] …` MISSING its colon (renders as
///     literal brackets — the exact bug class readers report);
///   * a `[^id]:` definition nothing references.
pub fn footnote_findings(md: &str) -> Vec<(usize, String)> {
    use std::collections::BTreeSet;
    let mut refs: Vec<(usize, String)> = Vec::new();
    let mut defs: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut fence = false;
    for (n, line) in md.lines().enumerate() {
        let ln = n + 1;
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        // definition (or colon-less near-definition) at line start
        if let Some(rest) = line.strip_prefix("[^") {
            if let Some(close) = rest.find(']') {
                let id = &rest[..close];
                if !id.is_empty() && !id.contains(' ') {
                    let after = &rest[close + 1..];
                    if after.starts_with(':') {
                        defs.insert(id.to_string());
                        continue;
                    } else if after.starts_with(' ') {
                        out.push((ln, format!("footnote definition \"[^{id}]\" is missing its colon (write \"[^{id}]:\") — renders as literal brackets")));
                        continue;
                    }
                }
            }
        }
        // references anywhere in the line
        let mut rest: &str = line;
        let mut _col = 0;
        while let Some(p) = rest.find("[^") {
            let after = &rest[p + 2..];
            if let Some(close) = after.find(']') {
                let id = &after[..close];
                if !id.is_empty() && !id.contains(' ') {
                    refs.push((ln, id.to_string()));
                }
                rest = &after[close + 1..];
                _col += p + 2 + close + 1;
            } else {
                break;
            }
        }
    }
    for (ln, id) in &refs {
        if !defs.contains(id) {
            out.push((*ln, format!("footnote reference \"[^{id}]\" has no definition — renders as literal brackets")));
        }
    }
    let referenced: BTreeSet<&String> = refs.iter().map(|(_, id)| id).collect();
    for id in &defs {
        if !referenced.contains(id) {
            out.push((0, format!("footnote definition \"[^{id}]:\" is never referenced")));
        }
    }
    out.sort();
    out
}

pub fn run(repo: &Repo, book: Option<String>, lang: Option<String>, grammar: bool) -> Result<()> {
    let books = match &book {
        Some(s) => vec![repo.find_book(s)?],
        None => {
            let mut v = Vec::new();
            for d in repo.book_dirs()? {
                v.push(repo.load_book_at(&d)?);
            }
            v
        }
    };

    let mut grand_total = 0usize;
    for (b, dir) in books {
        let Merged { ignore, forbid, allow_percentages, disable_rules, variant } =
            merged_lint(&repo.config.lint, &b.lint);
        for l in langs_for(&b, &lang) {
            let Some(content) = b.content.get(&l) else {
                println!("  {} [{l}] — no [content.{l}]", b.slug);
                continue;
            };
            let files = match content.resolve(&dir) {
                Ok(f) => f,
                Err(e) => {
                    println!("  {} [{l}] — content error: {e}", b.slug);
                    continue;
                }
            };
            let opts = Opts {
                ignore: ignore.clone(),
                forbid: forbid.clone(),
                allow_percentages,
                grammar,
                disable_rules: disable_rules.clone(),
                variant,
            };
            let lang_enum = Lang::from_code(&l);

            let mut notes = Vec::new();
            if lang_enum == Lang::Es {
                notes.push(match variant {
                    Variant::Rioplatense => "voseo-aware".to_string(),
                    Variant::General => "formal (tuteo)".to_string(),
                });
            }
            if grammar {
                notes.push("grammar".to_string());
                if !disable_rules.is_empty() {
                    notes.push(format!("{} rule(s) off", disable_rules.len()));
                }
            }
            if !ignore.is_empty() {
                notes.push(format!("{} allow-listed", ignore.len()));
            }
            let note = if notes.is_empty() { String::new() } else { format!("; {}", notes.join(", ")) };
            println!("\nLinting {}/{l} — {} chapters (prose-lint{note})\n", b.slug, files.len());

            let mut total = 0usize;
            let mut accent_total = 0usize;
            let mut hidden_total = 0usize;
            for path in &files {
                let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let raw = std::fs::read_to_string(path).unwrap_or_default();
                // Broken reference-style footnotes render as literal "[^N]"
                // bracket text in the published book — the classic shipped-bug.
                for (line, msg) in footnote_findings(&raw) {
                    println!("      [footnote:{line}] {msg}");
                    total += 1;
                }
                let report = lint_markdown(&raw, lang_enum, &opts);
                let ptotal: usize = report.findings.iter().map(|f| f.count).sum();
                let paccent: usize = report.findings.iter().filter(|f| f.kind == Kind::Tilde).map(|f| f.count).sum();

                let flag = if paccent > 0 { " [accents!]" } else { "" };
                println!("  {name}: {ptotal} issues ({paccent} accent){flag}");
                for f in &report.findings {
                    let loc = if f.line > 0 { format!(":{}", f.line) } else { String::new() };
                    let rep = if f.suggestion.is_empty() { String::new() } else { format!(" -> {}", f.suggestion) };
                    let times = if f.count > 1 { format!(" (×{})", f.count) } else { String::new() };
                    println!("      [{}{}] {}{}{}", f.kind.tag(), loc, f.text, rep, times);
                    if !f.message.is_empty() {
                        println!("            {}", f.message);
                    }
                    println!("            {}", f.context);
                }
                println!();
                total += ptotal;
                accent_total += paccent;
                hidden_total += report.hidden;
            }
            let suppressed = if hidden_total > 0 { format!(", {hidden_total} allow-listed hidden") } else { String::new() };
            println!(
                "TOTAL: {total} issues across {} chapters ({accent_total} accent/tilde{suppressed}).",
                files.len()
            );
            grand_total += total;
        }
    }

    if grand_total > 0 {
        bail!("lint found {grand_total} issue(s)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::footnote_findings;

    #[test]
    fn broken_footnotes_detected() {
        // the exact defect classes shipped in abre-la-válvula (reader complaint)
        let md = "# T\n\nref here [^6].\n\nanother [^3] here.\n\n[^6] Servicio + Disciplina = Creatividad\n\n[^9]: never referenced\n";
        let f = footnote_findings(md);
        let msgs: Vec<&str> = f.iter().map(|(_, m)| m.as_str()).collect();
        assert!(msgs.iter().any(|m| m.contains("[^6]") && m.contains("missing its colon")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("[^3]") && m.contains("no definition")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("[^9]") && m.contains("never referenced")), "{msgs:?}");
    }

    #[test]
    fn healthy_footnotes_clean() {
        let md = "# T\n\nref [^1].\n\n[^1]: a proper definition\n";
        assert!(footnote_findings(md).is_empty());
        // code fences don't count
        let code = "# T\n\n```md\n[^5] not a real footnote\n```\n";
        assert!(footnote_findings(code).is_empty());
    }
}
