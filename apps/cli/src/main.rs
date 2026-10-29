//! `chimerazer` — command-line interface.

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
                      [--allow-source-only]
  chimerazer hold ITEM.. --baseline DUMP --state FILE [--pid PID] [--log FILE]
  chimerazer finish <STATE> --baseline DUMP [--log FILE]
  chimerazer restore <DUMP> [--dry-run] [--pid PID] [--log FILE]
  chimerazer set ITEM.. [--pid PID]
  chimerazer buttons <DUMP> [--storage N]

  read   sends one catalogued read command; ARG are hex bytes
         (e.g. `chimerazer read 04:85 00`). Prints raw frames.
         --txn overrides the product's transaction ID (protocol research).
  dump   reads everything readable into normalized `key = value` lines
         (no serial number; transport details as `#` comments).
  diff   compares two dumps, ignoring comments and `volatile.*` keys.
  validate  first hardware validation of a write (M2): PARAM is poll-rate,
         dpi, dpi-stages, brightness or effect (probe RRGGBB for static
         colour, or spectrum). Refuses to write unless DUMP has no
         read errors and matches the device. Writes the same value, the
         probe and the original back, reading after each write, then checks
         the whole device against DUMP. --log writes the protocol (markdown).
  hold   persistence check for several values with one reconnect. ITEM is
         dpi:<storage>:<probe>, dpi-stages:<storage>:active=<id> or
         brightness:<storage>:<logo|scroll_wheel>:<probe> or
         effect:<storage>:<logo|scroll_wheel>:<RRGGBB|spectrum>. Writes each probe
         and leaves it in place; `finish` reads what survived the reconnect,
         restores the originals and checks the whole device against DUMP.
  restore  writes back the values of DUMP that have a verified write on
         this link (DPI, DPI stages, LED brightness, LED effect by wire;
         see VERIFIED_WRITES). An unverified value is skipped and fails
         the run if it differs. Refuses a DUMP with read errors or another active profile.
         One value at a time, read back after each; no atomicity: a failure
         stops the run and leaves earlier values written. --dry-run only
         reports what would change.
  set    writes values in normal mode (ITEM as for hold, e.g. dpi:1:800,
         effect:1:logo:ff00ff); only writes verified on this link, each
         read back. Stops at the first failure.
         Buttons: button:<storage>:<slot>:<binding>, binding is disabled,
         mouse:<left|right|middle|back|forward|scroll-up|scroll-down>,
         key:[mods+]<key> (ctrl shift alt super, r-prefixed for right),
         dpi:<down|up|cycle>, profile-cycle or raw:<14 hex>; turbo=<rate>
         after mouse/key (key:up,turbo=55 or quoted \"key:up turbo=55\").
  buttons  decodes the 30 button slots of a dump (no device access).";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("list") => list(),
        Some("read") => read(&args[1..]),
        Some("dump") => dump(&args[1..]),
        Some("diff") => diff(&args[1..]),
        Some("validate") => validate(&args[1..]),
        Some("hold") => hold(&args[1..]),
        Some("finish") => finish(&args[1..]),
        Some("restore") => restore(&args[1..]),
        Some("set") => set(&args[1..]),
        Some("buttons") => buttons(&args[1..]),
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
    let (mut storage, mut led, mut allow_source_only) = (naga_pro::LIVE_STORAGE, None, false);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--probe" => probe = Some(value()?.clone()),
            "--baseline" => baseline = Some(value()?.clone()),
            "--log" => log = Some(value()?.clone()),
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
    let report = run(&mut dev, usb, param, &probe, &baseline, mode);
    let md = render_report(
        &format!("Validation: {param}"),
        model,
        usb,
        &baseline,
        param,
        &probe,
        &report,
    );
    finish_output(log, md, &report, param)
}

