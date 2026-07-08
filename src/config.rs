//! Layered configuration: repo defaults -> book -> edition -> language.
//! Single source of truth for metadata, listing, layout, and content selection.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub type LangMap = BTreeMap<String, String>;

/// Public-copy phrases that must never appear (KDP listings, blurbs, titles).
pub const FORBIDDEN_PUBLIC: &[&str] = &["Animal Farm", "Rebelión en la granja"];

// ---------- repo level (bookmill.toml) ----------
#[derive(Debug, Deserialize)]
pub struct RepoConfig {
    pub author: Option<String>,
    /// repo-wide series name; parsed for forward use (books carry their own series).
    #[allow(dead_code)]
    pub series: Option<String>,
    /// repo-wide default publication date (e.g. 2026); books override via [meta].date
    pub date: Option<toml::Value>,
    #[serde(default)]
    pub languages: Vec<String>,
    /// directory holding the books (convention: "books"; this repo uses "libros")
    #[serde(default = "default_books_dir")]
    pub books_dir: String,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub editions: BTreeMap<String, Edition>,
    /// repo-wide build options ([build]); reserved config, not yet consumed.
    #[serde(default)]
    #[allow(dead_code)]
    pub build: BuildOpts,
    /// repo-wide cover design defaults; books override via their own [cover]
    pub cover: Option<CoverConfig>,
    /// repo-wide audiobook defaults (engine/voice/speed); books override via [audiobook]
    pub audiobook: Option<Audiobook>,
    /// prose-lint allow-list + forbidden terms ([lint]); merged repo→book by `lint`.
    #[serde(default)]
    pub lint: LintConfig,
}

// ---------- prose lint ([lint]) ----------
/// Native prose-lint config. `ignore` holds proper names / technical terms that
/// must never be flagged as spelling errors (it still never hides a genuine
/// tilde/accent issue — see `lint.rs`). `forbid` holds terms that MUST NOT appear
/// in the prose (reported wherever found). Read at both the repo root and the
/// per-book `bookmill.toml`; the two `ignore`/`forbid` lists are merged.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct LintConfig {
    #[serde(default)]
    pub ignore: Vec<String>,
    #[serde(default)]
    pub forbid: Vec<String>,
}

fn default_books_dir() -> String {
    "books".into()
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Defaults {
    pub trim: Option<String>,
    pub bleed: Option<String>,
    /// paper stock: "white" | "cream" | "groundwood" (default "white").
    pub paper: Option<String>,
    /// ink/color: "black" | "standard-color" | "premium-color" (default "black").
    pub ink: Option<String>,
    /// cover finish: "matte" | "glossy" (default "matte"; listing/cover only).
    pub finish: Option<String>,
    /// repo-wide default fonts ([defaults.fonts]); reserved config, not yet consumed.
    #[allow(dead_code)]
    pub fonts: Option<Fonts>,
    /// repo-wide default print margins (inches); books override via [pdf.margins]
    pub margins: Option<Margins>,
}

/// Print page margins, in inches. Resolved per field: book [pdf.margins] wins,
/// then repo [defaults.margins], then a built-in 6x9 text fallback.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct Margins {
    pub top: Option<f64>,
    pub bottom: Option<f64>,
    pub inner: Option<f64>,
    pub outer: Option<f64>,
    pub bindingoffset: Option<f64>,
}

/// Font family overrides ([defaults.fonts]); reserved config, not yet consumed.
#[derive(Debug, Deserialize, Default, Clone)]
#[allow(dead_code)]
pub struct Fonts {
    pub serif: Option<String>,
    pub sans: Option<String>,
    pub sub: Option<String>,
}

