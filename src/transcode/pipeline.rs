use ffmpeg_next::{codec, format, media, Dictionary, Rational};

use super::{
    audio::AudioTranscoder,
    config::{AudioAction, ChapterInfo, SubtitleConfig, VideoConfig, VideoCodec},
    error::TranscodeError,
    video::VideoTranscoder,
};

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

    let mut ictx = format::input(&input_path)?;
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

                let out_stream = octx.add_stream_with(vt.encoder.as_ref())?;
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
        out_stream.set_metadata(stream_metadata(&sub_cfg.language, &sub_cfg.name));
        let out_idx = out_stream.index();
        actions[src] = StreamAction::CopySubtitle { out_idx };
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

    // --- Main packet loop ---
    for (in_stream, mut packet) in ictx.packets() {
        let idx = in_stream.index();
        match &actions[idx] {
            StreamAction::CopyVideo { out_idx }
            | StreamAction::CopyAudio { out_idx }
            | StreamAction::CopySubtitle { out_idx } => {
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
