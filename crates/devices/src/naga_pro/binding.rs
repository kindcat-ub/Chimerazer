//! Button bindings: the 7-byte function block of `02:8C` / `02:0C`.
//!
//! A block is `[class, length, data..]`. The firmware's own defaults and
//! Synapse/OpenSnek writes use different lengths for the same data
//! (`02 01 00 <key>` and `02 02 <mods> <key>`), so a decoded binding keeps
//! its form and encodes back to the same bytes. Anything not recognised is
//! kept as raw bytes. Sources: `opensnek-naga-default-banks`,
//! `opensnek-naga-side-panels`, `opensnek-naga-button-encoding` and our
//! own reads (`chimerazer-m1-reads`).

use std::fmt;

/// A function block exactly as stored.
pub type Block = [u8; 7];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binding {
    /// `00 00 ..`: the button does nothing.
    Disabled,
    /// `01 01 <button>`.
    Mouse { button: u8 },
    /// `02 02 <mods> <key>`, or the firmware-native `02 01 00 <key>`.
    Key { mods: u8, key: u8, native: bool },
    /// `0d 04 <mods> <key> <rate_hi> <rate_lo>`.
    KeyTurbo { mods: u8, key: u8, rate: u16 },
    /// `0e 03 <button> <rate_hi> <rate_lo>`, or the native `0e 01 ..`.
    MouseTurbo { button: u8, rate: u16, native: bool },
    /// `06 01 <action>`: `01` stage down, `02` stage up, `06` cycle.
    Dpi { action: u8 },
    /// `07 01 04`: cycle onboard profiles.
    ProfileCycle,
    /// Any other block, kept byte for byte.
    Raw(Block),
}

impl Binding {
    pub fn decode(b: &Block) -> Self {
        let rate = |hi: u8, lo: u8| u16::from_be_bytes([hi, lo]);
        match *b {
            [0, 0, 0, 0, 0, 0, 0] => Self::Disabled,
            [0x01, 0x01, button, 0, 0, 0, 0] => Self::Mouse { button },
            [0x02, 0x02, mods, key, 0, 0, 0] => Self::Key {
                mods,
                key,
                native: false,
            },
            [0x02, 0x01, 0, key, 0, 0, 0] => Self::Key {
                mods: 0,
                key,
                native: true,
            },
            [0x0D, 0x04, mods, key, hi, lo, 0] => Self::KeyTurbo {
                mods,
                key,
                rate: rate(hi, lo),
            },
            [0x0E, len @ (0x01 | 0x03), button, hi, lo, 0, 0] => Self::MouseTurbo {
                button,
                rate: rate(hi, lo),
                native: len == 0x01,
            },
            [0x06, 0x01, action, 0, 0, 0, 0] => Self::Dpi { action },
            [0x07, 0x01, 0x04, 0, 0, 0, 0] => Self::ProfileCycle,
            raw => Self::Raw(raw),
        }
    }

    pub fn encode(&self) -> Block {
        match *self {
            Self::Disabled => [0; 7],
            Self::Mouse { button } => [0x01, 0x01, button, 0, 0, 0, 0],
            Self::Key {
                mods,
                key,
                native: false,
            } => [0x02, 0x02, mods, key, 0, 0, 0],
            Self::Key {
                key, native: true, ..
            } => [0x02, 0x01, 0, key, 0, 0, 0],
            Self::KeyTurbo { mods, key, rate } => {
                let [hi, lo] = rate.to_be_bytes();
                [0x0D, 0x04, mods, key, hi, lo, 0]
            }
            Self::MouseTurbo {
                button,
                rate,
                native,
            } => {
                let [hi, lo] = rate.to_be_bytes();
                [0x0E, if native { 0x01 } else { 0x03 }, button, hi, lo, 0, 0]
            }
            Self::Dpi { action } => [0x06, 0x01, action, 0, 0, 0, 0],
            Self::ProfileCycle => [0x07, 0x01, 0x04, 0, 0, 0, 0],
            Self::Raw(b) => b,
        }
    }

