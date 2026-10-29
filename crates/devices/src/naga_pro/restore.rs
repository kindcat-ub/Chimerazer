//! Restore of verified values from a dump.
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
    pub remaining: Vec<(String, Option<String>, Option<String>)>,
}

impl RestoreReport {
    /// All verified values match the dump and nothing failed.
    pub fn ok(&self) -> bool {
        self.refused.is_none()
            && self
                .entries
                .iter()
                .all(|e| !matches!(e.outcome, Outcome::Failed(_) | Outcome::NotAttempted))
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
        if dry_run {
            entry.outcome = Outcome::WouldRestore;
            report.entries.push(entry);
            continue;
        }

        let mode = WriteMode::Normal {
            scopes: VERIFIED_WRITES,
        };
        let written = match write_request(
            usb,
            param.write_spec(),
            param.read_spec(),
            &param.write_args(&target),
            mode,
        ) {
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
                failed = true;
                report.entries.push(entry);
                continue;
            }
        };
        // The read-back decides, as in validation.
        match read_value(io, usb, param, &mut entry.traces) {
            Ok(after) => {
                entry.after = Some(param.render(&after));
                entry.outcome = match (after == target, written) {
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
    fn receiver_restores_only_live_dpi() {
        let mut sim = Sim::new();
        let dump = read_snapshot(&mut sim, &RECEIVER).values;
        sim.dpi.insert(1, [0x03, 0x20, 0x03, 0x20]);
        sim.dpi.insert(0, [0x03, 0x20, 0x03, 0x20]);
        let r = restore(&mut sim, &RECEIVER, &dump, false);
        assert!(matches!(outcome(&r, "p1.dpi"), Outcome::Skipped(_)));
        assert!(matches!(outcome(&r, "live.dpi"), Outcome::Restored { .. }));
        // Profile 1 was not written through the receiver.
        assert_eq!(sim.dpi[&1], [0x03, 0x20, 0x03, 0x20]);
        assert!(r.remaining.iter().any(|(k, _, _)| k == "p1.dpi"));
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
