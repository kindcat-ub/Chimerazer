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

use super::binding::Binding;
use super::state::{compare, read_snapshot};
use super::*;
use crate::{CommandSpec, Trace, UsbId, WriteMode, exchange, read_request, write_request};

/// One writable value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Param {
    PollRate,
    Dpi {
        storage: u8,
    },
    DpiStages {
        storage: u8,
    },
    Brightness {
        storage: u8,
        led: u8,
    },
    /// LED effect (`0F:82` / `0F:02`); only static and spectrum are written.
    Effect {
        storage: u8,
        led: u8,
    },
    /// Button binding (`02:8C` / `02:0C`) of layer 0, or 1 for HyperShift.
    /// The value is the 7-byte function block ([`super::binding`]).
    Button {
        storage: u8,
        slot: u8,
        layer: u8,
    },
}

/// Effect IDs `0F:02` may write.
const EFFECT_STATIC: u8 = 0x01;
const EFFECT_SPECTRUM: u8 = 0x03;

fn led_name(led: u8) -> &'static str {
    if led == LED_LOGO {
        "logo"
    } else {
        "scroll_wheel"
    }
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
            Self::Effect { storage, led } => {
                write!(f, "effect (storage {storage}, led {led:#04x})")
            }
            Self::Button {
                storage,
                slot,
                layer,
            } => {
                let hs = if *layer == 1 { ", hypershift" } else { "" };
                write!(f, "button {slot} (storage {storage}{hs})")
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
    /// A button binding of one of the 30 slots in `BUTTON_SLOTS`.
    pub fn button(storage: u8, slot: u8) -> Result<Self, String> {
        Self::button_layer(storage, slot, 0)
    }

    /// A button binding of layer 0 or of the HyperShift layer 1.
    pub fn button_layer(storage: u8, slot: u8, layer: u8) -> Result<Self, String> {
        if layer > 1 {
            return Err(format!("layer {layer} is not 0 or 1"));
        }
        if storage != LIVE_STORAGE && !PROFILES.contains(&storage) {
            return Err(format!("storage {storage} is not 0..=5"));
        }
        if !BUTTON_SLOTS.iter().any(|s| s.id == slot) {
            return Err(format!("slot {slot} is not one of the 30 Naga Pro slots"));
        }
        Ok(Self::Button {
            storage,
            slot,
            layer,
        })
    }

    /// Parses `poll-rate`, `dpi`, `dpi-stages`, `brightness`, `effect` with the
    /// storage and LED they apply to.
    pub fn parse(name: &str, storage: u8, led: Option<u8>) -> Result<Self, String> {
        if storage != LIVE_STORAGE && !PROFILES.contains(&storage) {
            return Err(format!("storage {storage} is not 0..=5"));
        }
        match name {
            "poll-rate" => Ok(Self::PollRate),
            "dpi" => Ok(Self::Dpi { storage }),
            "dpi-stages" => Ok(Self::DpiStages { storage }),
            "brightness" | "effect" => {
                let led = led.ok_or(format!("{name} needs --led scroll_wheel|logo"))?;
                if led != LED_SCROLL_WHEEL && led != LED_LOGO {
                    return Err(format!("unknown LED {led:#04x}"));
                }
                Ok(if name == "effect" {
                    Self::Effect { storage, led }
                } else {
                    Self::Brightness { storage, led }
                })
            }
            other => Err(format!(
                "unknown parameter {other} (poll-rate, dpi, dpi-stages, brightness, effect)"
            )),
        }
    }

    pub fn read_spec(&self) -> &'static CommandSpec {
        match self {
            Self::PollRate => &POLL_RATE,
            Self::Dpi { .. } => &DPI,
            Self::DpiStages { .. } => &DPI_STAGES,
            Self::Brightness { .. } => &LED_BRIGHTNESS,
            Self::Effect { .. } => &LED_EFFECT,
            Self::Button { .. } => &BUTTON_FUNCTION,
        }
    }

    pub fn write_spec(&self) -> &'static CommandSpec {
        match self {
            Self::PollRate => &SET_POLL_RATE,
            Self::Dpi { .. } => &SET_DPI,
            Self::DpiStages { .. } => &SET_DPI_STAGES,
            Self::Brightness { .. } => &SET_LED_BRIGHTNESS,
            Self::Effect { .. } => &SET_LED_STATIC,
            Self::Button { .. } => &SET_BUTTON_FUNCTION,
        }
    }

    /// The write command for `value`: `0F:02` has one layout per effect.
    /// `None` if this value cannot be written.
    pub fn write_spec_for(&self, value: &[u8]) -> Option<&'static CommandSpec> {
        match self {
            Self::Effect { .. } => match value.first() {
                Some(&EFFECT_STATIC) => Some(&SET_LED_STATIC),
                Some(&EFFECT_SPECTRUM) => Some(&SET_LED_SPECTRUM),
                _ => None,
            },
            _ => Some(self.write_spec()),
        }
    }

    pub(crate) fn read_args(&self) -> Vec<u8> {
        match *self {
            Self::PollRate => vec![],
            Self::Dpi { storage } | Self::DpiStages { storage } => vec![storage],
            Self::Brightness { storage, led } | Self::Effect { storage, led } => {
                vec![storage, led]
            }
            Self::Button {
                storage,
                slot,
                layer,
            } => vec![storage, slot, layer],
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
            // The storage echo is not reliable on stored-profile reads.
            Self::Effect { led, .. } => {
                echo(1, led)?;
                Ok(args[2..12].to_vec())
            }
            // args[2] is a stored flag, not an echo; the block follows it.
            Self::Button { storage, slot, .. } => {
                echo(0, storage)?;
                echo(1, slot)?;
                Ok(args[3..10].to_vec())
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
            // The layouts of `SET_LED_STATIC` and `SET_LED_SPECTRUM`.
            Self::Effect { storage, led } => {
                let n = if value.first() == Some(&EFFECT_STATIC) {
                    7
                } else {
                    4
                };
                [&[storage, led][..], &value[..n.min(value.len())]].concat()
            }
            Self::Button {
                storage,
                slot,
                layer,
            } => [&[storage, slot, layer][..], value].concat(),
        }
    }

    /// The dump key holding this value (`naga_pro::state`).
    pub fn dump_key(&self) -> String {
        let st = |s: u8| {
            if s == LIVE_STORAGE {
                "live".to_string()
            } else {
                format!("p{s}")
            }
        };
        match *self {
            Self::PollRate => "device.poll_rate".into(),
            Self::Dpi { storage } => format!("{}.dpi", st(storage)),
            Self::DpiStages { storage } => format!("{}.dpi_stages.raw", st(storage)),
            Self::Brightness { storage, led } => {
                format!("{}.led.{}.brightness", st(storage), led_name(led))
            }
            Self::Effect { storage, led } => {
                format!("{}.led.{}.effect", st(storage), led_name(led))
            }
            Self::Button {
                storage,
                slot,
                layer,
            } => {
                let hs = if layer == 1 { ".hs" } else { "" };
                format!("{}.button.{slot:02}{hs}", st(storage))
            }
        }
    }

    /// Dump keys whose value this parameter determines.
    pub fn covered_keys(&self) -> Vec<String> {
        let key = self.dump_key();
        match self {
            Self::DpiStages { .. } => {
                let decoded = key.trim_end_matches(".raw").to_string();
                vec![key, decoded]
            }
            _ => vec![key],
        }
    }

    /// The value bytes stored in a dump under [`Param::dump_key`].
    pub fn from_dump(&self, text: &str) -> Result<Vec<u8>, String> {
        match self {
            Self::PollRate => match text {
                "1000 Hz" => Ok(vec![0x01]),
                "500 Hz" => Ok(vec![0x02]),
                "125 Hz" => Ok(vec![0x08]),
                _ => Err(format!("unknown poll rate {text}")),
            },
            Self::Dpi { .. } => {
                let (x, y) = text.split_once('x').ok_or(format!("bad DPI {text}"))?;
                let p = |v: &str| v.parse::<u16>().map_err(|_| format!("bad DPI {text}"));
                Ok([p(x)?.to_be_bytes(), p(y)?.to_be_bytes()].concat())
            }
            Self::DpiStages { .. } => {
                let bytes: Vec<u8> = text
                    .split_whitespace()
                    .map(|b| u8::from_str_radix(b, 16).map_err(|_| format!("bad stages {text}")))
                    .collect::<Result<_, _>>()?;
                let count = usize::from(*bytes.get(1).ok_or("empty stages")?);
                if count == 0 || count > 5 || bytes.len() != 2 + count * 7 {
                    return Err(format!("bad stage table {text}"));
                }
                Ok(bytes)
            }
            Self::Brightness { .. } => Ok(vec![
                text.parse().map_err(|_| format!("bad brightness {text}"))?,
            ]),
            Self::Effect { .. } => {
                let bytes: Vec<u8> = text
                    .split_whitespace()
                    .map(|b| u8::from_str_radix(b, 16).map_err(|_| format!("bad effect {text}")))
                    .collect::<Result<_, _>>()?;
                if bytes.len() != 10 {
                    return Err(format!("bad effect {text}"));
                }
                Ok(bytes)
            }
            // `layer=<flag> block=<hex>`, as `state` writes it.
            Self::Button { .. } => {
                let hex = text
                    .split_once("block=")
                    .map(|(_, h)| h.replace(' ', ""))
                    .ok_or(format!("bad button value {text}"))?;
                Ok(Binding::parse(&format!("raw:{hex}"))?.encode().to_vec())
            }
        }
    }

    fn storage(&self) -> Option<u8> {
        match *self {
            Self::PollRate => None,
            Self::Dpi { storage }
            | Self::DpiStages { storage }
            | Self::Brightness { storage, .. }
            | Self::Effect { storage, .. }
            | Self::Button { storage, .. } => Some(storage),
        }
    }

    /// Every write command this parameter may use.
    fn write_specs(&self) -> &'static [CommandSpec] {
        match self {
            Self::Effect { .. } => &[SET_LED_STATIC, SET_LED_SPECTRUM],
            Self::PollRate => std::slice::from_ref(&SET_POLL_RATE),
            Self::Dpi { .. } => std::slice::from_ref(&SET_DPI),
            Self::DpiStages { .. } => std::slice::from_ref(&SET_DPI_STAGES),
            Self::Brightness { .. } => std::slice::from_ref(&SET_LED_BRIGHTNESS),
            Self::Button { .. } => std::slice::from_ref(&SET_BUTTON_FUNCTION),
        }
    }

    fn spec_verified(&self, spec: &CommandSpec, link: crate::Link) -> bool {
        if let Self::Button {
            storage,
            slot,
            layer,
        } = *self
        {
            return spec.confirmation == crate::Confirmation::OurHardware
                && button_param_verified(storage, slot, layer, link);
        }
        spec.confirmation == crate::Confirmation::OurHardware
            && VERIFIED_WRITES.iter().any(|s| {
                // `Command` equality includes the size, which tells the
                // `0F:02` layouts apart.
                s.command == spec.command && s.link == link && s.storage == self.storage()
            })
    }

    /// Whether some value of this parameter may be written in normal
    /// operation on `link`. For effects, see [`Param::verified_for`].
    pub fn verified_on(&self, link: crate::Link) -> bool {
        self.write_specs()
            .iter()
            .any(|spec| self.spec_verified(spec, link))
    }

    /// Whether writing exactly `value` is verified on `link`: the write
    /// command `value` needs (with its size) must be in scope.
    pub fn verified_for(&self, link: crate::Link, value: &[u8]) -> bool {
        self.write_spec_for(value).is_some_and(|spec| {
            self.spec_verified(spec, link)
                && VERIFIED_WRITES.iter().any(|scope| {
                    scope.command == spec.command
                        && scope.link == link
                        && scope.storage == self.storage()
                        && scope
                            .accepts
                            .is_none_or(|accepts| accepts(&self.write_args(value)))
                })
        })
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
            Self::Effect { .. } => match value {
                [EFFECT_STATIC, 0, 0, 1, r, g, b, ..] => format!("static #{r:02x}{g:02x}{b:02x}"),
                [EFFECT_SPECTRUM, rest @ ..] if rest.iter().all(|&b| b == 0) => "spectrum".into(),
                _ => format!("effect [{}]", hex(value)),
            },
            Self::Button { .. } => match <[u8; 7]>::try_from(value) {
                Ok(b) => Binding::decode(&b).to_string(),
                Err(_) => format!("[{}]", hex(value)),
            },
        }
    }

    /// Builds a probe value from text, relative to the current `original`.
    /// Probes stay inside documented ranges and must differ from `original`.
    pub fn probe(&self, text: &str, original: &[u8]) -> Result<Vec<u8>, String> {
        let value = self.value(text, original)?;
        if value == original {
            return Err("probe equals the current value; choose a different one".into());
        }
        Ok(value)
    }

    /// Builds a value from text (the forms [`Param::probe`] accepts),
    /// relative to the `current` value, inside documented ranges.
    pub fn value(&self, text: &str, current: &[u8]) -> Result<Vec<u8>, String> {
        let original = current;
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
            Self::Effect { .. } => {
                let mut v = vec![0; 10];
                if text == "spectrum" {
                    v[0] = EFFECT_SPECTRUM;
                } else {
                    let hexrgb = text.trim_start_matches('#');
                    let rgb = u32::from_str_radix(hexrgb, 16)
                        .ok()
                        .filter(|_| hexrgb.len() == 6)
                        .ok_or("effect probe: spectrum or RRGGBB")?;
                    let [_, r, g, b] = rgb.to_be_bytes();
                    v[..7].copy_from_slice(&[EFFECT_STATIC, 0, 0, 1, r, g, b]);
                }
                v
            }
            Self::Button { .. } => Binding::parse(text)?.encode().to_vec(),
        };
        Ok(value)
    }
}