/// `dpi:1:800`, `dpi-stages:1:active=2`, `brightness:1:logo:100`.
fn parse_item(item: &str) -> Result<(naga_pro::params::Param, String), String> {
    // The binding text may contain ':' itself (key:ctrl+f5).
    if let Some(rest) = item.strip_prefix("button:") {
        let mut parts = rest.splitn(3, ':');
        let (Some(st), Some(slot), Some(binding)) = (parts.next(), parts.next(), parts.next())
        else {
            return Err(format!("bad item {item} (e.g. button:1:80:key:f5)"));
        };
        let num = |s: &str| s.parse::<u8>().map_err(|_| format!("bad number in {item}"));
        return Ok((
            naga_pro::params::Param::button(num(st)?, num(slot)?)?,
            binding.to_string(),
        ));
    }
    let parts: Vec<&str> = item.split(':').collect();
    let storage = |s: &str| {
        s.parse::<u8>()
            .map_err(|_| format!("bad storage in {item}"))
    };
    let (param, probe) = match parts.as_slice() {
        [name @ ("brightness" | "effect"), st, led, probe] => {
            let led = match *led {
                "scroll_wheel" => naga_pro::LED_SCROLL_WHEEL,
                "logo" => naga_pro::LED_LOGO,
                other => return Err(format!("unknown LED {other}")),
            };
            (
                naga_pro::params::Param::parse(name, storage(st)?, Some(led))?,
                *probe,
            )
        }
        [name, st, probe] => (
            naga_pro::params::Param::parse(name, storage(st)?, None)?,
            *probe,
        ),
        _ => {
            return Err(format!(
                "bad item {item} (e.g. dpi:1:800, brightness:1:logo:100)"
            ));
        }
    };
    Ok((param, probe.to_string()))
}

fn hold(args: &[String]) -> Result<(), String> {
    let (mut baseline_path, mut state_path, mut log, mut pid) = (None, None, None, None);
    let mut allow_source_only = false;
    let mut items = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--baseline" => baseline_path = Some(value()?.clone()),
            "--state" => state_path = Some(value()?.clone()),
            "--log" => log = Some(value()?.clone()),
            "--pid" => {
                let v = value()?;
                pid = Some(u16::from_str_radix(v, 16).map_err(|_| format!("bad pid {v}"))?);
            }
            "--allow-source-only" => allow_source_only = true,
            item => items.push(parse_item(item)?),
        }
    }
    let baseline_path = baseline_path.ok_or("--baseline is required")?;
    let state_path = state_path.ok_or("--state is required")?;
    if items.is_empty() {
        return Err(USAGE.to_string());
    }
    let (hid, model, usb) = select(pid)?;
    if *model != naga_pro::MODEL {
        return Err(format!("hold is not implemented for {}", model.name));
    }
    let baseline = load_baseline(&baseline_path, model)?;
    let mut dev = Hidraw::open(&hid.node).map_err(|e| format!("{}: {e}", hid.node.display()))?;
    let mode = chimerazer_devices::WriteMode::FirstValidation { allow_source_only };
    let refs: Vec<(naga_pro::params::Param, &str)> =
        items.iter().map(|(p, t)| (*p, t.as_str())).collect();
    let (report, held) = naga_pro::params::hold(&mut dev, usb, &refs, &baseline, mode);
    if let Some(held) = &held {
        let mut state = format!(
            "# chimerazer hold state\npid = {:04x}\nallow_source_only = {allow_source_only}\ncount = {}\n",
            usb.product_id,
            held.len()
        );
        for (i, h) in held.iter().enumerate() {
            state.push_str(&format!(
                "item.{i}.param = {}\nitem.{i}.original = {}\nitem.{i}.probe = {}\n",
                param_token(h.param),
                hex(&h.original),
                hex(&h.probe)
            ));
        }
        std::fs::write(&state_path, state).map_err(|e| format!("{state_path}: {e}"))?;
    }
    let probes: Vec<String> = items.iter().map(|(p, t)| format!("{p} = {t}")).collect();
    let md = render_report(
        "Persistence check, phase 1",
        model,
        usb,
        &baseline,
        report.param,
        &probes.join("; "),
        &report,
    );
    let label = format!("{} value(s): {}", items.len(), probes.join("; "));
    write_log(log, &md)?;
    if !report.passed {
        return Err(format!("FAILED: {label} (see the protocol)"));
    }
    eprintln!("PASSED: {label}");
    eprintln!(
        "probes left in place; reconnect the mouse, then run: chimerazer finish {state_path} --baseline {baseline_path}"
    );
    Ok(())
}

