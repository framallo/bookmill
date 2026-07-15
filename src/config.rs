//! Layered configuration: repo defaults -> book -> edition -> language.
//! Single source of truth for metadata, listing, layout, and content selection.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
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
    /// repo-wide default for auto-grayscaling non-`{bw=…}` print images (default
    /// false); a book overrides via `[pdf].auto_grayscale`. See resolve_auto_grayscale.
    pub auto_grayscale: Option<bool>,
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
    /// directory holding the per-book KDP metadata worksheets (`<slug>.md`),
    /// used by `bookmill kdp`. Defaults to "kdp" when unset (back-compatible);
    /// a repo can relocate them, e.g. `worksheet_dir = "publishing/kdp"`.
    pub worksheet_dir: Option<String>,
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
    /// show chapter-plate alt text as a visible caption for this edition; overrides
    /// the book `[pdf].plate_captions`. Set false to hide captions on e.g. KDP.
    pub captions: Option<bool>,
    /// image downsample resolution (DPI) for this edition's *digital* (retail) PDF;
    /// lower = smaller download. Overrides the book `[pdf].digital_pdf_dpi`. Only the
    /// retail PDF is compressed (via Ghostscript); the KDP/POD print interior keeps
    /// full-res images. None = no compression (default).
    pub digital_pdf_dpi: Option<u32>,
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
    /// per-book free-sample options ([sample]); controls the opening-chapters
    /// preview EPUB/PDF generated into output/. Defaults: enabled, ~first 15%.
    #[serde(default)]
    pub sample: Sample,
}

// ---------- free sample ([sample]) ----------
/// Free-sample options: the opening chapters exported as a shareable preview
/// (EPUB + PDF) next to the interiors. Defaults keep it on at ~the first 15%.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct Sample {
    /// number of leading content files (chapters, after any front matter) to
    /// include; explicit value wins over the default heuristic.
    pub chapters: Option<usize>,
    /// set false to skip generating the free sample for this book (default true).
    pub enabled: Option<bool>,
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
    /// draw the accent rule under the title (default true). Set `rule = false` to
    /// remove the horizontal line on both the eBook front and the paperback front.
    pub rule: Option<bool>,
    // strokes / shadows
    pub title_shadow: Option<String>,
    pub title_stroke: Option<String>,
    pub sub_shadow: Option<String>,
    pub sub_stroke: Option<String>,
    pub blurb_color: Option<String>,
    pub blurb_shadow: Option<String>,
    pub blurb_stroke: Option<String>,
    // back-cover blurb emphasis: phrases (per language, in `[cover.<lang>]`) rendered
    // in a standout style. Color/style/family default to the title color, italic,
    // and the blurb serif; override at book or repo level.
    pub blurb_emph_color: Option<String>,
    pub blurb_emph_style: Option<String>,
    pub blurb_emph_family: Option<String>,
    /// which cover surface the web editor exposes: "front" (digital-only, eBook
    /// front only) or "wrap" (paperback: full back+spine+front, front and back
    /// both editable). Absent → derived from editions (any print edition → "wrap",
    /// else "front"). See `cover_edit_mode`.
    pub edit: Option<String>,
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
    pub heavy_shadow: Option<String>,
    pub trim_w: Option<f64>,
    pub trim_h: Option<f64>,
    pub bleed: Option<f64>,
    /// **Shared** eBook-front layout (`[cover.layout]`) — one design for every
    /// language. `[cover.<lang>.layout]` overlays it field-by-field, so ES and EN
    /// render the same cover and differ only where a language actually needs a
    /// tweak (its own text, a smaller title because the words are longer).
    pub layout: Option<CoverLayout>,
    /// **Shared** back-panel layout (`[cover.wrap]`), overlaid the same way by
    /// `[cover.<lang>.wrap]`.
    pub wrap: Option<CoverWrapLayout>,
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
    /// phrases within the back-cover blurb to render in the emphasis style
    /// (matched as consecutive words, punctuation/accent-insensitive).
    pub blurb_emph: Option<Vec<String>>,
    /// cover title override (else book [title.<lang>])
    pub title: Option<String>,
    /// cover subtitle override (else book [subtitle.<lang>])
    pub sub: Option<String>,
    /// absolute per-element eBook-front layout (`[cover.<lang>.layout]`), written
    /// by the web cover editor. When present, the SVG renderer positions
    /// title/subtitle/author from these absolute coordinates instead of its
    /// default flex stack. Absent => unchanged default layout (no regression).
    pub layout: Option<CoverLayout>,
    /// absolute back-panel layout for the paperback wrap (`[cover.<lang>.wrap]`),
    /// written by the editor's full-wrap mode. Front and wrap are one design: the
    /// front layout above governs both the eBook front and the wrap's front panel;
    /// this governs the wrap's *back* panel (blurb/badge/author). Coordinates are
    /// fractions of the **back panel**. Absent => default flex back (no regression).
    pub wrap: Option<CoverWrapLayout>,
}

