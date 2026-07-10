//! Native, Chrome-free cover rasterization via **resvg/usvg** (pure Rust).
//!
//! Mirrors the design produced by `cover_tmpl` (the HTML/Chrome path) but emits
//! SVG instead of HTML, then renders the front PNG with `resvg` and the wrap PDF
//! with `svg2pdf`. SVG has no flexbox / auto text wrapping, so this module lays
//! the text out by hand from font metrics (ttf-parser) to approximate the CSS
//! flex layout. A renderer swap is never byte-identical with Chrome — we aim for
//! visual equivalence.
//!
//! Fonts are `include_bytes!`'d from `templates/cover/fonts/` (see that dir's
//! README) so covers render with no system fonts and no network.

use crate::config::{BookConfig, CoverElement, RepoConfig};
use crate::cover_tmpl::{resolve, two_line_parts, Resolved};
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::sync::Arc;

const FRONT_SVG_TMPL: &str = include_str!("../templates/cover/front.svg.tmpl");
const WRAP_SVG_TMPL: &str = include_str!("../templates/cover/wrap.svg.tmpl");

// ---------------------------------------------------------------------------
// Bundled fonts (see templates/cover/fonts/README.md for provenance/licensing).
// ---------------------------------------------------------------------------
struct BFont {
    family: &'static str,
    weight: u16,
    italic: bool,
    data: &'static [u8],
}

macro_rules! font {
    ($fam:literal, $w:literal, $it:literal, $file:literal) => {
        BFont {
            family: $fam,
            weight: $w,
            italic: $it,
            data: include_bytes!(concat!("../templates/cover/fonts/", $file)),
        }
    };
}

const FONTS: &[BFont] = &[
    font!("Montserrat", 400, false, "montserrat-400-normal.ttf"),
    font!("Montserrat", 500, false, "montserrat-500-normal.ttf"),
    font!("Montserrat", 600, false, "montserrat-600-normal.ttf"),
    font!("Montserrat", 400, true, "montserrat-400-italic.ttf"),
    font!("Montserrat", 500, true, "montserrat-500-italic.ttf"),
    font!("Playfair Display", 400, false, "playfair-display-400-normal.ttf"),
    font!("Playfair Display", 700, false, "playfair-display-700-normal.ttf"),
    font!("Playfair Display", 800, false, "playfair-display-800-normal.ttf"),
    font!("Baloo 2", 600, false, "baloo-2-600-normal.ttf"),
    font!("Baloo 2", 700, false, "baloo-2-700-normal.ttf"),
    font!("Baloo 2", 800, false, "baloo-2-800-normal.ttf"),
    font!("Oswald", 500, false, "oswald-500-normal.ttf"),
    font!("Oswald", 600, false, "oswald-600-normal.ttf"),
    font!("Oswald", 700, false, "oswald-700-normal.ttf"),
    font!("Patrick Hand", 400, false, "patrick-hand-400-normal.ttf"),
];

/// Holds the shared font database (for usvg) plus the raw font bytes (for our own
/// metric/measurement queries via ttf-parser).
pub struct CoverRenderer {
    db: Arc<usvg::fontdb::Database>,
    /// Per-FONTS-entry registered family name (e.g. "Baloo 2 ExtraBold"), parsed
    /// from each file's name table. Fontsource ships each weight as its own
    /// family, so to select a specific weight we must name that exact family
    /// rather than rely on usvg's family+weight matching.
    names: Vec<String>,
}

impl CoverRenderer {
    pub fn new() -> Self {
        let mut db = usvg::fontdb::Database::new();
        let mut names = Vec::with_capacity(FONTS.len());
        for f in FONTS {
            db.load_font_data(f.data.to_vec());
            names.push(family_name(f.data).unwrap_or_else(|| f.family.to_string()));
        }
        CoverRenderer { db: Arc::new(db), names }
    }

