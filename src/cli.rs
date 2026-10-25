//! Command-line interface.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Parser, Subcommand, ValueEnum};

use brother_2270dw::config::Config;
use brother_2270dw::discovery::{self, DiscoveredPrinter};
use brother_2270dw::error::{Error, Result};
use brother_2270dw::job::{Duplex, JobOptions, Orientation, Paper};
use brother_2270dw::monitor::{self, AlertLevel};
use brother_2270dw::printer::Printer;
use brother_2270dw::status::{DeviceStatus, Supply, SupplyKind};

#[derive(Parser)]
#[command(
    name = "brother-2270dw",
    version,
    about = "Driver for the Brother HL-2270DW network laser printer",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Find printers on the local network (mDNS + SNMP broadcast)
    Discover {
        #[arg(long, default_value_t = 3)]
        timeout: u64,
    },
    /// Show printer status, supplies, counters and alerts
    Status {
        #[command(flatten)]
        printer: PrinterArgs,
        /// Output JSON instead of human-readable text
        #[arg(long)]
        json: bool,
    },
    /// Watch printer status and alert on low toner/drum or errors
    Monitor {
        #[command(flatten)]
        printer: PrinterArgs,
        /// Poll interval in seconds
        #[arg(long, default_value_t = 60)]
        interval: u64,
        /// Poll once and exit (for cron/systemd timers)
        #[arg(long)]
        once: bool,
        /// Output JSON
        #[arg(long)]
        json: bool,
        /// POST JSON to this webhook URL when an alert first triggers
        #[arg(long)]
        webhook: Option<String>,
        /// Run this shell command (JSON piped to stdin) on alert
        #[arg(long)]
        command: Option<String>,
        /// With --once, exit non-zero (1=warning, 2=critical) when unhealthy
        #[arg(long)]
        exit_on_alert: bool,
    },
    /// Show device information via PJL INFO ID
    Info(PrinterArgs),
    /// Print a file (text, image, PDF, PCL, or PostScript)
    Print {
        files: Vec<PathBuf>,
        #[command(flatten)]
        printer: PrinterArgs,
        #[command(flatten)]
        opts: PrintOpts,
    },
    /// Print text (optionally with a TrueType font for Unicode)
    Text {
        /// Text to print (or read from stdin if omitted)
        text: Option<String>,
        #[command(flatten)]
        printer: PrinterArgs,
        /// TrueType font file for Unicode rendering
        #[arg(long)]
        font_file: Option<PathBuf>,
        /// Font size in points
        #[arg(long, default_value_t = 12)]
        size: u16,
    },
    /// Print a built-in self-test page
    SelfTest {
        #[command(flatten)]
        printer: PrinterArgs,
    },
    /// Print a single raster corner-marks calibration page
    TestPage {
        #[command(flatten)]
        printer: PrinterArgs,
    },
    /// Show or change configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Reset the printer (PJL UEL)
    Reset(PrinterArgs),
    /// Run a small HTTP status/print server
    Serve {
        #[command(flatten)]
        printer: PrinterArgs,
        /// Listen address (host:port)
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: String,
    },
    /// Show or configure the printer's wireless (Wi-Fi) network
    Wifi {
        #[command(subcommand)]
        action: WifiAction,
    },
    /// CUPS backend (reads a job from stdin and forwards to the printer)
    #[command(hide = true)]
    CupsBackend,
    /// Detect the printer over USB and show its device ID
    Usb,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the current configuration as TOML
    Show,
    /// Write a default configuration file and print its path
    Init,
    /// Set a single configuration key
    Set { key: String, value: String },
}

#[derive(Subcommand)]
enum WifiAction {
    /// Show the current wireless configuration
    Status(WifiArgs),
    /// Configure the wireless network (Infrastructure mode)
    Set {
        #[command(flatten)]
        args: WifiArgs,
        /// Wireless network name (SSID)
        #[arg(long)]
        ssid: String,
        /// WPA/WPA2 passphrase (configures WPA2-PSK AES)
        #[arg(long)]
        passphrase: Option<String>,
        /// Use an open (unencrypted) network
        #[arg(long)]
        open: bool,
    },
}

#[derive(Args, Clone)]
struct WifiArgs {
    /// Printer IP or hostname (auto-discovered if omitted)
    #[arg(long)]
    host: Option<String>,
    /// Web admin password (default: access)
    #[arg(long, default_value = "access")]
    password: String,
    /// SNMP community (for discovery)
    #[arg(long, default_value = "public")]
    community: String,
    /// Discovery timeout in seconds
    #[arg(long, default_value_t = 5)]
    timeout: u64,
}

