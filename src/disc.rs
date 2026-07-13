use std::error::Error;
use std::ffi::CString;
use std::fmt;

use libbluray_sys::{bd_close, bd_get_disc_info, bd_open};

use crate::dvd::ffi::{ifoClose, ifoOpen, DVDClose, DVDOpen};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscFormat {
    BluRay,
    Dvd,
}

impl fmt::Display for DiscFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BluRay => write!(f, "Blu-ray"),
            Self::Dvd => write!(f, "DVD"),
        }
    }
}

/// Detect whether the device/ISO/directory at `path` holds a Blu-ray or a
/// DVD-Video disc. `bd_open` succeeds on non-BD media too, so the
/// `bluray_detected` flag — not open success — is the Blu-ray signal.
pub fn detect_disc_format(path: &str) -> Result<DiscFormat, Box<dyn Error>> {
    let c_path = CString::new(path)?;

    unsafe {
        let bd = bd_open(c_path.as_ptr(), std::ptr::null());
        if !bd.is_null() {
            let info = bd_get_disc_info(bd);
            let is_bluray = !info.is_null() && (*info).bluray_detected != 0;
            bd_close(bd);
            if is_bluray {
                return Ok(DiscFormat::BluRay);
            }
        }

        let dvd = DVDOpen(c_path.as_ptr());
        if !dvd.is_null() {
            let ifo = ifoOpen(dvd, 0);
            let is_dvd = !ifo.is_null();
            if is_dvd {
                ifoClose(ifo);
            }
            DVDClose(dvd);
            if is_dvd {
                return Ok(DiscFormat::Dvd);
            }
        }
    }

    Err(format!("No Blu-ray or DVD video disc detected at '{}'", path).into())
}
