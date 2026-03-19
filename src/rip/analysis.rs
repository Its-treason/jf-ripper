use std::ffi::CStr;

use libbluray_sys::{
    bd_free_title_info, bd_get_main_title, bd_get_title_info, bd_get_titles, bd_open,
};

#[derive(Debug, Clone)]
pub struct AnalysedTitle {
    pub index: u32,
    pub playlist: u32,
    pub duration_secs: u64,
    pub chapter_count: u32,
    pub chapters: Vec<ChapterMark>,
    pub audio_streams: Vec<AudioStreamInfo>,
    pub subtitle_streams: Vec<SubtitleStreamInfo>,
    pub score: i32,
}

#[derive(Debug, Clone)]
pub struct ChapterMark {
    pub index: u32,
    pub start_ticks: u64,
    pub duration_ticks: u64,
}

#[derive(Debug, Clone)]
pub struct AudioStreamInfo {
    pub index_in_clip: usize,
    pub language: String,
    pub coding_type: u8,
}

#[derive(Debug, Clone)]
pub struct SubtitleStreamInfo {
    pub index_in_clip: usize,
    pub language: String,
}

#[derive(Debug, Clone)]
pub enum DiscType {
    Movie { title_idx: u32 },
    Show { episode_indices: Vec<u32> },
}

#[derive(Debug, Clone)]
pub struct TitleAnalysis {
    pub titles: Vec<AnalysedTitle>,
    pub main_title_idx: u32,
    pub detected_type: DiscType,
}

fn lang_from_bytes(lang: &[u8; 4]) -> String {
    let end = lang.iter().position(|&b| b == 0).unwrap_or(lang.len());
    std::str::from_utf8(&lang[..end])
        .unwrap_or("und")
        .to_string()
}

pub fn analyse_disc(bd_path: &str) -> Result<TitleAnalysis, Box<dyn std::error::Error>> {
    let bd_path_c = format!("{}\0", bd_path);
    let device = CStr::from_bytes_with_nul(bd_path_c.as_bytes())?;

    unsafe {
        let bd = bd_open(device.as_ptr(), std::ptr::null());
        if bd.is_null() {
            return Err(format!("Failed to open Blu-ray device at '{}' (no disc?)", bd_path).into());
        }

        let total_titles = bd_get_titles(bd, 3, 10);
        let main_title = bd_get_main_title(bd) as u32;

        let mut titles = Vec::new();

        for i in 0..total_titles {
            let info = bd_get_title_info(bd, i, 0);
            if info.is_null() {
                continue;
            }
            let t = &*info;

            let duration_secs = t.duration / 90_000;

            // Score
            let mut score: i32 = 0;
            if t.idx == main_title {
                score += 5;
            }
            if duration_secs < 300 {
                score -= 1;
            } else if duration_secs > 600 {
                score += 1;
            }

            // Chapters
            let chapters = if t.chapter_count > 0 && !t.chapters.is_null() {
                let ch_slice = std::slice::from_raw_parts(t.chapters, t.chapter_count as usize);
                ch_slice
                    .iter()
                    .map(|ch| ChapterMark {
                        index: ch.idx,
                        start_ticks: ch.start,
                        duration_ticks: ch.duration,
                    })
                    .collect()
            } else {
                Vec::new()
            };

            // Audio and subtitle streams from first clip
            let mut audio_streams = Vec::new();
            let mut subtitle_streams = Vec::new();

            if t.clip_count > 0 && !t.clips.is_null() {
                let clip = &*t.clips;

                if clip.audio_stream_count > 0 && !clip.audio_streams.is_null() {
                    let streams = std::slice::from_raw_parts(
                        clip.audio_streams,
                        clip.audio_stream_count as usize,
                    );
                    for (idx, s) in streams.iter().enumerate() {
                        audio_streams.push(AudioStreamInfo {
                            index_in_clip: idx,
                            language: lang_from_bytes(&s.lang),
                            coding_type: s.coding_type,
                        });
                    }
                }

                if clip.pg_stream_count > 0 && !clip.pg_streams.is_null() {
                    let streams = std::slice::from_raw_parts(
                        clip.pg_streams,
                        clip.pg_stream_count as usize,
                    );
                    for (idx, s) in streams.iter().enumerate() {
                        subtitle_streams.push(SubtitleStreamInfo {
                            index_in_clip: idx,
                            language: lang_from_bytes(&s.lang),
                        });
                    }
                }
            }

            titles.push(AnalysedTitle {
                index: t.idx,
                playlist: t.playlist as u32,
                duration_secs,
                chapter_count: t.chapter_count,
                chapters,
                audio_streams,
                subtitle_streams,
                score,
            });

            bd_free_title_info(info);
        }

        // Sort by score descending, then index ascending
        titles.sort_by(|a, b| b.score.cmp(&a.score).then(a.index.cmp(&b.index)));

        let detected_type = detect_disc_type(&titles, main_title);

        Ok(TitleAnalysis {
            titles,
            main_title_idx: main_title,
            detected_type,
        })
    }
}

fn detect_disc_type(titles: &[AnalysedTitle], main_title: u32) -> DiscType {
    // Collect candidates with duration > 15 minutes
    let candidates: Vec<&AnalysedTitle> = titles
        .iter()
        .filter(|t| t.duration_secs > 15 * 60)
        .collect();

    if candidates.len() < 2 {
        return DiscType::Movie {
            title_idx: main_title,
        };
    }

    // Find the largest group of titles with similar duration and matching audio languages
    let mut best_group: Vec<u32> = Vec::new();

    for a in &candidates {
        let mut group = vec![a.index];
        let a_dur = a.duration_secs as f64;
        let a_langs = audio_language_key(&a.audio_streams);

        for b in &candidates {
            if a.index == b.index {
                continue;
            }
            let b_dur = b.duration_secs as f64;
            // Within 15% duration
            if a_dur > b_dur * 1.15 || a_dur < b_dur * 0.85 {
                continue;
            }
            // Same audio language set
            let b_langs = audio_language_key(&b.audio_streams);
            if a_langs != b_langs {
                continue;
            }
            group.push(b.index);
        }

        if group.len() > best_group.len() {
            best_group = group;
        }
    }

    if best_group.len() < 2 {
        return DiscType::Movie {
            title_idx: main_title,
        };
    }

    // Deduplicate if > 3 titles have the same duration (Blu-ray duplicates)
    if best_group.len() > 3 {
        let mut seen_durations: Vec<u64> = Vec::new();
        best_group.retain(|&idx| {
            if let Some(t) = titles.iter().find(|t| t.index == idx) {
                if seen_durations.contains(&t.duration_secs) {
                    return false;
                }
                seen_durations.push(t.duration_secs);
            }
            true
        });
    }

    best_group.sort();

    if best_group.len() < 2 {
        DiscType::Movie {
            title_idx: main_title,
        }
    } else {
        DiscType::Show {
            episode_indices: best_group,
        }
    }
}

fn audio_language_key(streams: &[AudioStreamInfo]) -> Vec<String> {
    let mut langs: Vec<String> = streams
        .iter()
        .filter(|s| s.language != "und")
        .map(|s| s.language.clone())
        .collect();
    langs.sort();
    langs.dedup();
    langs
}
