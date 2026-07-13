//! Pure-Rust parser for the DVD-Video IFO tables we need.
//!
//! Reads raw IFO bytes (obtained via DVDReadBytes) instead of going through
//! libdvdread's packed `ifo_handle_t` structs. All integers are big-endian,
//! all sector pointers are in 2048-byte units. Offsets follow ifo_types.h /
//! the dvd.sourceforge.net DVD-Video spec notes.

use std::error::Error;

pub const SECTOR: usize = 2048;

fn rd_u16(data: &[u8], off: usize) -> Result<u16, Box<dyn Error>> {
    data.get(off..off + 2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
        .ok_or_else(|| format!("IFO truncated at offset {:#x}", off).into())
}

fn rd_u32(data: &[u8], off: usize) -> Result<u32, Box<dyn Error>> {
    data.get(off..off + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| format!("IFO truncated at offset {:#x}", off).into())
}

fn rd_u8(data: &[u8], off: usize) -> Result<u8, Box<dyn Error>> {
    data.get(off)
        .copied()
        .ok_or_else(|| format!("IFO truncated at offset {:#x}", off).into())
}

/// Decode a 4-byte BCD dvd_time_t to seconds. Byte 3 encodes the frame rate
/// in bits 7-6 (0b01 = 25 fps PAL, 0b11 = 29.97 fps NTSC) and BCD frames in
/// bits 5-0.
pub fn decode_dvd_time(t: [u8; 4]) -> f64 {
    fn bcd(b: u8) -> u64 {
        ((b >> 4) as u64) * 10 + (b & 0x0f) as u64
    }
    let secs = bcd(t[0]) * 3600 + bcd(t[1]) * 60 + bcd(t[2]);
    let fps = match t[3] >> 6 {
        0b01 => 25.0,
        0b11 => 30000.0 / 1001.0,
        _ => 25.0, // illegal per spec; PAL is the safer guess
    };
    let frames = bcd(t[3] & 0x3f) as f64;
    secs as f64 + frames / fps
}

// ---------------------------------------------------------------------------
// VMG (VIDEO_TS.IFO)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct VmgTitle {
    /// 1-based VMG title number (what the user selects).
    pub title_nr: u32,
    pub nr_of_angles: u8,
    pub nr_of_ptts: u16,
    /// Which VTS_nn holds this title.
    pub title_set_nr: u8,
    /// 1-based title number inside that VTS.
    pub vts_ttn: u8,
}

/// Parse VIDEO_TS.IFO: the TT_SRPT title table (sector pointer at 0xC4).
pub fn parse_vmg(data: &[u8]) -> Result<Vec<VmgTitle>, Box<dyn Error>> {
    if data.get(..12) != Some(b"DVDVIDEO-VMG") {
        return Err("not a VMG IFO (missing DVDVIDEO-VMG magic)".into());
    }

    let tt_srpt = rd_u32(data, 0xC4)? as usize * SECTOR;
    let count = rd_u16(data, tt_srpt)? as usize;

    let mut titles = Vec::with_capacity(count);
    for i in 0..count {
        let e = tt_srpt + 8 + i * 12;
        titles.push(VmgTitle {
            title_nr: i as u32 + 1,
            nr_of_angles: rd_u8(data, e + 1)?,
            nr_of_ptts: rd_u16(data, e + 2)?,
            title_set_nr: rd_u8(data, e + 6)?,
            vts_ttn: rd_u8(data, e + 7)?,
        });
    }
    Ok(titles)
}

// ---------------------------------------------------------------------------
// VTS (VTS_nn_0.IFO)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCoding {
    Ac3,
    Mpeg1,
    Mpeg2Ext,
    Lpcm,
    Dts,
    Unknown(u8),
}

impl AudioCoding {
    fn from_attr(bits: u8) -> Self {
        match bits {
            0 => Self::Ac3,
            2 => Self::Mpeg1,
            3 => Self::Mpeg2Ext,
            4 => Self::Lpcm,
            6 => Self::Dts,
            other => Self::Unknown(other),
        }
    }

