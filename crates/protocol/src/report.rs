use std::fmt;

/// Size of a Razer configuration report, without the HID report ID byte.
pub const REPORT_LEN: usize = 90;

/// Size of the argument area of a report.
pub const ARGS_LEN: usize = 80;

const STATUS: usize = 0;
const TXN: usize = 1;
const REMAINING: usize = 2;
const PROTOCOL: usize = 4;
const DATA_SIZE: usize = 5;
const CLASS: usize = 6;
const ID: usize = 7;
const ARGS: usize = 8;
const CRC: usize = 88;

/// A command address: class, id and the declared argument size.
///
/// Bit 7 of `id` marks a read (get) command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Command {
    pub class: u8,
    pub id: u8,
    pub size: u8,
}

impl Command {
    pub const fn new(class: u8, id: u8, size: u8) -> Self {
        Self { class, id, size }
    }

    pub const fn is_read(&self) -> bool {
        self.id & 0x80 != 0
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02X}:{:02X}", self.class, self.id)
    }
}

/// Report status byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    New,
    Busy,
    Ok,
    Failed,
    Timeout,
    NotSupported,
}

impl Status {
    pub const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0x00 => Some(Self::New),
            0x01 => Some(Self::Busy),
            0x02 => Some(Self::Ok),
            0x03 => Some(Self::Failed),
            0x04 => Some(Self::Timeout),
            0x05 => Some(Self::NotSupported),
            _ => None,
        }
    }

    pub const fn to_byte(self) -> u8 {
        match self {
            Self::New => 0x00,
            Self::Busy => 0x01,
            Self::Ok => 0x02,
            Self::Failed => 0x03,
            Self::Timeout => 0x04,
            Self::NotSupported => 0x05,
        }
    }
}

/// XOR of bytes 2..88 of a report.
pub fn crc(report: &[u8; REPORT_LEN]) -> u8 {
    report[REMAINING..CRC].iter().fold(0, |acc, b| acc ^ b)
}

/// A request report ready to be sent as a feature report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub transaction_id: u8,
    pub command: Command,
    pub args: [u8; ARGS_LEN],
}

impl Request {
    /// Builds a request. `args` longer than the argument area is an error,
    /// shorter is zero-padded.
    pub fn new(transaction_id: u8, command: Command, args: &[u8]) -> Result<Self, ArgsTooLong> {
        if args.len() > ARGS_LEN || usize::from(command.size) > ARGS_LEN {
            return Err(ArgsTooLong);
        }
        let mut buf = [0; ARGS_LEN];
        buf[..args.len()].copy_from_slice(args);
        Ok(Self {
            transaction_id,
            command,
            args: buf,
        })
    }

    pub fn encode(&self) -> [u8; REPORT_LEN] {
        let mut report = [0; REPORT_LEN];
        report[STATUS] = Status::New.to_byte();
        report[TXN] = self.transaction_id;
        report[DATA_SIZE] = self.command.size;
        report[CLASS] = self.command.class;
        report[ID] = self.command.id;
        report[ARGS..CRC].copy_from_slice(&self.args);
        report[CRC] = crc(&report);
        report
    }
}

/// Arguments do not fit into a report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArgsTooLong;

impl fmt::Display for ArgsTooLong {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "arguments exceed {ARGS_LEN} bytes")
    }
}

impl std::error::Error for ArgsTooLong {}

/// A structurally valid response report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: Status,
    pub transaction_id: u8,
    pub data_size: u8,
    pub class: u8,
    pub id: u8,
    pub args: [u8; ARGS_LEN],
}