/// One step of a validation run.
#[derive(Debug)]
pub struct Step {
    /// The parameter this step acted on.
    pub param: Param,
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
    /// At least one write frame went out.
    wrote: bool,
}

pub(crate) enum WriteError {
    /// The write gate refused; no frame was sent.
    Refused(String),
    /// A frame was sent and the exchange reported an error.
    Reported(String),
    /// A frame was sent, but the button flag or HyperShift block was not preserved.
    Preservation(String),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(e) | Self::Reported(e) | Self::Preservation(e) => f.write_str(e),
        }
    }
}

/// Shared write path for validation and normal operations. A layer-0 write
/// changes the slot flag to zero; replaying the unchanged HyperShift block
/// restores flag one without replacing the user's HyperShift assignment.
pub(crate) fn write_value(
    io: &mut impl FeatureIo,
    usb: &UsbId,
    param: Param,
    value: &[u8],
    mode: WriteMode,
    traces: &mut Vec<Trace>,
    wrote: &mut bool,
) -> Result<(), WriteError> {
    let spec = param
        .write_spec_for(value)
        .ok_or_else(|| WriteError::Refused(format!("{param}: value cannot be written")))?;
    let request = write_request(usb, spec, param.read_spec(), &param.write_args(value), mode)
        .map_err(|e| WriteError::Refused(e.to_string()))?;
    if matches!(mode, WriteMode::Normal { .. }) && matches!(param, Param::Button { .. }) {
        let active = read_request(usb, &ACTIVE_PROFILE, &[])
            .map_err(|e| WriteError::Refused(e.to_string()))?;
        let (trace, result) = exchange(io, usb, &active);
        traces.push(trace);
        let response = result.map_err(|e| WriteError::Refused(e.to_string()))?;
        if response.args[0] != 1 {
            return Err(WriteError::Refused(
                "button writes require active profile 1".into(),
            ));
        }
    }
    let normal_secondary =
        matches!(mode, WriteMode::Normal { .. }) && matches!(param, Param::Button { layer: 1, .. });
    let secondary_primary = if normal_secondary {
        let (flag, _) = read_button(io, usb, param, traces).map_err(WriteError::Refused)?;
        if flag != 1 {
            return Err(WriteError::Refused(
                "secondary write would change the stored flag".into(),
            ));
        }
        let Param::Button { storage, slot, .. } = param else {
            unreachable!()
        };
        let primary = Param::Button {
            storage,
            slot,
            layer: 0,
        };
        let (flag, block) = read_button(io, usb, primary, traces).map_err(WriteError::Refused)?;
        if flag != 1 {
            return Err(WriteError::Refused(
                "button flags disagree between layers".into(),
            ));
        }
        Some((primary, block))
    } else {
        None
    };
    let saved = if let Param::Button {
        storage,
        slot,
        layer: 0,
    } = param
    {
        let (flag, _) = read_button(io, usb, param, traces).map_err(WriteError::Refused)?;
        match flag {
            0 => Some((flag, None)),
            1 => {
                let hs = Param::Button {
                    storage,
                    slot,
                    layer: 1,
                };
                let (hs_flag, block) =
                    read_button(io, usb, hs, traces).map_err(WriteError::Refused)?;
                if hs_flag != flag {
                    return Err(WriteError::Refused(
                        "button flags disagree between layers".into(),
                    ));
                }
                let repair_mode = match mode {
                    WriteMode::Normal { .. } => WriteMode::Normal {
                        scopes: BUTTON_REPLAY_WRITES,
                    },
                    _ => mode,
                };
                let repair = write_request(
                    usb,
                    spec,
                    hs.read_spec(),
                    &hs.write_args(&block),
                    repair_mode,
                )
                .map_err(|e| WriteError::Refused(e.to_string()))?;
                Some((flag, Some((hs, block, repair))))
            }
            _ => {
                return Err(WriteError::Refused(format!(
                    "unknown button flag {flag:#04x}"
                )));
            }
        }
    } else if normal_secondary {
        Some((1, None))
    } else {
        None
    };
    *wrote = true;
    let (trace, result) = exchange(io, usb, &request);
    traces.push(trace);
    let mut notes = Vec::new();
    if let Err(e) = result {
        notes.push(e.to_string());
    }
    if let Some((primary, block)) = secondary_primary {
        let (flag, after) =
            read_button(io, usb, primary, traces).map_err(WriteError::Preservation)?;
        if flag != 1 || after != block {
            return Err(WriteError::Preservation(
                "secondary write changed the primary binding or flag".into(),
            ));
        }
    }
    if let Some((flag, repair)) = saved {
        if let Some((hs, block, request)) = repair {
            // Attempt preservation even if the primary write reported an error.
            let (trace, result) = exchange(io, usb, &request);
            traces.push(trace);
            if let Err(e) = result {
                notes.push(format!("HyperShift replay: {e}"));
            }
            let (after_flag, after_block) =
                read_button(io, usb, hs, traces).map_err(WriteError::Preservation)?;
            if after_flag != flag || after_block != block {
                return Err(WriteError::Preservation(
                    "HyperShift block or flag differs".into(),
                ));
            }
        }
        let (after_flag, _) =
            read_button(io, usb, param, traces).map_err(WriteError::Preservation)?;
        if after_flag != flag {
            return Err(WriteError::Preservation("layer-0 flag differs".into()));
        }
    }
    if notes.is_empty() {
        Ok(())
    } else {
        Err(WriteError::Reported(notes.join("; ")))
    }
}

