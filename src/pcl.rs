//! PCL5e page generation: resident-font text and raster graphics.
//!
//! Text is sent using the printer's built-in scalable fonts (the printer
//! rasterizes glyphs itself), and images are dithered to 1-bit and transferred
//! as PCL raster graphics using TIFF packbits (compression method 2). The
//! HL-2270DW's PCL5e raster engine renders at 300 dpi, so raster images are
//! sized for 300 dpi.

use crate::error::{Error, Result};
use crate::job::{JobOptions, Orientation};

/// A resident font selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Font {
    /// PCL typeface family code.
    pub typeface: u16,
    /// Height in points.
    pub point_size: u16,
    /// Pitch in characters per inch (`None` = proportional).
    pub pitch: Option<u16>,
    pub bold: bool,
    pub italic: bool,
}

impl Font {
    pub fn courier(point_size: u16, pitch: u16) -> Self {
        Font { typeface: 4148, point_size, pitch: Some(pitch), bold: false, italic: false }
    }
    pub fn times(point_size: u16) -> Self {
        Font { typeface: 4099, point_size, pitch: None, bold: false, italic: false }
    }
    pub fn univers(point_size: u16) -> Self {
        Font { typeface: 4101, point_size, pitch: None, bold: false, italic: false }
    }
    pub fn letter_gothic(point_size: u16, pitch: u16) -> Self {
        Font { typeface: 4163, point_size, pitch: Some(pitch), bold: false, italic: false }
    }
}

impl Default for Font {
    fn default() -> Self {
        Font::courier(12, 10)
    }
}

/// Text rendering style (font + line spacing).
#[derive(Debug, Clone)]
pub struct TextStyle {
    pub font: Font,
    /// Line spacing as a multiple of the point size.
    pub line_spacing: f32,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle { font: Font::default(), line_spacing: 1.0 }
    }
}

/// ISO 8859-1 (Latin-1) symbol set; pairs with [`encode_latin1`].
const SYMBOL_SET: &str = "0N";

/// Horizontal origin offset of the printer's PCL logical page, in decipoints
/// (~0.42"). See `raster_body`.
const ORIGIN_OFFSET_X_DP: i64 = 300;
/// Vertical origin offset of the printer's PCL logical page, in decipoints.
/// Zero: the top-margin reset (`&l0E`) takes effect, so the vertical origin is
/// at the physical page top. See `raster_body`.
const ORIGIN_OFFSET_Y_DP: i64 = 0;

/// Escape prefix `ESC <cmd>`.
fn esc(cmd: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(cmd.len() + 1);
    v.push(0x1b);
    v.extend_from_slice(cmd.as_bytes());
    v
}

/// Build the `<Esc>(s...T` font-selection sequence.
fn font_select(font: &Font) -> Vec<u8> {
    let mut out = vec![0x1b, b'('];
    out.extend_from_slice(SYMBOL_SET.as_bytes());
    out.extend_from_slice(b"\x1b(s");
    if let Some(pitch) = font.pitch {
        out.extend_from_slice(format!("0p{pitch}h").as_bytes());
    } else {
        out.extend_from_slice(b"1p");
    }
    out.extend_from_slice(format!("{}v", font.point_size).as_bytes());
    out.extend_from_slice(format!("{}s", if font.italic { 1 } else { 0 }).as_bytes());
    out.extend_from_slice(format!("{}b", if font.bold { 3 } else { 0 }).as_bytes());
    out.extend_from_slice(format!("{}T", font.typeface).as_bytes());
    out
}

/// Encode `&str` as ISO 8859-1 bytes, replacing unrepresentable characters
/// with `?`.
pub fn encode_latin1(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| if (c as u32) <= 0xff { c as u8 } else { b'?' })
        .collect()
}

