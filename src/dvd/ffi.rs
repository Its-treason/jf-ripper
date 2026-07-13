//! Minimal hand-written libdvdread bindings.
//!
//! Only opaque pointers are used — `ifo_handle_t` in particular is a large
//! packed struct we never dereference (IFO data is parsed from raw bytes in
//! `ifo.rs` instead). Signatures match /usr/include/dvdread/dvd_reader.h and
//! ifo_read.h (libdvdread 8.x, API stable since 4.x).

use std::os::raw::{c_char, c_int, c_void};

#[repr(C)]
pub struct dvd_reader_t {
    _private: [u8; 0],
}

#[repr(C)]
pub struct dvd_file_t {
    _private: [u8; 0],
}

#[repr(C)]
pub struct ifo_handle_t {
    _private: [u8; 0],
}

/// dvd_read_domain_t
pub const DVD_READ_INFO_FILE: c_int = 0;
pub const DVD_READ_TITLE_VOBS: c_int = 3;

/// DVD sector size; DVDReadBlocks/DVDFileSize operate in these units.
pub const DVD_BLOCK_SIZE: usize = 2048;

#[link(name = "dvdread")]
unsafe extern "C" {
    pub fn DVDOpen(path: *const c_char) -> *mut dvd_reader_t;
    pub fn DVDClose(dvd: *mut dvd_reader_t);

    pub fn DVDOpenFile(
        dvd: *mut dvd_reader_t,
        titlenum: c_int,
        domain: c_int,
    ) -> *mut dvd_file_t;
    pub fn DVDCloseFile(file: *mut dvd_file_t);

    /// Reads `block_count` blocks at block `offset` (title-set relative).
    /// Returns blocks read or -1. CSS decryption happens here when libdvdcss
    /// is available.
    pub fn DVDReadBlocks(
        file: *mut dvd_file_t,
        offset: c_int,
        block_count: usize,
        data: *mut u8,
    ) -> isize;

    /// Byte-wise read (info files only), advances the file position.
    pub fn DVDReadBytes(file: *mut dvd_file_t, data: *mut c_void, size: usize) -> isize;

    /// File size in blocks.
    pub fn DVDFileSize(file: *mut dvd_file_t) -> isize;

    /// Used only as a "is this a DVD-Video" probe; the handle is never
    /// dereferenced on the Rust side.
    pub fn ifoOpen(dvd: *mut dvd_reader_t, title: c_int) -> *mut ifo_handle_t;
    pub fn ifoClose(ifo: *mut ifo_handle_t);
}
