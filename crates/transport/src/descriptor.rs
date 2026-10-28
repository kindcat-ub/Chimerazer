//! Minimal HID report descriptor walk: size of a feature report.

/// Total size in bytes of the feature report `report_id` declared by a HID
/// report descriptor (`report_id` 0 means the descriptor uses no report IDs).
/// Returns `None` when the descriptor declares no such feature report or is
/// truncated.
pub fn feature_report_len(descriptor: &[u8], report_id: u8) -> Option<usize> {
    #[derive(Clone, Copy, Default)]
    struct Globals {
        report_size: u32,
        report_count: u32,
        report_id: u8,
    }

    let mut globals = Globals::default();
    let mut stack = Vec::new();
    let mut bits: Option<u64> = None;
    let mut i = 0;

    while i < descriptor.len() {
        let prefix = descriptor[i];
        if prefix == 0xFE {
            // Long item: [0xFE, size, tag, data..]
            let size = usize::from(*descriptor.get(i + 1)?);
            i += 3 + size;
            continue;
        }
        let size = match prefix & 0x03 {
            3 => 4,
            n => usize::from(n),
        };
        let data = descriptor.get(i + 1..i + 1 + size)?;
        let value = data
            .iter()
            .rev()
            .fold(0u32, |acc, &b| (acc << 8) | u32::from(b));
        match prefix & 0xFC {
            0x74 => globals.report_size = value,
            0x94 => globals.report_count = value,
            0x84 => globals.report_id = value as u8,
            0xA4 => stack.push(globals),
            0xB4 => globals = stack.pop()?,
            0xB0 if globals.report_id == report_id => {
                let item = u64::from(globals.report_size) * u64::from(globals.report_count);
                bits = Some(bits.unwrap_or(0) + item);
            }
            _ => {}
        }
        i += 1 + size;
    }

    bits.map(|b| b.div_ceil(8) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vendor collection with one 90-byte feature report and no report ID,
    /// shaped like the Razer control interface.
    const RAZER_CONTROL: &[u8] = &[
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, // generic desktop / mouse
        0x06, 0x00, 0xFF, 0x09, 0x02, // vendor page 0xFF00
        0x15, 0x00, 0x26, 0xFF, 0x00, // logical 0..255
        0x75, 0x08, 0x95, 0x5A, 0xB1, 0x01, // 8 bits x 90, Feature
        0xC0,
    ];

    #[test]
    fn finds_razer_control_report() {
        assert_eq!(feature_report_len(RAZER_CONTROL, 0), Some(90));
        assert_eq!(feature_report_len(RAZER_CONTROL, 1), None);
    }

    #[test]
    fn ignores_input_reports_and_honours_report_ids() {
        let desc = [
            0x85, 0x04, 0x75, 0x08, 0x95, 0x10, 0x81, 0x02, // input id 4
            0x85, 0x05, 0x95, 0x01, 0xB1, 0x02, // feature id 5, 1 byte
        ];
        assert_eq!(feature_report_len(&desc, 0), None);
        assert_eq!(feature_report_len(&desc, 4), None);
        assert_eq!(feature_report_len(&desc, 5), Some(1));
    }

    #[test]
    fn push_pop_restores_globals() {
        let desc = [0x75, 0x08, 0x95, 0x02, 0xA4, 0x95, 0x10, 0xB4, 0xB1, 0x02];
        assert_eq!(feature_report_len(&desc, 0), Some(2));
    }

    #[test]
    fn truncated_descriptor_is_none() {
        assert_eq!(feature_report_len(&[0x75, 0x08, 0x95], 0), None);
    }
}
