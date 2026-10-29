//! Snapshot of everything readable on a Naga Pro, as normalized
//! `key = value` lines.
//!
//! Normalized means: no transport metadata (product ID, link, hidraw path),
//! no unit identifiers (the serial number is never read), and link-dependent
//! values under the `volatile.` prefix so that comparisons can skip them.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use chimerazer_protocol::{ARGS_LEN, DpiStages, DpiXy, LedBrightness, ProfileInventory};
use chimerazer_transport::FeatureIo;

use super::*;
use crate::{CommandSpec, ExchangeError, Trace, UsbId, exchange, read_request};

/// Keys with this prefix depend on the link or on time (battery, charging)
/// and are not part of the comparable state.
pub const VOLATILE_PREFIX: &str = "volatile.";

/// Storages whose HyperShift layer (`02:8C` with layer `01`) is read, as
/// `<storage>.button.NN.hs`. Profiles 2-5 belong to M5.
pub const HYPERSHIFT_STORAGES: [u8; 2] = [LIVE_STORAGE, 1];

/// Normalized state plus the raw exchanges that produced it.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub values: BTreeMap<String, String>,
    pub traces: Vec<Trace>,
}

impl Snapshot {
    /// `key = value` lines, sorted.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (k, v) in &self.values {
            let _ = writeln!(out, "{k} = {v}");
        }
        out
    }
}

