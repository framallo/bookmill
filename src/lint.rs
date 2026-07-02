//! `bookmill lint` — native (Python-free) prose linter for the book series.
//!
//! This is a self-contained port of the high-value checks from the legacy
//! `scripts/lint-prose.py`, so the default `bookmill lint` needs no Python, Java,
//! or LanguageTool. It focuses on the checks that actually catch bugs on this
//! project, in priority order:
//!
//!   1. **Missing / misplaced Spanish tildes** (the #1 AI failure here). Powered
//!      by the pure-Rust, Hunspell-compatible `spellbook` crate against a bundled
//!      es_ES dictionary. Rule: a token that is NOT in the dictionary but whose
//!      *accented* form IS valid → "missing tilde -> <suggestion>" (e.g.
//!      `leon`→`león`, `habia`→`había`, `senor`→`señor`).
//!   2. **Unknown words** (spelling), after tildes, suppressed by the allow-list
//!      and the voseo rules below.
//!   3. **AI-ism / canon** regex checks: em dashes in body text, the `%` symbol
//!      (the series writes percentages in words), and a forbidden-terms set.
//!
//! **Voseo-awareness.** The es_ES Hunspell dictionary does not contain Argentine
//! voseo forms (`tenés`, `pensás`, `mirá`, `sos`, `fijate`, …). Those are canon
//! for this series, so a built-in voseo allow (2nd-person endings `-ás/-és/-ís`,
//! imperative endings `-á/-é/-í`, `sos`, and a handful of clitic imperatives)
//! stops them from ever being reported as spelling errors. Voseo forms carry
//! their accent already, so they are never mistaken for a *missing*-tilde issue.
//!
//! **Deep mode.** `bookmill lint --deep` keeps the old behavior: it shells out to
//! `scripts/lint-prose.py` (LanguageTool) for full grammar — see `scripts.rs`.
//!
//! **Dictionaries.** es_ES + en_US Hunspell `.aff`/`.dic` are embedded into the
//! binary from `assets/dictionaries/` via `include_str!` (LibreOffice-family
//! dictionaries shipped with Calibre). They load in ~20 ms and add ~1.4 MB to the
//! binary. If a language has no bundled dictionary the spelling/tilde checks are
//! skipped for it (the regex canon checks still run) — this is not a silent
//! no-op: it prints a clear note.

use crate::config::{BookConfig, LintConfig, FORBIDDEN_PUBLIC};
use crate::discover::Repo;
use anyhow::{bail, Result};
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::OnceLock;

// ---- embedded Hunspell dictionaries (see module docs) ----
const ES_AFF: &str = include_str!("../assets/dictionaries/es_ES.aff");
const ES_DIC: &str = include_str!("../assets/dictionaries/es_ES.dic");
const EN_AFF: &str = include_str!("../assets/dictionaries/en_US.aff");
const EN_DIC: &str = include_str!("../assets/dictionaries/en_US.dic");

/// Default forbidden terms the project always cares about (merged with the
/// `[lint].forbid` config + `FORBIDDEN_PUBLIC`). Matched case-insensitively.
const DEFAULT_FORBID: &[&str] = &["Pingüina", "Animal Farm", "Orwell", "Rebelión en la granja"];

/// Explicit voseo / clitic-imperative forms the suffix rules don't catch.
const VOSEO_WORDS: &[&str] = &[
    "sos", "vos", "fijate", "fijense", "fijate", "dale", "andate", "vení",
    "pensalo", "decime", "mirame", "escuchame", "esperame", "vamos", "ponete",
    "tomate", "quedate", "callate", "acordate", "fijense",
];

/// A classification of a single token (cached per unique token).
enum Class {
    /// in the dictionary (or accepted): not an issue
    Ok,
    /// missing/misplaced tilde; carries the suggested corrected form
    Tilde(String),
    /// correct voseo form: silently accepted (not counted)
    Voseo,
    /// unknown word (spelling)
    Spell,
}

/// One reportable issue within a chapter.
struct Finding {
    kind: Kind,
    /// the flagged token / term
    text: String,
    /// suggestion (tilde fixes) — empty otherwise
    suggestion: String,
    /// a short context snippet
    context: String,
    /// source line (1-based) for regex/canon findings; 0 = not line-anchored
    line: usize,
    /// occurrences collapsed into this finding (>=1)
    count: usize,
}

