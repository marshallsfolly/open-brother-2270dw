//! Persistent configuration, stored as TOML in
//! `~/.config/open-brother-2270dw/config.toml`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::job::{Duplex, JobOptions, Orientation, Paper};
use crate::pcl::{Font, TextStyle};

pub const APP_DIR: &str = "open-brother-2270dw";
pub const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub printer: PrinterConfig,
    pub defaults: DefaultsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PrinterConfig {
    /// IP address or hostname; empty means "auto-discover".
    pub host: String,
    pub port: u16,
    pub community: String,
    pub timeout_secs: u64,
    /// `"raw"` (port 9100) or `"lpr"` (port 515).
    pub transport: String,
    pub queue: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DefaultsConfig {
    pub paper: String,
    pub duplex: String,
    pub orientation: String,
    pub resolution: u32,
    pub copies: u32,
    pub toner_save: bool,
    /// `"courier"`, `"times"`, `"univers"`, or `"letter-gothic"`.
    pub font: String,
    pub font_size: u16,
    pub pitch: u16,
    pub line_spacing: f32,
}

impl Default for PrinterConfig {
    fn default() -> Self {
        PrinterConfig {
            host: String::new(),
            port: 9100,
            community: "public".into(),
            timeout_secs: 5,
            transport: "raw".into(),
            queue: "BINARY_P1".into(),
        }
    }
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        DefaultsConfig {
            paper: "letter".into(),
            duplex: "off".into(),
            orientation: "portrait".into(),
            resolution: 300,
            copies: 1,
            toner_save: false,
            font: "courier".into(),
            font_size: 12,
            pitch: 10,
            line_spacing: 1.0,
        }
    }
}

impl Config {
    /// The configuration file path.
    pub fn path() -> PathBuf {
        let base = if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
            PathBuf::from(x)
        } else if let Some(h) = std::env::var_os("HOME") {
            PathBuf::from(h).join(".config")
        } else {
            PathBuf::from(".")
        };
        base.join(APP_DIR).join(CONFIG_FILE)
    }

    /// Load the configuration, returning the default when the file is absent.
    pub fn load() -> Result<Config> {
        let path = Self::path();
        if !path.exists() {
            return Ok(Config::default());
        }
        let text = std::fs::read_to_string(&path)?;
        let cfg = toml::from_str(&text).map_err(|e| {
            crate::error::Error::Config(format!("{}: {e}", path.display()))
        })?;
        Ok(cfg)
    }

    pub fn load_or_default() -> Config {
        Config::load().unwrap_or_default()
    }

    /// Write the configuration, creating the parent directory if needed.
    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| crate::error::Error::Config(e.to_string()))?;
        std::fs::write(&path, text)?;
        Ok(())
    }

    /// Build [`JobOptions`] from the configured defaults.
    pub fn job_options(&self) -> JobOptions {
        JobOptions {
            copies: self.defaults.copies.max(1),
            paper: self.defaults.paper.parse().unwrap_or(Paper::Letter),
            duplex: self.defaults.duplex.parse().unwrap_or(Duplex::Off),
            orientation: self.defaults.orientation.parse().unwrap_or(Orientation::Portrait),
            resolution: self.defaults.resolution.clamp(75, 300),
            toner_save: self.defaults.toner_save,
            job_name: "open-brother-2270dw".into(),
            queue: self.printer.queue.clone(),
        }
    }

    /// Build the default [`TextStyle`].
    pub fn text_style(&self) -> TextStyle {
        let size = self.defaults.font_size.max(4);
        let font = match self.defaults.font.to_ascii_lowercase().as_str() {
            "times" => Font::times(size),
            "univers" | "helvetica" | "arial" => Font::univers(size),
            "letter-gothic" | "letter_gothic" => Font::letter_gothic(size, self.defaults.pitch.max(1)),
            _ => Font::courier(size, self.defaults.pitch.max(1)),
        };
        TextStyle { font, line_spacing: self.defaults.line_spacing.max(0.5) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrips() {
        let cfg = Config::default();
        let text = toml::to_string(&cfg).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed.defaults.paper, "letter");
        assert_eq!(parsed.defaults.resolution, 300);
        assert_eq!(parsed.printer.community, "public");
    }

    #[test]
    fn job_options_from_defaults() {
        let cfg = Config::default();
        let opts = cfg.job_options();
        assert_eq!(opts.paper, Paper::Letter);
        assert_eq!(opts.duplex, Duplex::Off);
        assert_eq!(opts.resolution, 300);
    }
}
