//! Native cover HTML rendering from `[cover]` config + the bundled templates
//! (`templates/cover/{front,wrap}.html.tmpl`). This replaces the per-book `CFG`
//! dict in `scripts/make-covers.py`: it reproduces that script's emitted HTML
//! byte-for-byte so the existing headless-Chrome rasterizer yields identical
//! covers. Design lives in TOML now (repo `[cover]` defaults + book `[cover]`
//! overrides + `[cover.<lang>]` per-language overrides).

use crate::config::{BookConfig, CoverConfig, CoverLang, RepoConfig};

const FRONT_TMPL: &str = include_str!("../templates/cover/front.html.tmpl");
const WRAP_TMPL: &str = include_str!("../templates/cover/wrap.html.tmpl");

// Built-in defaults, mirroring the module constants in make-covers.py.
const DEFAULT_AUTHOR: &str = "Federico Ramallo";
const BADGE_ES: &str = "Serie Isla de la Libertad";
const BADGE_EN: &str = "Liberty Island Series";
const PAPER_MULT: f64 = 0.002252;
const TRIM_W: f64 = 6.0;
const TRIM_H: f64 = 9.0;
const BLEED: f64 = 0.125;
const FONT_LINK: &str = "https://fonts.googleapis.com/css2?family=Playfair+Display:ital,wght@0,700;0,800;0,900;1,500&family=Oswald:wght@500;600;700&family=Baloo+2:wght@600;700;800&family=Patrick+Hand&family=Quicksand:wght@500;600;700&family=Montserrat:ital,wght@0,400;0,500;0,600;1,400&display=swap";
const HEAVY_SHADOW: &str = "0 0 5px rgba(0,0,0,0.98),0 0 13px rgba(0,0,0,0.92),0 0 26px rgba(0,0,0,0.72),0 2px 3px rgba(0,0,0,1)";