#[derive(PartialEq)]
enum Kind {
    Tilde,
    Spell,
    Canon,
}

fn strip_markdown_re() -> &'static Vec<(Regex, &'static str)> {
    static RE: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    RE.get_or_init(|| {
        vec![
            (Regex::new(r"(?s)```.*?```").unwrap(), " "), // code fences
            (Regex::new(r"`[^`]*`").unwrap(), " "),        // inline code
            (Regex::new(r"!?\[([^\]]*)\]\([^)]*\)").unwrap(), "$1"), // links/images
            (Regex::new(r"\{[^}]*\}").unwrap(), " "),      // pandoc attrs {width=80%}
            (Regex::new(r"(?m)^#{1,6}\s*").unwrap(), ""),  // headings
            (Regex::new(r"[*_>#]").unwrap(), ""),          // emphasis marks
            (Regex::new(r"[ \t]{2,}").unwrap(), " "),      // collapse spaces
        ]
    })
}

/// Cheap markdown → plain prose so the spellchecker sees words, not syntax.
/// Mirrors `strip_markdown` in lint-prose.py.
fn strip_markdown(text: &str) -> String {
    let mut s = text.to_string();
    for (re, rep) in strip_markdown_re() {
        s = re.replace_all(&s, *rep).into_owned();
    }
    s
}

// ---- accent handling ----

/// Strip a single accent/diacritic to its base letter (á→a, ñ→n, ü→u, …).
fn deaccent_char(c: char) -> char {
    match c {
        'á' => 'a', 'é' => 'e', 'í' => 'i', 'ó' => 'o', 'ú' | 'ü' => 'u', 'ñ' => 'n',
        'Á' => 'A', 'É' => 'E', 'Í' => 'I', 'Ó' => 'O', 'Ú' | 'Ü' => 'U', 'Ñ' => 'N',
        other => other,
    }
}

/// The accent variants a base letter can take (including itself). Vowels take an
/// acute (u also a diaeresis); n takes ñ; everything else is invariant.
fn variants(base: char) -> &'static [char] {
    match base {
        'a' => &['a', 'á'],
        'e' => &['e', 'é'],
        'i' => &['i', 'í'],
        'o' => &['o', 'ó'],
        'u' => &['u', 'ú', 'ü'],
        'n' => &['n', 'ñ'],
        'A' => &['A', 'Á'],
        'E' => &['E', 'É'],
        'I' => &['I', 'Í'],
        'O' => &['O', 'Ó'],
        'U' => &['U', 'Ú', 'Ü'],
        'N' => &['N', 'Ñ'],
        _ => &[],
    }
}

/// Find an accented form of `token` that IS valid in the dictionary, if any.
/// Works from the de-accented base, so it catches both missing tildes
/// (`leon`→`león`) and misplaced ones (`musíca`→`música`). Returns the first
/// valid candidate that differs from the original token.
fn tilde_suggestion(dict: &spellbook::Dictionary, token: &str) -> Option<String> {
    let base: Vec<char> = token.chars().map(deaccent_char).collect();
    // per-position option lists (a single-element list for non-accentable chars)
    let opts: Vec<Vec<char>> = base
        .iter()
        .map(|&c| {
            let v = variants(c);
            if v.is_empty() {
                vec![c]
            } else {
                v.to_vec()
            }
        })
        .collect();
    // count accentable positions to bound the search
    let accentable: usize = base.iter().filter(|&&c| !variants(c).is_empty()).count();
    let full_product: u64 = opts.iter().map(|o| o.len() as u64).product();

    let mut found: Option<String> = None;
    if full_product <= 8192 {
        // exhaustive cartesian product over per-position options
        let mut idx = vec![0usize; opts.len()];
        loop {
            let cand: String = opts.iter().zip(&idx).map(|(o, &i)| o[i]).collect();
            if cand != token && dict.check(&cand) {
                found = Some(cand);
                break;
            }
            // increment mixed-radix counter
            let mut p = opts.len();
            loop {
                if p == 0 {
                    idx.clear();
                    break;
                }
                p -= 1;
                idx[p] += 1;
                if idx[p] < opts[p].len() {
                    break;
                }
                idx[p] = 0;
            }
            if idx.is_empty() {
                break;
            }
        }
    } else if accentable > 0 {
        // too many combos: try single-position accentings only (one accent added)
        'outer: for (pos, &c) in base.iter().enumerate() {
            for &alt in variants(c).iter().skip(1) {
                let cand: String = base
                    .iter()
                    .enumerate()
                    .map(|(i, &b)| if i == pos { alt } else { b })
                    .collect();
                if cand != token && dict.check(&cand) {
                    found = Some(cand);
                    break 'outer;
                }
            }
        }
    }
    found
}

