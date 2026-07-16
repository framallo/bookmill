//! Native Typst PDF engine — the sole PDF backend.
//!
//! Converts the resolved chapter markdown into a single Typst document and
//! compiles it with the **`typst` crate as a library** — no `typst` binary on
//! PATH is required. Every PDF output (retail + KDP print) renders through this
//! engine. (EPUB renders via `epub_native` and the editor `.docx` via
//! `docx_native` — all native Rust, no pandoc and no external tools anywhere.)
//!
//! The compile step is driven by a small [`World`] (`BookWorld`) that:
//!   * serves the generated markup as the *main* source file;
//!   * resolves `image("/…")` references from the repo root (the same rooting
//!     the CLI's `--root` gave us), via Typst's `VirtualPath::realize`;
//!   * loads fonts with `typst_kit` (Typst's embedded defaults + system fonts).
//!
//! Reproduced interior conventions:
//!   * page geometry (paper size incl. bleed + margins) from `PageGeometry`;
//!   * chapters open on a recto page when `openright`;
//!   * auto-numbered chapter headings ("Capítulo N" / "Chapter N", terracotta),
//!     with `{.unnumbered}` headings (the epilogue) showing only the title;
//!   * full-page chapter-opening plates that bleed to all four edges;
//!   * small centered `.spot` tailpiece images (~2.4in);
//!   * a title page, a copyright page, and a table of contents.
//!
//! The Markdown -> Typst converter is intentionally small: headings (with the
//! `{.unnumbered}`/`{.spot}` attributes), paragraphs, `**bold**`/`*italic*`,
//! links, standalone images, `---` scene breaks, GFM pipe tables, and inline
//! `^[...]` footnotes. Anything else falls through as escaped literal text.
//! Reference-style GFM footnotes (`[^id]` + `[^id]:` definitions) are deferred:
//! they need a two-pass collect across blocks and no book in the series uses
//! them; inline `^[...]` covers the footnote need for now.

use crate::build::{BookMeta, PageGeometry};
use crate::discover::Repo;
use anyhow::{anyhow, Context, Result};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::fonts::FontStore;
use typst_layout::PagedDocument;
use typst_pdf::{PdfOptions, PdfStandard, PdfStandards};

/// Render a book (one language / one PDF flavor) to `out` by compiling the
/// generated Typst markup with the `typst` crate (no external CLI).
#[allow(clippy::too_many_arguments)]
pub fn run(
    repo: &Repo,
    meta: &BookMeta,
    cpdf: Option<&Path>,
    chaps: &[PathBuf],
    openright: bool,
    plate_framed: bool,
    plate_width: f32,
    captions: bool,
    retail: bool,
    grayscale: bool,
    proof: bool,
    auto_grayscale: bool,
    cover: Option<&Path>,
    geometry: Option<PageGeometry>,
    lang: &str,
    out: &Path,
) -> Result<()> {
    let (doc, described) = build_doc(repo, meta, cpdf, chaps, openright, plate_framed, plate_width, captions, retail, grayscale, proof, cover, geometry, lang)?;
    let odir = out.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(odir)?;
    // Keep the generated markup on disk for debugging only — it is NOT handed to
    // any `typst` binary; compilation happens natively below.
    let typ = odir.join(".typst-build.typ");
    std::fs::write(&typ, &doc).with_context(|| format!("writing {}", typ.display()))?;

    // Compile natively. The World resolves images under the repo root, exactly
    // as the CLI's `--root <repo.root>` did.
    let world = BookWorld::new(repo.root.clone(), doc, grayscale, auto_grayscale)?;
    let result = typst::compile::<PagedDocument>(&world);
    let document = result.output.map_err(|diags| {
        anyhow!(
            "typst compile failed for {}:\n{}",
            out.display(),
            join_diags(diags.iter().map(|d| d.message.to_string()))
        )
    })?;
    // Export. Typst tags every PDF it writes (structure tree, `/Lang` from
    // `#set text(lang:)`, `/Title` from `#set document`); asking for **PDF/UA-1** on
    // top makes it *validate* that tagging — above all, that every image carries alt
    // text. So UA-1 is requested only when the book's images really are all described
    // (`described`), the same "derive, never assert" rule the EPUB metadata follows.
    // If the validated export still trips on something, fall back to the plain (still
    // tagged) PDF and say so, rather than failing a build that used to work.
    let ua1 = described.then(ua1_standards).flatten();
    let mut opts = PdfOptions::default();
    if let Some(std) = ua1.clone() {
        opts.standards = std;
    }
    let plain = |d: &PagedDocument| {
        typst_pdf::pdf(d, &PdfOptions::default()).map_err(|diags| {
            anyhow!(
                "typst PDF export failed for {}:\n{}",
                out.display(),
                join_diags(diags.iter().map(|d| d.message.to_string()))
            )
        })
    };
    let pdf = match typst_pdf::pdf(&document, &opts) {
        Ok(bytes) => bytes,
        Err(diags) if ua1.is_some() => {
            eprintln!(
                "  (pdf: PDF/UA-1 validation failed for {}; writing a tagged PDF with no UA-1 \
                 claim: {})",
                out.display(),
                join_diags(diags.iter().map(|d| d.message.to_string()))
            );
            plain(&document)?
        }
        Err(_) => plain(&document)?,
    };
    std::fs::write(out, pdf).with_context(|| format!("writing {}", out.display()))?;
    // Drop the memoization cache so repeated builds in one process don't grow it
    // unbounded (each book is a distinct document; nothing is reused).
    comemo::evict(0);
    Ok(())
}

/// The PDF/UA-1 export configuration, or `None` if this Typst build can't express it.
fn ua1_standards() -> Option<PdfStandards> {
    PdfStandards::new(&[PdfStandard::Ua_1]).ok()
}

/// True when every image this build renders carries alt text.
///
/// Decorative **spot** tailpieces are exempt: they ship as PDF *artifacts* (see
/// [`emit_blocks`]), which is exactly what a decorative image should be, and an
/// artifact needs no alt. Gates the PDF/UA-1 claim — we never assert a standard the
/// content doesn't earn.
fn all_images_described(blocks: &[Block]) -> bool {
    blocks.iter().all(|b| match b {
        Block::Image { spot, alt, .. } => *spot || !alt.trim().is_empty(),
        _ => true,
    })
}

fn join_diags(msgs: impl Iterator<Item = String>) -> String {
    let v: Vec<String> = msgs.collect();
    if v.is_empty() {
        "(no diagnostics)".to_string()
    } else {
        v.join("\n")
    }
}

/// True if `path` is a pre-made grayscale print variant — i.e. it lives directly
/// in a `bw/` directory (the convention the markdown `{bw=…}` attribute points
/// at). Such a file is already grayscale, so the print build uses it verbatim
/// instead of re-encoding it.
fn is_bw_variant_path(path: &Path) -> bool {
    path.parent()
        .and_then(|p| p.file_name())
        .map(|n| n == "bw")
        .unwrap_or(false)
}

/// Raster image whose bytes can be recolored (the `image` crate has png+jpeg).
fn is_raster_image(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg")
    )
}

