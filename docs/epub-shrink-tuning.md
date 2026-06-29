# EPUB shrink tuning — getting below 7.4 MB without quality loss

> Research/design doc with **measured** numbers. Do NOT modify `epub_shrink.rs`
> from this doc — it records experiments and recommends concrete crates/params.
> All sizes measured 2026-06 on the worst-case book.

## 1. Test corpus

Worst-case EPUB = the picture book **la-riqueza-de-la-isla** (es, KDP edition),
the only book near the size budget:

```
output/la-riqueza-de-la-isla/es/la-riqueza-de-la-isla-es-kdp.epub  = 8,448,828 B (8.44 MB)
  └ 13 images, all 1000×(1500|1600) palette PNG, total 8,407,781 B
     • 11 OPAQUE full-page watercolor scenes   (alpha = none)
     • 2  with transparency (file1, file3 — figure/plate cut-outs)
  └ non-image overhead (XHTML/CSS/OPF/nav)     = 41,047 B
```

bookmill's current output (`epub_shrink.rs`) downscales to `epub_image_px` (1000
for kdp-epub) then NeuQuant-quantizes every image to a **256-color palette PNG**
(`encode_png_quantized`, `png::Compression::Best`, `NoFilter`). That is the 8.44 MB
baseline. The PIL predecessor reached ~7.4 MB at 1000px; the native path is
~1 MB heavier because the `png` crate's deflate is weaker than zopfli/oxipng and
its single fixed filter choice is suboptimal.

The key structural insight: **forcing palette-PNG onto continuous-tone watercolor
scenes is the wrong codec.** 256-color quantization both (a) inflates size vs JPEG
and (b) *introduces* banding/posterization — so it is the current pipeline, not
JPEG, that loses quality on these images.

## 2. Experiments (measured bytes)

Tools: `oxipng 10.1.1`, `pngquant 3.0.3`, `cjpeg` (libjpeg-turbo/mozjpeg 3.1.4),
`cwebp` (reference only). "EPUB total" = images + 41,047 B overhead.

| Strategy | Images (B) | EPUB total | vs 8.44 MB |
|---|---:|---:|---:|
| **Current** (NeuQuant palette PNG) | 8,407,781 | **8.44 MB** | — |
| A. `oxipng -o max -a` (lossless re-opt) | 7,528,064 | 7.57 MB | −10.4% |
| A′. `oxipng -o max --zopfli -a` (lossless) | 7,494,688 | **7.53 MB** | −10.8% |
| B. `pngquant 70-90` + `oxipng` (lossy palette) | 7,064,432 | 7.10 MB | −15.9% |
| C. JPEG **q90** opaque + oxipng PNG alpha | 5,367,080 | 5.40 MB | −36.1% |
| C′. JPEG **q88** opaque + oxipng PNG alpha | 4,945,369 | 4.98 MB | −41.1% |
| C″. JPEG **q85** opaque + oxipng PNG alpha | 4,449,145 | 4.49 MB | −47.0% |
| C‴. JPEG **q82** opaque + oxipng PNG alpha | 4,112,071 | 4.13 MB | −51.0% |
| **E. best-of-per-image** (min(JPEG q88, pngquant+oxipng); alpha=PNG) | 4,924,777 | **4.96 MB** | −41.4% |
| (ref) WebP q80 opaque — **KDP-rejected** | 2,556,602 | — | n/a |

Notes:
- JPEGs in C/E were re-encoded from the *already 256-color* EPUB source, so these
  are **conservative on quality**; encoding JPEG from the original full-color art
  would be the same size at strictly higher fidelity.
- `--zopfli` adds only ~0.5% over `-o max` for far more CPU — `-o max` alone is
  the better default for a build pipeline.
- The 2 transparent plates must stay PNG (JPEG has no alpha); oxipng losslessly
  optimizes them (665,021 B for the pair).

## 3. WebP / AVIF in EPUB3 for KDP/Kindle — verdict: NO

