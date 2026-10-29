//! Writable Naga Pro parameters (M2) and their first hardware validation.
//!
//! A validation run follows `docs/ARCHITECTURE.md`: it starts only from a
//! complete baseline dump that matches the device, writes the same value,
//! a probe value and the original value with a read-back after each write,
//! and ends with a full snapshot that must match the baseline again.

use std::collections::BTreeMap;
use std::fmt;

use chimerazer_protocol::ARGS_LEN;
use chimerazer_transport::FeatureIo;

use super::state::{compare, read_snapshot};
use super::*;
use crate::{
    CommandSpec, ExchangeError, Trace, UsbId, WriteMode, exchange, read_request, write_request,
};

/// One writable value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Param {
    PollRate,
    Dpi { storage: u8 },
    DpiStages { storage: u8 },
    Brightness { storage: u8, led: u8 },
}

impl fmt::Display for Param {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PollRate => write!(f, "poll-rate"),
            Self::Dpi { storage } => write!(f, "dpi (storage {storage})"),
            Self::DpiStages { storage } => write!(f, "dpi-stages (storage {storage})"),
            Self::Brightness { storage, led } => {
                write!(f, "brightness (storage {storage}, led {led:#04x})")
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

impl Param {
    /// Parses `poll-rate`, `dpi`, `dpi-stages`, `brightness` with the
    /// storage and LED they apply to.
    pub fn parse(name: &str, storage: u8, led: Option<u8>) -> Result<Self, String> {
        if storage != LIVE_STORAGE && !PROFILES.contains(&storage) {
            return Err(format!("storage {storage} is not 0..=5"));
        }
        match name {
            "poll-rate" => Ok(Self::PollRate),
            "dpi" => Ok(Self::Dpi { storage }),
            "dpi-stages" => Ok(Self::DpiStages { storage }),
            "brightness" => {
                let led = led.ok_or("brightness needs --led scroll_wheel|logo")?;
                if led != LED_SCROLL_WHEEL && led != LED_LOGO {
                    return Err(format!("unknown LED {led:#04x}"));
                }
                Ok(Self::Brightness { storage, led })
            }
            other => Err(format!(
                "unknown parameter {other} (poll-rate, dpi, dpi-stages, brightness)"
            )),
        }
    }

    pub fn read_spec(&self) -> &'static CommandSpec {
        match self {
            Self::PollRate => &POLL_RATE,
            Self::Dpi { .. } => &DPI,
            Self::DpiStages { .. } => &DPI_STAGES,
            Self::Brightness { .. } => &LED_BRIGHTNESS,
        }
    }

    pub fn write_spec(&self) -> &'static CommandSpec {
        match self {
            Self::PollRate => &SET_POLL_RATE,
            Self::Dpi { .. } => &SET_DPI,
            Self::DpiStages { .. } => &SET_DPI_STAGES,
            Self::Brightness { .. } => &SET_LED_BRIGHTNESS,
        }
    }

    fn read_args(&self) -> Vec<u8> {
        match *self {
            Self::PollRate => vec![],
            Self::Dpi { storage } | Self::DpiStages { storage } => vec![storage],
            Self::Brightness { storage, led } => vec![storage, led],
        }
    }

    /// The value bytes inside a read response, after checking its echo.
    pub fn extract(&self, args: &[u8; ARGS_LEN]) -> Result<Vec<u8>, String> {
        let echo = |i: usize, want: u8| {
            if args[i] == want {
                Ok(())
            } else {
                Err(format!("echo {:#04x}, expected {want:#04x}", args[i]))
            }
        };
        match *self {
            Self::PollRate => Ok(vec![args[0]]),
            Self::Dpi { storage } => {
                echo(0, storage)?;
                Ok(args[1..5].to_vec())
            }
            Self::DpiStages { storage } => {
                echo(0, storage)?;
                let count = args[2];
                if count == 0 || count > 5 {
                    return Err(format!("stage count {count}"));
                }
                Ok(args[1..3 + usize::from(count) * 7].to_vec())
            }
            Self::Brightness { led, .. } => {
                echo(1, led)?;
                Ok(vec![args[2]])
            }
        }
    }

    /// Write arguments that set `value` (as returned by [`Param::extract`]).
    pub fn write_args(&self, value: &[u8]) -> Vec<u8> {
        match *self {
            Self::PollRate => value.to_vec(),
            Self::Dpi { storage } => [&[storage][..], value, &[0, 0]].concat(),
            Self::DpiStages { storage } => [&[storage][..], value].concat(),
            Self::Brightness { storage, led } => [&[storage, led][..], value].concat(),
        }
    }