    fn usvg_options(&self) -> usvg::Options<'_> {
        let mut opt = usvg::Options::default();
        opt.fontdb = self.db.clone();
        opt
    }

    /// Render the eBook front cover to a 1600x2560 PNG.
    pub fn render_front_png(
        &self,
        repo: &RepoConfig,
        book: &BookConfig,
        lang: &str,
        cover_dir: &Path,
        out_png: &Path,
    ) -> Result<()> {
        let r = resolve(repo, book, lang);
        let svg = self.front_svg(&r, cover_dir)?;
        if std::env::var("BOOKMILL_DUMP_SVG").is_ok() {
            let _ = std::fs::write(out_png.with_extension("svg"), &svg);
        }
        let tree = usvg::Tree::from_str(&svg, &self.usvg_options())
            .context("usvg parse (front)")?;
        let size = tree.size().to_int_size();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
            .context("alloc pixmap")?;
        resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
        pixmap.save_png(out_png).context("save front png")?;
        Ok(())
    }

    /// The eBook front cover as an SVG string (the exact source `render_front_png`
    /// rasterizes). Used by the web editor so its canvas is the authoritative
    /// render, eliminating editor/output drift. (Consumed via the library surface
    /// `bookmill::editor_cover_svg`, so it reads as dead code inside the binary.)
    #[allow(dead_code)]
    pub fn front_svg_string(
        &self,
        repo: &RepoConfig,
        book: &BookConfig,
        lang: &str,
        cover_dir: &Path,
    ) -> Result<String> {
        let r = resolve(repo, book, lang);
        self.front_svg(&r, cover_dir)
    }

    /// The full paperback wrap (back + spine + front) as an SVG string — the exact
    /// source `render_wrap_pdf` rasterizes. Powers the web editor's full-wrap canvas.
    /// (Consumed via `bookmill::editor_cover_svg`, so it reads as dead code inside
    /// the binary.)
    #[allow(dead_code)]
    pub fn wrap_svg_string(
        &self,
        repo: &RepoConfig,
        book: &BookConfig,
        lang: &str,
        cover_dir: &Path,
        pages: u32,
    ) -> Result<String> {
        let r = resolve(repo, book, lang);
        self.wrap_svg(&r, cover_dir, pages)
    }

    /// Render the paperback wrap to a print-sized PDF (full_w x full_h inches).
    pub fn render_wrap_pdf(
        &self,
        repo: &RepoConfig,
        book: &BookConfig,
        lang: &str,
        cover_dir: &Path,
        pages: u32,
        out_pdf: &Path,
    ) -> Result<()> {
        let r = resolve(repo, book, lang);
        let svg = self.wrap_svg(&r, cover_dir, pages)?;
        let tree = usvg::Tree::from_str(&svg, &self.usvg_options())
            .context("usvg parse (wrap)")?;
        // The wrap SVG is authored at 96 user-units per inch; svg2pdf at dpi=96
        // maps user-units -> points so the PDF page is exactly full_w x full_h in.
        let conv = svg2pdf::ConversionOptions {
            embed_text: false, // outline text -> strokes/paint-order render faithfully
            ..Default::default()
        };
        let page = svg2pdf::PageOptions { dpi: 96.0 };
        let pdf = svg2pdf::to_pdf(&tree, conv, page)
            .map_err(|e| anyhow::anyhow!("svg2pdf (wrap): {e}"))?;
        std::fs::write(out_pdf, pdf).context("write wrap pdf")?;
        Ok(())
    }

    // -- metrics -----------------------------------------------------------

    /// Index into `FONTS` of the bundled face best matching family/weight/italic.
    fn pick_index(&self, family: &str, weight: u16, italic: bool) -> usize {
        let mut best: Option<(usize, i32)> = None;
        for (i, f) in FONTS.iter().enumerate() {
            if !f.family.eq_ignore_ascii_case(family) {
                continue;
            }
            let mut cost = (f.weight as i32 - weight as i32).abs();
            if f.italic != italic {
                cost += 1000;
            }
            if best.map_or(true, |(_, c)| cost < c) {
                best = Some((i, cost));
            }
        }
        best.map(|(i, _)| i).unwrap_or(0) // fall back to Montserrat 400
    }

    fn pick_face(&self, family: &str, weight: u16, italic: bool) -> &'static [u8] {
        FONTS[self.pick_index(family, weight, italic)].data
    }

    /// The exact registered family name to put in the SVG for a logical
    /// family+weight+italic (e.g. "Baloo 2" weight 800 -> "Baloo 2 ExtraBold").
    fn svg_family(&self, family: &str, weight: u16, italic: bool) -> &str {
        &self.names[self.pick_index(family, weight, italic)]
    }

    /// Emit a single `<text>` line. Resolves `s.family` (a logical family) to the
    /// exact bundled face's registered name so the requested weight is honored.
    fn emit_line(&self, out: &mut String, content: &str, x: f64, baseline_y: f64, s: &TextStyle) {
        let fam = self.svg_family(s.family, s.weight, s.italic);
        // Quote the family: usvg parses font-family as CSS, and an unquoted name
        // containing a numeric token (e.g. "Baloo 2") is invalid and silently
        // falls back to Times New Roman (then drops the text). Single quotes fix
        // it. The registered name encodes the weight, so no font-weight needed.
        let mut attrs = format!(
            "x=\"{}\" y=\"{}\" text-anchor=\"{}\" font-family=\"'{}'\" font-size=\"{}\"",
            fmt(x),
            fmt(baseline_y),
            s.anchor,
            xml_attr(fam),
            fmt(s.size)
        );
        if s.italic {
            attrs.push_str(" font-style=\"italic\"");
        }
        if s.letter_spacing != 0.0 {
            attrs.push_str(&format!(" letter-spacing=\"{}\"", fmt(s.letter_spacing)));
        }
        // Colors/families are author- and web-editor-controlled; escape them so a
        // crafted value can't break out of the SVG attribute (L4).
        attrs.push_str(&format!(" fill=\"{}\"", xml_attr(s.fill)));
        if s.opacity < 1.0 {
            attrs.push_str(&format!(" opacity=\"{}\"", fmt(s.opacity)));
        }
        if let Some((w, c)) = parse_stroke(s.stroke) {
            attrs.push_str(&format!(
                " stroke=\"{}\" stroke-width=\"{}\" paint-order=\"stroke\" stroke-linejoin=\"round\"",
                xml_attr(&c),
                fmt(w)
            ));
        }
        if !s.shadow_id.is_empty() {
            attrs.push_str(&format!(" filter=\"url(#{})\"", s.shadow_id));
        }
        out.push_str(&format!("<text {attrs}>{content}</text>"));
    }

    /// Normalized (ascender, descender-as-positive) for a face, fraction of em.
    /// The bundled fonts are `include_bytes!`'d and always parse; if a future font
    /// swap fails, degrade to typical metrics rather than panicking (L3).
    fn vmetrics(&self, family: &str, weight: u16, italic: bool) -> (f64, f64) {
        let data = self.pick_face(family, weight, italic);
        let Ok(face) = ttf_parser::Face::parse(data, 0) else {
            return (0.8, 0.2);
        };
        let upm = face.units_per_em() as f64;
        let asc = face.ascender() as f64 / upm;
        let desc = -(face.descender() as f64) / upm;
        (asc, desc)
    }

    /// Width of `text` at `size` px (sum of advances; no letter-spacing).
    /// Falls back to a 0.5-em-per-char estimate if the face can't be parsed (L3).
    fn text_width(&self, text: &str, family: &str, weight: u16, italic: bool, size: f64) -> f64 {
        let data = self.pick_face(family, weight, italic);
        let Ok(face) = ttf_parser::Face::parse(data, 0) else {
            return text.chars().count() as f64 * 0.5 * size;
        };
        let upm = face.units_per_em() as f64;
        let mut w = 0.0;
        for ch in text.chars() {
            let adv = face
                .glyph_index(ch)
                .and_then(|g| face.glyph_hor_advance(g))
                .unwrap_or((upm * 0.5) as u16) as f64;
            w += adv / upm * size;
        }
        w
    }

    /// Greedy word-wrap to `max_w` px.
    fn wrap(
        &self,
        text: &str,
        family: &str,
        weight: u16,
        italic: bool,
        size: f64,
        max_w: f64,
    ) -> Vec<String> {
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.is_empty() {
            return vec![String::new()];
        }
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        for w in words {
            let trial = if cur.is_empty() {
                w.to_string()
            } else {
                format!("{cur} {w}")
            };
            if self.text_width(&trial, family, weight, italic, size) <= max_w || cur.is_empty() {
                cur = trial;
            } else {
                lines.push(std::mem::take(&mut cur));
                cur = w.to_string();
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        lines
    }

    // -- SVG assembly ------------------------------------------------------

    fn front_svg(&self, r: &Resolved, cover_dir: &Path) -> Result<String> {
        const W: f64 = 1600.0;
        const H: f64 = 2560.0;
        let pad_x = 120.0;
        let top = 150.0;
        let bottom = H - 150.0;
        let cx = W / 2.0;
        let content_w = W - 2.0 * pad_x;

        // background image (cover-fit). When a wraparound photo exists the front
        // is its right slice; else bg.jpg centered.
        let (bg_file, pos_right) = match &r.wrap_bg {
            Some(w) => (w.clone(), true),
            None => (r.bg.clone(), false),
        };
        let mut defs = String::new();
        let mut image = String::new();
        let filt_id = filter_def(&mut defs, &r.filt, "imgfilt");
        let par = if pos_right { "xMaxYMid slice" } else { "xMidYMid slice" };
        let href = data_uri(&cover_dir.join(&bg_file))
            .with_context(|| format!("embedding {bg_file}"))?;
        image.push_str(&format!(
            "<image x=\"0\" y=\"0\" width=\"{W}\" height=\"{H}\" preserveAspectRatio=\"{par}\" xlink:href=\"{href}\"{}/>",
            attr_filter(&filt_id)
        ));

        // Web-editor absolute layout: when [cover.<lang>.layout] is present, place
        // title/subtitle/author from its saved canvas fractions instead of the flex
        // stack below. The eBook front is the only edited surface, so this branch is
        // front-only; the wrap keeps its own math. Absent => unchanged flex layout.
        let mut text = String::new();
        if r.layout.as_ref().map_or(false, |l| {
            l.title.is_some() || l.subtitle.is_some() || l.author.is_some()
        }) {
            self.front_absolute(&mut text, &mut defs, r, W, H, top, cx);
            return Ok(FRONT_SVG_TMPL
                .replace("{{DEFS}}", &defs)
                .replace("{{BGCOLOR}}", &xml_attr(&r.bgcolor))
                .replace("{{ACCENT}}", &xml_attr(&r.accent))
                .replace("{{IMAGE}}", &image)
                .replace("{{TEXT}}", &text));
        }

        // Block heights.
        let badge_h = line_box(32.0, 1.2);
        let title_lines = two_line_parts(&r.title);
        let title_h = title_lines.len() as f64 * line_box(r.title_size, 1.05);
        let rule_block = 46.0 + 3.0;
        let sub_lines = self.wrap(&r.sub, &r.sub_font, 500, r.sub_italic == "italic", 48.0, content_w);
        let sub_h = 40.0 + sub_lines.len() as f64 * line_box(48.0, 1.3);
        let title_block_h = title_h + rule_block + sub_h;
        let author_h = line_box(42.0, 1.2);
        // (`text` declared above, before the absolute-layout branch.)

        // Resolve flex auto-margins.
        let gap1 = Margin::parse(&r.title_mt, content_w);
        let gap2 = Margin::parse(&r.author_mt, content_w);
        let fixed = badge_h + title_block_h + author_h + gap1.fixed() + gap2.fixed();
        let autos = gap1.is_auto() as u32 + gap2.is_auto() as u32;
        let leftover = (bottom - top - fixed).max(0.0);
        let share = if autos > 0 { leftover / autos as f64 } else { 0.0 };
        let g1 = gap1.value(share);
        let g2 = gap2.value(share);

        let mut y = top;
        // badge
        self.emit_line(
            &mut text,
            &up(&r.badge),
            cx,
            y + baseline(32.0, 1.2, self.vmetrics("Montserrat", 400, false)),
            &TextStyle {
                family: "Montserrat",
                weight: 400,
                size: 32.0,
                italic: false,
                letter_spacing: 9.0,
                fill: &r.badge_color,
                opacity: 0.95,
                stroke: &r.badge_stroke,
                shadow_id: "",
                anchor: "middle",
            },
        );
        y += badge_h + g1;

        // title
        let tvm = self.vmetrics(&r.serif, 800, false);
        let tshadow = shadow_def(&mut defs, &r.title_shadow, "tsh");
        for (i, ln) in title_lines.iter().enumerate() {
            self.emit_line(
                &mut text,
                &esc(ln),
                cx,
                y + baseline(r.title_size, 1.05, tvm) + i as f64 * line_box(r.title_size, 1.05),
                &TextStyle {
                    family: &r.serif,
                    weight: 800,
                    size: r.title_size,
                    italic: false,
                    letter_spacing: 0.0,
                    fill: &r.title_color,
                    opacity: 1.0,
                    stroke: &r.title_stroke,
                    shadow_id: &tshadow,
                    anchor: "middle",
                },
            );
        }
        y += title_h;
        // rule
        y += 46.0;
        text.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"220\" height=\"3\" fill=\"{}\" opacity=\"0.85\"/>",
            cx - 110.0,
            y,
            r.accent
        ));
        y += 3.0;
        // subtitle
        let svm = self.vmetrics(&r.sub_font, 500, r.sub_italic == "italic");
        let sshadow = shadow_def(&mut defs, &r.sub_shadow, "ssh");
        let sub_top = y + 40.0;
        for (i, ln) in sub_lines.iter().enumerate() {
            self.emit_line(
                &mut text,
                &esc(ln),
                cx,
                sub_top + baseline(48.0, 1.3, svm) + i as f64 * line_box(48.0, 1.3),
                &TextStyle {
                    family: &r.sub_font,
                    weight: 500,
                    size: 48.0,
                    italic: r.sub_italic == "italic",
                    letter_spacing: 0.0,
                    fill: &r.sub_color,
                    opacity: 1.0,
                    stroke: &r.sub_stroke,
                    shadow_id: &sshadow,
                    anchor: "middle",
                },
            );
        }
        y = sub_top + sub_lines.len() as f64 * line_box(48.0, 1.3);
        // author
        y += g2;
        self.emit_line(
            &mut text,
            &up(&r.author),
            cx,
            y + baseline(42.0, 1.2, self.vmetrics("Montserrat", 400, false)),
            &TextStyle {
                family: "Montserrat",
                weight: 400,
                size: 42.0,
                italic: false,
                letter_spacing: 7.0,
                fill: &r.author_color,
                opacity: 1.0,
                stroke: &r.author_stroke,
                shadow_id: "",
                anchor: "middle",
            },
        );

        Ok(FRONT_SVG_TMPL
            .replace("{{DEFS}}", &defs)
            .replace("{{BGCOLOR}}", &xml_attr(&r.bgcolor))
            .replace("{{ACCENT}}", &xml_attr(&r.accent))
            .replace("{{IMAGE}}", &image)
            .replace("{{TEXT}}", &text))
    }

    /// Absolute eBook-front text layout from `[cover.<lang>.layout]` (web cover
    /// editor). Places title/subtitle/author at their saved canvas-fraction
    /// centers/sizes (`emit_abs_element`), keeps the fixed series badge at the
    /// default top, and draws the accent rule under the title block. Mirrors the
    /// editor's Konva model: each block is centered on (xPct·W, yPct·H), wrapped to
    /// wPct·W, at fontPct·H with line-height 1.0, text horizontally centered.
    fn front_absolute(
        &self,
        text: &mut String,
        defs: &mut String,
        r: &Resolved,
        w: f64,
        h: f64,
        top: f64,
        cx: f64,
    ) {
        let l = r.layout.as_ref();

        // Series badge — not an editor element; stays at the default top center.
        self.emit_line(
            text,
            &up(&r.badge),
            cx,
            top + baseline(32.0, 1.2, self.vmetrics("Montserrat", 400, false)),
            &TextStyle {
                family: "Montserrat",
                weight: 400,
                size: 32.0,
                italic: false,
                letter_spacing: 9.0,
                fill: &r.badge_color,
                opacity: 0.95,
                stroke: &r.badge_stroke,
                shadow_id: "",
                anchor: "middle",
            },
        );

        // Title (absolute).
        let (t_top, t_h, t_cx) = self.emit_abs_element(
            text,
            defs,
            l.and_then(|l| l.title.as_ref()),
            (0.5, 0.62, 0.80, r.title_size / h),
            &r.title,
            &r.title_color,
            &r.serif,
            "bold",
            &r.title_shadow,
            "tsh",
            w,
            h,
            0.0,
        );

        // Accent rule, centered under the title block (default 46px gap, 220x3).
        let rule_y = t_top + t_h + 46.0;
        text.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"220\" height=\"3\" fill=\"{}\" opacity=\"0.85\"/>",
            fmt(t_cx - 110.0),
            fmt(rule_y),
            r.accent
        ));

        // Subtitle (absolute).
        let sub_style = if r.sub_italic == "italic" { "italic" } else { "normal" };
        self.emit_abs_element(
            text,
            defs,
            l.and_then(|l| l.subtitle.as_ref()),
            (0.5, 0.76, 0.85, 48.0 / h),
            &r.sub,
            &r.sub_color,
            &r.sub_font,
            sub_style,
            &r.sub_shadow,
            "ssh",
            w,
            h,
            0.0,
        );

        // Author (absolute; rendered as stored — the editor does not upper-case it).
        self.emit_abs_element(
            text,
            defs,
            l.and_then(|l| l.author.as_ref()),
            (0.5, 0.93, 0.80, 42.0 / h),
            &r.author,
            &r.author_color,
            "Montserrat",
            "normal",
            "none",
            "aush",
            w,
            h,
            0.0,
        );
    }

    /// Emit one absolutely-positioned text block, taking each field from `el` when
    /// present and otherwise the supplied default (`def` = x_pct, y_pct, w_pct,
    /// font_pct). Returns the block's `(top_y, total_height, center_x)` so callers
    /// can anchor adjacent decoration (e.g. the rule under the title).
    #[allow(clippy::too_many_arguments)]
    fn emit_abs_element(
        &self,
        out: &mut String,
        defs: &mut String,
        el: Option<&CoverElement>,
        def: (f64, f64, f64, f64),
        def_text: &str,
        def_fill: &str,
        def_family: &str,
        def_style: &str,
        shadow_css: &str,
        shadow_id: &str,
        w: f64,
        h: f64,
        x0: f64,
    ) -> (f64, f64, f64) {
        self.emit_abs_element_emph(
            out, defs, el, def, def_text, def_fill, def_family, def_style, shadow_css,
            shadow_id, w, h, x0, None,
        )
    }

    /// Like [`Self::emit_abs_element`], but when `emph` names phrases present in the
    /// block's text, those word-runs are re-styled (color/style/family) via inline
    /// `<tspan>`s. `emph = None` (every other caller) is byte-identical to the plain
    /// path — the emphasis machinery only engages for the back-cover blurb.
    #[allow(clippy::too_many_arguments)]
    fn emit_abs_element_emph(
        &self,
        out: &mut String,
        defs: &mut String,
        el: Option<&CoverElement>,
        def: (f64, f64, f64, f64),
        def_text: &str,
        def_fill: &str,
        def_family: &str,
        def_style: &str,
        shadow_css: &str,
        shadow_id: &str,
        w: f64,
        h: f64,
        x0: f64,
        emph: Option<&EmphSpec>,
    ) -> (f64, f64, f64) {
        let (def_x, def_y, def_w, def_font) = def;
        let x_pct = el.map(|e| e.x_pct).unwrap_or(def_x);
        let y_pct = el.map(|e| e.y_pct).unwrap_or(def_y);
        let w_pct = el.map(|e| e.w_pct).unwrap_or(def_w);
        let font_pct = el.map(|e| e.font_pct).unwrap_or(def_font);
        let fill = el.and_then(|e| e.fill.clone()).unwrap_or_else(|| def_fill.to_string());
        let family = el
            .and_then(|e| e.font_family.clone())
            .unwrap_or_else(|| def_family.to_string());
        let style = el
            .and_then(|e| e.font_style.clone())
            .unwrap_or_else(|| def_style.to_string());
        let content = el
            .and_then(|e| e.text.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| def_text.to_string());

        let size = font_pct * h;
        let block_cx = x0 + x_pct * w;
        let box_w = (w_pct * w).max(1.0);
        let italic = style.contains("italic");
        let weight: u16 = if style.contains("bold") { 700 } else { 400 };
        let lh = 1.0_f64; // Konva default line-height.
        let vm = self.vmetrics(&family, weight, italic);
        let shadow = shadow_def(defs, shadow_css, shadow_id);

        // Emphasis path: wrap into styled runs and emit each line as tspans.
        if let Some(e) = emph.filter(|e| !e.phrases.is_empty()) {
            let ew: u16 = if e.style.contains("bold") { 700 } else { 400 };
            let ei = e.style.contains("italic");
            let elines = self.wrap_emph(
                &content, e.phrases, &family, weight, italic, e.family, ew, ei, size, box_w,
            );
            let total_h = elines.len() as f64 * line_box(size, lh);
            let block_top = y_pct * h - total_h / 2.0;
            for (i, ln) in elines.iter().enumerate() {
                self.emit_emph_line(
                    out,
                    ln,
                    block_cx,
                    block_top + baseline(size, lh, vm) + i as f64 * line_box(size, lh),
                    &TextStyle {
                        family: &family,
                        weight,
                        size,
                        italic,
                        letter_spacing: 0.0,
                        fill: &fill,
                        opacity: 1.0,
                        stroke: "0px transparent",
                        shadow_id: &shadow,
                        anchor: "middle",
                    },
                    e.color,
                    e.family,
                    ew,
                    ei,
                );
            }
            return (block_top, total_h, block_cx);
        }

        let lines = self.wrap(&content, &family, weight, italic, size, box_w);
        let total_h = lines.len() as f64 * line_box(size, lh);
        let block_top = y_pct * h - total_h / 2.0;
        for (i, ln) in lines.iter().enumerate() {
            self.emit_line(
                out,
                &esc(ln),
                block_cx,
                block_top + baseline(size, lh, vm) + i as f64 * line_box(size, lh),
                &TextStyle {
                    family: &family,
                    weight,
                    size,
                    italic,
                    letter_spacing: 0.0,
                    fill: &fill,
                    opacity: 1.0,
                    stroke: "0px transparent",
                    shadow_id: &shadow,
                    anchor: "middle",
                },
            );
        }
        (block_top, total_h, block_cx)
    }

    /// Word-wrap `content` to `max_w`, tagging each word with whether it falls inside
    /// any `phrases` entry (matched as a consecutive run of words, compared
    /// punctuation/accent-insensitively via [`fold`]). Emphasized words are measured
    /// with the emphasis face so wrapping accounts for their (italic) advance.
    #[allow(clippy::too_many_arguments)]
    fn wrap_emph(
        &self,
        content: &str,
        phrases: &[String],
        base_family: &str,
        base_weight: u16,
        base_italic: bool,
        emph_family: &str,
        emph_weight: u16,
        emph_italic: bool,
        size: f64,
        max_w: f64,
    ) -> Vec<Vec<(String, bool)>> {
        let words: Vec<&str> = content.split_whitespace().collect();
        if words.is_empty() {
            return vec![vec![]];
        }
        let norm: Vec<String> = words.iter().map(|w| fold(w)).collect();
        let mut emph = vec![false; words.len()];
        for p in phrases {
            let pw: Vec<String> =
                p.split_whitespace().map(fold).filter(|s| !s.is_empty()).collect();
            if pw.is_empty() || pw.len() > norm.len() {
                continue;
            }
            for start in 0..=(norm.len() - pw.len()) {
                if (0..pw.len()).all(|j| norm[start + j] == pw[j]) {
                    for j in 0..pw.len() {
                        emph[start + j] = true;
                    }
                }
            }
        }
        let space_w = self.text_width(" ", base_family, base_weight, base_italic, size);
        let mut lines: Vec<Vec<(String, bool)>> = Vec::new();
        let mut cur: Vec<(String, bool)> = Vec::new();
        let mut cur_w = 0.0;
        for (i, wd) in words.iter().enumerate() {
            let e = emph[i];
            let (fam, wt, it) = if e {
                (emph_family, emph_weight, emph_italic)
            } else {
                (base_family, base_weight, base_italic)
            };
            let ww = self.text_width(wd, fam, wt, it, size);
            let add = if cur.is_empty() { ww } else { space_w + ww };
            if !cur.is_empty() && cur_w + add > max_w {
                lines.push(std::mem::take(&mut cur));
                cur.push((wd.to_string(), e));
                cur_w = ww;
            } else {
                cur.push((wd.to_string(), e));
                cur_w += add;
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        lines
    }

    /// Emit one wrapped line as a centered `<text>` whose emphasized runs become
    /// `<tspan>`s carrying the emphasis fill/family/style. `xml:space="preserve"`
    /// keeps the single spaces that separate runs.
    #[allow(clippy::too_many_arguments)]
    fn emit_emph_line(
        &self,
        out: &mut String,
        line: &[(String, bool)],
        cx: f64,
        baseline_y: f64,
        base: &TextStyle,
        emph_color: &str,
        emph_family: &str,
        emph_weight: u16,
        emph_italic: bool,
    ) {
        let base_fam = self.svg_family(base.family, base.weight, base.italic);
        let emph_fam = self.svg_family(emph_family, emph_weight, emph_italic);
        let mut attrs = format!(
            "x=\"{}\" y=\"{}\" text-anchor=\"{}\" xml:space=\"preserve\" font-family=\"'{}'\" font-size=\"{}\" fill=\"{}\"",
            fmt(cx),
            fmt(baseline_y),
            base.anchor,
            xml_attr(base_fam),
            fmt(base.size),
            xml_attr(base.fill),
        );
        if base.italic {
            attrs.push_str(" font-style=\"italic\"");
        }
        if !base.shadow_id.is_empty() {
            attrs.push_str(&format!(" filter=\"url(#{})\"", base.shadow_id));
        }
        let mut inner = String::new();
        let mut i = 0;
        let mut first = true;
        while i < line.len() {
            let e = line[i].1;
            let mut j = i;
            let mut words: Vec<&str> = Vec::new();
            while j < line.len() && line[j].1 == e {
                words.push(line[j].0.as_str());
                j += 1;
            }
            let mut seg = words.join(" ");
            if !first {
                seg = format!(" {seg}");
            }
            first = false;
            if e {
                let mut t = format!(" font-family=\"'{}'\"", xml_attr(emph_fam));
                if emph_italic {
                    t.push_str(" font-style=\"italic\"");
                }
                t.push_str(&format!(" fill=\"{}\"", xml_attr(emph_color)));
                inner.push_str(&format!("<tspan{t}>{}</tspan>", esc(&seg)));
            } else {
                inner.push_str(&format!("<tspan>{}</tspan>", esc(&seg)));
            }
            i = j;
        }
        out.push_str(&format!("<text {attrs}>{inner}</text>"));
    }

    /// Absolute back-panel text layout from `[cover.<lang>.wrap]` (web wrap editor).
    /// Places badge/blurb/author at their saved back-panel-fraction centers/sizes
    /// via `emit_abs_element`, mapped into the back panel's absolute wrap rectangle
    /// (origin at the wrap's left edge `x0=0`, width `back_w`, height `fh`). Each
    /// element falls back to a default fraction chosen to sit where the flex stack
    /// puts it (badge near the top, blurb in the upper-middle, author near the
    /// bottom-left), so a partial saved layout still renders sensibly. Editor model:
    /// centered on (xPct·back_w, yPct·fh), wrapped to wPct·back_w, at fontPct·fh.
    fn back_absolute(
        &self,
        body: &mut String,
        defs: &mut String,
        r: &Resolved,
        back_w: f64,
        fh: f64,
        bpad_x: f64,
    ) {
        let wl = r.wrap_layout.as_ref();
        let bcontent_w = back_w - 2.0 * bpad_x;
        let w_frac = bcontent_w / back_w;
        let emspec = EmphSpec {
            phrases: &r.blurb_emph,
            color: &r.blurb_emph_color,
            family: &r.blurb_emph_family,
            style: &r.blurb_emph_style,
        };

        // Badge (absolute; default: top-center, small caps like the flex badge).
        self.emit_abs_element(
            body,
            defs,
            wl.and_then(|w| w.badge.as_ref()),
            (0.5, 0.085, w_frac, 13.0 / fh),
            &r.badge,
            &r.badge_color,
            "Montserrat",
            "normal",
            "none",
            "wbbsh",
            back_w,
            fh,
            0.0,
        );

        // Blurb (absolute; default: upper-middle of the back panel). The only block
        // that takes an emphasis spec — highlighted phrases become inline tspans.
        self.emit_abs_element_emph(
            body,
            defs,
            wl.and_then(|w| w.blurb.as_ref()),
            (0.5, 0.42, w_frac, 21.0 / fh),
            &r.blurb,
            &r.blurb_color,
            &r.serif,
            "normal",
            &r.blurb_shadow,
            "wbbbsh",
            back_w,
            fh,
            0.0,
            Some(&emspec),
        );

        // Author (absolute; default: near the bottom, kept off the bottom-right
        // barcode keep-out by defaulting to center-x). Rendered upper-cased.
        self.emit_abs_element(
            body,
            defs,
            wl.and_then(|w| w.author.as_ref()),
            (0.5, 0.94, w_frac, 13.0 / fh),
            &r.author,
            &r.author_color,
            "Montserrat",
            "normal",
            "none",
            "wbaush",
            back_w,
            fh,
            0.0,
        );
    }

    fn wrap_svg(&self, r: &Resolved, cover_dir: &Path, pages: u32) -> Result<String> {
        const DPI: f64 = 96.0;
        let spine = round4(pages as f64 * r.paper_mult);
        let full_w = round4(2.0 * r.trim_w + spine + 2.0 * r.bleed);
        let full_h = round4(r.trim_h + 2.0 * r.bleed);
        let fw = full_w * DPI;
        let fh = full_h * DPI;
        let back_w = (r.bleed + r.trim_w) * DPI;
        let spine_w = spine * DPI;
        let spine_x = back_w;
        let front_x = back_w + spine_w;
        let front_w = (r.trim_w + r.bleed) * DPI;

        let mut defs = String::new();
        let mut body = String::new();

        // background
        let mut wrapbg = String::new();
        let filt_id = filter_def(&mut defs, &r.filt, "imgfilt");
        if let Some(w) = &r.wrap_bg {
            let href = data_uri(&cover_dir.join(w))?;
            wrapbg.push_str(&format!(
                "<image x=\"0\" y=\"0\" width=\"{fw}\" height=\"{fh}\" preserveAspectRatio=\"xMidYMid slice\" xlink:href=\"{href}\"{}/>",
                attr_filter(&filt_id)
            ));
        } else {
            // solid back panel
            body.push_str(&format!(
                "<rect x=\"0\" y=\"0\" width=\"{back_w}\" height=\"{fh}\" fill=\"{}\"/>",
                xml_attr(&r.bgcolor)
            ));
        }

        // ---- BACK panel -------------------------------------------------
        let bpad_t = 0.7 * DPI;
        let bpad_x = 0.55 * DPI;
        let bpad_b = 0.55 * DPI;
        let bcx = back_w / 2.0;
        let bcontent_w = back_w - 2.0 * bpad_x;
        // Web-editor absolute back-panel layout: when [cover.<lang>.wrap] is present,
        // place badge/blurb/author from its saved fractions (of the BACK panel:
        // width = back_w, height = fh, origin at the wrap's left edge) instead of the
        // flex stack below. Absent => unchanged default flex back (no regression).
        let has_wrap_layout = r.wrap_layout.as_ref().map_or(false, |wl| {
            wl.blurb.is_some() || wl.badge.is_some() || wl.author.is_some()
        });
        if has_wrap_layout {
            self.back_absolute(&mut body, &mut defs, r, back_w, fh, bpad_x);
        } else {
        let mut by = bpad_t;
        // bbadge
        self.emit_line(
            &mut body,
            &up(&r.badge),
            bcx,
            by + baseline(13.0, 1.2, self.vmetrics("Montserrat", 400, false)),
            &TextStyle {
                family: "Montserrat",
                weight: 400,
                size: 13.0,
                italic: false,
                letter_spacing: 4.0,
                fill: &r.badge_color,
                opacity: 0.95,
                stroke: &r.badge_stroke,
                shadow_id: "",
                anchor: "middle",
            },
        );
        by += line_box(13.0, 1.2) + 0.25 * DPI;
        // bblurb
        let blvm = self.vmetrics(&r.serif, 400, false);
        let bshadow = shadow_def(&mut defs, &r.blurb_shadow, "bbsh");
        if r.blurb_emph.is_empty() {
            let blurb_lines = self.wrap(&r.blurb, &r.serif, 400, false, 21.0, bcontent_w);
            for (i, ln) in blurb_lines.iter().enumerate() {
                self.emit_line(
                    &mut body,
                    &esc(ln),
                    bcx,
                    by + baseline(21.0, 1.55, blvm) + i as f64 * line_box(21.0, 1.55),
                    &TextStyle {
                        family: &r.serif,
                        weight: 400,
                        size: 21.0,
                        italic: false,
                        letter_spacing: 0.0,
                        fill: &r.blurb_color,
                        opacity: 1.0,
                        stroke: &r.blurb_stroke,
                        shadow_id: &bshadow,
                        anchor: "middle",
                    },
                );
            }
        } else {
            let ew: u16 = if r.blurb_emph_style.contains("bold") { 700 } else { 400 };
            let ei = r.blurb_emph_style.contains("italic");
            let elines = self.wrap_emph(
                &r.blurb, &r.blurb_emph, &r.serif, 400, false, &r.blurb_emph_family, ew, ei,
                21.0, bcontent_w,
            );
            for (i, ln) in elines.iter().enumerate() {
                self.emit_emph_line(
                    &mut body,
                    ln,
                    bcx,
                    by + baseline(21.0, 1.55, blvm) + i as f64 * line_box(21.0, 1.55),
                    &TextStyle {
                        family: &r.serif,
                        weight: 400,
                        size: 21.0,
                        italic: false,
                        letter_spacing: 0.0,
                        fill: &r.blurb_color,
                        opacity: 1.0,
                        stroke: &r.blurb_stroke,
                        shadow_id: &bshadow,
                        anchor: "middle",
                    },
                    &r.blurb_emph_color,
                    &r.blurb_emph_family,
                    ew,
                    ei,
                );
            }
        }
        // bfoot: author, bottom-left (margin-bottom auto on blurb pushes it down)
        self.emit_line(
            &mut body,
            &up(&r.author),
            bpad_x,
            fh - bpad_b - line_box(13.0, 1.2) + baseline(13.0, 1.2, self.vmetrics("Montserrat", 400, false)),
            &TextStyle {
                family: "Montserrat",
                weight: 400,
                size: 13.0,
                italic: false,
                letter_spacing: 3.0,
                fill: &r.author_color,
                opacity: 0.85,
                stroke: &r.author_stroke,
                shadow_id: "",
                anchor: "start",
            },
        );
        }

        // ---- SPINE ------------------------------------------------------
        body.push_str(&format!(
            "<rect x=\"{spine_x}\" y=\"0\" width=\"{spine_w}\" height=\"{fh}\" fill=\"{}\"/>",
            if r.wrap_bg.is_some() { "none".to_string() } else { xml_attr(&r.bgcolor) }
        ));
        // inset accent borders (box-shadow inset 3px both sides)
        body.push_str(&format!(
            "<rect x=\"{spine_x}\" y=\"0\" width=\"3\" height=\"{fh}\" fill=\"{}\"/>",
            r.accent
        ));
        body.push_str(&format!(
            "<rect x=\"{}\" y=\"0\" width=\"3\" height=\"{fh}\" fill=\"{}\"/>",
            spine_x + spine_w - 3.0,
            r.accent
        ));
        // KDP allows spine text only at >=100 pages (matches make-covers.py); below
        // that the spine stays blank but keeps its accent border rules.
        if pages >= 100 {
            let scx = spine_x + spine_w / 2.0;
            let scy = fh / 2.0;
            let spine_text = format!("{}   \u{00b7}   {}", r.title, r.author);
            body.push_str(&format!(
                "<text transform=\"translate({scx},{scy}) rotate(-90)\" text-anchor=\"middle\" \
                 font-family=\"'{}'\" font-size=\"14\" letter-spacing=\"2\" fill=\"{}\">{}</text>",
                xml_attr(&r.serif),
                xml_attr(&r.title_color),
                esc(&spine_text)
            ));
        }

        // ---- FRONT panel ------------------------------------------------
        let fpad_t = 0.55 * DPI;
        let fpad_x = 0.45 * DPI;
        let fpad_b = 0.5 * DPI;
        let fcx = front_x + front_w / 2.0;
        let fcontent_w = front_w - 2.0 * fpad_x;
        let ftop = fpad_t;
        let fbottom = fh - fpad_b;

        // front bg image (only when no wraparound photo)
        if r.wrap_bg.is_none() {
            let href = data_uri(&cover_dir.join(&r.bg))?;
            body.push_str(&format!(
                "<image x=\"{front_x}\" y=\"0\" width=\"{front_w}\" height=\"{fh}\" preserveAspectRatio=\"xMidYMid slice\" xlink:href=\"{href}\"{}/>",
                attr_filter(&filt_id)
            ));
        }
        // front gradient (over the front panel only)
        body.push_str(&format!(
            "<rect x=\"{front_x}\" y=\"0\" width=\"{front_w}\" height=\"{fh}\" fill=\"url(#grad)\"/>"
        ));

        // front title size
        let ft_px = r.wrap_title_px.unwrap_or((r.title_size * 0.30).round());
        let ft_shadow_css = r.wrap_shadow.clone().unwrap_or_else(|| r.title_shadow.clone());

        // block heights (front)
        let fbadge_h = line_box(13.0, 1.2);
        let ftitle_lines = two_line_parts(&r.title);
        let ftitle_h = ftitle_lines.len() as f64 * line_box(ft_px, r.wrap_lh);
        let frule_block = 0.18 * DPI + 2.0;
        let fsub_lines = self.wrap(&r.sub, &r.sub_font, 500, r.sub_italic == "italic", 18.0, fcontent_w);
        let fsub_h = 0.14 * DPI + fsub_lines.len() as f64 * line_box(18.0, 1.3);
        let ftitle_block_h = ftitle_h + frule_block + fsub_h;
        let fauthor_h = line_box(15.0, 1.2);
        // titleblock margin-top 7%, author margin-top auto
        let fgap1 = 0.07 * fcontent_w;
        let fixed = fbadge_h + ftitle_block_h + fauthor_h + fgap1;
        let fgap2 = (fbottom - ftop - fixed).max(0.0);

        let mut fy = ftop;
        self.emit_line(
            &mut body,
            &up(&r.badge),
            fcx,
            fy + baseline(13.0, 1.2, self.vmetrics("Montserrat", 400, false)),
            &TextStyle {
                family: "Montserrat",
                weight: 400,
                size: 13.0,
                italic: false,
                letter_spacing: 4.0,
                fill: &r.badge_color,
                opacity: 0.95,
                stroke: &r.badge_stroke,
                shadow_id: "",
                anchor: "middle",
            },
        );
        let has_layout = r.layout.as_ref().map_or(false, |l| {
            l.title.is_some() || l.subtitle.is_some() || l.author.is_some()
        });
        if has_layout {
            // Web-editor absolute layout ([cover.<lang>.layout]) mapped into the
            // front panel: offset by front_x, sized to front_w x fh, so the wrap
            // front matches the eBook front (which is the edited surface). Without
            // this the wrap kept its own flex math and the title landed off-band.
            let l = r.layout.as_ref();
            let (t_top, t_h, t_cx) = self.emit_abs_element(
                &mut body,
                &mut defs,
                l.and_then(|l| l.title.as_ref()),
                (0.5, 0.62, 0.80, ft_px / fh),
                &r.title,
                &r.title_color,
                &r.serif,
                "bold",
                &ft_shadow_css,
                "ftsh",
                front_w,
                fh,
                front_x,
            );
            let rule_y = t_top + t_h + 0.18 * DPI;
            body.push_str(&format!(
                "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"2\" fill=\"{}\" opacity=\"0.85\"/>",
                fmt(t_cx - 0.6 * DPI),
                fmt(rule_y),
                fmt(1.2 * DPI),
                r.accent
            ));
            let fsub_style = if r.sub_italic == "italic" { "italic" } else { "normal" };
            self.emit_abs_element(
                &mut body,
                &mut defs,
                l.and_then(|l| l.subtitle.as_ref()),
                (0.5, 0.76, 0.85, 18.0 / fh),
                &r.sub,
                &r.sub_color,
                &r.sub_font,
                fsub_style,
                &r.sub_shadow,
                "fssh",
                front_w,
                fh,
                front_x,
            );
            self.emit_abs_element(
                &mut body,
                &mut defs,
                l.and_then(|l| l.author.as_ref()),
                (0.5, 0.93, 0.80, 15.0 / fh),
                &r.author,
                &r.author_color,
                "Montserrat",
                "normal",
                "none",
                "faush",
                front_w,
                fh,
                front_x,
            );
        } else {
        fy += fbadge_h + fgap1;
        let ftvm = self.vmetrics(&r.serif, 800, false);
        let ftshadow = shadow_def(&mut defs, &ft_shadow_css, "ftsh");
        for (i, ln) in ftitle_lines.iter().enumerate() {
            self.emit_line(
                &mut body,
                &esc(ln),
                fcx,
                fy + baseline(ft_px, r.wrap_lh, ftvm) + i as f64 * line_box(ft_px, r.wrap_lh),
                &TextStyle {
                    family: &r.serif,
                    weight: 800,
                    size: ft_px,
                    italic: false,
                    letter_spacing: 0.0,
                    fill: &r.title_color,
                    opacity: 1.0,
                    stroke: &r.wrap_stroke,
                    shadow_id: &ftshadow,
                    anchor: "middle",
                },
            );
        }
        fy += ftitle_h + 0.18 * DPI;
        body.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"2\" fill=\"{}\" opacity=\"0.85\"/>",
            fcx - 0.6 * DPI,
            fy,
            1.2 * DPI,
            r.accent
        ));
        fy += 2.0;
        let fsvm = self.vmetrics(&r.sub_font, 500, r.sub_italic == "italic");
        let fsshadow = shadow_def(&mut defs, &r.sub_shadow, "fssh");
        let fsub_top = fy + 0.14 * DPI;
        for (i, ln) in fsub_lines.iter().enumerate() {
            self.emit_line(
                &mut body,
                &esc(ln),
                fcx,
                fsub_top + baseline(18.0, 1.3, fsvm) + i as f64 * line_box(18.0, 1.3),
                &TextStyle {
                    family: &r.sub_font,
                    weight: 500,
                    size: 18.0,
                    italic: r.sub_italic == "italic",
                    letter_spacing: 0.0,
                    fill: &r.sub_color,
                    opacity: 1.0,
                    stroke: &r.sub_stroke,
                    shadow_id: &fsshadow,
                    anchor: "middle",
                },
            );
        }
        let fy_author = ftop + fbadge_h + fgap1 + ftitle_block_h + fgap2;
        self.emit_line(
            &mut body,
            &up(&r.author),
            fcx,
            fy_author + baseline(15.0, 1.2, self.vmetrics("Montserrat", 400, false)),
            &TextStyle {
                family: "Montserrat",
                weight: 400,
                size: 15.0,
                italic: false,
                letter_spacing: 3.0,
                fill: &r.author_color,
                opacity: 1.0,
                stroke: &r.author_stroke,
                shadow_id: "",
                anchor: "middle",
            },
        );
        }

        Ok(WRAP_SVG_TMPL
            .replace("{{FULL_W_PX}}", &fmt(fw))
            .replace("{{FULL_H_PX}}", &fmt(fh))
            .replace("{{BACK_W_PX}}", &fmt(back_w))
            .replace("{{BGCOLOR}}", &xml_attr(&r.bgcolor))
            .replace("{{DEFS}}", &defs)
            .replace("{{WRAPBG}}", &wrapbg)
            .replace("{{BODY}}", &body))
    }
}