/// True when `token` is a correct Argentine voseo form (accepted, never flagged).
fn is_voseo(token: &str) -> bool {
    let low = token.to_lowercase();
    if VOSEO_WORDS.contains(&low.as_str()) {
        return true;
    }
    if low.chars().count() < 3 {
        return false;
    }
    // present-indicative voseo: tenés, pensás, salís, comés…
    if low.ends_with("ás") || low.ends_with("és") || low.ends_with("ís") {
        return true;
    }
    // voseo imperative: mirá, comé, viví, andá, dejá (single accented final vowel)
    matches!(low.chars().last(), Some('á') | Some('é') | Some('í'))
}

/// Pronominal enclitics that attach to imperatives (longest first so we strip the
/// full clitic before a shorter suffix of it).
const CLITICS: &[&str] = &[
    "noslos", "noslas", "noslo", "nosla", "melos", "melas", "telos", "telas",
    "selos", "selas", "melo", "mela", "telo", "tela", "selo", "sela", "los",
    "las", "nos", "me", "te", "se", "lo", "la", "le",
];

/// True when `token` is a voseo imperative + enclitic pronoun that Spanish writes
/// WITHOUT the accent the tú form needs, e.g. `cuidalo` (voseo `cuidá`+`lo`) vs
/// the tú `cuídalo`, `imaginate` vs `imagínate`, `leela` vs `léela`. The es_ES
/// dictionary only has the accented tú form, so without this the tilde detector
/// would "correct" a perfectly good voseo form. Signal: strip the enclitic and
/// the remaining stem is itself a real verb form (`cuida`, `imagina`, `pregunta`).
/// This is deliberately voseo-biased (the series is voseo canon): it can hide a
/// genuinely mis-accented enclitic, but never a plain missing-tilde word (those
/// don't end in a clitic with a valid verb stem — e.g. `terminos`→`términos`
/// strips to `termi`, which is not a word, so it still surfaces).
fn is_voseo_enclitic(dict: &spellbook::Dictionary, token: &str) -> bool {
    let low = token.to_lowercase();
    for cl in CLITICS {
        if low.len() > cl.len() && low.ends_with(cl) {
            let stem = &low[..low.len() - cl.len()];
            if stem.chars().count() >= 3 && (dict.check(stem) || is_voseo(stem)) {
                return true;
            }
        }
    }
    false
}

/// Load a bundled dictionary for a language, cached process-wide.
fn dictionary(lang: &str) -> Option<&'static spellbook::Dictionary> {
    static ES: OnceLock<Option<spellbook::Dictionary>> = OnceLock::new();
    static EN: OnceLock<Option<spellbook::Dictionary>> = OnceLock::new();
    let cell = match lang {
        "es" => &ES,
        "en" => &EN,
        _ => return None,
    };
    cell.get_or_init(|| {
        let (aff, dic) = if lang == "es" { (ES_AFF, ES_DIC) } else { (EN_AFF, EN_DIC) };
        match spellbook::Dictionary::new(aff, dic) {
            Ok(d) => Some(d),
            Err(e) => {
                eprintln!("  (bundled {lang} dictionary failed to load: {e}; skipping spell/tilde checks)");
                None
            }
        }
    })
    .as_ref()
}

/// A token is `word[-word][…]` of unicode letters, keeping internal hyphens and
/// apostrophes (so "Böhm-Bawerk", "l'État" stay whole). Yields (token, char_pos).
fn tokenize(text: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut start = 0usize;
    for (ci, ch) in text.chars().enumerate() {
        let joiner = (ch == '-' || ch == '\'' || ch == '’') && !cur.is_empty();
        if ch.is_alphabetic() || joiner {
            if cur.is_empty() {
                start = ci;
            }
            cur.push(ch);
        } else if !cur.is_empty() {
            out.push((cur.trim_matches(|c| c == '-' || c == '\'' || c == '’').to_string(), start));
            cur.clear();
        }
    }
    if !cur.is_empty() {
        out.push((cur.trim_matches(|c| c == '-' || c == '\'' || c == '’').to_string(), start));
    }
    out.retain(|(t, _)| t.chars().count() > 1 && t.chars().any(|c| c.is_alphabetic()));
    out
}

