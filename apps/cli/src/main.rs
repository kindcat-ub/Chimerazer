//! `chimerazer` — read-only command-line interface (M1).

use std::fmt::Write as _;
use std::process::ExitCode;

use chimerazer_devices::{
    CommandSpec, Model, RAZER_VENDOR_ID, UsbId, exchange, is_control_interface, lookup, naga_pro,
    read_request,
};
use chimerazer_protocol::{
    ButtonInventory, DpiStages, DpiXy, LedBrightness, ProfileInventory, Response,
};
use chimerazer_transport::{HidInterface, Hidraw, enumerate};

const USAGE: &str = "\
usage:
  chimerazer list
  chimerazer read <CC:II> [ARG..] [--pid PID] [--txn XX]
  chimerazer dump [--pid PID] [--out FILE]
  chimerazer diff <FILE> <FILE>
  chimerazer validate <PARAM> --probe VALUE --baseline DUMP [--storage N]
                      [--led scroll_wheel|logo] [--pid PID] [--log FILE]
                      [--allow-source-only] [--hold STATE]
  chimerazer finish <STATE> --baseline DUMP [--log FILE]

  read   sends one catalogued read command; ARG are hex bytes
         (e.g. `chimerazer read 04:85 00`). Prints raw frames.
         --txn overrides the product's transaction ID (protocol research).
  dump   reads everything readable into normalized `key = value` lines
         (no serial number; transport details as `#` comments).
  diff   compares two dumps, ignoring comments and `volatile.*` keys.
  validate  first hardware validation of a write (M2): PARAM is poll-rate,
         dpi, dpi-stages or brightness. Refuses to write unless DUMP has no
         read errors and matches the device. Writes the same value, the
         probe and the original back, reading after each write, then checks
         the whole device against DUMP. --log writes the protocol (markdown).
         --hold: persistence check; write the probe, leave it in place and
         save STATE. Reconnect the mouse, then `finish` reads what survived,
         restores the original and checks the whole device against DUMP.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("list") => list(),
        Some("read") => read(&args[1..]),
        Some("dump") => dump(&args[1..]),
        Some("diff") => diff(&args[1..]),
        Some("validate") => validate(&args[1..]),
        Some("finish") => finish(&args[1..]),
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

fn list() -> Result<(), String> {
    let all = enumerate().map_err(|e| format!("cannot read sysfs: {e}"))?;
    for hid in all.iter().filter(|h| h.vendor_id == RAZER_VENDOR_ID) {
        let role = if is_control_interface(hid.vendor_id, hid.feature_report_len) {
            match lookup(hid.product_id) {
                Some((model, usb)) => format!("control: {} ({:?})", model.name, usb.link),
                None => "control: unknown model".to_string(),
            }
        } else {
            "-".to_string()
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

fn parse_command(model: &Model, s: &str) -> Result<&'static CommandSpec, String> {
    model
        .commands
        .iter()
        .find(|c| c.command.to_string().eq_ignore_ascii_case(s))
        .ok_or_else(|| format!("{s} is not in the {} command catalog", model.name))
}

/// The one Razer control interface matching `pid`, with its known model.
fn select(pid: Option<u16>) -> Result<(HidInterface, &'static Model, &'static UsbId), String> {
    let all = enumerate().map_err(|e| format!("cannot read sysfs: {e}"))?;
    let controls: Vec<&HidInterface> = all
        .iter()
        .filter(|h| is_control_interface(h.vendor_id, h.feature_report_len))
        .filter(|h| pid.is_none_or(|p| p == h.product_id))
        .collect();
    let hid = match controls.as_slice() {
        [one] => (*one).clone(),
        [] => return Err("no Razer control interface found (see `chimerazer list`)".into()),
        many => {
            let pids: Vec<String> = many
                .iter()
                .map(|h| format!("{:04x}", h.product_id))
                .collect();
            return Err(format!(
                "several devices found, choose with --pid {}",
                pids.join("|")
            ));
        }
    };
    let (model, usb) = lookup(hid.product_id).ok_or_else(|| {
        format!(
            "1532:{:04x} is a Razer control interface, but its model is not supported yet",
            hid.product_id
        )
    })?;
    Ok((hid, model, usb))
}

fn dump(args: &[String]) -> Result<(), String> {
    let mut pid = None;
    let mut out = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--pid" => {
                let v = it.next().ok_or("--pid needs a value")?;
                pid = Some(u16::from_str_radix(v, 16).map_err(|_| format!("bad pid {v}"))?);
            }
            "--out" => out = Some(it.next().ok_or("--out needs a value")?.clone()),
            other => return Err(format!("unexpected argument {other}\n{USAGE}")),
        }
    }
    let (hid, model, usb) = select(pid)?;
    if *model != naga_pro::MODEL {
        return Err(format!("dump is not implemented for {}", model.name));
    }
    let mut dev = Hidraw::open(&hid.node).map_err(|e| format!("{}: {e}", hid.node.display()))?;
    let snapshot = naga_pro::state::read_snapshot(&mut dev, usb);

    let mut text = format!(
        "# chimerazer dump\n# model {}\n# device 1532:{:04x} {:?} {}\n# exchanges {}\n",
        model.name,
        usb.product_id,
        usb.link,
        hid.node.display(),
        snapshot.traces.len()
    );
    text.push_str(&snapshot.to_text());
    match out {
        Some(path) => std::fs::write(&path, &text).map_err(|e| format!("{path}: {e}"))?,
        None => print!("{text}"),
    }
    let errors = snapshot
        .values
        .values()
        .filter(|v| v.starts_with("error:"))
        .count();
    eprintln!(
        "{} values, {} errors, {} exchanges",
        snapshot.values.len(),
        errors,
        snapshot.traces.len()
    );
    Ok(())
}