/// Parses `key = value` lines, ignoring blank lines and `#` comments.
pub fn parse_text(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once(" = "))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Differences between two snapshots, skipping volatile keys:
/// `(key, left, right)` with `None` for a missing key.
pub fn compare(
    left: &BTreeMap<String, String>,
    right: &BTreeMap<String, String>,
) -> Vec<(String, Option<String>, Option<String>)> {
    let keys: std::collections::BTreeSet<&String> = left.keys().chain(right.keys()).collect();
    keys.into_iter()
        .filter(|k| !k.starts_with(VOLATILE_PREFIX))
        .filter_map(|k| {
            let (l, r) = (left.get(k), right.get(k));
            (l != r).then(|| (k.clone(), l.cloned(), r.cloned()))
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn storage_key(storage: u8) -> String {
    if storage == LIVE_STORAGE {
        "live".to_string()
    } else {
        format!("p{storage}")
    }
}

struct Reader<'a, IO> {
    io: &'a mut IO,
    usb: &'a UsbId,
    snapshot: Snapshot,
}

impl<IO: FeatureIo> Reader<'_, IO> {
    /// Runs one read; on failure records `error: ...` under `key` and
    /// returns `None`.
    fn read(&mut self, key: &str, spec: &CommandSpec, args: &[u8]) -> Option<[u8; ARGS_LEN]> {
        let request = read_request(self.usb, spec, args).expect("catalogued read");
        let (trace, result) = exchange(self.io, self.usb, &request);
        self.snapshot.traces.push(trace);
        match result {
            Ok(response) => Some(response.args),
            Err(e) => {
                let msg = match e {
                    ExchangeError::Device(s) => format!("{s:?}"),
                    other => other.to_string(),
                };
                self.set(key, format!("error: {msg}"));
                None
            }
        }
    }

    fn set(&mut self, key: &str, value: String) {
        self.snapshot.values.insert(key.to_string(), value);
    }

    fn echo_ok(&mut self, key: &str, got: u8, want: u8) -> bool {
        if got != want {
            self.set(key, format!("error: echo {got:#04x}, expected {want:#04x}"));
        }
        got == want
    }
}

/// Reads every catalogued command except profile metadata (`05:88`, which
/// the Naga Pro rejects): device settings, the live layer and all five
/// onboard profile banks.
pub fn read_snapshot(io: &mut impl FeatureIo, usb: &UsbId) -> Snapshot {
    let mut r = Reader {
        io,
        usb,
        snapshot: Snapshot::default(),
    };

    if let Some(a) = r.read("device.firmware", &FIRMWARE_VERSION, &[]) {
        r.set("device.firmware", format!("{}.{:02}", a[0], a[1]));
    }
    if let Some(a) = r.read("device.mode", &DEVICE_MODE, &[]) {
        r.set("device.mode", hex(&a[..2]));
    }
    if let Some(a) = r.read("device.poll_rate", &POLL_RATE, &[]) {
        let v = match a[0] {
            0x01 => "1000 Hz".to_string(),
            0x02 => "500 Hz".to_string(),
            0x08 => "125 Hz".to_string(),
            other => format!("raw {other:02x}"),
        };
        r.set("device.poll_rate", v);
    }
    if let Some(a) = r.read("power.low_battery_threshold", &LOW_BATTERY_THRESHOLD, &[]) {
        r.set("power.low_battery_threshold", format!("{:#04x}", a[0]));
    }
    if let Some(a) = r.read("power.idle_seconds", &IDLE_TIME, &[]) {
        r.set(
            "power.idle_seconds",
            u16::from_be_bytes([a[0], a[1]]).to_string(),
        );
    }
    if let Some(a) = r.read("volatile.battery", &BATTERY_LEVEL, &[]) {
        r.set("volatile.battery", format!("{}/255", a[1]));
    }
    if let Some(a) = r.read("volatile.charging", &CHARGING, &[]) {
        r.set("volatile.charging", (a[1] == 1).to_string());
    }

    if let Some(a) = r.read("profiles.summary", &PROFILE_SUMMARY, &[]) {
        r.set("profiles.summary", hex(&a[..3]));
    }
    if let Some(a) = r.read("profiles.count", &PROFILE_COUNT, &[]) {
        r.set("profiles.count", a[0].to_string());
    }
    if let Some(a) = r.read("profiles.assigned", &PROFILE_INVENTORY, &[]) {
        match ProfileInventory::decode(&a) {
            Ok(inv) => {
                r.set("profiles.max", inv.max_profile.to_string());
                r.set(
                    "profiles.assigned",
                    inv.assigned
                        .iter()
                        .map(u8::to_string)
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
            Err(e) => r.set("profiles.assigned", format!("error: {e}")),
        }
    }
    if let Some(a) = r.read("profiles.active", &ACTIVE_PROFILE, &[]) {
        r.set("profiles.active", a[0].to_string());
    }
    if let Some(a) = r.read("buttons.inventory", &BUTTON_INVENTORY, &[0; 5]) {
        match chimerazer_protocol::ButtonInventory::decode(&a) {
            Ok(inv) => r.set(
                "buttons.inventory",
                inv.slots
                    .iter()
                    .map(u8::to_string)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            Err(e) => r.set("buttons.inventory", format!("error: {e}")),
        }
    }

    for storage in std::iter::once(LIVE_STORAGE).chain(PROFILES) {
        read_storage(&mut r, storage);
    }

    r.snapshot
}

fn read_storage<IO: FeatureIo>(r: &mut Reader<'_, IO>, storage: u8) {
    let st = storage_key(storage);

    let key = format!("{st}.dpi");
    if let Some(a) = r.read(&key, &DPI, &[storage])
        && r.echo_ok(&key, a[0], storage)
    {
        match DpiXy::decode(&a) {
            Ok(d) => r.set(&key, format!("{}x{}", d.x, d.y)),
            Err(e) => r.set(&key, format!("error: {e}")),
        }
    }

    let key = format!("{st}.dpi_stages");
    if let Some(a) = r.read(&key, &DPI_STAGES, &[storage])
        && r.echo_ok(&key, a[0], storage)
    {
        match DpiStages::decode(&a) {
            Ok(t) => {
                let rows: Vec<String> = t
                    .stages
                    .iter()
                    .map(|s| format!("{}:{}x{}", s.id, s.x, s.y))
                    .collect();
                r.set(&key, format!("active={} {}", t.active, rows.join(" ")));
                // Rows verbatim, reserved bytes included: they are not
                // always zero and must survive a future restore.
                let len = 3 + usize::from(a[2]) * 7;
                r.set(&format!("{key}.raw"), hex(&a[1..len]));
            }
            Err(e) => r.set(&key, format!("error: {e}")),
        }
    }

    for (led, name) in [(LED_SCROLL_WHEEL, "scroll_wheel"), (LED_LOGO, "logo")] {
        let key = format!("{st}.led.{name}.brightness");
        if let Some(a) = r.read(&key, &LED_BRIGHTNESS, &[storage, led])
            && r.echo_ok(&key, a[1], led)
        {
            match LedBrightness::decode(&a) {
                Ok(b) => r.set(&key, b.value.to_string()),
                Err(e) => r.set(&key, format!("error: {e}")),
            }
        }
        let key = format!("{st}.led.{name}.effect");
        if let Some(a) = r.read(&key, &LED_EFFECT, &[storage, led])
            && r.echo_ok(&key, a[1], led)
        {
            // Effect ID and parameters; storage echo (args[0]) is not
            // reliable on stored-profile reads and is left out.
            r.set(&key, hex(&a[2..12]));
        }
    }

    for slot in BUTTON_SLOTS {
        let base = format!("{st}.button.{:02}", slot.id);
        let hypershift = HYPERSHIFT_STORAGES.contains(&storage);
        for (layer, key) in [(0, base.clone()), (1, format!("{base}.hs"))] {
            if layer == 1 && !hypershift {
                continue;
            }
            if let Some(a) = r.read(&key, &BUTTON_FUNCTION, &[storage, slot.id, layer]) {
                match decode_button_function(&a, storage, slot.id) {
                    Ok(b) => r.set(
                        &key,
                        format!("layer={} block={}", b.stored_layer, hex(&b.block)),
                    ),
                    Err(e) => r.set(&key, format!("error: {e}")),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips_and_compare_skips_volatile() {
        let mut s = Snapshot::default();
        s.values.insert("live.dpi".into(), "1600x1600".into());
        s.values.insert("volatile.battery".into(), "242/255".into());
        let text = format!("# comment\n{}", s.to_text());
        let parsed = parse_text(&text);
        assert_eq!(parsed, s.values);

        let mut other = parsed.clone();
        other.insert("volatile.battery".into(), "249/255".into());
        assert!(compare(&parsed, &other).is_empty());

        other.insert("live.dpi".into(), "800x800".into());
        other.insert("p1.dpi".into(), "400x400".into());
        assert_eq!(
            compare(&parsed, &other),
            vec![
                (
                    "live.dpi".into(),
                    Some("1600x1600".into()),
                    Some("800x800".into())
                ),
                ("p1.dpi".into(), None, Some("400x400".into())),
            ]
        );
    }
}