/// Absolute back-panel layout for the paperback wrap (`[cover.<lang>.wrap]`).
/// One optional [`CoverElement`] per back-cover text block; coordinates are
/// fractions of the back panel (width = trim + bleed, height = the full wrap).
#[derive(Debug, Deserialize, Default, Clone)]
pub struct CoverWrapLayout {
    pub blurb: Option<CoverElement>,
    pub badge: Option<CoverElement>,
    pub author: Option<CoverElement>,
    /// The spine. Its text runs bottom-to-top, so `wPct` is measured ALONG the text
    /// (a fraction of the wrap's height) while `xPct` places it ACROSS the spine's
    /// width. KDP only prints spine text at 100+ pages; below that it stays blank.
    pub spine: Option<CoverElement>,
}

impl CoverWrapLayout {
    /// Overlay a per-language wrap layout onto the shared one, block by block.
    pub fn merge(base: Option<&Self>, over: Option<&Self>) -> Option<Self> {
        if base.is_none() && over.is_none() {
            return None;
        }
        let (b, o) = (base.cloned().unwrap_or_default(), over.cloned().unwrap_or_default());
        Some(Self {
            blurb: CoverElement::merge(b.blurb.as_ref(), o.blurb.as_ref()),
            badge: CoverElement::merge(b.badge.as_ref(), o.badge.as_ref()),
            author: CoverElement::merge(b.author.as_ref(), o.author.as_ref()),
            spine: CoverElement::merge(b.spine.as_ref(), o.spine.as_ref()),
        })
    }
}

/// Default geometry of the series badge, as fractions of the eBook front canvas
/// (1600×2560): the badge sat at y = 150 + half a 32px/1.2 line box, 32px tall.
/// The wrap's front panel and the editor seed from these same numbers so one badge
/// element means one badge design across every surface.
pub const BADGE_Y_PCT: f64 = (150.0 + 32.0 * 1.2 / 2.0) / 2560.0;
pub const BADGE_W_PCT: f64 = 0.80;
pub const BADGE_FONT_PCT: f64 = 32.0 / 2560.0;
/// Badge tracking, as a multiple of its font size (the old fixed 9px at 32px).
pub const BADGE_TRACKING: f64 = 9.0 / 32.0;
/// Badge line-height, matching the flex badge's line box.
pub const BADGE_LINE_HEIGHT: f64 = 1.2;
pub const BADGE_OPACITY: f64 = 0.95;

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
    /// The series badge ("Serie Isla de la Libertad"). Governs the badge on both
    /// the eBook front and the wrap's front panel; the back panel's badge is its
    /// own block under `[cover.wrap]`.
    pub badge: Option<CoverElement>,
}

