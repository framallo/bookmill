//! Cover config resolution (`resolve` → `Resolved`): layers the `[cover]` TOML
//! (repo defaults + book overrides + `[cover.<lang>]` per-language overrides)
//! into the flat struct the native SVG/resvg renderer (`cover_svg`) consumes.
//! (The former headless-Chrome HTML path was removed; resvg is the only engine.)

use crate::config::{BookConfig, CoverConfig, CoverLang, CoverLayout, CoverWrapLayout, RepoConfig};

// Built-in defaults.
const DEFAULT_AUTHOR: &str = "Federico Ramallo";
const BADGE_ES: &str = "Serie Isla de la Libertad";
const BADGE_EN: &str = "Liberty Island Series";
const TRIM_W: f64 = 6.0;
const TRIM_H: f64 = 9.0;
const BLEED: f64 = 0.125;
const HEAVY_SHADOW: &str = "0 0 5px rgba(0,0,0,0.98),0 0 13px rgba(0,0,0,0.92),0 0 26px rgba(0,0,0,0.72),0 2px 3px rgba(0,0,0,1)";

/// All cover fields resolved for one (book, language): book `[cover]` over repo
/// `[cover]` over built-in defaults, with `[cover.<lang>]` and `[title]`/
/// `[subtitle]`/`[listing]` text mixed in.
pub(crate) struct Resolved {
    pub(crate) author: String,
    pub(crate) badge: String,
    pub(crate) serif: String,
    pub(crate) sub_font: String,
    pub(crate) sub_italic: String,
    pub(crate) title_size: f64,
    pub(crate) title_mt: String,
    pub(crate) author_mt: String,
    pub(crate) bgcolor: String,
    pub(crate) filt: String,
    pub(crate) accent: String,
    pub(crate) title_color: String,
    pub(crate) sub_color: String,
    pub(crate) author_color: String,
    pub(crate) title_shadow: String,
    pub(crate) title_stroke: String,
    pub(crate) sub_shadow: String,
    pub(crate) sub_stroke: String,
    pub(crate) blurb_color: String,
    pub(crate) blurb_shadow: String,
    pub(crate) blurb_stroke: String,
    /// back-cover blurb phrases to emphasize + their style (color/style/family).
    pub(crate) blurb_emph: Vec<String>,
    pub(crate) blurb_emph_color: String,
    pub(crate) blurb_emph_style: String,
    pub(crate) blurb_emph_family: String,
    pub(crate) author_stroke: String,
    pub(crate) badge_color: String,
    pub(crate) badge_stroke: String,
    pub(crate) bg: String,
    pub(crate) wrap_bg: Option<String>,
    pub(crate) wrap_title_px: Option<f64>,
    pub(crate) wrap_lh: f64,
    pub(crate) wrap_stroke: String,
    pub(crate) wrap_shadow: Option<String>,
    pub(crate) paper_mult: f64,
    pub(crate) trim_w: f64,
    pub(crate) trim_h: f64,
    pub(crate) bleed: f64,
    // text
    pub(crate) title: String,
    pub(crate) sub: String,
    pub(crate) blurb: String,
    /// absolute eBook-front layout from `[cover.<lang>.layout]` (web editor),
    /// honored by the SVG front renderer only. `None` => default flex layout.
    pub(crate) layout: Option<CoverLayout>,
    /// absolute back-panel layout from `[cover.<lang>.wrap]` (web editor's wrap
    /// mode), honored by the SVG wrap renderer's BACK panel only. Coordinates are
    /// fractions of the back panel. `None` => default flex back stack.
    pub(crate) wrap_layout: Option<CoverWrapLayout>,
}

/// First-set lookup: book field, then repo field, then `default`.
fn pick(
    book: Option<&CoverConfig>,
    repo: Option<&CoverConfig>,
    get: impl Fn(&CoverConfig) -> Option<String>,
    default: &str,
) -> String {
    book.and_then(&get)
        .or_else(|| repo.and_then(&get))
        .unwrap_or_else(|| default.to_string())
}

fn pick_opt(
    book: Option<&CoverConfig>,
    repo: Option<&CoverConfig>,
    get: impl Fn(&CoverConfig) -> Option<String>,
) -> Option<String> {
    book.and_then(&get).or_else(|| repo.and_then(&get))
}

