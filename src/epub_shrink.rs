//! Native, Python-free EPUB image shrinker — replaces
//! `scripts/shrink-epub-images.py`.
//!
//! Downscales images inside an EPUB so the Kindle delivery file stays small,
//! without touching the print PDF (which uses the full-res originals). The codec
//! is chosen per image (content-aware — see `docs/epub-shrink-tuning.md`):
//!   * resize any image wider than `max_w` (Lanczos), preserving aspect;
//!   * **opaque** images -> RGB JPEG q88 (mozjpeg: progressive, trellis,
//!     optimized Huffman). A `.png` that turns into JPEG is renamed to `.jpg`
//!     and every reference (OPF manifest href + media-type, XHTML/NCX/CSS
//!     src/href) is rewritten so the EPUB stays epubcheck-clean;
//!   * **images with transparency** -> 256-color palette PNG (keeps alpha as a
//!     tRNS palette so transparent chapter plates stay transparent), then a
//!     lossless `oxipng -o max` post-pass;
//!   * the **cover image** is left untouched (no recompression, no rename);
//!   * rezip with `mimetype` first and STORED (uncompressed), everything else
//!     DEFLATED, preserving the original entry order.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;

/// JPEG quality for opaque images (measured sweet spot: ~5 MB EPUB, higher
/// fidelity than the old 256-color palette — see docs/epub-shrink-tuning.md).
const JPEG_QUALITY: f32 = 88.0;