/// HTML-escape, matching Python's `html.escape(s, quote=True)`.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Force a title onto two balanced lines, split at the space nearest the middle.
/// Mirrors make-covers.py `two_line`. Single-word titles stay on one line.
fn two_line(s: &str) -> String {
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.len() < 2 {
        return esc(s);
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
    let first = words[..best_i].join(" ");
    let second = words[best_i..].join(" ");
    format!("{}<br>{}", esc(&first), esc(&second))
}

/// Round to 4 decimals (matches Python `round(x, 4)` for the non-tie values here).
fn round4(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
}

/// Format a number like Python `str(round(x, 4))`: up to 4 decimals, no trailing
/// zeros, no trailing dot.
fn fmt_num(x: f64) -> String {
    let s = format!("{:.4}", round4(x));
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

/// All cover fields resolved for one (book, language): book `[cover]` over repo
/// `[cover]` over built-in defaults, with `[cover.<lang>]` and `[title]`/
/// `[subtitle]`/`[listing]` text mixed in.
pub(crate) struct Resolved {
    pub(crate) author: String,
    pub(crate) badge: String,
    pub(crate) font_link: String,
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

    Resolved {
        author,
        badge,
        font_link: pick(bc, rc, |c| c.font_link.clone(), FONT_LINK),
        serif: pick(bc, rc, |c| c.serif.clone(), "Playfair Display"),
        sub_italic: if has_subfont { "normal".into() } else { "italic".into() },
        sub_font,
        title_size,
        title_mt: pick(bc, rc, |c| c.title_mt.clone(), "auto"),
        author_mt: pick(bc, rc, |c| c.author_mt.clone(), "84px"),
        bgcolor: pick(bc, rc, |c| c.bgcolor.clone(), "#000000"),
        filt: pick(bc, rc, |c| c.filt.clone(), "none"),
        accent: accent.clone(),
        title_color: pick(bc, rc, |c| c.title_color.clone(), "#FFFFFF"),
        sub_color: sub_color.clone(),
        author_color: pick(bc, rc, |c| c.author_color.clone(), "#FFFFFF"),
        title_shadow: pick(bc, rc, |c| c.title_shadow.clone(), &heavy),
        title_stroke: pick(bc, rc, |c| c.title_stroke.clone(), "0px transparent"),
        sub_shadow: pick(bc, rc, |c| c.sub_shadow.clone(), "0 1px 6px rgba(0,0,0,0.5)"),
        sub_stroke: pick(bc, rc, |c| c.sub_stroke.clone(), "0px transparent"),
        blurb_color: pick_opt(bc, rc, |c| c.blurb_color.clone()).unwrap_or(sub_color),
        blurb_shadow: pick(bc, rc, |c| c.blurb_shadow.clone(), "none"),
        blurb_stroke: pick(bc, rc, |c| c.blurb_stroke.clone(), "0px transparent"),
        author_stroke: pick(bc, rc, |c| c.author_stroke.clone(), "0px transparent"),
        badge_color: pick(bc, rc, |c| c.badge_color.clone(), &accent),
        badge_stroke: pick(bc, rc, |c| c.badge_stroke.clone(), "0px transparent"),
        bg: pick(bc, rc, |c| c.bg.clone(), "bg.jpg"),
        wrap_bg: pick_opt(bc, rc, |c| c.wrap_bg.clone()),
        wrap_title_px: bc.and_then(|c| c.wrap_title_px).or_else(|| rc.and_then(|c| c.wrap_title_px)),
        wrap_lh: pick_f(bc, rc, |c| c.wrap_lh, 1.05),
        wrap_stroke: pick(bc, rc, |c| c.wrap_stroke.clone(), "0px transparent"),
        wrap_shadow: pick_opt(bc, rc, |c| c.wrap_shadow.clone()),
        paper_mult: pick_f(bc, rc, |c| c.paper_mult, PAPER_MULT),
        trim_w: pick_f(bc, rc, |c| c.trim_w, TRIM_W),
        trim_h: pick_f(bc, rc, |c| c.trim_h, TRIM_H),
        bleed: pick_f(bc, rc, |c| c.bleed, BLEED),
        title,
        sub,
        blurb,
    }
}

/// Render the eBook front cover HTML (1600x2560).
pub fn front_html(repo: &RepoConfig, book: &BookConfig, lang: &str) -> String {
    let r = resolve(repo, book, lang);
    let (fbg_src, fbg_pos) = match &r.wrap_bg {
        Some(w) => (w.clone(), "right"),
        None => (r.bg.clone(), "center"),
    };
    let mut s = FRONT_TMPL.to_string();
    let repls: &[(&str, &str)] = &[
        ("{{FONT_LINK}}", &r.font_link),
        ("{{BGCOLOR}}", &r.bgcolor),
        ("{{FBG_POS}}", fbg_pos),
        ("{{FILT}}", &r.filt),
        ("{{ACCENT}}", &r.accent),
        ("{{BADGE_COLOR}}", &r.badge_color),
        ("{{BADGE_STROKE}}", &r.badge_stroke),
        ("{{TITLE_MT}}", &r.title_mt),
        ("{{SERIF}}", &r.serif),
        ("{{TITLE_SIZE}}", &fmt_num(r.title_size)),
        ("{{TITLE_COLOR}}", &r.title_color),
        ("{{TITLE_SHADOW}}", &r.title_shadow),
        ("{{TITLE_STROKE}}", &r.title_stroke),
        ("{{SUB_FONT}}", &r.sub_font),
        ("{{SUB_ITALIC}}", &r.sub_italic),
        ("{{SUB_COLOR}}", &r.sub_color),
        ("{{SUB_SHADOW}}", &r.sub_shadow),
        ("{{SUB_STROKE}}", &r.sub_stroke),
        ("{{AUTHOR_MT}}", &r.author_mt),
        ("{{AUTHOR_COLOR}}", &r.author_color),
        ("{{AUTHOR_STROKE}}", &r.author_stroke),
        ("{{FBG_SRC}}", &fbg_src),
        ("{{BADGE}}", &esc(&r.badge)),
        ("{{TITLE_HTML}}", &two_line(&r.title)),
        ("{{SUB}}", &esc(&r.sub)),
        ("{{AUTHOR}}", &esc(&r.author)),
    ];
    for (k, v) in repls {
        s = s.replace(k, v);
    }
    s
}

/// Render the paperback wrap cover HTML. `pages` drives the spine width.
pub fn wrap_html(repo: &RepoConfig, book: &BookConfig, lang: &str, pages: u32) -> String {
    let r = resolve(repo, book, lang);

    let ft_px = match r.wrap_title_px {
        Some(p) => p,
        None => (r.title_size * 0.30).round(),
    };
    let ft_shadow = r.wrap_shadow.clone().unwrap_or_else(|| r.title_shadow.clone());

    let spine = round4(pages as f64 * r.paper_mult);
    let full_w = round4(2.0 * r.trim_w + spine + 2.0 * r.bleed);
    let full_h = round4(r.trim_h + 2.0 * r.bleed);
    let back_pct = round4((r.bleed + r.trim_w) / full_w * 100.0);
    let spine_pct = round4(spine / full_w * 100.0);
    let front_pct = round4((r.trim_w + r.bleed) / full_w * 100.0);

    let spine_inner = if pages >= 79 {
        format!(
            "<div class=\"spinetxt\"><span>{}&nbsp;&nbsp;&middot;&nbsp;&nbsp;{}</span></div>",
            esc(&r.title),
            esc(&r.author)
        )
    } else {
        String::new()
    };

    let (wrapbg_img, back_bg, front_bg) = match &r.wrap_bg {
        Some(w) => (
            format!("<img class=\"wrapbg\" src=\"{w}\">"),
            "transparent".to_string(),
            String::new(),
        ),
        None => (
            String::new(),
            r.bgcolor.clone(),
            format!("<img class=\"bg\" src=\"{}\">", r.bg),
        ),
    };

    let mut s = WRAP_TMPL.to_string();
    let repls: &[(&str, &str)] = &[
        ("{{FONT_LINK}}", &r.font_link),
        ("{{FULL_W}}", &fmt_num(full_w)),
        ("{{FULL_H}}", &fmt_num(full_h)),
        ("{{BGCOLOR}}", &r.bgcolor),
        ("{{FILT}}", &r.filt),
        ("{{BACK_PCT}}", &fmt_num(back_pct)),
        ("{{SPINE_PCT}}", &fmt_num(spine_pct)),
        ("{{FRONT_PCT}}", &fmt_num(front_pct)),
        ("{{BADGE_COLOR}}", &r.badge_color),
        ("{{BADGE_STROKE}}", &r.badge_stroke),
        ("{{SERIF}}", &r.serif),
        ("{{FT_PX}}", &fmt_num(ft_px)),
        ("{{FT_LH}}", &fmt_num(r.wrap_lh)),
        ("{{TITLE_COLOR}}", &r.title_color),
        ("{{FT_SHADOW}}", &ft_shadow),
        ("{{FT_STROKE}}", &r.wrap_stroke),
        ("{{ACCENT}}", &r.accent),
        ("{{SUB_FONT}}", &r.sub_font),
        ("{{SUB_ITALIC}}", &r.sub_italic),
        ("{{SUB_COLOR}}", &r.sub_color),
        ("{{SUB_SHADOW}}", &r.sub_shadow),
        ("{{SUB_STROKE}}", &r.sub_stroke),
        ("{{AUTHOR_COLOR}}", &r.author_color),
        ("{{AUTHOR_STROKE}}", &r.author_stroke),
        ("{{BACK_BG}}", &back_bg),
        ("{{BLURB_COLOR}}", &r.blurb_color),
        ("{{BLURB_SHADOW}}", &r.blurb_shadow),
        ("{{BLURB_STROKE}}", &r.blurb_stroke),
        ("{{WRAPBG_IMG}}", &wrapbg_img),
        ("{{BACK_SCRIM}}", ""),
        ("{{SPINE_INNER}}", &spine_inner),
        ("{{FRONT_BG}}", &front_bg),
        ("{{BADGE}}", &esc(&r.badge)),
        ("{{BLURB}}", &esc(&r.blurb)),
        ("{{AUTHOR}}", &esc(&r.author)),
        ("{{TITLE_HTML}}", &two_line(&r.title)),
        ("{{SUB}}", &esc(&r.sub)),
    ];
    for (k, v) in repls {
        s = s.replace(k, v);
    }
    s
}
