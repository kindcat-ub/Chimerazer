//! Device adapters. One module per device model, described as data.
//!
//! Every command a device module can issue is listed in its `COMMANDS`
//! catalog with a confirmation status and source IDs; the same table lives in
//! `docs/protocol/<device>.md` and a test keeps the two in sync.

use std::fmt;
use std::time::Duration;

use chimerazer_protocol::{ArgsTooLong, Command, Request};

mod exchange;
pub mod naga_pro;

pub use exchange::{ExchangeError, Trace, exchange};

/// Razer USB vendor ID.
pub const RAZER_VENDOR_ID: u16 = 0x1532;

/// What we know about one device model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Model {
    pub name: &'static str,
    pub usb_ids: &'static [UsbId],
    pub commands: &'static [CommandSpec],
}

/// Every model with a module in this crate.
pub const MODELS: &[Model] = &[naga_pro::MODEL];

/// The model and transport parameters for a product ID, if the model is known.
pub fn lookup(product_id: u16) -> Option<(&'static Model, &'static UsbId)> {
    MODELS.iter().find_map(|m| {
        m.usb_ids
            .iter()
            .find(|u| u.product_id == product_id)
            .map(|u| (m, u))
    })
}

/// A Razer configuration endpoint is recognised by capability, not by a
/// product list: vendor 0x1532 and a 90-byte feature report with no report
/// ID. Whether the model is known is a separate question ([`lookup`]).
pub fn is_control_interface(vendor_id: u16, feature_report_len: Option<usize>) -> bool {
    vendor_id == RAZER_VENDOR_ID && feature_report_len == Some(chimerazer_protocol::REPORT_LEN)
}

/// How the device is attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Wired,
    Receiver,
}

/// Transport parameters of one USB product ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbId {
    pub product_id: u16,
    pub link: Link,
    /// USB interface number that accepts 90-byte feature reports.
    pub interface: u8,
    pub transaction_id: u8,
    /// Wait between sending a request and the first response read.
    pub response_delay: Duration,
}

/// How far a command has been confirmed on this device model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmation {
    /// Verified on our own unit.
    OurHardware,
    /// Observed on a real unit of this model by another project.
    ExternalHardware,
    /// Known only from source code or documentation.
    SourceOnly,
}

impl Confirmation {
    /// Label used in `docs/protocol/*.md`.
    pub const fn label(self) -> &'static str {
        match self {
            Self::OurHardware => "OurHardware",
            Self::ExternalHardware => "ExternalHardware",
            Self::SourceOnly => "SourceOnly",
        }
    }
}

/// A catalogued command with its provenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub command: Command,
    pub name: &'static str,
    pub confirmation: Confirmation,
    /// Source IDs defined in `docs/protocol/SOURCES.md`.
    pub sources: &'static [&'static str],
}

/// A request could not be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// A read was requested with a write command.
    NotARead(Command),
    /// A write was requested with a read command.
    NotAWrite(Command),
    /// The write gate refused the command in this mode.
    WriteRefused {
        command: Command,
        reason: &'static str,
    },
    Args(ArgsTooLong),
}

/// How a write is being performed (`docs/ARCHITECTURE.md`, "Первичная
/// аппаратная валидация записи").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode {
    /// Regular operation: only commands verified on our hardware.
    Normal,
    /// First hardware validation of a command that is not yet verified.
    FirstValidation {
        /// The project owner explicitly allowed this `SourceOnly` command.
        allow_source_only: bool,
    },
}