/// Shrink images inside `epub` in place. Images no wider than `max_w` are only
/// recompressed; wider ones are downscaled first.
pub fn shrink_epub(epub: &Path, max_w: u32) -> Result<(u64, u64)> {
    let before = std::fs::metadata(epub).map(|m| m.len()).unwrap_or(0);

    // Read every entry into memory, preserving order (EPUBs are small).
    let bytes = std::fs::read(epub).with_context(|| format!("reading {}", epub.display()))?;
    let reader = std::io::Cursor::new(&bytes);
    let mut zin = zip::ZipArchive::new(reader).context("opening epub zip")?;

    struct Entry {
        name: String,
        is_dir: bool,
        data: Vec<u8>,
    }
    let mut entries: Vec<Entry> = Vec::with_capacity(zin.len());
    for i in 0..zin.len() {
        let mut f = zin.by_index(i)?;
        let name = f.name().to_string();
        let is_dir = f.is_dir();
        let mut data = Vec::with_capacity(f.size() as usize);
        if !is_dir {
            f.read_to_end(&mut data)?;
        }
        entries.push(Entry { name, is_dir, data });
    }

    // The cover image is left untouched (constraint: don't touch cover assets;
    // also avoids rewriting the SVG cover wrapper). Locate it from the OPF.
    let cover_href = find_cover_image(&entries.iter().map(|e| (e.name.as_str(), e.data.as_slice())).collect::<Vec<_>>());

    // Map of renamed entries (old zip path -> new zip path) for opaque PNGs that
    // became JPEGs; used afterwards to rewrite references.
    let mut renames: HashMap<String, String> = HashMap::new();

    // Transform image entries.
    for e in entries.iter_mut() {
        if e.is_dir {
            continue;
        }
        let lower = e.name.to_ascii_lowercase();
        let is_png = lower.ends_with(".png");
        let is_jpg = lower.ends_with(".jpg") || lower.ends_with(".jpeg");
        if !is_png && !is_jpg {
            continue;
        }
        // The cover keeps its prior handling: downscale + palette PNG (never a
        // lossy JPEG re-encode, never renamed) so the cover asset's format and
        // character are untouched — only delivery size is trimmed.
        let is_cover = cover_href.as_deref() == Some(e.name.as_str());
        match shrink_image(&e.data, max_w, is_png, is_cover) {
            Ok(Some((new_data, new_ext))) => {
                e.data = new_data;
                if let Some(ext) = new_ext {
                    let new_name = swap_ext(&e.name, ext);
                    if new_name != e.name {
                        renames.insert(e.name.clone(), new_name.clone());
                        e.name = new_name;
                    }
                }
            }
            Ok(None) => {} // keep original (e.g. decode failed gracefully)
            Err(_) => {}   // never fail the build on a single image
        }
    }

    // Rewrite references for any .png -> .jpg renames so the OPF media-types and
    // every src/href stay consistent (keeps the EPUB epubcheck-clean).
    if !renames.is_empty() {
        // basename old -> basename new (XHTML/NCX/CSS use relative paths, but
        // the trailing filename is what's stable to swap).
        let base_map: HashMap<String, String> = renames
            .iter()
            .map(|(o, n)| (basename(o).to_string(), basename(n).to_string()))
            .collect();
        let opf_re = regex::Regex::new(r#"<item\b[^>]*?/?>"#).unwrap();
        let href_re = regex::Regex::new(r#"href="([^"]*)""#).unwrap();
        let mt_re = regex::Regex::new(r#"media-type="[^"]*""#).unwrap();

        for e in entries.iter_mut() {
            if e.is_dir {
                continue;
            }
            let lower = e.name.to_ascii_lowercase();
            if lower.ends_with(".opf") {
                if let Ok(s) = std::str::from_utf8(&e.data) {
                    let out = rewrite_opf(s, &base_map, &opf_re, &href_re, &mt_re);
                    e.data = out.into_bytes();
                }
            } else if is_text_ref(&lower) {
                if let Ok(s) = std::str::from_utf8(&e.data) {
                    let mut out = s.to_string();
                    for (old, new) in &base_map {
                        out = out.replace(old.as_str(), new.as_str());
                    }
                    e.data = out.into_bytes();
                }
            }
        }
    }

    // Rezip: mimetype first + STORED, the rest DEFLATED, original order.
    let out_path = epub.with_extension("epub.tmp");
    {
        let file = std::fs::File::create(&out_path)
            .with_context(|| format!("creating {}", out_path.display()))?;
        let mut zout = zip::ZipWriter::new(std::io::BufWriter::new(file));
        let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(9));

        if let Some(mt) = entries.iter().find(|e| e.name == "mimetype") {
            zout.start_file("mimetype", stored)?;
            zout.write_all(&mt.data)?;
        }
        for e in &entries {
            if e.name == "mimetype" {
                continue;
            }
            if e.is_dir {
                zout.add_directory(e.name.trim_end_matches('/'), deflated)?;
            } else {
                zout.start_file(&e.name, deflated)?;
                zout.write_all(&e.data)?;
            }
        }
        zout.finish()?;
    }
    std::fs::rename(&out_path, epub)
        .with_context(|| format!("replacing {}", epub.display()))?;

    let after = std::fs::metadata(epub).map(|m| m.len()).unwrap_or(before);
    Ok((before, after))
}

/// The filename portion of a zip entry path (after the last `/`).
fn basename(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// Replace a zip entry's extension (e.g. `media/file0.png` -> `media/file0.jpg`).
fn swap_ext(name: &str, new_ext: &str) -> String {
    match name.rfind('.') {
        Some(i) => format!("{}.{}", &name[..i], new_ext),
        None => format!("{name}.{new_ext}"),
    }
}

/// Text entry kinds that may reference images by relative path.
fn is_text_ref(lower: &str) -> bool {
    lower.ends_with(".xhtml")
        || lower.ends_with(".html")
        || lower.ends_with(".htm")
        || lower.ends_with(".ncx")
        || lower.ends_with(".css")
        || lower.ends_with(".svg")
        || lower.ends_with(".smil")
}

fn media_type_for(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        "image/png"
    }
}

/// Find the cover image's zip path: prefer the manifest item with
/// `properties="cover-image"`, else the `<meta name="cover" content="ID">` item.
fn find_cover_image(entries: &[(&str, &[u8])]) -> Option<String> {
    let (opf_name, opf_bytes) = entries
        .iter()
        .find(|(n, _)| n.to_ascii_lowercase().ends_with(".opf"))?;
    let opf = std::str::from_utf8(opf_bytes).ok()?;
    let opf_dir = match opf_name.rfind('/') {
        Some(i) => &opf_name[..=i], // includes trailing '/'
        None => "",
    };
    let item_re = regex::Regex::new(r#"<item\b[^>]*?/?>"#).ok()?;
    let attr = |tag: &str, key: &str| -> Option<String> {
        let re = regex::Regex::new(&format!(r#"{key}="([^"]*)""#)).ok()?;
        re.captures(tag).map(|c| c[1].to_string())
    };

    // 1) properties="cover-image"
    for cap in item_re.captures_iter(opf) {
        let tag = &cap[0];
        if tag.contains("cover-image") {
            if let Some(href) = attr(tag, "href") {
                return Some(format!("{opf_dir}{href}"));
            }
        }
    }
    // 2) <meta name="cover" content="ID"> -> item with that id
    let meta_re = regex::Regex::new(r#"<meta\b[^>]*name="cover"[^>]*content="([^"]*)""#).ok()?;
    let cover_id = meta_re.captures(opf).map(|c| c[1].to_string());
    if let Some(id) = cover_id {
        for cap in item_re.captures_iter(opf) {
            let tag = &cap[0];
            if attr(tag, "id").as_deref() == Some(id.as_str()) {
                if let Some(href) = attr(tag, "href") {
                    return Some(format!("{opf_dir}{href}"));
                }
            }
        }
    }
    None
}

/// Rewrite the OPF manifest: for any item whose href basename was renamed, swap
/// the href filename and set the matching media-type.
fn rewrite_opf(
    opf: &str,
    base_map: &HashMap<String, String>,
    item_re: &regex::Regex,
    href_re: &regex::Regex,
    mt_re: &regex::Regex,
) -> String {
    item_re
        .replace_all(opf, |caps: &regex::Captures| {
            let tag = &caps[0];
            let Some(hcap) = href_re.captures(tag) else {
                return tag.to_string();
            };
            let href = hcap[1].to_string();
            let base = basename(&href);
            let Some(new_base) = base_map.get(base) else {
                return tag.to_string();
            };
            let new_href = href.replace(base, new_base);
            let mt = media_type_for(new_base);
            let t = tag.replace(
                &format!("href=\"{href}\""),
                &format!("href=\"{new_href}\""),
            );
            mt_re
                .replace(&t, format!("media-type=\"{mt}\"").as_str())
                .to_string()
        })
        .to_string()
}

/// Resize/recompress a single image. Returns `Some((bytes, new_ext))` with the
/// new encoding (`new_ext = Some("jpg")` when an opaque PNG was re-encoded as
/// JPEG and must be renamed; `None` keeps the original extension), or `None` if
/// the image couldn't be decoded (caller keeps the original).
fn shrink_image(
    data: &[u8],
    max_w: u32,
    is_png: bool,
    force_png: bool,
) -> Result<Option<(Vec<u8>, Option<&'static str>)>> {
    let img = match image::load_from_memory(data) {
        Ok(i) => i,
        Err(_) => return Ok(None),
    };
    let (w, h) = (img.width(), img.height());
    let img = if w > max_w {
        let nh = ((h as u64 * max_w as u64 + w as u64 / 2) / w as u64).max(1) as u32;
        img.resize_exact(max_w, nh, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    if is_png {
        // Content-aware: opaque -> JPEG (smaller, no banding); alpha -> PNG.
        let rgba = img.to_rgba8();
        let has_alpha = rgba.as_raw().chunks_exact(4).any(|p| p[3] != 255);
        if has_alpha || force_png {
            let png = encode_png_quantized(&img)?;
            let png = oxipng::optimize_from_memory(&png, &oxipng::Options::max_compression())
                .unwrap_or(png);
            Ok(Some((png, None)))
        } else {
            let jpg = encode_jpeg(&img, JPEG_QUALITY)?;
            Ok(Some((jpg, Some("jpg"))))
        }
    } else {
        // Existing JPEG: re-encode at q88 (keep extension).
        let jpg = encode_jpeg(&img, JPEG_QUALITY)?;
        Ok(Some((jpg, None)))
    }
}

/// Encode an opaque image as a progressive RGB JPEG with mozjpeg (trellis
/// quantization + optimized Huffman tables — smaller than baseline at equal
/// quality).
fn encode_jpeg(img: &image::DynamicImage, quality: f32) -> Result<Vec<u8>> {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<Vec<u8>> {
        let mut comp = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
        comp.set_size(w, h);
        comp.set_quality(quality);
        comp.set_progressive_mode();
        let mut started = comp.start_compress(Vec::new()).context("jpeg start")?;
        started
            .write_scanlines(rgb.as_raw())
            .context("jpeg scanlines")?;
        started.finish().context("jpeg finish")
    }));
    match result {
        Ok(r) => r,
        Err(_) => anyhow::bail!("mozjpeg panicked"),
    }
}

/// Encode a 256-color palette PNG (NeuQuant), preserving alpha via a tRNS palette
/// when the source has transparency — mirrors PIL's `quantize(256, FASTOCTREE)`.
/// Callers run an oxipng `-o max` pass over the result for the lossless win.
fn encode_png_quantized(img: &image::DynamicImage) -> Result<Vec<u8>> {
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let pixels = rgba.as_raw(); // RGBA, 4 bytes/pixel
    let has_alpha = pixels.chunks_exact(4).any(|p| p[3] != 255);

    // Build a 256-color palette over RGBA samples. sample_fac 10 ~ PIL quality
    // and fast (lower = slower/higher quality, but 1 is impractically slow).
    let nq = color_quant::NeuQuant::new(10, 256, pixels);
    let pal_rgba = nq.color_map_rgba(); // 256 * 4
    let n = pal_rgba.len() / 4;

    // Sort the palette so perceptually-similar colors get adjacent indices: this
    // makes the index image low-frequency and far more compressible (NeuQuant's
    // native ordering is arbitrary). `remap[old] = new`.
    let mut order: Vec<usize> = (0..n).collect();
    let lum = |i: usize| {
        let c = &pal_rgba[i * 4..i * 4 + 4];
        // weight by alpha so transparent entries cluster together
        (c[3] as u32) << 16 | (299 * c[0] as u32 + 587 * c[1] as u32 + 114 * c[2] as u32) / 1000
    };
    order.sort_by_key(|&i| lum(i));
    let mut remap = vec![0u8; n];
    let mut plte: Vec<u8> = Vec::with_capacity(n * 3);
    let mut trns: Vec<u8> = Vec::with_capacity(n);
    for (new_idx, &old_idx) in order.iter().enumerate() {
        remap[old_idx] = new_idx as u8;
        let c = &pal_rgba[old_idx * 4..old_idx * 4 + 4];
        plte.extend_from_slice(&c[0..3]);
        trns.push(c[3]);
    }

    // Map each pixel to its nearest palette index (then through the sort remap).
    let mut indices = vec![0u8; (w * h) as usize];
    for (i, px) in pixels.chunks_exact(4).enumerate() {
        indices[i] = remap[nq.index_of(px)];
    }

    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Indexed);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Best);
        // Indexed/palette PNGs compress best with NO row filter — filtering
        // palette index values is meaningless and inflates the stream. (This is
        // what PIL's `optimize=True` settles on for palette images.)
        enc.set_filter(png::FilterType::NoFilter);
        enc.set_adaptive_filter(png::AdaptiveFilterType::NonAdaptive);
        enc.set_palette(plte);
        if has_alpha {
            // Trailing fully-opaque entries can be omitted from tRNS.
            let last_non_opaque = trns.iter().rposition(|&a| a != 255).map(|i| i + 1).unwrap_or(0);
            if last_non_opaque > 0 {
                enc.set_trns(trns[..last_non_opaque].to_vec());
            }
        }
        let mut writer = enc.write_header().context("png header")?;
        writer.write_image_data(&indices).context("png data")?;
    }
    Ok(out)
}
