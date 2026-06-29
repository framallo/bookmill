# Bundled cover fonts

The native (Chrome-free) resvg cover renderer (`src/cover_svg.rs`) needs real
font files on disk — unlike the HTML/Chrome path, it cannot pull web fonts from
the Google Fonts CDN at render time. These TrueType files are `include_bytes!`'d
into the `bookmill` binary, so covers render identically after `cargo install`
with no system fonts required.

## Provenance

All files are the static per-weight TrueType instances published by
[Fontsource](https://fontsource.org) (the same upstream as Google Fonts),
downloaded from the jsdelivr CDN:

```
https://cdn.jsdelivr.net/fontsource/fonts/<family>@latest/latin-<weight>-<style>.ttf
```

| Family          | Weights / styles bundled        | Used by (cover `serif`/`subfont`) |
|-----------------|---------------------------------|-----------------------------------|
| Montserrat      | 400/500/600 normal, 400/500 italic | badge + author (always); default subtitle |
| Playfair Display| 400/700/800 normal              | default `serif` (novel + most companions) |
| Baloo 2         | 600/700/800 normal              | `serif` for la-riqueza / kids' books |
| Patrick Hand    | 400 normal                      | `subfont` for la-riqueza / kids' books |
| Oswald          | 500/600/700 normal              | `serif` for historias-absurdas |

## Licensing

Montserrat, Playfair Display, Baloo 2, Patrick Hand, and Oswald are all licensed
under the SIL Open Font License 1.1 — free to bundle and redistribute with the
binary. If you add a cover that uses a new family, drop its static TTF here and
register it in the `FONTS` table in `src/cover_svg.rs`.
