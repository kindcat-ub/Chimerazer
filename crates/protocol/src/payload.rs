//! Decoders for response argument payloads shared across Razer mice.
//!
//! Every decoder takes the full argument area of a response (`args[0..80]`).

use std::fmt;

/// Payload does not match the expected encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadError {
    TooShort { needed: usize, got: usize },
    StageCount(u8),
    InventoryCount(u8),
}

impl fmt::Display for PayloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort { needed, got } => write!(f, "payload too short: {got} < {needed}"),
            Self::StageCount(n) => write!(f, "invalid DPI stage count {n}"),
            Self::InventoryCount(n) => write!(f, "invalid inventory count {n}"),
        }
    }
}

impl std::error::Error for PayloadError {}

fn need(args: &[u8], len: usize) -> Result<(), PayloadError> {
    if args.len() < len {
        return Err(PayloadError::TooShort {
            needed: len,
            got: args.len(),
        });
    }
    Ok(())
}

fn be16(hi: u8, lo: u8) -> u16 {
    u16::from_be_bytes([hi, lo])
}

/// `04:85` scalar DPI: `[storage, x_be16, y_be16, ..]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DpiXy {
    pub storage: u8,
    pub x: u16,
    pub y: u16,
}

impl DpiXy {
    pub fn decode(args: &[u8]) -> Result<Self, PayloadError> {
        need(args, 5)?;
        Ok(Self {
            storage: args[0],
            x: be16(args[1], args[2]),
            y: be16(args[3], args[4]),
        })
    }
}

/// One row of a DPI stage table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DpiStage {
    /// Stage ID token. Not an index.
    pub id: u8,
    pub x: u16,
    pub y: u16,
}

/// `04:86` DPI stage table:
/// `[storage, active_stage_id, count, count × [id, x_be16, y_be16, 0, 0]]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DpiStages {
    pub storage: u8,
    /// Stage ID token of the active stage; matches one of `stages[..].id`
    /// on a consistent table.
    pub active: u8,
    pub stages: Vec<DpiStage>,
}

impl DpiStages {
    pub const MAX_STAGES: u8 = 5;
    const ROW: usize = 7;

    pub fn decode(args: &[u8]) -> Result<Self, PayloadError> {
        need(args, 3)?;
        let count = args[2];
        if count == 0 || count > Self::MAX_STAGES {
            return Err(PayloadError::StageCount(count));
        }
        let rows = &args[3..];
        need(rows, usize::from(count) * Self::ROW)?;
        let stages = rows
            .as_chunks::<{ Self::ROW }>()
            .0
            .iter()
            .take(count.into())
            .map(|r| DpiStage {
                id: r[0],
                x: be16(r[1], r[2]),
                y: be16(r[3], r[4]),
            })
            .collect();
        Ok(Self {
            storage: args[0],
            active: args[1],
            stages,
        })
    }

    /// The active stage, if the active token names one of the rows.
    pub fn active_stage(&self) -> Option<&DpiStage> {
        self.stages.iter().find(|s| s.id == self.active)
    }
}

/// `0F:84` LED brightness: `[storage, led, value]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedBrightness {
    pub storage: u8,
    pub led: u8,
    pub value: u8,
}

impl LedBrightness {
    pub fn decode(args: &[u8]) -> Result<Self, PayloadError> {
        need(args, 3)?;
        Ok(Self {
            storage: args[0],
            led: args[1],
            value: args[2],
        })
    }
}

/// `05:81` onboard profile inventory: `[max_profile_id, assigned ids.., 0, ..]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileInventory {
    pub max_profile: u8,
    pub assigned: Vec<u8>,
}

impl ProfileInventory {
    pub fn decode(args: &[u8]) -> Result<Self, PayloadError> {
        need(args, 1)?;
        Ok(Self {
            max_profile: args[0],
            assigned: args[1..]
                .iter()
                .copied()
                .take_while(|&id| id != 0)
                .collect(),
        })
    }
}

/// `02:84` mappable button inventory: `[count, slot ids..]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ButtonInventory {
    pub slots: Vec<u8>,
}

impl ButtonInventory {
    pub fn decode(args: &[u8]) -> Result<Self, PayloadError> {
        need(args, 1)?;
        let count = args[0];
        if count == 0 {
            return Err(PayloadError::InventoryCount(count));
        }
        need(&args[1..], count.into())?;
        Ok(Self {
            slots: args[1..=count.into()].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpi_xy() {
        let d = DpiXy::decode(&[0x01, 0x06, 0x40, 0x03, 0x20, 0, 0]).unwrap();
        assert_eq!((d.storage, d.x, d.y), (1, 1600, 800));
        assert!(DpiXy::decode(&[0; 4]).is_err());
    }

    #[test]
    fn dpi_stages_rejects_bad_count() {
        assert_eq!(
            DpiStages::decode(&[0, 1, 0]),
            Err(PayloadError::StageCount(0))
        );
        assert_eq!(
            DpiStages::decode(&[0, 1, 6]),
            Err(PayloadError::StageCount(6))
        );
        assert!(matches!(
            DpiStages::decode(&[0, 1, 2, 1, 0, 100, 0, 100, 0, 0]),
            Err(PayloadError::TooShort { .. })
        ));
    }

    #[test]
    fn dpi_stages_active_is_a_token() {
        let args = [
            0, 0x07, 2, 0x05, 0x01, 0x90, 0x01, 0x90, 0, 0, 0x07, 0x03, 0x20, 0x03, 0x20, 0, 0,
        ];
        let t = DpiStages::decode(&args).unwrap();
        assert_eq!(t.active_stage().map(|s| s.x), Some(800));
    }

    #[test]
    fn profile_inventory_stops_at_zero() {
        let mut args = [0u8; 80];
        args[..4].copy_from_slice(&[5, 1, 2, 0]);
        args[5] = 9;
        let inv = ProfileInventory::decode(&args).unwrap();
        assert_eq!((inv.max_profile, inv.assigned), (5, vec![1, 2]));
    }

    #[test]
    fn button_inventory_checks_count() {
        assert_eq!(
            ButtonInventory::decode(&[0]),
            Err(PayloadError::InventoryCount(0))
        );
        assert!(ButtonInventory::decode(&[3, 1, 2]).is_err());
        assert_eq!(
            ButtonInventory::decode(&[2, 1, 2, 9]).unwrap().slots,
            vec![1, 2]
        );
    }
}
