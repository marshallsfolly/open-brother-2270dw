//! PDF printing via vector PostScript conversion.
//!
//! The HL-2270DW has no native PDF support, but it does have BR-Script 3
//! (PostScript 3). We therefore convert PDF → PostScript with `pdftops`
//! (poppler-utils) and send that to the printer, so text stays vector text
//! (crisp, minimal toner) rather than rasterizing the whole page to an image.

use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};

/// Convert `pdf` to PostScript Level 3, written to `out_path`.
pub fn to_postscript(pdf: &Path, out_path: &Path) -> Result<()> {
    let status = Command::new("pdftops")
        .arg("-level3")
        .arg(pdf)
        .arg(out_path)
        .status()
        .map_err(|e| {
            Error::Unsupported(format!(
                "failed to run `pdftops` (PDF printing needs poppler-utils): {e}"
            ))
        })?;
    if !status.success() {
        return Err(Error::Unsupported(format!(
            "pdftops failed to convert {} (exit status {status})",
            pdf.display()
        )));
    }
    Ok(())
}
