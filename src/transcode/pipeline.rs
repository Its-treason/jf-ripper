use ffmpeg_next::{codec, format, media, rescale::Rescale, Dictionary, Rational};

use super::{
    audio::AudioTranscoder,
    config::{AudioAction, ChapterInfo, SubtitleConfig, VideoConfig, VideoCodec},
    error::TranscodeError,
    video::VideoTranscoder,
};

/// Tracks PTS for copy streams to detect and correct discontinuities.
struct CopyPtsTracker {
    /// Single normalization offset applied to both PTS and DTS, anchored on
    /// the first timestamp seen. Using one offset preserves pts >= dts for
    /// B-frame streams, which separate per-field offsets would break.
    offset: Option<i64>,
    expected_next_pts: i64,
    pts_correction: i64,
    last_pts: i64,
    /// Approximate time base denominator / numerator for 1-second threshold
    one_sec_threshold: i64,
}

impl CopyPtsTracker {
    fn new(time_base: Rational) -> Self {
        Self {
            offset: None,
            expected_next_pts: 0,
            pts_correction: 0,
            last_pts: 0,
            one_sec_threshold: time_base.1 as i64 / time_base.0.max(1) as i64,
        }
    }

    /// Adjust PTS/DTS for a copy-mode packet. Returns (adjusted_pts, adjusted_dts).
    fn adjust(&mut self, pts: Option<i64>, dts: Option<i64>, is_audio: bool) -> (Option<i64>, Option<i64>) {
        let offset = match self.offset {
            Some(o) => o,
            None => match dts.or(pts) {
                Some(anchor) => *self.offset.insert(anchor),
                None => return (pts, dts),
            },
        };

        let raw_pts = match pts {
            Some(p) => p,
            // No PTS on this packet (common in DVD MPEG-PS): still normalize
            // the DTS so it stays consistent with neighboring packets.
            None => return (None, dts.map(|d| d - offset + self.pts_correction)),
        };

        let normalized_pts = raw_pts - offset + self.pts_correction;

        // Detect discontinuity
        if self.expected_next_pts != 0 {
            let diff = normalized_pts - self.expected_next_pts;
            // For audio: only detect backward jumps (forward gaps may be intentional silence)
            let is_discontinuity = if is_audio {
                diff < -self.one_sec_threshold
            } else {
                diff.abs() > self.one_sec_threshold
            };

            if is_discontinuity {
                eprintln!(
                    "[copy] PTS discontinuity: expected {}, got {} (diff {}), correcting",
                    self.expected_next_pts, normalized_pts, diff
                );
                self.pts_correction -= diff;
                let normalized_pts = raw_pts - offset + self.pts_correction;
                self.last_pts = normalized_pts;
                self.expected_next_pts = 0; // reset; will be set from next packet
                let adjusted_dts = dts.map(|d| d - offset + self.pts_correction);
                return (Some(normalized_pts), adjusted_dts);
            }
        }

        self.last_pts = normalized_pts;
        self.expected_next_pts = normalized_pts; // next packet should be >= this

        let adjusted_dts = dts.map(|d| d - offset + self.pts_correction);
        (Some(normalized_pts), adjusted_dts)
    }
}

/// Build a stream metadata Dictionary from optional language and name fields.
fn stream_metadata<'a>(language: &'a Option<String>, name: &'a Option<String>) -> Dictionary<'a> {
    let mut dict = Dictionary::new();
    if let Some(lang) = language {
        dict.set("language", lang);
    }
    if let Some(title) = name {
        dict.set("title", title);
    }
    dict
}

/// What to do with each input stream packet in the main loop
enum StreamAction {
    CopyVideo { out_idx: usize },
    CopyAudio { out_idx: usize },
    CopySubtitle { out_idx: usize },
    EncodeVideo,
    EncodeAudio { transcoder_idx: usize },
    Drop,
}

