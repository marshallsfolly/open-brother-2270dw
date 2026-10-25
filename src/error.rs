//! Crate-wide error type.

use std::io;

/// Errors produced by the driver.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("SNMP error: {0}")]
    Snmp(String),

    #[error("ASN.1/BER error: {0}")]
    Ber(String),

    #[error("mDNS/DNS error: {0}")]
    Mdns(String),

    #[error("PJL error: {0}")]
    Pjl(String),

    #[error("PCL error: {0}")]
    Pcl(String),

    #[error("printer reported an error: {0}")]
    Printer(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("timed out while {0}")]
    Timeout(String),

    #[error("no printer found")]
    NotFound,

    #[error("unsupported file or format: {0}")]
    Unsupported(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Convenience alias for a fallible result in this crate.
pub type Result<T> = std::result::Result<T, Error>;