/// Convert image bytes to grayscale, re-encoded in the **same container format**
/// as the input (so a `.jpg`-named source stays JPEG and a `.png` stays PNG —
/// Typst picks its decoder from the file extension, so the byte format must not
/// change). PNG keeps its alpha channel (luma + alpha) so cut-out chapter plates
/// still float on the page color; JPEG has no alpha (luma only). Returns `None`
/// on any decode/encode failure so the caller falls back to the original bytes.
fn to_grayscale(data: &[u8]) -> Option<Vec<u8>> {
    let fmt = image::guess_format(data).ok()?;
    let img = image::load_from_memory(data).ok()?;
    let mut out = Vec::new();
    let cur = &mut Cursor::new(&mut out);
    match fmt {
        // PNG: preserve transparency; standard luma weighting carries alpha through.
        image::ImageFormat::Png => image::DynamicImage::ImageLumaA8(img.to_luma_alpha8())
            .write_to(cur, image::ImageFormat::Png)
            .ok()?,
        // JPEG: no alpha; re-encode at high quality for print.
        image::ImageFormat::Jpeg => {
            let gray = img.to_luma8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(cur, 92)
                .encode_image(&gray)
                .ok()?
        }
        // Only png/jpeg reach here (see `is_raster_image`); anything else is left alone.
        _ => return None,
    }
    Some(out)
}

// ---------- Typst compilation World ----------

/// Process-wide font store: Typst's embedded defaults plus the system fonts.
/// Scanning the system font directories is done once and shared by every build.
fn font_store() -> &'static FontStore {
    static FONTS: OnceLock<FontStore> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut store = FontStore::new();
        // Embedded defaults first (Libertinus Serif / New Computer Modern / …),
        // then everything installed on the system (Playfair Display, etc.).
        store.extend(typst_kit::fonts::embedded());
        store.extend(typst_kit::fonts::system());
        store
    })
}

/// A minimal Typst [`World`]: the generated markup is the main source, image
/// (and any source) files resolve under `root`, fonts come from [`font_store`].
struct BookWorld {
    root: PathBuf,
    library: LazyHash<Library>,
    fonts: &'static FontStore,
    main_id: FileId,
    main: Source,
    /// This is a B&W (black-ink) print interior: apply the print grayscale split
    /// (EPUB/retail pass false and keep color). Enables `{bw=…}` variant reads.
    grayscale: bool,
    /// When grayscale, also naively luma-convert images that have NO `{bw=…}`
    /// variant. Off by default (config `[pdf].auto_grayscale`); when off, non-bw
    /// images are left untouched (KDP still prints black-ink interiors in gray).
    auto_grayscale: bool,
}

impl BookWorld {
    fn new(root: PathBuf, markup: String, grayscale: bool, auto_grayscale: bool) -> Result<Self> {
        // Main source lives at a fixed project-rooted vpath; image paths in the
        // markup are absolute (`/images/…`) so they resolve from `root`.
        let vpath = VirtualPath::new("/.typst-build.typ")
            .map_err(|e| anyhow!("invalid main source path: {e}"))?;
        let main_id = FileId::new(RootedPath::new(VirtualRoot::Project, vpath));
        let main = Source::new(main_id, markup);
        Ok(Self {
            root,
            library: LazyHash::new(Library::default()),
            fonts: font_store(),
            main_id,
            main,
            grayscale,
            auto_grayscale,
        })
    }

    /// Map a `FileId` to a real path under the repo root, mirroring `--root`.
    /// Package roots are unsupported (the documents import no packages).
    fn realize(&self, id: FileId) -> FileResult<PathBuf> {
        match id.root() {
            VirtualRoot::Project => {
                id.vpath().realize(&self.root).map_err(|_| FileError::AccessDenied)
            }
            VirtualRoot::Package(_) => Err(FileError::AccessDenied),
        }
    }
}

impl World for BookWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.fonts.book()
    }

    fn main(&self) -> FileId {
        self.main_id
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main_id {
            return Ok(self.main.clone());
        }
        let path = self.realize(id)?;
        let text = std::fs::read_to_string(&path).map_err(|e| FileError::from_io(e, &path))?;
        Ok(Source::new(id, text))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        let path = self.realize(id)?;
        // Print-interior B&W split (KDP/POD print PDF only; EPUB and retail pass
        // grayscale=false, keeping full color). Two ways an image goes grayscale:
        //   1. The markdown `{bw=…}` attribute already swapped in a pre-made
        //      grayscale variant (see emit_blocks; print build only). That path
        //      lives in a `bw/` directory and is read verbatim — no re-encode, so
        //      a hand-tuned print variant survives untouched. Always applied.
        //   2. On-the-fly luma conversion of an image WITHOUT a `{bw=…}` variant —
        //      only when `auto_grayscale` is enabled (config `[pdf].auto_grayscale`,
        //      default off). When off, non-bw images are read as-is (color); KDP
        //      still prints a black-ink interior in grayscale at press time.
        // On any read/decode failure it falls back to the original bytes so the
        // build never breaks over the split.
        if self.grayscale && is_raster_image(&path) {
            if is_bw_variant_path(&path) {
                let data = std::fs::read(&path).map_err(|e| FileError::from_io(e, &path))?;
                return Ok(Bytes::new(data));
            }
            if self.auto_grayscale {
                let data = std::fs::read(&path).map_err(|e| FileError::from_io(e, &path))?;
                return Ok(Bytes::new(to_grayscale(&data).unwrap_or(data)));
            }
        }
        let data = std::fs::read(&path).map_err(|e| FileError::from_io(e, &path))?;
        Ok(Bytes::new(data))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.font(index)
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        // The documents never call `today()`; return a constant so it's defined.
        Datetime::from_ymd(2024, 1, 1)
    }
}

// ---------- document assembly ----------