    /// Map onto the Blu-ray HDMV coding-type constants the rest of the
    /// codebase uses for display (see `format_coding_type`).
    pub fn as_bd_coding_type(&self) -> u8 {
        match self {
            Self::Ac3 => 0x81,
            Self::Dts => 0x82,
            Self::Lpcm => 0x80,
            Self::Mpeg1 | Self::Mpeg2Ext => 0x03,
            Self::Unknown(_) => 0x00,
        }
    }
}

/// MPEG-PS stream id for audio substream number `n`, as ffmpeg's mpeg
/// demuxer reports it in `AVStream.id`.
pub fn audio_stream_id(coding: AudioCoding, n: u8) -> u16 {
    match coding {
        AudioCoding::Ac3 => 0x80 + n as u16,
        AudioCoding::Dts => 0x88 + n as u16,
        AudioCoding::Lpcm => 0xA0 + n as u16,
        AudioCoding::Mpeg1 | AudioCoding::Mpeg2Ext => 0x1C0 + n as u16,
        AudioCoding::Unknown(_) => 0x80 + n as u16,
    }
}

/// MPEG-PS stream id for subpicture substream number `n`.
pub fn subp_stream_id(n: u8) -> u16 {
    0x20 + n as u16
}

#[derive(Debug, Clone)]
pub struct AudioAttr {
    pub coding: AudioCoding,
    /// ISO 639-1 two-letter code, if the disc provides one.
    pub lang: Option<String>,
    pub channels: u8,
}

