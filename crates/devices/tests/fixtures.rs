//! Decodes Naga Pro payloads captured on real hardware by OpenSnek.

use std::collections::BTreeSet;
use std::path::Path;

use chimerazer_devices::naga_pro::{self, BUTTON_SLOTS, decode_button_function};
use chimerazer_protocol::{
    ARGS_LEN, ButtonInventory, DpiStage, DpiStages, DpiXy, LedBrightness, ProfileInventory,
};

#[derive(Debug)]
enum Entry {
    Args {
        command: String,
        args: [u8; ARGS_LEN],
    },
    Block {
        slot: u8,
        block: [u8; 7],
    },
}

fn hex(bytes: &[&str]) -> Vec<u8> {
    bytes
        .iter()
        .map(|b| u8::from_str_radix(b, 16).unwrap_or_else(|_| panic!("bad hex byte {b}")))
        .collect()
}

fn load(name: &str) -> Vec<Entry> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/naga-pro")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            match fields.as_slice() {
                ["args", command, bytes @ ..] => {
                    let bytes = hex(bytes);
                    assert!(bytes.len() <= ARGS_LEN, "{line}");
                    let mut args = [0; ARGS_LEN];
                    args[..bytes.len()].copy_from_slice(&bytes);
                    Entry::Args {
                        command: command.to_string(),
                        args,
                    }
                }
                ["block", slot, bytes @ ..] => Entry::Block {
                    slot: slot.parse().unwrap(),
                    block: hex(bytes).try_into().unwrap_or_else(|_| panic!("{line}")),
                },
                _ => panic!("unrecognised fixture line: {line}"),
            }
        })
        .collect()
}

fn args_for<'a>(entries: &'a [Entry], command: &str) -> Vec<&'a [u8; ARGS_LEN]> {
    entries
        .iter()
        .filter_map(|e| match e {
            Entry::Args { command: c, args } if c == command => Some(args),
            _ => None,
        })
        .collect()
}

fn captures() -> Vec<Entry> {
    load("opensnek-captures.txt")
}

#[test]
fn every_fixture_command_is_catalogued() {
    let catalogued: BTreeSet<String> = naga_pro::COMMANDS
        .iter()
        .map(|s| s.command.to_string())
        .collect();
    for entry in captures() {
        if let Entry::Args { command, .. } = entry {
            assert!(
                catalogued.contains(&command),
                "{command} is not in naga_pro::COMMANDS"
            );
        }
    }
}

#[test]
fn dpi_scalar() {
    let entries = captures();
    let [args] = args_for(&entries, "04:85")[..] else {
        panic!("expected one 04:85")
    };
    assert_eq!(
        DpiXy::decode(args).unwrap(),
        DpiXy {
            storage: 3,
            x: 1600,
            y: 1600
        }
    );
}

#[test]
fn dpi_stages() {
    let entries = captures();
    let [args] = args_for(&entries, "04:86")[..] else {
        panic!("expected one 04:86")
    };
    let table = DpiStages::decode(args).unwrap();
    assert_eq!((table.storage, table.active), (3, 3));
    let expected: Vec<DpiStage> = [400, 800, 1600, 3200, 6400]
        .into_iter()
        .zip(1..)
        .map(|(dpi, id)| DpiStage { id, x: dpi, y: dpi })
        .collect();
    assert_eq!(table.stages, expected);
    assert_eq!(table.active_stage().map(|s| s.x), Some(1600));
    assert!(table.stages.iter().all(|s| s.x <= naga_pro::MAX_DPI));
}

#[test]
fn led_brightness() {
    let entries = captures();
    let decoded: Vec<LedBrightness> = args_for(&entries, "0F:84")
        .into_iter()
        .map(|a| LedBrightness::decode(a).unwrap())
        .collect();
    assert_eq!(
        decoded,
        [
            LedBrightness {
                storage: 3,
                led: naga_pro::LED_SCROLL_WHEEL,
                value: 85
            },
            LedBrightness {
                storage: 3,
                led: naga_pro::LED_LOGO,
                value: 85
            },
        ]
    );
}

#[test]
fn profile_reads() {
    let entries = captures();
    let [inventory] = args_for(&entries, "05:81")[..] else {
        panic!("expected one 05:81")
    };
    let inventory = ProfileInventory::decode(inventory).unwrap();
    assert_eq!(inventory.max_profile, *naga_pro::PROFILES.end());
    assert_eq!(inventory.assigned, vec![1, 2]);

    let [count] = args_for(&entries, "05:80")[..] else {
        panic!("expected one 05:80")
    };
    assert_eq!(usize::from(count[0]), inventory.assigned.len());

    let [active] = args_for(&entries, "05:84")[..] else {
        panic!("expected one 05:84")
    };
    assert!(inventory.assigned.contains(&active[0]));
}

#[test]
fn button_inventory_matches_slot_table() {
    let entries = captures();
    let [args] = args_for(&entries, "02:84")[..] else {
        panic!("expected one 02:84")
    };
    let reported: BTreeSet<u8> = ButtonInventory::decode(args)
        .unwrap()
        .slots
        .into_iter()
        .collect();
    let table: BTreeSet<u8> = BUTTON_SLOTS.iter().map(|s| s.id).collect();
    assert_eq!(reported, table);
}

#[test]
fn default_bank_covers_every_slot() {
    let blocks: BTreeSet<u8> = captures()
        .into_iter()
        .filter_map(|e| match e {
            Entry::Block { slot, .. } => Some(slot),
            Entry::Args { .. } => None,
        })
        .collect();
    let table: BTreeSet<u8> = BUTTON_SLOTS.iter().map(|s| s.id).collect();
    assert_eq!(blocks, table);
}

#[test]
fn default_bank_blocks_are_well_formed() {
    for entry in captures() {
        if let Entry::Block { slot, block } = entry {
            // Native blocks declare length 1 even when the payload is longer
            // (keyboard keys sit in byte 3), so only the class is checked here.
            assert!(
                matches!(block[0], 0x01 | 0x02 | 0x06 | 0x07 | 0x0E),
                "slot {slot}: unexpected class {:#04x}",
                block[0]
            );
            assert_eq!(block[1], 0x01, "slot {slot}");
        }
    }
}

#[test]
fn synapse_button_reads_keep_block_at_args_3() {
    let entries = captures();
    let reads = args_for(&entries, "02:8C");
    assert_eq!(reads.len(), 2);

    let right = decode_button_function(reads[0], 1, 2).unwrap();
    assert_eq!(right.stored_layer, 1);
    assert_eq!(right.block, [0x01, 0x01, 0x02, 0x05, 0x00, 0x00, 0x02]);

    let scroll = decode_button_function(reads[1], 1, 9).unwrap();
    assert_eq!(scroll.stored_layer, 1);
    assert_eq!(scroll.block, [0x01, 0x01, 0x09, 0xE4, 0x00, 0x00, 0x02]);
}
