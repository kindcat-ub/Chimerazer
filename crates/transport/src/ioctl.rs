//! `HIDIOCSFEATURE` / `HIDIOCGFEATURE` from `<linux/hidraw.h>`.
#![allow(unsafe_code)]

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};

const IOC_READ_WRITE: libc::c_ulong = 3;
const HID_IOC_TYPE: libc::c_ulong = b'H' as libc::c_ulong;

const fn ioc_rw(nr: libc::c_ulong, len: usize) -> libc::c_ulong {
    (IOC_READ_WRITE << 30) | ((len as libc::c_ulong) << 16) | (HID_IOC_TYPE << 8) | nr
}

/// `HIDIOCSFEATURE(len)`.
pub(crate) const fn set_feature_request(len: usize) -> libc::c_ulong {
    ioc_rw(0x06, len)
}

/// `HIDIOCGFEATURE(len)`.
pub(crate) const fn get_feature_request(len: usize) -> libc::c_ulong {
    ioc_rw(0x07, len)
}

fn call(fd: BorrowedFd<'_>, request: libc::c_ulong, buf: &mut [u8]) -> io::Result<usize> {
    // SAFETY: `fd` is a live descriptor for the borrow's duration; the
    // request encodes `buf.len()`, so the kernel reads and writes at most
    // that many bytes of `buf`, which is a valid, exclusive slice.
    // The request parameter type differs between libc flavours (glibc:
    // c_ulong, musl: c_int); `as _` keeps the bit pattern either way.
    let rc = unsafe { libc::ioctl(fd.as_raw_fd(), request as _, buf.as_mut_ptr()) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(rc as usize)
}

/// `buf[0]` is the report ID.
pub(crate) fn set_feature(fd: BorrowedFd<'_>, buf: &mut [u8]) -> io::Result<usize> {
    call(fd, set_feature_request(buf.len()), buf)
}

/// `buf[0]` must hold the report ID on entry; on return the report follows
/// it. Returns the number of bytes written including the report ID.
pub(crate) fn get_feature(fd: BorrowedFd<'_>, buf: &mut [u8]) -> io::Result<usize> {
    call(fd, get_feature_request(buf.len()), buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_numbers_match_kernel_headers() {
        // _IOC(_IOC_WRITE|_IOC_READ, 'H', 0x06/0x07, 91)
        assert_eq!(set_feature_request(91), 0xC05B_4806);
        assert_eq!(get_feature_request(91), 0xC05B_4807);
    }
}
