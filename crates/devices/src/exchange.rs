//! One request/response round trip over a feature-report transport.

use std::fmt;
use std::io;
use std::thread::sleep;
use std::time::Duration;

use chimerazer_protocol::{REPORT_LEN, Request, Response, ResponseError, Status};
use chimerazer_transport::FeatureIo;

use crate::UsbId;

/// Response reads after the first one, for a device that is still busy or
/// returned a stale frame.
const POLLS: usize = 6;
const POLL_INTERVAL: Duration = Duration::from_millis(30);

/// Every raw frame of one exchange, for logs and fixtures.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trace {
    pub request: Vec<u8>,
    pub responses: Vec<Vec<u8>>,
}

#[derive(Debug)]
pub enum ExchangeError {
    Io(io::Error),
    /// The device answered this request with a non-OK status.
    Device(Status),
    /// No valid answer within the poll budget; carries the last rejection.
    NoAnswer(Option<ResponseError>),
}

impl fmt::Display for ExchangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Device(s) => write!(f, "device returned status {s:?}"),
            Self::NoAnswer(Some(e)) => write!(f, "no valid answer: {e}"),
            Self::NoAnswer(None) => write!(f, "no valid answer"),
        }
    }
}

impl std::error::Error for ExchangeError {}

/// Sends `request` and waits for the response that answers it.
///
/// Busy, not-yet-processed and stale frames are polled again; a definitive
/// failure status ends the exchange. The trace is returned in every case.
pub fn exchange(
    io: &mut impl FeatureIo,
    usb: &UsbId,
    request: &Request,
) -> (Trace, Result<Response, ExchangeError>) {
    let encoded = request.encode();
    let mut trace = Trace {
        request: encoded.to_vec(),
        responses: Vec::new(),
    };
    if let Err(e) = io.send_feature(&encoded) {
        return (trace, Err(ExchangeError::Io(e)));
    }
    sleep(usb.response_delay);

    let mut last = None;
    for attempt in 0..=POLLS {
        if attempt > 0 {
            sleep(POLL_INTERVAL);
        }
        let mut buf = [0u8; REPORT_LEN];
        let len = match io.get_feature(&mut buf) {
            Ok(len) => len,
            Err(e) => return (trace, Err(ExchangeError::Io(e))),
        };
        trace.responses.push(buf[..len].to_vec());
        let outcome = Response::parse(&buf[..len]).and_then(|r| match r.answer_to(request) {
            Ok(_) => Ok(r),
            Err(e) => Err(e),
        });
        match outcome {
            Ok(response) => return (trace, Ok(response)),
            Err(ResponseError::Status(
                s @ (Status::Failed | Status::Timeout | Status::NotSupported),
            )) => return (trace, Err(ExchangeError::Device(s))),
            Err(e) => last = Some(e),
        }
    }
    (trace, Err(ExchangeError::NoAnswer(last)))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::time::Duration;

    use chimerazer_protocol::{Command, crc};

    use super::*;
    use crate::Link;

    const USB: UsbId = UsbId {
        product_id: 0x008F,
        link: Link::Wired,
        interface: 0,
        transaction_id: 0x1F,
        response_delay: Duration::ZERO,
    };
    const DPI: Command = Command::new(0x04, 0x85, 0x07);

    struct Fake {
        sent: Vec<Vec<u8>>,
        replies: VecDeque<[u8; REPORT_LEN]>,
    }

    impl FeatureIo for Fake {
        fn send_feature(&mut self, report: &[u8]) -> io::Result<()> {
            self.sent.push(report.to_vec());
            Ok(())
        }

        fn get_feature(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let reply = self.replies.pop_front().ok_or(io::ErrorKind::TimedOut)?;
            buf[..REPORT_LEN].copy_from_slice(&reply);
            Ok(REPORT_LEN)
        }
    }

    fn reply(request: &Request, status: Status, txn: u8) -> [u8; REPORT_LEN] {
        let mut r = request.encode();
        r[0] = status.to_byte();
        r[1] = txn;
        r[8..13].copy_from_slice(&[0x00, 0x06, 0x40, 0x06, 0x40]);
        r[88] = crc(&r);
        r
    }

    fn run(replies: Vec<[u8; REPORT_LEN]>) -> (Fake, Trace, Result<Response, ExchangeError>) {
        let request = Request::new(0x1F, DPI, &[0]).unwrap();
        let mut fake = Fake {
            sent: Vec::new(),
            replies: replies.into(),
        };
        let (trace, result) = exchange(&mut fake, &USB, &request);
        (fake, trace, result)
    }

    #[test]
    fn returns_first_valid_answer() {
        let req = Request::new(0x1F, DPI, &[0]).unwrap();
        let (fake, trace, result) = run(vec![reply(&req, Status::Ok, 0x1F)]);
        assert_eq!(fake.sent, vec![req.encode().to_vec()]);
        assert_eq!(trace.responses.len(), 1);
        assert_eq!(&result.unwrap().args[..3], &[0x00, 0x06, 0x40]);
    }

    #[test]
    fn polls_through_busy_and_stale_frames() {
        let req = Request::new(0x1F, DPI, &[0]).unwrap();
        let (_, trace, result) = run(vec![
            reply(&req, Status::Busy, 0x1F),
            reply(&req, Status::Ok, 0xFF),
            reply(&req, Status::Ok, 0x1F),
        ]);
        assert!(result.is_ok());
        assert_eq!(trace.responses.len(), 3);
    }

    #[test]
    fn stops_on_failure_status() {
        let req = Request::new(0x1F, DPI, &[0]).unwrap();
        let (_, trace, result) = run(vec![
            reply(&req, Status::NotSupported, 0x1F),
            reply(&req, Status::Ok, 0x1F),
        ]);
        assert!(matches!(
            result,
            Err(ExchangeError::Device(Status::NotSupported))
        ));
        assert_eq!(trace.responses.len(), 1);
    }

    #[test]
    fn gives_up_after_poll_budget() {
        let req = Request::new(0x1F, DPI, &[0]).unwrap();
        let (_, trace, result) = run(vec![reply(&req, Status::Busy, 0x1F); POLLS + 2]);
        assert!(matches!(
            result,
            Err(ExchangeError::NoAnswer(Some(ResponseError::Status(
                Status::Busy
            ))))
        ));
        assert_eq!(trace.responses.len(), POLLS + 1);
    }
}