    /// Human form of a value.
    pub fn render(&self, value: &[u8]) -> String {
        match self {
            Self::PollRate => match value[0] {
                0x01 => "1000 Hz".into(),
                0x02 => "500 Hz".into(),
                0x08 => "125 Hz".into(),
                r => format!("raw {r:02x}"),
            },
            Self::Dpi { .. } => format!(
                "{}x{}",
                u16::from_be_bytes([value[0], value[1]]),
                u16::from_be_bytes([value[2], value[3]])
            ),
            Self::DpiStages { .. } => format!("active={} [{}]", value[0], hex(value)),
            Self::Brightness { .. } => value[0].to_string(),
        }
    }

    /// Builds a probe value from text, relative to the current `original`.
    /// Probes stay inside documented ranges and must differ from `original`.
    pub fn probe(&self, text: &str, original: &[u8]) -> Result<Vec<u8>, String> {
        let value = match self {
            Self::PollRate => match text {
                "1000" => vec![0x01],
                "500" => vec![0x02],
                "125" => vec![0x08],
                _ => return Err("poll-rate probe: 125, 500 or 1000".into()),
            },
            Self::Dpi { .. } => {
                let (x, y) = text.split_once('x').unwrap_or((text, text));
                let parse = |s: &str| -> Result<u16, String> {
                    let v: u16 = s.parse().map_err(|_| format!("bad DPI {s}"))?;
                    if !(100..=MAX_DPI).contains(&v) {
                        return Err(format!("DPI {v} outside 100..={MAX_DPI}"));
                    }
                    Ok(v)
                };
                let (x, y) = (parse(x)?, parse(y)?);
                [x.to_be_bytes(), y.to_be_bytes()].concat()
            }
            Self::DpiStages { .. } => {
                let token: u8 = text
                    .strip_prefix("active=")
                    .and_then(|t| t.parse().ok())
                    .ok_or("dpi-stages probe: active=<stage id>")?;
                let count = usize::from(original[1]);
                let ids: Vec<u8> = (0..count).map(|i| original[2 + i * 7]).collect();
                if !ids.contains(&token) {
                    return Err(format!("stage id {token} not in table {ids:?}"));
                }
                let mut v = original.to_vec();
                v[0] = token;
                v
            }
            Self::Brightness { .. } => {
                vec![text.parse().map_err(|_| format!("bad brightness {text}"))?]
            }
        };
        if value == original {
            return Err("probe equals the current value; choose a different one".into());
        }
        Ok(value)
    }
}

/// One step of a validation run.
#[derive(Debug)]
pub struct Step {
    pub name: &'static str,
    pub expected: Option<String>,
    pub got: Option<String>,
    pub ok: bool,
    pub note: String,
    pub traces: Vec<Trace>,
}

/// Outcome of a validation run, with every raw frame.
#[derive(Debug)]
pub struct Report {
    pub param: Param,
    pub steps: Vec<Step>,
    pub passed: bool,
}

/// A baseline dump may only be used if every value was read.
pub fn baseline_errors(baseline: &BTreeMap<String, String>) -> Vec<String> {
    baseline
        .iter()
        .filter(|(_, v)| v.starts_with("error:"))
        .map(|(k, v)| format!("{k} = {v}"))
        .collect()
}

struct Runner<'a, IO> {
    io: &'a mut IO,
    usb: &'a UsbId,
    param: Param,
    mode: WriteMode,
    steps: Vec<Step>,
}