fn validate(args: &[String]) -> Result<(), String> {
    use naga_pro::params::{Param, validate as run};

    let (name, rest) = args.split_first().ok_or(USAGE)?;
    let (mut probe, mut baseline, mut log, mut pid) = (None, None, None, None);
    let mut hold_state: Option<String> = None;
    let (mut storage, mut led, mut allow_source_only) = (naga_pro::LIVE_STORAGE, None, false);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--probe" => probe = Some(value()?.clone()),
            "--baseline" => baseline = Some(value()?.clone()),
            "--log" => log = Some(value()?.clone()),
            "--hold" => hold_state = Some(value()?.clone()),
            "--pid" => {
                let v = value()?;
                pid = Some(u16::from_str_radix(v, 16).map_err(|_| format!("bad pid {v}"))?);
            }
            "--storage" => {
                let v = value()?;
                storage = v.parse().map_err(|_| format!("bad storage {v}"))?;
            }
            "--led" => {
                led = Some(match value()?.as_str() {
                    "scroll_wheel" => naga_pro::LED_SCROLL_WHEEL,
                    "logo" => naga_pro::LED_LOGO,
                    other => return Err(format!("unknown LED {other}")),
                })
            }
            "--allow-source-only" => allow_source_only = true,
            other => return Err(format!("unexpected argument {other}\n{USAGE}")),
        }
    }
    let probe = probe.ok_or("--probe is required")?;
    let baseline_path = baseline.ok_or("--baseline is required (a dump taken just before)")?;
    let param = Param::parse(name, storage, led)?;

    let (hid, model, usb) = select(pid)?;
    if *model != naga_pro::MODEL {
        return Err(format!("validate is not implemented for {}", model.name));
    }
    let baseline_text =
        std::fs::read_to_string(&baseline_path).map_err(|e| format!("{baseline_path}: {e}"))?;
    if !baseline_text
        .lines()
        .any(|l| l == format!("# model {}", model.name))
    {
        return Err(format!("{baseline_path} is not a {} dump", model.name));
    }
    let baseline = naga_pro::state::parse_text(&baseline_text);

    let mut dev = Hidraw::open(&hid.node).map_err(|e| format!("{}: {e}", hid.node.display()))?;
    let mode = chimerazer_devices::WriteMode::FirstValidation { allow_source_only };
    let title = match &hold_state {
        Some(_) => format!("Persistence check, phase 1: {param}"),
        None => format!("Validation: {param}"),
    };
    let report = match &hold_state {
        None => run(&mut dev, usb, param, &probe, &baseline, mode),
        Some(state_path) => {
            let (report, original) =
                naga_pro::params::hold(&mut dev, usb, param, &probe, &baseline, mode);
            if let Some(original) = original {
                let probe_bytes = param.probe(&probe, &original)?;
                let state = format!(
                    "# chimerazer hold state\nparam = {name}\nstorage = {storage}\nled = {}\npid = {:04x}\noriginal = {}\nprobe = {}\nprobe_text = {probe}\nallow_source_only = {allow_source_only}\n",
                    led.map_or("-".to_string(), |l| format!("{l:02x}")),
                    usb.product_id,
                    hex(&original),
                    hex(&probe_bytes),
                );
                std::fs::write(state_path, state).map_err(|e| format!("{state_path}: {e}"))?;
            }
            report
        }
    };
    let md = render_report(&title, model, usb, &baseline, param, &probe, &report);
    finish_output(log, md, &report, param)?;
    if let (Some(path), true) = (&hold_state, report.passed) {
        eprintln!(
            "probe left in place; reconnect the mouse, then run: chimerazer finish {path} --baseline {baseline_path}"
        );
    }
    Ok(())
}