// ---------------------------------------------------------------------------
// Layout / text helpers
// ---------------------------------------------------------------------------

/// CSS line box height for a font size + line-height multiple.
fn line_box(size: f64, lh: f64) -> f64 {
    size * lh
}

/// First-baseline offset within a line box of height `size*lh`, given normalized
/// (ascender, descender) metrics.
fn baseline(size: f64, lh: f64, vm: (f64, f64)) -> f64 {
    let (asc, desc) = vm;
    (size * lh - size * (asc + desc)) / 2.0 + size * asc
}

/// A flex margin that may be `auto`, a `%` (of the reference width), or `px`.
enum Margin {
    Auto,
    Px(f64),
}
impl Margin {
    fn parse(s: &str, ref_w: f64) -> Margin {
        let s = s.trim();
        if s == "auto" {
            Margin::Auto
        } else if let Some(p) = s.strip_suffix('%') {
            Margin::Px(p.trim().parse::<f64>().unwrap_or(0.0) / 100.0 * ref_w)
        } else if let Some(p) = s.strip_suffix("px") {
            Margin::Px(p.trim().parse::<f64>().unwrap_or(0.0))
        } else {
            Margin::Px(s.parse::<f64>().unwrap_or(0.0))
        }
    }
    fn is_auto(&self) -> bool {
        matches!(self, Margin::Auto)
    }
    fn fixed(&self) -> f64 {
        match self {
            Margin::Px(v) => *v,
            Margin::Auto => 0.0,
        }
    }
    fn value(&self, auto_share: f64) -> f64 {
        match self {
            Margin::Px(v) => *v,
            Margin::Auto => auto_share,
        }
    }
}

