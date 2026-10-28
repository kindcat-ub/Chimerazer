//! Every supported product ID has a udev rule granting hidraw access.

use std::path::Path;

use chimerazer_devices::{RAZER_VENDOR_ID, naga_pro};

#[test]
fn rules_cover_all_product_ids() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/udev/70-chimerazer.rules");
    let rules =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let rule_lines: Vec<&str> = rules
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect();

    for usb in &naga_pro::USB_IDS {
        let vendor = format!("ATTRS{{idVendor}}==\"{RAZER_VENDOR_ID:04x}\"");
        let product = format!("ATTRS{{idProduct}}==\"{:04x}\"", usb.product_id);
        let interface = format!("ATTRS{{bInterfaceNumber}}==\"{:02x}\"", usb.interface);
        assert!(
            rule_lines.iter().any(|l| {
                l.contains("KERNEL==\"hidraw*\"")
                    && l.contains(&vendor)
                    && l.contains(&product)
                    && l.contains(&interface)
                    && l.contains("TAG+=\"uaccess\"")
            }),
            "no hidraw rule for {:04x}",
            usb.product_id
        );
    }
}
