# KDP Publishing Requirements (per format) + Web "Publish Checklist" design

> Research/design doc. Sources are official KDP help pages under
> `kdp.amazon.com/en_US/help` (US marketplace), verified 2026-06. Figures that KDP
> only enforces in the upload form (not printed on a help page) are flagged
> *(form-enforced)*. Where KDP gives a formula it is reproduced verbatim.
>
> This doc feeds two things: (a) the spec tables bookmill validates against, and
> (b) the web **publish-checklist** that maps each requirement to ✓/✗ per format,
> reusing `bookmill validate --deep` (`src/deep.rs`).

---

## 1. Quick per-format spec tables

### 1.1 Paperback

| Item | Spec |
|---|---|
| Page count (black/white paper) | **24–828** |
| Page count (black/cream) | 24–776 |
| Page count (black/groundwood) | 24–812 |
| Page count (premium color/white) | 24–828 |
| Page count (standard color/white) | **72–600** |
| Trim sizes (regular) | 5×8, 5.06×7.81, 5.25×8, 5.5×8.5, **6×9**, 6.14×9.21 |
| Trim sizes (large) | 6.69×9.61, 7×10, 7.44×9.69, 7.5×9.25, 8×10, 8.25×6, 8.25×8.25, 8.27×11.69, 8.5×8.5, 8.5×11 |
| Custom trim | width 4″–8.5″, height 6″–11.69″ |
| "Large" definition (higher print cost) | > 6.12″ wide **or** > 9″ tall |
| Bleed | 0.125″ trimmed top/bottom/outside |
| Full-bleed page (6×9) | **6.125 × 9.25 in** (= 441 × 666 pt) |
| Paper | white / cream / groundwood |
| Ink | black / standard color / premium color |
| Cover finish | glossy or matte (80 lb white stock) |
| Cover format | single **PDF**, ≥ 300 DPI, ≤ 650 MB (≤ 40 MB rec.) |
| Interior format | PDF (required for bleed), or DOC/DOCX/RTF/HTML/TXT (auto-converted) |
| Image DPI | ≥ 300 (≤ 600 to stay under file cap) |
| ISBN | required (free KDP or own); **own per format** |

### 1.2 Hardcover (case laminate)

| Item | Spec |
|---|---|
| Page count (all paper/ink) | **75–550** |
| Trim sizes | 5.5×8.5, 6×9, 6.14×9.21, 7×10, 8.25×11 |
| Bleed | 0.125″ (same as paperback) |
| Construction | case laminate only — **no dust jacket** |
| Cover wrap | image extends **0.51″ (15 mm)** past front edge (wraps the board) |
| Safe margin | text/art ≥ 0.635″ (16 mm) from book edge; ≥ 0.4″ (10 mm) off spine |
| Headbands | added automatically when > 120 pages |
| Spine width | **no published multiplier** — use the KDP Cover Calculator |
| Cover format | single PDF, ≥ 300 DPI |
| Barcode keep-out | 2″ × 1.2″, ≥ 0.76″ from bottom, ≥ 0.25″ from spine hinge |
| Distribution | no Expanded Distribution on hardcover |

### 1.3 Kindle eBook

| Item | Spec |
|---|---|
| Marketing cover | **2560 × 1600 px** (H×W), ratio **1.6:1**, JPEG (TIFF ok), RGB/sRGB, ≤ 5 MB, ≥ 300 DPI |
| Min cover | shortest side ≥ 500 px (else not displayed) |
| Manuscript formats | **EPUB, DOCX, KPF** (MOBI no longer accepted) |
| Interior image formats | JPEG, PNG, GIF (single-frame), BMP, SVG |
| **NOT supported interior** | TIFF, animated GIF, **transparency/alpha**, **WebP**, **AVIF** |
| Color | sRGB (CMYK auto-converted) |
| ISBN | not required (Amazon assigns ASIN) |
| Delivery fee (70% only) | **$0.15/MB** US, min $0.01 → drives EPUB-size budget |