struct TextStyle<'a> {
    family: &'a str,
    weight: u16,
    size: f64,
    italic: bool,
    letter_spacing: f64,
    fill: &'a str,
    opacity: f64,
    stroke: &'a str,
    shadow_id: &'a str,
    anchor: &'a str,
}

/// Inline emphasis for a wrapped text block: which phrases to highlight and the
/// style (color/font-style/family) applied to their word-runs. Used only for the
/// back-cover blurb; every other text block passes `None`.
struct EmphSpec<'a> {
    phrases: &'a [String],
    color: &'a str,
    family: &'a str,
    style: &'a str,
}


// ---------------------------------------------------------------------------
// CSS-ish parsing: strokes, shadows, filters, colors
// ---------------------------------------------------------------------------

/// "6px #000000" -> Some((6.0, "#000000")); "0px transparent" / "none" -> None.
fn parse_stroke(s: &str) -> Option<(f64, String)> {
    let s = s.trim();
    if s.is_empty() || s == "none" {
        return None;
    }
    let mut it = s.split_whitespace();
    let w = it.next()?;
    let w: f64 = w.trim_end_matches("px").parse().ok()?;
    if w <= 0.0 {
        return None;
    }
    let color = it.next().unwrap_or("#000000");
    if color == "transparent" {
        return None;
    }
    let (hex, op) = parse_color(color);
    // bake low opacity into the color is non-trivial; strokes here are opaque.
    let _ = op;
    Some((w, hex))
}