#[derive(Args, Clone)]
struct PrinterArgs {
    /// Printer IP or hostname (auto-discovered if omitted)
    #[arg(long)]
    host: Option<String>,
    /// SNMP community string
    #[arg(long, default_value = "public")]
    community: String,
    /// Operation timeout in seconds
    #[arg(long, default_value_t = 5)]
    timeout: u64,
    /// Transport: raw (port 9100) or lpr (port 515)
    #[arg(long, value_enum, default_value_t = Transport::Raw)]
    transport: Transport,
    /// LPR queue name (with --transport lpr)
    #[arg(long)]
    queue: Option<String>,
}

#[derive(Args, Clone)]
struct PrintOpts {
    /// Paper size (letter, legal, a4, a5, a6, executive, b5, ...)
    #[arg(long)]
    paper: Option<String>,
    /// Duplex mode (off, long-edge, short-edge)
    #[arg(long)]
    duplex: Option<String>,
    /// Orientation (portrait, landscape)
    #[arg(long)]
    orientation: Option<String>,
    /// Print resolution in dpi (300 or 600)
    #[arg(long)]
    resolution: Option<u32>,
    /// Number of copies
    #[arg(long)]
    copies: Option<u32>,
    /// Enable toner-save mode
    #[arg(long)]
    toner_save: bool,
    /// Job name shown on the printer display
    #[arg(long)]
    job_name: Option<String>,
    /// Font (courier, times, univers, letter-gothic)
    #[arg(long)]
    font: Option<String>,
    /// Font size in points
    #[arg(long)]
    size: Option<u16>,
    /// Pitch in characters per inch (fixed-pitch fonts)
    #[arg(long)]
    pitch: Option<u16>,
    /// Line spacing as a multiple of the point size
    #[arg(long)]
    line_spacing: Option<f32>,
    /// Compose N images per sheet (N-up) when printing images
    #[arg(long)]
    nup: Option<usize>,
}