/// Build a complete PCL text job body for `text`.
pub fn text_body(text: &str, opts: &JobOptions, style: &TextStyle) -> Vec<u8> {
    let mut out = Vec::new();

    out.extend_from_slice(b"\x1bE");
    // Paper size and orientation first: they reset the logical page and its
    // margins, so the zero-margin commands must come after them.
    out.extend_from_slice(&esc(&format!("&l{}A", opts.paper.pcl_code())));
    out.extend_from_slice(&esc(match opts.orientation {
        Orientation::Portrait => "&l0O",
        Orientation::Landscape => "&l1O",
    }));
    out.extend_from_slice(&esc("&l0E")); // top margin 0
    out.extend_from_slice(&esc("&l0L")); // perforation skip off
    out.extend_from_slice(&esc("&a0L")); // left margin 0 columns
    out.extend_from_slice(&font_select(&style.font));

    // Line spacing as a vertical motion index in 1/48 inch.
    let line_advance_pts = style.font.point_size as f32 * style.line_spacing;
    let vmi = (line_advance_pts * 48.0 / 72.0).round().max(1.0) as u32;
    out.extend_from_slice(&esc(&format!("&l{vmi}C")));
    // Make LF behave as CR+LF.
    out.extend_from_slice(&esc("&k2G"));

    // Right margin (fixed-pitch fonts only) so long lines wrap.
    if let Some(pitch) = style.font.pitch {
        let (pw, ph) = opts.paper.size_points();
        let printable_w_pts = if opts.orientation == Orientation::Landscape { ph } else { pw };
        let printable_w_pts = printable_w_pts.saturating_sub(36);
        let hmi_pts = 72.0 / pitch as f32;
        let columns = (printable_w_pts as f32 / hmi_pts).floor() as u32;
        if columns > 0 {
            out.extend_from_slice(&esc(&format!("&a{columns}M")));
        }
    }

    // Paginate.
    let (pw, ph) = opts.paper.size_points();
    let printable_h_pts = if opts.orientation == Orientation::Landscape { pw } else { ph };
    let printable_h_pts = printable_h_pts.saturating_sub(36);
    let lines_per_page = ((printable_h_pts as f32) / line_advance_pts).floor().max(1.0) as usize;

    for (line_no, raw) in text.split('\n').enumerate() {
        let line = expand_tabs(raw);
        if line_no > 0 && line_no % lines_per_page == 0 {
            out.push(0x0c); // form feed
        }
        out.extend_from_slice(&encode_latin1(&line));
        out.push(b'\n');
    }
    // No trailing form feed: the job-end UEL ejects the final page.
    out
}

fn expand_tabs(line: &str) -> String {
    const TAB: usize = 8;
    let mut out = String::with_capacity(line.len());
    let mut col = 0usize;
    for c in line.chars() {
        if c == '\t' {
            let spaces = TAB - (col % TAB);
            out.extend(std::iter::repeat(' ').take(spaces));
            col += spaces;
        } else {
            out.push(c);
            col += 1;
        }
    }
    out
}

/// Build a complete PCL raster-graphics job body from an 8-bit grayscale
/// image (`luma` is `width * height` bytes). The image is centered on the
/// page and printed at `opts.resolution` dpi.
pub fn raster_body(luma: &[u8], width: usize, height: usize, opts: &JobOptions) -> Result<Vec<u8>> {
    let dpi = opts.resolution.clamp(75, 300);
    let bits = dither_floyd_steinberg(luma, width, height)?;
    let row_bytes = width.div_ceil(8);

    let mut out = Vec::new();
    out.extend_from_slice(b"\x1bE");
    // Paper size and orientation first (they reset the logical page and its
    // margins), then zero the margins, then position the cursor.
    out.extend_from_slice(&esc(&format!("&l{}A", opts.paper.pcl_code())));
    out.extend_from_slice(&esc(match opts.orientation {
        Orientation::Portrait => "&l0O",
        Orientation::Landscape => "&l1O",
    }));
    out.extend_from_slice(&esc("&l0E")); // top margin 0
    out.extend_from_slice(&esc("&l0L")); // perforation skip off
    out.extend_from_slice(&esc("&a0L")); // left margin 0

    // Center the image using decipoint cursor positioning (1/720 inch).
    let (pw, ph) = opts.paper.size_points();
    let (page_w_dp, page_h_dp) = match opts.orientation {
        Orientation::Portrait => (pw * 10, ph * 10),
        Orientation::Landscape => (ph * 10, pw * 10),
    };
    let img_w_dp = width as u32 * 720 / dpi;
    let img_h_dp = height as u32 * 720 / dpi;
    let left = page_w_dp.saturating_sub(img_w_dp) / 2;
    let top = page_h_dp.saturating_sub(img_h_dp) / 2;
    // The HL-2270DW's PCL logical-page origin sits ~0.42" right and ~0.67"
    // down of the physical page corner (its default margins), so subtract
    // that offset to center content on the physical page. This may produce
    // negative cursor positions, which PCL accepts to move up/left.
    let left = left as i64 - ORIGIN_OFFSET_X_DP;
    let top = top as i64 - ORIGIN_OFFSET_Y_DP;
    out.extend_from_slice(&esc(&format!("&a{left}H")));
    out.extend_from_slice(&esc(&format!("&a{top}V")));

    out.extend_from_slice(&esc(&format!("*t{dpi}R")));
    out.extend_from_slice(&esc("*r1A"));
    out.extend_from_slice(&esc("*b2M")); // TIFF packbits
    out.extend_from_slice(&esc(&format!("*r{width}S")));
    out.extend_from_slice(&esc(&format!("*r{height}T")));

    for row in bits.chunks_exact(row_bytes) {
        let enc = packbits_encode(row);
        out.extend_from_slice(&esc(&format!("*b{}W", enc.len())));
        out.extend_from_slice(&enc);
    }
    out.extend_from_slice(&esc("*rB"));
    // No trailing form feed: the job-end UEL ejects the page.
    Ok(out)
}