#[allow(clippy::too_many_arguments)]
fn build_doc(
    repo: &Repo,
    meta: &BookMeta,
    cpdf: Option<&Path>,
    chaps: &[PathBuf],
    openright: bool,
    plate_framed: bool,
    plate_width: f32,
    captions: bool,
    retail: bool,
    grayscale: bool,
    proof: bool,
    cover: Option<&Path>,
    geometry: Option<PageGeometry>,
    lang: &str,
) -> Result<(String, bool)> {
    let g = geometry.unwrap_or(PageGeometry {
        pw: 6.0,
        ph: 9.0,
        top: 0.75,
        bottom: 0.75,
        inner: 0.75,
        outer: 0.6,
        bindingoffset: 0.375,
    });
    // Proof (proofreading) builds ignore the book's KDP trim and print on US
    // Legal (8.5×14) with tight uniform margins and no binding offset — the goal
    // is to fit the most text per sheet so the printed proof takes the fewest
    // pages. It's throwaway paper, not a bound interior.
    let g = if proof {
        PageGeometry { pw: 8.5, ph: 14.0, top: 0.5, bottom: 0.5, inner: 0.5, outer: 0.5, bindingoffset: 0.0 }
    } else {
        g
    };
    let inside = g.inner + g.bindingoffset;
    let chlabel = if lang == "es" { "Capítulo" } else { "Chapter" };
    let toc_title = if lang == "es" { "Índice" } else { "Contents" };
    // recto-open break used before each chapter heading. Proof builds never
    // force a chapter onto an odd page — blank verso pages just waste sheets.
    let recto = if openright && !proof {
        "pagebreak(to: \"odd\", weak: true)"
    } else {
        "pagebreak(weak: true)"
    };

    let mut s = String::new();
    // ---- preamble ----
    s.push_str(&format!(
        "#set document(title: {t}, author: {a})\n",
        t = ty_str(&meta.title),
        a = ty_str(&meta.author),
    ));
    s.push_str(&format!(
        "#set page(width: {pw:.4}in, height: {ph:.4}in, margin: (top: {top:.4}in, \
bottom: {bottom:.4}in, inside: {inside:.4}in, outside: {outside:.4}in))\n",
        pw = g.pw,
        ph = g.ph,
        top = g.top,
        bottom = g.bottom,
        inside = inside,
        outside = g.outer,
    ));
    // Retail-only paper-texture page background: scale
    // `<repo>/images/paper-texture.jpg` to the full page behind every page. KDP
    // print PDFs must NOT get it. Skip silently if the image is absent.
    if retail && repo.paper_texture().exists() {
        s.push_str(&format!(
            "#set page(background: image(\"/{}\", width: 100%, height: 100%))\n",
            repo.config.paths.paper_texture,
        ));
    }
    s.push_str(&format!("#set text(size: 11pt, lang: {})\n", ty_str(lang)));
    s.push_str("#set par(justify: true, leading: 0.72em, first-line-indent: 1.2em)\n");
    s.push_str("#set heading(numbering: \"1\")\n");
    s.push_str("#let islatitle = rgb(\"#C2571C\")\n");
    s.push_str(&format!("#let chlabel = {}\n", ty_str(chlabel)));
    // Chapter plate on the verso (left) page, facing the chapter opener on the
    // recto. Two styles:
    //   * bleed (default): zero-margin, cover-fit, fills the paper edge-to-edge
    //     (incl. bleed) — mirrors LaTeX `\cleartoverso` + `\AddThisPageImage`.
    //   * framed: centered within the page margins, contained (never cropped),
    //     with a thin terracotta keyline border — no bleed.
    if plate_framed {
        // framed plate: contained image + optional italic caption (from the image
        // alt text) below it, constrained to the image width so long captions wrap.
        // `w` is the plate width as a fraction of the text column; it defaults to
        // the book-level `plate_width` (default 0.78) but a single image can
        // override it with a pandoc `{width=NN%}` attribute (see emit_blocks).
        // `w`/`h`/`ft` map to Typst image width/height/fit; `b` toggles the keyline
        // border. All default to the book-level style but a single image can override
        // each via its `{width= height= fit= border=}` attributes (see emit_blocks).
        let pw = (plate_width.clamp(0.1, 1.0) * 100.0).round() as u32;
        s.push_str(&format!(
            "#let plate(p, c: none, w: {pw}%, h: auto, ft: \"contain\", b: true, a: none) = [\n  \
#pagebreak(to: \"even\", weak: true)\n  \
#v(1fr)\n  \
#align(center, box(stroke: if b {{ 0.75pt + islatitle }} else {{ none }}, inset: 0pt, \
image(p, width: w, height: h, fit: ft, alt: a)))\n  \
#if c != none [\n    #v(0.85em)\n    \
#align(center, block(width: w, \
text(size: 9.5pt, style: \"italic\", fill: luma(70))[#c]))\n  ]\n  \
#v(1fr)\n  #pagebreak()\n]\n",
        ));
    } else {
        // full-bleed plate: image fills the page, so there is no room for a caption
        // (the alt still ships as EPUB accessibility text). The style args
        // (`c`/`w`/`h`/`ft`/`b`) are accepted + ignored — a full-bleed plate always
        // fills the page, so per-image width/fit/border can't apply.
        s.push_str(
            "#let plate(p, c: none, w: none, h: none, ft: none, b: none, a: none) = [\n  \
#pagebreak(to: \"even\", weak: true)\n  \
#set page(margin: 0pt, header: none, footer: none)\n  \
#image(p, width: 100%, height: 100%, fit: \"cover\", alt: a)\n  #pagebreak()\n]\n",
        );
    }
    // Small centered tailpiece (spot). Fit inside a 2.4in box (preserve aspect),
    // mirroring pandoc's keepaspectratio — a tall vignette stays narrow so it keeps
    // ≥300dpi. Wrapped in `pdf.artifact`: a spot is purely decorative (the EPUB drops
    // it entirely), and a decorative image belongs in the tag tree as an artifact, not
    // as an undescribed figure — that's what keeps it out of a screen reader's way.
    s.push_str(
        "#let spot(p) = { v(1.5em); pdf.artifact(kind: \"other\", \
align(center, image(p, width: 2.4in, height: 2.4in, fit: \"contain\"))); v(1em) }\n",
    );
    // centered scene break
    s.push_str(
        "#let scenebreak = { v(0.6em); align(center)[#sym.dot.c#h(0.6em)#sym.dot.c#h(0.6em)#sym.dot.c]; v(0.6em) }\n",
    );
    // chapter heading: terracotta "Capítulo N" label line + centered title
    s.push_str(&format!(
        "#show heading.where(level: 1): it => {{\n  {recto}\n  \
block(width: 100%, above: 20pt, below: 40pt, {{\n    set align(center)\n    \
if it.numbering != none {{\n      \
text(fill: islatitle, size: 14pt, smallcaps[#chlabel #context counter(heading).display(\"1\")])\n      \
linebreak()\n      v(12pt)\n    }}\n    \
text(fill: islatitle, weight: \"bold\", size: 22pt, it.body)\n  }})\n}}\n",
    ));
    s.push_str(
        "#show heading.where(level: 2): it => block(above: 1.2em, below: 0.6em, \
text(weight: \"bold\", size: 13pt, it.body))\n",
    );
    s.push_str("\n");

    // Front matter (copyright page, TOC) is numbered in lowercase roman; the body
    // restarts at arabic 1 at the first chapter (see the `numbering: "1"` +
    // counter reset below). Title/cover pages pass `footer: none`, so no visible
    // number there, but the counter still advances like a real book's front matter.
    s.push_str("#set page(numbering: \"i\")\n");

    // ---- front matter ----
    // optional retail cover plate (page 1)
    if retail {
        if let Some(cv) = cover {
            let p = typst_img_path(&repo.root, &cv.display().to_string());
            // The cover art is content, not decoration: describe it, so a reader that
            // opens on page 1 is told what it is rather than meeting a silent figure.
            let calt = if lang == "es" {
                format!("Cubierta del libro: {}", meta.title)
            } else {
                format!("Book cover: {}", meta.title)
            };
            s.push_str(&format!(
                "#page(margin: 0pt, footer: none, header: none)[#image({}, width: 100%, height: 100%, fit: \"cover\", alt: {})]\n",
                ty_str(&p),
                ty_str(&calt)
            ));
        }
    }
    // title page
    s.push_str("#page(footer: none)[\n  #v(1fr)\n  #align(center)[\n");
    s.push_str(&format!(
        "    #text(size: 28pt, weight: \"bold\")[{}]\n",
        inline(&meta.title)
    ));
    if let Some(sub) = &meta.subtitle {
        s.push_str(&format!(
            "    #v(10pt)\n    #text(size: 15pt, style: \"italic\")[{}]\n",
            inline(sub)
        ));
    }
    s.push_str(&format!(
        "    #v(28pt)\n    #text(size: 13pt)[{}]\n",
        inline(&meta.author)
    ));
    s.push_str("  ]\n  #v(1fr)\n]\n");

    // copyright page + table of contents (only when the book ships a copyright)
    if let Some(cp) = cpdf {
        let txt = std::fs::read_to_string(cp)
            .with_context(|| format!("reading {}", cp.display()))?;
        let blocks = parse_blocks(&txt);
        s.push_str("#page(footer: none)[\n  #v(1fr)\n  #set align(center)\n  #set text(size: 10pt)\n");
        for b in &blocks {
            if let Block::Para(t) = b {
                s.push_str(&format!("  {}\n\n", inline(t)));
            }
        }
        s.push_str("  #v(1fr)\n]\n");
        s.push_str(&format!(
            "#outline(title: [{}], depth: 1, indent: auto)\n",
            inline(toc_title)
        ));
    }

    // start body page numbering at 1
    s.push_str("#set page(numbering: \"1\")\n#counter(page).update(1)\n\n");

    // ---- chapters ----
    // `described` gates the PDF/UA-1 claim (see `all_images_described`). A proof
    // build renders no images at all, so it is trivially described.
    let mut described = true;
    for ch in chaps {
        let txt = std::fs::read_to_string(ch)
            .with_context(|| format!("reading {}", ch.display()))?;
        let blocks = parse_blocks(&txt);
        if !proof {
            described &= all_images_described(&blocks);
        }
        emit_blocks(&mut s, &blocks, &repo.root, captions, grayscale, proof);
        s.push('\n');
    }

    Ok((s, described))
}