#[derive(ValueEnum, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Raw,
    Lpr,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load_or_default();

    match cli.command {
        Command::Discover { timeout } => run_discover(Duration::from_secs(timeout), &cfg),
        Command::Status { printer, json } => {
            let printer = resolve_printer(&printer, &cfg)?;
            let status = printer.status()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                print_status(&status);
            }
            Ok(())
        }
        Command::Monitor {
            printer,
            interval,
            once,
            json,
            webhook,
            command,
            exit_on_alert,
        } => {
            let printer = resolve_printer(&printer, &cfg)?;
            run_monitor(
                &printer,
                Duration::from_secs(interval.max(1)),
                once,
                json,
                webhook.as_deref(),
                command.as_deref(),
                exit_on_alert,
            )
        }
        Command::Info(args) => {
            let printer = resolve_printer(&args, &cfg)?;
            let info = printer.info()?;
            if info.is_empty() {
                println!("(no INFO ID response)");
            }
            for (k, v) in &info {
                println!("{k} = {v}");
            }
            Ok(())
        }
        Command::Print { files, printer, opts } => {
            let printer = resolve_printer(&printer, &cfg)?;
            let (job_opts, style) = build_print(&cfg, &opts)?;
            if let Some(nup) = opts.nup {
                if nup > 1 {
                    printer.print_images_nup(&files, nup, &job_opts)?;
                    println!("sent {} images ({nup}-up) to {}", files.len(), printer.addr);
                    return Ok(());
                }
            }
            for file in &files {
                printer.print_file(file, &job_opts, &style)?;
                println!("sent {} to {}", file.display(), printer.addr);
            }
            Ok(())
        }
        Command::Text {
            text,
            printer,
            font_file,
            size,
        } => {
            let printer = resolve_printer(&printer, &cfg)?;
            let text = match text {
                Some(t) => t,
                None => std::io::read_to_string(std::io::stdin())?,
            };
            let opts = cfg.job_options();
            match font_file {
                Some(fp) => printer.print_text_ttf(&text, &fp, size as f32, &opts)?,
                None => {
                    let style = cfg.text_style();
                    printer.print_text(&text, &opts, &style)?;
                }
            }
            println!("sent text to {}", printer.addr);
            Ok(())
        }
        Command::SelfTest { printer } => {
            let printer = resolve_printer(&printer, &cfg)?;
            let opts = cfg.job_options();
            printer.self_test(&opts)?;
            println!("sent self-test page to {}", printer.addr);
            Ok(())
        }
        Command::TestPage { printer } => {
            let printer = resolve_printer(&printer, &cfg)?;
            let opts = cfg.job_options();
            printer.print_marks(&opts)?;
            println!("sent corner-marks page to {}", printer.addr);
            Ok(())
        }
        Command::Reset(args) => {
            let printer = resolve_printer(&args, &cfg)?;
            printer.reset()?;
            println!("reset sent to {}", printer.addr);
            Ok(())
        }
        Command::Serve { printer, listen } => {
            let printer = resolve_printer(&printer, &cfg)?;
            brother_2270dw::serve::run(&printer, &cfg, &listen)
        }
        Command::Wifi { action } => match action {
            WifiAction::Status(args) => {
                let host = resolve_wifi_host(&args, &cfg)?;
                let s = brother_2270dw::wifi::status(&host, &args.password)?;
                println!("ssid:       {}", s.ssid);
                println!("mode:       {}", s.mode);
                println!("security:   {}", s.auth);
                println!("encryption: {}", s.encryption);
                Ok(())
            }
            WifiAction::Set {
                args,
                ssid,
                passphrase,
                open,
            } => {
                let host = resolve_wifi_host(&args, &cfg)?;
                if let Some(key) = passphrase {
                    brother_2270dw::wifi::configure(
                        &host,
                        &args.password,
                        &ssid,
                        brother_2270dw::wifi::AUTH_WPA_WPA2,
                        brother_2270dw::wifi::ENC_AES,
                        Some(&key),
                    )?;
                    println!("configured Wi-Fi on {host}: SSID \"{ssid}\", WPA2-PSK AES");
                } else if open {
                    brother_2270dw::wifi::configure(
                        &host,
                        &args.password,
                        &ssid,
                        brother_2270dw::wifi::AUTH_OPEN,
                        brother_2270dw::wifi::ENC_AES,
                        None,
                    )?;
                    println!("configured Wi-Fi on {host}: SSID \"{ssid}\", open (no encryption)");
                } else {
                    return Err(Error::InvalidArgument(
                        "wifi set requires --passphrase <KEY> or --open".into(),
                    ));
                }
                Ok(())
            }
        },
        Command::CupsBackend => brother_2270dw::cups::backend_main(),
        Command::Usb => {
            let p = brother_2270dw::usb::UsbPrinter::open()?;
            println!("{}", p.device_id()?);
            Ok(())
        }
        Command::Config { action } => run_config(action, &cfg),
    }
}

fn run_discover(timeout: Duration, cfg: &Config) -> Result<()> {
    let community = cfg.printer.community.clone();
    let found = discovery::discover(timeout, &community);
    if found.is_empty() {
        println!("no printers found");
        return Ok(());
    }
    println!("found {} device(s):", found.len());
    for p in &found {
        print_discovered(p);
    }
    Ok(())
}

fn print_discovered(p: &DiscoveredPrinter) {
    let model = p.model.as_deref().unwrap_or("unknown model");
    let host = p.hostname.as_deref().unwrap_or("-");
    let port = p.port.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
    let tag = if p.is_brother { "Brother" } else { "" };
    println!("  {:<16} {:<28} port {:<5} {tag} {}", p.ip, host, port, model);
}

fn print_status(s: &DeviceStatus) {
    println!("Model:     {}", s.model.as_deref().unwrap_or("unknown"));
    println!("Firmware:  {}", s.firmware.as_deref().unwrap_or("unknown"));
    println!("Serial:    {}", s.serial.as_deref().unwrap_or("unknown"));
    println!("Hostname:  {}", s.hostname.as_deref().unwrap_or("unknown"));
    println!("Page count: {}", s.page_count.map(|n| n.to_string()).unwrap_or_else(|| "unknown".into()));
    println!("State:     {}", s.state.label());
    if !s.supplies.is_empty() {
        println!("Supplies:");
        for sup in &s.supplies {
            println!("  - {}", format_supply(sup));
        }
    }
    if !s.alerts.is_empty() {
        println!("Alerts:");
        for a in &s.alerts {
            println!("  - [{}] {}", a.severity, a.description);
        }
    }
}

