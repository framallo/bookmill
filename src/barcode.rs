//! EAN-13 barcode generator (pure Rust → SVG fragment).
//!
//! Renders a book's ISBN-13 as a scannable Bookland EAN-13 barcode: white
//! quiet-zone box, the 95-module bar pattern, human-readable digits below the
//! bars (leading digit in the left quiet zone, `>` end sentinel in the right),
//! and an "ISBN 979-8-…" caption line underneath — the standard back-cover
//! block. Geometry is parameterized so the cover renderer can place/scale it.
//!
//! Encoding: 3 guard bars `101`, 6 left digits (L/G parity chosen by the first
//! digit), center guard `01010`, 6 right digits (R codes), end guard `101`.

/// L-codes (odd parity) for digits 0-9.
const L: [&str; 10] = [
    "0001101", "0011001", "0010011", "0111101", "0100011",
    "0110001", "0101111", "0111011", "0110111", "0001011",
];
/// G-codes (even parity).
const G: [&str; 10] = [
    "0100111", "0110011", "0011011", "0100001", "0011101",
    "0111001", "0000101", "0010001", "0001001", "0010111",
];
/// R-codes (right half).
const R: [&str; 10] = [
    "1110010", "1100110", "1101100", "1000010", "1011100",
    "1001110", "1010000", "1000100", "1001000", "1110100",
];
/// First-digit → parity pattern of the 6 left-half digits (L=odd, G=even).
const PARITY: [&str; 10] = [
    "LLLLLL", "LLGLGG", "LLGGLG", "LLGGGL", "LGLLGG",
    "LGGLLG", "LGGGLL", "LGLGLG", "LGLGGL", "LGGLGL",
];

/// The 95-char module pattern ('0'/'1') for 13 digits, or None if input isn't
/// 13 digits after stripping separators.
fn modules(isbn: &str) -> Option<(Vec<u8>, String)> {
    let digits: Vec<u8> = isbn
        .chars()
        .filter(|c| c.is_ascii_digit())
        .map(|c| c as u8 - b'0')
        .collect();
    if digits.len() != 13 {
        return None;
    }
    let mut m = String::with_capacity(95);
    m.push_str("101");
    let parity = PARITY[digits[0] as usize];
    for (i, d) in digits[1..7].iter().enumerate() {
        let code = if parity.as_bytes()[i] == b'L' { L[*d as usize] } else { G[*d as usize] };
        m.push_str(code);
    }
    m.push_str("01010");
    for d in &digits[7..13] {
        m.push_str(R[*d as usize]);
    }
    m.push_str("101");
    Some((digits, m))
}