    /// Parses the text form written by `Display`, e.g. `key:ctrl+f5`,
    /// `key:up turbo=55`, `mouse:back`, `dpi:cycle`, `raw:0e030900640000`.
    /// New bindings use the Synapse/OpenSnek forms, not the native ones.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim().replace(",turbo=", " turbo=");
        let text = text.as_str();
        let (body, turbo) = match text.split_once(" turbo=") {
            Some((b, r)) => (b, Some(parse_u16(r)?)),
            None => (text, None),
        };
        let (kind, arg) = body.split_once(':').unwrap_or((body, ""));
        match (kind, turbo) {
            ("disabled", None) => Ok(Self::Disabled),
            ("profile-cycle", None) => Ok(Self::ProfileCycle),
            ("dpi", None) => Ok(Self::Dpi {
                action: match arg {
                    "down" => 0x01,
                    "up" => 0x02,
                    "cycle" => 0x06,
                    other => parse_u8(other)?,
                },
            }),
            ("mouse", turbo) => {
                let button = mouse_button(arg)?;
                Ok(match turbo {
                    None => Self::Mouse { button },
                    Some(rate) => Self::MouseTurbo {
                        button,
                        rate,
                        native: false,
                    },
                })
            }
            ("key", turbo) => {
                let (mods, key) = key_chord(arg)?;
                Ok(match turbo {
                    None => Self::Key {
                        mods,
                        key,
                        native: false,
                    },
                    Some(rate) => Self::KeyTurbo { mods, key, rate },
                })
            }
            ("raw", None) => {
                let hex = arg.replace(' ', "");
                if hex.len() != 14 {
                    return Err(format!("raw needs 7 bytes: {arg}"));
                }
                let mut b = [0; 7];
                for (i, byte) in b.iter_mut().enumerate() {
                    *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                        .map_err(|_| format!("bad raw block {arg}"))?;
                }
                Ok(Self::Raw(b))
            }
            _ => Err(format!(
                "unknown binding {text} (disabled, mouse:<button>, key:<mods+key>, \
                 dpi:<down|up|cycle>, profile-cycle, raw:<14 hex>; turbo=<rate> after mouse/key)"
            )),
        }
    }
}

fn parse_u8(s: &str) -> Result<u8, String> {
    match s.strip_prefix("0x") {
        Some(h) => u8::from_str_radix(h, 16),
        None => s.parse(),
    }
    .map_err(|_| format!("bad number {s}"))
}

fn parse_u16(s: &str) -> Result<u16, String> {
    match s.strip_prefix("0x") {
        Some(h) => u16::from_str_radix(h, 16),
        None => s.parse(),
    }
    .map_err(|_| format!("bad number {s}"))
}

const MOUSE_BUTTONS: &[(u8, &str)] = &[
    (0x01, "left"),
    (0x02, "right"),
    (0x03, "middle"),
    (0x04, "back"),
    (0x05, "forward"),
    (0x09, "scroll-up"),
    (0x0A, "scroll-down"),
];

fn mouse_button(s: &str) -> Result<u8, String> {
    MOUSE_BUTTONS
        .iter()
        .find(|(_, n)| *n == s)
        .map(|(id, _)| *id)
        .map_or_else(|| parse_u8(s), Ok)
}

/// HID modifier bits (USB HID usage page 0x07, byte 0 of a keyboard report).
const MODIFIERS: &[(u8, &str)] = &[
    (0x01, "ctrl"),
    (0x02, "shift"),
    (0x04, "alt"),
    (0x08, "super"),
    (0x10, "rctrl"),
    (0x20, "rshift"),
    (0x40, "ralt"),
    (0x80, "rsuper"),
];