#[derive(Debug, Clone)]
pub struct SubpAttr {
    pub lang: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CellPlayback {
    /// Bits: 0b00 = normal cell, 0b01 = first of angle block,
    /// 0b10 = middle, 0b11 = last. For angle-1 extraction keep 0b00/0b01.
    pub block_mode: u8,
    pub duration_secs: f64,
    pub first_sector: u32,
    pub last_sector: u32,
}

impl CellPlayback {
    /// Whether this cell belongs to angle 1 (or no angle block at all).
    pub fn is_angle1(&self) -> bool {
        self.block_mode <= 0b01
    }
}

#[derive(Debug, Clone)]
pub struct Pgc {
    pub playback_secs: f64,
    /// Per audio-attr-index control word; bit 15 = stream present,
    /// substream number n = (ctrl >> 8) & 7.
    pub audio_control: [u16; 8],
    /// Per subp-attr-index control word; bit 31 = stream present. Substream
    /// numbers per display mode: byte0 = 4:3, byte1 = wide, byte2 =
    /// letterbox, byte3 = pan&scan (each & 0x1F).
    pub subp_control: [u32; 32],
    /// 1-based entry cell number for each program (chapter starts).
    pub program_entry_cells: Vec<u8>,
    pub cells: Vec<CellPlayback>,
}

impl Pgc {
    /// Start time (seconds) of each cell in the angle-1 output timeline
    /// (skipped angle cells contribute no duration).
    pub fn cell_start_times(&self) -> Vec<f64> {
        let mut starts = Vec::with_capacity(self.cells.len());
        let mut acc = 0.0;
        for cell in &self.cells {
            starts.push(acc);
            if cell.is_angle1() {
                acc += cell.duration_secs;
            }
        }
        starts
    }
}

#[derive(Debug, Clone)]
pub struct VtsInfo {
    /// True when the VTS video attribute says 16:9.
    pub widescreen: bool,
    pub audio_attrs: Vec<AudioAttr>,
    pub subp_attrs: Vec<SubpAttr>,
    /// PTT (chapter) table per vts_ttn: `title_ptts[ttn-1]` = [(pgcn, pgn)].
    pub title_ptts: Vec<Vec<(u16, u16)>>,
    /// PGCs by 1-based pgcn: `pgcs[pgcn-1]`.
    pub pgcs: Vec<Pgc>,
}

impl VtsInfo {
    /// The PGC a title plays (v1: the one its first PTT points at).
    pub fn pgc_for_title(&self, vts_ttn: u8) -> Option<(u16, &Pgc)> {
        let ptts = self.title_ptts.get(vts_ttn as usize - 1)?;
        let &(pgcn, _) = ptts.first()?;
        let pgc = self.pgcs.get(pgcn as usize - 1)?;
        Some((pgcn, pgc))
    }
}

fn parse_lang(data: &[u8], off: usize) -> Option<String> {
    let b = data.get(off..off + 2)?;
    if b[0].is_ascii_lowercase() && b[1].is_ascii_lowercase() {
        Some(String::from_utf8_lossy(b).into_owned())
    } else {
        None
    }
}

/// Parse VTS_nn_0.IFO: VTSI_MAT stream attributes, the PTT (chapter) search
/// table and the title PGCs.
pub fn parse_vts(data: &[u8]) -> Result<VtsInfo, Box<dyn Error>> {
    if data.get(..12) != Some(b"DVDVIDEO-VTS") {
        return Err("not a VTS IFO (missing DVDVIDEO-VTS magic)".into());
    }

    // --- VTSI_MAT attributes ---
    let video_attr0 = rd_u8(data, 0x200)?;
    let widescreen = (video_attr0 >> 2) & 0x03 == 0x03;

    let nr_audio = rd_u16(data, 0x202)?.min(8) as usize;
    let mut audio_attrs = Vec::with_capacity(nr_audio);
    for i in 0..nr_audio {
        let a = 0x204 + i * 8;
        let byte0 = rd_u8(data, a)?;
        let byte1 = rd_u8(data, a + 1)?;
        let lang_type = (byte0 >> 2) & 0x03;
        audio_attrs.push(AudioAttr {
            coding: AudioCoding::from_attr(byte0 >> 5),
            lang: if lang_type == 1 { parse_lang(data, a + 2) } else { None },
            channels: (byte1 & 0x07) + 1,
        });
    }

    let nr_subp = rd_u16(data, 0x254)?.min(32) as usize;
    let mut subp_attrs = Vec::with_capacity(nr_subp);
    for i in 0..nr_subp {
        let a = 0x256 + i * 6;
        let byte0 = rd_u8(data, a)?;
        let lang_type = byte0 & 0x03;
        subp_attrs.push(SubpAttr {
            lang: if lang_type == 1 { parse_lang(data, a + 2) } else { None },
        });
    }

    // --- VTS_PTT_SRPT: chapter search pointer table ---
    let ptt_base = rd_u32(data, 0xC8)? as usize * SECTOR;
    let nr_titles = rd_u16(data, ptt_base)? as usize;
    let last_byte = rd_u32(data, ptt_base + 4)? as usize;

    let mut title_ptts = Vec::with_capacity(nr_titles);
    for i in 0..nr_titles {
        let start = rd_u32(data, ptt_base + 8 + i * 4)? as usize;
        let end = if i + 1 < nr_titles {
            rd_u32(data, ptt_base + 8 + (i + 1) * 4)? as usize
        } else {
            last_byte + 1
        };
        let mut ptts = Vec::new();
        let mut off = ptt_base + start;
        let title_end = ptt_base + end;
        while off + 4 <= title_end {
            ptts.push((rd_u16(data, off)?, rd_u16(data, off + 2)?));
            off += 4;
        }
        title_ptts.push(ptts);
    }

    // --- VTS_PGCIT: title program chains ---
    let pgcit_base = rd_u32(data, 0xCC)? as usize * SECTOR;
    let nr_pgcs = rd_u16(data, pgcit_base)? as usize;

    let mut pgcs = Vec::with_capacity(nr_pgcs);
    for i in 0..nr_pgcs {
        let pgc_off = rd_u32(data, pgcit_base + 8 + i * 8 + 4)? as usize;
        pgcs.push(parse_pgc(data, pgcit_base + pgc_off)?);
    }

    Ok(VtsInfo {
        widescreen,
        audio_attrs,
        subp_attrs,
        title_ptts,
        pgcs,
    })
}

fn parse_pgc(data: &[u8], base: usize) -> Result<Pgc, Box<dyn Error>> {
    let nr_of_programs = rd_u8(data, base + 0x02)?;
    let nr_of_cells = rd_u8(data, base + 0x03)?;

    let time = data
        .get(base + 0x04..base + 0x08)
        .ok_or("IFO truncated in PGC playback_time")?;
    let playback_secs = decode_dvd_time([time[0], time[1], time[2], time[3]]);

    let mut audio_control = [0u16; 8];
    for (j, ctrl) in audio_control.iter_mut().enumerate() {
        *ctrl = rd_u16(data, base + 0x0C + j * 2)?;
    }
    let mut subp_control = [0u32; 32];
    for (j, ctrl) in subp_control.iter_mut().enumerate() {
        *ctrl = rd_u32(data, base + 0x1C + j * 4)?;
    }

    let program_map_off = rd_u16(data, base + 0xE6)? as usize;
    let cell_playback_off = rd_u16(data, base + 0xE8)? as usize;

    let mut program_entry_cells = Vec::with_capacity(nr_of_programs as usize);
    for p in 0..nr_of_programs as usize {
        program_entry_cells.push(rd_u8(data, base + program_map_off + p)?);
    }

    let mut cells = Vec::with_capacity(nr_of_cells as usize);
    for c in 0..nr_of_cells as usize {
        let e = base + cell_playback_off + c * 24;
        let cat = rd_u8(data, e)?;
        let time = data
            .get(e + 4..e + 8)
            .ok_or("IFO truncated in cell playback_time")?;
        cells.push(CellPlayback {
            block_mode: cat >> 6,
            duration_secs: decode_dvd_time([time[0], time[1], time[2], time[3]]),
            first_sector: rd_u32(data, e + 8)?,
            last_sector: rd_u32(data, e + 20)?,
        });
    }

    Ok(Pgc {
        playback_secs,
        audio_control,
        subp_control,
        program_entry_cells,
        cells,
    })
}

/// Convert an ISO 639-1 code (as stored on DVDs) to the ISO 639-2 codes used
/// throughout the rest of the codebase (T-form, matching `iso639_to_name`).
pub fn iso639_1_to_2(code: &str) -> String {
    match code {
        "en" => "eng",
        "de" => "deu",
        "fr" => "fra",
        "es" => "spa",
        "it" => "ita",
        "pt" => "por",
        "ru" => "rus",
        "ja" => "jpn",
        "ko" => "kor",
        "zh" => "zho",
        "ar" => "ara",
        "hi" => "hin",
        "nl" => "nld",
        "pl" => "pol",
        "sv" => "swe",
        "no" => "nor",
        "da" => "dan",
        "fi" => "fin",
        "cs" => "ces",
        "hu" => "hun",
        "tr" => "tur",
        "th" => "tha",
        "vi" => "vie",
        "ro" => "ron",
        "el" => "ell",
        "he" | "iw" => "heb",
        "ca" => "cat",
        "bg" => "bul",
        "hr" => "hrv",
        "sk" => "slk",
        "sl" => "slv",
        "sr" => "srp",
        "uk" => "ukr",
        "id" | "in" => "ind",
        "ms" => "msa",
        _ => "und",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dvd_time_pal() {
        // 01:02:03.12 @ 25fps: BCD 0x01 0x02 0x03, frame byte 0b01 << 6 | 0x12
        let secs = decode_dvd_time([0x01, 0x02, 0x03, 0x40 | 0x12]);
        assert!((secs - (3723.0 + 12.0 / 25.0)).abs() < 1e-9);
    }

    #[test]
    fn dvd_time_ntsc() {
        // 00:00:10.15 @ 29.97fps
        let secs = decode_dvd_time([0x00, 0x00, 0x10, 0xC0 | 0x15]);
        assert!((secs - (10.0 + 15.0 / (30000.0 / 1001.0))).abs() < 1e-9);
    }

    /// Build a minimal synthetic VMG IFO with two titles.
    fn synthetic_vmg() -> Vec<u8> {
        let mut d = vec![0u8; 2 * SECTOR + 64];
        d[..12].copy_from_slice(b"DVDVIDEO-VMG");
        // TT_SRPT at sector 2
        d[0xC4..0xC8].copy_from_slice(&2u32.to_be_bytes());
        let t = 2 * SECTOR;
        d[t..t + 2].copy_from_slice(&2u16.to_be_bytes()); // 2 titles
        // title 1: 1 angle, 8 ptts, vts 1, ttn 1
        let e = t + 8;
        d[e + 1] = 1;
        d[e + 2..e + 4].copy_from_slice(&8u16.to_be_bytes());
        d[e + 6] = 1;
        d[e + 7] = 1;
        // title 2: 3 angles, 2 ptts, vts 2, ttn 1
        let e = t + 20;
        d[e + 1] = 3;
        d[e + 2..e + 4].copy_from_slice(&2u16.to_be_bytes());
        d[e + 6] = 2;
        d[e + 7] = 1;
        d
    }

    #[test]
    fn parse_synthetic_vmg() {
        let titles = parse_vmg(&synthetic_vmg()).unwrap();
        assert_eq!(titles.len(), 2);
        assert_eq!(titles[0].title_nr, 1);
        assert_eq!(titles[0].nr_of_ptts, 8);
        assert_eq!(titles[0].title_set_nr, 1);
        assert_eq!(titles[1].nr_of_angles, 3);
        assert_eq!(titles[1].title_set_nr, 2);
    }

    #[test]
    fn vmg_rejects_bad_magic() {
        assert!(parse_vmg(&vec![0u8; 4096]).is_err());
    }

    /// Build a minimal synthetic VTS IFO: 1 title, 1 PGC with 2 programs and
    /// 3 cells (the middle one an angle-2 cell), AC3 German + LPCM audio,
    /// one English subpicture, widescreen.
    fn synthetic_vts() -> Vec<u8> {
        let mut d = vec![0u8; 4 * SECTOR];
        d[..12].copy_from_slice(b"DVDVIDEO-VTS");

        // video attr: 16:9 (aspect bits 3-2 = 0b11)
        d[0x200] = 0b0000_1100;

        // 2 audio streams
        d[0x202..0x204].copy_from_slice(&2u16.to_be_bytes());
        // audio 0: AC3 (format 0), lang_type 1, lang "de"
        d[0x204] = 0b000_0_01_00;
        d[0x204 + 1] = 0x02; // 3 channels
        d[0x204 + 2] = b'd';
        d[0x204 + 3] = b'e';
        // audio 1: LPCM (format 4), lang_type 1, lang "en"
        d[0x20C] = 0b100_0_01_00;
        d[0x20C + 1] = 0x01; // 2 channels
        d[0x20C + 2] = b'e';
        d[0x20C + 3] = b'n';

        // 1 subpicture stream, lang "en"
        d[0x254..0x256].copy_from_slice(&1u16.to_be_bytes());
        d[0x256] = 0x01; // lang_type 1
        d[0x256 + 2] = b'e';
        d[0x256 + 3] = b'n';

        // PTT_SRPT at sector 2: 1 title, 2 PTTs -> (pgc 1, pg 1), (pgc 1, pg 2)
        d[0xC8..0xCC].copy_from_slice(&2u32.to_be_bytes());
        let p = 2 * SECTOR;
        d[p..p + 2].copy_from_slice(&1u16.to_be_bytes());
        d[p + 4..p + 8].copy_from_slice(&19u32.to_be_bytes()); // last byte
        d[p + 8..p + 12].copy_from_slice(&12u32.to_be_bytes()); // title 1 offset
        d[p + 12..p + 14].copy_from_slice(&1u16.to_be_bytes()); // pgcn
        d[p + 14..p + 16].copy_from_slice(&1u16.to_be_bytes()); // pgn
        d[p + 16..p + 18].copy_from_slice(&1u16.to_be_bytes());
        d[p + 18..p + 20].copy_from_slice(&2u16.to_be_bytes());

        // PGCIT at sector 3: 1 PGC at offset 16
        d[0xCC..0xD0].copy_from_slice(&3u32.to_be_bytes());
        let g = 3 * SECTOR;
        d[g..g + 2].copy_from_slice(&1u16.to_be_bytes());
        d[g + 8 + 4..g + 8 + 8].copy_from_slice(&16u32.to_be_bytes());

        let pgc = g + 16;
        d[pgc + 2] = 2; // programs
        d[pgc + 3] = 3; // cells
        // playback 00:01:00.00 PAL
        d[pgc + 4..pgc + 8].copy_from_slice(&[0x00, 0x01, 0x00, 0x40]);
        // audio_control[0]: present, substream 0
        d[pgc + 0x0C..pgc + 0x0E].copy_from_slice(&0x8000u16.to_be_bytes());
        // audio_control[1]: present, substream 1
        d[pgc + 0x0E..pgc + 0x10].copy_from_slice(&0x8100u16.to_be_bytes());
        // subp_control[0]: present, wide stream 2
        d[pgc + 0x1C..pgc + 0x20].copy_from_slice(&0x8002_0000u32.to_be_bytes());
        // program map at +0x100, cell playback at +0x104
        d[pgc + 0xE6..pgc + 0xE8].copy_from_slice(&0x100u16.to_be_bytes());
        d[pgc + 0xE8..pgc + 0xEA].copy_from_slice(&0x104u16.to_be_bytes());
        d[pgc + 0x100] = 1; // program 1 -> cell 1
        d[pgc + 0x101] = 3; // program 2 -> cell 3

        // cells: 30s normal, 20s angle-middle (skipped), 30s normal
        let cases: [(u8, [u8; 4], u32, u32); 3] = [
            (0b00, [0x00, 0x00, 0x30, 0x40], 0, 99),
            (0b10, [0x00, 0x00, 0x20, 0x40], 100, 149),
            (0b00, [0x00, 0x00, 0x30, 0x40], 150, 299),
        ];
        for (c, (mode, time, first, last)) in cases.iter().enumerate() {
            let e = pgc + 0x104 + c * 24;
            d[e] = mode << 6;
            d[e + 4..e + 8].copy_from_slice(time);
            d[e + 8..e + 12].copy_from_slice(&first.to_be_bytes());
            d[e + 20..e + 24].copy_from_slice(&last.to_be_bytes());
        }
        d
    }

    #[test]
    fn parse_synthetic_vts() {
        let vts = parse_vts(&synthetic_vts()).unwrap();

        assert!(vts.widescreen);

        assert_eq!(vts.audio_attrs.len(), 2);
        assert_eq!(vts.audio_attrs[0].coding, AudioCoding::Ac3);
        assert_eq!(vts.audio_attrs[0].lang.as_deref(), Some("de"));
        assert_eq!(vts.audio_attrs[0].channels, 3);
        assert_eq!(vts.audio_attrs[1].coding, AudioCoding::Lpcm);
        assert_eq!(vts.audio_attrs[1].lang.as_deref(), Some("en"));

        assert_eq!(vts.subp_attrs.len(), 1);
        assert_eq!(vts.subp_attrs[0].lang.as_deref(), Some("en"));

        assert_eq!(vts.title_ptts.len(), 1);
        assert_eq!(vts.title_ptts[0], vec![(1, 1), (1, 2)]);

        let (pgcn, pgc) = vts.pgc_for_title(1).unwrap();
        assert_eq!(pgcn, 1);
        assert_eq!(pgc.playback_secs, 60.0);
        assert_eq!(pgc.program_entry_cells, vec![1, 3]);
        assert_eq!(pgc.cells.len(), 3);
        assert!(pgc.cells[0].is_angle1());
        assert!(!pgc.cells[1].is_angle1());

        // audio control: substream numbers 0 and 1
        assert_eq!((pgc.audio_control[0] >> 8) & 0x07, 0);
        assert_eq!((pgc.audio_control[1] >> 8) & 0x07, 1);

        // cell start times skip the angle-2 cell's duration
        let starts = pgc.cell_start_times();
        assert_eq!(starts, vec![0.0, 30.0, 30.0]);
    }

    #[test]
    fn stream_ids() {
        assert_eq!(audio_stream_id(AudioCoding::Ac3, 0), 0x80);
        assert_eq!(audio_stream_id(AudioCoding::Dts, 1), 0x89);
        assert_eq!(audio_stream_id(AudioCoding::Lpcm, 0), 0xA0);
        assert_eq!(audio_stream_id(AudioCoding::Mpeg1, 2), 0x1C2);
        assert_eq!(subp_stream_id(3), 0x23);
    }

    #[test]
    fn lang_mapping() {
        assert_eq!(iso639_1_to_2("de"), "deu");
        assert_eq!(iso639_1_to_2("en"), "eng");
        assert_eq!(iso639_1_to_2("xx"), "und");
    }
}