fn format_supply(s: &Supply) -> String {
    let kind = s.kind.label();
    let name = if s.description.is_empty() { kind.to_string() } else { s.description.clone() };
    let level = match (s.qualitative, s.level, s.max) {
        (Some(q), _, _) => q.to_string(),
        (None, Some(l), Some(m)) if m > 0 => {
            let pct = (l as f32 / m as f32) * 100.0;
            format!("{l}/{m} {} ({pct:.0}%)", s.unit)
        }
        (None, Some(l), _) => format!("{l} {}", s.unit),
        _ => "unknown".to_string(),
    };
    let kind_tag = match s.kind {
        SupplyKind::Toner => "toner",
        SupplyKind::Drum => "drum",
        SupplyKind::Other => "supply",
    };
    format!("{kind_tag:<6} {name:<26} {level}")
}

fn run_monitor(
    printer: &Printer,
    interval: Duration,
    once: bool,
    json: bool,
    webhook: Option<&str>,
    command: Option<&str>,
    exit_on_alert: bool,
) -> Result<()> {
    let mut last_level: Option<AlertLevel> = None;
    loop {
        let status = printer.status()?;
        let level = monitor::assess(&status);
        if json {
            println!("{}", serde_json::to_string(&status)?);
        } else {
            print_status(&status);
            println!("health: {}", level.label());
        }
        // Alert only on a transition into warning/critical, to avoid spamming.
        if level > AlertLevel::Healthy && last_level != Some(level) {
            if let Some(url) = webhook {
                monitor::webhook_notify(url, &status)?;
            }
            if let Some(cmd) = command {
                monitor::command_notify(cmd, &status)?;
            }
        }
        last_level = Some(level);
        if once {
            if exit_on_alert && level > AlertLevel::Healthy {
                std::process::exit(level.exit_code());
            }
            break;
        }
        std::thread::sleep(interval);
    }
    Ok(())
}

fn resolve_wifi_host(args: &WifiArgs, cfg: &Config) -> Result<String> {
    if let Some(h) = &args.host {
        return Ok(discovery::resolve_host(h)?.to_string());
    }
    if !cfg.printer.host.is_empty() {
        return Ok(discovery::resolve_host(&cfg.printer.host)?.to_string());
    }
    let found = discovery::discover(Duration::from_secs(args.timeout), &args.community);
    let candidate = found
        .iter()
        .find(|p| p.is_brother)
        .or_else(|| found.first())
        .cloned()
        .ok_or(Error::NotFound)?;
    eprintln!("using discovered printer at {}", candidate.ip);
    Ok(candidate.ip.to_string())
}

fn resolve_printer(args: &PrinterArgs, cfg: &Config) -> Result<Printer> {
    let timeout = Duration::from_secs(args.timeout);
    let host = args
        .host
        .clone()
        .or_else(|| {
            if cfg.printer.host.is_empty() {
                None
            } else {
                Some(cfg.printer.host.clone())
            }
        });

    let ip = match host {
        Some(h) => discovery::resolve_host(&h)?,
        None => {
            let found = discovery::discover(timeout, &args.community);
            let candidate = found
                .iter()
                .find(|p| p.is_brother)
                .or_else(|| found.first())
                .cloned()
                .ok_or(Error::NotFound)?;
            eprintln!("using discovered printer at {}", candidate.ip);
            candidate.ip
        }
    };

    let mut printer = Printer::new(ip)
        .with_community(&args.community)
        .with_timeout(timeout);
    if args.transport == Transport::Lpr {
        let queue = args
            .queue
            .clone()
            .unwrap_or_else(|| cfg.printer.queue.clone());
        printer = printer.with_lpr(&queue);
    }
    Ok(printer)
}