impl Response {
    /// Parses a feature-report buffer. Accepts 90 bytes, or 91 bytes with a
    /// leading report ID of 0 as returned by hidraw.
    pub fn parse(buf: &[u8]) -> Result<Self, ResponseError> {
        let report: &[u8; REPORT_LEN] = match buf.len() {
            REPORT_LEN => buf.try_into().expect("length checked"),
            91 if buf[0] == 0 => buf[1..].try_into().expect("length checked"),
            91 => return Err(ResponseError::ReportId(buf[0])),
            n => return Err(ResponseError::Length(n)),
        };
        let expected = crc(report);
        if report[CRC] != expected {
            return Err(ResponseError::Crc {
                expected,
                found: report[CRC],
            });
        }
        let status = Status::from_byte(report[STATUS])
            .ok_or(ResponseError::UnknownStatus(report[STATUS]))?;
        if usize::from(report[DATA_SIZE]) > ARGS_LEN {
            return Err(ResponseError::DataSize(report[DATA_SIZE]));
        }
        if report[PROTOCOL] != 0 {
            return Err(ResponseError::ProtocolType(report[PROTOCOL]));
        }
        Ok(Self {
            status,
            transaction_id: report[TXN],
            data_size: report[DATA_SIZE],
            class: report[CLASS],
            id: report[ID],
            args: report[ARGS..CRC].try_into().expect("slice is ARGS_LEN"),
        })
    }

    /// Checks that this response answers `request` successfully and returns
    /// its arguments. Command-specific argument echoes (profile, slot, LED)
    /// are the caller's responsibility.
    pub fn answer_to(&self, request: &Request) -> Result<&[u8; ARGS_LEN], ResponseError> {
        if self.transaction_id != request.transaction_id
            || self.class != request.command.class
            || self.id != request.command.id
        {
            return Err(ResponseError::Echo {
                sent: request.command,
                transaction_id: self.transaction_id,
                class: self.class,
                id: self.id,
            });
        }
        if self.status != Status::Ok {
            return Err(ResponseError::Status(self.status));
        }
        Ok(&self.args)
    }
}

/// Why a response was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseError {
    Length(usize),
    ReportId(u8),
    Crc {
        expected: u8,
        found: u8,
    },
    UnknownStatus(u8),
    DataSize(u8),
    ProtocolType(u8),
    /// The response belongs to a different transaction or command.
    Echo {
        sent: Command,
        transaction_id: u8,
        class: u8,
        id: u8,
    },
    /// The device answered with a status other than `Ok`.
    Status(Status),
}

impl fmt::Display for ResponseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length(n) => write!(f, "unexpected report length {n}"),
            Self::ReportId(id) => write!(f, "unexpected report id {id:#04x}"),
            Self::Crc { expected, found } => {
                write!(
                    f,
                    "crc mismatch: expected {expected:#04x}, found {found:#04x}"
                )
            }
            Self::UnknownStatus(s) => write!(f, "unknown status {s:#04x}"),
            Self::DataSize(s) => write!(f, "data size {s} exceeds {ARGS_LEN}"),
            Self::ProtocolType(p) => write!(f, "unexpected protocol type {p:#04x}"),
            Self::Echo {
                sent,
                transaction_id,
                class,
                id,
            } => write!(
                f,
                "response {class:02X}:{id:02X} txn {transaction_id:#04x} does not answer {sent}"
            ),
            Self::Status(s) => write!(f, "device returned status {s:?}"),
        }
    }
}

impl std::error::Error for ResponseError {}

#[cfg(test)]
mod tests {
    use super::*;

    const DPI_READ: Command = Command::new(0x04, 0x85, 0x07);

    fn reply(request: &Request, status: Status, args: &[u8]) -> [u8; REPORT_LEN] {
        let mut report = request.encode();
        report[STATUS] = status.to_byte();
        report[ARGS..ARGS + args.len()].copy_from_slice(args);
        report[CRC] = crc(&report);
        report
    }

    #[test]
    fn encodes_header_args_and_crc() {
        let req = Request::new(0x1F, DPI_READ, &[0x00]).unwrap();
        let report = req.encode();
        assert_eq!(
            &report[..8],
            &[0x00, 0x1F, 0x00, 0x00, 0x00, 0x07, 0x04, 0x85]
        );
        assert!(report[8..88].iter().all(|&b| b == 0));
        assert_eq!(report[88], 0x07 ^ 0x04 ^ 0x85);
        assert_eq!(report[89], 0x00);
    }

