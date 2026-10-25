//! Diagnostic: render text with a TrueType font and report coverage stats.

use brother_2270dw::font;

fn main() {
    let font_path = std::env::args().nth(1).expect("usage: font_render <font.ttf> [text]");
    let text = std::env::args().nth(2).unwrap_or_else(|| "Hello, world! 123".into());
    let (luma, w, h) =
        font::render_text(&text, std::path::Path::new(&font_path), 12.0, 300, 7.5).expect("render");
    let ink: u64 = luma.iter().map(|&p| if p < 128 { 1 } else { 0 }).sum();
    let total = (w * h) as u64;
    println!("{w}x{h} px, ink coverage {:.2}% ({ink}/{total})", ink as f64 / total as f64 * 100.0);
}