/// Render the barcode block as an SVG `<g>` fragment.
///
/// `(x, y)` is the top-left of the white box; `w` its width in SVG units. The
/// box height is derived (bars + digits + caption ≈ 0.62 × w). `isbn_display`
/// is the hyphenated form for the caption (falls back to the raw digits).
pub fn ean13_svg(isbn: &str, x: f64, y: f64, w: f64) -> Option<String> {
    let (digits, m) = modules(isbn)?;
    let display: String = {
        let d = isbn.trim();
        if d.chars().filter(|c| c.is_ascii_digit()).count() == 13 && d.contains('-') {
            d.to_string()
        } else {
            let s: String = digits.iter().map(|d| (d + b'0') as char).collect();
            format!("{}-{}-{}-{}-{}", &s[..3], &s[3..4], &s[4..7], &s[7..12], &s[12..])
        }
    };
    // proportions (in units of box width)
    let pad = 0.06 * w; // quiet zone
    let bar_w = (w - 2.0 * pad) / 106.0; // 95 modules + leading digit gutter (11 modules)
    let gutter = 11.0 * bar_w; // left space for the leading human digit
    let bar_h = 0.34 * w;
    let digit_size = 0.075 * w;
    let caption_size = 0.062 * w;
    let box_h = pad + bar_h + digit_size * 1.25 + caption_size * 1.6 + pad;
    let mut s = String::new();
    s.push_str(&format!(
        "<g><rect x=\"{x:.2}\" y=\"{y:.2}\" width=\"{w:.2}\" height=\"{box_h:.2}\" fill=\"#ffffff\"/>\n"
    ));
    let bx = x + pad + gutter;
    let by = y + pad;
    // bars; guards (positions 0-2, 45-49, 92-94) extend below into the digit line
    for (i, c) in m.chars().enumerate() {
        if c != '1' {
            continue;
        }
        let guard = i < 3 || (45..50).contains(&i) || i >= 92;
        let h = if guard { bar_h + digit_size * 0.6 } else { bar_h };
        s.push_str(&format!(
            "<rect x=\"{:.2}\" y=\"{by:.2}\" width=\"{:.2}\" height=\"{h:.2}\" fill=\"#000000\"/>\n",
            bx + i as f64 * bar_w,
            bar_w * 1.02,
        ));
    }
    // human-readable digits: leading digit left of bars; two 6-digit groups
    let dy = by + bar_h + digit_size;
    let font = "font-family=\"Montserrat\"";
    let s13: String = digits.iter().map(|d| (d + b'0') as char).collect();
    s.push_str(&format!(
        "<text x=\"{:.2}\" y=\"{dy:.2}\" {font} font-size=\"{digit_size:.2}\" fill=\"#000\">{}</text>\n",
        x + pad * 0.7,
        &s13[..1]
    ));
    let group_w = 42.0 * bar_w;
    let g1x = bx + 3.0 * bar_w + group_w / 2.0;
    let g2x = bx + 50.0 * bar_w + group_w / 2.0;
    s.push_str(&format!(
        "<text x=\"{g1x:.2}\" y=\"{dy:.2}\" {font} font-size=\"{digit_size:.2}\" letter-spacing=\"{:.2}\" text-anchor=\"middle\" fill=\"#000\">{}</text>\n",
        bar_w * 1.5,
        &s13[1..7]
    ));
    s.push_str(&format!(
        "<text x=\"{g2x:.2}\" y=\"{dy:.2}\" {font} font-size=\"{digit_size:.2}\" letter-spacing=\"{:.2}\" text-anchor=\"middle\" fill=\"#000\">{}</text>\n",
        bar_w * 1.5,
        &s13[7..13]
    ));
    // '>' end sentinel in the right quiet zone
    s.push_str(&format!(
        "<text x=\"{:.2}\" y=\"{dy:.2}\" {font} font-size=\"{digit_size:.2}\" fill=\"#000\">&gt;</text>\n",
        bx + 95.0 * bar_w + bar_w * 2.0
    ));
    // ISBN caption centered below
    s.push_str(&format!(
        "<text x=\"{:.2}\" y=\"{:.2}\" {font} font-size=\"{caption_size:.2}\" letter-spacing=\"{:.2}\" text-anchor=\"middle\" fill=\"#000\">ISBN {display}</text>\n",
        x + w / 2.0,
        dy + caption_size * 1.4,
        caption_size * 0.08,
    ));
    s.push_str("</g>\n");
    Some(s)
}

/// Height of the barcode box for a given width (same derivation as `ean13_svg`).
pub fn box_height(w: f64) -> f64 {
    let pad = 0.06 * w;
    pad + 0.34 * w + 0.075 * w * 1.25 + 0.062 * w * 1.6 + pad
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_the_book_isbn() {
        // the EN paperback ISBN from the cover: 979-8-234-07044-9
        let (digits, m) = modules("979-8-234-07044-9").unwrap();
        assert_eq!(digits.len(), 13);
        assert_eq!(m.len(), 95);
        assert!(m.starts_with("101") && m.ends_with("101"));
        assert_eq!(&m[45..50], "01010");
        // svg renders
        let svg = ean13_svg("979-8-234-07044-9", 0.0, 0.0, 200.0).unwrap();
        assert!(svg.contains("ISBN 979-8-234-07044-9"));
        assert!(svg.matches("<rect").count() > 20);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(ean13_svg("12345", 0.0, 0.0, 100.0).is_none());
        assert!(ean13_svg("free", 0.0, 0.0, 100.0).is_none());
    }
}
