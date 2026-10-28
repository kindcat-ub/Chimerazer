use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};

use crate::{FeatureIo, ioctl};

/// An open hidraw node, held under an exclusive advisory lock so that two
/// Chimerazer processes never interleave commands on one device.
#[derive(Debug)]
pub struct Hidraw {
    file: File,
    path: PathBuf,
}

impl Hidraw {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        file.try_lock().map_err(|e| match e {
            std::fs::TryLockError::WouldBlock => io::Error::new(
                io::ErrorKind::WouldBlock,
                format!("{} is in use by another process", path.display()),
            ),
            std::fs::TryLockError::Error(e) => e,
        })?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl FeatureIo for Hidraw {
    fn send_feature(&mut self, report: &[u8]) -> io::Result<()> {
        let mut buf = Vec::with_capacity(report.len() + 1);
        buf.push(0); // report ID
        buf.extend_from_slice(report);
        let sent = ioctl::set_feature(self.file.as_fd(), &mut buf)?;
        if sent != buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!("short feature write: {sent} of {}", buf.len()),
            ));
        }
        Ok(())
    }

    fn get_feature(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut raw = vec![0; buf.len() + 1]; // raw[0] = report ID 0
        let got = ioctl::get_feature(self.file.as_fd(), &mut raw)?;
        let data = got.saturating_sub(1).min(buf.len());
        buf[..data].copy_from_slice(&raw[1..=data]);
        Ok(data)
    }
}
