//! Normal-mode writes: one value ([`set`]) or the values of a dump
//! ([`restore`]).
//!
//! Only values whose write is `OurHardware` and inside [`VERIFIED_WRITES`]
//! for the current link are written. Values are applied one at a time; the
//! device offers no transaction, so a failure leaves earlier writes in
//! place. The run stops at the first failed value, and the report lists
//! what was restored, unchanged, failed, not attempted and skipped.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use chimerazer_transport::FeatureIo;

use super::params::Param;
use super::state::{VOLATILE_PREFIX, compare, read_snapshot};
use super::*;
use crate::{ExchangeError, Trace, UsbId, WriteMode, exchange, read_request, write_request};

/// Values restore can write, profile storage first: the live layer of the
/// active profile follows the profile, so it is usually unchanged by then.
pub fn candidates() -> Vec<Param> {
    let mut out = Vec::new();
    for storage in [1, LIVE_STORAGE] {
        out.push(Param::Dpi { storage });
        out.push(Param::DpiStages { storage });
        out.push(Param::Brightness {
            storage,
            led: LED_LOGO,
        });
        out.push(Param::Brightness {
            storage,
            led: LED_SCROLL_WHEEL,
        });
        for led in [LED_LOGO, LED_SCROLL_WHEEL] {
            out.push(Param::Effect { storage, led });
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Written and confirmed by a read-back. `note` records anomalies such
    /// as an error status on an applied write.
    Restored {
        note: String,
    },
    /// Already equal to the dump; nothing written.
    Unchanged,
    /// Differs from the dump; a dry run wrote nothing.
    WouldRestore,
    Failed(String),
    /// Not tried because an earlier value failed.
    NotAttempted,
    /// Not written: not verified for this link/storage, or not in the dump.
    Skipped(String),
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Restored { note } if note.is_empty() => write!(f, "restored"),
            Self::Restored { note } => write!(f, "restored ({note})"),
            Self::Unchanged => write!(f, "unchanged"),
            Self::WouldRestore => write!(f, "would restore (dry run)"),
            Self::Failed(e) => write!(f, "FAILED: {e}"),
            Self::NotAttempted => write!(f, "not attempted"),
            Self::Skipped(why) => write!(f, "skipped: {why}"),
        }
    }
}

#[derive(Debug)]
pub struct Entry {
    pub param: Param,
    pub key: String,
    pub target: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub outcome: Outcome,
    pub traces: Vec<Trace>,
}

#[derive(Debug, Default)]
pub struct RestoreReport {
    /// Why nothing was written, if the dump or the device was refused.
    pub refused: Option<String>,
    pub entries: Vec<Entry>,
    /// Dump keys that restore does not cover (not writable or not verified).
    pub not_restorable: Vec<String>,
    /// Differences from the dump after the run (volatile keys excluded).
    pub remaining: Vec<Difference>,
    /// Dump keys restore may write on this link.
    pub verified_keys: BTreeSet<String>,
}

type Difference = (String, Option<String>, Option<String>);

impl RestoreReport {
    /// Remaining differences in values restore may write on this link.
    pub fn unrestored(&self) -> Vec<&Difference> {
        self.remaining
            .iter()
            .filter(|(k, _, _)| self.verified_keys.contains(k))
            .collect()
    }

    /// Remaining differences outside what restore may write.
    pub fn outside_scope(&self) -> Vec<&Difference> {
        self.remaining
            .iter()
            .filter(|(k, _, _)| !self.verified_keys.contains(k))
            .collect()
    }

    /// Nothing failed and every value restore may write on this link now
    /// matches the dump. Says nothing about values outside that scope.
    pub fn ok(&self) -> bool {
        self.refused.is_none()
            && self
                .entries
                .iter()
                .all(|e| !matches!(e.outcome, Outcome::Failed(_) | Outcome::NotAttempted))
            && self.unrestored().is_empty()
    }
}

fn read_value(
    io: &mut impl FeatureIo,
    usb: &UsbId,
    param: Param,
    traces: &mut Vec<Trace>,
) -> Result<Vec<u8>, String> {
    let request =
        read_request(usb, param.read_spec(), &param.read_args()).map_err(|e| e.to_string())?;
    let (trace, result) = exchange(io, usb, &request);
    traces.push(trace);
    let response = result.map_err(|e| e.to_string())?;
    param.extract(&response.args)
}