/// Repo-wide build options ([build]); reserved config, not yet consumed.
#[derive(Debug, Deserialize, Default, Clone)]
#[allow(dead_code)]
pub struct BuildOpts {
    pub output_dir: Option<String>,
    #[serde(default)]
    pub formats: Vec<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Edition {
    /// distribution market/region label; reserved config, not yet consumed.
    #[allow(dead_code)]
    pub market: Option<String>,
    pub trim: Option<String>,
    /// bleed per edition (e.g. "0.125in", "0"); falls back to book/repo defaults
    pub bleed: Option<String>,
    /// paper stock: "white" | "cream" | "groundwood"; now load-bearing (spine + validation).
    pub paper: Option<String>,
    /// ink/color: "black" | "standard-color" | "premium-color"; drives spine + cost/eligibility.
    pub ink: Option<String>,
    /// cover finish: "matte" | "glossy"; listing/cover only, no interior geometry effect.
    pub finish: Option<String>,
    /// ISBN: "free" (KDP-assigned, non-portable) or an explicit ISBN-13.
    pub isbn: Option<String>,
    /// distribution target: "kdp-paperback" | "kdp-epub" | "gumroad" | "bubok" | ...
    pub target: Option<String>,
    /// max image width (px) for EPUB editions; lower = smaller Kindle delivery
    pub epub_image_px: Option<u32>,
}

// ---------- book level (book.toml) ----------
#[derive(Debug, Deserialize)]
pub struct BookConfig {
    pub slug: String,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub editions: Vec<String>,
    #[serde(default)]
    pub title: LangMap,
    #[serde(default)]
    pub subtitle: LangMap,
    #[serde(default)]
    pub meta: Meta,
    #[serde(default)]
    pub listing: BTreeMap<String, Listing>,
    #[serde(default)]
    pub content: BTreeMap<String, Content>,
    #[serde(default)]
    pub pdf: PdfOpts,
    /// per-book cover design (palette/titles/blurb/wrap_bg…); merges over repo [cover]
    pub cover: Option<CoverConfig>,
    /// per-book audiobook overrides (voice/speed per language); merges over repo [audiobook]
    pub audiobook: Option<Audiobook>,
    /// per-book prose-lint additions ([lint]); merged over the repo-root [lint].
    #[serde(default)]
    pub lint: LintConfig,
    /// per-book publishing status ([status]); drives the TUI ribbon (LIVE/REVIEW/…).
    #[serde(default)]
    pub status: Status,
}

// ---------- publishing status ([status]) ----------
/// Per-channel publishing status, recorded in the book's `[status]` table.
/// Values are free-form strings ("live", "in-review", "draft", "blocked",
/// "canceled", …); `ribbon()` distills them into one badge for the TUI.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct Status {
    /// Explicit overall override; wins over the per-channel derivation.
    pub overall: Option<String>,
    pub kdp_paperback: Option<String>,
    pub kdp_kindle: Option<String>,
    pub kdp_hardcover: Option<String>,
    /// Date first submitted to the store (informational; recorded, not yet surfaced).
    #[allow(dead_code)]
    pub submitted: Option<String>,
    /// Store ASINs keyed however the book likes (e.g. `kindle_es`, `paperback_en`).
    #[allow(dead_code)]
    #[serde(default)]
    pub asin: BTreeMap<String, String>,
}

impl Status {
    /// Canonical ribbon key for the books list: `"live" | "in-review" |
    /// "blocked" | "draft"`. `overall` wins; otherwise the best per-channel
    /// status decides (live beats in-review beats blocked beats draft), so a
    /// paperback that is live while the hardcover is blocked still reads LIVE.
    pub fn ribbon(&self) -> &'static str {
        let channels = [&self.overall, &self.kdp_paperback, &self.kdp_kindle, &self.kdp_hardcover];
        let has = |k: &str| {
            channels
                .iter()
                .filter_map(|c| c.as_deref())
                .any(|v| v.trim().eq_ignore_ascii_case(k))
        };
        if has("live") || has("published") {
            "live"
        } else if has("in-review") || has("in review") || has("review") || has("submitted") {
            "in-review"
        } else if has("blocked") {
            "blocked"
        } else {
            // no status recorded, or only draft/canceled channels → not yet out
            "draft"
        }
    }
}