/// Emit a chapter's blocks, applying the heading+plate reorder: a chapter that
/// opens with a standalone (non-spot) image renders the image as a full-page
/// verso plate BEFORE its heading, so heading+body open together on the recto.
fn emit_blocks(s: &mut String, blocks: &[Block], root: &Path, captions: bool, grayscale: bool, proof: bool) {
    // On a grayscale (print) build, an image's `{bw=…}` variant replaces `src`;
    // otherwise (EPUB/retail) `src` is used and its color is kept.
    let eff_src = |src: &str, bw: &Option<String>| -> String {
        match (grayscale, bw) {
            (true, Some(b)) => b.clone(),
            _ => src.to_string(),
        }
    };
    let mut i = 0;
    while i < blocks.len() {
        match &blocks[i] {
            Block::Heading { level, text, unnumbered } if *level == 1 => {
                // look ahead for an opening plate image
                if let Some(Block::Image {
                    src, spot: false, alt, width, height, fit, border, bw, ..
                }) = blocks.get(i + 1)
                {
                    // Proof build: drop the full-page chapter plate, keep the heading
                    // (and the plate's alt as a light placeholder).
                    if proof {
                        emit_heading(s, *level, text, *unnumbered);
                        if !alt.trim().is_empty() {
                            s.push_str(&format!(
                                "#align(center, text(fill: luma(45%), style: \"italic\")[[{}]])\n\n",
                                inline(alt.trim())
                            ));
                        }
                        i += 2;
                        continue;
                    }
                    let p = typst_img_path(root, &eff_src(src, bw));
                    let cap = if !captions || alt.trim().is_empty() {
                        "none".to_string()
                    } else {
                        ty_str(alt)
                    };
                    // per-image overrides (`{width=… height=… fit=… border=…}`) beat
                    // the book defaults baked into the #plate helper.
                    let mut a = String::new();
                    if let Some(w) = width { a += &format!(", w: {w}"); }
                    if let Some(h) = height { a += &format!(", h: {h}"); }
                    if let Some(f) = fit { a += &format!(", ft: \"{f}\""); }
                    if *border == Some(false) { a += ", b: false"; }
                    // Alt text for the tag tree (independent of `c`, the *visible*
                    // caption: a full-bleed plate shows no caption but is still described).
                    if !alt.trim().is_empty() { a += &format!(", a: {}", ty_str(alt.trim())); }
                    s.push_str(&format!("#plate({}, c: {}{a})\n", ty_str(&p), cap));
                    emit_heading(s, *level, text, *unnumbered);
                    i += 2;
                    continue;
                }
                emit_heading(s, *level, text, *unnumbered);
                i += 1;
            }
            Block::Heading { level, text, unnumbered } => {
                emit_heading(s, *level, text, *unnumbered);
                i += 1;
            }
            Block::Image { src, spot, width, height, fit, align, bw, alt, .. } => {
                // Proof (proofreading) build: no images, just the alt text — a light
                // italic placeholder where the figure would go, so the reviewer reads
                // clean prose without waiting on image-heavy PDFs. Decorative spot
                // vignettes (usually alt-less) simply drop out.
                if proof {
                    let alt = alt.trim();
                    if !alt.is_empty() {
                        s.push_str(&format!(
                            "#align(center, text(fill: luma(45%), style: \"italic\")[[{}]])\n\n",
                            inline(alt)
                        ));
                    }
                    i += 1;
                    continue;
                }
                let p = typst_img_path(root, &eff_src(src, bw));
                if *spot {
                    s.push_str(&format!("#spot({})\n", ty_str(&p)));
                } else {
                    let w = width.clone().unwrap_or_else(|| "100%".to_string());
                    let mut args = format!("width: {w}");
                    if let Some(h) = height { args += &format!(", height: {h}"); }
                    if let Some(f) = fit { args += &format!(", fit: \"{f}\""); }
                    if !alt.trim().is_empty() { args += &format!(", alt: {}", ty_str(alt.trim())); }
                    let al = align.as_deref().unwrap_or("center");
                    s.push_str(&format!("#align({al}, image({}, {args}))\n", ty_str(&p)));
                }
                i += 1;
            }
            Block::Table { header, rows } => {
                emit_table(s, header, rows);
                i += 1;
            }
            Block::List { ordered, items } => {
                // Native Typst list markup: `- ` bullet / `+ ` numbered, one item
                // per line. Item content reuses the same inline() converter as
                // paragraphs, so bold/italic/links inside a bullet still render.
                for it in items {
                    s.push_str(if *ordered { "+ " } else { "- " });
                    s.push_str(&inline(it));
                    s.push('\n');
                }
                s.push('\n');
                i += 1;
            }
            Block::Rule => {
                s.push_str("#scenebreak\n");
                i += 1;
            }
            Block::Para(t) => {
                s.push_str(&inline(t));
                s.push_str("\n\n");
                i += 1;
            }
            Block::Code { lang, code } => {
                // Typst `raw` highlights syntax natively from `lang` (Ruby/ERB/
                // YAML/Bash/SQL all ship). A tinted, breakable panel wraps it. On a
                // grayscale (B&W print) build we drop the theme so code prints as
                // plain black monospace instead of shipping color into a KDP
                // black-ink interior. The literal fence uses 4 backticks so any
                // 3-backtick run inside the code survives.
                let lang = lang.as_deref().unwrap_or("txt");
                s.push_str(
                    "#block(width: 100%, fill: luma(245), inset: 8pt, radius: 3pt, breakable: true)[\n",
                );
                if grayscale {
                    s.push_str("#set raw(theme: none); #set text(fill: black)\n");
                }
                s.push_str(&format!("````{lang}\n{code}````\n"));
                s.push_str("]\n\n");
                i += 1;
            }
        }
    }
}