/// Writes `value` (already checked with [`Param::verified_for`]) in normal
/// mode and reads it back; the read-back decides `entry.outcome`.
fn write_verified(
    io: &mut impl FeatureIo,
    usb: &UsbId,
    param: Param,
    value: &[u8],
    entry: &mut Entry,
) {
    let mode = WriteMode::Normal {
        scopes: VERIFIED_WRITES,
    };
    let Some(spec) = param.write_spec_for(value) else {
        entry.outcome = Outcome::Failed("value cannot be written, nothing sent".into());
        return;
    };
    let written = match write_request(usb, spec, param.read_spec(), &param.write_args(value), mode)
    {
        Ok(request) => {
            let (trace, result) = exchange(io, usb, &request);
            entry.traces.push(trace);
            result.map(|_| ()).map_err(|e| match e {
                ExchangeError::Device(s) => format!("device reported {s:?}"),
                other => other.to_string(),
            })
        }
        Err(e) => {
            entry.outcome = Outcome::Failed(format!("write refused, nothing sent: {e}"));
            return;
        }
    };
    match read_value(io, usb, param, &mut entry.traces) {
        Ok(after) => {
            entry.after = Some(param.render(&after));
            entry.outcome = match (after == value, written) {
                (true, Ok(())) => Outcome::Restored {
                    note: String::new(),
                },
                (true, Err(w)) => Outcome::Restored {
                    note: format!("{w}, read-back shows the value applied"),
                },
                (false, Ok(())) => Outcome::Failed("read-back differs".into()),
                (false, Err(w)) => Outcome::Failed(format!("{w}, read-back differs")),
            };
        }
        Err(e) => entry.outcome = Outcome::Failed(format!("read-back: {e}")),
    }
}

/// Reads `param`, builds the new value from `text` ([`Param::value`]) and,
/// if it differs and its write is verified on this link, writes it and
/// reads it back. `Restored` in the result means written and confirmed.
pub fn set(io: &mut impl FeatureIo, usb: &UsbId, param: Param, text: &str) -> Entry {
    let mut entry = Entry {
        param,
        key: param.dump_key(),
        target: None,
        before: None,
        after: None,
        outcome: Outcome::NotAttempted,
        traces: Vec::new(),
    };
    let before = match read_value(io, usb, param, &mut entry.traces) {
        Ok(v) => v,
        Err(e) => {
            entry.outcome = Outcome::Failed(format!("read: {e}"));
            return entry;
        }
    };
    entry.before = Some(param.render(&before));
    let value = match param.value(text, &before) {
        Ok(v) => v,
        Err(e) => {
            entry.outcome = Outcome::Skipped(e);
            return entry;
        }
    };
    entry.target = Some(param.render(&value));
    if value == before {
        entry.after = entry.before.clone();
        entry.outcome = Outcome::Unchanged;
    } else if !param.verified_for(usb.link, &value) {
        entry.outcome = Outcome::Skipped(format!(
            "writing {} is not verified for {:?}",
            param.render(&value),
            usb.link
        ));
    } else {
        write_verified(io, usb, param, &value, &mut entry);
    }
    entry
}