// ---------- cover design ([cover]) ----------
/// Cover design config — the per-book `CFG` dict from `scripts/make-covers.py`,
/// moved into TOML. Used at both repo level (defaults) and book level (overrides).
/// Every field is optional; rendering resolves book -> repo -> built-in default.
/// Per-language overrides (title size, cover blurb, title/sub text) live in the
/// `[cover.<lang>]` subtables, collected here via `serde(flatten)` into `lang`.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct CoverConfig {
    // fonts / sizing
    pub serif: Option<String>,
    pub subfont: Option<String>,
    pub title_size: Option<f64>,
    pub title_mt: Option<String>,
    pub author_mt: Option<String>,
    // palette
    pub bgcolor: Option<String>,
    pub title_color: Option<String>,
    pub sub_color: Option<String>,
    pub author_color: Option<String>,
    pub accent: Option<String>,
    // strokes / shadows
    pub title_shadow: Option<String>,
    pub title_stroke: Option<String>,
    pub sub_shadow: Option<String>,
    pub sub_stroke: Option<String>,
    pub blurb_color: Option<String>,
    pub blurb_shadow: Option<String>,
    pub blurb_stroke: Option<String>,
    pub author_stroke: Option<String>,
    pub badge_color: Option<String>,
    pub badge_stroke: Option<String>,
    // background art (filenames inside the book's cover/ dir)
    pub bg: Option<String>,
    pub wrap_bg: Option<String>,
    // wrap-specific
    pub wrap_title_px: Option<f64>,
    pub wrap_lh: Option<f64>,
    pub wrap_stroke: Option<String>,
    pub wrap_shadow: Option<String>,
    // photo filter + spine math
    pub filt: Option<String>,
    pub paper_mult: Option<f64>,
    /// inert in make-covers.py (kept for fidelity with the old CFG)
    #[allow(dead_code)]
    pub bottom: Option<String>,
    // text (usually sourced from [title]/[subtitle]/[listing]; overridable here)
    pub blurb: Option<String>,
    // series badge text + page geometry defaults (repo-level)
    pub badge_es: Option<String>,
    pub badge_en: Option<String>,
    pub font_link: Option<String>,
    pub heavy_shadow: Option<String>,
    pub trim_w: Option<f64>,
    pub trim_h: Option<f64>,
    pub bleed: Option<f64>,
    /// per-language overrides: [cover.es], [cover.en]
    #[serde(flatten, default)]
    pub lang: BTreeMap<String, CoverLang>,
}

/// Per-language cover overrides (`[cover.<lang>]`).
#[derive(Debug, Deserialize, Default, Clone)]
pub struct CoverLang {
    /// front title size override (e.g. la-riqueza EN = 235)
    pub title_size: Option<f64>,
    /// cover back-blurb override (else falls back to [listing.<lang>].blurb)
    pub blurb: Option<String>,
    /// cover title override (else book [title.<lang>])
    pub title: Option<String>,
    /// cover subtitle override (else book [subtitle.<lang>])
    pub sub: Option<String>,
    /// absolute per-element eBook-front layout (`[cover.<lang>.layout]`), written
    /// by the web cover editor. When present, the SVG renderer positions
    /// title/subtitle/author from these absolute coordinates instead of its
    /// default flex stack. Absent => unchanged default layout (no regression).
    pub layout: Option<CoverLayout>,
}

/// Absolute eBook-front cover layout (`[cover.<lang>.layout]`) authored by the
/// web cover editor (`web/src/cover.rs`). One optional [`CoverElement`] per
/// draggable text block. Coordinates are fractions of the **1600×2560 eBook
/// front canvas** — this layout governs the eBook front PNG only (the editor is
/// front-only); the paperback wrap keeps its own flex math.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct CoverLayout {
    pub title: Option<CoverElement>,
    pub subtitle: Option<CoverElement>,
    pub author: Option<CoverElement>,
}

/// One absolutely-positioned cover text block. Field names/units match exactly
/// what the web editor persists (`element_inline` in `web/src/cover.rs`):
///   * `xPct`/`yPct` — the block's **center**, as a fraction of canvas width/height.
///   * `wPct` — wrap-box width, as a fraction of canvas **width**.
///   * `fontPct` — font size, as a fraction of canvas **height**.
///   * `fill` / `fontFamily` / `fontStyle` / `text` — style + content overrides.
/// Numeric fields default leniently so a hand-edited partial block never fails the
/// build; the editor always writes complete values.
#[derive(Debug, Deserialize, Clone)]
pub struct CoverElement {
    #[serde(rename = "xPct", default = "half")]
    pub x_pct: f64,
    #[serde(rename = "yPct", default = "half")]
    pub y_pct: f64,
    #[serde(rename = "wPct", default = "default_w_pct")]
    pub w_pct: f64,
    #[serde(rename = "fontPct", default = "default_font_pct")]
    pub font_pct: f64,
    pub fill: Option<String>,
    #[serde(rename = "fontFamily")]
    pub font_family: Option<String>,
    #[serde(rename = "fontStyle")]
    pub font_style: Option<String>,
    pub text: Option<String>,
}