/// Emit a GFM table as a Typst `#table(...)` with bold header cells. Column count
/// is taken from the header; short body rows are padded so the grid stays valid.
fn emit_table(s: &mut String, header: &[String], rows: &[Vec<String>]) {
    let cols = header.len().max(1);
    s.push_str(&format!(
        "#table(\n  columns: {cols},\n  table.header({}),\n",
        header
            .iter()
            .map(|c| format!("[#strong[{}]]", inline(c)))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    for row in rows {
        let mut cells: Vec<String> =
            row.iter().map(|c| format!("[{}]", inline(c))).collect();
        while cells.len() < cols {
            cells.push("[]".to_string());
        }
        cells.truncate(cols);
        s.push_str(&format!("  {},\n", cells.join(", ")));
    }
    s.push_str(")\n\n");
}

fn emit_heading(s: &mut String, level: usize, text: &str, unnumbered: bool) {
    let body = inline(text);
    if level == 1 {
        if unnumbered {
            s.push_str(&format!("#heading(level: 1, numbering: none)[{body}]\n"));
        } else {
            s.push_str(&format!("= {body}\n"));
        }
    } else {
        s.push_str(&format!("{} {body}\n", "=".repeat(level)));
    }
}

// ---------- block parsing ----------

enum Block {
    Heading { level: usize, text: String, unnumbered: bool },
    Para(String),
    /// A fenced code block. `lang` is the info-string language (```ruby → "ruby"),
    /// driving Typst's built-in syntax highlighting; `None` renders plain mono.
    Code { lang: Option<String>, code: String },
    /// A standalone image with its pandoc `{…}` attributes. `width`/`height` are
    /// raw Typst dimensions (`"80%"`, `"3in"`); `fit` ∈ cover/contain/stretch;
    /// `align` ∈ left/center/right; `border` toggles the framed-plate keyline
    /// (`None` = book default). All optional so an image with no attrs is unchanged.
    Image {
        src: String,
        alt: String,
        spot: bool,
        width: Option<String>,
        height: Option<String>,
        fit: Option<String>,
        align: Option<String>,
        border: Option<bool>,
        /// Explicit grayscale variant for the print interior (`{bw=path}`). When
        /// set, the KDP/POD print PDF uses this file verbatim instead of
        /// converting `src` on the fly; the EPUB/retail build ignores it and
        /// keeps `src` in color. `None` → bookmill grayscales `src` itself.
        bw: Option<String>,
    },
    Rule,
    /// GFM pipe table: a header row plus body rows, each a vector of raw cell
    /// strings (inline markdown, converted at emit time).
    Table { header: Vec<String>, rows: Vec<Vec<String>> },
    /// A Markdown list. `ordered` selects the Typst marker (`+` numbered vs `-`
    /// bullet); each item is raw inline markdown (converted at emit time). Flat
    /// only — nested/indented sublists are flattened into the one list.
    List { ordered: bool, items: Vec<String> },
}

/// Remove HTML comments `<!-- ... -->` (including multi-line ones) from Markdown
/// before parsing, so pipeline notes carried in frontmatter/chapters (e.g. the
/// copyright page's editing note) never render as body text. Pandoc dropped
/// these; bookmill's block parser would otherwise treat them as paragraphs.
fn strip_html_comments(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start + 4..].find("-->") {
            Some(end) => rest = &rest[start + 4 + end + 3..],
            None => {
                // unterminated comment: drop the remainder
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Split markdown into blocks. Drops fenced code blocks (```; used by the
/// copyright pages to carry raw LaTeX, which Typst must not see). Recognizes
/// ATX headings, `---`/`***` scene-break rules, standalone images, and
/// blank-line-delimited paragraphs.
fn parse_blocks(md: &str) -> Vec<Block> {
    let md = strip_html_comments(md);
    let md = md.as_str();
    let mut out = Vec::new();
    let mut para: Vec<String> = Vec::new();

    let flush = |para: &mut Vec<String>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Para(para.join(" ")));
            para.clear();
        }
    };

    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end();
        let trimmed = line.trim();

        // Fenced code block. A pandoc raw-attribute fence (```{=latex}) is dropped
        // entirely (the copyright pages embed raw LaTeX Typst must never render);
        // any other fence is captured as a Code block, its info-string language
        // driving Typst's syntax highlighting.
        if let Some(rest) = trimmed.strip_prefix("```").or_else(|| trimmed.strip_prefix("~~~")) {
            flush(&mut para, &mut out);
            let info = rest.trim();
            let is_raw = info.starts_with('{'); // ```{=latex} / ```{.foo}
            let lang = (!is_raw && !info.is_empty())
                .then(|| info.split_whitespace().next().unwrap().to_string());
            let mut code = String::new();
            i += 1;
            while i < lines.len() {
                let lt = lines[i].trim();
                if lt.starts_with("```") || lt.starts_with("~~~") {
                    i += 1;
                    break;
                }
                code.push_str(lines[i]); // keep raw indentation
                code.push('\n');
                i += 1;
            }
            if !is_raw {
                out.push(Block::Code { lang, code });
            }
            continue;
        }

        if trimmed.is_empty() {
            flush(&mut para, &mut out);
            i += 1;
            continue;
        }

        // GFM pipe table: a `| … |` header row immediately followed by a
        // `|---|---|` separator row. Consumes the whole contiguous table.
        if is_table_row(trimmed)
            && lines
                .get(i + 1)
                .map(|l| is_table_separator(l.trim()))
                .unwrap_or(false)
        {
            flush(&mut para, &mut out);
            let header = split_table_row(trimmed);
            let mut rows = Vec::new();
            let mut j = i + 2;
            while j < lines.len() {
                let t = lines[j].trim();
                if t.is_empty() || !is_table_row(t) {
                    break;
                }
                rows.push(split_table_row(t));
                j += 1;
            }
            out.push(Block::Table { header, rows });
            i = j;
            continue;
        }

        // scene-break rule: a line of only -, *, or _ (>=3)
        if is_rule(trimmed) {
            flush(&mut para, &mut out);
            out.push(Block::Rule);
            i += 1;
            continue;
        }

        // ATX heading
        if let Some(h) = parse_heading(trimmed) {
            flush(&mut para, &mut out);
            out.push(h);
            i += 1;
            continue;
        }

        // standalone image paragraph
        if let Some(img) = parse_image(trimmed) {
            flush(&mut para, &mut out);
            out.push(img);
            i += 1;
            continue;
        }

        // Markdown list: a run of `- `/`* `/`+ ` (bullet) or `N.`/`N)` (ordered)
        // marker lines. A blank line or the start of another block ends it; an
        // unmarked non-blank line is a lazy continuation of the current item
        // (a wrapped list item). Without this the marker lines fell through to
        // the paragraph accumulator and joined with spaces, so only the first
        // "- " survived as a Typst bullet and the rest printed as literal dashes.
        if let Some((ordered, first)) = list_marker(trimmed) {
            flush(&mut para, &mut out);
            let mut items = vec![first];
            i += 1;
            while i < lines.len() {
                let t = lines[i].trim();
                if t.is_empty() {
                    break;
                }
                if let Some((_, content)) = list_marker(t) {
                    items.push(content);
                } else if is_rule(t)
                    || parse_heading(t).is_some()
                    || parse_image(t).is_some()
                    || t.starts_with("```")
                    || t.starts_with("~~~")
                    || is_table_row(t)
                {
                    break;
                } else if let Some(last) = items.last_mut() {
                    last.push(' ');
                    last.push_str(t);
                }
                i += 1;
            }
            out.push(Block::List { ordered, items });
            continue;
        }

        para.push(line.to_string());
        i += 1;
    }
    flush(&mut para, &mut out);
    out
}

/// If a trimmed line begins a Markdown list item, return `(ordered, content)`
/// with the marker stripped. Bullets are `-`/`*`/`+` followed by a space;
/// ordered items are `<digits>.`/`<digits>)` followed by a space. The marker
/// must be followed by a space so `*emphasis*` and `-3` are not mistaken for
/// list items (a bare `---`/`***` rule is already handled earlier).
fn list_marker(line: &str) -> Option<(bool, String)> {
    for m in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(m) {
            return Some((false, rest.trim_start().to_string()));
        }
    }
    let digits: String = line.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        let after = &line[digits.len()..];
        if let Some(rest) = after.strip_prefix(". ").or_else(|| after.strip_prefix(") ")) {
            return Some((true, rest.trim_start().to_string()));
        }
    }
    None
}

