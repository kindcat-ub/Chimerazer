//! HID interface discovery through `/sys/class/hidraw`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::descriptor::feature_report_len;

const SYSFS_HIDRAW: &str = "/sys/class/hidraw";

/// One hidraw node as described by sysfs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HidInterface {
    /// `/dev/hidrawN`. The number is not stable across reconnects.
    pub node: PathBuf,
    pub vendor_id: u16,
    pub product_id: u16,
    /// USB interface number, if the node sits on a USB interface.
    pub interface: Option<u8>,
    /// Size of feature report ID 0 declared by the report descriptor.
    pub feature_report_len: Option<usize>,
}

/// Lists hidraw interfaces of the running system.
pub fn enumerate() -> io::Result<Vec<HidInterface>> {
    enumerate_in(Path::new(SYSFS_HIDRAW), Path::new("/dev"))
}

/// Lists hidraw interfaces under a sysfs class directory. Nodes whose
/// attributes cannot be read are skipped.
pub fn enumerate_in(class_dir: &Path, dev_dir: &Path) -> io::Result<Vec<HidInterface>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(class_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|n| n.starts_with("hidraw")) else {
            continue;
        };
        if let Some(interface) = describe(&entry.path(), &dev_dir.join(name)) {
            found.push(interface);
        }
    }
    found.sort_by(|a, b| a.node.cmp(&b.node));
    Ok(found)
}

fn describe(sys: &Path, node: &Path) -> Option<HidInterface> {
    let device = sys.join("device");
    let uevent = fs::read_to_string(device.join("uevent")).ok()?;
    let (vendor_id, product_id) = parse_hid_id(&uevent)?;
    let interface = fs::read_to_string(device.join("../bInterfaceNumber"))
        .ok()
        .and_then(|s| u8::from_str_radix(s.trim(), 16).ok());
    let feature_report_len = fs::read(device.join("report_descriptor"))
        .ok()
        .and_then(|d| feature_report_len(&d, 0));
    Some(HidInterface {
        node: node.to_path_buf(),
        vendor_id,
        product_id,
        interface,
        feature_report_len,
    })
}

/// Parses `HID_ID=0003:00001532:0000008F` from a HID device uevent.
fn parse_hid_id(uevent: &str) -> Option<(u16, u16)> {
    let id = uevent.lines().find_map(|l| l.strip_prefix("HID_ID="))?;
    let mut parts = id.trim().split(':').skip(1);
    let vendor = u32::from_str_radix(parts.next()?, 16).ok()?;
    let product = u32::from_str_radix(parts.next()?, 16).ok()?;
    Some((u16::try_from(vendor).ok()?, u16::try_from(product).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hid_id() {
        let uevent = "DRIVER=hid-generic\nHID_ID=0003:00001532:0000008F\nHID_NAME=Razer\n";
        assert_eq!(parse_hid_id(uevent), Some((0x1532, 0x008F)));
        assert_eq!(parse_hid_id("HID_NAME=x\n"), None);
        assert_eq!(parse_hid_id("HID_ID=0003:zz:0000008F\n"), None);
    }

    /// Builds `class/hidrawN/device -> usb/<intf>/<hid>` like real sysfs.
    fn fake_node(root: &Path, n: u32, product: u16, intf: u8, descriptor: &[u8]) {
        let usb_intf = root.join(format!("usb/1-1:1.{intf}"));
        let hid = usb_intf.join(format!("0003:1532:{product:04X}.{n:04X}"));
        fs::create_dir_all(&hid).unwrap();
        fs::write(usb_intf.join("bInterfaceNumber"), format!("{intf:02x}\n")).unwrap();
        fs::write(
            hid.join("uevent"),
            format!("HID_ID=0003:00001532:{product:08X}\n"),
        )
        .unwrap();
        fs::write(hid.join("report_descriptor"), descriptor).unwrap();
        let class = root.join(format!("class/hidraw{n}"));
        fs::create_dir_all(&class).unwrap();
        std::os::unix::fs::symlink(&hid, class.join("device")).unwrap();
    }

    #[test]
    fn enumerates_fake_sysfs() {
        let root = std::env::temp_dir().join(format!("chz-sysfs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let control = [0x75, 0x08, 0x95, 0x5A, 0xB1, 0x01];
        fake_node(&root, 9, 0x008F, 0, &control);
        fake_node(&root, 10, 0x008F, 1, &[0x75, 0x08, 0x95, 0x08, 0x81, 0x02]);

        let found = enumerate_in(&root.join("class"), Path::new("/dev")).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(
            found,
            [
                HidInterface {
                    node: "/dev/hidraw10".into(),
                    vendor_id: 0x1532,
                    product_id: 0x008F,
                    interface: Some(1),
                    feature_report_len: None,
                },
                HidInterface {
                    node: "/dev/hidraw9".into(),
                    vendor_id: 0x1532,
                    product_id: 0x008F,
                    interface: Some(0),
                    feature_report_len: Some(90),
                },
            ]
        );
    }
}