fn half() -> f64 {
    0.5
}
fn default_w_pct() -> f64 {
    0.8
}
fn default_font_pct() -> f64 {
    0.03
}

// ---------- audiobook design ([audiobook]) ----------
/// Audiobook synthesis config — used at both repo level (defaults) and book level
/// (overrides). Drives the TTS engine (kab/Kokoro on the ANE today). Top-level
/// scalars are engine-wide; per-language voice/code/speed live in `[audiobook.<lang>]`
/// subtables, collected via `serde(flatten)` into `lang` (same pattern as [cover]).
#[derive(Debug, Deserialize, Default, Clone)]
pub struct Audiobook {
    /// engine id — "kab" (default; the only built-in for now)
    pub engine: Option<String>,
    /// path to the kab binary; else env KAB/BOOKMILL_KAB, else "kab" on PATH,
    /// else the known build path (~/work/libs/kokoro-audiobook-mcp/.build/release/kab)
    pub kab_bin: Option<String>,
    /// m4b "artist" metadata; else book [meta].author / repo author
    pub artist: Option<String>,
    /// default speech speed (per-language overrides win); default 1.0
    pub speed: Option<f64>,
    /// speak the chapter heading aloud? default true (false => chapter markers only)
    pub speak_titles: Option<bool>,
    /// per-language overrides: [audiobook.es], [audiobook.en]
    #[serde(flatten, default)]
    pub lang: BTreeMap<String, AudiobookLang>,
}

/// Per-language audiobook overrides (`[audiobook.<lang>]`).
#[derive(Debug, Deserialize, Default, Clone)]
pub struct AudiobookLang {
    /// Kokoro voice (e.g. "ef_dora", "af_heart"); else [`default_voice`]
    pub voice: Option<String>,
    /// ane_book language code: a=English e=Spanish f=French i=Italian p=Portuguese;
    /// else [`default_ane_code`] for the language
    pub code: Option<String>,
    /// speech speed override for this language
    pub speed: Option<f64>,
    /// speak the chapter heading aloud? overrides the top-level default
    pub speak_titles: Option<bool>,
}

/// Built-in default Kokoro voice per language (female voices used by the series).
pub fn default_voice(lang: &str) -> &'static str {
    match lang {
        "es" => "ef_dora",
        _ => "af_heart",
    }
}

/// Built-in default ane_book language code per BCP-47-ish language tag.
pub fn default_ane_code(lang: &str) -> &'static str {
    match lang {
        "es" => "e",
        "fr" => "f",
        "it" => "i",
        "pt" => "p",
        _ => "a",
    }
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct PdfOpts {
    /// "recto" => chapters open on the right page (openright); else openany
    pub chapter_opens: Option<String>,
    /// per-book trim override (e.g. "6x9"); else repo defaults / edition trim
    pub trim: Option<String>,
    /// per-book print bleed (e.g. "0.125in" for full-bleed picture books, "0" for
    /// text-only interiors); falls back to the edition / repo-defaults bleed
    pub bleed: Option<String>,
    /// per-book paper stock override ("white"|"cream"|"groundwood"); lets a single
    /// book opt into a stock without a new edition. Falls back to edition / defaults.
    pub paper: Option<String>,
    /// per-book ink/color override ("black"|"standard-color"|"premium-color");
    /// e.g. a picture book opting into premium color. Falls back to edition / defaults.
    pub ink: Option<String>,
    /// per-book cover finish override ("matte"|"glossy"). Falls back to edition /
    /// defaults. Cover/listing only (no interior geometry); reserved, not yet consumed.
    #[allow(dead_code)]
    pub finish: Option<String>,
    /// per-book print margins (inches); falls back to repo [defaults.margins]
    pub margins: Option<Margins>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Meta {
    pub author: Option<String>,
    pub series: Option<String>,
    pub date: Option<toml::Value>,
    pub rights: Option<String>,
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct Listing {
    #[serde(default)]
    pub keywords: Vec<String>,
    pub blurb: Option<String>,
    #[serde(default)]
    pub bisac: Vec<String>,
    pub reading_age: Option<String>,
}

/// Content selection: glob OR explicit files, with optional front/back matter.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct Content {
    pub glob: Option<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub prepend: Vec<String>,
    #[serde(default)]
    pub append: Vec<String>,
}

// ---------- loading ----------
pub fn load_repo(path: &Path) -> Result<RepoConfig> {
    let s = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))
}

