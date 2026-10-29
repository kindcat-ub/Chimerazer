//! Razer Naga Pro (RZ01-0342): wired `1532:008F`, 2.4 GHz receiver `1532:0090`.
//!
//! Facts are ported from OpenSnek (Apache-2.0) and cross-checked against
//! OpenRazer; see `docs/protocol/naga-pro.md` for the provenance of every
//! command.

pub mod state;

use std::fmt;
use std::time::Duration;

use chimerazer_protocol::{ARGS_LEN, Command};

use crate::{CommandSpec, Confirmation, Link, Model, UsbId};

pub const MODEL: Model = Model {
    name: "Razer Naga Pro",
    usb_ids: &USB_IDS,
    commands: COMMANDS,
};

/// Both product IDs expose the control endpoint on interface 0 and answer
/// transaction ID `0x1F`. Response delays are OpenRazer's per-PID waits.
pub const USB_IDS: [UsbId; 2] = [
    UsbId {
        product_id: 0x008F,
        link: Link::Wired,
        interface: 0,
        transaction_id: 0x1F,
        response_delay: Duration::from_micros(600),
    },
    UsbId {
        product_id: 0x0090,
        link: Link::Receiver,
        interface: 0,
        transaction_id: 0x1F,
        response_delay: Duration::from_millis(31),
    },
];

pub fn usb_id(product_id: u16) -> Option<&'static UsbId> {
    USB_IDS.iter().find(|id| id.product_id == product_id)
}

pub const MAX_DPI: u16 = 20_000;

/// Storage ID of the live (effective) layer.
pub const LIVE_STORAGE: u8 = 0;

/// Onboard profile IDs.
pub const PROFILES: std::ops::RangeInclusive<u8> = 1..=5;

pub const LED_SCROLL_WHEEL: u8 = 0x01;
pub const LED_LOGO: u8 = 0x04;

/// Which physical part a button slot belongs to. The three side panels are
/// swappable; the firmware keeps one static slot table for all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotGroup {
    Body,
    TwoButtonPanel,
    SixButtonPanel,
    TwelveButtonPanel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonSlot {
    pub id: u8,
    pub name: &'static str,
    pub group: SlotGroup,
}

const fn slot(id: u8, name: &'static str, group: SlotGroup) -> ButtonSlot {
    ButtonSlot { id, name, group }
}

/// All 30 mappable slots, matching the device's own `02:84` inventory.
pub const BUTTON_SLOTS: [ButtonSlot; 30] = {
    use SlotGroup::*;
    [
        slot(1, "Left Click", Body),
        slot(2, "Right Click", Body),
        slot(3, "Middle Click", Body),
        slot(9, "Scroll Up", Body),
        slot(10, "Scroll Down", Body),
        slot(11, "DPI Down", Body),
        slot(12, "DPI Up", Body),
        slot(14, "Profile Button", Body),
        slot(52, "Wheel Tilt Left", Body),
        slot(53, "Wheel Tilt Right", Body),
        // Reversed relative to panel labels: label 1 is slot 5, label 2 is slot 4.
        slot(5, "Panel Button 1", TwoButtonPanel),
        slot(4, "Panel Button 2", TwoButtonPanel),
        slot(80, "Side Button 1", SixButtonPanel),
        slot(81, "Side Button 2", SixButtonPanel),
        slot(82, "Side Button 3", SixButtonPanel),
        slot(83, "Side Button 4", SixButtonPanel),
        slot(84, "Side Button 5", SixButtonPanel),
        slot(85, "Side Button 6", SixButtonPanel),
        slot(64, "Side Button 1", TwelveButtonPanel),
        slot(65, "Side Button 2", TwelveButtonPanel),
        slot(66, "Side Button 3", TwelveButtonPanel),
        slot(67, "Side Button 4", TwelveButtonPanel),
        slot(68, "Side Button 5", TwelveButtonPanel),
        slot(69, "Side Button 6", TwelveButtonPanel),
        slot(70, "Side Button 7", TwelveButtonPanel),
        slot(71, "Side Button 8", TwelveButtonPanel),
        slot(72, "Side Button 9", TwelveButtonPanel),
        slot(73, "Side Button 10", TwelveButtonPanel),
        slot(74, "Side Button 11", TwelveButtonPanel),
        slot(75, "Side Button 12", TwelveButtonPanel),
    ]
};

pub const FIRMWARE_VERSION: CommandSpec = CommandSpec {
    command: Command::new(0x00, 0x81, 0x02),
    name: "firmware version",
    confirmation: Confirmation::OurHardware,
    sources: &[
        "chimerazer-m1-reads",
        "opensnek-usb-protocol",
        "openrazer-common",
    ],
};

pub const SERIAL: CommandSpec = CommandSpec {
    command: Command::new(0x00, 0x82, 0x16),
    name: "serial number",
    confirmation: Confirmation::OurHardware,
    sources: &[
        "chimerazer-m1-reads",
        "opensnek-usb-protocol",
        "openrazer-common",
    ],
};