impl CoverLayout {
    /// Overlay a per-language front layout onto the shared one, block by block.
    /// `None` on both sides stays `None` (→ the renderer's default flex stack).
    pub fn merge(base: Option<&Self>, over: Option<&Self>) -> Option<Self> {
        if base.is_none() && over.is_none() {
            return None;
        }
        let (b, o) = (base.cloned().unwrap_or_default(), over.cloned().unwrap_or_default());
        Some(Self {
            title: CoverElement::merge(b.title.as_ref(), o.title.as_ref()),
            subtitle: CoverElement::merge(b.subtitle.as_ref(), o.subtitle.as_ref()),
            author: CoverElement::merge(b.author.as_ref(), o.author.as_ref()),
            badge: CoverElement::merge(b.badge.as_ref(), o.badge.as_ref()),
        })
    }
}

/// One absolutely-positioned cover text block. Field names/units match exactly
/// what the web editor persists (`element_inline` in `web/src/cover.rs`):
///   * `xPct`/`yPct` — the block's **center**, as a fraction of canvas width/height.
///   * `wPct` — wrap-box width, as a fraction of canvas **width**.
///   * `fontPct` — font size, as a fraction of canvas **height**.
///   * `fill` / `fontFamily` / `fontStyle` / `text` — style + content overrides.
///
/// **Every field is optional** so a `[cover.<lang>.layout]` block can override a
/// single property of the shared `[cover.layout]` and inherit the rest — that is
/// what keeps the ES and EN covers the same design with only the text (and the
/// occasional size tweak) differing. See [`CoverElement::over`].
#[derive(Debug, Deserialize, Clone, Default)]
pub struct CoverElement {
    #[serde(rename = "xPct")]
    pub x_pct: Option<f64>,
    #[serde(rename = "yPct")]
    pub y_pct: Option<f64>,
    #[serde(rename = "wPct")]
    pub w_pct: Option<f64>,
    #[serde(rename = "fontPct")]
    pub font_pct: Option<f64>,
    pub fill: Option<String>,
    #[serde(rename = "fontFamily")]
    pub font_family: Option<String>,
    #[serde(rename = "fontStyle")]
    pub font_style: Option<String>,
    pub text: Option<CoverText>,
    // ---- optional formatting (web editor). All default to the renderer's prior
    // behavior when absent, so existing layouts render unchanged. ----
    /// horizontal alignment within the block box: "left" | "center" | "right".
    pub align: Option<String>,
    /// line-height multiplier for multi-line blocks (default 1.0).
    #[serde(rename = "lineHeight")]
    pub line_height: Option<f64>,
    /// letter-spacing (tracking) as a **multiple of the block's font size**
    /// (default 0). Scale-free like `fontPct`, so one badge element tracks the
    /// same on the 2560px eBook front and the 888px wrap.
    #[serde(rename = "letterSpacing")]
    pub letter_spacing: Option<f64>,
    /// case transform: "upper" | "lower" | "none" (default none).
    #[serde(rename = "textTransform")]
    pub text_transform: Option<String>,
    /// text outline, CSS-ish "<width>px <color>" (e.g. "2px #000000"); default none.
    pub stroke: Option<String>,
    /// draw the block's drop shadow (default true — prior behavior).
    pub shadow: Option<bool>,
    /// block opacity 0..1 (default 1).
    pub opacity: Option<f64>,
}

impl CoverElement {
    /// Field-by-field overlay: `self` is the shared base, `over` the per-language
    /// override. Any field the override sets wins; everything else is inherited.
    /// This is what "same cover, minor tweaks per language" means in practice.
    pub fn over(&self, over: &CoverElement) -> CoverElement {
        macro_rules! pick {
            ($f:ident) => {
                over.$f.clone().or_else(|| self.$f.clone())
            };
        }
        CoverElement {
            x_pct: pick!(x_pct),
            y_pct: pick!(y_pct),
            w_pct: pick!(w_pct),
            font_pct: pick!(font_pct),
            fill: pick!(fill),
            font_family: pick!(font_family),
            font_style: pick!(font_style),
            text: pick!(text),
            align: pick!(align),
            line_height: pick!(line_height),
            letter_spacing: pick!(letter_spacing),
            text_transform: pick!(text_transform),
            stroke: pick!(stroke),
            shadow: pick!(shadow),
            opacity: pick!(opacity),
        }
    }