fn build_print(cfg: &Config, opts: &PrintOpts) -> Result<(JobOptions, brother_2270dw::pcl::TextStyle)> {
    let mut job = cfg.job_options();
    if let Some(p) = &opts.paper {
        job.paper = p.parse::<Paper>()?;
    }
    if let Some(d) = &opts.duplex {
        job.duplex = d.parse::<Duplex>()?;
    }
    if let Some(o) = &opts.orientation {
        job.orientation = o.parse::<Orientation>()?;
    }
    if let Some(r) = opts.resolution {
        job.resolution = r.clamp(75, 300);
    }
    if let Some(c) = opts.copies {
        job.copies = c.max(1);
    }
    job.toner_save = opts.toner_save || job.toner_save;
    if let Some(n) = &opts.job_name {
        job.job_name = n.clone();
    }

    let mut style = cfg.text_style();
    if let Some(f) = &opts.font {
        let size = opts.size.unwrap_or(style.font.point_size);
        let pitch = opts.pitch.unwrap_or(cfg.defaults.pitch.max(1));
        style.font = match f.to_ascii_lowercase().as_str() {
            "times" => brother_2270dw::pcl::Font::times(size),
            "univers" | "helvetica" | "arial" => brother_2270dw::pcl::Font::univers(size),
            "letter-gothic" | "letter_gothic" => {
                brother_2270dw::pcl::Font::letter_gothic(size, pitch)
            }
            _ => brother_2270dw::pcl::Font::courier(size, pitch),
        };
    } else if let Some(size) = opts.size {
        style.font.point_size = size;
    }
    if let Some(ls) = opts.line_spacing {
        style.line_spacing = ls.max(0.5);
    }

    Ok((job, style))
}

fn run_config(action: ConfigAction, cfg: &Config) -> Result<()> {
    match action {
        ConfigAction::Show => {
            let text = toml::to_string_pretty(cfg)
                .map_err(|e| Error::Config(e.to_string()))?;
            print!("{text}");
            Ok(())
        }
        ConfigAction::Init => {
            let path = Config::path();
            cfg.save()?;
            println!("wrote default configuration to {}", path.display());
            Ok(())
        }
        ConfigAction::Set { key, value } => {
            let mut cfg = cfg.clone();
            set_key(&mut cfg, &key, &value)?;
            cfg.save()?;
            println!("set {key} = {value}");
            Ok(())
        }
    }
}

fn set_key(cfg: &mut Config, key: &str, value: &str) -> Result<()> {
    match key.to_ascii_lowercase().as_str() {
        "host" => cfg.printer.host = value.to_string(),
        "port" => cfg.printer.port = value.parse().map_err(|_| Error::InvalidArgument(format!("bad port: {value}")))?,
        "community" => cfg.printer.community = value.to_string(),
        "timeout" | "timeout-secs" | "timeout_secs" => {
            cfg.printer.timeout_secs = value.parse().map_err(|_| Error::InvalidArgument(format!("bad timeout: {value}")))?
        }
        "transport" => {
            if value != "raw" && value != "lpr" {
                return Err(Error::InvalidArgument("transport must be 'raw' or 'lpr'".into()));
            }
            cfg.printer.transport = value.to_string();
        }
        "queue" => cfg.printer.queue = value.to_string(),
        "paper" => {
            value.parse::<Paper>()?;
            cfg.defaults.paper = value.to_string();
        }
        "duplex" => {
            value.parse::<Duplex>()?;
            cfg.defaults.duplex = value.to_string();
        }
        "orientation" => {
            value.parse::<Orientation>()?;
            cfg.defaults.orientation = value.to_string();
        }
        "resolution" => {
            cfg.defaults.resolution = value.parse().map_err(|_| Error::InvalidArgument(format!("bad resolution: {value}")))?
        }
        "copies" => {
            cfg.defaults.copies = value.parse().map_err(|_| Error::InvalidArgument(format!("bad copies: {value}")))?
        }
        "toner-save" | "toner_save" => {
            cfg.defaults.toner_save = parse_bool(value)?;
        }
        "font" => cfg.defaults.font = value.to_string(),
        "size" | "font-size" | "font_size" => {
            cfg.defaults.font_size = value.parse().map_err(|_| Error::InvalidArgument(format!("bad font size: {value}")))?
        }
        "pitch" => {
            cfg.defaults.pitch = value.parse().map_err(|_| Error::InvalidArgument(format!("bad pitch: {value}")))?
        }
        "line-spacing" | "line_spacing" => {
            cfg.defaults.line_spacing = value.parse().map_err(|_| Error::InvalidArgument(format!("bad line spacing: {value}")))?
        }
        _ => {
            return Err(Error::InvalidArgument(format!(
                "unknown configuration key '{key}'"
            )))
        }
    }
    Ok(())
}

fn parse_bool(s: &str) -> Result<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "on" | "yes" | "1" => Ok(true),
        "false" | "off" | "no" | "0" => Ok(false),
        _ => Err(Error::InvalidArgument(format!("expected a boolean, got {s}"))),
    }
}