pub const DEVICE_MODE: CommandSpec = CommandSpec {
    command: Command::new(0x00, 0x84, 0x02),
    name: "device mode",
    confirmation: Confirmation::OurHardware,
    sources: &[
        "chimerazer-m1-reads",
        "opensnek-usb-protocol",
        "openrazer-common",
    ],
};

pub const POLL_RATE: CommandSpec = CommandSpec {
    command: Command::new(0x00, 0x85, 0x01),
    name: "poll rate",
    confirmation: Confirmation::OurHardware,
    sources: &[
        "chimerazer-m1-reads",
        "opensnek-usb-protocol",
        "openrazer-mouse",
    ],
};

pub const PROFILE_SUMMARY: CommandSpec = CommandSpec {
    command: Command::new(0x00, 0x87, 0x00),
    name: "onboard profile summary",
    confirmation: Confirmation::OurHardware,
    sources: &["chimerazer-m1-reads", "opensnek-naga-default-banks"],
};

pub const BUTTON_INVENTORY: CommandSpec = CommandSpec {
    command: Command::new(0x02, 0x84, 0x05),
    name: "mappable button inventory",
    confirmation: Confirmation::OurHardware,
    sources: &["chimerazer-m1-reads", "opensnek-naga-side-panels"],
};

pub const BUTTON_FUNCTION: CommandSpec = CommandSpec {
    command: Command::new(0x02, 0x8C, 0x0A),
    name: "button function",
    confirmation: Confirmation::OurHardware,
    sources: &[
        "chimerazer-m1-reads",
        "opensnek-naga-default-banks",
        "opensnek-naga-button-encoding",
    ],
};

pub const DPI: CommandSpec = CommandSpec {
    command: Command::new(0x04, 0x85, 0x07),
    name: "DPI",
    confirmation: Confirmation::OurHardware,
    sources: &[
        "chimerazer-m1-reads",
        "opensnek-naga-default-banks",
        "openrazer-mouse",
    ],
};

pub const DPI_STAGES: CommandSpec = CommandSpec {
    command: Command::new(0x04, 0x86, 0x26),
    name: "DPI stages",
    confirmation: Confirmation::OurHardware,
    sources: &[
        "chimerazer-m1-reads",
        "opensnek-naga-default-banks",
        "openrazer-mouse",
    ],
};

pub const PROFILE_COUNT: CommandSpec = CommandSpec {
    command: Command::new(0x05, 0x80, 0x00),
    name: "onboard profile count",
    confirmation: Confirmation::OurHardware,
    sources: &["chimerazer-m1-reads", "opensnek-naga-default-banks"],
};

pub const PROFILE_INVENTORY: CommandSpec = CommandSpec {
    command: Command::new(0x05, 0x81, 0x00),
    name: "onboard profile inventory",
    confirmation: Confirmation::OurHardware,
    sources: &["chimerazer-m1-reads", "opensnek-naga-default-banks"],
};

pub const ACTIVE_PROFILE: CommandSpec = CommandSpec {
    command: Command::new(0x05, 0x84, 0x00),
    name: "active onboard profile",
    confirmation: Confirmation::OurHardware,
    sources: &["chimerazer-m1-reads", "opensnek-naga-default-banks"],
};

pub const PROFILE_METADATA: CommandSpec = CommandSpec {
    command: Command::new(0x05, 0x88, 0x50),
    name: "onboard profile metadata chunk",
    confirmation: Confirmation::SourceOnly,
    sources: &["opensnek-crud-spec", "opensnek-naga-default-banks"],
};

pub const BATTERY_LEVEL: CommandSpec = CommandSpec {
    command: Command::new(0x07, 0x80, 0x02),
    name: "battery level",
    confirmation: Confirmation::OurHardware,
    sources: &[
        "chimerazer-m1-reads",
        "opensnek-usb-protocol",
        "openrazer-mouse",
    ],
};

pub const LOW_BATTERY_THRESHOLD: CommandSpec = CommandSpec {
    command: Command::new(0x07, 0x81, 0x01),
    name: "low battery threshold",
    confirmation: Confirmation::OurHardware,
    sources: &[
        "chimerazer-m1-reads",
        "opensnek-usb-protocol",
        "openrazer-mouse",
    ],
};

pub const IDLE_TIME: CommandSpec = CommandSpec {
    command: Command::new(0x07, 0x83, 0x02),
    name: "idle time",
    confirmation: Confirmation::OurHardware,
    sources: &[
        "chimerazer-m1-reads",
        "opensnek-usb-protocol",
        "openrazer-mouse",
    ],
};

