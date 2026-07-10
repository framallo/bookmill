# Edition options — exposing KDP book-creation choices in TOML

> Research/design doc. Proposes the `[editions.*]` / `[pdf]` / `[cover]` config
> surface bookmill should expose so every KDP "create a book" choice is driven
> from `bookmill.toml`, and how each maps to the build (geometry, cost,
> eligibility). Defaults keep current behavior byte-identical.

## 1. What KDP asks at book creation (and where bookmill stands today)

| KDP choice | KDP values | Today in bookmill | Gap |
|---|---|---|---|
| Trim size | §1 of kdp-requirements | `edition.trim` / `pdf.trim` / `defaults.trim` | ok |
| Bleed | yes / no | `edition.bleed` / `pdf.bleed` / `defaults.bleed` (string `"0.125in"`/`"0"`) | ok |
| Paper type | white / cream / groundwood | `edition.paper` / `defaults.paper` — **parsed but unused** | **no build/cost effect** |
| Ink / color | black / standard color / premium color | none | **missing** |
| Cover finish | glossy / matte | none | **missing** (listing-only, no build effect) |
| Format | paperback / hardcover / eBook | `edition.target` (`kdp-paperback`/`kdp-hardcover`/`kdp-epub`) | ok |
| ISBN | free KDP / own | `edition.isbn` (`Option<String>`) | stored, not validated/used |
| Spine paper multiplier | 0.002252 / 0.0025 / 0.002347 | `[cover].paper_mult` (single value) | **not derived from paper/ink** |
| Marketplace | US / UK / DE / … | `edition.market` | ok (informational) |
| Digital PDF size | full-res / downsampled | `pdf.digital_pdf_dpi` / `edition.digital_pdf_dpi` (Ghostscript) | ok (implemented) |

The big gaps: **paper type doesn't change anything**, **ink/color doesn't
exist**, and the **spine multiplier is a hand-set constant** instead of being
derived from paper+ink. These matter for cost, eligibility, and spine width.

## 2. Proposed TOML schema additions

All additive and optional; absence preserves today's behavior.

### 2.1 `[editions.<name>]` (repo config)

```toml
[editions.kdp-paperback]
target  = "kdp-paperback"
market  = "US"
trim    = "6x9"
bleed   = "0.125in"        # already supported
paper   = "white"          # white | cream | groundwood   (now load-bearing)
ink     = "black"          # black | standard-color | premium-color   (NEW)
finish  = "matte"          # matte | glossy  (NEW; cover/listing only, no geometry)
isbn    = "free"           # free | "978-..."  (NEW semantics: "free" = KDP-assigned)
```

```toml
[editions.kdp-hardcover]
target = "kdp-hardcover"
trim   = "6x9"
paper  = "white"
ink    = "black"
# spine width is calculator-driven for hardcover (see §4); no multiplier here
```

### 2.2 New types in `config.rs`

```rust
// add to struct Edition
pub ink: Option<String>,     // "black" | "standard-color" | "premium-color"
pub finish: Option<String>,  // "matte" | "glossy"

// optional: a typed paper enum resolved from paper+ink
pub enum PaperInk { BlackWhite, BlackCream, BlackGroundwood, StandardColor, PremiumColor }
```

`PdfOpts` already carries per-book `trim`/`bleed`/`margins`; add a per-book
`paper`/`ink` override mirror so a single book can opt into color without a new
edition. Resolution order stays **edition → book → repo defaults** (as in
`build.rs::resolve_geometry`).

### 2.3 Spine multiplier — derive, don't hardcode

Replace the single `[cover].paper_mult` constant with a derivation from
`paper`+`ink`, with `paper_mult` kept as an explicit override:

```rust
fn spine_mult(paper: &str, ink: &str) -> f64 {
    match (paper, ink) {
        (_, "premium-color")       => 0.002347,
        ("cream", _)               => 0.0025,
        _ /* white/groundwood, black or standard-color */ => 0.002252,
    }
}
```
Then `cover_svg.rs::wrap_svg` uses `r.paper_mult` (override) else
`spine_mult(paper, ink)`. (Hardcover spine is **not** a simple multiplier — §4.)

## 3. How each option maps to the build

| Option | Build effect |
|---|---|
| `trim` | `parse_trim` → paper width/height (already wired) |
| `bleed` | `KdpPdf`: `pw=tw+bleed, ph=th+2*bleed`; `RetailPdf`: bare trim (already wired) |
| `paper` + `ink` | (a) spine multiplier (§2.3); (b) **eligibility/cost validation** (§5); (c) listing record. No interior-geometry change. |
| `finish` | none on geometry; recorded for the listing + (optional) a note in the wrap export filename |
| `isbn` | `"free"` → omit ISBN/barcode area note (KDP prints barcode); explicit ISBN → can render a barcode/leave keep-out; validated as 13-digit if not `"free"` |
| `market` | informational; future per-market trim/ISBN/pricing |
| `digital_pdf_dpi` | `RetailPdf` only: downsample embedded images to N DPI via Ghostscript so the retail/gumroad download stays small; the print interior (`KdpPdf`) keeps full-res. **Implemented** — `pdf_shrink.rs`, resolved edition → book `[pdf]` (`resolve_digital_pdf_dpi`); a missing `gs` warns and keeps the full-res PDF (never fails the build). Also exposed manually as `bookmill build shrink <file.pdf> --dpi N`. |

## 4. Hardcover specifics

- **Eligibility:** `kdp-hardcover` requires **≥ 75 pages**. bookmill should refuse
  to plan a hardcover job (or emit a `fail` check) when the built interior is
  < 75pp. This is the activation gate the repo already documents for la-riqueza
  (60pp → hardcover not yet active).
- **Cover is different:** case-laminate wrap is wider (0.51″ board wrap vs 0.125″
  bleed) and the spine width is **calculator-derived**, not a per-page multiplier.
  This needs a *separate* wrap template (`wrap-hardcover-<lang>.*`) — see
  web-ui-design.md §"hardcover case-laminate wrap". Interior PDF is the **same
  bleed file** as paperback (`build.rs::outputs_for_target` already returns
  `KdpPdf` for both).
- **No dust jacket, no Expanded Distribution, headbands > 120pp** — record-only.

## 5. New validations driven by these options

Add to `config::validate_book` / deep checks (surfaced in the publish checklist):

1. **Page range per paper/ink** (from kdp-requirements §1): e.g. standard-color
   min 72 / max 600; hardcover min 75 / max 550. Needs the built page count. When
   a built interior is out of range, `bookmill build` **warns and skips that print
   edition for that language** (the other editions and every other book still
   build) — one too-short companion never blocks the whole run. `validate --deep`
   still reports it. (Was previously a hard error that failed the entire plan.)
2. **Color trim/ink compatibility:** standard color is white-paper + paperback
   only; flag standard-color on cream or hardcover.
3. **ISBN format:** if `isbn` is not `"free"`, validate ISBN-13 checksum and warn
   that a free KDP ISBN is non-portable / imprint = "Independently published".
4. **Finish** is free-text-checked against `{matte, glossy}`.

## 6. Defaults (no behavior change)

```toml
[defaults]
trim  = "6x9"
bleed = "0"
paper = "white"
ink   = "black"      # NEW default
finish = "matte"     # NEW default
```
- Existing repo `[cover].paper_mult = 0.002252` stays valid as an override;
  derivation only kicks in when `paper_mult` is unset.
- la-riqueza (submitted, 6.125×9.25, 60pp, white/black) resolves to the **same**
  geometry and the **same** 0.002252 spine multiplier — no regression.