/// `dpi:1`, `dpi-stages:1`, `brightness:1:logo` — the state-file form.
fn param_token(p: naga_pro::params::Param) -> String {
    use naga_pro::params::Param::*;
    match p {
        PollRate => "poll-rate:0".into(),
        Button { storage, slot } => format!("button:{storage}:{slot}"),
        Dpi { storage } => format!("dpi:{storage}"),
        DpiStages { storage } => format!("dpi-stages:{storage}"),
        Brightness { storage, led } | Effect { storage, led } => format!(
            "{}:{storage}:{}",
            if matches!(p, Effect { .. }) {
                "effect"
            } else {
                "brightness"
            },
            if led == naga_pro::LED_LOGO {
                "logo"
            } else {
                "scroll_wheel"
            }
        ),
    }
}

fn load_baseline(
    path: &str,
    model: &Model,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    if !text.lines().any(|l| l == format!("# model {}", model.name)) {
        return Err(format!("{path} is not a {} dump", model.name));
    }
    Ok(naga_pro::state::parse_text(&text))
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
    let pid = u16::from_str_radix(field("pid")?, 16).map_err(|_| "bad pid".to_string())?;
    let count: usize = field("count")?
        .parse()
        .map_err(|_| "bad count".to_string())?;
    let mut held = Vec::new();
    for i in 0..count {
        let (param, _) = parse_item(&format!("{}:x", field(&format!("item.{i}.param"))?))?;
        held.push(naga_pro::params::Held {
            param,
            original: bytes(&format!("item.{i}.original"))?,
            probe: bytes(&format!("item.{i}.probe"))?,
        });
    }
    let allow_source_only = field("allow_source_only")? == "true";

    let (hid, model, usb) = select(Some(pid))?;
    let baseline = load_baseline(&baseline_path, model)?;
    let mut dev = Hidraw::open(&hid.node).map_err(|e| format!("{}: {e}", hid.node.display()))?;
    let mode = chimerazer_devices::WriteMode::FirstValidation { allow_source_only };
    let report = naga_pro::params::finish_hold(&mut dev, usb, &held, &baseline, mode);
    let md = render_report(
        "Persistence check, phase 2",
        model,
        usb,
        &baseline,
        report.param,
        "-",
        &report,
    );
    write_log(log, &md)?;
    let label = format!("{} value(s)", held.len());
    let persistence = match naga_pro::params::persisted(&report) {
        Some(true) => "persistence PASSED",
        Some(false) => "persistence FAILED",
        None => "persistence not checked",
    };
    if report.passed {
        eprintln!("PASSED: {label}, {persistence}");
        Ok(())
    } else {
        Err(format!(
            "FAILED: {label}, {persistence} (rollback and final diff in the protocol)"
        ))
    }
}

