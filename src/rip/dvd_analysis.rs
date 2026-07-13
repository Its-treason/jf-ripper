use std::collections::HashMap;
use std::error::Error;

use crate::disc::DiscFormat;
use crate::dvd::ifo::{self, VtsInfo};
use crate::dvd::{subp_stream_number, Dvd};

use super::analysis::{
    score_and_detect, AnalysedTitle, AudioStreamInfo, ChapterMark, SubtitleStreamInfo,
    TitleAnalysis,
};

/// Analyse a DVD into the same `TitleAnalysis` shape the Blu-ray path
/// produces, so the TUI, stream matching, and transcode pipeline work
/// unchanged. Stream "PIDs" are MPEG-PS stream ids (what ffmpeg reports in
/// `AVStream.id` for .vob input) and chapter ticks are 90 kHz.
pub fn analyse_dvd(
    dvd_path: &str,
    player_language: &str,
) -> Result<TitleAnalysis, Box<dyn Error>> {
    let dvd = Dvd::open(dvd_path)?;
    let vmg = dvd.vmg()?;
    if vmg.is_empty() {
        return Err("DVD has no titles".into());
    }

    let mut vts_cache: HashMap<u8, VtsInfo> = HashMap::new();
    let mut titles = Vec::new();

    for entry in &vmg {
        let vts = match vts_cache.entry(entry.title_set_nr) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => match dvd.vts(entry.title_set_nr) {
                Ok(v) => e.insert(v),
                Err(err) => {
                    eprintln!(
                        "Warning: skipping title {} (VTS {} unreadable: {})",
                        entry.title_nr, entry.title_set_nr, err
                    );
                    continue;
                }
            },
        };

        let Some((_, pgc)) = vts.pgc_for_title(entry.vts_ttn) else {
            continue;
        };

        titles.push(AnalysedTitle {
            index: entry.title_nr,
            playlist: entry.title_set_nr as u32,
            duration_secs: pgc.playback_secs as u64,
            chapter_count: pgc.program_entry_cells.len() as u32,
            chapters: build_chapters(pgc),
            audio_streams: build_audio_streams(vts, pgc),
            subtitle_streams: build_subtitle_streams(vts, pgc),
            score: 0,
        });
    }

    if titles.is_empty() {
        return Err("No playable titles found on DVD".into());
    }

    // DVDs have no "main title" marker; the longest title is the best guess.
    let main_title = titles
        .iter()
        .max_by_key(|t| t.duration_secs)
        .map(|t| t.index)
        .unwrap();

    let detected_type = score_and_detect(&mut titles, main_title, player_language);

    Ok(TitleAnalysis {
        titles,
        main_title_idx: main_title,
        detected_type,
        format: DiscFormat::Dvd,
    })
}

/// Chapters are the PGC's programs; start times come from the entry cell's
/// position on the angle-1 timeline, expressed in 90 kHz ticks like Blu-ray.
fn build_chapters(pgc: &ifo::Pgc) -> Vec<ChapterMark> {
    let cell_starts = pgc.cell_start_times();

    let starts: Vec<f64> = pgc
        .program_entry_cells
        .iter()
        .map(|&cell_nr| {
            cell_starts
                .get(cell_nr as usize - 1)
                .copied()
                .unwrap_or(0.0)
        })
        .collect();

    starts
        .iter()
        .enumerate()
        .map(|(i, &start)| {
            let end = starts.get(i + 1).copied().unwrap_or(pgc.playback_secs);
            ChapterMark {
                index: i as u32,
                start_ticks: (start * 90_000.0) as u64,
                duration_ticks: ((end - start).max(0.0) * 90_000.0) as u64,
            }
        })
        .collect()
}

fn build_audio_streams(vts: &VtsInfo, pgc: &ifo::Pgc) -> Vec<AudioStreamInfo> {
    let mut streams = Vec::new();
    for (i, attr) in vts.audio_attrs.iter().enumerate() {
        let ctrl = pgc.audio_control[i];
        if ctrl & 0x8000 == 0 {
            continue;
        }
        let n = ((ctrl >> 8) & 0x07) as u8;
        streams.push(AudioStreamInfo {
            index_in_clip: streams.len(),
            pid: ifo::audio_stream_id(attr.coding, n),
            language: attr
                .lang
                .as_deref()
                .map(ifo::iso639_1_to_2)
                .unwrap_or_else(|| "und".to_string()),
            coding_type: attr.coding.as_bd_coding_type(),
        });
    }
    streams
}

fn build_subtitle_streams(vts: &VtsInfo, pgc: &ifo::Pgc) -> Vec<SubtitleStreamInfo> {
    let mut streams = Vec::new();
    for (i, attr) in vts.subp_attrs.iter().enumerate() {
        let ctrl = pgc.subp_control[i];
        if ctrl & 0x8000_0000 == 0 {
            continue;
        }
        let n = subp_stream_number(ctrl, vts.widescreen);
        streams.push(SubtitleStreamInfo {
            index_in_clip: streams.len(),
            pid: ifo::subp_stream_id(n),
            language: attr
                .lang
                .as_deref()
                .map(ifo::iso639_1_to_2)
                .unwrap_or_else(|| "und".to_string()),
            forced: false,
        });
    }
    streams
}
