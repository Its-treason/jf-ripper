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

        Ok(Self {
            decoder,
            encoder,
            scaler,
            in_tb: in_stream.time_base(),
            out_tb: Rational(1, 1), // updated after write_header
            out_stream_idx,
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
            // Rescale frame PTS to encoder time base
            if let Some(pts) = decoded.pts() {
                let rescaled = pts.rescale(self.in_tb, self.encoder.time_base());
                decoded.set_pts(Some(rescaled));
            }

            let frame_to_encode = if let Some(scaler) = &mut self.scaler {
                let mut converted = frame::Video::empty();
                scaler.run(&decoded, &mut converted).map_err(TranscodeError::Ffmpeg)?;
                converted.set_pts(decoded.pts());
                converted
            } else {
                decoded.clone()
            };

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
            if let Some(pts) = decoded.pts() {
                let rescaled = pts.rescale(self.in_tb, self.encoder.time_base());
                decoded.set_pts(Some(rescaled));
            }

            let frame_to_encode = if let Some(scaler) = &mut self.scaler {
                let mut converted = frame::Video::empty();
                scaler.run(&decoded, &mut converted).map_err(TranscodeError::Ffmpeg)?;
                converted.set_pts(decoded.pts());
                converted
            } else {
                decoded.clone()
            };

            self.encoder.send_frame(&frame_to_encode)?;
            self.drain_encoder(&mut output)?;
        }

        self.encoder.send_eof()?;
        self.drain_encoder(&mut output)?;

        Ok(output)
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