/// True for an uppercase Roman numeral (I, IV, XIX, XXI, MCMLXXXIV…). These are
/// common in a history book and are never spelling errors.
fn is_roman_numeral(token: &str) -> bool {
    !token.is_empty()
        && token.chars().all(|c| matches!(c, 'I' | 'V' | 'X' | 'L' | 'C' | 'D' | 'M'))
}

/// True when a token is allow-listed (whole token, or every hyphen part).
fn is_ignored(token: &str, ignore: &std::collections::HashSet<String>) -> bool {
    if ignore.is_empty() {
        return false;
    }
    let low = token.to_lowercase();
    if ignore.contains(&low) {
        return true;
    }
    let parts: Vec<&str> = low.split(['-', '\'', '’']).filter(|s| !s.is_empty()).collect();
    parts.len() > 1 && parts.iter().all(|p| ignore.contains(*p))
}

/// Grab a short one-line context window around a char position in `text`.
fn context_at(chars: &[char], pos: usize, len: usize) -> String {
    let start = pos.saturating_sub(28);
    let end = (pos + len + 28).min(chars.len());
    let snip: String = chars[start..end].iter().collect();
    let snip = snip.replace(['\n', '\r'], " ");
    let snip = snip.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("…{snip}…")
}

// ---- per-chapter regex/canon checks ----

fn canon_res() -> (&'static Regex, &'static Regex, &'static Regex) {
    static ATTR: OnceLock<Regex> = OnceLock::new();
    static CODE: OnceLock<Regex> = OnceLock::new();
    static DDASH: OnceLock<Regex> = OnceLock::new();
    (
        ATTR.get_or_init(|| Regex::new(r"\{[^}]*\}").unwrap()),
        CODE.get_or_init(|| Regex::new(r"`[^`]*`").unwrap()),
        DDASH.get_or_init(|| Regex::new(r"(?:^|\s)--(?:\s|$)").unwrap()),
    )
}

/// Regex/canon checks over the RAW lines (so line numbers are exact): em dashes
/// in body text, the `%` symbol, and forbidden terms. Skips code fences and the
/// chapter title / heading lines for the em-dash check.
fn canon_findings(raw: &str, forbid: &[String]) -> Vec<Finding> {
    let (attr_re, code_re, ddash_re) = canon_res();
    let mut out = Vec::new();
    let mut in_fence = false;
    for (i, line) in raw.lines().enumerate() {
        let lineno = i + 1;
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let is_heading = trimmed.starts_with('#');
        // strip pandoc attrs + inline code before punctuation checks
        let cleaned = attr_re.replace_all(line, " ");
        let cleaned = code_re.replace_all(&cleaned, " ").into_owned();

        if !is_heading {
            if cleaned.contains('—') || ddash_re.is_match(&cleaned) {
                out.push(Finding {
                    kind: Kind::Canon,
                    text: "—".into(),
                    suggestion: String::new(),
                    context: format!("…{}…", line.trim()),
                    line: lineno,
                    count: 1,
                });
            }
            if cleaned.contains('%') {
                out.push(Finding {
                    kind: Kind::Canon,
                    text: "%".into(),
                    suggestion: "write percentages in words".into(),
                    context: format!("…{}…", line.trim()),
                    line: lineno,
                    count: 1,
                });
            }
        }
        let low = cleaned.to_lowercase();
        for term in forbid {
            if low.contains(&term.to_lowercase()) {
                out.push(Finding {
                    kind: Kind::Canon,
                    text: term.clone(),
                    suggestion: "forbidden term".into(),
                    context: format!("…{}…", line.trim()),
                    line: lineno,
                    count: 1,
                });
            }
        }
    }
    out
}

// ---- per-chapter spelling / tilde checks ----

struct ChapterReport {
    name: String,
    findings: Vec<Finding>,
    total: usize,
    accent: usize,
    hidden: usize,
}