fn pick_f(
    book: Option<&CoverConfig>,
    repo: Option<&CoverConfig>,
    get: impl Fn(&CoverConfig) -> Option<f64>,
    default: f64,
) -> f64 {
    book.and_then(&get)
        .or_else(|| repo.and_then(&get))
        .unwrap_or(default)
}

fn pick_opt_f(
    book: Option<&CoverConfig>,
    repo: Option<&CoverConfig>,
    get: impl Fn(&CoverConfig) -> Option<f64>,
) -> Option<f64> {
    book.and_then(&get).or_else(|| repo.and_then(&get))
}

/// Split a title onto balanced lines exactly like `two_line`, but return the raw
/// (unescaped) line strings — used by the SVG renderer, which escapes for XML
/// itself and needs the lines separately (SVG has no `<br>`).
pub(crate) fn two_line_parts(s: &str) -> Vec<String> {
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.len() < 2 {
        return vec![s.to_string()];
    }
    let half = s.chars().count() as f64 / 2.0;
    let mut best_i = 1usize;
    let mut best_d: Option<f64> = None;
    let mut cur = 0f64;
    for i in 1..words.len() {
        cur += words[i - 1].chars().count() as f64 + 1.0;
        let d = (cur - half).abs();
        if best_d.is_none() || d < best_d.unwrap() {
            best_d = Some(d);
            best_i = i;
        }
    }
    vec![words[..best_i].join(" "), words[best_i..].join(" ")]
}

