//! Linux hidraw transport.
//!
//! Finds HID interfaces through sysfs (no libudev) and exchanges feature
//! reports through the hidraw ioctls. Knows nothing about the Razer protocol:
//! it moves byte buffers.

mod descriptor;
mod discovery;
mod hidraw;
mod ioctl;

pub use descriptor::feature_report_len;
pub use discovery::{HidInterface, enumerate, enumerate_in};
pub use hidraw::Hidraw;

/// Moves feature reports to and from a device. Implemented by [`Hidraw`] and
/// by test fakes.
pub trait FeatureIo {
    /// Sends `report` as feature report ID 0.
    fn send_feature(&mut self, report: &[u8]) -> std::io::Result<()>;

    /// Reads feature report ID 0 into `buf`, returning the number of bytes
    /// written, report data only (without the report ID byte).
    fn get_feature(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
}