---

## 2. Print geometry & cover formulas (load-bearing)

**Full-bleed interior page** (bleed only on the three outside edges, never the gutter):

```
page_width  = trim_width  + 0.125          (outside edge only)
page_height = trim_height + 0.125 + 0.125   (top + bottom)
```
6×9 → 6.125 × 9.25 in. (bookmill already does this in `build.rs::resolve_paper_dims`:
`KdpPdf` adds `(tw + bleed, th + 2*bleed)`.)

**Paperback wrap cover:**
```
cover_width  = bleed + back_width + spine + front_width + bleed
cover_height = bleed + trim_height + bleed
spine = pages × multiplier
  white = 0.002252″   cream = 0.0025″   premium color = 0.002347″   standard color = 0.002252″
```
- Spine **text** allowed only at **≥ 79 pages**, ≥ 0.0625″ clearance each side.
- Front/back text safe ≥ 0.125″ inside trim.
- Barcode keep-out **~2″ × 1.2″ bottom-right of back cover**.

bookmill matches this in `cover_svg.rs::wrap_svg` (`spine = pages * paper_mult`,
`full_w = 2*trim_w + spine + 2*bleed`, spine text gated on `pages >= 79`). The
default `paper_mult = 0.002252` (white) is in the repo `[cover]`.

**Print royalty:** `royalty = (0.60 × list_price) − printing_cost` (amazon.com;
50% some marketplaces, 40% Expanded Distribution). Trim/bleed/finish do **not**
change printing cost; page count and ink do.

**eBook royalty:** 35% tier = `0.35 × (price − VAT)`, price $0.99–$200 (min rises
with file size: <3 MB→$0.99, 3–10 MB→$1.99, ≥10 MB→$2.99). 70% tier =
`0.70 × (price − VAT − delivery)`, price **$2.99–$9.99**, delivery $0.15/MB, and
price must be ≤ 20% below the lowest print edition.

---

## 3. Metadata / listing fields (all formats)

| Field | Limit / rule |
|---|---|
| Title + subtitle | **combined < 200 characters** |
| Keywords | **7 fields**, **50 chars each** *(form-enforced)*, space-separated; no title/author/category terms, no "book", no quotes, **no HTML** |
| Categories (BISAC) | author selects **up to 3** |
| Description | **≤ 4000 characters** (HTML tags count toward it) |
| Reading age | ranges 0–2, 3–5, 6–8, 9–12, 13–17 (+ adult) |
| Language | ES and EN both fully supported |
| ISBN | eBook none (ASIN); print required; **separate ISBN per format**; free KDP ISBN is KDP-only/non-portable, imprint = "Independently published"; low-content books not eligible for free ISBN |

### 3.1 Allowed HTML tags in the book description (CRITICAL)

KDP's "Write a Book Description" page lists exactly these as **supported**:

```
<br>   <p>…</p>   <b>…</b>   <strong>…</strong>   <i>…</i>   <em>…</em>
<u>…</u>   <h4>…</h4>   <h5>…</h5>   <h6>…</h6>   <ol>…</ol>   <ul>…</ul>   <li>…</li>
```

**NOT supported / cause errors:** `<h1>`, `<h2>`, `<h3>` (only h4–h6 exist),
stray/empty brackets (`<<`, `>>`, `<>`), `< text` with bad spacing, **Unicode
emojis**, and any unclosed/unpaired tag. Tags count against the 4000-char cap.

**bookmill implication:** the `[listing.<lang>].blurb` is currently plain text
validated only for length and forbidden phrases (`config.rs::validate_book`). For
the web UI we should:
1. add an allowed-tag validator (whitelist above; reject `<h1>`–`<h3>`, emojis,
   unbalanced tags) — a new `Issue` in `validate_book`;
2. count characters **including** markup against 4000.

---

## 4. House rules already enforced (keep)