pub(crate) fn resolve(repo: &RepoConfig, book: &BookConfig, lang: &str) -> Resolved {
    let bc = book.cover.as_ref();
    let rc = repo.cover.as_ref();
    let ml: Option<&CoverLang> = bc.and_then(|c| c.lang.get(lang));

    let accent = pick(bc, rc, |c| c.accent.clone(), "#000000");
    let sub_color = pick(bc, rc, |c| c.sub_color.clone(), "#FFFFFF");
    // `subfont` presence flips the subtitle to upright (non-italic), as in make-covers.py.
    let has_subfont = bc.and_then(|c| c.subfont.clone()).is_some()
        || rc.and_then(|c| c.subfont.clone()).is_some();
    let sub_font = pick(bc, rc, |c| c.subfont.clone(), "Montserrat");

    // Heavy title halo: book/repo `heavy_shadow` override, else the built-in.
    let heavy = pick_opt(bc, rc, |c| c.heavy_shadow.clone()).unwrap_or_else(|| HEAVY_SHADOW.to_string());

    let author = repo.author.clone().unwrap_or_else(|| DEFAULT_AUTHOR.to_string());
    let badge = match lang {
        "es" => pick(bc, rc, |c| c.badge_es.clone(), BADGE_ES),
        _ => pick(bc, rc, |c| c.badge_en.clone(), BADGE_EN),
    };

    // text: per-lang [cover.<lang>] override, then [title]/[subtitle]/[listing].
    let title = ml
        .and_then(|m| m.title.clone())
        .or_else(|| book.title.get(lang).cloned())
        .unwrap_or_default();
    let sub = ml
        .and_then(|m| m.sub.clone())
        .or_else(|| book.subtitle.get(lang).cloned())
        .unwrap_or_default();
    let blurb = ml
        .and_then(|m| m.blurb.clone())
        .or_else(|| book.listing.get(lang).and_then(|l| l.blurb.clone()))
        .or_else(|| bc.and_then(|c| c.blurb.clone()))
        .unwrap_or_default()
        .trim()
        .to_string();

    // front title size: per-lang override, then book/repo title_size.
    let base_title_size = pick_f(bc, rc, |c| c.title_size, 120.0);
    let title_size = ml.and_then(|m| m.title_size).unwrap_or(base_title_size);

    // shared locals so the blurb-emphasis defaults can borrow them.
    let serif = pick(bc, rc, |c| c.serif.clone(), "Playfair Display");
    let title_color = pick(bc, rc, |c| c.title_color.clone(), "#FFFFFF");
    // Back-cover blurb emphasis: phrases per language; color/style/family default
    // to the title color (gold), italic, and the blurb serif.
    let blurb_emph = ml.and_then(|m| m.blurb_emph.clone()).unwrap_or_default();
    let blurb_emph_color = pick(bc, rc, |c| c.blurb_emph_color.clone(), &title_color);
    let blurb_emph_style = pick(bc, rc, |c| c.blurb_emph_style.clone(), "italic");
    let blurb_emph_family = pick(bc, rc, |c| c.blurb_emph_family.clone(), &serif);

    Resolved {
        author,
        badge,
        serif: serif.clone(),
        sub_italic: if has_subfont { "normal".into() } else { "italic".into() },
        sub_font,
        title_size,
        title_mt: pick(bc, rc, |c| c.title_mt.clone(), "auto"),
        author_mt: pick(bc, rc, |c| c.author_mt.clone(), "84px"),
        bgcolor: pick(bc, rc, |c| c.bgcolor.clone(), "#000000"),
        filt: pick(bc, rc, |c| c.filt.clone(), "none"),
        accent: accent.clone(),
        title_color: title_color.clone(),
        sub_color: sub_color.clone(),
        author_color: pick(bc, rc, |c| c.author_color.clone(), "#FFFFFF"),
        title_shadow: pick(bc, rc, |c| c.title_shadow.clone(), &heavy),
        title_stroke: pick(bc, rc, |c| c.title_stroke.clone(), "0px transparent"),
        sub_shadow: pick(bc, rc, |c| c.sub_shadow.clone(), "0 1px 6px rgba(0,0,0,0.5)"),
        sub_stroke: pick(bc, rc, |c| c.sub_stroke.clone(), "0px transparent"),
        blurb_color: pick_opt(bc, rc, |c| c.blurb_color.clone()).unwrap_or(sub_color),
        blurb_shadow: pick(bc, rc, |c| c.blurb_shadow.clone(), "none"),
        blurb_stroke: pick(bc, rc, |c| c.blurb_stroke.clone(), "0px transparent"),
        blurb_emph,
        blurb_emph_color,
        blurb_emph_style,
        blurb_emph_family,
        author_stroke: pick(bc, rc, |c| c.author_stroke.clone(), "0px transparent"),
        badge_color: pick(bc, rc, |c| c.badge_color.clone(), &accent),
        badge_stroke: pick(bc, rc, |c| c.badge_stroke.clone(), "0px transparent"),
        bg: pick(bc, rc, |c| c.bg.clone(), "bg.jpg"),
        wrap_bg: pick_opt(bc, rc, |c| c.wrap_bg.clone()),
        wrap_title_px: bc.and_then(|c| c.wrap_title_px).or_else(|| rc.and_then(|c| c.wrap_title_px)),
        wrap_lh: pick_f(bc, rc, |c| c.wrap_lh, 1.05),
        wrap_stroke: pick(bc, rc, |c| c.wrap_stroke.clone(), "0px transparent"),
        wrap_shadow: pick_opt(bc, rc, |c| c.wrap_shadow.clone()),
        // Spine width per page: an explicit `[cover].paper_mult` (book over repo)
        // wins for byte-identical fidelity with shipped covers; otherwise it is
        // derived from the resolved paper+ink stock (KDP's spine calculator) so a
        // cream-paper book gets 0.0025 instead of the white-stock 0.002252. Covers
        // are not edition-specific, so resolve paper/ink at the book level.
        paper_mult: pick_opt_f(bc, rc, |c| c.paper_mult).unwrap_or_else(|| {
            crate::config::spine_mult(
                &crate::config::resolve_paper(None, book, repo),
                &crate::config::resolve_ink(None, book, repo),
            )
        }),
        trim_w: pick_f(bc, rc, |c| c.trim_w, TRIM_W),
        trim_h: pick_f(bc, rc, |c| c.trim_h, TRIM_H),
        bleed: pick_f(bc, rc, |c| c.bleed, BLEED),
        title,
        sub,
        blurb,
        layout: ml.and_then(|m| m.layout.clone()),
        wrap_layout: ml.and_then(|m| m.wrap.clone()),
    }
}
