//! BR-Script 3 / PostScript job generation.
//!
//! The HL-2270DW ships with BR-Script 3 (a PostScript 3 emulation), so
//! PostScript jobs can be submitted directly. This module provides a small
//! text renderer and a wrapper for arbitrary PostScript programs.

use crate::job::{JobOptions, Orientation, Paper};

/// A minimal PostScript program that renders `text` in a monospaced font.
pub fn text_body(text: &str, opts: &JobOptions, point_size: f32, line_spacing: f32) -> Vec<u8> {
    let (pw, ph) = opts.paper.size_points();
    // Page dimensions in PostScript points, swapped for landscape.
    let (page_w, page_h) = match opts.orientation {
        Orientation::Portrait => (pw as f32, ph as f32),
        Orientation::Landscape => (ph as f32, pw as f32),
    };

    let margin = 36.0;
    let line_advance = point_size * line_spacing;
    let lines_per_page = (((page_h - 2.0 * margin) / line_advance).floor()).max(1.0) as usize;

    let mut out = Vec::new();
    out.extend_from_slice(prologue(page_w, page_h).as_bytes());
    out.extend_from_slice(
        format!("/Courier findfont {point_size} scalefont setfont\n").as_bytes(),
    );

    for (line_no, raw) in text.split('\n').enumerate() {
        if line_no > 0 && line_no % lines_per_page == 0 {
            out.extend_from_slice(b"showpage\n");
        }
        let y = page_h - margin - ((line_no % lines_per_page) as f32) * line_advance;
        out.extend_from_slice(format!("{margin} {y} moveto ({}) show\n", ps_escape(raw)).as_bytes());
    }
    out.extend_from_slice(b"showpage\n");
    out.extend_from_slice(b"%%EOF\n");
    out
}

fn prologue(page_w: f32, page_h: f32) -> String {
    format!(
        "%!PS-Adobe-3.0\n\
         %%LanguageLevel: 3\n\
         %%DocumentData: Clean7Bit\n\
         %%BoundingBox: 0 0 {} {}\n\
         %%Pages: (atend)\n\
         %%EndComments\n",
        page_w as u32, page_h as u32
    )
}

/// Wrap an arbitrary PostScript program in the DSC structure BR-Script
/// expects (passes `program` through verbatim).
pub fn wrap_program(program: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(program.len() + 64);
    out.extend_from_slice(b"%!PS-Adobe-3.0\n%%LanguageLevel: 3\n%%EndComments\n");
    out.extend_from_slice(program);
    if !program.ends_with(b"\n") {
        out.push(b'\n');
    }
    out.extend_from_slice(b"%%EOF\n");
    out
}

/// Escape a string for use inside a PostScript `(...)` literal.
fn ps_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\r' => {}
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                // Emit as a backslash-octal escape over the raw byte (Latin-1).
                let bytes = c as u32;
                if bytes <= 0xff {
                    out.push_str(&format!("\\{:03o}", bytes));
                } else {
                    out.push('?');
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// Return the BR-Script language name for PJL.
pub const LANGUAGE_NAME: &str = "POSTSCRIPT";

/// The paper name used in the PS `setpagedevice` fallback (informational).
pub fn paper_name(opts: &JobOptions) -> &'static str {
    let _ = opts;
    match opts.paper {
        Paper::A4 => "A4",
        Paper::Legal => "legal",
        _ => "letter",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_parens_and_backslash() {
        assert_eq!(ps_escape("a(b)c\\d"), "a\\(b\\)c\\\\d");
    }

    #[test]
    fn text_body_has_dsc_and_shows_lines() {
        let opts = JobOptions::default();
        let body = text_body("hello\nworld", &opts, 12.0, 1.0);
        let s = String::from_utf8(body).unwrap();
        assert!(s.starts_with("%!PS-Adobe-3.0"));
        assert!(s.contains("(hello) show"));
        assert!(s.contains("(world) show"));
        assert!(s.ends_with("%%EOF\n"));
    }

    #[test]
    fn wrap_program_passes_through() {
        let body = wrap_program(b"0 0 moveto");
        let s = String::from_utf8(body).unwrap();
        assert!(s.starts_with("%!PS-Adobe-3.0"));
        assert!(s.contains("0 0 moveto"));
        assert!(s.ends_with("%%EOF\n"));
    }
}
