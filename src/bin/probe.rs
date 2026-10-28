//! rode-probe: CLI for the Wireless GO II (and raw HID exploration).
//!
//!   rode-probe list                       enumerate RØDE HID interfaces
//!   rode-probe desc                       dump + decode the report descriptor
//!   rode-probe status                     everything we can read
//!   rode-probe get <op>                   GET a report-9 register
//!   rode-probe gain <coarse|fine|camera> [value]
//!   rode-probe channel <split|merged> [--safety]
//!   rode-probe safety <on|off>
//!   rode-probe autodim <on|off>
//!   rode-probe rx-button <autodim|marker>
//!   rode-probe sync-clock                 RX + TX clock = now
//!   rode-probe tx-led <bright|dim>
//!   rode-probe tx-pad <on|off>
//!   rode-probe tx-compress <on|off>
//!   rode-probe tx-record <off|always|manual|backup> [--button mute|marker|record|disabled]
//!   rode-probe send <hex> [--len 17]      raw frame, prints the reply
//!   rode-probe listen [--seconds N]       print unsolicited input reports

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use hidapi::HidApi;
use rode_central_linux::device::GoII;
use rode_central_linux::hid::{enumerate, RodeHid, RodeHidInfo};
use rode_central_linux::protocol::{self as p, ChannelMode, GainMode, Model, RecordMode, RxButton, TxButton, TxLed};
use std::time::Duration;