pub fn run(
    input_path: &str,
    output_path: &str,
    container_format: &str,
    video_cfg: VideoConfig,
    audio_cfgs: &[super::config::AudioConfig],
    subtitle_cfgs: &[SubtitleConfig],
    chapters: &[ChapterInfo],
    metadata: &[(String, String)],
) -> Result<(), TranscodeError> {
    ffmpeg_next::init()?;

    // DVD MPEG-PS leaves PTS unset on many packets; have the demuxer generate
    // them from DTS (same as ffmpeg's -fflags +genpts). No effect on m2ts.
    let mut in_opts = Dictionary::new();
    in_opts.set("fflags", "+genpts");
    let mut ictx = format::input_with_dictionary(&input_path, in_opts)?;
    let mut octx = format::output_as(&output_path, container_format)?;

    let needs_global_header = octx
        .format()
        .flags()
        .contains(format::format::flag::Flags::GLOBAL_HEADER);

    // Determine which input stream is the video source
    let video_src_idx = match video_cfg.source_stream {
        Some(idx) => Some(idx),
        None => ictx
            .streams()
            .best(media::Type::Video)
            .map(|s| s.index()),
    };

    // Build the stream action table and open transcoders
    let nb_streams = ictx.nb_streams() as usize;
    let mut actions: Vec<StreamAction> = (0..nb_streams).map(|_| StreamAction::Drop).collect();

    let mut video_transcoder: Option<VideoTranscoder> = None;
    let mut audio_transcoders: Vec<AudioTranscoder> = Vec::new();

    // --- Video stream ---
    if let Some(vsrc) = video_src_idx {
        let in_stream = ictx.stream(vsrc).ok_or_else(|| {
            TranscodeError::InvalidConfig(format!("Video source stream {} not found", vsrc))
        })?;

        match video_cfg.codec {
            VideoCodec::Copy => {
                let mut out_stream = octx.add_stream(codec::encoder::find(codec::Id::None))?;
                out_stream.set_parameters(in_stream.parameters());
                // Clear container-specific codec tags (e.g. "HDMV" from m2ts) that
                // are not valid in MKV/MP4; 0 tells the muxer to pick the right tag.
                // Muxers read the stream-level aspect ratio (matroska derives
                // DisplayWidth/Height from it), so carry it over from the input.
                unsafe {
                    let out = out_stream.as_mut_ptr();
                    (*(*out).codecpar).codec_tag = 0;
                    let in_sar = (*in_stream.as_ptr()).sample_aspect_ratio;
                    (*out).sample_aspect_ratio = if in_sar.num > 0 {
                        in_sar
                    } else {
                        (*(*out).codecpar).sample_aspect_ratio
                    };
                }
                let out_idx = out_stream.index();
                actions[vsrc] = StreamAction::CopyVideo { out_idx };
            }
            ref encode_codec => {
                let codec_name = encode_codec.encoder_name().unwrap();
                let vt = VideoTranscoder::new(
                    &in_stream,
                    codec_name,
                    video_cfg.crf,
                    video_cfg.preset.as_deref(),
                    &video_cfg.extra_options,
                    0, // placeholder; set below after add_stream_with
                    needs_global_header,
                )?;

                let mut out_stream = octx.add_stream_with(vt.encoder.as_ref())?;
                // Muxers read the stream-level aspect ratio, not the codec-level
                // one: matroska derives DisplayWidth/Height from it. Without it
                // players that trust the container (mpv, Jellyfin) show 5:4 for
                // anamorphic DVDs even though the H264 VUI carries the SAR.
                unsafe {
                    (*out_stream.as_mut_ptr()).sample_aspect_ratio =
                        (*vt.encoder.as_ptr()).sample_aspect_ratio;
                }
                let out_idx = out_stream.index();

                let mut vt = vt;
                vt.out_stream_idx = out_idx;
                video_transcoder = Some(vt);
                actions[vsrc] = StreamAction::EncodeVideo;
            }
        }
    }

    // --- Audio streams ---
    for audio_cfg in audio_cfgs {
        let src = audio_cfg.source_stream;
        let in_stream = ictx.stream(src).ok_or_else(|| {
            TranscodeError::InvalidConfig(format!("Audio source stream {} not found", src))
        })?;

        match &audio_cfg.action {
            AudioAction::Copy => {
                let mut out_stream = octx.add_stream(codec::encoder::find(codec::Id::None))?;
                out_stream.set_parameters(in_stream.parameters());
                // Clear container-specific codec tags (e.g. DTS-HD's from m2ts)
                // that are not valid in MKV/MP4, same as the video copy path.
                unsafe {
                    (*(*out_stream.as_mut_ptr()).codecpar).codec_tag = 0;
                }
                out_stream.set_metadata(stream_metadata(&audio_cfg.language, &audio_cfg.name));
                let out_idx = out_stream.index();
                actions[src] = StreamAction::CopyAudio { out_idx };
            }
            AudioAction::Encode { codec_name, bitrate, extra_options } => {
                let transcoder_idx = audio_transcoders.len();
                let at = AudioTranscoder::new(
                    &in_stream,
                    codec_name,
                    *bitrate,
                    extra_options,
                    0, // placeholder
                    needs_global_header,
                )?;

                let mut out_stream = octx.add_stream_with(at.encoder.as_ref())?;
                out_stream.set_metadata(stream_metadata(&audio_cfg.language, &audio_cfg.name));
                let out_idx = out_stream.index();

                let mut at = at;
                at.out_stream_idx = out_idx;
                audio_transcoders.push(at);
                actions[src] = StreamAction::EncodeAudio { transcoder_idx };
            }
        }
    }

    // --- Subtitle streams (copy only) ---
    for sub_cfg in subtitle_cfgs {
        let src = sub_cfg.source_stream;
        let in_stream = ictx.stream(src).ok_or_else(|| {
            TranscodeError::InvalidConfig(format!("Subtitle source stream {} not found", src))
        })?;

        let mut out_stream = octx.add_stream(codec::encoder::find(codec::Id::None))?;
        out_stream.set_parameters(in_stream.parameters());
        unsafe {
            (*(*out_stream.as_mut_ptr()).codecpar).codec_tag = 0;
            if sub_cfg.forced {
                (*out_stream.as_mut_ptr()).disposition =
                    ffmpeg_next::ffi::AV_DISPOSITION_FORCED as i32;
            }
        }
        out_stream.set_metadata(stream_metadata(&sub_cfg.language, &sub_cfg.name));
        let out_idx = out_stream.index();
        actions[src] = StreamAction::CopySubtitle { out_idx };
    }

    // --- MP4 codec tags ---
    // Set proper codec tags for MP4 containers (e.g. 'avc1' instead of generic H264).
    // Without these, some players may not recognize the streams correctly.
    if container_format == "mp4" {
        for i in 0..octx.nb_streams() {
            let stream = octx.stream(i as usize).unwrap();
            unsafe {
                let codecpar = (*stream.as_ptr()).codecpar;
                let codec_id: codec::Id = (*codecpar).codec_id.into();
                if let Some(tag) = mp4_codec_tag(codec_id) {
                    (*(codecpar as *mut ffmpeg_next::ffi::AVCodecParameters)).codec_tag = tag;
                }
            }
        }
    }

    // --- Chapters ---
    for ch in chapters {
        octx.add_chapter(ch.id, Rational(1, 1000), ch.start_ms, ch.end_ms, &ch.title)?;
    }

    // --- Global metadata ---
    if !metadata.is_empty() {
        let mut dict = Dictionary::new();
        for (k, v) in metadata {
            dict.set(k, v);
        }
        octx.set_metadata(dict);
    }

    // --- Write header (muxer may change stream time bases here) ---
    octx.write_header()?;

    // Fix up the output time bases now that the muxer has settled them
    for action in &mut actions {
        match action {
            StreamAction::CopyVideo { out_idx }
            | StreamAction::CopyAudio { out_idx }
            | StreamAction::CopySubtitle { out_idx } => {
                let _ = out_idx; // time base handled inline in packet loop
            }
            _ => {}
        }
    }
    if let Some(vt) = &mut video_transcoder {
        vt.out_tb = octx.stream(vt.out_stream_idx).unwrap().time_base();
    }
    for at in &mut audio_transcoders {
        at.out_tb = octx.stream(at.out_stream_idx).unwrap().time_base();
    }

    // --- PTS trackers for copy streams ---
    let mut copy_pts_trackers: Vec<Option<CopyPtsTracker>> = (0..nb_streams).map(|_| None).collect();
    for (idx, action) in actions.iter().enumerate() {
        match action {
            StreamAction::CopyVideo { .. }
            | StreamAction::CopyAudio { .. }
            | StreamAction::CopySubtitle { .. } => {
                let tb = ictx.stream(idx).unwrap().time_base();
                copy_pts_trackers[idx] = Some(CopyPtsTracker::new(tb));
            }
            _ => {}
        }
    }

    // --- Main packet loop ---
    for (in_stream, mut packet) in ictx.packets() {
        let idx = in_stream.index();
        match &actions[idx] {
            StreamAction::CopyVideo { out_idx } => {
                if let Some(tracker) = &mut copy_pts_trackers[idx] {
                    let (adj_pts, adj_dts) = tracker.adjust(packet.pts(), packet.dts(), false);
                    let out_tb = octx.stream(*out_idx).unwrap().time_base();
                    let in_tb = in_stream.time_base();
                    if let Some(pts) = adj_pts {
                        packet.set_pts(Some(pts.rescale(in_tb, out_tb)));
                    }
                    if let Some(dts) = adj_dts {
                        packet.set_dts(Some(dts.rescale(in_tb, out_tb)));
                    }
                    packet.set_stream(*out_idx);
                    packet.write_interleaved(&mut octx)?;
                }
            }
            StreamAction::CopyAudio { out_idx } => {
                if let Some(tracker) = &mut copy_pts_trackers[idx] {
                    let (adj_pts, adj_dts) = tracker.adjust(packet.pts(), packet.dts(), true);
                    let out_tb = octx.stream(*out_idx).unwrap().time_base();
                    let in_tb = in_stream.time_base();
                    if let Some(pts) = adj_pts {
                        packet.set_pts(Some(pts.rescale(in_tb, out_tb)));
                    }
                    if let Some(dts) = adj_dts {
                        packet.set_dts(Some(dts.rescale(in_tb, out_tb)));
                    }
                    packet.set_stream(*out_idx);
                    packet.write_interleaved(&mut octx)?;
                }
            }
            StreamAction::CopySubtitle { out_idx } => {
                let out_tb = octx.stream(*out_idx).unwrap().time_base();
                packet.rescale_ts(in_stream.time_base(), out_tb);
                packet.set_stream(*out_idx);
                packet.write_interleaved(&mut octx)?;
            }
            StreamAction::EncodeVideo => {
                if let Some(vt) = &mut video_transcoder {
                    vt.send_packet(&packet)?;
                    for mut pkt in vt.receive_packets()? {
                        pkt.set_stream(vt.out_stream_idx);
                        pkt.write_interleaved(&mut octx)?;
                    }
                }
            }
            StreamAction::EncodeAudio { transcoder_idx } => {
                let idx = *transcoder_idx;
                let at = &mut audio_transcoders[idx];
                at.send_packet(&packet)?;
                let out_idx = at.out_stream_idx;
                for mut pkt in at.receive_packets()? {
                    pkt.set_stream(out_idx);
                    pkt.write_interleaved(&mut octx)?;
                }
            }
            StreamAction::Drop => {}
        }
    }

    // --- Flush encoders ---
    if let Some(vt) = &mut video_transcoder {
        let out_idx = vt.out_stream_idx;
        for mut pkt in vt.flush()? {
            pkt.set_stream(out_idx);
            pkt.write_interleaved(&mut octx)?;
        }
    }
    for at in &mut audio_transcoders {
        let out_idx = at.out_stream_idx;
        for mut pkt in at.flush()? {
            pkt.set_stream(out_idx);
            pkt.write_interleaved(&mut octx)?;
        }
    }

    octx.write_trailer()?;

    Ok(())
}

/// Return the MP4-specific codec tag for common codecs.
/// These ensure proper identification in MP4/M4V containers.
fn mp4_codec_tag(codec_id: codec::Id) -> Option<u32> {
    match codec_id {
        codec::Id::H264 => Some(0x31637661), // 'avc1'
        codec::Id::HEVC => Some(0x31637668), // 'hvc1'
        codec::Id::AC3 => Some(0x332D6361),  // 'ac-3'
        codec::Id::EAC3 => Some(0x332D6365), // 'ec-3'
        codec::Id::AAC => Some(0x6134706D),  // 'mp4a'
        _ => None,
    }
}
