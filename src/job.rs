//! Print-job model (shared types) and job assembly.

use crate::pjl;

/// Supported paper sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paper {
    Letter,
    Legal,
    A4,
    A5,
    A6,
    Executive,
    B5Jis,
    Envelope10,
    EnvelopeDl,
    EnvelopeC5,
}

impl Paper {
    /// Human-readable name.
    pub fn name(self) -> &'static str {
        match self {
            Paper::Letter => "Letter",
            Paper::Legal => "Legal",
            Paper::A4 => "A4",
            Paper::A5 => "A5",
            Paper::A6 => "A6",
            Paper::Executive => "Executive",
            Paper::B5Jis => "B5 (JIS)",
            Paper::Envelope10 => "Com-10 envelope",
            Paper::EnvelopeDl => "DL envelope",
            Paper::EnvelopeC5 => "C5 envelope",
        }
    }

    /// Value for the `@PJL SET PAPER=` command.
    pub fn pjl_name(self) -> &'static str {
        match self {
            Paper::Letter => "LETTER",
            Paper::Legal => "LEGAL",
            Paper::A4 => "A4",
            Paper::A5 => "A5",
            Paper::A6 => "A6",
            Paper::Executive => "EXECUTIVE",
            Paper::B5Jis => "JISB5",
            Paper::Envelope10 => "COM10",
            Paper::EnvelopeDl => "DL",
            Paper::EnvelopeC5 => "C5",
        }
    }

    /// PCL paper-size code for `<Esc>&l#A`.
    pub fn pcl_code(self) -> &'static str {
        match self {
            Paper::Executive => "1",
            Paper::Letter => "2",
            Paper::Legal => "3",
            Paper::A6 => "24",
            Paper::A5 => "25",
            Paper::A4 => "26",
            Paper::B5Jis => "45",
            Paper::Envelope10 => "81",
            Paper::EnvelopeDl => "90",
            Paper::EnvelopeC5 => "91",
        }
    }

    /// Page size in points (1/72 inch), as `(width, height)` in portrait
    /// orientation.
    pub fn size_points(self) -> (u32, u32) {
        match self {
            Paper::Letter => (612, 792),
            Paper::Legal => (612, 1008),
            Paper::A4 => (595, 842),
            Paper::A5 => (420, 595),
            Paper::A6 => (297, 420),
            Paper::Executive => (522, 756),
            Paper::B5Jis => (516, 729),
            Paper::Envelope10 => (297, 684),
            Paper::EnvelopeDl => (312, 624),
            Paper::EnvelopeC5 => (459, 649),
        }
    }
}

/// Duplex mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duplex {
    Off,
    LongEdge,
    ShortEdge,
}

/// Page orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Portrait,
    Landscape,
}

/// Page-description language used for a job body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Pcl,
    Pclxl,
    Postscript,
}

/// Options controlling a print job.
#[derive(Debug, Clone)]
pub struct JobOptions {
    pub copies: u32,
    pub paper: Paper,
    pub duplex: Duplex,
    pub orientation: Orientation,
    pub resolution: u32,
    pub toner_save: bool,
    pub job_name: String,
    /// LPR queue name (ignored for raw port 9100).
    pub queue: String,
}

impl Default for JobOptions {
    fn default() -> Self {
        JobOptions {
            copies: 1,
            paper: Paper::Letter,
            duplex: Duplex::Off,
            orientation: Orientation::Portrait,
            resolution: 300,
            toner_save: false,
            job_name: "open-brother-2270dw".into(),
            queue: crate::lpr::DEFAULT_QUEUE.into(),
        }
    }
}

/// Wrap a language-specific job body in a complete PJL job:
/// `UEL` + PJL header + `ENTER LANGUAGE` + body + `EOJ` + `UEL`.
pub fn wrap_job(language: Language, body: &[u8], opts: &JobOptions) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 256);
    out.extend_from_slice(pjl::UEL);
    out.extend_from_slice(&pjl::job_header(opts));
    out.extend_from_slice(&pjl::enter_language(language));
    out.extend_from_slice(body);
    out.extend_from_slice(&pjl::end_job());
    out
}

impl std::fmt::Display for Paper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.pjl_name())
    }
}

impl std::str::FromStr for Paper {
    type Err = crate::Error;
    fn from_str(s: &str) -> crate::Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "letter" => Ok(Paper::Letter),
            "legal" => Ok(Paper::Legal),
            "a4" => Ok(Paper::A4),
            "a5" => Ok(Paper::A5),
            "a6" => Ok(Paper::A6),
            "executive" => Ok(Paper::Executive),
            "b5" | "b5jis" | "jisb5" => Ok(Paper::B5Jis),
            "com10" | "envelope10" | "env10" => Ok(Paper::Envelope10),
            "dl" | "envelopedl" | "envdl" => Ok(Paper::EnvelopeDl),
            "c5" | "envelopec5" | "envc5" => Ok(Paper::EnvelopeC5),
            _ => Err(crate::Error::InvalidArgument(format!("unknown paper size: {s}"))),
        }
    }
}

impl std::str::FromStr for Duplex {
    type Err = crate::Error;
    fn from_str(s: &str) -> crate::Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "off" | "none" | "simplex" => Ok(Duplex::Off),
            "longedge" | "long-edge" | "long" | "duplex" => Ok(Duplex::LongEdge),
            "shortedge" | "short-edge" | "short" => Ok(Duplex::ShortEdge),
            _ => Err(crate::Error::InvalidArgument(format!("unknown duplex mode: {s}"))),
        }
    }
}

impl std::fmt::Display for Duplex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Duplex::Off => "off",
                Duplex::LongEdge => "long-edge",
                Duplex::ShortEdge => "short-edge",
            }
        )
    }
}

impl std::str::FromStr for Orientation {
    type Err = crate::Error;
    fn from_str(s: &str) -> crate::Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "portrait" | "port" => Ok(Orientation::Portrait),
            "landscape" | "land" => Ok(Orientation::Landscape),
            _ => Err(crate::Error::InvalidArgument(format!("unknown orientation: {s}"))),
        }
    }
}

impl std::fmt::Display for Orientation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Orientation::Portrait => "portrait",
                Orientation::Landscape => "landscape",
            }
        )
    }
}