/// Parse "#RRGGBB" or "rgba(r,g,b,a)" / "rgb(...)" -> (hex, opacity).
fn parse_color(c: &str) -> (String, f64) {
    let c = c.trim();
    if c == "transparent" {
        return ("#000000".into(), 0.0);
    }
    if let Some(rest) = c.strip_prefix("rgba(").or_else(|| c.strip_prefix("rgb(")) {
        let rest = rest.trim_end_matches(')');
        let parts: Vec<f64> = rest
            .split(',')
            .filter_map(|p| p.trim().parse::<f64>().ok())
            .collect();
        if parts.len() >= 3 {
            let hex = format!(
                "#{:02X}{:02X}{:02X}",
                parts[0] as u8, parts[1] as u8, parts[2] as u8
            );
            let op = if parts.len() >= 4 { parts[3] } else { 1.0 };
            return (hex, op);
        }
    }
    (c.to_string(), 1.0)
}

/// Build a CSS-filter chain (brightness/saturate/contrast) into `defs`; returns
/// the filter id, or "" for `none`.
fn filter_def(defs: &mut String, css: &str, id: &str) -> String {
    let css = css.trim();
    if css.is_empty() || css == "none" {
        return String::new();
    }
    let mut prims = String::new();
    // parse functions: name(value)
    let mut rest = css;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().rsplit(char::is_whitespace).next().unwrap_or("").to_string();
        let close = match rest[open..].find(')') {
            Some(c) => open + c,
            None => break,
        };
        let val: f64 = rest[open + 1..close].trim().parse().unwrap_or(1.0);
        match name.as_str() {
            "brightness" => prims.push_str(&format!(
                "<feComponentTransfer><feFuncR type=\"linear\" slope=\"{val}\"/><feFuncG type=\"linear\" slope=\"{val}\"/><feFuncB type=\"linear\" slope=\"{val}\"/></feComponentTransfer>"
            )),
            "contrast" => {
                let int = 0.5 - 0.5 * val;
                prims.push_str(&format!(
                    "<feComponentTransfer><feFuncR type=\"linear\" slope=\"{val}\" intercept=\"{int}\"/><feFuncG type=\"linear\" slope=\"{val}\" intercept=\"{int}\"/><feFuncB type=\"linear\" slope=\"{val}\" intercept=\"{int}\"/></feComponentTransfer>"
                ));
            }
            "saturate" => prims.push_str(&format!(
                "<feColorMatrix type=\"saturate\" values=\"{val}\"/>"
            )),
            _ => {}
        }
        rest = &rest[close + 1..];
    }
    if prims.is_empty() {
        return String::new();
    }
    // CSS filters operate in sRGB; SVG filters default to linearRGB. Force sRGB
    // so brightness/contrast/saturate match the Chrome/CSS result.
    defs.push_str(&format!(
        "<filter id=\"{id}\" x=\"0\" y=\"0\" width=\"100%\" height=\"100%\" color-interpolation-filters=\"sRGB\">{prims}</filter>"
    ));
    id.to_string()
}

