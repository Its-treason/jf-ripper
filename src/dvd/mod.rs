pub mod ffi;
pub mod ifo;

use std::error::Error;
use std::ffi::CString;
use std::fs::File;
use std::io::Write;

use ffi::*;

pub struct Dvd {
    reader: *mut dvd_reader_t,
}

impl Dvd {
    pub fn open(path: &str) -> Result<Self, Box<dyn Error>> {
        warn_if_no_dvdcss();
        let c_path = CString::new(path)?;
        let reader = unsafe { DVDOpen(c_path.as_ptr()) };
        if reader.is_null() {
            return Err(format!("Failed to open DVD at '{}' (no disc?)", path).into());
        }
        Ok(Self { reader })
    }

    /// Read a whole IFO file (title_set 0 = VIDEO_TS.IFO, n = VTS_nn_0.IFO).
    pub fn read_ifo(&self, title_set: i32) -> Result<Vec<u8>, Box<dyn Error>> {
        let file = DvdFile::open(self, title_set, DVD_READ_INFO_FILE)
            .ok_or_else(|| format!("Failed to open IFO for title set {}", title_set))?;

        let blocks = unsafe { DVDFileSize(file.0) };
        if blocks <= 0 {
            return Err(format!("IFO for title set {} has no size", title_set).into());
        }

        let size = blocks as usize * DVD_BLOCK_SIZE;
        let mut buf = vec![0u8; size];
        let read = unsafe { DVDReadBytes(file.0, buf.as_mut_ptr() as *mut _, size) };
        if read <= 0 {
            return Err(format!("Failed to read IFO for title set {}", title_set).into());
        }
        buf.truncate(read as usize);
        Ok(buf)
    }

    pub fn vmg(&self) -> Result<Vec<ifo::VmgTitle>, Box<dyn Error>> {
        ifo::parse_vmg(&self.read_ifo(0)?)
    }

    pub fn vts(&self, title_set: u8) -> Result<ifo::VtsInfo, Box<dyn Error>> {
        ifo::parse_vts(&self.read_ifo(title_set as i32)?)
    }
}

impl Drop for Dvd {
    fn drop(&mut self) {
        unsafe { DVDClose(self.reader) };
    }
}

struct DvdFile(*mut dvd_file_t);

impl DvdFile {
    fn open(dvd: &Dvd, title_set: i32, domain: i32) -> Option<Self> {
        let f = unsafe { DVDOpenFile(dvd.reader, title_set, domain) };
        if f.is_null() { None } else { Some(Self(f)) }
    }
}

impl Drop for DvdFile {
    fn drop(&mut self) {
        unsafe { DVDCloseFile(self.0) };
    }
}

/// libdvdread silently returns scrambled data for CSS discs when libdvdcss
/// isn't loadable — probe for it once so the user gets a warning instead of
/// a corrupt rip.
fn warn_if_no_dvdcss() {
    let name = CString::new("libdvdcss.so.2").unwrap();
    let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY) };
    if handle.is_null() {
        eprintln!(
            "Warning: libdvdcss.so.2 not found — CSS-encrypted DVDs will produce corrupt rips"
        );
    } else {
        unsafe { libc::dlclose(handle) };
    }
}

/// Read one DVD title's angle-1 cells into a single MPEG-PS .vob file.
/// Same signature as `bluray::read_title`.
pub fn read_title(title: u32, out_path: &str, dvd_path: &str) -> Result<u64, Box<dyn Error>> {
    const CHUNK_BLOCKS: usize = 4096; // 8 MB

    let dvd = Dvd::open(dvd_path)?;
    let vmg = dvd.vmg()?;
    let entry = vmg
        .iter()
        .find(|t| t.title_nr == title)
        .ok_or_else(|| format!("Title {} not found (disc has {})", title, vmg.len()))?;

    let vts = dvd.vts(entry.title_set_nr)?;
    let (_, pgc) = vts
        .pgc_for_title(entry.vts_ttn)
        .ok_or_else(|| format!("No PGC found for title {}", title))?;

    let vobs = DvdFile::open(&dvd, entry.title_set_nr as i32, DVD_READ_TITLE_VOBS)
        .ok_or_else(|| format!("Failed to open VOBs for title set {}", entry.title_set_nr))?;

    let mut file = File::create(out_path)?;
    let mut buf = vec![0u8; CHUNK_BLOCKS * DVD_BLOCK_SIZE];
    let mut total_bytes: u64 = 0;

    for cell in pgc.cells.iter().filter(|c| c.is_angle1()) {
        let mut sector = cell.first_sector;
        while sector <= cell.last_sector {
            let count = CHUNK_BLOCKS.min((cell.last_sector - sector + 1) as usize);
            let read = unsafe { DVDReadBlocks(vobs.0, sector as i32, count, buf.as_mut_ptr()) };
            if read <= 0 {
                return Err(format!("DVDReadBlocks failed at sector {}", sector).into());
            }
            let bytes = read as usize * DVD_BLOCK_SIZE;
            file.write_all(&buf[..bytes])?;
            total_bytes += bytes as u64;
            sector += read as u32;
        }
    }

    println!("Finished reading");
    Ok(total_bytes)
}

