//! `chimerazer` — read-only command-line interface (M1).

use std::fmt::Write as _;
use std::process::ExitCode;

use chimerazer_devices::{CommandSpec, RAZER_VENDOR_ID, UsbId, exchange, naga_pro, read_request};
use chimerazer_protocol::{
    ButtonInventory, DpiStages, DpiXy, LedBrightness, ProfileInventory, Response,
};
use chimerazer_transport::{HidInterface, Hidraw, enumerate};

const USAGE: &str = "\
usage:
  chimerazer list
  chimerazer read <CC:II> [ARG..] [--pid 008f|0090]

  read   sends one catalogued read command; ARG are hex bytes
         (e.g. `chimerazer read 04:85 00`). Prints raw frames.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("list") => list(),
        Some("read") => read(&args[1..]),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// The Razer control interface of a supported product, if `hid` is one.
fn control_usb(hid: &HidInterface) -> Option<&'static UsbId> {
    let usb = naga_pro::usb_id(hid.product_id)?;
    (hid.vendor_id == RAZER_VENDOR_ID
        && hid.interface == Some(usb.interface)
        && hid.feature_report_len == Some(chimerazer_protocol::REPORT_LEN))
    .then_some(usb)
}

fn list() -> Result<(), String> {
    let all = enumerate().map_err(|e| format!("cannot read sysfs: {e}"))?;
    for hid in all.iter().filter(|h| h.vendor_id == RAZER_VENDOR_ID) {
        let role = match control_usb(hid) {
            Some(usb) => format!("{} control ({:?})", naga_pro::MODEL, usb.link),
            None => "-".to_string(),
        };
        println!(
            "{}  {:04x}:{:04x}  interface {}  feature {}  {role}",
            hid.node.display(),
            hid.vendor_id,
            hid.product_id,
            hid.interface.map_or("?".into(), |i| i.to_string()),
            hid.feature_report_len.map_or("-".into(), |n| n.to_string()),
        );
    }
    Ok(())
}

fn parse_command(s: &str) -> Result<&'static CommandSpec, String> {
    naga_pro::COMMANDS
        .iter()
        .find(|c| c.command.to_string().eq_ignore_ascii_case(s))
        .ok_or_else(|| format!("{s} is not in the Naga Pro command catalog"))
}

fn read(args: &[String]) -> Result<(), String> {
    let (cmd, rest) = args.split_first().ok_or(USAGE)?;
    let spec = parse_command(cmd)?;
    let mut pid = None;
    let mut bytes = Vec::new();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        if a == "--pid" {
            let v = it.next().ok_or("--pid needs a value")?;
            pid = Some(u16::from_str_radix(v, 16).map_err(|_| format!("bad pid {v}"))?);
        } else {
            bytes.push(u8::from_str_radix(a, 16).map_err(|_| format!("bad hex byte {a}"))?);
        }
    }

    let all = enumerate().map_err(|e| format!("cannot read sysfs: {e}"))?;
    let candidates: Vec<(&HidInterface, &UsbId)> = all
        .iter()
        .filter_map(|h| control_usb(h).map(|u| (h, u)))
        .filter(|(_, u)| pid.is_none_or(|p| p == u.product_id))
        .collect();
    let (hid, usb) = match candidates.as_slice() {
        [one] => *one,
        [] => return Err("no supported device found (see `chimerazer list`)".into()),
        many => {
            let pids: Vec<String> = many
                .iter()
                .map(|(_, u)| format!("{:04x}", u.product_id))
                .collect();
            return Err(format!(
                "several devices found, choose with --pid {}",
                pids.join("|")
            ));
        }
    };

    let request = read_request(usb, spec, &bytes).map_err(|e| e.to_string())?;
    let mut dev = Hidraw::open(&hid.node).map_err(|e| format!("{}: {e}", hid.node.display()))?;

    println!(
        "device   {} {:04x}:{:04x} {:?}",
        hid.node.display(),
        hid.vendor_id,
        usb.product_id,
        usb.link
    );
    println!(
        "command  {} {} [{}]",
        spec.command,
        spec.name,
        spec.confirmation.label()
    );
    let (trace, result) = exchange(&mut dev, usb, &request);
    println!("> {}", hex(&trace.request));
    for frame in &trace.responses {
        println!("< {}", hex(frame));
    }
    let response = result.map_err(|e| format!("error    {e}"))?;
    println!(
        "status   {:?}  data_size {}",
        response.status, response.data_size
    );
    println!(
        "args     {}",
        hex(&response.args[..usize::from(response.data_size).max(1)])
    );
    if let Some(decoded) = decode(spec, &bytes, &response) {
        println!("decoded  {decoded}");
    }
    Ok(())
}

fn decode(spec: &CommandSpec, req: &[u8], r: &Response) -> Option<String> {
    let a = &r.args;
    let c = spec.command;
    let out = if c == naga_pro::DPI.command {
        format!("{:?}", DpiXy::decode(a))
    } else if c == naga_pro::DPI_STAGES.command {
        format!("{:?}", DpiStages::decode(a))
    } else if c == naga_pro::LED_BRIGHTNESS.command {
        format!("{:?}", LedBrightness::decode(a))
    } else if c == naga_pro::PROFILE_INVENTORY.command {
        format!("{:?}", ProfileInventory::decode(a))
    } else if c == naga_pro::BUTTON_INVENTORY.command {
        format!("{:?}", ButtonInventory::decode(a))
    } else if c == naga_pro::BUTTON_FUNCTION.command {
        let (&storage, &slot) = (req.first()?, req.get(1)?);
        format!("{:?}", naga_pro::decode_button_function(a, storage, slot))
    } else {
        return None;
    };
    Some(out)
}