#[allow(clippy::too_many_arguments)]
fn lint_chapter(
    path: &Path,
    lang: &str,
    dict: Option<&spellbook::Dictionary>,
    ignore: &std::collections::HashSet<String>,
    forbid: &[String],
    cache: &mut HashMap<String, Class>,
) -> ChapterReport {
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let raw = std::fs::read_to_string(path).unwrap_or_default();

    let mut findings: Vec<Finding> = Vec::new();
    let mut hidden = 0usize;

    // --- spelling / tilde over stripped prose ---
    if let Some(dict) = dict {
        let stripped = strip_markdown(&raw);
        let chars: Vec<char> = stripped.chars().collect();
        // group repeats: (kind, token, suggestion) -> (count, first_context)
        let mut groups: BTreeMap<(u8, String, String), (usize, String)> = BTreeMap::new();
        for (tok, pos) in tokenize(&stripped) {
            if is_ignored(&tok, ignore) {
                hidden += 1;
                continue;
            }
            let class = cache.entry(tok.clone()).or_insert_with(|| {
                if dict.check(&tok) || is_roman_numeral(&tok) {
                    Class::Ok
                } else if lang != "es" {
                    // Tilde/voseo logic is Spanish-only. In English, an accented
                    // proper name (Jesús, Frédéric) is simply unknown to the dict;
                    // "fixing" it by stripping the accent would be wrong.
                    Class::Spell
                } else if is_voseo(&tok) {
                    // MUST precede the tilde check: the dictionary contains the
                    // non-voseo tú-form (tenes/podes/comes), so tilde_suggestion
                    // would otherwise "fix" a correct voseo tenés → tenes.
                    Class::Voseo
                } else if let Some(sug) = tilde_suggestion(dict, &tok) {
                    if is_voseo_enclitic(dict, &tok) {
                        Class::Voseo
                    } else {
                        Class::Tilde(sug)
                    }
                } else {
                    Class::Spell
                }
            });
            let (kind_b, sug) = match class {
                Class::Ok | Class::Voseo => continue,
                Class::Tilde(s) => (0u8, s.clone()),
                Class::Spell => (1u8, String::new()),
            };
            let key = (kind_b, tok.clone(), sug);
            let entry = groups.entry(key).or_insert_with(|| (0, context_at(&chars, pos, tok.chars().count())));
            entry.0 += 1;
        }
        for ((kind_b, tok, sug), (count, ctx)) in groups {
            findings.push(Finding {
                kind: if kind_b == 0 { Kind::Tilde } else { Kind::Spell },
                text: tok,
                suggestion: sug,
                context: ctx,
                line: 0,
                count,
            });
        }
    }

    // --- regex/canon over raw lines ---
    findings.extend(canon_findings(&raw, forbid));

    // order: tilde first, then spell, then canon; tildes/spell by token, canon by line
    findings.sort_by(|a, b| {
        let ra = kind_rank(&a.kind);
        let rb = kind_rank(&b.kind);
        ra.cmp(&rb).then(a.line.cmp(&b.line)).then(a.text.cmp(&b.text))
    });

    let total: usize = findings.iter().map(|f| f.count).sum();
    let accent: usize = findings.iter().filter(|f| f.kind == Kind::Tilde).map(|f| f.count).sum();
    ChapterReport { name, findings, total, accent, hidden }
}

fn kind_rank(k: &Kind) -> u8 {
    match k {
        Kind::Tilde => 0,
        Kind::Spell => 1,
        Kind::Canon => 2,
    }
}

fn tag(k: &Kind) -> &'static str {
    match k {
        Kind::Tilde => "TILDE",
        Kind::Spell => "SPELL",
        Kind::Canon => "CANON",
    }
}

// ---- driver ----

/// Build the effective allow-list (lowercased) and forbidden-term list for a book
/// by merging the repo-root `[lint]` with the per-book `[lint]`.
fn merged_lint(repo_lint: &LintConfig, book_lint: &LintConfig) -> (std::collections::HashSet<String>, Vec<String>) {
    let mut ignore = std::collections::HashSet::new();
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
    (ignore, forbid)
}

fn langs_for(book: &BookConfig, lang: &Option<String>) -> Vec<String> {
    match lang {
        Some(l) if l != "all" => vec![l.clone()],
        _ => book.languages.clone(),
    }
}