    /// Overlay two optional elements (either side may be absent).
    pub fn merge(base: Option<&CoverElement>, over: Option<&CoverElement>) -> Option<CoverElement> {
        match (base, over) {
            (Some(b), Some(o)) => Some(b.over(o)),
            (Some(b), None) => Some(b.clone()),
            (None, Some(o)) => Some(o.clone()),
            (None, None) => None,
        }
    }
}

/// A cover text block's content: either a plain string, or a list of **styled
/// runs** (the per-character styling model — a run is a span of characters that
/// share a style, which is how `canvas-editor`'s per-element styles collapse into
/// something a human can still read and diff in TOML).
///
/// ```toml
/// text = "No hay plata"                                  # plain
/// text = [                                               # styled runs
///   { t = "No hay " },
///   { t = "plata", bold = true, fill = "#D4A937" },
/// ]
/// ```
/// The renderer emits one `<tspan>` per run and measures each with its own face,
/// so wrapping accounts for mixed metrics.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum CoverText {
    Plain(String),
    Runs(Vec<TextRun>),
}

impl CoverText {
    /// The unstyled text, for measuring/fallbacks/`words`-style consumers.
    pub fn plain(&self) -> String {
        match self {
            CoverText::Plain(s) => s.clone(),
            CoverText::Runs(rs) => rs.iter().map(|r| r.t.as_str()).collect(),
        }
    }

    /// Normalize to runs (a plain string is one unstyled run).
    pub fn runs(&self) -> Vec<TextRun> {
        match self {
            CoverText::Plain(s) => vec![TextRun { t: s.clone(), ..Default::default() }],
            CoverText::Runs(rs) => rs.clone(),
        }
    }

    /// True when no run carries a style — lets the renderer keep its simple
    /// single-face path (and the editor keep writing a plain string).
    pub fn is_plain(&self) -> bool {
        match self {
            CoverText::Plain(_) => true,
            CoverText::Runs(rs) => rs.iter().all(|r| !r.styled()),
        }
    }
}

/// One styled run within a cover text block. `t` is the text; every style field is
/// an optional override of the block's own style.
#[derive(Debug, Deserialize, Serialize, Clone, Default, PartialEq)]
#[serde(default)]
pub struct TextRun {
    /// the run's characters
    pub t: String,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    /// per-run color (else the block's fill)
    pub fill: Option<String>,
    /// per-run font family (else the block's family)
    pub family: Option<String>,
    /// per-run size as a **multiple of the block's font size** (1.0 = same).
    pub size: Option<f64>,
}