fn finish(args: &[String]) -> Result<(), String> {
    let (state_path, rest) = args.split_first().ok_or(USAGE)?;
    let (mut baseline_path, mut log) = (None, None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--baseline" => baseline_path = Some(value()?.clone()),
            "--log" => log = Some(value()?.clone()),
            other => return Err(format!("unexpected argument {other}\n{USAGE}")),
        }
    }
    let baseline_path = baseline_path.ok_or("--baseline is required")?;
    let state_text =
        std::fs::read_to_string(state_path).map_err(|e| format!("{state_path}: {e}"))?;
    let state = naga_pro::state::parse_text(&state_text);
    let field = |k: &str| state.get(k).ok_or(format!("{state_path}: missing {k}"));
    let bytes = |k: &str| -> Result<Vec<u8>, String> {
        field(k)?
            .split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).map_err(|_| format!("{state_path}: bad {k}")))
            .collect()
    };
    let led = match field("led")?.as_str() {
        "-" => None,
        l => Some(u8::from_str_radix(l, 16).map_err(|_| format!("{state_path}: bad led"))?),
    };
    let storage: u8 = field("storage")?
        .parse()
        .map_err(|_| format!("{state_path}: bad storage"))?;
    let param = naga_pro::params::Param::parse(field("param")?, storage, led)?;
    let pid = u16::from_str_radix(field("pid")?, 16).map_err(|_| "bad pid".to_string())?;
    let (original, probe) = (bytes("original")?, bytes("probe")?);
    let allow_source_only = field("allow_source_only")? == "true";

    let (hid, model, usb) = select(Some(pid))?;
    let baseline_text =
        std::fs::read_to_string(&baseline_path).map_err(|e| format!("{baseline_path}: {e}"))?;
    let baseline = naga_pro::state::parse_text(&baseline_text);
    let mut dev = Hidraw::open(&hid.node).map_err(|e| format!("{}: {e}", hid.node.display()))?;
    let mode = chimerazer_devices::WriteMode::FirstValidation { allow_source_only };
    let report =
        naga_pro::params::finish_hold(&mut dev, usb, param, &original, &probe, &baseline, mode);
    let md = render_report(
        &format!("Persistence check, phase 2: {param}"),
        model,
        usb,
        &baseline,
        param,
        field("probe_text")?,
        &report,
    );
    finish_output(log, md, &report, param)
}

