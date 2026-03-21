use ffmpeg_next::{
    codec, format, frame, rescale::Rescale, software::scaling, Dictionary, Error, Packet, Rational,
};

use super::error::TranscodeError;

pub struct VideoTranscoder {
    pub decoder: codec::decoder::Video,
    pub encoder: codec::encoder::video::Encoder,
    pub scaler: Option<scaling::Context>,
    pub in_tb: Rational,
    /// Set after write_header() by reading back the muxer-chosen time base
    pub out_tb: Rational,
    pub out_stream_idx: usize,
    /// First PTS seen, used to normalize timestamps to start from 0
    first_pts: Option<i64>,
    /// PTS discontinuity correction
    expected_next_pts: i64,
    pts_correction: i64,
    frame_duration: Option<i64>,
}

impl VideoTranscoder {
    pub fn new(
        in_stream: &ffmpeg_next::Stream,
        codec_name: &str,
        crf: Option<u32>,
        preset: Option<&str>,
        extra_options: &[(String, String)],
        out_stream_idx: usize,
        needs_global_header: bool,
    ) -> Result<Self, TranscodeError> {
        // Open decoder
        let dec_ctx = codec::Context::from_parameters(in_stream.parameters())?;
        let decoder = dec_ctx.decoder().video()?;

        let target_format = format::Pixel::YUV420P;

        // Build scaler if source pixel format differs from target
        let scaler = if decoder.format() != target_format {
            Some(
                scaling::Context::get(
                    decoder.format(),
                    decoder.width(),
                    decoder.height(),
                    target_format,
                    decoder.width(),
                    decoder.height(),
                    scaling::Flags::BILINEAR,
                )
                .map_err(TranscodeError::Ffmpeg)?,
            )
        } else {
            None
        };

        // Find encoder codec
        let codec = ffmpeg_next::encoder::find_by_name(codec_name)
            .ok_or_else(|| TranscodeError::EncoderNotFound(codec_name.to_string()))?;

        let mut enc_ctx = codec::Context::new_with_codec(codec).encoder().video()?;

        enc_ctx.set_width(decoder.width());
        enc_ctx.set_height(decoder.height());
        enc_ctx.set_format(target_format);
        enc_ctx.set_time_base(in_stream.time_base());
        enc_ctx.set_frame_rate(decoder.frame_rate());

        // Copy color metadata from input to encoder
        unsafe {
            let in_par = *(*in_stream.as_ptr()).codecpar;
            let enc = enc_ctx.as_mut_ptr();
            (*enc).color_primaries = in_par.color_primaries;
            (*enc).color_trc = in_par.color_trc;
            (*enc).colorspace = in_par.color_space;
            (*enc).color_range = in_par.color_range;
        }

        if needs_global_header {
            unsafe {
                let flags = (*enc_ctx.as_mut_ptr()).flags;
                (*enc_ctx.as_mut_ptr()).flags =
                    flags | ffmpeg_next::ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
            }
        }

        let mut dict = Dictionary::new();
        if let Some(crf) = crf {
            dict.set("crf", &crf.to_string());
        }
        if let Some(preset) = preset {
            dict.set("preset", preset);
        }
        for (k, v) in extra_options {
            dict.set(k, v);
        }

        let encoder = enc_ctx.open_as_with(codec, dict)?;

        // Compute expected frame duration in input timebase units from frame rate
        let frame_duration = decoder.frame_rate().and_then(|fr| {
            if fr.0 > 0 && fr.1 > 0 {
                let tb = in_stream.time_base();
                Some((tb.1 as i64 * fr.1 as i64) / (tb.0 as i64 * fr.0 as i64))
            } else {
                None
            }
        });

        Ok(Self {
            decoder,
            encoder,
            scaler,
            in_tb: in_stream.time_base(),
            out_tb: Rational(1, 1), // updated after write_header
            out_stream_idx,
            first_pts: None,
            expected_next_pts: 0,
            pts_correction: 0,
            frame_duration,
        })
    }

    /// Feed an encoded input packet to the decoder.
    pub fn send_packet(&mut self, packet: &Packet) -> Result<(), TranscodeError> {
        self.decoder.send_packet(packet)?;
        Ok(())
    }

