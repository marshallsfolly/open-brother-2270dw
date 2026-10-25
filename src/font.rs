//! TrueType/Unicode text rendering (via `fontdue`), fed through the PCL raster
//! path. This supports arbitrary Unicode text and fonts — beyond the printer's
//! resident fonts and Latin-1.

use std::path::Path;

use crate::error::{Error, Result};

/// Render `text` with the given TrueType font to a grayscale bitmap at `dpi`.
/// Returns `(luma, width, height)`; `luma` uses the crate convention
/// (0 = black, 255 = white). `point_size` is in points; text wraps to
/// `max_width_in` inches.
pub fn render_text(
    text: &str,
    font_path: &Path,
    point_size: f32,
    dpi: u32,
    max_width_in: f32,
) -> Result<(Vec<u8>, usize, usize)> {
    let data = std::fs::read(font_path)
        .map_err(|e| Error::Unsupported(format!("read font {}: {e}", font_path.display())))?;
    let font = fontdue::Font::from_bytes(data, fontdue::FontSettings::default())
        .map_err(|e| Error::Unsupported(format!("parse font {}: {e}", font_path.display())))?;

    let px = (point_size * dpi as f32 / 72.0).round().max(1.0);
    let line = font
        .horizontal_line_metrics(px)
        .map(|m| m.new_line_size)
        .unwrap_or(px * 1.2);
    let ascent = font
        .horizontal_line_metrics(px)
        .map(|m| m.ascent)
        .unwrap_or(px);
    let max_w = (max_width_in * dpi as f32).round().max(1.0);

    // Word-wrap each newline-separated paragraph into lines.
    let mut lines: Vec<String> = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        let mut cur_w = 0.0f32;
        for word in para.split_inclusive(char::is_whitespace) {
            let word_w = measure(&font, px, word);
            if cur.is_empty() {
                cur = word.to_string();
                cur_w = word_w;
            } else if cur_w + word_w <= max_w {
                cur.push_str(word);
                cur_w += word_w;
            } else {
                lines.push(std::mem::take(&mut cur));
                cur = word.to_string();
                cur_w = word_w;
            }
        }
        lines.push(cur);
    }

    let width = max_w as usize;
    let height = ((lines.len() as f32) * line).round().max(1.0) as usize;
    let mut luma = vec![0xffu8; width * height];

    for (li, text_line) in lines.iter().enumerate() {
        let baseline = (li as f32) * line + ascent;
        let mut pen_x = 0.0f32;
        for ch in text_line.chars() {
            let (m, bmp) = font.rasterize(ch, px);
            let bx = (pen_x + m.xmin as f32).round() as isize;
            let by = (baseline + m.ymin as f32).round() as isize;
            blit_glyph(&mut luma, width, height, &bmp, m.width, m.height, (bx, by));
            pen_x += m.advance_width;
        }
    }

    Ok((luma, width, height))
}

fn measure(font: &fontdue::Font, px: f32, text: &str) -> f32 {
    text.chars().map(|c| font.rasterize(c, px).0.advance_width).sum()
}

/// Copy a glyph bitmap into the grayscale canvas. fontdue coverage is
/// 0..=255 (255 = fully inked); invert to the crate luma convention.
fn blit_glyph(
    luma: &mut [u8],
    canvas_w: usize,
    canvas_h: usize,
    bmp: &[u8],
    gw: usize,
    gh: usize,
    pos: (isize, isize),
) {
    let (x, y) = pos;
    for gy in 0..gh {
        let ty = y + gy as isize;
        if ty < 0 || ty as usize >= canvas_h {
            continue;
        }
        for gx in 0..gw {
            let tx = x + gx as isize;
            if tx < 0 || tx as usize >= canvas_w {
                continue;
            }
            let coverage = bmp[gy * gw + gx];
            // Ink: 0 (white) .. 255 (black). Store as luma (255 = white).
            luma[ty as usize * canvas_w + tx as usize] = 255 - coverage;
        }
    }
}

/// Look for a usable system TrueType font (DejaVu Sans), if present.
pub fn find_system_font() -> Option<std::path::PathBuf> {
    for path in [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/run/current-system/sw/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ] {
        let p = std::path::PathBuf::from(path);
        if p.exists() {
            return Some(p);
        }
    }
    None
}
