//! # open-brother-2270dw
//!
//! A fully featured Rust driver for the Brother HL-2270DW monochrome laser
//! printer when it is attached to the local network (Ethernet or Wi-Fi).
//!
//! The crate provides:
//!
//! * **Discovery** — find the printer on the LAN via mDNS/DNS-SD and SNMP
//!   broadcast.
//! * **Status** — read toner/drum levels, page counters, error states and
//!   device identity over SNMP (Printer MIB, RFC 1759/3805).
//! * **Transport** — raw TCP printing on port 9100 (HP JetDirect) and LPR on
//!   port 515 (RFC 1179).
//! * **Job control** — PJL for duplex, paper, resolution, copies, toner-save
//!   and device `INFO` queries.
//! * **Page generation** — PCL5e (resident-font text plus 600 dpi raster
//!   graphics) and BR-Script 3 / PostScript passthrough.

pub mod ber;
pub mod config;
pub mod cups;
pub mod discovery;
pub mod error;
pub mod font;
pub mod job;
pub mod lpr;
pub mod mdns;
pub mod monitor;
pub mod pcl;
pub mod pdf;
pub mod pjl;
pub mod postscript;
pub mod printer;
pub mod raw;
pub mod serve;
pub mod snmp;
pub mod status;
pub mod usb;
pub mod wifi;

pub use error::{Error, Result};
pub use job::{Duplex, JobOptions, Language, Orientation, Paper};
pub use printer::Printer;