    #[test]
    fn crc_ignores_status_and_transaction_id() {
        let a = Request::new(0x1F, DPI_READ, &[0x01]).unwrap().encode();
        let b = Request::new(0xFF, DPI_READ, &[0x01]).unwrap().encode();
        assert_eq!(a[88], b[88]);
    }

    #[test]
    fn rejects_oversized_args() {
        assert_eq!(Request::new(0x1F, DPI_READ, &[0; 81]), Err(ArgsTooLong));
        assert_eq!(
            Request::new(0x1F, Command::new(0x04, 0x85, 81), &[]),
            Err(ArgsTooLong)
        );
    }

    #[test]
    fn parses_successful_answer() {
        let req = Request::new(0x1F, DPI_READ, &[0x00]).unwrap();
        let raw = reply(&req, Status::Ok, &[0x00, 0x06, 0x40, 0x06, 0x40]);
        let resp = Response::parse(&raw).unwrap();
        assert_eq!(
            &resp.answer_to(&req).unwrap()[..5],
            &[0x00, 0x06, 0x40, 0x06, 0x40]
        );
    }

    #[test]
    fn accepts_leading_report_id_zero() {
        let req = Request::new(0x1F, DPI_READ, &[0x00]).unwrap();
        let mut raw = vec![0x00];
        raw.extend_from_slice(&reply(&req, Status::Ok, &[]));
        assert!(Response::parse(&raw).is_ok());
        raw[0] = 0x01;
        assert_eq!(Response::parse(&raw), Err(ResponseError::ReportId(0x01)));
    }

    #[test]
    fn rejects_bad_length_and_crc() {
        assert_eq!(Response::parse(&[0; 64]), Err(ResponseError::Length(64)));
        let req = Request::new(0x1F, DPI_READ, &[0x00]).unwrap();
        let mut raw = reply(&req, Status::Ok, &[]);
        raw[20] ^= 0xFF;
        assert!(matches!(
            Response::parse(&raw),
            Err(ResponseError::Crc { .. })
        ));
    }

    #[test]
    fn rejects_unknown_status() {
        let req = Request::new(0x1F, DPI_READ, &[]).unwrap();
        let mut raw = reply(&req, Status::Ok, &[]);
        raw[STATUS] = 0x07;
        assert_eq!(
            Response::parse(&raw),
            Err(ResponseError::UnknownStatus(0x07))
        );
    }

    #[test]
    fn rejects_foreign_transaction_and_command() {
        let req = Request::new(0x1F, DPI_READ, &[]).unwrap();
        let other_txn = Request::new(0xFF, DPI_READ, &[]).unwrap();
        let other_cmd = Request::new(0x1F, Command::new(0x04, 0x86, 0x26), &[]).unwrap();
        for raw in [
            reply(&other_txn, Status::Ok, &[]),
            reply(&other_cmd, Status::Ok, &[]),
        ] {
            let resp = Response::parse(&raw).unwrap();
            assert!(matches!(
                resp.answer_to(&req),
                Err(ResponseError::Echo { .. })
            ));
        }
    }

    #[test]
    fn surfaces_non_ok_status() {
        let req = Request::new(0x1F, DPI_READ, &[]).unwrap();
        for status in [
            Status::Busy,
            Status::Failed,
            Status::Timeout,
            Status::NotSupported,
        ] {
            let resp = Response::parse(&reply(&req, status, &[])).unwrap();
            assert_eq!(resp.answer_to(&req), Err(ResponseError::Status(status)));
        }
    }

    #[test]
    fn status_byte_round_trips() {
        for byte in 0..=5 {
            assert_eq!(Status::from_byte(byte).unwrap().to_byte(), byte);
        }
        assert_eq!(Status::from_byte(6), None);
    }
}