/// A line that looks like a GFM table row: contains a `|` and (after trimming a
/// single optional leading/trailing pipe) is non-empty.
fn is_table_row(s: &str) -> bool {
    let s = s.trim();
    s.contains('|') && s.trim_matches('|').contains(|c| c != '|')
}

/// A GFM header/body separator row, e.g. `|---|:--:|---:|`. Cells contain only
/// `-`, `:`, and spaces, and at least one `-`.
fn is_table_separator(s: &str) -> bool {
    if !s.contains('|') || !s.contains('-') {
        return false;
    }
    split_table_row(s)
        .iter()
        .all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' '))
}

/// Split a `| a | b | c |` row into trimmed cell strings, dropping the optional
/// leading/trailing pipe. Does not handle escaped `\|` inside cells (unused here).
fn split_table_row(s: &str) -> Vec<String> {
    let s = s.trim();
    let s = s.strip_prefix('|').unwrap_or(s);
    let s = s.strip_suffix('|').unwrap_or(s);
    s.split('|').map(|c| c.trim().to_string()).collect()
}

fn is_rule(s: &str) -> bool {
    let s = s.replace(' ', "");
    s.len() >= 3
        && (s.chars().all(|c| c == '-') || s.chars().all(|c| c == '*') || s.chars().all(|c| c == '_'))
}

fn parse_heading(s: &str) -> Option<Block> {
    if !s.starts_with('#') {
        return None;
    }
    let level = s.chars().take_while(|&c| c == '#').count();
    let rest = s[level..].trim_start();
    if rest.is_empty() {
        return None;
    }
    // strip a trailing pandoc attribute block: `# Title {.unnumbered}`
    let (title, attrs) = split_attrs(rest);
    let unnumbered = attrs.contains(".unnumbered");
    Some(Block::Heading {
        level: level.min(6),
        text: title.trim().to_string(),
        unnumbered,
    })
}

/// Parse a standalone `![alt](src){attrs}` line. Returns None if the line is not
/// solely an image.
fn parse_image(s: &str) -> Option<Block> {
    if !s.starts_with("![") {
        return None;
    }
    let close_alt = s.find("](")?;
    let after = &s[close_alt + 2..];
    let close_paren = after.find(')')?;
    let src = after[..close_paren].trim().to_string();
    let tail = after[close_paren + 1..].trim();
    // must be only an image (optionally followed by an attribute block)
    if !tail.is_empty() && !tail.starts_with('{') {
        return None;
    }
    let alt = s[2..close_alt].trim().to_string();
    let (_, attrs) = split_attrs(s);
    let spot = attrs.contains(".spot");
    // `.plain` class (or `border=false`) drops the framed-plate keyline border.
    let border = if attrs.contains(".plain") {
        Some(false)
    } else {
        match attr_str(&attrs, "border").as_deref() {
            Some("false") | Some("no") | Some("off") | Some("none") => Some(false),
            Some("true") | Some("yes") | Some("on") => Some(true),
            _ => None,
        }
    };
    Some(Block::Image {
        src,
        alt,
        spot,
        width: attr_dim(&attrs, "width"),
        height: attr_dim(&attrs, "height"),
        fit: attr_enum(&attrs, "fit", &["cover", "contain", "stretch"]),
        align: attr_enum(&attrs, "align", &["left", "center", "right"]),
        border,
        bw: attr_str(&attrs, "bw"),
    })
}

/// Split a trailing `{ ... }` pandoc attribute block off the end of a line.
/// Returns (content_without_attrs, attrs_inner).
fn split_attrs(s: &str) -> (String, String) {
    let s = s.trim();
    if s.ends_with('}') {
        if let Some(open) = s.rfind('{') {
            return (s[..open].trim().to_string(), s[open + 1..s.len() - 1].to_string());
        }
    }
    (s.to_string(), String::new())
}

/// Raw value token for `key=…` in a pandoc attribute string (stops at the next
/// whitespace). Surrounding quotes are trimmed. Returns None if absent/empty.
fn attr_str(attrs: &str, key: &str) -> Option<String> {
    let pat = format!("{key}=");
    let idx = attrs.find(&pat)?;
    let rest = &attrs[idx + pat.len()..];
    let val: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
    let val = val.trim_matches(|c| c == '"' || c == '\'');
    (!val.is_empty()).then(|| val.to_string())
}

/// A dimension attribute → a Typst length/ratio string. A bare number gets `%`
/// (back-compat with `{width=80}`); a value carrying a unit or `%` passes through
/// (`80%`, `3in`, `4cm`, `120pt`).
fn attr_dim(attrs: &str, key: &str) -> Option<String> {
    let v = attr_str(attrs, key)?;
    if !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.') {
        Some(format!("{v}%"))
    } else {
        Some(v)
    }
}

