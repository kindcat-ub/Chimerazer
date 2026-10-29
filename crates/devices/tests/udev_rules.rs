//! The udev rule grants access to Razer hidraw nodes by vendor alone, so
//! detection by capability is not blocked by a product list.

use std::path::Path;

use chimerazer_devices::RAZER_VENDOR_ID;

#[test]
fn rule_covers_vendor_without_product_list() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/udev/70-chimerazer.rules");
    let rules =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let rule_lines: Vec<&str> = rules
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let vendor = format!("ATTRS{{idVendor}}==\"{RAZER_VENDOR_ID:04x}\"");
    assert_eq!(rule_lines.len(), 1, "{rule_lines:?}");
    let rule = rule_lines[0];
    assert!(rule.contains("KERNEL==\"hidraw*\""));
    assert!(rule.contains(&vendor));
    assert!(rule.contains("TAG+=\"uaccess\""));
    // ATTRS keys must match one parent device; product or interface
    // attributes would either narrow detection or silently never match.
    assert!(!rule.contains("idProduct"));
    assert!(!rule.contains("bInterfaceNumber"));
}