fn format_secs(secs: f64) -> String {
    let s = secs as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

pub fn disc_info(dvd_path: &str) -> Result<(), Box<dyn Error>> {
    let dvd = Dvd::open(dvd_path)?;
    let vmg = dvd.vmg()?;

    let title_sets: std::collections::HashSet<u8> =
        vmg.iter().map(|t| t.title_set_nr).collect();

    println!("DVD-Video Info:");
    println!("  Titles: {}", vmg.len());
    println!("  Title sets (VTS): {}", title_sets.len());
    for t in &vmg {
        println!(
            "  Title {:>2}: VTS {}  angles={}  chapters={}",
            t.title_nr, t.title_set_nr, t.nr_of_angles, t.nr_of_ptts
        );
    }
    Ok(())
}

pub fn list_titles(dvd_path: &str) -> Result<(), Box<dyn Error>> {
    let dvd = Dvd::open(dvd_path)?;
    let vmg = dvd.vmg()?;

    for entry in &vmg {
        let vts = match dvd.vts(entry.title_set_nr) {
            Ok(v) => v,
            Err(e) => {
                println!("Title {:>3}  (failed to read VTS {}: {})", entry.title_nr, entry.title_set_nr, e);
                continue;
            }
        };
        let Some((pgcn, pgc)) = vts.pgc_for_title(entry.vts_ttn) else {
            println!("Title {:>3}  (no PGC)", entry.title_nr);
            continue;
        };

        println!(
            "Title {:>3}  vts={:02} pgc={:02}  duration={}  angles={}  chapters={}  cells={}",
            entry.title_nr,
            entry.title_set_nr,
            pgcn,
            format_secs(pgc.playback_secs),
            entry.nr_of_angles,
            entry.nr_of_ptts,
            pgc.cells.len(),
        );

        for (i, attr) in vts.audio_attrs.iter().enumerate() {
            let ctrl = pgc.audio_control[i];
            if ctrl & 0x8000 == 0 {
                continue;
            }
            let n = ((ctrl >> 8) & 0x07) as u8;
            println!(
                "    Audio  codec={:?}  lang={}  channels={}  id=0x{:04x}",
                attr.coding,
                attr.lang.as_deref().unwrap_or("??"),
                attr.channels,
                ifo::audio_stream_id(attr.coding, n),
            );
        }

        for (i, attr) in vts.subp_attrs.iter().enumerate() {
            let ctrl = pgc.subp_control[i];
            if ctrl & 0x8000_0000 == 0 {
                continue;
            }
            let n = subp_stream_number(ctrl, vts.widescreen);
            println!(
                "    Subtitle  lang={}  id=0x{:04x}",
                attr.lang.as_deref().unwrap_or("??"),
                ifo::subp_stream_id(n),
            );
        }
        println!();
    }
    Ok(())
}

/// Pick the substream number from a PGC subp_control word for the disc's
/// display mode: byte1 = widescreen, byte0 (bits 28-24) = 4:3.
pub fn subp_stream_number(ctrl: u32, widescreen: bool) -> u8 {
    if widescreen {
        ((ctrl >> 16) & 0x1F) as u8
    } else {
        ((ctrl >> 24) & 0x1F) as u8
    }
}