fn attr_filter(id: &str) -> String {
    if id.is_empty() {
        String::new()
    } else {
        format!(" filter=\"url(#{id})\"")
    }
}

/// Build a text-shadow filter into `defs` (chained feDropShadow); returns id or "".
fn shadow_def(defs: &mut String, css: &str, id: &str) -> String {
    let shadows = parse_text_shadows(css);
    if shadows.is_empty() {
        return String::new();
    }
    let mut prims = String::new();
    // Chain feDropShadows: each layers another halo/offset over the running result.
    for sh in &shadows {
        prims.push_str(&format!(
            "<feDropShadow dx=\"{}\" dy=\"{}\" stdDeviation=\"{}\" flood-color=\"{}\" flood-opacity=\"{}\"/>",
            fmt(sh.dx),
            fmt(sh.dy),
            fmt(sh.blur / 2.0),
            sh.color,
            fmt(sh.opacity)
        ));
    }
    defs.push_str(&format!(
        "<filter id=\"{id}\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"200%\" color-interpolation-filters=\"sRGB\">{prims}</filter>"
    ));
    id.to_string()
}

struct Shadow {
    dx: f64,
    dy: f64,
    blur: f64,
    color: String,
    opacity: f64,
}

/// Parse a CSS text-shadow list ("dx dy blur color, ..."), splitting on
/// top-level commas (not the commas inside rgba()).
fn parse_text_shadows(css: &str) -> Vec<Shadow> {
    let css = css.trim();
    if css.is_empty() || css == "none" {
        return Vec::new();
    }
    let mut out = Vec::new();
    for part in split_top_commas(css) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        // tokens: numbers (with px) then a color (hex or rgba(...))
        let mut nums: Vec<f64> = Vec::new();
        let mut color = String::from("#000000");
        let mut opacity = 1.0;
        let mut rest = part;
        loop {
            rest = rest.trim_start();
            if rest.is_empty() {
                break;
            }
            if rest.starts_with('#') || rest.starts_with("rgb") {
                let (hex, op) = parse_color(rest);
                color = hex;
                opacity = op;
                break;
            }
            // read one whitespace token (a length)
            let tok_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let tok = &rest[..tok_end];
            if let Ok(v) = tok.trim_end_matches("px").parse::<f64>() {
                nums.push(v);
            }
            rest = &rest[tok_end..];
        }
        let dx = nums.first().copied().unwrap_or(0.0);
        let dy = nums.get(1).copied().unwrap_or(0.0);
        let blur = nums.get(2).copied().unwrap_or(0.0);
        out.push(Shadow { dx, dy, blur, color, opacity });
    }
    out
}