/// HID keyboard usages with a name; others are written as `0xNN`.
fn key_name(key: u8) -> Option<String> {
    let named = match key {
        0x04..=0x1D => return Some(char::from(b'a' + key - 0x04).to_string()),
        0x1E..=0x26 => return Some(char::from(b'1' + key - 0x1E).to_string()),
        0x27 => "0",
        0x28 => "enter",
        0x29 => "esc",
        0x2A => "backspace",
        0x2B => "tab",
        0x2C => "space",
        0x2D => "minus",
        0x2E => "equal",
        0x2F => "lbracket",
        0x30 => "rbracket",
        0x31 => "backslash",
        0x33 => "semicolon",
        0x34 => "quote",
        0x35 => "grave",
        0x36 => "comma",
        0x37 => "dot",
        0x38 => "slash",
        0x39 => "capslock",
        0x3A..=0x45 => return Some(format!("f{}", key - 0x39)),
        0x46 => "printscreen",
        0x47 => "scrolllock",
        0x48 => "pause",
        0x49 => "insert",
        0x4A => "home",
        0x4B => "pageup",
        0x4C => "delete",
        0x4D => "end",
        0x4E => "pagedown",
        0x4F => "right",
        0x50 => "left",
        0x51 => "down",
        0x52 => "up",
        0x53 => "numlock",
        0x59..=0x61 => return Some(format!("kp{}", key - 0x58)),
        0x62 => "kp0",
        0x68..=0x73 => return Some(format!("f{}", key - 0x68 + 13)),
        _ => return None,
    };
    Some(named.to_string())
}

fn key_code(name: &str) -> Result<u8, String> {
    if let Some(code) = (0..=u8::MAX).find(|&k| key_name(k).as_deref() == Some(name)) {
        return Ok(code);
    }
    parse_u8(name).map_err(|_| format!("unknown key {name}"))
}

fn key_chord(s: &str) -> Result<(u8, u8), String> {
    let mut parts: Vec<&str> = s.split('+').collect();
    let key = key_code(
        parts
            .pop()
            .filter(|k| !k.is_empty())
            .ok_or("key: missing key")?,
    )?;
    let mut mods = 0;
    for m in parts {
        mods |= MODIFIERS
            .iter()
            .find(|(_, n)| *n == m)
            .map(|(bit, _)| *bit)
            .ok_or(format!("unknown modifier {m}"))?;
    }
    Ok((mods, key))
}

fn chord(mods: u8, key: u8) -> String {
    let mut s = String::new();
    for (bit, name) in MODIFIERS {
        if mods & bit != 0 {
            s.push_str(name);
            s.push('+');
        }
    }
    match key_name(key) {
        Some(n) => s.push_str(&n),
        None => s.push_str(&format!("0x{key:02x}")),
    }
    s
}

fn button(id: u8) -> String {
    MOUSE_BUTTONS
        .iter()
        .find(|(b, _)| *b == id)
        .map_or_else(|| format!("0x{id:02x}"), |(_, n)| (*n).to_string())
}