impl TextRun {
    /// True when this run overrides any of the block's style.
    pub fn styled(&self) -> bool {
        self.bold.is_some()
            || self.italic.is_some()
            || self.fill.is_some()
            || self.family.is_some()
            || self.size.is_some()
    }
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
    /// chapter-opening plate style: "bleed" (default) fills the verso page
    /// edge-to-edge (cover-fit, zero margin); "framed" centers the image within
    /// the page margins with a thin border (no bleed).
    pub plate_style: Option<String>,
    /// framed-plate width as a fraction of the text column (0.0–1.0) — the book's
    /// default plate width. Only used when `plate_style = "framed"`. Default 0.78;
    /// set 1.0 for full-width plates that fill the text column edge-to-edge (still
    /// within margins, no bleed).
    ///
    /// Any single image can override the book defaults with a pandoc attribute
    /// block after its `![alt](src)`. The full set exposed to Typst:
    ///   `{width=55%}` / `{width=3in}`   — length or percent (bare number ⇒ %)
    ///   `{height=2in}`                   — length or percent
    ///   `{fit=cover|contain|stretch}`    — how the image fills its box
    ///   `{align=left|center|right}`      — inline-image placement (default center)
    ///   `{border=false}` or `{.plain}`   — drop the framed-plate keyline border
    ///   `{.spot}`                        — small centered tailpiece (print-only)
    /// e.g. `![…](images/ch03.jpg){width=55% fit=contain .plain}`. width + align
    /// also carry into the EPUB `<img>`; height/fit/border are print-PDF-only.
    pub plate_width: Option<f32>,
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
    /// Also drives the B&W-print split: when resolved to "black", interior images are
    /// rendered grayscale in the KDP/POD **print** PDF (matching what a black-ink
    /// interior actually prints), while the retail digital PDF and the Kindle EPUB
    /// keep them in color. A color ink value keeps the print images in color too.
    pub ink: Option<String>,
    /// per-book cover finish override ("matte"|"glossy"). Falls back to edition /
    /// defaults. Cover/listing only (no interior geometry); reserved, not yet consumed.
    #[allow(dead_code)]
    pub finish: Option<String>,
    /// per-book print margins (inches); falls back to repo [defaults.margins]
    pub margins: Option<Margins>,
    /// show each chapter plate's alt text as a visible caption (PDF: a line under
    /// the framed plate; EPUB: a `<figcaption>`). Default true. Set false to keep
    /// plates caption-free (the alt still ships as EPUB accessibility text). A
    /// per-edition `[editions.<name>].captions` overrides this (e.g. hide on KDP).
    pub plate_captions: Option<bool>,
    /// auto-convert color interior images to grayscale on the fly for the B&W
    /// (black-ink) print PDF. **Default false** — off unless a book opts in.
    /// When off, only images that declare an explicit `{bw=…}` variant become
    /// grayscale in the print interior; everything else stays as-is (KDP still
    /// prints a black-ink interior in grayscale at press time). When true,
    /// bookmill also naively luma-converts any image lacking a `{bw=…}` variant.
    pub auto_grayscale: Option<bool>,
    /// downsample images in the *digital* (retail/gumroad) PDF to this DPI so the
    /// download stays small — e.g. `digital_pdf_dpi = 150` turns a full-res color
    /// picture book from ~190 MB into a few MB. Runs Ghostscript on the retail PDF
    /// only; the KDP/POD print interior keeps its full-res images. An edition may
    /// override via `[editions.<name>].digital_pdf_dpi`. None = off (default).
    pub digital_pdf_dpi: Option<u32>,
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

/// Which cover surface the web editor should expose for a book: `"front"`
/// (digital-only) or `"wrap"` (paperback — full back+spine+front). An explicit
/// `[cover].edit` (book over repo) wins; otherwise it is derived from the book's
/// editions — any **print** edition (kdp-paperback / kdp-hardcover / bubok) means
/// a wrap exists to edit, else the book is digital-only and only the front matters.
pub fn cover_edit_mode(book: &BookConfig, repo: &RepoConfig) -> String {
    if let Some(m) = book
        .cover
        .as_ref()
        .and_then(|c| c.edit.clone())
        .or_else(|| repo.cover.as_ref().and_then(|c| c.edit.clone()))
    {
        let m = m.trim().to_lowercase();
        if m == "front" || m == "wrap" {
            return m;
        }
    }
    let has_print = book.editions.iter().any(|e| {
        repo.editions
            .get(e)
            .and_then(|ed| ed.target.as_deref())
            .map(|t| matches!(t, "kdp-paperback" | "kdp-hardcover" | "bubok"))
            .unwrap_or(false)
    });
    if has_print { "wrap".into() } else { "front".into() }
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

/// Resolve whether chapter-plate captions (from image alt text) are shown for one
/// (edition, book): edition `[editions.<name>].captions` → book `[pdf].plate_captions`
/// → default `true`. Lets a book keep captions while hiding them on a specific
/// edition (e.g. KDP).
pub fn resolve_captions(edition: Option<&Edition>, book: &BookConfig) -> bool {
    edition
        .and_then(|e| e.captions)
        .or(book.pdf.plate_captions)
        .unwrap_or(true)
}

/// Resolve whether bookmill auto-grayscales non-`{bw=…}` interior images for the
/// black-ink print PDF: book `[pdf].auto_grayscale` → repo `[defaults].auto_grayscale`
/// → default `false` (off). Explicit `{bw=…}` variants are always used regardless.
pub fn resolve_auto_grayscale(book: &BookConfig, repo: &RepoConfig) -> bool {
    book.pdf
        .auto_grayscale
        .or(repo.defaults.auto_grayscale)
        .unwrap_or(false)
}

/// Resolve the digital-PDF downsample DPI for one (edition, book):
/// edition `[editions.<name>].digital_pdf_dpi` → book `[pdf].digital_pdf_dpi` → None.
/// `Some(dpi)` means the retail (gumroad) PDF's images are downsampled to `dpi` via
/// Ghostscript (the print interior keeps full-res); `None`/0 = no compression.
pub fn resolve_digital_pdf_dpi(edition: Option<&Edition>, book: &BookConfig) -> Option<u32> {
    edition
        .and_then(|e| e.digital_pdf_dpi)
        .or(book.pdf.digital_pdf_dpi)
        .filter(|&d| d > 0)
}

/// Whether to generate the free sample for a book ([sample].enabled, default true).
pub fn sample_enabled(book: &BookConfig) -> bool {
    book.sample.enabled.unwrap_or(true)
}

/// How many leading content files the free sample includes. Explicit
/// `[sample].chapters` wins (clamped to the book length); otherwise the first
/// ~15% (min 1), capped at half the book so a short book doesn't give itself away.
pub fn sample_count(book: &BookConfig, total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    match book.sample.chapters {
        Some(n) => n.clamp(1, total),
        None => {
            let half = (total / 2).max(1);
            let fifteen = ((total as f64) * 0.15).ceil() as usize;
            fifteen.clamp(1, half)
        }
    }
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

    /// A per-language layout overrides only the fields it sets; everything else
    /// is inherited from the shared `[cover.layout]`. This is what keeps the ES
    /// and EN covers one design.
    #[test]
    fn lang_layout_overlays_shared_field_by_field() {
        let shared: CoverElement = toml::from_str(
            r##"xPct = 0.5
               yPct = 0.14
               fontPct = 0.045
               fill = "#D4A937"
               fontFamily = "Playfair Display""##,
        )
        .unwrap();
        // EN only changes the text and nudges the size down.
        let over: CoverElement =
            toml::from_str("text = \"No Silver\\non Rat Island\"\nfontPct = 0.04").unwrap();
        let m = shared.over(&over);
        assert_eq!(m.font_pct, Some(0.04), "override wins");
        assert_eq!(m.x_pct, Some(0.5), "inherited");
        assert_eq!(m.y_pct, Some(0.14), "inherited");
        assert_eq!(m.fill.as_deref(), Some("#D4A937"), "inherited");
        assert_eq!(m.font_family.as_deref(), Some("Playfair Display"), "inherited");
        assert_eq!(m.text.unwrap().plain(), "No Silver\non Rat Island");
    }

    /// `text` accepts a plain string (every existing book) or styled runs.
    #[test]
    fn cover_text_parses_plain_and_runs() {
        let plain: CoverElement = toml::from_str("text = \"No hay plata\"").unwrap();
        let t = plain.text.unwrap();
        assert!(t.is_plain());
        assert_eq!(t.plain(), "No hay plata");
        assert_eq!(t.runs().len(), 1, "a plain string is one unstyled run");

        let styled: CoverElement = toml::from_str(
            r##"text = [ { t = "No hay " }, { t = "plata", bold = true, fill = "#FFF", size = 1.25 } ]"##,
        )
        .unwrap();
        let t = styled.text.unwrap();
        assert!(!t.is_plain(), "a styled run is not plain");
        assert_eq!(t.plain(), "No hay plata", "plain() is the flat concatenation");
        let rs = t.runs();
        assert_eq!(rs.len(), 2);
        assert!(!rs[0].styled());
        assert!(rs[1].styled());
        assert_eq!(rs[1].bold, Some(true));
        assert_eq!(rs[1].size, Some(1.25));
    }

    /// Runs with no style set are still "plain" — the renderer keeps its simple
    /// single-face path and the editor keeps writing `text = "…"`.
    #[test]
    fn unstyled_runs_count_as_plain() {
        let e: CoverElement =
            toml::from_str(r#"text = [ { t = "No hay " }, { t = "plata" } ]"#).unwrap();
        assert!(e.text.unwrap().is_plain());
    }

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
    fn sample_count_default_and_override() {
        // default heuristic: ~15% (ceil), min 1, capped at half the book
        let bare = book_cfg("slug = 's'\n");
        assert_eq!(sample_count(&bare, 0), 0);
        assert_eq!(sample_count(&bare, 1), 1);
        assert_eq!(sample_count(&bare, 12), 2); // ceil(1.8)=2
        assert_eq!(sample_count(&bare, 3), 1); // ceil(0.45)=1
        assert_eq!(sample_count(&bare, 2), 1); // capped at half
        // explicit override wins, clamped to [1, total]
        let five = book_cfg("slug = 's'\n[sample]\nchapters = 5\n");
        assert_eq!(sample_count(&five, 12), 5);
        assert_eq!(sample_count(&five, 3), 3); // clamped to total
        assert!(sample_enabled(&bare));
        let off = book_cfg("slug = 's'\n[sample]\nenabled = false\n");
        assert!(!sample_enabled(&off));
    }

    #[test]
    fn resolve_digital_pdf_dpi_precedence() {
        let book = book_cfg("slug = 's'\n[pdf]\ndigital_pdf_dpi = 150\n");
        let ed = Edition { digital_pdf_dpi: Some(120), ..Default::default() };
        // edition wins over book [pdf]
        assert_eq!(resolve_digital_pdf_dpi(Some(&ed), &book), Some(120));
        // book [pdf] applies when the edition sets nothing
        let ed_bare = Edition::default();
        assert_eq!(resolve_digital_pdf_dpi(Some(&ed_bare), &book), Some(150));
        assert_eq!(resolve_digital_pdf_dpi(None, &book), Some(150));
        // unset anywhere → None (no compression)
        let bare = book_cfg("slug = 's'\n");
        assert_eq!(resolve_digital_pdf_dpi(None, &bare), None);
        // an explicit 0 is treated as "off"
        let zero = book_cfg("slug = 's'\n[pdf]\ndigital_pdf_dpi = 0\n");
        assert_eq!(resolve_digital_pdf_dpi(None, &zero), None);
    }

    #[test]
    fn cover_edit_mode_derives_from_editions_and_override() {
        let repo = repo_cfg(
            "[editions.kdp-paperback]\ntarget = 'kdp-paperback'\n\
             [editions.kdp-epub]\ntarget = 'kdp-epub'\n\
             [editions.gumroad]\ntarget = 'gumroad'\n",
        );
        // a print edition present → wrap
        let paperback = book_cfg("slug = 's'\neditions = ['kdp-paperback', 'kdp-epub']\n");
        assert_eq!(cover_edit_mode(&paperback, &repo), "wrap");
        // digital-only → front
        let digital = book_cfg("slug = 's'\neditions = ['kdp-epub', 'gumroad']\n");
        assert_eq!(cover_edit_mode(&digital, &repo), "front");
        // explicit [cover].edit overrides the derivation
        let forced = book_cfg("slug = 's'\neditions = ['kdp-paperback']\n[cover]\nedit = 'front'\n");
        assert_eq!(cover_edit_mode(&forced, &repo), "front");
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