/// The write gate. `write` is the write command, `read` the command that
/// reads the same value back; both must be catalogued.
pub fn write_request(
    usb: &UsbId,
    write: &CommandSpec,
    read: &CommandSpec,
    args: &[u8],
    mode: WriteMode,
) -> Result<Request, RequestError> {
    let refuse = |reason| {
        Err(RequestError::WriteRefused {
            command: write.command,
            reason,
        })
    };
    if write.command.is_read() {
        return Err(RequestError::NotAWrite(write.command));
    }
    if !read.command.is_read() {
        return Err(RequestError::NotARead(read.command));
    }
    match (mode, write.confirmation) {
        (_, Confirmation::OurHardware) => {}
        (WriteMode::Normal, _) => {
            return refuse("not verified on our hardware; use first validation");
        }
        (WriteMode::FirstValidation { .. }, _)
            if read.confirmation != Confirmation::OurHardware =>
        {
            return refuse("its read-back command is not verified on our hardware");
        }
        (WriteMode::FirstValidation { .. }, Confirmation::ExternalHardware) => {}
        (
            WriteMode::FirstValidation {
                allow_source_only: true,
            },
            Confirmation::SourceOnly,
        ) => {}
        (WriteMode::FirstValidation { .. }, Confirmation::SourceOnly) => {
            return refuse("SourceOnly needs the owner's explicit permission");
        }
    }
    Request::new(usb.transaction_id, write.command, args).map_err(RequestError::Args)
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotARead(c) => write!(f, "{c} is not a read command"),
            Self::NotAWrite(c) => write!(f, "{c} is not a write command"),
            Self::WriteRefused { command, reason } => {
                write!(f, "write {command} refused: {reason}")
            }
            Self::Args(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for RequestError {}

/// Builds a read request for `spec` on the product `usb`.
pub fn read_request(usb: &UsbId, spec: &CommandSpec, args: &[u8]) -> Result<Request, RequestError> {
    if !spec.command.is_read() {
        return Err(RequestError::NotARead(spec.command));
    }
    Request::new(usb.transaction_id, spec.command, args).map_err(RequestError::Args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_interface_is_recognised_by_capability() {
        assert!(is_control_interface(RAZER_VENDOR_ID, Some(90)));
        assert!(!is_control_interface(RAZER_VENDOR_ID, None));
        assert!(!is_control_interface(RAZER_VENDOR_ID, Some(1)));
        assert!(!is_control_interface(0x046D, Some(90)));
    }

    #[test]
    fn lookup_finds_known_products_only() {
        let (model, usb) = lookup(0x0090).unwrap();
        assert_eq!(model.name, naga_pro::MODEL.name);
        assert_eq!(usb.link, Link::Receiver);
        assert!(lookup(0x0092).is_none());
    }

    const SPEC: fn(u8, u8, Confirmation) -> CommandSpec = |class, id, confirmation| CommandSpec {
        command: Command::new(class, id, 1),
        name: "test",
        confirmation,
        sources: &["test"],
    };

    #[test]
    fn write_gate() {
        use Confirmation::*;
        let usb = &naga_pro::USB_IDS[0];
        let first = WriteMode::FirstValidation {
            allow_source_only: false,
        };
        let owner_ok = WriteMode::FirstValidation {
            allow_source_only: true,
        };
        let read_ok = SPEC(0x04, 0x85, OurHardware);
        let read_ext = SPEC(0x04, 0x85, ExternalHardware);
        let ok = |w, r, m| write_request(usb, &w, &r, &[0], m).is_ok();

        assert!(ok(
            SPEC(0x04, 0x05, OurHardware),
            read_ok,
            WriteMode::Normal
        ));
        assert!(!ok(
            SPEC(0x04, 0x05, ExternalHardware),
            read_ok,
            WriteMode::Normal
        ));
        assert!(ok(SPEC(0x04, 0x05, ExternalHardware), read_ok, first));
        assert!(!ok(SPEC(0x04, 0x05, ExternalHardware), read_ext, first));
        assert!(!ok(SPEC(0x04, 0x05, SourceOnly), read_ok, first));
        assert!(ok(SPEC(0x04, 0x05, SourceOnly), read_ok, owner_ok));
        assert!(!ok(
            SPEC(0x04, 0x85, OurHardware),
            read_ok,
            WriteMode::Normal
        ));
        assert!(!ok(
            SPEC(0x04, 0x05, OurHardware),
            SPEC(0x04, 0x05, OurHardware),
            WriteMode::Normal
        ));
    }

    #[test]
    fn product_ids_are_unique_across_models() {
        let mut ids: Vec<u16> = MODELS
            .iter()
            .flat_map(|m| m.usb_ids.iter().map(|u| u.product_id))
            .collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }
}