impl fmt::Display for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let native = |n: bool| if n { " (native)" } else { "" };
        match *self {
            Self::Disabled => write!(f, "disabled"),
            Self::Mouse { button: b } => write!(f, "mouse:{}", button(b)),
            Self::Key {
                mods,
                key,
                native: n,
            } => write!(f, "key:{}{}", chord(mods, key), native(n)),
            Self::KeyTurbo { mods, key, rate } => {
                write!(f, "key:{} turbo={rate}", chord(mods, key))
            }
            Self::MouseTurbo {
                button: b,
                rate,
                native: n,
            } => write!(f, "mouse:{} turbo={rate}{}", button(b), native(n)),
            Self::Dpi { action } => match action {
                0x01 => write!(f, "dpi:down"),
                0x02 => write!(f, "dpi:up"),
                0x06 => write!(f, "dpi:cycle"),
                a => write!(f, "dpi:0x{a:02x}"),
            },
            Self::ProfileCycle => write!(f, "profile-cycle"),
            Self::Raw(b) => {
                write!(f, "raw:")?;
                b.iter().try_for_each(|x| write!(f, "{x:02x}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blocks read from our unit (`fixtures/naga-pro/our-hardware-frames.txt`)
    /// and from the OpenSnek Naga Pro captures, with their text form.
    const KNOWN: &[(Block, &str)] = &[
        ([0x01, 0x01, 0x01, 0, 0, 0, 0], "mouse:left"),
        ([0x01, 0x01, 0x04, 0, 0, 0, 0], "mouse:back"),
        ([0x01, 0x01, 0x0A, 0, 0, 0, 0], "mouse:scroll-down"),
        ([0x06, 0x01, 0x01, 0, 0, 0, 0], "dpi:down"),
        ([0x06, 0x01, 0x02, 0, 0, 0, 0], "dpi:up"),
        ([0x07, 0x01, 0x04, 0, 0, 0, 0], "profile-cycle"),
        (
            [0x0E, 0x03, 0x09, 0x00, 0x64, 0, 0],
            "mouse:scroll-up turbo=100",
        ),
        ([0x02, 0x02, 0x00, 0x1E, 0, 0, 0], "key:1"),
        ([0x02, 0x02, 0x00, 0x2E, 0, 0, 0], "key:equal"),
        ([0x02, 0x02, 0x00, 0x46, 0, 0, 0], "key:printscreen"),
        ([0x02, 0x02, 0x00, 0x4F, 0, 0, 0], "key:right"),
        ([0x0D, 0x04, 0x00, 0x52, 0x00, 0x37, 0], "key:up turbo=55"),
        // Firmware-native forms from the unassigned banks.
        ([0x02, 0x01, 0x00, 0x1E, 0, 0, 0], "key:1 (native)"),
        (
            [0x0E, 0x01, 0x09, 0x00, 0x14, 0, 0],
            "mouse:scroll-up turbo=20 (native)",
        ),
        // The Synapse-written mouse form stays raw.
        ([0x01, 0x01, 0x01, 0x04, 0xE4, 0, 0], "raw:01010104e40000"),
        ([0, 0, 0, 0, 0, 0, 0], "disabled"),
    ];

    #[test]
    fn known_blocks_round_trip() {
        for (block, text) in KNOWN {
            let b = Binding::decode(block);
            assert_eq!(b.to_string(), *text, "{block:02x?}");
            assert_eq!(b.encode(), *block, "{text}");
        }
    }

    #[test]
    fn text_forms_parse_to_written_forms() {
        for (block, text) in KNOWN.iter().filter(|(_, t)| !t.contains("native")) {
            assert_eq!(Binding::parse(text).unwrap().encode(), *block, "{text}");
        }
        assert_eq!(
            Binding::parse("key:ctrl+shift+f5").unwrap().encode(),
            [0x02, 0x02, 0x03, 0x3E, 0, 0, 0]
        );
        assert_eq!(
            Binding::parse("mouse:back turbo=142").unwrap().encode(),
            [0x0E, 0x03, 0x04, 0x00, 0x8E, 0, 0]
        );
        assert_eq!(
            Binding::parse("key:up,turbo=55").unwrap().encode(),
            [0x0D, 0x04, 0x00, 0x52, 0x00, 0x37, 0]
        );
        assert_eq!(
            Binding::parse("key:0x87").unwrap().encode(),
            [0x02, 0x02, 0, 0x87, 0, 0, 0]
        );
        for bad in [
            "key:",
            "key:hyper+a",
            "mouse:sideways",
            "raw:0102",
            "dpi:up turbo=5",
            "nope",
        ] {
            assert!(Binding::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn every_named_key_round_trips() {
        for k in 0..=u8::MAX {
            if let Some(name) = key_name(k) {
                assert_eq!(key_code(&name), Ok(k), "{name}");
            }
        }
    }
}
