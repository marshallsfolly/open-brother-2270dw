//! High-level printer API: status, info, and print jobs.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use image::imageops::FilterType;

use crate::error::{Error, Result};
use crate::job::{wrap_job, JobOptions, Language, Orientation};
use crate::lpr;
use crate::pcl::{self, TextStyle};
use crate::pjl;
use crate::postscript;
use crate::raw::{RawConnection, RAW_PORT};
use crate::snmp::DEFAULT_COMMUNITY;
use crate::status::{query_status, DeviceStatus};

/// A handle to a printer on the network.
#[derive(Debug, Clone)]
pub struct Printer {
    pub addr: IpAddr,
    pub community: String,
    pub timeout: Duration,
    /// Raw print port (default 9100).
    pub port: u16,
    /// When true, submit jobs over LPR (port 515) instead of raw 9100.
    pub use_lpr: bool,
    /// LPR queue name (used only when `use_lpr` is true).
    pub queue: String,
}

impl Printer {
    pub fn new(addr: IpAddr) -> Self {
        Printer {
            addr,
            community: DEFAULT_COMMUNITY.to_string(),
            timeout: Duration::from_secs(5),
            port: RAW_PORT,
            use_lpr: false,
            queue: lpr::DEFAULT_QUEUE.to_string(),
        }
    }