pub const CHARGING: CommandSpec = CommandSpec {
    command: Command::new(0x07, 0x84, 0x02),
    name: "charging status",
    confirmation: Confirmation::OurHardware,
    sources: &[
        "chimerazer-m1-reads",
        "opensnek-usb-protocol",
        "openrazer-mouse",
    ],
};

pub const LED_EFFECT: CommandSpec = CommandSpec {
    command: Command::new(0x0F, 0x82, 0x0C),
    name: "LED effect state",
    confirmation: Confirmation::OurHardware,
    sources: &["chimerazer-m1-reads", "opensnek-crud-spec"],
};

pub const LED_BRIGHTNESS: CommandSpec = CommandSpec {
    command: Command::new(0x0F, 0x84, 0x03),
    name: "LED brightness",
    confirmation: Confirmation::OurHardware,
    sources: &[
        "chimerazer-m1-reads",
        "opensnek-naga-default-banks",
        "opensnek-device-support",
    ],
};

/// Every command this module can issue. Read-only until M2.
pub const COMMANDS: &[CommandSpec] = &[
    FIRMWARE_VERSION,
    SERIAL,
    DEVICE_MODE,
    POLL_RATE,
    PROFILE_SUMMARY,
    BUTTON_INVENTORY,
    BUTTON_FUNCTION,
    DPI,
    DPI_STAGES,
    PROFILE_COUNT,
    PROFILE_INVENTORY,
    ACTIVE_PROFILE,
    PROFILE_METADATA,
    BATTERY_LEVEL,
    LOW_BATTERY_THRESHOLD,
    IDLE_TIME,
    CHARGING,
    LED_EFFECT,
    LED_BRIGHTNESS,
];

/// A 7-byte button function block: `[class, len, data..]`.
pub type FunctionBlock = [u8; 7];

/// Decoded `02:8C` response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonFunction {
    pub storage: u8,
    pub slot: u8,
    /// Layer flag stored with the binding. On the Naga Pro this is not an
    /// echo of the requested hypershift layer; Synapse-written bindings carry 1.
    pub stored_layer: u8,
    pub block: FunctionBlock,
}

/// The response does not echo the requested storage and slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EchoMismatch {
    pub expected: (u8, u8),
    pub found: (u8, u8),
}

impl fmt::Display for EchoMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "button read echoed storage/slot {:?}, expected {:?}",
            self.found, self.expected
        )
    }
}

impl std::error::Error for EchoMismatch {}

/// Request arguments for `02:8C`.
pub fn button_function_args(storage: u8, slot: u8, hypershift: u8) -> [u8; 3] {
    [storage, slot, hypershift]
}

/// Decodes `02:8C` response arguments. The block always starts at
/// `args[3]`, whatever the stored layer flag is.
pub fn decode_button_function(
    args: &[u8; ARGS_LEN],
    storage: u8,
    slot: u8,
) -> Result<ButtonFunction, EchoMismatch> {
    if (args[0], args[1]) != (storage, slot) {
        return Err(EchoMismatch {
            expected: (storage, slot),
            found: (args[0], args[1]),
        });
    }
    Ok(ButtonFunction {
        storage,
        slot,
        stored_layer: args[2],
        block: args[3..10].try_into().expect("7-byte window"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read_request;

    #[test]
    fn product_ids_are_unique_and_lookup_works() {
        assert_ne!(USB_IDS[0].product_id, USB_IDS[1].product_id);
        assert_eq!(usb_id(0x0090).map(|u| u.link), Some(Link::Receiver));
        assert!(usb_id(0x0091).is_none());
    }

    #[test]
    fn button_slots_are_unique() {
        let mut ids: Vec<u8> = BUTTON_SLOTS.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), BUTTON_SLOTS.len());
    }

    #[test]
    fn catalog_is_read_only_and_unique() {
        let mut seen = Vec::new();
        for spec in COMMANDS {
            assert!(spec.command.is_read(), "{} is not a read", spec.command);
            assert!(!spec.sources.is_empty(), "{} has no source", spec.command);
            assert!(
                !seen.contains(&spec.command),
                "{} listed twice",
                spec.command
            );
            seen.push(spec.command);
        }
    }

    #[test]
    fn builds_requests_with_the_product_transaction_id() {
        for usb in &USB_IDS {
            let req = read_request(usb, &DPI, &[LIVE_STORAGE]).unwrap();
            let raw = req.encode();
            assert_eq!(&raw[..8], &[0x00, 0x1F, 0x00, 0x00, 0x00, 0x07, 0x04, 0x85]);
        }
    }

    #[test]
    fn button_read_rejects_foreign_echo() {
        let mut args = [0u8; ARGS_LEN];
        args[..3].copy_from_slice(&[1, 5, 0]);
        assert!(decode_button_function(&args, 1, 4).is_err());
        assert!(decode_button_function(&args, 0, 5).is_err());
        assert!(decode_button_function(&args, 1, 5).is_ok());
    }
}
