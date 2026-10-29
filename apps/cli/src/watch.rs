//! `chimerazer watch`: prints what the mouse sends to the system while
//! buttons are pressed, so remaps can be checked without typing into an
//! editor. Reads the Razer evdev nodes (`/dev/input/eventN`); needs read
//! access to them (group `input`). Test aid only, no device writes.

use std::fs::{self, File};
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use chimerazer_devices::RAZER_VENDOR_ID;

const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const REL_HWHEEL: u16 = 0x06;
const REL_WHEEL: u16 = 0x08;
/// `struct input_event` on 64-bit Linux: timeval (16), type, code, value.
const EVENT_LEN: usize = 24;

/// Razer evdev nodes, found through sysfs like the hidraw ones.
fn razer_event_nodes() -> Result<Vec<PathBuf>, String> {
    let mut nodes = Vec::new();
    let dir = fs::read_dir("/sys/class/input").map_err(|e| format!("/sys/class/input: {e}"))?;
    for entry in dir.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|n| n.starts_with("event")) else {
            continue;
        };
        let vendor = fs::read_to_string(entry.path().join("device/id/vendor")).unwrap_or_default();
        if u16::from_str_radix(vendor.trim(), 16).ok() == Some(RAZER_VENDOR_ID) {
            nodes.push(PathBuf::from("/dev/input").join(name));
        }
    }
    nodes.sort();
    Ok(nodes)
}

/// Linux input key code to a short name; letters, digits and the
/// punctuation used by remap probes print as the character itself.
fn key_name(code: u16) -> String {
    const ROWS: &[(u16, &str)] = &[
        (2, "1234567890-="),
        (16, "qwertyuiop"),
        (30, "asdfghjkl"),
        (44, "zxcvbnm,./"),
    ];
    for (start, chars) in ROWS {
        if let Some(c) = code
            .checked_sub(*start)
            .and_then(|i| chars.chars().nth(usize::from(i)))
        {
            return c.to_string();
        }
    }
    match code {
        1 => "<esc>".into(),
        14 => "<backspace>".into(),
        15 => "<tab>".into(),
        28 => "<enter>".into(),
        57 => "<space>".into(),
        59..=68 => format!("<f{}>", code - 58),
        87 | 88 => format!("<f{}>", code - 76),
        99 => "<printscreen>".into(),
        103 => "<up>".into(),
        105 => "<left>".into(),
        106 => "<right>".into(),
        108 => "<down>".into(),
        0x110 => "<lmb>".into(),
        0x111 => "<rmb>".into(),
        0x112 => "<mmb>".into(),
        0x113 => "<back>".into(),
        0x114 => "<forward>".into(),
        other => format!("<key {other}>"),
    }
}

/// One line per press, then the presses joined into a string.
pub fn watch(args: &[String]) -> Result<(), String> {
    let seconds: u64 = match args {
        [] => 60,
        [flag, n] if flag == "--seconds" => n.parse().map_err(|_| format!("bad seconds {n}"))?,
        _ => return Err("usage: chimerazer watch [--seconds N]".into()),
    };
    let nodes = razer_event_nodes()?;
    if nodes.is_empty() {
        return Err("no Razer input devices found".into());
    }
    let (tx, rx) = mpsc::channel();
    let mut opened = 0;
    for node in nodes {
        match File::open(&node) {
            Ok(mut f) => {
                opened += 1;
                let tx = tx.clone();
                thread::spawn(move || {
                    let mut buf = [0u8; EVENT_LEN];
                    while f.read_exact(&mut buf).is_ok() {
                        let ty = u16::from_ne_bytes([buf[16], buf[17]]);
                        let code = u16::from_ne_bytes([buf[18], buf[19]]);
                        let value = i32::from_ne_bytes([buf[20], buf[21], buf[22], buf[23]]);
                        if tx.send((ty, code, value)).is_err() {
                            break;
                        }
                    }
                });
            }
            Err(e) => eprintln!("{}: {e}", node.display()),
        }
    }
    if opened == 0 {
        return Err("cannot open any Razer input device (add yourself to group `input`)".into());
    }
    drop(tx);
    eprintln!("listening for {seconds} s on {opened} device(s); press buttons now");
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut typed = String::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Ok((ty, code, value)) = rx.recv_timeout(left) else {
            break;
        };
        let name = match (ty, code, value) {
            (EV_KEY, code, 1) => key_name(code),
            (EV_REL, REL_WHEEL, v) => format!("<wheel {}>", if v > 0 { "up" } else { "down" }),
            (EV_REL, REL_HWHEEL, v) => format!("<tilt {}>", if v > 0 { "right" } else { "left" }),
            _ => continue,
        };
        println!("{name}");
        typed.push_str(&name);
    }
    println!("typed: {typed}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::key_name;

    #[test]
    fn probe_keys_print_as_characters() {
        let probes: String = [
            30, 48, 46, 32, 18, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17,
            45, 21, 44, 51, 52,
        ]
        .into_iter()
        .map(key_name)
        .collect();
        assert_eq!(probes, "abcdeghijklmnopqrstuvwxyz,.");
        assert_eq!(key_name(0x110), "<lmb>");
    }
}