    /// Drain all available decoded frames through the encoder and return encoded packets.
    pub fn receive_packets(&mut self) -> Result<Vec<Packet>, TranscodeError> {
        let mut output = Vec::new();
        let mut decoded = frame::Video::empty();

        while self.decoder.receive_frame(&mut decoded).is_ok() {
            let frame_to_encode = self.prepare_frame(&mut decoded)?;
            self.encoder.send_frame(&frame_to_encode)?;
            self.drain_encoder(&mut output)?;
        }

        Ok(output)
    }

    /// Flush the encoder and return remaining packets.
    pub fn flush(&mut self) -> Result<Vec<Packet>, TranscodeError> {
        self.decoder.send_eof()?;

        let mut output = Vec::new();
        let mut decoded = frame::Video::empty();
        while self.decoder.receive_frame(&mut decoded).is_ok() {
            let frame_to_encode = self.prepare_frame(&mut decoded)?;
            self.encoder.send_frame(&frame_to_encode)?;
            self.drain_encoder(&mut output)?;
        }

        self.encoder.send_eof()?;
        self.drain_encoder(&mut output)?;

        Ok(output)
    }

    /// Normalize PTS, apply scaler if needed, copy color metadata, and clear pict_type.
    fn prepare_frame(&mut self, decoded: &mut frame::Video) -> Result<frame::Video, TranscodeError> {
        // Normalize PTS to start from 0, detect discontinuities, then rescale
        if let Some(pts) = decoded.pts() {
            let first = *self.first_pts.get_or_insert(pts);
            let adjusted = self.adjust_pts(pts - first);
            let rescaled = adjusted.rescale(self.in_tb, self.encoder.time_base());
            decoded.set_pts(Some(rescaled));
        }

        let mut frame_to_encode = if let Some(scaler) = &mut self.scaler {
            let mut converted = frame::Video::empty();
            scaler.run(decoded, &mut converted).map_err(TranscodeError::Ffmpeg)?;
            converted.set_pts(decoded.pts());
            // Copy color metadata from decoded frame to converted frame
            unsafe {
                let src = *decoded.as_ptr();
                let dst = converted.as_mut_ptr();
                (*dst).color_primaries = src.color_primaries;
                (*dst).color_trc = src.color_trc;
                (*dst).colorspace = src.colorspace;
                (*dst).color_range = src.color_range;
            }
            converted
        } else {
            decoded.clone()
        };

        // Clear picture type so the encoder decides frame types on its own
        // instead of inheriting forced IDR flags from the source stream.
        unsafe {
            (*frame_to_encode.as_mut_ptr()).pict_type =
                ffmpeg_next::ffi::AVPictureType::AV_PICTURE_TYPE_NONE;
        }

        Ok(frame_to_encode)
    }

    /// Detect and correct PTS discontinuities. Returns corrected PTS (relative to first_pts=0).
    fn adjust_pts(&mut self, raw_pts: i64) -> i64 {
        let corrected = raw_pts + self.pts_correction;

        // Detect discontinuity: if the gap between expected and actual exceeds 1 second
        // in timebase units, apply a correction.
        if self.expected_next_pts != 0 {
            let diff = corrected - self.expected_next_pts;
            // 1 second threshold in timebase units: tb.1 / tb.0
            let one_sec = self.in_tb.1 as i64 / self.in_tb.0.max(1) as i64;
            if diff.abs() > one_sec {
                eprintln!(
                    "[video] PTS discontinuity: expected {}, got {} (diff {}), correcting",
                    self.expected_next_pts, corrected, diff
                );
                self.pts_correction -= diff;
                let corrected = raw_pts + self.pts_correction;
                if let Some(dur) = self.frame_duration {
                    self.expected_next_pts = corrected + dur;
                }
                return corrected;
            }
        }

        if let Some(dur) = self.frame_duration {
            self.expected_next_pts = corrected + dur;
        }

        corrected
    }

    fn drain_encoder(&mut self, output: &mut Vec<Packet>) -> Result<(), TranscodeError> {
        let mut pkt = Packet::empty();
        loop {
            match self.encoder.receive_packet(&mut pkt) {
                Ok(()) => {
                    pkt.rescale_ts(self.encoder.time_base(), self.out_tb);
                    pkt.set_stream(self.out_stream_idx);
                    output.push(pkt.clone());
                }
                Err(Error::Other { .. }) => break,
                Err(Error::Eof) => break,
                Err(e) => return Err(TranscodeError::Ffmpeg(e)),
            }
        }
        Ok(())
    }
}
