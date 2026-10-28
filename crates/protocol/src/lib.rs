//! Razer HID configuration protocol: the 90-byte feature report and the
//! payload encodings shared across devices.
//!
//! This crate performs no I/O. Facts about the report layout come from
//! OpenSnek (`docs/protocol/USB_PROTOCOL.md`) and OpenRazer
//! (`driver/razercommon.h`); see `docs/protocol/SOURCES.md`.

mod payload;
mod report;

pub use payload::{
    ButtonInventory, DpiStage, DpiStages, DpiXy, LedBrightness, PayloadError, ProfileInventory,
};
pub use report::{
    ARGS_LEN, ArgsTooLong, Command, REPORT_LEN, Request, Response, ResponseError, Status, crc,
};