fn restore(args: &[String]) -> Result<(), String> {
    use naga_pro::restore::Outcome;

    let (dump_path, rest) = args.split_first().ok_or(USAGE)?;
    let (mut pid, mut log, mut dry_run) = (None, None, false);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--log" => log = Some(value()?.clone()),
            "--dry-run" => dry_run = true,
            "--pid" => {
                let v = value()?;
                pid = Some(u16::from_str_radix(v, 16).map_err(|_| format!("bad pid {v}"))?);
            }
            other => return Err(format!("unexpected argument {other}\n{USAGE}")),
        }
    }
    let (hid, model, usb) = select(pid)?;
    if *model != naga_pro::MODEL {
        return Err(format!("restore is not implemented for {}", model.name));
    }
    let dump = load_baseline(dump_path, model)?;
    let mut dev = Hidraw::open(&hid.node).map_err(|e| format!("{}: {e}", hid.node.display()))?;
    let report = naga_pro::restore::restore(&mut dev, usb, &dump, dry_run);

    let mut md = format!(
        "# Restore{}\n\n- model: {}\n- device: 1532:{:04x} {:?}\n- dump: `{dump_path}`\n\n\
         Values are written one at a time; the device has no transaction. \
         A failure stops the run and leaves the values written before it.\n",
        if dry_run { " (dry run)" } else { "" },
        model.name,
        usb.product_id,
        usb.link,
    );
    if let Some(why) = &report.refused {
        md.push_str(&format!("\n**Refused, nothing written:** {why}\n"));
    }
    if !report.entries.is_empty() {
        md.push_str(
            "\n## Values\n\n| key | dump | before | after | outcome |\n|---|---|---|---|---|\n",
        );
        let cell = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".into());
        for e in &report.entries {
            md.push_str(&format!(
                "| `{}` | {} | {} | {} | {} |\n",
                e.key,
                cell(&e.target),
                cell(&e.before),
                cell(&e.after),
                e.outcome
            ));
        }
        let frames: Vec<_> = report
            .entries
            .iter()
            .filter(|e| !e.traces.is_empty())
            .collect();
        if !frames.is_empty() {
            md.push_str("\n## Frames\n");
            for e in frames {
                md.push_str(&format!("\n`{}`\n\n```\n", e.key));
                for t in &e.traces {
                    md.push_str(&format!("> {}\n", hex(&t.request)));
                    for r in &t.responses {
                        md.push_str(&format!("< {}\n", hex(r)));
                    }
                }
                md.push_str("```\n");
            }
        }
    }
    if !report.not_restorable.is_empty() {
        md.push_str(&format!(
            "\n## Not restorable ({})\n\nNo verified write: {}\n",
            report.not_restorable.len(),
            report
                .not_restorable
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if report.refused.is_none() {
        let list =
            |md: &mut String, title: &str, diffs: &[&(String, Option<String>, Option<String>)]| {
                md.push_str(&format!("\n## {title} ({})\n\n", diffs.len()));
                for (k, a, b) in diffs {
                    md.push_str(&format!(
                        "- `{k}`: dump `{}`, device `{}`\n",
                        a.as_deref().unwrap_or("-"),
                        b.as_deref().unwrap_or("-")
                    ));
                }
            };
        list(
            &mut md,
            "Restorable values still differing from the dump",
            &report.unrestored(),
        );
        list(
            &mut md,
            "Other differences (outside the verified scope, not touched)",
            &report.outside_scope(),
        );
    }
    write_log(log, &md)?;

    let count = |f: fn(&Outcome) -> bool| report.entries.iter().filter(|e| f(&e.outcome)).count();
    let summary = format!(
        "restored {}, unchanged {}, would restore {}, skipped {}, failed {}, not attempted {}; \
         restorable values still differing {}; outside the verified scope: {} keys not restorable, {} differing",
        count(|o| matches!(o, Outcome::Restored { .. })),
        count(|o| matches!(o, Outcome::Unchanged)),
        count(|o| matches!(o, Outcome::WouldRestore)),
        count(|o| matches!(o, Outcome::Skipped(_))),
        count(|o| matches!(o, Outcome::Failed(_))),
        count(|o| matches!(o, Outcome::NotAttempted)),
        report.unrestored().len(),
        report.not_restorable.len(),
        report.outside_scope().len(),
    );
    match &report.refused {
        Some(why) => Err(format!("REFUSED: {why}")),
        None if dry_run => {
            eprintln!("DRY RUN, nothing written: {summary}");
            Ok(())
        }
        None if report.ok() => {
            let scope = if report.outside_scope().is_empty() {
                "verified values restored; nothing else differs"
            } else {
                "verified values restored; other differences remain (not restored)"
            };
            eprintln!("OK, {scope}: {summary}");
            Ok(())
        }
        None => Err(format!("FAILED: {summary}")),
    }
}

fn set(args: &[String]) -> Result<(), String> {
    use naga_pro::restore::Outcome;

    let (mut pid, mut items) = (None, Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--pid" => {
                let v = it.next().ok_or("--pid needs a value")?;
                pid = Some(u16::from_str_radix(v, 16).map_err(|_| format!("bad pid {v}"))?);
            }
            item => items.push(parse_item(item)?),
        }
    }
    if items.is_empty() {
        return Err(USAGE.to_string());
    }
    let (hid, model, usb) = select(pid)?;
    if *model != naga_pro::MODEL {
        return Err(format!("set is not implemented for {}", model.name));
    }
    let mut dev = Hidraw::open(&hid.node).map_err(|e| format!("{}: {e}", hid.node.display()))?;
    for (param, text) in items {
        let e = naga_pro::restore::set(&mut dev, usb, param, &text);
        println!(
            "{param}: {} -> {}: {}",
            e.before.as_deref().unwrap_or("-"),
            e.after.as_deref().unwrap_or("-"),
            e.outcome
        );
        if !matches!(e.outcome, Outcome::Restored { .. } | Outcome::Unchanged) {
            return Err(format!("stopped at {param}"));
        }
    }
    Ok(())
}