pub fn load_book(path: &Path) -> Result<BookConfig> {
    let s = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))
}

// ---------- content resolution ----------
impl Content {
    /// Resolve to an ordered list of chapter files (prepend + body + append).
    /// `base` is the book directory the relative paths are anchored to.
    pub fn resolve(&self, base: &Path) -> Result<Vec<PathBuf>> {
        let mut out: Vec<PathBuf> = self.prepend.iter().map(|p| base.join(p)).collect();
        if let Some(g) = &self.glob {
            let pat = base.join(g);
            let pat_str = pat
                .to_str()
                .with_context(|| format!("non-UTF-8 content glob path: {}", pat.display()))?;
            let mut hits: Vec<PathBuf> = glob::glob(pat_str)
                .with_context(|| format!("bad glob {g}"))?
                .filter_map(|r| r.ok())
                .collect();
            if hits.is_empty() {
                bail!("content glob matched no files: {g}");
            }
            hits.sort();
            out.extend(hits);
        }
        for f in &self.files {
            let p = base.join(f);
            if !p.exists() {
                bail!("content file not found: {}", p.display());
            }
            out.push(p);
        }
        out.extend(self.append.iter().map(|p| base.join(p)));
        Ok(out)
    }
}

// ---------- validation (KDP + house rules) ----------
pub struct Issue {
    pub level: &'static str, // "error" | "warn"
    pub msg: String,
}

pub fn validate_book(book: &BookConfig) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut err = |m: String| issues.push(Issue { level: "error", msg: m });

    for lang in &book.languages {
        if !book.title.contains_key(lang) {
            err(format!("[{lang}] missing title"));
        }
    }
    for (lang, l) in &book.listing {
        if l.keywords.len() > 7 {
            err(format!("[{lang}] {} keywords (KDP max 7)", l.keywords.len()));
        }
        for k in &l.keywords {
            if k.chars().count() > 50 {
                err(format!("[{lang}] keyword over 50 chars: {k:?}"));
            }
        }
        if let Some(b) = &l.blurb {
            if b.chars().count() > 4000 {
                err(format!("[{lang}] blurb over 4000 chars"));
            }
        }
        if l.bisac.len() > 3 {
            err(format!("[{lang}] {} BISAC codes (max 3)", l.bisac.len()));
        }
    }
    // forbidden phrases anywhere in public copy
    let mut public: Vec<String> = Vec::new();
    public.extend(book.title.values().cloned());
    public.extend(book.subtitle.values().cloned());
    for l in book.listing.values() {
        if let Some(b) = &l.blurb {
            public.push(b.clone());
        }
        public.extend(l.keywords.clone());
    }
    for text in &public {
        for bad in FORBIDDEN_PUBLIC {
            if text.contains(bad) {
                err(format!("forbidden phrase in public copy: {bad:?}"));
            }
        }
    }
    issues
}

// ---------- paper/ink → spine, cost, eligibility ----------

/// Allowed values per KDP option (used by validation + docs).
pub const PAPER_VALUES: &[&str] = &["white", "cream", "groundwood"];
pub const INK_VALUES: &[&str] = &["black", "standard-color", "premium-color"];
pub const FINISH_VALUES: &[&str] = &["matte", "glossy"];

/// KDP spine paper multiplier (inches of spine per page), derived from the
/// paper stock + ink. Replaces the hand-set `[cover].paper_mult` constant — the
/// cover renderer should use an explicit `paper_mult` override when present, else
/// fall back to this. Values per KDP's print spine calculator:
///   premium color           → 0.002347
///   cream (black/std color)  → 0.0025
///   white/groundwood (black/std color) → 0.002252
pub fn spine_mult(paper: &str, ink: &str) -> f64 {
    match (paper, ink) {
        (_, "premium-color") => 0.002347,
        ("cream", _) => 0.0025,
        _ => 0.002252,
    }
}

/// Resolve the paper stock for one (edition, book): edition → book `[pdf]` →
/// repo `[defaults]` → "white".
pub fn resolve_paper(edition: Option<&Edition>, book: &BookConfig, repo: &RepoConfig) -> String {
    edition
        .and_then(|e| e.paper.clone())
        .or_else(|| book.pdf.paper.clone())
        .or_else(|| repo.defaults.paper.clone())
        .unwrap_or_else(|| "white".into())
}