/// Floyd–Steinberg error-diffusion dither to 1 bit, MSB-first rows.
fn dither_floyd_steinberg(luma: &[u8], width: usize, height: usize) -> Result<Vec<u8>> {
    if luma.len() != width * height {
        return Err(Error::Pcl(format!(
            "grayscale buffer length {} does not match {width}x{height}",
            luma.len()
        )));
    }
    // Work in "ink" space: 0.0 = white, 1.0 = black.
    let mut buf: Vec<f32> = luma.iter().map(|&p| 1.0 - (p as f32 / 255.0)).collect();
    let row_bytes = width.div_ceil(8);
    let mut out = vec![0u8; row_bytes * height];

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let old = buf[idx];
            let new = if old >= 0.5 { 1.0 } else { 0.0 };
            let err = old - new;
            buf[idx] = new;

            if x + 1 < width {
                buf[idx + 1] += err * 7.0 / 16.0;
            }
            if y + 1 < height {
                if x > 0 {
                    buf[idx + width - 1] += err * 3.0 / 16.0;
                }
                buf[idx + width] += err * 5.0 / 16.0;
                if x + 1 < width {
                    buf[idx + width + 1] += err * 1.0 / 16.0;
                }
            }

            if new >= 0.5 {
                out[y * row_bytes + x / 8] |= 1 << (7 - (x % 8));
            }
        }
    }
    Ok(out)
}

/// TIFF packbits (PCL compression method 2) encoder for one raster row.
pub fn packbits_encode(row: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let n = row.len();
    let mut i = 0;
    while i < n {
        // Length of the run of identical bytes starting at `i`.
        let mut run = 1;
        while i + run < n && row[i + run] == row[i] && run < 128 {
            run += 1;
        }
        if run >= 3 {
            out.push((257 - run) as u8);
            out.push(row[i]);
            i += run;
        } else {
            // Literal run (up to 128 bytes) that stops before a 3+ repeat.
            let start = i;
            let mut j = i;
            while j < n && (j - start) < 128 {
                if j + 2 < n && row[j] == row[j + 1] && row[j + 1] == row[j + 2] {
                    break;
                }
                j += 1;
            }
            if j == start {
                j += 1; // force progress (defensive)
            }
            let len = j - start;
            out.push((len - 1) as u8);
            out.extend_from_slice(&row[start..j]);
            i = j;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packbits_decode(enc: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < enc.len() {
            let h = enc[i] as i8;
            i += 1;
            if h >= 0 {
                // literal: (h+1) bytes
                let n = h as usize + 1;
                out.extend_from_slice(&enc[i..i + n]);
                i += n;
            } else if h != -128 {
                // repeat: (1 - h) copies of next byte
                let n = (1 - h as i32) as usize;
                let b = enc[i];
                i += 1;
                out.extend(std::iter::repeat(b).take(n));
            }
        }
        out
    }

    #[test]
    fn packbits_roundtrip() {
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            vec![0x00],
            vec![0xff, 0xff, 0xff, 0xff],
            vec![0x00, 0x00, 0x00, 0xff, 0xff, 0xff],
            (0..=255).collect(),
            vec![0xaa; 300],
            vec![0x00, 0x01, 0x02, 0x02, 0x02, 0x03, 0x04],
        ];
        for c in cases {
            let enc = packbits_encode(&c);
            assert_eq!(packbits_decode(&enc), c, "roundtrip failed for {c:?}");
        }
    }

    #[test]
    fn dither_produces_correct_shape_and_bits() {
        // 8x1 all-white -> all zeros; 8x1 all-black -> all ones.
        let white = dither_floyd_steinberg(&[0xff; 8], 8, 1).unwrap();
        assert_eq!(white, vec![0x00]);
        let black = dither_floyd_steinberg(&[0x00; 8], 8, 1).unwrap();
        assert_eq!(black, vec![0xff]);
    }

    #[test]
    fn latin1_maps_and_replaces() {
        assert_eq!(encode_latin1("abc"), b"abc".to_vec());
        assert_eq!(encode_latin1("café"), b"caf\xe9".to_vec());
        assert_eq!(encode_latin1("漢"), b"?".to_vec());
    }

    #[test]
    fn font_select_contains_traits() {
        let f = Font::courier(12, 10);
        let s = String::from_utf8(font_select(&f)).unwrap();
        assert!(s.contains("(0N"));
        assert!(s.contains("0p10h"));
        assert!(s.contains("12v"));
        assert!(s.contains("4148T"));
    }
}