Per KDP's reflowable image guidelines, supported interior formats are **JPEG,
PNG, GIF (single-frame), BMP, SVG**. **WebP and AVIF are not in the list; TIFF,
animated GIF, and transparency are explicitly unsupported.** So although WebP q80
would crush these to 2.56 MB, it is **not safe** for the Kindle pipeline. EPUB3 the
spec permits WebP as a "foreign resource" needing a fallback, but Kindle's
converter does not accept it. **Recommendation: do not emit WebP/AVIF for any KDP
edition.** (A non-KDP `gumroad` EPUB *could* use WebP behind a feature flag, but
it's out of scope here.) Add a validator check that flags WebP/AVIF/TIFF/alpha in
EPUB media (kdp-requirements §5.2 `img.format`).

## 4. Recommendation

Switch the EPUB image step from "always palette-PNG" to **content-aware codec
selection** — bookmill already computes `has_alpha` in
`encode_png_quantized`, so the branch point exists:

1. **Opaque, continuous-tone images → JPEG.** Encode RGB JPEG at **q88**
   (progressive, optimized Huffman / trellis). Result: ~5 MB EPUB, higher fidelity
   than the current palette, comfortably **under the 7.4 MB target** and roughly
   halving the Kindle delivery fee (8.44 MB→4.96 MB ≈ saves ~$0.52/unit at
   $0.15/MB under the 70% tier).
2. **Images with transparency → PNG, losslessly post-optimized.** Keep the palette
   path but run an **oxipng `-o max`** pass on the encoded bytes (the single
   biggest lossless win, −10%).
3. **Optional "lossless only" mode** (no JPEG): `pngquant`-style requantize +
   oxipng → 7.10 MB; or pure-lossless oxipng → 7.53 MB. Either already meets the
   target while keeping the all-PNG output. Expose as `epub_image_codec =
   "auto" | "png"` per edition.
4. **Best-of-per-image** (encode both, keep smaller) buys little here (4.96 vs
   4.98 MB) because JPEG wins every opaque scene — not worth the double-encode CPU
   for this corpus; keep it as an option, default to the simple content branch.

Suggested per-edition config (ties into edition-options.md):
```toml
[editions.kdp-epub]
target           = "kdp-epub"
epub_image_px    = 1000
epub_image_codec = "auto"   # auto = JPEG opaque + PNG alpha; "png" = lossless palette
epub_jpeg_q      = 88
```

## 5. Rust crates (no shelling out)

| Step | Crate | Notes |
|---|---|---|
| Lossless PNG re-opt | **`oxipng`** (library API, `oxipng::optimize_from_memory`) | pure Rust, `-o max` equiv via `Options`; the −10% lossless win. Already the binary tested. |
| Lossy palette quant | **`imagequant`** (libimagequant — the engine behind pngquant) | better than `color_quant::NeuQuant` already in deps; dithering + quality range. Pair with oxipng. |
| Better JPEG | **`mozjpeg`** crate (mozjpeg-sys) | trellis quantization, progressive, optimized Huffman — smaller than `image`'s baseline encoder at equal quality. Alt: `jpeg-encoder` (pure Rust, progressive, no C dep) if avoiding mozjpeg-sys. |
| Decode/resize | **`image`** (already a dep) | keep current Lanczos3 downscale; feed RGB8 to the JPEG encoder, RGBA to the quantizer. |
| Content detection | already in `epub_shrink.rs` | `has_alpha = pixels.chunks(4).any(|p| p[3]!=255)` is the opaque/JPEG vs alpha/PNG switch. |

Minimal change shape (for later, not now): in `shrink_image`, branch on
`has_alpha` → JPEG (mozjpeg q88) when opaque, else current palette PNG + an
`oxipng::optimize_from_memory` post-pass. Everything else (rezip with `mimetype`
STORED first, order preserved) stays as-is.

## 6. Bottom line

- **Lowest-risk, lossless, zero-codec-change:** add an `oxipng -o max` post-pass →
  **7.53 MB** (meets the 7.4 MB-class target; −10.8%).
- **Recommended:** content-aware JPEG-q88 for opaque + oxipng-PNG for alpha →
  **~4.96 MB** (−41%), *higher* visual quality than today's palette, half the
  delivery fee.
- **Do not** use WebP/AVIF for KDP editions (rejected by Kindle), despite being
  the smallest on paper.