/// Restores verified values of `dump` (a parsed `chimerazer dump` of this
/// model) on the device. With `dry_run` nothing is written: the report
/// shows what would be restored.
pub fn restore(
    io: &mut impl FeatureIo,
    usb: &UsbId,
    dump: &BTreeMap<String, String>,
    dry_run: bool,
) -> RestoreReport {
    let mut report = RestoreReport::default();
    let refuse = |mut r: RestoreReport, why: String| {
        r.refused = Some(why);
        r
    };

    // The dump must be complete, and the device must be in the same
    // onboard-profile configuration: selecting a profile is not verified.
    let errors: Vec<String> = dump
        .iter()
        .filter(|(_, v)| v.starts_with("error:"))
        .map(|(k, v)| format!("{k} = {v}"))
        .collect();
    if !errors.is_empty() {
        return refuse(
            report,
            format!("dump has read errors: {}", errors.join("; ")),
        );
    }
    let now = read_snapshot(io, usb);
    let device_errors: Vec<String> = now
        .values
        .iter()
        .filter(|(_, v)| v.starts_with("error:"))
        .map(|(k, v)| format!("{k} = {v}"))
        .collect();
    if !device_errors.is_empty() {
        return refuse(
            report,
            format!("device read errors: {}", device_errors.join("; ")),
        );
    }
    for key in ["profiles.active", "profiles.assigned"] {
        if dump.get(key) != now.values.get(key) {
            return refuse(
                report,
                format!(
                    "{key} differs (dump {:?}, device {:?}); changing profiles is not verified",
                    dump.get(key),
                    now.values.get(key)
                ),
            );
        }
    }

    let mut covered = BTreeSet::new();
    let mut failed = false;
    for param in candidates() {
        let key = param.dump_key();
        covered.extend(param.covered_keys());
        if param.verified_on(usb.link) {
            report.verified_keys.extend(param.covered_keys());
        }
        let mut entry = Entry {
            param,
            key: key.clone(),
            target: dump.get(&key).cloned(),
            before: None,
            after: None,
            outcome: Outcome::NotAttempted,
            traces: Vec::new(),
        };
        if !param.verified_on(usb.link) {
            entry.outcome = Outcome::Skipped(format!("write not verified for {:?}", usb.link));
            report.entries.push(entry);
            continue;
        }
        let target = match dump.get(&key).map(|t| param.from_dump(t)) {
            Some(Ok(t)) => t,
            Some(Err(e)) => {
                entry.outcome = Outcome::Skipped(format!("unreadable dump value: {e}"));
                report.entries.push(entry);
                continue;
            }
            None => {
                entry.outcome = Outcome::Skipped("not in dump".into());
                report.entries.push(entry);
                continue;
            }
        };
        if failed {
            report.entries.push(entry);
            continue;
        }

        let before = match read_value(io, usb, param, &mut entry.traces) {
            Ok(v) => v,
            Err(e) => {
                entry.outcome = Outcome::Failed(format!("read before write: {e}"));
                failed = true;
                report.entries.push(entry);
                continue;
            }
        };
        entry.before = Some(param.render(&before));
        if before == target {
            entry.after = entry.before.clone();
            entry.outcome = Outcome::Unchanged;
            report.entries.push(entry);
            continue;
        }
        // Some writes of a parameter are verified and others are not (the
        // `0F:02` layouts): the value itself must be writable. A skipped
        // value stays in `verified_keys`, so its difference fails the run.
        if !param.verified_for(usb.link, &target) {
            entry.outcome = Outcome::Skipped(format!(
                "writing {} is not verified for {:?}",
                param.render(&target),
                usb.link
            ));
            report.entries.push(entry);
            continue;
        }
        if dry_run {
            entry.outcome = Outcome::WouldRestore;
            report.entries.push(entry);
            continue;
        }

        write_verified(io, usb, param, &target, &mut entry);
        failed |= matches!(entry.outcome, Outcome::Failed(_));
        report.entries.push(entry);
    }

    report.not_restorable = dump
        .keys()
        .filter(|k| !k.starts_with(VOLATILE_PREFIX) && !covered.contains(*k))
        .cloned()
        .collect();
    let after = read_snapshot(io, usb);
    report.remaining = compare(dump, &after.values);
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Link;
    use crate::naga_pro::sim::{Sim, USB as WIRED};
    const RECEIVER: UsbId = UsbId {
        product_id: 0x0090,
        link: Link::Receiver,
        ..WIRED
    };

    fn outcome<'a>(r: &'a RestoreReport, key: &str) -> &'a Outcome {
        &r.entries.iter().find(|e| e.key == key).unwrap().outcome
    }

    #[test]
    fn restores_changed_profile_values_and_reports_the_rest() {
        let mut sim = Sim::new();
        let dump = read_snapshot(&mut sim, &WIRED).values;
        sim.dpi.insert(1, [0x03, 0x20, 0x03, 0x20]);
        sim.dpi.insert(0, [0x03, 0x20, 0x03, 0x20]);
        sim.brightness.insert((1, LED_LOGO), 100);

        let r = restore(&mut sim, &WIRED, &dump, false);
        assert!(r.ok(), "{r:#?}");
        assert!(matches!(outcome(&r, "p1.dpi"), Outcome::Restored { .. }));
        assert!(matches!(outcome(&r, "live.dpi"), Outcome::Restored { .. }));
        assert!(matches!(
            outcome(&r, "p1.led.logo.brightness"),
            Outcome::Restored { .. }
        ));
        assert_eq!(*outcome(&r, "p1.dpi_stages.raw"), Outcome::Unchanged);
        assert!(r.remaining.is_empty(), "{:?}", r.remaining);
        // Buttons, other profiles and device settings are reported, not touched.
        assert!(r.not_restorable.contains(&"p2.dpi".to_string()));
        assert!(!r.not_restorable.iter().any(|k| k.starts_with("volatile.")));
        assert!(!r.not_restorable.contains(&"p1.dpi".to_string()));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let mut sim = Sim::new();
        let dump = read_snapshot(&mut sim, &WIRED).values;
        sim.dpi.insert(1, [0x03, 0x20, 0x03, 0x20]);
        let r = restore(&mut sim, &WIRED, &dump, true);
        assert_eq!(*outcome(&r, "p1.dpi"), Outcome::WouldRestore);
        assert_eq!(*outcome(&r, "live.dpi"), Outcome::Unchanged);
        assert_eq!(sim.dpi[&1], [0x03, 0x20, 0x03, 0x20]);
        assert_eq!(r.remaining.len(), 1);
        // A verified value still differs: not a successful restore.
        assert!(!r.ok());
        assert_eq!(r.unrestored().len(), 1);
    }

    #[test]
    fn unverified_effect_restore_is_reported_and_fails() {
        // The dump has spectrum everywhere; profile 1 now holds a static
        // colour. Spectrum into a profile is not verified.
        let mut sim = Sim::new();
        let dump = read_snapshot(&mut sim, &WIRED).values;
        let fuchsia = [1, 0, 0, 1, 0xff, 0, 0xff, 0, 0, 0];
        sim.effect.insert((1, LED_LOGO), fuchsia);
        sim.effect.insert((0, LED_LOGO), fuchsia);
        let r = restore(&mut sim, &WIRED, &dump, false);
        match outcome(&r, "p1.led.logo.effect") {
            Outcome::Skipped(why) => assert!(why.contains("not verified"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(sim.effect[&(1, LED_LOGO)], fuchsia);
        assert!(matches!(
            outcome(&r, "live.led.logo.effect"),
            Outcome::Restored { .. }
        ));
        assert!(!r.ok(), "{r:#?}");
        assert!(
            r.unrestored()
                .iter()
                .any(|(k, _, _)| k == "p1.led.logo.effect")
        );
    }

    #[test]
    fn static_colour_is_restored_into_profile_1() {
        let mut sim = Sim::new();
        let fuchsia = [1, 0, 0, 1, 0xff, 0, 0xff, 0, 0, 0];
        sim.effect.insert((1, LED_LOGO), fuchsia);
        let dump = read_snapshot(&mut sim, &WIRED).values;
        sim.effect
            .insert((1, LED_LOGO), [3, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let r = restore(&mut sim, &WIRED, &dump, false);
        assert!(r.ok(), "{r:#?}");
        assert_eq!(sim.effect[&(1, LED_LOGO)], fuchsia);
        // Sent with the static layout (size 09).
        let e = r
            .entries
            .iter()
            .find(|e| e.key == "p1.led.logo.effect")
            .unwrap();
        assert_eq!(e.traces[1].request[5], 0x09);
    }

    #[test]
    fn set_writes_only_verified_values() {
        let mut sim = Sim::new();
        let p1_logo = Param::Effect {
            storage: 1,
            led: LED_LOGO,
        };
        let e = set(&mut sim, &WIRED, p1_logo, "ff00ff");
        assert!(matches!(e.outcome, Outcome::Restored { .. }), "{e:?}");
        assert_eq!(e.after.as_deref(), Some("static #ff00ff"));

        let e = set(&mut sim, &WIRED, p1_logo, "ff00ff");
        assert_eq!(e.outcome, Outcome::Unchanged);
        assert_eq!(e.traces.len(), 1);

        // Spectrum into profile 1 is not verified: nothing is sent.
        let e = set(&mut sim, &WIRED, p1_logo, "spectrum");
        assert!(matches!(e.outcome, Outcome::Skipped(_)), "{e:?}");
        assert_eq!(e.traces.len(), 1);
        assert_eq!(sim.effect[&(1, LED_LOGO)][0], 1);

        // Through the receiver only profile values and live DPI.
        let e = set(&mut sim, &RECEIVER, Param::Dpi { storage: 1 }, "800");
        assert!(matches!(e.outcome, Outcome::Restored { .. }), "{e:?}");
        let e = set(&mut sim, &RECEIVER, p1_logo, "00ff00");
        assert!(matches!(e.outcome, Outcome::Skipped(_)), "{e:?}");

        let e = set(&mut sim, &WIRED, Param::Dpi { storage: 1 }, "50");
        assert!(matches!(e.outcome, Outcome::Skipped(_)), "{e:?}");
    }

    #[test]
    fn dump_with_errors_is_refused() {
        let mut sim = Sim::new();
        let mut dump = read_snapshot(&mut sim, &WIRED).values;
        dump.insert("p5.dpi".into(), "error: Failed".into());
        sim.dpi.insert(1, [0x03, 0x20, 0x03, 0x20]);
        let r = restore(&mut sim, &WIRED, &dump, false);
        assert!(r.refused.unwrap().contains("read errors"));
        assert!(r.entries.is_empty());
        assert_eq!(sim.dpi[&1], [0x03, 0x20, 0x03, 0x20]);
    }

    #[test]
    fn changed_active_profile_is_refused() {
        let mut sim = Sim::new();
        let mut dump = read_snapshot(&mut sim, &WIRED).values;
        dump.insert("profiles.active".into(), "2".into());
        let r = restore(&mut sim, &WIRED, &dump, false);
        assert!(r.refused.unwrap().contains("profiles.active"));
    }

    #[test]
    fn receiver_restores_profile_1_and_skips_unverified_live_values() {
        let mut sim = Sim::new();
        let dump = read_snapshot(&mut sim, &RECEIVER).values;
        sim.dpi.insert(1, [0x03, 0x20, 0x03, 0x20]);
        sim.brightness.insert((1, LED_LOGO), 100);
        // Live brightness through the receiver is not verified.
        sim.brightness.insert((0, LED_LOGO), 100);
        let r = restore(&mut sim, &RECEIVER, &dump, false);
        assert!(matches!(outcome(&r, "p1.dpi"), Outcome::Restored { .. }));
        assert!(matches!(
            outcome(&r, "p1.led.logo.brightness"),
            Outcome::Restored { .. }
        ));
        assert!(matches!(
            outcome(&r, "live.led.logo.brightness"),
            Outcome::Skipped(_)
        ));
        assert_eq!(sim.brightness[&(0, LED_LOGO)], 100);
        // The unverified value is reported separately, not as unrestored.
        assert!(r.ok(), "{r:#?}");
        assert!(r.unrestored().is_empty());
        assert!(
            r.outside_scope()
                .iter()
                .any(|(k, _, _)| k == "live.led.logo.brightness")
        );
    }

    #[test]
    fn failure_stops_the_run_and_lists_the_rest() {
        let mut sim = Sim::new();
        let dump = read_snapshot(&mut sim, &WIRED).values;
        sim.dpi.insert(1, [0x03, 0x20, 0x03, 0x20]);
        sim.brightness.insert((1, LED_LOGO), 100);
        sim.ignore_writes = true;
        let r = restore(&mut sim, &WIRED, &dump, false);
        assert!(!r.ok());
        assert!(matches!(outcome(&r, "p1.dpi"), Outcome::Failed(_)));
        assert_eq!(
            *outcome(&r, "p1.led.logo.brightness"),
            Outcome::NotAttempted
        );
        assert!(r.remaining.iter().any(|(k, _, _)| k == "p1.dpi"));
    }

    #[test]
    fn nacked_but_applied_write_counts_as_restored() {
        let mut sim = Sim::new();
        let dump = read_snapshot(&mut sim, &WIRED).values;
        sim.dpi.insert(1, [0x03, 0x20, 0x03, 0x20]);
        sim.nack_applied_writes = true;
        let r = restore(&mut sim, &WIRED, &dump, false);
        assert!(r.ok(), "{r:#?}");
        match outcome(&r, "p1.dpi") {
            Outcome::Restored { note } => assert!(note.contains("Failed")),
            other => panic!("{other:?}"),
        }
    }
}