fn render_report(
    title: &str,
    model: &Model,
    usb: &UsbId,
    baseline: &std::collections::BTreeMap<String, String>,
    param: naga_pro::params::Param,
    probe: &str,
    report: &naga_pro::params::Report,
) -> String {
    let write = param.write_spec();
    let mut md = format!(
        "# {title}\n\n- model: {}\n- device: 1532:{:04x} {:?}\n- firmware: {}\n- write: `{}` {} [{}]\n- read-back: `{}` {}\n- probe: `{probe}`\n- result: **{}**\n\n## Steps\n",
        model.name,
        usb.product_id,
        usb.link,
        baseline.get("device.firmware").map_or("?", String::as_str),
        write.command,
        write.name,
        write.confirmation.label(),
        param.read_spec().command,
        param.read_spec().name,
        if report.passed { "PASSED" } else { "FAILED" },
    );
    for (i, step) in report.steps.iter().enumerate() {
        md.push_str(&format!(
            "\n### {}. {} — {}\n\n",
            i + 1,
            step.name,
            if step.ok { "ok" } else { "FAILED" }
        ));
        if let Some(e) = &step.expected {
            md.push_str(&format!("- expected: `{e}`\n"));
        }
        if let Some(g) = &step.got {
            md.push_str(&format!("- read back: `{g}`\n"));
        }
        if !step.note.is_empty() {
            md.push_str(&format!("- note: {}\n", step.note));
        }
        // Snapshots are summarised; their hundreds of frames are not logged.
        if !step.traces.is_empty() && step.traces.len() <= 4 {
            md.push_str("\n```\n");
            for t in &step.traces {
                md.push_str(&format!("> {}\n", hex(&t.request)));
                for r in &t.responses {
                    md.push_str(&format!("< {}\n", hex(r)));
                }
            }
            md.push_str("```\n");
        }
    }
    md
}

fn finish_output(
    log: Option<String>,
    md: String,
    report: &naga_pro::params::Report,
    param: naga_pro::params::Param,
) -> Result<(), String> {
    match log {
        Some(path) => std::fs::write(&path, &md).map_err(|e| format!("{path}: {e}"))?,
        None => print!("{md}"),
    }
    if report.passed {
        eprintln!("PASSED: {param}");
        Ok(())
    } else {
        Err(format!("FAILED: {param} (see the protocol)"))
    }
}

fn diff(args: &[String]) -> Result<(), String> {
    let [a, b] = args else {
        return Err(USAGE.to_string());
    };
    let read = |p: &String| std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"));
    let (left, right) = (
        naga_pro::state::parse_text(&read(a)?),
        naga_pro::state::parse_text(&read(b)?),
    );
    let diffs = naga_pro::state::compare(&left, &right);
    for (key, l, r) in &diffs {
        println!(
            "{key}: {} -> {}",
            l.as_deref().unwrap_or("<missing>"),
            r.as_deref().unwrap_or("<missing>")
        );
    }
    if diffs.is_empty() {
        println!("identical ({} keys compared)", left.len());
        Ok(())
    } else {
        Err(format!("{} differences", diffs.len()))
    }
}

fn read(args: &[String]) -> Result<(), String> {
    let (cmd, rest) = args.split_first().ok_or(USAGE)?;
    let mut pid = None;
    let mut txn = None;
    let mut bytes = Vec::new();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        if a == "--txn" {
            let v = it.next().ok_or("--txn needs a value")?;
            txn = Some(u8::from_str_radix(v, 16).map_err(|_| format!("bad txn {v}"))?);
        } else if a == "--pid" {
            let v = it.next().ok_or("--pid needs a value")?;
            pid = Some(u16::from_str_radix(v, 16).map_err(|_| format!("bad pid {v}"))?);
        } else {
            bytes.push(u8::from_str_radix(a, 16).map_err(|_| format!("bad hex byte {a}"))?);
        }
    }

    let (hid, model, usb) = select(pid)?;
    let spec = parse_command(model, cmd)?;

    let usb = &UsbId {
        transaction_id: txn.unwrap_or(usb.transaction_id),
        ..*usb
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
    println!("args     {}", hex(significant_args(&response.args)));
    if let Some(decoded) = decode(model, spec, &bytes, &response) {
        println!("decoded  {decoded}");
    }
    Ok(())
}

/// Arguments up to the last non-zero byte. `data_size` is not used: the
/// Naga Pro reports 0 for `05:80`/`05:84` and 5 for the 30-slot `02:84`.
fn significant_args(args: &[u8]) -> &[u8] {
    let end = args.iter().rposition(|&b| b != 0).map_or(1, |i| i + 1);
    &args[..end]
}

fn decode(model: &Model, spec: &CommandSpec, req: &[u8], r: &Response) -> Option<String> {
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
    } else if c == naga_pro::BUTTON_FUNCTION.command && *model == naga_pro::MODEL {
        let (&storage, &slot) = (req.first()?, req.get(1)?);
        format!("{:?}", naga_pro::decode_button_function(a, storage, slot))
    } else {
        return None;
    };
    Some(out)
}
