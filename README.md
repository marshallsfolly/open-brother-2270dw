# open-brother-2270dw

A fully featured, dependency-light Rust driver for the **Brother HL-2270DW**
monochrome network laser printer. It talks to the printer over Ethernet or
Wi-Fi, discovers it on the LAN, reads live status, and submits print jobs —
with zero dependence on CUPS or Brother's proprietary tools.

It is packaged as a Nix flake so it can be built as a derivation on any Nix
system, and it also builds with plain `cargo`.

## Features

- **Discovery** — find the printer on the LAN three ways: mDNS/DNS-SD,
  SNMP broadcast, and a raw port-9100 subnet scan (reliable even when the
  printer ignores multicast and broadcast).
- **Status** — live toner/drum levels, page counters, device identity and
  active alerts, read over SNMP from the Host Resources MIB and Printer MIB
  (RFC 1759/3805).
- **Monitoring & alerts** — `status --json`, a `monitor` command that polls and
  alerts on low toner/drum or errors (webhook or shell command), and
  Prometheus metrics.
- **Device info** — `@PJL INFO ID` (model, machine ID, firmware).
- **Transports** — raw TCP printing on port 9100 (HP JetDirect) and LPR on
  port 515 (RFC 1179).
- **Job control** — PJL for paper, orientation, duplex, resolution, copies and
  toner-save.
- **Page generation** —
  - PCL5e text using the printer's resident scalable fonts;
  - PCL5e 300 dpi raster graphics (Floyd–Steinberg dithering + TIFF packbits)
    for images, with N-up layout;
  - TrueType/Unicode text rendering (via `fontdue`) for arbitrary fonts and
    scripts;
  - BR-Script 3 / PostScript passthrough, and PDF printing (converted to
    vector PostScript, so text stays crisp and toner-efficient).
- **Web server** — a `serve` command exposing a status page, JSON, Prometheus
  metrics, and a print endpoint.
- **CUPS** — a backend binary + PPD for system-wide printing.
- **Configuration** — persistent TOML config plus a CLI to manage it.

## Prerequisites

- A network-reachable HL-2270DW (Ethernet or Wi-Fi). Default SNMP community is
  `public`.

## Building

### With Nix

```console
$ nix build          # builds ./result/bin/brother-2270dw
$ nix develop        # dev shell with rustc, cargo, rustfmt, clippy
```

The derivation pins `nixpkgs` in `flake.lock`, so `nix build` is reproducible
on any Nix system with flakes enabled.

### With cargo

```console
$ cargo build --release
```

## Usage

```console
$ brother-2270dw discover            # find printers on the LAN
$ brother-2270dw status              # toner/drum/page-count/alerts (auto-discovers)
$ brother-2270dw status --json       # machine-readable status
$ brother-2270dw info                # device identity via PJL
$ brother-2270dw print report.txt    # print a text file
$ brother-2270dw print photo.png     # print an image (fit to page)
$ brother-2270dw print doc.pdf       # print a PDF (vector PostScript)
$ brother-2270dw print job.pcl       # raw PCL passthrough
$ brother-2270dw print page.ps       # PostScript passthrough
$ brother-2270dw print a.png b.png --nup 4   # 4 images per sheet
$ brother-2270dw text "hello" --font-file /path/Font.ttf --size 14   # Unicode text
$ brother-2270dw monitor --once --exit-on-alert       # health check for cron
$ brother-2270dw monitor --interval 60 --webhook https://ntfy.sh/topic
$ brother-2270dw serve --listen 0.0.0.0:8080          # HTTP status/print server
$ brother-2270dw wifi status         # show current wireless config
$ brother-2270dw wifi set --ssid HomeNet --passphrase "secret"  # join Wi-Fi
$ brother-2270dw usb                 # detect printer over USB + device ID
$ brother-2270dw self-test           # built-in test page
$ brother-2270dw test-page           # single raster alignment page (corner marks)
$ brother-2270dw reset               # PJL reset
$ brother-2270dw config show         # print current config (TOML)
$ brother-2270dw config init         # write a default config file
$ brother-2270dw config set host 192.168.1.50
```

Every command that needs a printer accepts `--host` (IP or hostname) and
auto-discovers when it is omitted. Use `--help` on any subcommand for all
options.

### Setting up Wi-Fi

Brother configures the HL-2270DW's Wi-Fi over **Ethernet** on Linux (their
USB wireless "wizard" is Windows/macOS-only proprietary software). The
first-time flow is:

1. Plug the printer into your router with an Ethernet cable.
2. `brother-2270dw discover` to find its IP (or `usb` to confirm it is seen).
3. `brother-2270dw wifi set --host <ip> --ssid "MyNet" --passphrase "…"`.
4. Unplug the Ethernet cable — the printer now joins your Wi-Fi network.

Use `wifi set --ssid NAME --open` for an unencrypted network. The web admin
password defaults to `access`; pass `--password` if you changed it.

### Print options

```console
$ brother-2270dw print doc.txt \
    --host 192.168.1.50 \
    --paper letter \
    --duplex long-edge \
    --orientation portrait \
    --resolution 300 \
    --copies 2 \
    --font courier --size 11 --pitch 10 \
    --line-spacing 1.2 \
    --toner-save
```

### Transport

```console
$ brother-2270dw print doc.txt --transport lpr --queue BINARY_P1
```

Use `--transport lpr` when raw port 9100 is blocked.

## Configuration

Config lives at `~/.config/open-brother-2270dw/config.toml`
(`$XDG_CONFIG_HOME` is respected). `config init` writes the defaults:

```toml
[printer]
host = ""          # empty = auto-discover
port = 9100
community = "public"
timeout_secs = 5
transport = "raw"  # or "lpr"
queue = "BINARY_P1"

[defaults]
paper = "letter"
duplex = "off"
orientation = "portrait"
resolution = 300
copies = 1
toner_save = false
font = "courier"
font_size = 12
pitch = 10
line_spacing = 1.0
```

Supported paper sizes: `letter`, `legal`, `a4`, `a5`, `a6`, `executive`,
`b5`, `com10`, `dl`, `c5`. Fonts: `courier`, `times`, `univers`,
`letter-gothic`.

## Library

The crate is usable as a library:

```rust
use std::time::Duration;
use brother_2270dw::{JobOptions, Printer};

let printer = Printer::new("192.168.1.50".parse().unwrap())
    .with_timeout(Duration::from_secs(5));

let status = printer.status()?;
println!("state = {:?}, pages = {:?}", status.state, status.page_count);

let opts = JobOptions::default();
printer.print_file("report.txt".as_ref(), &opts, &Default::default())?;
```

See the module docs in `src/` for the full API surface (`snmp`, `pcl`, `pjl`,
`postscript`, `status`, `discovery`, `job`, `config`, `monitor`, `serve`,
`font`, `pdf`, `cups`).

## How it works

- **SNMP** is implemented from scratch over UDP (ASN.1 BER codec in
  `src/ber.rs`, client in `src/snmp.rs`) — no external SNMP library.
- **mDNS** is a minimal DNS-SD querier in `src/mdns.rs` (DNS packet
  encoding/parsing with name compression).
- **PCL5e** generation is in `src/pcl.rs`; **PostScript/BR-Script** in
  `src/postscript.rs`; **PJL** in `src/pjl.rs`.
- **PDF** printing converts PDF → PostScript with `pdftops` (poppler-utils),
  so text stays vector. **TrueType text** uses `fontdue`. **Web/JSON** uses
  `tiny_http`/`serde_json`; **webhooks** use `ureq`.
- The protocol core (SNMP/mDNS/PCL/PJL/PS) is dependency-free; the few helper
  crates are `clap`, `image`, `serde` + `toml`, `serde_json`, `thiserror`,
  `tiny_http`, `fontdue`, and `ureq`.

## CUPS

```console
$ sudo ln -s $(nix build --print-out-paths --no-link)/bin/brother2270-cups-backend \
    /usr/lib/cups/backend/brother2270
$ sudo lpadmin -p HL2270DW -E -v brother2270://192.168.1.50:9100 \
    -P $out/share/ppd/brother-hl-2270dw.ppd
$ lp -d HL2270DW document.pdf
```

## Tested against

Brother HL-2270DW, firmware `1.03` —
discovery, SNMP status, PJL `INFO ID`, PCL text + 300 dpi raster printing, and
PDF (vector PostScript) printing were all verified live on Letter (8.5×11)
paper. The printer's PCL5e raster engine renders at 300 dpi, and its
logical-page origin is offset ~0.42″ right of the physical corner, which the
driver compensates for in `src/pcl.rs` (`ORIGIN_OFFSET_*`).

## License

MIT — see `LICENSE`.