/// An enum-valued attribute, validated (case-insensitively) against an allow-list.
/// An unrecognized value is ignored rather than passed through to Typst.
fn attr_enum(attrs: &str, key: &str, allowed: &[&str]) -> Option<String> {
    let v = attr_str(attrs, key)?.to_ascii_lowercase();
    allowed.iter().find(|a| **a == v).map(|a| a.to_string())
}

// ---------- inline conversion ----------

/// Convert markdown inline text to Typst markup: `**bold**` -> `*bold*`,
/// `*italic*`/`_italic_` -> `_italic_`, `[t](u)` -> `#link("u")[t]`, everything
/// else escaped as literal text.
fn inline(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // inline footnote ^[text]  ->  #footnote[text]
        // (pandoc-style inline footnote; reference-style GFM `[^id]` is deferred —
        // see module note. No book in the series uses footnotes yet.)
        if c == '^' && chars.get(i + 1) == Some(&'[') {
            if let Some(end) = find_char(&chars, i + 2, ']') {
                let inner: String = chars[i + 2..end].iter().collect();
                out.push_str("#footnote[");
                out.push_str(&inline(&inner));
                out.push(']');
                i = end + 1;
                continue;
            }
        }
        // bold **...** -> #strong[...] (function form, not the `*..*` shorthand, so
        // bold text that starts/ends with `/` can't emit a `*/`/`/*` sequence that
        // Typst would parse as a block comment — matters for tech books).
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            if let Some(end) = find_seq(&chars, i + 2, &['*', '*']) {
                let inner: String = chars[i + 2..end].iter().collect();
                out.push_str("#strong[");
                out.push_str(&inline(&inner));
                out.push(']');
                i = end + 2;
                continue;
            }
        }
        // italic *...* or _..._ -> #emph[...] (function form, same reason as bold)
        if c == '*' || c == '_' {
            if let Some(end) = find_char(&chars, i + 1, c) {
                let inner: String = chars[i + 1..end].iter().collect();
                if !inner.is_empty() {
                    out.push_str("#emph[");
                    out.push_str(&inline(&inner));
                    out.push(']');
                    i = end + 1;
                    continue;
                }
            }
        }
        // link [text](url)
        if c == '[' {
            if let Some((text, url, next)) = parse_inline_link(&chars, i) {
                out.push_str(&format!(
                    "#link({})[{}]",
                    ty_str(&url),
                    inline(&text)
                ));
                i = next;
                continue;
            }
        }
        push_escaped(&mut out, c);
        i += 1;
    }
    out
}