#[derive(Parser)]
struct Cli {
    /// Select by product id (hex, e.g. 002a). Defaults to the only device present.
    #[arg(long, global = true)]
    pid: Option<String>,
    /// Select by HID serial (HID_UNIQ).
    #[arg(long, global = true)]
    serial: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    List,
    Desc,
    Status,
    Get { op: String },
    Gain { mode: String, value: Option<u8> },
    Channel { mode: String, #[arg(long)] safety: bool },
    Safety { state: String },
    Autodim { state: String },
    RxButton { function: String },
    SyncClock,
    TxLed { level: String },
    TxPad { state: String },
    TxCompress { state: String },
    TxRecord { mode: String, #[arg(long, default_value = "marker")] button: String },
    Send {
        hex: String,
        #[arg(long, default_value_t = 17)]
        len: usize,
        #[arg(long, default_value_t = 2000)]
        timeout_ms: u64,
    },
    Listen {
        #[arg(long, default_value_t = 30)]
        seconds: u64,
    },
}

fn pick(api: &HidApi, cli: &Cli) -> Result<RodeHidInfo> {
    let mut devs = enumerate(api);
    if let Some(p) = &cli.pid {
        let p = u16::from_str_radix(p.trim_start_matches("0x"), 16)?;
        devs.retain(|d| d.pid == p);
    }
    if let Some(s) = &cli.serial {
        devs.retain(|d| &d.serial == s);
    }
    match devs.len() {
        0 => bail!("no matching RØDE HID device"),
        1 => Ok(devs.remove(0)),
        n => bail!("{n} matching devices; narrow with --pid/--serial (see `list`)"),
    }
}

fn on_off(s: &str) -> Result<bool> {
    match s {
        "on" | "1" | "true" => Ok(true),
        "off" | "0" | "false" => Ok(false),
        _ => bail!("expected on|off, got {s}"),
    }
}

fn now() -> u32 {
    rode_central_linux::device::device_now()
}

fn print_desc(d: &[u8]) {
    // Minimal HID descriptor walker: enough to print report ids and sizes.
    let mut i = 0;
    let (mut rsize, mut rcount, mut rid) = (0u32, 0u32, 0u8);
    let mut indent: usize = 0;
    while i < d.len() {
        let b = d[i];
        let size = match b & 3 { 3 => 4, s => s as usize };
        let tag = b & 0xFC;
        let val: u32 = d[i + 1..i + 1 + size].iter().rev().fold(0, |a, &x| (a << 8) | x as u32);
        i += 1 + size;
        let bits = rsize * rcount;
        let line = match tag {
            0x04 => format!("Usage Page 0x{val:04X}"),
            0x08 => format!("Usage 0x{val:02X}"),
            0x14 => format!("Logical Min {val}"),
            0x24 => format!("Logical Max {val}"),
            0x74 => { rsize = val; format!("Report Size {val}") }
            0x94 => { rcount = val; format!("Report Count {val}") }
            0x84 => { rid = val as u8; format!("Report ID 0x{val:02X}") }
            0x80 => format!("INPUT   (id 0x{rid:02X}, {} B total)", bits / 8 + 1),
            0x90 => format!("OUTPUT  (id 0x{rid:02X}, {} B total)", bits / 8 + 1),
            0xB0 => format!("FEATURE (id 0x{rid:02X}, {} B payload)", bits / 8),
            0xA0 => { indent += 2; format!("Collection {val}") }
            0xC0 => { indent = indent.saturating_sub(2); "End Collection".into() }
            _ => format!("tag 0x{tag:02X} = {val}"),
        };
        println!("{}{line}", " ".repeat(indent));
    }
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();
    let api = HidApi::new()?;

    if let Cmd::List = cli.cmd {
        let devs = enumerate(&api);
        if devs.is_empty() {
            println!("No RØDE (VID 19F7) HID interfaces found.");
        }
        for d in devs {
            println!(
                "PID 0x{:04X}  {:<26} serial={:<12} usage={:04X}:{:04X}  {:?}  \"{}\"",
                d.pid, Model::from_pid(d.pid).name(), d.serial, d.usage_page, d.usage, d.path, d.product
            );
        }
        return Ok(());
    }

    let info = pick(&api, &cli)?;
    match &cli.cmd {
        Cmd::List => unreachable!(),
        Cmd::Desc => {
            let dev = RodeHid::open(&api, &info)?;
            let desc = dev.report_descriptor()?;
            println!("{} bytes: {}", desc.len(), hex::encode(&desc));
            print_desc(&desc);
        }
        Cmd::Send { hex, len, timeout_ms } => {
            let dev = RodeHid::open(&api, &info)?;
            let mut out = hex::decode(hex.replace(' ', ""))?;
            out.resize(*len, 0);
            println!("-> {}", hex::encode(&out));
            let resp = dev.transact(&out, 1600, Duration::from_millis(*timeout_ms))?;
            println!("<- {}", hex::encode(&resp));
            while let Some(r) = dev.read(Duration::from_millis(300))? {
                println!("<- {}", hex::encode(&r));
            }
        }
        Cmd::Listen { seconds } => {
            let dev = RodeHid::open(&api, &info)?;
            let end = std::time::Instant::now() + Duration::from_secs(*seconds);
            while std::time::Instant::now() < end {
                if let Some(r) = dev.read(Duration::from_millis(500))? {
                    println!("<- {}", hex::encode(&r));
                }
            }
        }
        other => {
            let dev = GoII::open(&api, &info)?;
            run_typed(&dev, other)?;
        }
    }
    Ok(())
}

fn run_typed(dev: &GoII, cmd: &Cmd) -> Result<()> {
    match cmd {
        Cmd::Status => {
            let s = dev.snapshot()?;
            println!("RX  {}  serial {:08X}  battery {}%  clock {}", s.name, s.serial, s.rx_battery, s.rx_clock);
            match s.gain_mode {
                Some(GainMode::Fine) => println!("    gain: Fine {} dB (step {})", p::fine_step_db(s.gain_value), s.gain_value),
                Some(m) => println!("    gain: {} value {}", m.label(), s.gain_value),
                None => println!("    gain: unknown mode"),
            }
            match s.tx {
                Some(t) => {
                    println!(
                        "TX1 serial {:08X}  fw {}  battery {}%  rec {:?}  button {:?}  led {}  compressed {}",
                        t.serial, t.firmware(), t.battery, t.record_mode(), t.button(),
                        if t.led == TxLed::Dim as u8 { "dim" } else { "bright" }, !t.uncompressed
                    );
                    println!("    raw {}", hex::encode(t.raw));
                }
                None => println!("TX1 not connected"),
            }
        }
        Cmd::Get { op } => {
            let op = u8::from_str_radix(op.trim_start_matches("0x"), 16)?;
            println!("{}", hex::encode(dev.get(op)?));
        }
        Cmd::Gain { mode, value } => {
            let m = match mode.as_str() {
                "coarse" => GainMode::Coarse,
                "fine" => GainMode::Fine,
                "camera" => GainMode::Camera,
                _ => bail!("mode must be coarse|fine|camera"),
            };
            let v = value.unwrap_or(match m { GainMode::Fine => 10, _ => 0 });
            dev.set_gain(m, v)?;
        }
        Cmd::Channel { mode, safety } => {
            let m = match mode.as_str() {
                "split" => ChannelMode::Split,
                "merged" => ChannelMode::Merged,
                _ => bail!("mode must be split|merged"),
            };
            dev.set_channel_mode(m, *safety)?;
        }
        Cmd::Safety { state } => dev.set_safety(on_off(state)?)?,
        Cmd::Autodim { state } => dev.set_auto_dim(on_off(state)?)?,
        Cmd::RxButton { function } => dev.set_rx_button(match function.as_str() {
            "autodim" => RxButton::AutoDim,
            "marker" => RxButton::Marker,
            _ => bail!("function must be autodim|marker"),
        })?,
        Cmd::SyncClock => {
            let t = now();
            dev.set_rx_clock(t)?;
            dev.set_tx_clock(t)?;
            println!("clock set to {t}");
        }
        Cmd::TxLed { level } => dev.set_tx_led(match level.as_str() {
            "bright" => TxLed::Bright,
            "dim" => TxLed::Dim,
            _ => bail!("level must be bright|dim"),
        })?,
        Cmd::TxPad { state } => dev.set_tx_pad(on_off(state)?)?,
        Cmd::TxCompress { state } => dev.set_tx_compress(on_off(state)?)?,
        Cmd::TxRecord { mode, button } => {
            let rec = match mode.as_str() {
                "off" => RecordMode::Off,
                "always" => RecordMode::Always,
                "manual" => RecordMode::Manual,
                "backup" => RecordMode::Backup,
                _ => bail!("mode must be off|always|manual|backup"),
            };
            let btn = match button.as_str() {
                "mute" => TxButton::Mute,
                "marker" => TxButton::Marker,
                "record" => TxButton::Record,
                "disabled" => TxButton::Disabled,
                _ => bail!("button must be mute|marker|record|disabled"),
            };
            dev.set_tx_record_and_button(rec, btn)?;
        }
        _ => unreachable!(),
    }
    println!("ok");
    Ok(())
}