fn buttons(args: &[String]) -> Result<(), String> {
    use naga_pro::params::Param;

    let (path, rest) = args.split_first().ok_or(USAGE)?;
    let storage = match rest {
        [] => 1,
        [flag, n] if flag == "--storage" => n.parse().map_err(|_| format!("bad storage {n}"))?,
        _ => return Err(USAGE.to_string()),
    };
    let dump = load_baseline(path, &naga_pro::MODEL)?;
    for slot in naga_pro::BUTTON_SLOTS {
        let param = Param::button(storage, slot.id)?;
        let key = param.dump_key();
        let shown = match dump.get(&key).map(|v| (v, param.from_dump(v))) {
            Some((v, Ok(block))) => format!("{:<28} {v}", param.render(&block)),
            Some((v, Err(_))) => v.clone(),
            None => "-".into(),
        };
        println!(
            "{:>3}  {:<17} {:<16} {shown}",
            slot.id,
            format!("{:?}", slot.group),
            slot.name
        );
    }
    Ok(())
}

fn write_log(log: Option<String>, md: &str) -> Result<(), String> {
    match log {
        Some(path) => std::fs::write(&path, md).map_err(|e| format!("{path}: {e}")),
        None => {
            print!("{md}");
            Ok(())
        }
    }
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
            "\n### {}. {} [{}] — {}\n\n",
            i + 1,
            step.name,
            step.param,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_items_round_trip_through_the_state_file() {
        for item in [
            "dpi:1:800",
            "dpi-stages:1:active=2",
            "brightness:1:logo:100",
            "brightness:0:scroll_wheel:7",
            "effect:1:logo:ff00ff",
            "effect:0:scroll_wheel:spectrum",
            "button:1:80:key:ctrl+f5",
            "button:0:85:key:up turbo=55",
        ] {
            let (param, probe) = parse_item(item).unwrap();
            assert_eq!(format!("{}:{probe}", param_token(param)), item);
        }
        assert!(parse_item("dpi:9:800").is_err());
        assert!(parse_item("brightness:1:nose:100").is_err());
        assert!(parse_item("dpi").is_err());
        assert!(parse_item("button:1:6:key:a").is_err());
        assert!(parse_item("button:1:80").is_err());
    }
}