fn find_seq(chars: &[char], from: usize, seq: &[char]) -> Option<usize> {
    let mut i = from;
    while i + seq.len() <= chars.len() {
        if chars[i..i + seq.len()] == *seq {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn find_char(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&i| chars[i] == target)
}

/// Parse `[text](url)` starting at `start` (which must be `[`). Returns
/// (text, url, index_after).
fn parse_inline_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let close = find_char(chars, start + 1, ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let close_paren = find_char(chars, close + 2, ')')?;
    let text: String = chars[start + 1..close].iter().collect();
    let url: String = chars[close + 2..close_paren].iter().collect();
    Some((text, url, close_paren + 1))
}

/// Escape a single char that is special in Typst markup.
fn push_escaped(out: &mut String, c: char) {
    match c {
        // `/` is escaped too: a leading `/ ` is a Typst term-list marker, `//` a
        // line comment, and `/*`/`*/` block-comment delimiters. `\/` renders as a
        // plain slash, so escaping it keeps dates/fractions/code punctuation safe.
        '\\' | '#' | '$' | '*' | '_' | '`' | '<' | '>' | '@' | '[' | ']' | '~' | '^' | '/' => {
            out.push('\\');
            out.push(c);
        }
        _ => out.push(c),
    }
}

/// Quote a string as a Typst string literal.
fn ty_str(s: &str) -> String {
    let esc = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{esc}\"")
}

/// Map a markdown image src to a Typst (`--root`-relative) path. Markdown srcs
/// are relative to the repo root (the build runs there); absolute srcs
/// under the repo are made root-relative; URLs are left untouched.
fn typst_img_path(root: &Path, src: &str) -> String {
    if src.starts_with("http://") || src.starts_with("https://") {
        return src.to_string();
    }
    let p = Path::new(src);
    if p.is_absolute() {
        if let Ok(rel) = p.strip_prefix(root) {
            return format!("/{}", rel.display());
        }
        return src.to_string();
    }
    format!("/{src}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plate/inline image block with the given alt (`spot` = decorative tailpiece).
    fn img(alt: &str, spot: bool) -> Block {
        Block::Image {
            src: "x.png".into(),
            alt: alt.into(),
            spot,
            width: None,
            height: None,
            fit: None,
            align: None,
            border: None,
            bw: None,
        }
    }

    #[test]
    fn pdf_ua_claim_requires_every_content_image_to_be_described() {
        // Every image described -> the book earns the PDF/UA-1 claim.
        assert!(all_images_described(&[img("Un león", false), img("Una rata", false)]));
        // A single undescribed image loses it: Typst *enforces* UA-1, and we would
        // rather ship an honest tagged PDF than assert a standard the book fails.
        assert!(!all_images_described(&[img("Un león", false), img("", false)]));
        assert!(!all_images_described(&[img("   ", false)]));
    }

    #[test]
    fn decorative_spot_needs_no_alt_because_it_ships_as_an_artifact() {
        // A spot tailpiece is marked `pdf.artifact` (the EPUB drops it outright), and
        // an artifact needs no alt — so an alt-less spot must not block the UA-1 claim.
        assert!(all_images_described(&[img("", true)]));
        assert!(all_images_described(&[img("Viñeta", true), img("Un león", false)]));
    }

    #[test]
    fn text_only_book_is_trivially_described() {
        assert!(all_images_described(&[Block::Para("solo texto".into())]));
        assert!(all_images_described(&[]));
    }

    #[test]
    fn is_bw_variant_path_detects_bw_dir() {
        assert!(is_bw_variant_path(Path::new("/repo/libros/x/images/bw/ch01.jpg")));
        assert!(!is_bw_variant_path(Path::new("/repo/libros/x/images/ch01.jpg")));
        // only the immediate parent counts, not a `bw` further up the tree
        assert!(!is_bw_variant_path(Path::new("/repo/bw/images/ch01.jpg")));
    }

    #[test]
    fn is_raster_image_matches_png_and_jpeg_only() {
        assert!(is_raster_image(Path::new("/a/b.png")));
        assert!(is_raster_image(Path::new("/a/b.JPG")));
        assert!(is_raster_image(Path::new("/a/b.jpeg")));
        assert!(!is_raster_image(Path::new("/a/b.svg")));
        assert!(!is_raster_image(Path::new("/a/b")));
    }

    #[test]
    fn grayscale_keeps_png_format_removes_color_preserves_alpha() {
        let mut img = image::RgbaImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgba([200, 10, 10, 255]));
        img.put_pixel(1, 0, image::Rgba([10, 200, 10, 128])); // semi-transparent
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();

        let gray = to_grayscale(&png).expect("grayscale png");
        // Still a PNG (format preserved so Typst's extension-based decode matches).
        assert_eq!(image::guess_format(&gray).unwrap(), image::ImageFormat::Png);
        let out = image::load_from_memory(&gray).unwrap().to_rgba8();
        // Grayscale: r == g == b for every pixel.
        for p in out.pixels() {
            assert_eq!(p.0[0], p.0[1]);
            assert_eq!(p.0[1], p.0[2]);
        }
        // Alpha preserved (cut-out plates must stay transparent).
        assert_eq!(out.get_pixel(1, 0).0[3], 128);
    }

    #[test]
    fn grayscale_keeps_jpeg_format() {
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([200, 10, 10]));
        let mut jpg = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut jpg), image::ImageFormat::Jpeg)
            .unwrap();
        let gray = to_grayscale(&jpg).expect("grayscale jpeg");
        // A .jpg source must stay JPEG bytes, not become PNG.
        assert_eq!(image::guess_format(&gray).unwrap(), image::ImageFormat::Jpeg);
    }

    #[test]
    fn inline_emphasis_and_escape() {
        assert_eq!(inline("plain text"), "plain text");
        assert_eq!(inline("**bold**"), "#strong[bold]");
        assert_eq!(inline("*italic*"), "#emph[italic]");
        // bold starting with `/`: `/` is escaped so it can't form `*/` (comment)
        // or a leading `/ ` term-list marker
        assert_eq!(inline("**/ 5.0**"), "#strong[\\/ 5.0]");
        assert_eq!(inline("a/b"), "a\\/b");
        assert_eq!(inline("a #b [c]"), "a \\#b \\[c\\]");
    }

    #[test]
    fn inline_link() {
        assert_eq!(inline("[t](http://x)"), "#link(\"http://x\")[t]");
    }

    #[test]
    fn heading_unnumbered() {
        match parse_heading("# Epílogo {.unnumbered}").unwrap() {
            Block::Heading { level, text, unnumbered } => {
                assert_eq!(level, 1);
                assert_eq!(text, "Epílogo");
                assert!(unnumbered);
            }
            _ => panic!("not a heading"),
        }
    }

    #[test]
    fn image_spot_and_width() {
        match parse_image("![a](x.png){.spot}").unwrap() {
            Block::Image { src, spot, .. } => {
                assert_eq!(src, "x.png");
                assert!(spot);
            }
            _ => panic!("not an image"),
        }
        // bare number → percent (back-compat); unit values pass through
        match parse_image("![a](y.png){width=80%}").unwrap() {
            Block::Image { width, spot, .. } => {
                assert_eq!(width.as_deref(), Some("80%"));
                assert!(!spot);
            }
            _ => panic!("not an image"),
        }
        match parse_image("![a](z.png){width=80}").unwrap() {
            Block::Image { width, .. } => assert_eq!(width.as_deref(), Some("80%")),
            _ => panic!("not an image"),
        }
    }

    #[test]
    fn image_full_attr_set() {
        match parse_image("![a](p.png){width=3in height=2in fit=contain align=left}").unwrap() {
            Block::Image { width, height, fit, align, border, spot, .. } => {
                assert_eq!(width.as_deref(), Some("3in"));
                assert_eq!(height.as_deref(), Some("2in"));
                assert_eq!(fit.as_deref(), Some("contain"));
                assert_eq!(align.as_deref(), Some("left"));
                assert_eq!(border, None);
                assert!(!spot);
            }
            _ => panic!("not an image"),
        }
        // `.plain` (or border=false) drops the framed keyline; unknown fit ignored
        match parse_image("![a](p.png){.plain fit=bogus}").unwrap() {
            Block::Image { border, fit, .. } => {
                assert_eq!(border, Some(false));
                assert_eq!(fit, None);
            }
            _ => panic!("not an image"),
        }
        match parse_image("![a](p.png){border=false}").unwrap() {
            Block::Image { border, .. } => assert_eq!(border, Some(false)),
            _ => panic!("not an image"),
        }
    }

    #[test]
    fn rule_detect() {
        assert!(is_rule("---"));
        assert!(is_rule("* * *"));
        assert!(!is_rule("-- a"));
    }

    #[test]
    fn inline_footnote() {
        assert_eq!(inline("a^[note]b"), "a#footnote[note]b");
        // a bare caret is escaped (Typst superscript), not treated as a footnote
        assert_eq!(inline("2^3"), "2\\^3");
    }

    #[test]
    fn table_parse_and_emit() {
        let md = "| A | B |\n|---|---|\n| 1 | two |\n| 3 | |\n";
        let blocks = parse_blocks(md);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            Block::Table { header, rows } => {
                assert_eq!(header, &vec!["A".to_string(), "B".to_string()]);
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0], vec!["1".to_string(), "two".to_string()]);
            }
            _ => panic!("not a table"),
        }
        let mut s = String::new();
        emit_blocks(&mut s, &blocks, Path::new("/repo"), true, false, false);
        assert!(s.contains("#table("));
        assert!(s.contains("columns: 2"));
        assert!(s.contains("table.header([#strong[A]], [#strong[B]])"));
        assert!(s.contains("[1], [two]"));
        // a separator line alone (no header above) must not become a table
        assert!(!s.contains("table.header([])"));
    }

    #[test]
    fn table_separator_detection() {
        assert!(is_table_separator("|---|---|"));
        assert!(is_table_separator("| :--- | ---: | :--: |"));
        assert!(!is_table_separator("| a | b |"));
        assert!(!is_table_separator("---")); // scene-break rule, not a table sep
    }

    #[test]
    fn img_path_rooting() {
        let root = Path::new("/repo");
        assert_eq!(typst_img_path(root, "libros/a/ch01.png"), "/libros/a/ch01.png");
        assert_eq!(typst_img_path(root, "/repo/libros/a/ch01.png"), "/libros/a/ch01.png");
    }

    #[test]
    fn bw_attr_parsed_and_swapped_only_for_print() {
        let md = "# T\n\n![a](libros/x/images/ch.jpg){bw=libros/x/images/bw/ch.jpg}\n";
        let blocks = parse_blocks(md);
        // the attribute is parsed onto the image block
        assert!(matches!(
            blocks.iter().find(|b| matches!(b, Block::Image { .. })),
            Some(Block::Image { bw: Some(p), .. }) if p == "libros/x/images/bw/ch.jpg"
        ));
        // color build (grayscale=false) keeps the color src
        let mut color = String::new();
        emit_blocks(&mut color, &blocks, Path::new("/repo"), true, false, false);
        assert!(color.contains("/libros/x/images/ch.jpg"));
        assert!(!color.contains("/libros/x/images/bw/ch.jpg"));
        // print build (grayscale=true) swaps in the bw variant
        let mut print = String::new();
        emit_blocks(&mut print, &blocks, Path::new("/repo"), true, true, false);
        assert!(print.contains("/libros/x/images/bw/ch.jpg"));
    }
}
