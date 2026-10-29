//! Raw frames captured from our Naga Pro (wired and receiver).

use std::collections::BTreeMap;
use std::path::Path;

use chimerazer_devices::{Confirmation, naga_pro, read_request};
use chimerazer_protocol::{
    ARGS_LEN, ButtonInventory, DpiStage, DpiStages, DpiXy, LedBrightness, ProfileInventory,
    REPORT_LEN, Response,
};

struct Frame {
    product_id: u16,
    command: String,
    request: Vec<u8>,
    response: Vec<u8>,
}

fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect()
}

fn frames() -> Vec<Frame> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/naga-pro/our-hardware-frames.txt");
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    lines
        .chunks(3)
        .map(|c| {
            let head: Vec<&str> = c[0].split_whitespace().collect();
            assert_eq!(head[0], "frame", "{c:?}");
            Frame {
                product_id: u16::from_str_radix(head[1], 16).unwrap(),
                command: head[2].to_string(),
                request: hex(c[1].strip_prefix("> ").unwrap()),
                response: hex(c[2].strip_prefix("< ").unwrap()),
            }
        })
        .collect()
}

/// Every captured exchange: our encoder reproduces the request byte for
/// byte, and the response passes length, CRC, status and echo checks.
#[test]
fn requests_reencode_and_responses_validate() {
    let frames = frames();
    assert_eq!(frames.len(), 98);
    for f in &frames {
        assert_eq!(f.request.len(), REPORT_LEN);
        let usb = naga_pro::usb_id(f.product_id).unwrap();
        let spec = naga_pro::COMMANDS
            .iter()
            .find(|s| s.command.to_string() == f.command)
            .unwrap();
        let request = read_request(usb, spec, &f.request[8..88]).unwrap();
        assert_eq!(
            request.encode().to_vec(),
            f.request,
            "{} request",
            f.command
        );
        let response = Response::parse(&f.response).unwrap();
        response
            .answer_to(&request)
            .unwrap_or_else(|e| panic!("{} on {:04x}: {e}", f.command, f.product_id));
    }
}

/// Normalized state: the response arguments are identical over the wire and
/// through the receiver, except battery level and charging, which depend on
/// the cable being plugged in.
#[test]
fn wired_and_receiver_state_match() {
    const LINK_DEPENDENT: [&str; 2] = ["07:80", "07:84"];
    let mut wired: BTreeMap<(String, Vec<u8>), [u8; ARGS_LEN]> = BTreeMap::new();
    let mut receiver = wired.clone();
    for f in frames() {
        if LINK_DEPENDENT.contains(&f.command.as_str()) {
            continue;
        }
        let args = Response::parse(&f.response).unwrap().args;
        let map = if f.product_id == 0x008F {
            &mut wired
        } else {
            &mut receiver
        };
        map.insert((f.command, f.request), args);
    }
    assert_eq!(wired.len(), 47);
    assert_eq!(wired, receiver);
}

#[test]
fn decoded_values_of_our_unit() {
    let state: BTreeMap<String, [u8; ARGS_LEN]> = frames()
        .into_iter()
        .filter(|f| f.product_id == 0x008F)
        .map(|f| {
            let key = format!("{} {:02x}", f.command, f.request[9]);
            (key, Response::parse(&f.response).unwrap().args)
        })
        .collect();

    assert_eq!(
        DpiXy::decode(&state["04:85 00"]).unwrap(),
        DpiXy {
            storage: 0,
            x: 1600,
            y: 1600
        }
    );
    let stages = DpiStages::decode(&state["04:86 00"]).unwrap();
    assert_eq!(stages.active_stage().map(|s| s.x), Some(1600));
    assert_eq!(
        stages.stages,
        [400, 800, 1600, 3200, 6400]
            .into_iter()
            .zip(1..)
            .map(|(dpi, id)| DpiStage { id, x: dpi, y: dpi })
            .collect::<Vec<_>>()
    );
    for (key, led) in [("0F:84 01", 0x01), ("0F:84 04", 0x04)] {
        assert_eq!(
            LedBrightness::decode(&state[key]).unwrap(),
            LedBrightness {
                storage: 0,
                led,
                value: 51
            }
        );
    }
    assert_eq!(
        ProfileInventory::decode(&state["05:81 00"]).unwrap(),
        ProfileInventory {
            max_profile: 5,
            assigned: vec![1, 2, 3, 4]
        }
    );
    assert_eq!(state["05:80 00"][0], 4);
    assert_eq!(state["05:84 00"][0], 1);
    assert_eq!(&state["00:81 00"][..2], &[0x02, 0x03]); // firmware 2.03
    assert_eq!(&state["00:84 00"][..2], &[0x00, 0x00]); // hardware mode
    assert_eq!(state["00:85 00"][0], 0x01); // 1000 Hz
    assert_eq!(&state["07:83 00"][..2], &[0x01, 0x2C]); // 300 s idle
    assert_eq!(state["07:84 00"][1], 0x01); // charging on the cable
    for key in ["0F:82 01", "0F:82 04"] {
        assert_eq!(state[key][2], 0x03, "{key}: effect");
    }
    let slots = ButtonInventory::decode(&state["02:84 00"]).unwrap().slots;
    let mut table: Vec<u8> = naga_pro::BUTTON_SLOTS.iter().map(|s| s.id).collect();
    let mut reported = slots.clone();
    table.sort_unstable();
    reported.sort_unstable();
    assert_eq!(reported, table);
}

/// Every button slot reads back with a well-formed echo on both links.
#[test]
fn all_button_slots_decode() {
    for link in [0x008F, 0x0090] {
        let mut slots: Vec<u8> = frames()
            .into_iter()
            .filter(|f| f.product_id == link && f.command == "02:8C")
            .map(|f| {
                let (storage, slot) = (f.request[8], f.request[9]);
                let args = Response::parse(&f.response).unwrap().args;
                naga_pro::decode_button_function(&args, storage, slot).unwrap();
                slot
            })
            .collect();
        let mut table: Vec<u8> = naga_pro::BUTTON_SLOTS.iter().map(|s| s.id).collect();
        slots.sort_unstable();
        table.sort_unstable();
        assert_eq!(slots, table, "{link:04x}");
    }
}

/// Commands with captured frames are marked as verified on our hardware.
#[test]
fn captured_commands_are_our_hardware() {
    for f in frames() {
        let spec = naga_pro::COMMANDS
            .iter()
            .find(|s| s.command.to_string() == f.command)
            .unwrap();
        assert_eq!(
            spec.confirmation,
            Confirmation::OurHardware,
            "{}",
            f.command
        );
        assert!(
            spec.sources.contains(&"chimerazer-m1-reads"),
            "{}",
            f.command
        );
    }
}