`config.rs::validate_book` + `deep.rs` already cover:
- title present per language; ≤7 keywords ≤50 chars; blurb ≤4000; ≤3 BISAC;
- **forbidden public phrases** ("Animal Farm" / "Rebelión en la granja") in any
  title/subtitle/blurb/keyword;
- (deep) epubcheck clean; PDF page size == trim+bleed (±1.5pt); front cover
  ≥ 1600×2560; wrap PDF landscape ≥400pt.

Gaps to add for full KDP coverage (see §5): page-count min/max per format/paper;
spine/trim eligibility; description HTML whitelist; eBook interior format ban
(WebP/AVIF/alpha/TIFF); cover JPEG ≤5 MB; hardcover ≥75pp gate; image-DPI audit
(the standing TODO in `deep.rs`).

---

## 5. Web "Publish Checklist" — requirement → ✓/✗ per format

The checklist is the publish-readiness view. It runs the **existing**
`bookmill validate --deep` engine and renders one row per requirement, with a
column per active edition/format. Each row resolves to `pass | warn | fail | n/a`.

### 5.1 Data model (server emits JSON; UI renders)

Add a machine-readable mode to deep validation (today `deep.rs` only prints). New
`bookmill validate --deep --json` returns:

```jsonc
{
  "book": "la-riqueza-de-la-isla",
  "formats": {
    "kdp-paperback": {
      "checks": [
        {"id":"pages.min",   "label":"Pages ≥ 24",            "status":"pass", "detail":"60pp"},
        {"id":"pdf.geometry","label":"Page = 6.125×9.25in",   "status":"pass", "detail":"441×666pt"},
        {"id":"cover.wrap",  "label":"Wrap PDF present",       "status":"pass", "detail":"landscape, 953×675pt"},
        {"id":"cover.barcode","label":"Barcode keep-out clear","status":"warn", "detail":"manual confirm"},
        {"id":"spine.text",  "label":"Spine text needs ≥79pp", "status":"n/a",  "detail":"60pp → no spine text"}
      ]
    },
    "kdp-hardcover": { "checks":[ {"id":"pages.min","label":"Pages ≥ 75","status":"fail","detail":"60pp"} ] },
    "kdp-epub": {
      "checks":[
        {"id":"epubcheck","label":"epubcheck clean","status":"pass"},
        {"id":"img.format","label":"No WebP/AVIF/alpha","status":"warn","detail":"2 transparent PNGs"},
        {"id":"epub.size","label":"Delivery size budget","status":"pass","detail":"4.96MB → ~$0.74 fee"},
        {"id":"cover.jpg","label":"Cover JPEG ≤5MB, 1600×2560","status":"pass"}
      ]
    }
  },
  "listing": {
    "es": [
      {"id":"title.len","label":"Title+subtitle <200","status":"pass","detail":"71"},
      {"id":"kw.count","label":"≤7 keywords","status":"pass"},
      {"id":"kw.len","label":"each ≤50 chars","status":"pass"},
      {"id":"desc.len","label":"description ≤4000","status":"pass","detail":"1842"},
      {"id":"desc.html","label":"only allowed HTML tags","status":"pass"},
      {"id":"bisac","label":"≤3 categories","status":"pass"},
      {"id":"forbidden","label":"no forbidden phrases","status":"pass"}
    ]
  }
}
```

### 5.2 Check sources (reuse, don't re-implement)