    pub fn with_community(mut self, community: &str) -> Self {
        self.community = community.to_string();
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_lpr(mut self, queue: &str) -> Self {
        self.use_lpr = true;
        self.queue = queue.to_string();
        self
    }

    /// Read device status, supplies, counters and alerts over SNMP.
    pub fn status(&self) -> Result<DeviceStatus> {
        let addr = std::net::SocketAddr::new(self.addr, 161);
        query_status(addr, &self.community, self.timeout)
    }

    /// Query `@PJL INFO ID` and return the parsed device identity.
    pub fn info(&self) -> Result<HashMap<String, String>> {
        let mut conn = RawConnection::connect(self.addr, self.port, self.timeout)?;
        let resp = conn.send_and_read(&pjl::info_request("ID"), self.timeout)?;
        Ok(pjl::parse_info_id(&resp).into_iter().collect())
    }

    /// Send a PJL reset (UEL).
    pub fn reset(&self) -> Result<()> {
        let mut conn = RawConnection::connect(self.addr, self.port, self.timeout)?;
        conn.send(&pjl::reset())
    }

    /// Print plain text using the printer's resident fonts.
    pub fn print_text(&self, text: &str, opts: &JobOptions, style: &TextStyle) -> Result<()> {
        let body = pcl::text_body(text, opts, style);
        self.submit(Language::Pcl, &body, opts)
    }

    /// Print an image file (PNG/JPEG/BMP/TIFF), fit to the page.
    pub fn print_image(&self, path: &Path, opts: &JobOptions) -> Result<()> {
        let img = image::open(path)
            .map_err(|e| Error::Unsupported(format!("{}: {e}", path.display())))?;
        let (luma, w, h) = fit_luma(&img, opts)?;
        let body = pcl::raster_body(&luma, w, h, opts)?;
        self.submit(Language::Pcl, &body, opts)
    }

    /// Print a file, dispatching on its extension: `.pcl`/`.prn` (raw PCL),
    /// `.ps`/`.eps` (PostScript), image extensions (raster), and everything
    /// else as text.
    pub fn print_file(&self, path: &Path, opts: &JobOptions, style: &TextStyle) -> Result<()> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "pcl" | "prn" => {
                let data = std::fs::read(path)?;
                self.submit(Language::Pcl, &data, opts)
            }
            "ps" | "eps" => {
                let data = std::fs::read(path)?;
                let body = postscript::wrap_program(&data);
                self.submit(Language::Postscript, &body, opts)
            }
            "pdf" => self.print_pdf(path, opts),
            "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" => self.print_image(path, opts),
            _ => {
                if image::open(path).is_ok() {
                    self.print_image(path, opts)
                } else {
                    let text = std::fs::read_to_string(path)?;
                    self.print_text(&text, opts, style)
                }
            }
        }
    }

    /// Print a PDF by converting it to PostScript (vector) and sending it to
    /// the printer's BR-Script interpreter — text stays text, no full-page
    /// rasterization.
    pub fn print_pdf(&self, path: &Path, opts: &JobOptions) -> Result<()> {
        let dir = std::env::temp_dir().join(format!(
            "brother-2270dw-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir)?;
        let result = (|| -> Result<()> {
            let ps = dir.join("document.ps");
            crate::pdf::to_postscript(path, &ps)?;
            let body = std::fs::read(&ps)?;
            self.submit(Language::Postscript, &body, opts)
        })();
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    /// Print text using a TrueType font (arbitrary Unicode), rasterized at the
    /// job resolution. Multi-page output is paginated automatically.
    pub fn print_text_ttf(
        &self,
        text: &str,
        font_path: &Path,
        point_size: f32,
        opts: &JobOptions,
    ) -> Result<()> {
        let dpi = opts.resolution.clamp(75, 300);
        let (pw, ph) = opts.paper.size_points();
        let (printable_w_pts, printable_h_pts) = match opts.orientation {
            Orientation::Portrait => (pw.saturating_sub(36), ph.saturating_sub(36)),
            Orientation::Landscape => (ph.saturating_sub(36), pw.saturating_sub(36)),
        };
        let max_w_in = printable_w_pts as f32 / 72.0;
        let (luma, w, h) = crate::font::render_text(text, font_path, point_size, dpi, max_w_in)?;
        let page_h_dots = ((printable_h_pts as f32 / 72.0) * dpi as f32).round().max(1.0) as usize;

        let mut y = 0usize;
        while y < h {
            let chunk_h = page_h_dots.min(h - y);
            let chunk = luma[y * w..(y + chunk_h) * w].to_vec();
            let body = pcl::raster_body(&chunk, w, chunk_h, opts)?;
            self.submit(Language::Pcl, &body, opts)?;
            y += chunk_h;
        }
        Ok(())
    }

    /// Compose up to `n` images into a grid (`ceil(sqrt(n))` per row) and
    /// print them on a single page.
    pub fn print_images_nup(
        &self,
        paths: &[std::path::PathBuf],
        n: usize,
        opts: &JobOptions,
    ) -> Result<()> {
        let n = n.clamp(1, paths.len().max(1));
        let cols = (n as f64).sqrt().ceil().max(1.0) as usize;
        let rows = n.div_ceil(cols);
        let dpi = opts.resolution.clamp(75, 300);

        let (pw, ph) = opts.paper.size_points();
        let (printable_w_pts, printable_h_pts) = match opts.orientation {
            Orientation::Portrait => (pw.saturating_sub(36), ph.saturating_sub(36)),
            Orientation::Landscape => (ph.saturating_sub(36), pw.saturating_sub(36)),
        };
        let canvas_w = ((printable_w_pts as f32 / 72.0) * dpi as f32).round().max(1.0) as usize;
        let canvas_h = ((printable_h_pts as f32 / 72.0) * dpi as f32).round().max(1.0) as usize;
        let cell_w = canvas_w / cols;
        let cell_h = canvas_h / rows;
        let mut canvas = vec![0xffu8; canvas_w * canvas_h];

        for (i, path) in paths.iter().take(n).enumerate() {
            let img = image::open(path)
                .map_err(|e| Error::Unsupported(format!("{}: {e}", path.display())))?;
            let luma = img.to_luma8();
            let resized = image::imageops::resize(&luma, cell_w as u32, cell_h as u32, FilterType::Triangle);
            let (cx, cy) = (i % cols, i / cols);
            let ox = cx * cell_w;
            let oy = cy * cell_h;
            for y in 0..cell_h {
                for x in 0..cell_w {
                    let px = resized.get_pixel(x as u32, y as u32)[0];
                    canvas[(oy + y) * canvas_w + (ox + x)] = px;
                }
            }
        }
        let body = pcl::raster_body(&canvas, canvas_w, canvas_h, opts)?;
        self.submit(Language::Pcl, &body, opts)
    }

    /// Submit an already-built PCL job body.
    pub fn print_pcl(&self, body: &[u8], opts: &JobOptions) -> Result<()> {
        self.submit(Language::Pcl, body, opts)
    }

    /// Submit an arbitrary PostScript program.
    pub fn print_postscript(&self, program: &[u8], opts: &JobOptions) -> Result<()> {
        let body = postscript::wrap_program(program);
        self.submit(Language::Postscript, &body, opts)
    }

    /// Print a built-in self-test page.
    pub fn self_test(&self, opts: &JobOptions) -> Result<()> {
        let style = TextStyle::default();
        let mut text = String::new();
        text.push_str("Brother HL-2270DW driver self-test page\n");
        text.push_str("====================================\n\n");
        text.push_str("This page was printed by open-brother-2270dw.\n");
        text.push_str("If you can read this, PCL text output works.\n\n");
        text.push_str("The quick brown fox jumps over the lazy dog.\n");
        text.push_str("0123456789 !@#$%^&*()_+-=[]{};:'\",.<>/?\n\n");
        text.push_str("The next page has only four small corner marks\n");
        text.push_str("(plus a center dot) marking the 8 x 10 inch\n");
        text.push_str("image frame, to check alignment with minimal\n");
        text.push_str("toner.\n");
        self.print_text(&text, opts, &style)?;
        self.print_marks(opts)
    }

    /// Print only the raster corner-marks calibration page (single page).
    pub fn print_marks(&self, opts: &JobOptions) -> Result<()> {
        let (luma, w, h) = calibration_sheet();
        let body = pcl::raster_body(&luma, w, h, opts)?;
        self.submit(Language::Pcl, &body, opts)
    }

    fn submit(&self, language: Language, body: &[u8], opts: &JobOptions) -> Result<()> {
        let job = wrap_job(language, body, opts);
        if self.use_lpr {
            lpr::print(self.addr, &opts.queue, &opts.job_name, &job, self.timeout)
        } else {
            let mut conn = RawConnection::connect(self.addr, self.port, self.timeout)?;
            conn.send(&job)
        }
    }
}

/// Fit an image to the printable area at the job resolution, returning a
/// grayscale buffer and its pixel dimensions.
fn fit_luma(img: &image::DynamicImage, opts: &JobOptions) -> Result<(Vec<u8>, usize, usize)> {
    let luma = img.to_luma8();
    let (iw, ih) = luma.dimensions();
    let dpi = opts.resolution.clamp(75, 300) as f32;
    let (pw, ph) = opts.paper.size_points();
    let (pw, ph) = (pw as f32, ph as f32);

    let (box_w_pts, box_h_pts) = match opts.orientation {
        Orientation::Portrait => (pw - 36.0, ph - 36.0),
        Orientation::Landscape => (ph - 36.0, pw - 36.0),
    };
    let box_w = box_w_pts / 72.0 * dpi;
    let box_h = box_h_pts / 72.0 * dpi;

    let scale = (box_w / iw as f32).min(box_h / ih as f32);
    let tw = (iw as f32 * scale).round().max(1.0) as u32;
    let th = (ih as f32 * scale).round().max(1.0) as u32;

    let resized = image::imageops::resize(&luma, tw, th, FilterType::Lanczos3);
    Ok((resized.into_raw(), tw as usize, th as usize))
}

/// Build an 8" x 10" (at 300 dpi) alignment sheet containing only four small
/// corner crosshairs, labeled TL/TR/BL/BR, plus a center dot, to locate the
/// frame with minimal toner.
fn calibration_sheet() -> (Vec<u8>, usize, usize) {
    let w = 2400usize; // 8.0"
    let h = 3000usize; // 10.0"
    let mut img = vec![0xffu8; w * h];
    let scale = 6isize;
    let text_h = 7 * scale;
    let text_w = |s: &str| s.len() as isize * 6 * scale;
    let inset = 80isize;

    // Crosshairs centered exactly on each corner of the image frame.
    for (cx, cy) in [
        (0isize, 0isize),
        (w as isize - 1, 0isize),
        (0isize, h as isize - 1),
        (w as isize - 1, h as isize - 1),
    ] {
        draw_cross(&mut img, w, h, cx, cy, 60, 6);
    }

    // Tiny center dot for orientation.
    draw_cross(&mut img, w, h, (w / 2) as isize, (h / 2) as isize, 8, 3);

    // Labels just inside each corner.
    draw_text(&mut img, w, h, inset, inset, "TL", scale);
    draw_text(&mut img, w, h, w as isize - 1 - inset - text_w("TR"), inset, "TR", scale);
    draw_text(&mut img, w, h, inset, h as isize - 1 - inset - text_h, "BL", scale);
    draw_text(
        &mut img,
        w,
        h,
        w as isize - 1 - inset - text_w("BR"),
        h as isize - 1 - inset - text_h,
        "BR",
        scale,
    );
    draw_text(&mut img, w, h, (w / 2) as isize - text_w("C") / 2, (h / 2) as isize + 14, "C", scale);

    (img, w, h)
}

/// A minimal 5x7 bitmap font for the label letters we need.
/// Each glyph is 7 rows; each row is 5 bits (bit 4 = leftmost column).
fn glyph(ch: char) -> [u8; 7] {
    match ch {
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'C' => [0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111],
        _ => [0; 7],
    }
}

/// Draw `text` (uppercase letters/digits we have glyphs for) starting at
/// `(x, y)`, scaled by `scale`.
fn draw_text(img: &mut [u8], w: usize, h: usize, x: isize, y: isize, text: &str, scale: isize) {
    let mut cx = x;
    for ch in text.chars() {
        let g = glyph(ch);
        for (row, bits) in g.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) != 0 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            set_px(img, w, h, cx + col as isize * scale + dx, y + row as isize * scale + dy, 0x00);
                        }
                    }
                }
            }
        }
        cx += 6 * scale;
    }
}

fn set_px(img: &mut [u8], w: usize, h: usize, x: isize, y: isize, v: u8) {
    if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
        img[(y as usize) * w + (x as usize)] = v;
    }
}

/// Draw a plus-shaped crosshair centered on `(cx, cy)`.
fn draw_cross(img: &mut [u8], w: usize, h: usize, cx: isize, cy: isize, arm: isize, thick: isize) {
    let half = thick / 2;
    for d in -arm..=arm {
        for t in -half..=half {
            set_px(img, w, h, cx + d, cy + t, 0x00); // horizontal arm
            set_px(img, w, h, cx + t, cy + d, 0x00); // vertical arm
        }
    }
}