fn split_top_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in s.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

// ---------------------------------------------------------------------------
// small utilities
// ---------------------------------------------------------------------------

fn round4(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
}

/// Format a float compactly (no trailing zeros).
fn fmt(x: f64) -> String {
    let s = format!("{:.4}", x);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

/// XML-escape text content (& < >).
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// XML-escape an attribute value (adds quotes).
fn xml_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;")
}

/// Unicode uppercase, then XML-escape (for badge/author text-transform).
fn up(s: &str) -> String {
    esc(&s.to_uppercase())
}

/// Fold a word for phrase matching: keep alphanumerics only (so surrounding
/// punctuation like « » , . ; is ignored), lowercase, and strip common Spanish
/// accents — so "«Donde" and "derecho.»" match "donde" and "derecho".
fn fold(w: &str) -> String {
    w.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'á' => 'a',
            'é' => 'e',
            'í' => 'i',
            'ó' => 'o',
            'ú' => 'u',
            'ü' => 'u',
            'ñ' => 'n',
            other => other,
        })
        .collect()
}

/// Read the (weight-specific) family name from a font's `name` table (ID 1).
fn family_name(data: &[u8]) -> Option<String> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    let mut fallback = None;
    for n in face.names() {
        if n.name_id == 1 {
            if let Some(s) = n.to_string() {
                if n.is_unicode() {
                    return Some(s);
                }
                fallback.get_or_insert(s);
            }
        }
    }
    fallback
}

/// Read an image file and return a `data:` URI (base64).
fn data_uri(path: &Path) -> Result<String> {
    if !path.exists() {
        bail!("image not found: {}", path.display());
    }
    let bytes = std::fs::read(path)?;
    let mime = match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()) {
        Some(e) if e == "png" => "image/png",
        Some(e) if e == "jpg" || e == "jpeg" => "image/jpeg",
        Some(e) if e == "webp" => "image/webp",
        _ => "application/octet-stream",
    };
    Ok(format!("data:{mime};base64,{}", b64(&bytes)))
}

/// Minimal standard base64 encoder (no padding omitted).
fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}