fn read_button(
    io: &mut impl FeatureIo,
    usb: &UsbId,
    param: Param,
    traces: &mut Vec<Trace>,
) -> Result<(u8, Vec<u8>), String> {
    let request =
        read_request(usb, param.read_spec(), &param.read_args()).map_err(|e| e.to_string())?;
    let (trace, result) = exchange(io, usb, &request);
    traces.push(trace);
    let response = result.map_err(|e| e.to_string())?;
    Ok((response.args[2], param.extract(&response.args)?))
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

    fn write(&mut self, value: &[u8], traces: &mut Vec<Trace>) -> Result<(), WriteError> {
        write_value(
            self.io,
            self.usb,
            self.param,
            value,
            self.mode,
            traces,
            &mut self.wrote,
        )
    }

    /// Writes `value`, reads it back and records the step. The read-back
    /// decides the outcome: the Naga Pro can report a failed write that
    /// was applied, so a write error with a matching read-back passes (and
    /// is noted), and an acknowledged write with a different read-back fails.
    fn write_step(&mut self, name: &'static str, value: &[u8]) -> Result<bool, String> {
        let mut traces = Vec::new();
        let expected = self.param.render(value);
        let written = self.write(value, &mut traces);
        if let Err(WriteError::Refused(e)) = written {
            self.note(
                name,
                false,
                format!("write refused, nothing sent: {e}"),
                traces,
            );
            return Err(e);
        }
        let read = self.read(&mut traces);
        let (ok, got, note) = match (&written, &read) {
            (Err(WriteError::Preservation(e)), got) => (
                false,
                got.as_ref().ok().map(|v| self.param.render(v)),
                format!("button preservation failed: {e}"),
            ),
            (_, Err(r)) => (false, None, format!("read-back failed: {r}")),
            (Ok(()), Ok(v)) if v == value => (true, Some(self.param.render(v)), String::new()),
            (Err(w), Ok(v)) if v == value => (
                true,
                Some(self.param.render(v)),
                format!("device reported {w}, but the read-back shows the value was applied"),
            ),
            (Ok(()), Ok(v)) => (
                false,
                Some(self.param.render(v)),
                "write acknowledged, read-back differs".into(),
            ),
            (Err(w), Ok(v)) => (
                false,
                Some(self.param.render(v)),
                format!("device reported {w}, read-back differs"),
            ),
        };
        self.steps.push(Step {
            param: self.param,
            name,
            expected: Some(expected),
            got,
            ok,
            note,
            traces,
        });
        Ok(ok)
    }

    fn note(&mut self, name: &'static str, ok: bool, note: String, traces: Vec<Trace>) {
        self.steps.push(Step {
            param: self.param,
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
        wrote: false,
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
    let Some((original, probe)) = start(r, probe, baseline) else {
        return false;
    };
    run_writes(r, &original, &probe, baseline)
}

/// Step 1 and the original value: `None` if writing must not start.
fn start<IO: FeatureIo>(
    r: &mut Runner<'_, IO>,
    probe: &str,
    baseline: &BTreeMap<String, String>,
) -> Option<(Vec<u8>, Vec<u8>)> {
    if !baseline_matches(r, baseline) {
        return None;
    }
    let mut traces = Vec::new();
    let original = match r.read(&mut traces) {
        Ok(v) => v,
        Err(e) => {
            r.note("read original", false, e, traces);
            return None;
        }
    };
    let probe = match r.param.probe(probe, &original) {
        Ok(p) => p,
        Err(e) => {
            r.note("read original", false, format!("bad probe: {e}"), traces);
            return None;
        }
    };
    let rendered = r.param.render(&original);
    r.note("read original", true, rendered, traces);
    Some((original, probe))
}

/// A probe written and left in place by [`hold`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub param: Param,
    pub original: Vec<u8>,
    pub probe: Vec<u8>,
}

/// Persistence check, phase 1, for one or more parameters: the baseline is
/// checked once, then for each parameter the same value and the probe are
/// written (each confirmed by a read-back) and the probe is left in place,
/// so that a single reconnect covers all of them. On any failure every probe
/// written so far is restored and the whole device is compared with the
/// baseline. Returns the report and, on success, what [`finish_hold`] needs.
pub fn hold(
    io: &mut impl FeatureIo,
    usb: &UsbId,
    items: &[(Param, &str)],
    baseline: &BTreeMap<String, String>,
    mode: WriteMode,
) -> (Report, Option<Vec<Held>>) {
    let Some(&(first, _)) = items.first() else {
        let report = Report {
            param: Param::PollRate,
            steps: Vec::new(),
            passed: false,
        };
        return (report, None);
    };
    let mut r = Runner {
        io,
        usb,
        param: first,
        mode,
        steps: Vec::new(),
        wrote: false,
    };
    let mut held: Vec<Held> = Vec::new();
    let mut ok = baseline_matches(&mut r, baseline);
    for &(param, probe) in items {
        if !ok {
            break;
        }
        r.param = param;
        let mut traces = Vec::new();
        let original = match r.read(&mut traces) {
            Ok(v) => v,
            Err(e) => {
                r.note("read original", false, e, traces);
                ok = false;
                break;
            }
        };
        let probe = match param.probe(probe, &original) {
            Ok(p) => p,
            Err(e) => {
                r.note("read original", false, format!("bad probe: {e}"), traces);
                ok = false;
                break;
            }
        };
        let rendered = param.render(&original);
        r.note("read original", true, rendered, traces);
        // Recorded before writing, so a failed probe is restored as well.
        held.push(Held {
            param,
            original: original.clone(),
            probe: probe.clone(),
        });
        ok = matches!(r.write_step("write same value", &original), Ok(true))
            && matches!(r.write_step("write probe (held)", &probe), Ok(true));
    }
    if !ok {
        if r.wrote {
            for h in held.iter().rev() {
                r.param = h.param;
                let _ = r.write_step("restore original", &h.original);
            }
            final_snapshot(&mut r, baseline);
        }
        let report = Report {
            param: first,
            steps: r.steps,
            passed: false,
        };
        return (report, None);
    }
    let report = Report {
        param: first,
        steps: r.steps,
        passed: true,
    };
    (report, Some(held))
}

/// Persistence check, phase 2 (after a reconnect): first reads every held
/// parameter, before any write; each must still hold its probe, otherwise
/// the check fails (see [`persisted`]). Then restores every original and
/// compares the whole device with the baseline taken before phase 1. A
/// clean rollback never turns a lost value into a pass.
pub fn finish_hold(
    io: &mut impl FeatureIo,
    usb: &UsbId,
    held: &[Held],
    baseline: &BTreeMap<String, String>,
    mode: WriteMode,
) -> Report {
    let Some(first) = held.first().map(|h| h.param) else {
        return Report {
            param: Param::PollRate,
            steps: Vec::new(),
            passed: false,
        };
    };
    let mut r = Runner {
        io,
        usb,
        param: first,
        mode,
        steps: Vec::new(),
        wrote: false,
    };
    let mut passed = true;
    // Read everything first: no write may touch the device before the
    // persistence verdict is recorded.
    for h in held {
        r.param = h.param;
        let mut traces = Vec::new();
        match r.read(&mut traces) {
            Ok(now) => {
                let survived = now == h.probe;
                let verdict = if survived {
                    "the probe survived the reconnect"
                } else if now == h.original {
                    "NOT PERSISTED: the device reverted to the original value"
                } else {
                    "NOT PERSISTED: the device holds a third value"
                };
                let note = format!("after reconnect: {} — {verdict}", h.param.render(&now));
                r.note(READ_AFTER_RECONNECT, survived, note, traces);
                passed &= survived;
            }
            Err(e) => {
                r.note(READ_AFTER_RECONNECT, false, e, traces);
                passed = false;
            }
        }
    }
    for h in held {
        r.param = h.param;
        passed &= matches!(r.write_step("restore original", &h.original), Ok(true));
    }
    passed &= final_snapshot(&mut r, baseline);
    Report {
        param: first,
        steps: r.steps,
        passed,
    }
}

const READ_AFTER_RECONNECT: &str = "read after reconnect";

/// Whether every value read after the reconnect still held its probe
/// (`None` if `report` is not from [`finish_hold`]).
pub fn persisted(report: &Report) -> Option<bool> {
    let mut reads = report
        .steps
        .iter()
        .filter(|s| s.name == READ_AFTER_RECONNECT)
        .peekable();
    reads.peek()?;
    Some(reads.all(|s| s.ok))
}

/// Refuses a baseline with read errors or one the device no longer matches.
fn baseline_matches<IO: FeatureIo>(
    r: &mut Runner<'_, IO>,
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

    true
}

/// Steps 2–4 and the final snapshot.
fn run_writes<IO: FeatureIo>(
    r: &mut Runner<'_, IO>,
    original: &[u8],
    probe: &[u8],
    baseline: &BTreeMap<String, String>,
) -> bool {
    // Steps 2–4. On any failure, restore and stop.
    let mut ok = true;
    for (name, value) in [
        ("write same value", original),
        ("write probe", probe),
        ("restore original", original),
    ] {
        match r.write_step(name, value) {
            Ok(true) => {}
            Ok(false) => {
                ok = false;
                if name != "restore original" {
                    // The outcome of the emergency restore is recorded as
                    // its own step; the run has already failed.
                    let _ = r.write_step("restore original", original);
                }
                break;
            }
            Err(_) => {
                ok = false;
                break;
            }
        }
    }

    // Whatever happened, if anything was written, the whole device is read
    // again and compared with the baseline to expose side effects.
    if r.wrote {
        let clean = final_snapshot(r, baseline);
        ok && clean
    } else {
        false
    }
}

fn final_snapshot<IO: FeatureIo>(
    r: &mut Runner<'_, IO>,
    baseline: &BTreeMap<String, String>,
) -> bool {
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
    use super::*;
    use crate::naga_pro::sim::{Sim, USB};

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
                "restore original",
                "final snapshot"
            ]
        );
        // The DPI never changed, so the final snapshot itself is clean.
        assert!(report.steps.last().unwrap().ok);
    }

    #[test]
    fn nacked_but_applied_writes_pass_on_read_back() {
        let mut sim = Sim::new();
        let base = baseline(&mut sim);
        sim.nack_applied_writes = true;
        let report = validate(
            &mut sim,
            &USB,
            Param::Dpi { storage: 0 },
            "800",
            &base,
            FIRST,
        );
        assert!(report.passed, "{:#?}", report.steps);
        let probe = report
            .steps
            .iter()
            .find(|s| s.name == "write probe")
            .unwrap();
        assert!(probe.note.contains("Failed"), "{}", probe.note);
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
    fn hold_then_finish_restores_and_checks_the_whole_device() {
        let mut sim = Sim::new();
        let base = baseline(&mut sim);
        let dpi = Param::Dpi { storage: 1 };
        let logo = Param::Brightness {
            storage: 1,
            led: LED_LOGO,
        };
        let (report, held) = hold(&mut sim, &USB, &[(dpi, "800"), (logo, "100")], &base, FIRST);
        assert!(report.passed, "{:#?}", report.steps);
        let held = held.unwrap();
        assert_eq!(held.len(), 2);
        assert_eq!(sim.dpi[&1], [0x03, 0x20, 0x03, 0x20]);
        assert_eq!(sim.brightness[&(1, LED_LOGO)], 100);

        // A reconnect that keeps the profile values.
        let report = finish_hold(&mut sim, &USB, &held, &base, FIRST);
        assert!(report.passed, "{:#?}", report.steps);
        let reads: Vec<&str> = report
            .steps
            .iter()
            .filter(|s| s.name == "read after reconnect")
            .map(|s| s.note.as_str())
            .collect();
        assert_eq!(reads.len(), 2);
        assert!(reads.iter().all(|n| n.contains("survived")));
        assert_eq!(report.steps.last().unwrap().name, "final snapshot");
        assert_eq!(persisted(&report), Some(true));
        assert_eq!(compare(&base, &baseline(&mut sim)), vec![]);
    }

    #[test]
    fn lost_probe_fails_finish_even_after_a_clean_rollback() {
        let mut sim = Sim::new();
        let base = baseline(&mut sim);
        let dpi = Param::Dpi { storage: 1 };
        let logo = Param::Brightness {
            storage: 1,
            led: LED_LOGO,
        };
        let (_, held) = hold(&mut sim, &USB, &[(dpi, "800"), (logo, "100")], &base, FIRST);
        let held = held.unwrap();
        // The power cycle loses the brightness probe only.
        sim.brightness.insert((1, LED_LOGO), 51);

        let report = finish_hold(&mut sim, &USB, &held, &base, FIRST);
        assert!(!report.passed);
        assert_eq!(persisted(&report), Some(false));
        // All reads come before the first write.
        let names: Vec<&str> = report.steps.iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            [
                "read after reconnect",
                "read after reconnect",
                "restore original",
                "restore original",
                "final snapshot"
            ]
        );
        // The rollback itself succeeded and the device matches the baseline.
        assert!(
            report.steps[2..].iter().all(|s| s.ok),
            "{:#?}",
            report.steps
        );
        assert_eq!(compare(&base, &baseline(&mut sim)), vec![]);
    }

    #[test]
    fn failed_hold_restores_every_probe_already_written() {
        let mut sim = Sim::new();
        let base = baseline(&mut sim);
        let dpi = Param::Dpi { storage: 1 };
        let logo = Param::Brightness {
            storage: 1,
            led: LED_LOGO,
        };
        // The second probe is invalid; the first one must be rolled back.
        let (report, held) = hold(&mut sim, &USB, &[(dpi, "800"), (logo, "51")], &base, FIRST);
        assert!(!report.passed);
        assert!(held.is_none());
        assert_eq!(sim.dpi[&1], [0x06, 0x40, 0x06, 0x40]);
        assert_eq!(report.steps.last().unwrap().name, "final snapshot");
        assert!(report.steps.last().unwrap().ok);
    }

    #[test]
    fn source_only_write_needs_owner_permission() {
        let mut sim = Sim::new();
        let base = baseline(&mut sim);
        let report = validate(&mut sim, &USB, Param::PollRate, "500", &base, FIRST);
        assert!(!report.passed);
        assert!(report.steps.iter().any(|s| s.note.contains("SourceOnly")));
        assert_eq!(sim.poll, 0x01);
        // Nothing was sent, so no final snapshot is needed.
        assert!(report.steps.iter().all(|s| s.name != "final snapshot"));
    }

    #[test]
    fn effect_validation_writes_static_and_restores_spectrum() {
        let mut sim = Sim::new();
        let base = baseline(&mut sim);
        let logo = Param::Effect {
            storage: 0,
            led: LED_LOGO,
        };
        let allowed = FIRST;
        let report = validate(&mut sim, &USB, logo, "ff00ff", &base, allowed);
        assert!(report.passed, "{:#?}", report.steps);
        let probe = report
            .steps
            .iter()
            .find(|s| s.name == "write probe")
            .unwrap();
        assert_eq!(probe.got.as_deref(), Some("static #ff00ff"));
        // Static is sent with size 09, spectrum with size 06.
        assert_eq!(probe.traces[0].request[5], 0x09);
        let same = report
            .steps
            .iter()
            .find(|s| s.name == "write same value")
            .unwrap();
        assert_eq!(same.traces[0].request[5], 0x06);
        assert_eq!(sim.effect[&(0, LED_LOGO)], [3, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn effect_scopes_tell_static_and_spectrum_apart() {
        use crate::{Link, RequestError, write_request};
        let static_ff = [1, 0, 0, 1, 0xff, 0, 0xff, 0, 0, 0];
        let spectrum = [3, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let p1 = Param::Effect {
            storage: 1,
            led: LED_LOGO,
        };
        let live = Param::Effect {
            storage: 0,
            led: LED_LOGO,
        };
        assert!(p1.verified_for(Link::Wired, &static_ff));
        assert!(!p1.verified_for(Link::Wired, &spectrum));
        assert!(live.verified_for(Link::Wired, &spectrum));
        assert!(live.verified_for(Link::Wired, &static_ff));
        assert!(!live.verified_for(Link::Receiver, &static_ff));
        assert!(!p1.verified_on(Link::Receiver));
        assert!(p1.verified_on(Link::Wired));

        // The gate itself refuses spectrum into a profile in normal mode.
        let normal = WriteMode::Normal {
            scopes: VERIFIED_WRITES,
        };
        let gate = |param: Param, value: &[u8], usb: &UsbId| {
            write_request(
                usb,
                param.write_spec_for(value).unwrap(),
                param.read_spec(),
                &param.write_args(value),
                normal,
            )
        };
        assert!(gate(p1, &static_ff, &USB).is_ok());
        assert!(gate(live, &spectrum, &USB).is_ok());
        assert!(matches!(
            gate(p1, &spectrum, &USB),
            Err(RequestError::WriteRefused { .. })
        ));
        let receiver = UsbId {
            product_id: 0x0090,
            link: Link::Receiver,
            ..USB
        };
        assert!(matches!(
            gate(live, &static_ff, &receiver),
            Err(RequestError::WriteRefused { .. })
        ));
    }

    #[test]
    fn unknown_effects_are_not_written() {
        let mut sim = Sim::new();
        sim.effect
            .insert((0, LED_LOGO), [2, 1, 0, 1, 9, 9, 9, 0, 0, 0]);
        let base = baseline(&mut sim);
        let logo = Param::Effect {
            storage: 0,
            led: LED_LOGO,
        };
        let allowed = WriteMode::FirstValidation {
            allow_source_only: true,
        };
        let report = validate(&mut sim, &USB, logo, "ff00ff", &base, allowed);
        assert!(!report.passed);
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.note.contains("cannot be written"))
        );
        assert_eq!(sim.effect[&(0, LED_LOGO)][0], 2);
    }

    #[test]
    fn button_validation_writes_a_block_and_restores_it() {
        let mut sim = Sim::new();
        let base = baseline(&mut sim);
        let side1 = Param::button(0, 80).unwrap();
        let report = validate(&mut sim, &USB, side1, "key:ctrl+f5", &base, FIRST);
        assert!(report.passed, "{:#?}", report.steps);
        let probe = report
            .steps
            .iter()
            .find(|s| s.name == "write probe")
            .unwrap();
        assert_eq!(probe.got.as_deref(), Some("key:ctrl+f5"));
        assert_eq!(
            &probe
                .traces
                .iter()
                .find(|t| t.request[7] == 0x0c)
                .unwrap()
                .request[8..18],
            &[0, 80, 0, 0x02, 0x02, 0x01, 0x3E, 0, 0, 0]
        );
        assert_eq!(sim.buttons[&(0, 80)].1, [0x02, 0x02, 0, 0x1E, 0, 0, 0]);
        // Not verified yet: normal mode refuses.
        assert!(!side1.verified_on(crate::Link::Wired));
        assert!(Param::button(1, 6).is_err());
        assert!(Param::button(1, 13).is_err());
    }

    #[test]
    fn hypershift_changes_fail_validation_hold_and_finish() {
        // A button write that also changes the HyperShift layer fails the
        // final snapshot even though the layer-0 read-back matches.
        let mut sim = Sim::new();
        sim.button_write_hits_hypershift = true;
        let base = baseline(&mut sim);
        assert!(base.contains_key("p1.button.80.hs"));
        assert!(base.contains_key("live.button.80.hs"));
        assert!(!base.contains_key("p2.button.80.hs"));
        let side1 = Param::button(0, 80).unwrap();
        let report = validate(&mut sim, &USB, side1, "key:f5", &base, FIRST);
        assert!(!report.passed, "{:#?}", report.steps);
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.name == "final snapshot" && !s.ok)
        );

        // hold refuses a baseline whose HyperShift values differ.
        let mut sim = Sim::new();
        let base = baseline(&mut sim);
        sim.hypershift.insert((1, 64), [0; 7]);
        let p1 = Param::button(1, 80).unwrap();
        let (report, held) = hold(&mut sim, &USB, &[(p1, "key:f5")], &base, FIRST);
        assert!(!report.passed && held.is_none());

        // finish fails when a HyperShift value changed across the reconnect,
        // although every layer-0 probe survived and was restored.
        let mut sim = Sim::new();
        let base = baseline(&mut sim);
        let (report, held) = hold(&mut sim, &USB, &[(p1, "key:f5")], &base, FIRST);
        assert!(report.passed, "{:#?}", report.steps);
        sim.hypershift.insert((1, 80), [0; 7]);
        let report = finish_hold(&mut sim, &USB, &held.unwrap(), &base, FIRST);
        assert_eq!(persisted(&report), Some(true));
        assert!(!report.passed, "{:#?}", report.steps);
    }

    #[test]
    fn stored_flag_and_hypershift_survive_validation() {
        for storage in [0, 1] {
            let mut sim = Sim::new();
            sim.buttons.get_mut(&(storage, 64)).unwrap().0 = 1;
            let base = baseline(&mut sim);
            let p = Param::button(storage, 64).unwrap();
            let report = validate(&mut sim, &USB, p, "key:q", &base, FIRST);
            assert!(report.passed, "{:#?}", report.steps);
            assert_eq!(sim.buttons[&(storage, 64)].0, 1);
            assert_eq!(baseline(&mut sim), base);
        }
        // HyperShift-layer writes are never allowed in normal mode.
        let hs = Param::button_layer(1, 64, 1).unwrap();
        assert_eq!(hs.dump_key(), "p1.button.64.hs");
        assert!(!hs.verified_on(crate::Link::Wired));
        assert!(Param::button_layer(1, 64, 2).is_err());
    }

    #[test]
    fn all_button_flags_survive_hold_and_finish_over_the_other_link() {
        for storage in [0, 1] {
            let mut sim = Sim::new();
            for (i, slot) in BUTTON_SLOTS.iter().enumerate() {
                sim.buttons.get_mut(&(storage, slot.id)).unwrap().0 = (i % 2) as u8;
            }
            let base = baseline(&mut sim);
            let hs = sim.hypershift.clone();
            let items: Vec<_> = BUTTON_SLOTS
                .iter()
                .map(|slot| (Param::button(storage, slot.id).unwrap(), "key:q"))
                .collect();
            let (report, held) = hold(&mut sim, &USB, &items, &base, FIRST);
            assert!(report.passed, "{:#?}", report.steps);
            for (i, slot) in BUTTON_SLOTS.iter().enumerate() {
                assert_eq!(sim.buttons[&(storage, slot.id)].0, (i % 2) as u8);
            }
            assert_eq!(sim.hypershift, hs);
            let receiver = UsbId {
                product_id: 0x0090,
                link: crate::Link::Receiver,
                ..USB
            };
            let report = finish_hold(&mut sim, &receiver, &held.unwrap(), &base, FIRST);
            assert!(report.passed, "{:#?}", report.steps);
            assert_eq!(baseline(&mut sim), base);
        }
    }

    #[test]
    fn applied_button_writes_with_error_status_preserve_the_flag() {
        let mut sim = Sim::new();
        sim.buttons.get_mut(&(1, 64)).unwrap().0 = 1;
        sim.nack_applied_writes = true;
        let base = baseline(&mut sim);
        let p = Param::button(1, 64).unwrap();
        let report = validate(&mut sim, &USB, p, "key:q", &base, FIRST);
        assert!(report.passed, "{:#?}", report.steps);
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.note.contains("HyperShift replay"))
        );
        assert_eq!(baseline(&mut sim), base);
    }

    #[test]
    fn ignored_flag_replay_fails_even_when_the_binding_matches() {
        let mut sim = Sim::new();
        sim.buttons.get_mut(&(1, 64)).unwrap().0 = 1;
        sim.ignore_hypershift_writes = true;
        let base = baseline(&mut sim);
        let p = Param::button(1, 64).unwrap();
        let report = validate(&mut sim, &USB, p, "key:q", &base, FIRST);
        assert!(!report.passed);
        assert!(
            report
                .steps
                .iter()
                .any(|s| !s.ok && s.note.contains("preservation failed"))
        );
    }

    #[test]
    fn normal_button_gate_checks_the_full_payload() {
        use crate::Link::{Receiver, Wired};
        let scope = WriteMode::Normal {
            scopes: VERIFIED_WRITES,
        };
        for link in [Wired, Receiver] {
            let usb = UsbId { link, ..USB };
            for storage in 0..=5 {
                for slot in BUTTON_SLOTS {
                    for layer in [0, 1] {
                        let p = Param::button_layer(storage, slot.id, layer).unwrap();
                        let value = Binding::parse("key:f8").unwrap().encode();
                        let allowed = storage == 1
                            && ![9, 10].contains(&slot.id)
                            && (layer == 0 || (link == Receiver && slot.id == 2));
                        assert_eq!(p.verified_for(link, &value), allowed, "{p} {link:?}");
                        assert_eq!(
                            write_request(
                                &usb,
                                &SET_BUTTON_FUNCTION,
                                &BUTTON_FUNCTION,
                                &p.write_args(&value),
                                scope
                            )
                            .is_ok(),
                            allowed
                        );
                    }
                }
            }
        }
        let cases = [
            (3, 0, "hypershift", true),
            (4, 0, "hypershift", false),
            (3, 0, "key:ctrl+shift+f8", true),
            (3, 0, "key:alt+f8", false),
            (3, 0, "profile-cycle", true),
            (5, 0, "profile-cycle", false),
            (52, 0, "key:f9 turbo=100", true),
            (52, 0, "key:f9 turbo=99", false),
            (53, 0, "dpi:cycle", true),
            (53, 0, "dpi:0xff", false),
            (2, 1, "key:f8", true),
            (2, 1, "key:f9", false),
            (2, 1, "mouse:right", true),
            (2, 1, "hypershift", false),
            (3, 0, "disabled", false),
            (3, 0, "raw:ffffffffffffff", false),
            (3, 0, "raw:0201001e000000", false),
            (3, 0, "mouse:0xff", false),
        ];
        for (slot, layer, text, allowed) in cases {
            let p = Param::button_layer(1, slot, layer).unwrap();
            let value = Binding::parse(text).unwrap().encode();
            assert_eq!(p.verified_for(Receiver, &value), allowed, "{text}");
            let usb = UsbId {
                link: Receiver,
                ..USB
            };
            assert_eq!(
                write_request(
                    &usb,
                    &SET_BUTTON_FUNCTION,
                    &BUTTON_FUNCTION,
                    &p.write_args(&value),
                    scope
                )
                .is_ok(),
                allowed,
                "wire gate: {text}"
            );
        }
        assert!(
            !Param::button(1, 3)
                .unwrap()
                .verified_for(Wired, &Binding::Hypershift.encode())
        );
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
        let effect = Param::Effect {
            storage: 1,
            led: LED_LOGO,
        };
        let spectrum = [3, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(
            effect.probe("#FF00FF", &spectrum).unwrap(),
            [1, 0, 0, 1, 0xff, 0, 0xff, 0, 0, 0]
        );
        assert!(effect.probe("spectrum", &spectrum).is_err());
        assert!(effect.probe("ff00f", &spectrum).is_err());
        assert_eq!(effect.write_args(&spectrum), [1, 4, 3, 0, 0, 0]);
    }
}