/// Resolve the ink/color for one (edition, book): edition → book `[pdf]` →
/// repo `[defaults]` → "black".
pub fn resolve_ink(edition: Option<&Edition>, book: &BookConfig, repo: &RepoConfig) -> String {
    edition
        .and_then(|e| e.ink.clone())
        .or_else(|| book.pdf.ink.clone())
        .or_else(|| repo.defaults.ink.clone())
        .unwrap_or_else(|| "black".into())
}

/// Validate an ISBN-13 (checksum + 13 digits). Hyphens/spaces are ignored.
pub fn isbn13_valid(s: &str) -> bool {
    let digits: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() != 13 {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .enumerate()
        .map(|(i, d)| if i % 2 == 0 { *d } else { *d * 3 })
        .sum();
    sum % 10 == 0
}

/// KDP page range (min, max) for a print target + ink. Coarse, from kdp-requirements §1.
/// Only the documented constraints are enforced: hardcover ≥75pp, standard-color
/// 72–600pp. Premium-color and black paperbacks use the general 24–828pp range
/// (KDP accepts color paperbacks well under 72pp — e.g. la-riqueza at 60pp).
pub fn page_range(target: &str, ink: &str) -> (u32, u32) {
    match (target, ink) {
        ("kdp-hardcover", _) => (75, 550),
        (_, "standard-color") => (72, 600),
        _ => (24, 828),
    }
}

/// Check that every edition a book declares in `book.editions` is defined in the
/// repo `[editions]` table. A typo'd or removed edition would otherwise silently
/// build a "retail" fallback interior instead of the intended KDP one (M1).
pub fn validate_book_editions(book: &BookConfig, repo: &RepoConfig) -> Vec<Issue> {
    let mut issues = Vec::new();
    for ed in &book.editions {
        if !repo.editions.contains_key(ed) {
            let known: Vec<&str> = repo.editions.keys().map(String::as_str).collect();
            issues.push(Issue {
                level: "error",
                msg: format!(
                    "{}: unknown edition {ed:?} (not in [editions]; known: {})",
                    book.slug,
                    if known.is_empty() { "none".into() } else { known.join(", ") }
                ),
            });
        }
    }
    issues
}

/// Validate the static, edition-level paper/ink/finish/ISBN choices. Self-contained
/// (no per-book overrides) — pairs with [`validate_color_compat`] which resolves
/// the per-(edition,book) ink/paper/target. Same `Issue` convention as
/// [`validate_book`]; surface it through the publish/validate path.
pub fn validate_editions(repo: &RepoConfig) -> Vec<Issue> {
    let mut issues = Vec::new();
    let one = |issues: &mut Vec<Issue>, where_: &str, field: &str, val: &Option<String>, allowed: &[&str]| {
        if let Some(v) = val {
            if !allowed.contains(&v.as_str()) {
                issues.push(Issue {
                    level: "error",
                    msg: format!("{where_} {field}={v:?} not one of {allowed:?}"),
                });
            }
        }
    };
    one(&mut issues, "[defaults]", "paper", &repo.defaults.paper, PAPER_VALUES);
    one(&mut issues, "[defaults]", "ink", &repo.defaults.ink, INK_VALUES);
    one(&mut issues, "[defaults]", "finish", &repo.defaults.finish, FINISH_VALUES);
    for (name, e) in &repo.editions {
        let w = format!("[editions.{name}]");
        one(&mut issues, &w, "paper", &e.paper, PAPER_VALUES);
        one(&mut issues, &w, "ink", &e.ink, INK_VALUES);
        one(&mut issues, &w, "finish", &e.finish, FINISH_VALUES);
        if let Some(isbn) = &e.isbn {
            if isbn != "free" && !isbn13_valid(isbn) {
                issues.push(Issue {
                    level: "error",
                    msg: format!(
                        "{w} isbn {isbn:?} is not a valid ISBN-13 (use \"free\" for a KDP-assigned ISBN)"
                    ),
                });
            } else if isbn == "free" {
                issues.push(Issue {
                    level: "warn",
                    msg: format!("{w} uses a free KDP ISBN (non-portable; imprint = \"Independently published\")"),
                });
            }
        }
    }
    issues
}

/// Validate the resolved color choice for one (edition, book): standard color is
/// white-paper + paperback only. `premium-color` is allowed on paperback + hardcover.
pub fn validate_color_compat(
    slug: &str,
    edition: &str,
    paper: &str,
    ink: &str,
    target: &str,
) -> Vec<Issue> {
    let mut issues = Vec::new();
    if ink == "standard-color" {
        if paper != "white" {
            issues.push(Issue {
                level: "error",
                msg: format!("{slug}/{edition}: standard-color ink requires white paper (got {paper:?})"),
            });
        }
        if target == "kdp-hardcover" {
            issues.push(Issue {
                level: "error",
                msg: format!("{slug}/{edition}: standard-color ink is paperback-only (not hardcover)"),
            });
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spine_mult_derivation() {
        assert_eq!(spine_mult("white", "black"), 0.002252);
        assert_eq!(spine_mult("groundwood", "black"), 0.002252);
        assert_eq!(spine_mult("cream", "black"), 0.0025);
        assert_eq!(spine_mult("white", "premium-color"), 0.002347);
        // premium color wins over cream
        assert_eq!(spine_mult("cream", "premium-color"), 0.002347);
        // la-riqueza submitted geometry: white/black → unchanged 0.002252
        assert_eq!(spine_mult("white", "black"), 0.002252);
    }

    #[test]
    fn isbn13_checksum() {
        assert!(isbn13_valid("978-3-16-148410-0"));
        assert!(!isbn13_valid("978-3-16-148410-1"));
        assert!(!isbn13_valid("123"));
    }

    // ---- layered config resolution (edition → book [pdf] → repo [defaults] → built-in) ----

    fn repo_cfg(s: &str) -> RepoConfig {
        toml::from_str(s).unwrap()
    }
    fn book_cfg(s: &str) -> BookConfig {
        toml::from_str(s).unwrap()
    }

    #[test]
    fn resolve_paper_precedence() {
        let repo = repo_cfg("[defaults]\npaper = 'cream'\n");
        let book = book_cfg("slug = 's'\n[pdf]\npaper = 'groundwood'\n");
        let ed = Edition { paper: Some("white".into()), ..Default::default() };
        // edition wins over book [pdf] and repo [defaults]
        assert_eq!(resolve_paper(Some(&ed), &book, &repo), "white");
        // book [pdf] wins over repo [defaults] when no edition sets it
        assert_eq!(resolve_paper(None, &book, &repo), "groundwood");
        // repo [defaults] when the book sets nothing
        let bare = book_cfg("slug = 's'\n");
        assert_eq!(resolve_paper(None, &bare, &repo), "cream");
        // built-in default when nothing is set anywhere
        let empty = repo_cfg("");
        assert_eq!(resolve_paper(None, &bare, &empty), "white");
        // an edition present but WITHOUT paper falls through to book/repo
        let ed_noink = Edition::default();
        assert_eq!(resolve_paper(Some(&ed_noink), &book, &repo), "groundwood");
    }

    #[test]
    fn resolve_ink_precedence() {
        let repo = repo_cfg("[defaults]\nink = 'standard-color'\n");
        let book = book_cfg("slug = 's'\n[pdf]\nink = 'premium-color'\n");
        let ed = Edition { ink: Some("black".into()), ..Default::default() };
        assert_eq!(resolve_ink(Some(&ed), &book, &repo), "black");
        assert_eq!(resolve_ink(None, &book, &repo), "premium-color");
        let bare = book_cfg("slug = 's'\n");
        assert_eq!(resolve_ink(None, &bare, &repo), "standard-color");
        assert_eq!(resolve_ink(None, &bare, &repo_cfg("")), "black");
    }

    #[test]
    fn unknown_edition_is_flagged() {
        let repo = repo_cfg("[editions.kdp-paperback]\ntarget = 'kdp-paperback'\n");
        let book = book_cfg("slug = 's'\neditions = ['kdp-paperback', 'typo']\n");
        let issues = validate_book_editions(&book, &repo);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].level, "error");
        assert!(issues[0].msg.contains("typo"));
        // all-known editions → no issues
        let ok = book_cfg("slug = 's'\neditions = ['kdp-paperback']\n");
        assert!(validate_book_editions(&ok, &repo).is_empty());
        // a book declaring no editions → no issues
        let none = book_cfg("slug = 's'\n");
        assert!(validate_book_editions(&none, &repo).is_empty());
    }
}
