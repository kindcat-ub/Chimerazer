//! Simulated Naga Pro for tests.

use std::collections::HashMap;
use std::io;
use std::time::Duration;

use chimerazer_protocol::{ARGS_LEN, REPORT_LEN, Status, crc};
use chimerazer_transport::FeatureIo;

use super::BUTTON_SLOTS;
use crate::{Link, UsbId};

pub const USB: UsbId = UsbId {
    product_id: 0x008F,
    link: Link::Wired,
    interface: 0,
    transaction_id: 0x1F,
    response_delay: Duration::ZERO,
};

/// A simulated Naga Pro holding DPI, stages, brightness and poll rate.
/// Every other read answers with a fixed zero-filled OK frame.
#[derive(Default)]
pub struct Sim {
    pub dpi: HashMap<u8, [u8; 4]>,
    pub stages: HashMap<u8, Vec<u8>>,
    pub brightness: HashMap<(u8, u8), u8>,
    /// `0F:82` effect bytes (`args[2..12]`) by storage and LED.
    pub effect: HashMap<(u8, u8), [u8; 10]>,
    /// `02:8C` stored flag and block of layer 0 by storage and slot.
    pub buttons: HashMap<(u8, u8), (u8, [u8; 7])>,
    /// The HyperShift layer (`02:8C` with layer 1) by storage and slot.
    pub hypershift: HashMap<(u8, u8), [u8; 7]>,
    /// A layer-0 button write also changes the HyperShift block.
    pub button_write_hits_hypershift: bool,
    /// Layer-1 writes report success but leave the flag and block unchanged.
    pub ignore_hypershift_writes: bool,
    /// Layer-1 reads fail (used to check that preservation is prepared first).
    pub reject_hypershift_reads: bool,
    pub poll: u8,
    pub ignore_writes: bool,
    /// A write to DPI also changes brightness on storage 0, LED 1.
    pub side_effect: bool,
    /// Writes are applied but answered with status Failed.
    pub nack_applied_writes: bool,
    reply: Option<[u8; REPORT_LEN]>,
}

impl Sim {
    pub fn new() -> Self {
        let mut s = Self {
            poll: 0x01,
            ..Self::default()
        };
        for st in 0..=5 {
            s.dpi.insert(st, [0x06, 0x40, 0x06, 0x40]);
            let mut t = vec![0x03, 0x05];
            for (id, dpi) in [(1u8, 400u16), (2, 800), (3, 1600), (4, 3200), (5, 6400)] {
                t.push(id);
                t.extend(dpi.to_be_bytes());
                t.extend(dpi.to_be_bytes());
                t.extend([0, 0]);
            }
            s.stages.insert(st, t);
            s.brightness.insert((st, 0x01), 51);
            s.brightness.insert((st, 0x04), 51);
            for led in [0x01, 0x04] {
                s.effect.insert((st, led), [3, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            }
            for slot in BUTTON_SLOTS {
                let block = [0x02, 0x02, 0, 0x1E, 0, 0, 0];
                s.buttons.insert((st, slot.id), (0, block));
                s.hypershift
                    .insert((st, slot.id), [0x02, 0x02, 0, 0x1F, 0, 0, 0]);
            }
        }
        s
    }
}

impl FeatureIo for Sim {
    fn send_feature(&mut self, report: &[u8]) -> io::Result<()> {
        let mut out: [u8; REPORT_LEN] = report.try_into().unwrap();
        let (class, id, a) = (report[6], report[7], &report[8..88]);
        let mut args = [0u8; ARGS_LEN];
        args.copy_from_slice(a);
        let write = id & 0x80 == 0;
        if write && !self.ignore_writes {
            match (class, id) {
                (0x00, 0x05) => self.poll = a[0],
                (0x04, 0x05) => {
                    self.dpi.insert(a[0], a[1..5].try_into().unwrap());
                    if self.side_effect {
                        self.brightness.insert((0, 0x01), 1);
                    }
                }
                (0x04, 0x06) => {
                    let n = 2 + usize::from(a[2]) * 7;
                    self.stages.insert(a[0], a[1..1 + n].to_vec());
                }
                (0x0F, 0x04) => {
                    self.brightness.insert((a[0], a[1]), a[2]);
                }
                (0x02, 0x0C) => {
                    if a[2] == 1 && self.ignore_hypershift_writes {
                        out[0] = Status::Ok.to_byte();
                        out[88] = crc(&out);
                        self.reply = Some(out);
                        return Ok(());
                    }
                    let block: [u8; 7] = a[3..10].try_into().unwrap();
                    // As on our unit: the write's layer byte becomes the
                    // slot's stored flag, whichever layer it addresses.
                    self.buttons.get_mut(&(a[0], a[1])).unwrap().0 = a[2];
                    if a[2] == 1 {
                        self.hypershift.insert((a[0], a[1]), block);
                    } else {
                        self.buttons.get_mut(&(a[0], a[1])).unwrap().1 = block;
                        if self.button_write_hits_hypershift {
                            self.hypershift.insert((a[0], a[1]), [0; 7]);
                        }
                    }
                }
                (0x0F, 0x02) => {
                    let mut e = [0; 10];
                    let n = usize::from(report[5]).saturating_sub(2).min(10);
                    e[..n].copy_from_slice(&a[2..2 + n]);
                    self.effect.insert((a[0], a[1]), e);
                }
                _ => {}
            }
        }
        if !write {
            match (class, id) {
                (0x00, 0x85) => args[0] = self.poll,
                (0x04, 0x85) => args[1..5].copy_from_slice(&self.dpi[&a[0]]),
                (0x04, 0x86) => {
                    let t = &self.stages[&a[0]];
                    args[1..1 + t.len()].copy_from_slice(t);
                }
                (0x0F, 0x84) => args[2] = self.brightness[&(a[0], a[1])],
                (0x02, 0x8C) => {
                    let (flag, block) = self.buttons[&(a[0], a[1])];
                    args[2] = flag;
                    if a[2] == 1 {
                        args[3..10].copy_from_slice(&self.hypershift[&(a[0], a[1])]);
                    } else {
                        args[3..10].copy_from_slice(&block);
                    }
                }
                (0x0F, 0x82) => args[2..12].copy_from_slice(&self.effect[&(a[0], a[1])]),
                (0x02, 0x84) => {
                    args[0] = 30;
                    for (i, s) in BUTTON_SLOTS.iter().enumerate() {
                        args[1 + i] = s.id;
                    }
                }
                (0x05, 0x81) => args[..5].copy_from_slice(&[5, 1, 2, 3, 4]),
                (0x00, 0x81) => args[..2].copy_from_slice(&[2, 3]),
                _ => {}
            }
        }
        out[0] = if (write && self.nack_applied_writes)
            || (!write && class == 2 && id == 0x8c && a[2] == 1 && self.reject_hypershift_reads)
        {
            Status::Failed.to_byte()
        } else {
            Status::Ok.to_byte()
        };
        out[8..88].copy_from_slice(&args);
        out[88] = crc(&out);
        self.reply = Some(out);
        Ok(())
    }

    fn get_feature(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        buf[..REPORT_LEN].copy_from_slice(&self.reply.unwrap());
        Ok(REPORT_LEN)
    }
}