impl<IO: FeatureIo> Runner<'_, IO> {
    fn read(&mut self, traces: &mut Vec<Trace>) -> Result<Vec<u8>, String> {
        let spec = self.param.read_spec();
        let request =
            read_request(self.usb, spec, &self.param.read_args()).map_err(|e| e.to_string())?;
        let (trace, result) = exchange(self.io, self.usb, &request);
        traces.push(trace);
        let response = result.map_err(|e| e.to_string())?;
        self.param.extract(&response.args)
    }

    fn write(&mut self, value: &[u8], traces: &mut Vec<Trace>) -> Result<(), String> {
        let request = write_request(
            self.usb,
            self.param.write_spec(),
            self.param.read_spec(),
            &self.param.write_args(value),
            self.mode,
        )
        .map_err(|e| e.to_string())?;
        let (trace, result) = exchange(self.io, self.usb, &request);
        traces.push(trace);
        match result {
            Ok(_) => Ok(()),
            Err(ExchangeError::Device(s)) => Err(format!("device returned {s:?}")),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Writes `value`, reads it back and records the step.
    fn write_step(&mut self, name: &'static str, value: &[u8]) -> bool {
        let mut traces = Vec::new();
        let expected = self.param.render(value);
        let written = self.write(value, &mut traces);
        let read = self.read(&mut traces);
        let (ok, got, note) = match (&written, &read) {
            (Ok(()), Ok(v)) if v == value => (true, Some(self.param.render(v)), String::new()),
            (Ok(()), Ok(v)) => (
                false,
                Some(self.param.render(v)),
                "read-back differs".into(),
            ),
            (Err(w), Ok(v)) => (
                false,
                Some(self.param.render(v)),
                format!("write failed: {w}"),
            ),
            (_, Err(r)) => (false, None, format!("read-back failed: {r}")),
        };
        self.steps.push(Step {
            name,
            expected: Some(expected),
            got,
            ok,
            note,
            traces,
        });
        ok
    }

    fn note(&mut self, name: &'static str, ok: bool, note: String, traces: Vec<Trace>) {
        self.steps.push(Step {
            name,
            expected: None,
            got: None,
            ok,
            note,
            traces,
        });
    }
}

/// Runs steps 1–4 of the first hardware validation for `param`.
///
/// `baseline` is a parsed dump of this device taken before the run; it must
/// contain no read errors and must match the device now. `probe` is the
/// text form accepted by [`Param::probe`].
pub fn validate(
    io: &mut impl FeatureIo,
    usb: &UsbId,
    param: Param,
    probe: &str,
    baseline: &BTreeMap<String, String>,
    mode: WriteMode,
) -> Report {
    let mut r = Runner {
        io,
        usb,
        param,
        mode,
        steps: Vec::new(),
    };
    let passed = run(&mut r, probe, baseline);
    Report {
        param,
        steps: r.steps,
        passed,
    }
}

fn run<IO: FeatureIo>(
    r: &mut Runner<'_, IO>,
    probe: &str,
    baseline: &BTreeMap<String, String>,
) -> bool {
    // Step 1: a complete baseline that matches the device.
    let errors = baseline_errors(baseline);
    if !errors.is_empty() {
        r.note(
            "baseline",
            false,
            format!(
                "baseline has read errors, refusing to write: {}",
                errors.join("; ")
            ),
            vec![],
        );
        return false;
    }
    let before = read_snapshot(r.io, r.usb);
    let errors = baseline_errors(&before.values);
    let diffs = compare(baseline, &before.values);
    if !errors.is_empty() || !diffs.is_empty() {
        let mut note = String::from("device does not match the baseline, refusing to write:");
        for e in &errors {
            note.push_str(&format!(" {e};"));
        }
        for (k, a, b) in &diffs {
            note.push_str(&format!(" {k}: {a:?} -> {b:?};"));
        }
        r.note("baseline", false, note, before.traces);
        return false;
    }
    r.note(
        "baseline",
        true,
        format!("{} values match the baseline", before.values.len()),
        vec![],
    );

    let mut traces = Vec::new();
    let original = match r.read(&mut traces) {
        Ok(v) => v,
        Err(e) => {
            r.note("read original", false, e, traces);
            return false;
        }
    };
    let probe = match r.param.probe(probe, &original) {
        Ok(p) => p,
        Err(e) => {
            r.note("read original", false, format!("bad probe: {e}"), traces);
            return false;
        }
    };
    let rendered = r.param.render(&original);
    r.note("read original", true, rendered, traces);

    // Steps 2–4. On any failure, restore and stop.
    let ok = r.write_step("write same value", &original)
        && r.write_step("write probe", &probe)
        && r.write_step("restore original", &original);
    if !ok {
        if r.steps.last().map(|s| s.name) != Some("restore original") {
            r.write_step("restore original", &original);
        }
        return false;
    }

    // Nothing else may have changed.
    let after = read_snapshot(r.io, r.usb);
    let diffs = compare(baseline, &after.values);
    let errors = baseline_errors(&after.values);
    let clean = diffs.is_empty() && errors.is_empty();
    let note = if clean {
        format!("{} values match the baseline", after.values.len())
    } else {
        let mut n = String::from("state differs from baseline after the run:");
        for (k, a, b) in &diffs {
            n.push_str(&format!(" {k}: {a:?} -> {b:?};"));
        }
        for e in &errors {
            n.push_str(&format!(" {e};"));
        }
        n
    };
    r.note("final snapshot", clean, note, vec![]);
    clean
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io;
    use std::time::Duration;

    use chimerazer_protocol::{REPORT_LEN, Status, crc};

    use super::*;
    use crate::Link;

    const USB: UsbId = UsbId {
        product_id: 0x008F,
        link: Link::Wired,
        interface: 0,
        transaction_id: 0x1F,
        response_delay: Duration::ZERO,
    };

    /// A simulated Naga Pro holding DPI, stages, brightness and poll rate.
    /// Every other read answers with a fixed zero-filled OK frame.
    #[derive(Default)]
    struct Sim {
        dpi: HashMap<u8, [u8; 4]>,
        stages: HashMap<u8, Vec<u8>>,
        brightness: HashMap<(u8, u8), u8>,
        poll: u8,
        ignore_writes: bool,
        /// A write to DPI also changes brightness on storage 0, LED 1.
        side_effect: bool,
        reply: Option<[u8; REPORT_LEN]>,
    }

    impl Sim {
        fn new() -> Self {
            let mut s = Self {
                poll: 0x01,
                ..Self::default()
            };
            for st in 0..=5 {
                s.dpi.insert(st, [0x06, 0x40, 0x06, 0x40]);
                let mut t = vec![0x03, 0x05];
                for (id, dpi) in [(1u8, 400u16), (2, 800), (3, 1600), (4, 3200), (5, 6400)] {
                    t.push(id);
                    t.extend(dpi.to_be_bytes());
                    t.extend(dpi.to_be_bytes());
                    t.extend([0, 0]);
                }
                s.stages.insert(st, t);
                s.brightness.insert((st, 0x01), 51);
                s.brightness.insert((st, 0x04), 51);
            }
            s
        }
    }

    impl FeatureIo for Sim {
        fn send_feature(&mut self, report: &[u8]) -> io::Result<()> {
            let mut out: [u8; REPORT_LEN] = report.try_into().unwrap();
            let (class, id, a) = (report[6], report[7], &report[8..88]);
            let mut args = [0u8; ARGS_LEN];
            args.copy_from_slice(a);
            let write = id & 0x80 == 0;
            if write && !self.ignore_writes {
                match (class, id) {
                    (0x00, 0x05) => self.poll = a[0],
                    (0x04, 0x05) => {
                        self.dpi.insert(a[0], a[1..5].try_into().unwrap());
                        if self.side_effect {
                            self.brightness.insert((0, 0x01), 1);
                        }
                    }
                    (0x04, 0x06) => {
                        let n = 2 + usize::from(a[2]) * 7;
                        self.stages.insert(a[0], a[1..1 + n].to_vec());
                    }
                    (0x0F, 0x04) => {
                        self.brightness.insert((a[0], a[1]), a[2]);
                    }
                    _ => {}
                }
            }
            if !write {
                match (class, id) {
                    (0x00, 0x85) => args[0] = self.poll,
                    (0x04, 0x85) => args[1..5].copy_from_slice(&self.dpi[&a[0]]),
                    (0x04, 0x86) => {
                        let t = &self.stages[&a[0]];
                        args[1..1 + t.len()].copy_from_slice(t);
                    }
                    (0x0F, 0x84) => args[2] = self.brightness[&(a[0], a[1])],
                    (0x02, 0x84) => {
                        args[0] = 30;
                        for (i, s) in BUTTON_SLOTS.iter().enumerate() {
                            args[1 + i] = s.id;
                        }
                    }
                    (0x05, 0x81) => args[..5].copy_from_slice(&[5, 1, 2, 3, 4]),
                    (0x00, 0x81) => args[..2].copy_from_slice(&[2, 3]),
                    _ => {}
                }
            }
            out[0] = Status::Ok.to_byte();
            out[8..88].copy_from_slice(&args);
            out[88] = crc(&out);
            self.reply = Some(out);
            Ok(())
        }

        fn get_feature(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            buf[..REPORT_LEN].copy_from_slice(&self.reply.unwrap());
            Ok(REPORT_LEN)
        }
    }

    const FIRST: WriteMode = WriteMode::FirstValidation {
        allow_source_only: false,
    };

    fn baseline(sim: &mut Sim) -> BTreeMap<String, String> {
        read_snapshot(sim, &USB).values
    }

    #[test]
    fn well_behaved_device_passes_and_ends_unchanged() {
        let mut sim = Sim::new();
        let base = baseline(&mut sim);
        for (param, probe) in [
            (Param::Dpi { storage: 0 }, "800x800"),
            (Param::DpiStages { storage: 5 }, "active=2"),
            (
                Param::Brightness {
                    storage: 0,
                    led: LED_LOGO,
                },
                "100",
            ),
        ] {
            let report = validate(&mut sim, &USB, param, probe, &base, FIRST);
            assert!(report.passed, "{param}: {:#?}", report.steps);
            let names: Vec<&str> = report.steps.iter().map(|s| s.name).collect();
            assert_eq!(
                names,
                [
                    "baseline",
                    "read original",
                    "write same value",
                    "write probe",
                    "restore original",
                    "final snapshot"
                ]
            );
        }
        assert_eq!(compare(&base, &baseline(&mut sim)), vec![]);
    }

    #[test]
    fn baseline_with_read_errors_blocks_writes() {
        let mut sim = Sim::new();
        let mut base = baseline(&mut sim);
        base.insert("p5.dpi".into(), "error: Failed".into());
        let report = validate(
            &mut sim,
            &USB,
            Param::Dpi { storage: 0 },
            "800",
            &base,
            FIRST,
        );
        assert!(!report.passed);
        assert_eq!(report.steps.len(), 1);
        assert!(report.steps[0].note.contains("read errors"));
    }

    #[test]
    fn stale_baseline_blocks_writes() {
        let mut sim = Sim::new();
        let base = baseline(&mut sim);
        sim.dpi.insert(0, [0x03, 0x20, 0x03, 0x20]);
        let report = validate(
            &mut sim,
            &USB,
            Param::Dpi { storage: 0 },
            "400",
            &base,
            FIRST,
        );
        assert!(!report.passed);
        assert!(report.steps[0].note.contains("live.dpi"));
        assert!(
            report.steps[0]
                .traces
                .iter()
                .all(|t| t.request[7] & 0x80 != 0)
        );
    }

    #[test]
    fn ignored_write_fails_at_probe_and_restores() {
        let mut sim = Sim::new();
        let base = baseline(&mut sim);
        sim.ignore_writes = true;
        let report = validate(
            &mut sim,
            &USB,
            Param::Dpi { storage: 0 },
            "800",
            &base,
            FIRST,
        );
        assert!(!report.passed);
        let names: Vec<&str> = report.steps.iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            [
                "baseline",
                "read original",
                "write same value",
                "write probe",
                "restore original"
            ]
        );
    }

    #[test]
    fn side_effect_fails_final_snapshot() {
        let mut sim = Sim::new();
        let base = baseline(&mut sim);
        sim.side_effect = true;
        let report = validate(
            &mut sim,
            &USB,
            Param::Dpi { storage: 0 },
            "800",
            &base,
            FIRST,
        );
        assert!(!report.passed);
        let last = report.steps.last().unwrap();
        assert_eq!(last.name, "final snapshot");
        assert!(last.note.contains("live.led.scroll_wheel.brightness"));
    }

    #[test]
    fn source_only_write_needs_owner_permission() {
        let mut sim = Sim::new();
        let base = baseline(&mut sim);
        let report = validate(&mut sim, &USB, Param::PollRate, "500", &base, FIRST);
        assert!(!report.passed);
        assert!(report.steps.iter().any(|s| s.note.contains("SourceOnly")));
        assert_eq!(sim.poll, 0x01);
    }

    #[test]
    fn probes_are_checked() {
        let dpi = Param::Dpi { storage: 0 };
        assert!(dpi.probe("50", &[0x06, 0x40, 0x06, 0x40]).is_err());
        assert!(dpi.probe("1600", &[0x06, 0x40, 0x06, 0x40]).is_err());
        assert_eq!(
            dpi.probe("800x400", &[0x06, 0x40, 0x06, 0x40]).unwrap(),
            [0x03, 0x20, 0x01, 0x90]
        );
        let stages = Param::DpiStages { storage: 0 };
        let table = [3, 2, 1, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0];
        assert!(stages.probe("active=2", &table).is_err());
        assert_eq!(stages.probe("active=1", &table).unwrap()[0], 1);
    }
}