/// Native prose lint for one book (or all) and one language (or all).
/// Returns non-zero exit (via `bail`) when any issue remains, so it can gate a
/// release exactly like the Python linter's exit code.
pub fn run(repo: &Repo, book: Option<String>, lang: Option<String>) -> Result<()> {
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
        let (ignore, forbid) = merged_lint(&repo.config.lint, &b.lint);
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
            let dict = dictionary(&l);
            let mut notes = Vec::new();
            if dict.is_none() {
                notes.push("no dictionary (spell/tilde skipped)".to_string());
            }
            if l == "es" {
                notes.push("voseo-aware".to_string());
            }
            if !ignore.is_empty() {
                notes.push(format!("{} allow-listed", ignore.len()));
            }
            let note = if notes.is_empty() { String::new() } else { format!("; {}", notes.join(", ")) };
            println!("\nLinting {}/{l} — {} chapters (native{note})\n", b.slug, files.len());

            let mut cache: HashMap<String, Class> = HashMap::new();
            let mut total = 0usize;
            let mut accent_total = 0usize;
            let mut hidden_total = 0usize;
            for path in &files {
                let rep = lint_chapter(path, &l, dict, &ignore, &forbid, &mut cache);
                total += rep.total;
                accent_total += rep.accent;
                hidden_total += rep.hidden;
                let flag = if rep.accent > 0 { " [accents!]" } else { "" };
                println!("  {}: {} issues ({} accent){flag}", rep.name, rep.total, rep.accent);
                for f in &rep.findings {
                    let rep_str = if f.suggestion.is_empty() { String::new() } else { format!(" -> {}", f.suggestion) };
                    let times = if f.count > 1 { format!(" (×{})", f.count) } else { String::new() };
                    let loc = if f.line > 0 { format!(":{}", f.line) } else { String::new() };
                    println!("      [{}{}] {}{}{}", tag(&f.kind), loc, f.text, rep_str, times);
                    println!("            {}", f.context);
                }
                println!();
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
    use super::*;

    fn es() -> &'static spellbook::Dictionary {
        dictionary("es").expect("es dict")
    }

    #[test]
    fn tilde_missing_is_detected() {
        let d = es();
        assert_eq!(tilde_suggestion(d, "leon").as_deref(), Some("león"));
        assert_eq!(tilde_suggestion(d, "habia").as_deref(), Some("había"));
        // already-correct word → no suggestion (it's valid, wouldn't reach here anyway)
        assert!(d.check("león"));
    }

    #[test]
    fn voseo_is_accepted() {
        assert!(is_voseo("tenés"));
        assert!(is_voseo("pensás"));
        assert!(is_voseo("mirá"));
        assert!(is_voseo("sos"));
        assert!(is_voseo("fijate"));
        assert!(!is_voseo("casa"));
        assert!(!is_voseo("el"));
    }

    #[test]
    fn voseo_enclitics_not_flagged_as_tilde() {
        let d = es();
        // correct voseo enclitic imperatives: accepted (their tú form has the accent)
        for w in ["cuidalo", "imaginate", "preguntale", "largate", "leela"] {
            assert!(is_voseo_enclitic(d, w), "{w} should be voseo enclitic");
        }
        // a genuine missing-tilde noun ending in a clitic-like suffix still surfaces
        assert!(!is_voseo_enclitic(d, "terminos"));
        assert_eq!(tilde_suggestion(d, "terminos").as_deref(), Some("términos"));
    }

    #[test]
    fn roman_numerals_skipped() {
        assert!(is_roman_numeral("XIX"));
        assert!(is_roman_numeral("MCMLXXXIV"));
        assert!(!is_roman_numeral("Milei"));
    }

    #[test]
    fn tokenize_keeps_compound_names() {
        let t = tokenize("Böhm-Bawerk dijo algo");
        assert!(t.iter().any(|(w, _)| w == "Böhm-Bawerk"));
    }

    #[test]
    fn canon_flags_emdash_and_percent_not_headings() {
        let f = canon_findings("# Título — con guion\nEl texto — largo.\nSubió 5%.\n", &[]);
        // heading line's em dash is NOT flagged; body em dash + % are
        assert!(f.iter().any(|x| x.text == "—" && x.line == 2));
        assert!(f.iter().any(|x| x.text == "%" && x.line == 3));
        assert!(!f.iter().any(|x| x.line == 1));
    }

    #[test]
    fn forbidden_terms_are_flagged() {
        let f = canon_findings("Esto es como Animal Farm de Orwell.\n", &["Animal Farm".into(), "Orwell".into()]);
        assert_eq!(f.iter().filter(|x| x.kind == Kind::Canon).count(), 2);
    }
}
