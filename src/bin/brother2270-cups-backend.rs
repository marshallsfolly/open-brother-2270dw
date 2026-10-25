//! CUPS backend binary. Symlink this into `/usr/lib/cups/backend/` (e.g. as
//! `brother2270`) and use a device URI of the form `brother2270://<ip>:9100`.

fn main() {
    if let Err(e) = brother_2270dw::cups::backend_main() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