| Check id | Source | Status mapping |
|---|---|---|
| `title.len` / `kw.*` / `desc.len` / `bisac` / `forbidden` | `config::validate_book` (extend with `title.len`, `desc.html`) | error→fail |
| `epubcheck` | `deep.rs::epub_check` | non-zero→fail, warnings→warn |
| `pdf.geometry` | `deep.rs::pdf_geometry_check` | mismatch→fail |
| `cover.front` / `cover.wrap` | `deep.rs::cover_checks` | low-res→warn |
| `pages.min` / `pages.max` | new: `pdfinfo` page count vs §1 table per paper/ink | below min→fail |
| `spine.text` | new: `pages ≥ 79` (paperback) | else→n/a |
| `img.format` | new: scan EPUB media for WebP/AVIF/TIFF/animated-GIF/alpha | any→warn(KDP-risky) |
| `epub.size` | new: file bytes → `$0.15/MB` delivery estimate | over budget→warn |
| `cover.jpg` | new: front JPEG ≤5 MB and ≥1600×2560, RGB | else→warn/fail |
| `cover.barcode` | manual (can't auto-verify) → always `warn`/manual checkbox | — |
| `img.dpi` | known TODO in `deep.rs` (needs PDF page model) | omitted v1 |

`n/a` is used when a check doesn't apply to a format (e.g. spine text on a 60pp
book, ISBN on an eBook).

### 5.3 UI panel

A right-hand **Publish** panel beside the editor/previewer:

- **Format tabs** (Paperback / Hardcover / Kindle) + a **Listing** tab per
  language. Each shows its checklist rows with ✓ / ! / ✗ icons and the `detail`.
- A top **traffic light** per format: green only if zero `fail`.
- **Rendered description preview:** the panel renders the `blurb` HTML through the
  KDP-allowed-tag whitelist (sanitize, strip disallowed tags, show a live char
  count *including markup*) so the user sees exactly what the Amazon detail page
  will show, with a **Copy** button.
- **Copy-ready listing block:** title, subtitle, 7 keywords (one per line),
  BISAC codes + human labels, reading age, price/royalty line (from the
  informational `[pricing.*]`), and the sanitized description — each with a copy
  button, so the user can paste straight into the KDP web form.
- A **Re-validate** button → `POST /api/validate?deep=1` (background job; streams
  the deep checks as they complete, mirroring the queue events in `build.rs`).

### 5.4 Minimal v1 vs full

- **v1:** `validate --deep --json`; render existing checks (config + epubcheck +
  geometry + cover-res) + page-count + description-HTML whitelist + sanitized
  description preview + copy-ready listing. Barcode + DPI = manual checkboxes.
- **Full:** EPUB interior-format scan (WebP/AVIF/alpha), delivery-fee estimate,
  cover JPEG byte/dimension/colorspace audit, hardcover ≥75pp gate, image-DPI
  audit once a native PDF page model exists.

---

## Source URLs

- Print Options (trim/pages/paper/ink): https://kdp.amazon.com/en_US/help/topic/G201834180
- Trim, Bleed, Margins: https://kdp.amazon.com/en_US/help/topic/GVBQ3CMEQW3W2VL6
- Paperback Submission Guidelines: https://kdp.amazon.com/en_US/help/topic/G201857950
- Create a Paperback Cover (spine/bleed/barcode): https://kdp.amazon.com/en_US/help/topic/G201953020
- Create a Hardcover Cover: https://kdp.amazon.com/en_US/help/topic/GDTKFJPNQCBTMRV6
- eBook Cover Image Guidelines: https://kdp.amazon.com/en_US/help/topic/G6GTK3T3NUHKLEFX
- eBook manuscript formats: https://kdp.amazon.com/en_US/help/topic/G200634390
- Reflowable image guidelines: https://kdp.amazon.com/en_US/help/topic/G75V4YX5X8GRGXWV
- Metadata Guidelines: https://kdp.amazon.com/en_US/help/topic/G201097560
- Keywords: https://kdp.amazon.com/en_US/help/topic/G201298500
- Write a Book Description (allowed HTML): https://kdp.amazon.com/en_US/help/topic/G201189630
- Reading age: https://kdp.amazon.com/en_US/help/topic/G201506310
- ISBN: https://kdp.amazon.com/en_US/help/topic/G201834170
- eBook royalties / delivery: https://kdp.amazon.com/en_US/help/topic/G200644210
- Paperback royalty: https://kdp.amazon.com/en_US/help/topic/G201834330
- Cover Calculator (hardcover spine): https://kdp.amazon.com/cover-calculator
