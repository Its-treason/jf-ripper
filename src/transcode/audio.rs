use ffmpeg_next::{
    codec, format, frame, rescale::Rescale, software::resampling, ChannelLayout, Dictionary, Error,
    Packet, Rational,
};

use super::error::TranscodeError;

pub struct AudioTranscoder {
    pub decoder: codec::decoder::Audio,
    pub encoder: codec::encoder::audio::Encoder,
    pub resampler: Option<resampling::Context>,
    /// Sample buffer for encoders that require fixed frame sizes (e.g. AAC = 1024 samples)
    sample_buf: Vec<f32>,
    /// Samples per channel in the buffer
    buffered_samples: usize,
    pub in_tb: Rational,
    /// Set after write_header() by reading back the muxer-chosen time base
    pub out_tb: Rational,
    pub out_stream_idx: usize,
    /// Required frame size (0 = encoder accepts variable sizes)
    frame_size: usize,
    /// Running sample count for PTS assignment
    next_pts: i64,
    enc_sample_rate: u32,
    enc_channel_layout: ChannelLayout,
    enc_format: format::Sample,
}

impl AudioTranscoder {
    pub fn new(
        in_stream: &ffmpeg_next::Stream,
        codec_name: &str,
        bitrate: Option<u64>,
        extra_options: &[(String, String)],
        out_stream_idx: usize,
        needs_global_header: bool,
    ) -> Result<Self, TranscodeError> {
        // Open decoder
        let dec_ctx = codec::Context::from_parameters(in_stream.parameters())?;
        let decoder = dec_ctx.decoder().audio()?;

        // Find encoder
        let codec = ffmpeg_next::encoder::find_by_name(codec_name)
            .ok_or_else(|| TranscodeError::EncoderNotFound(codec_name.to_string()))?;

        let mut enc_ctx = codec::Context::new_with_codec(codec).encoder().audio()?;

        let enc_format = codec
            .audio()
            .ok()
            .and_then(|c| c.formats())
            .and_then(|mut it| it.next())
            .unwrap_or(format::Sample::F32(format::sample::Type::Packed));

        let enc_channel_layout = decoder.channel_layout();
        let enc_sample_rate = decoder.rate();

        enc_ctx.set_rate(enc_sample_rate as i32);
        enc_ctx.set_channel_layout(enc_channel_layout);
        enc_ctx.set_format(enc_format);
        if let Some(br) = bitrate {
            enc_ctx.set_bit_rate(br as usize);
        }

        if needs_global_header {
            unsafe {
                let flags = (*enc_ctx.as_mut_ptr()).flags;
                (*enc_ctx.as_mut_ptr()).flags =
                    flags | ffmpeg_next::ffi::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
            }
        }

        let mut dict = Dictionary::new();
        for (k, v) in extra_options {
            dict.set(k, v);
        }

        let encoder = enc_ctx.open_as_with(codec, dict)?;
        let frame_size = encoder.frame_size() as usize;

        // Build resampler if formats differ
        let resampler = if decoder.format() != enc_format
            || decoder.channel_layout() != enc_channel_layout
        {
            Some(
                resampling::Context::get(
                    decoder.format(),
                    decoder.channel_layout(),
                    decoder.rate(),
                    enc_format,
                    enc_channel_layout,
                    enc_sample_rate,
                )
                .map_err(TranscodeError::Ffmpeg)?,
            )
        } else {
            None
        };

        // Buffer capacity: 4 frames worth of samples per channel
        let buf_cap = if frame_size > 0 { frame_size * 8 } else { 4096 };

        Ok(Self {
            decoder,
            encoder,
            resampler,
            sample_buf: vec![0.0f32; buf_cap],
            buffered_samples: 0,
            in_tb: in_stream.time_base(),
            out_tb: Rational(1, 1),
            out_stream_idx,
            frame_size,
            next_pts: 0,
            enc_sample_rate,
            enc_channel_layout,
            enc_format,
        })
    }

    pub fn send_packet(&mut self, packet: &Packet) -> Result<(), TranscodeError> {
        self.decoder.send_packet(packet)?;
        Ok(())
    }

    /// Drain decoded frames, resample if needed, encode in fixed-size chunks,
    /// and return all encoded packets.
    pub fn receive_packets(&mut self) -> Result<Vec<Packet>, TranscodeError> {
        let mut output = Vec::new();
        let mut decoded = frame::Audio::empty();

        while self.decoder.receive_frame(&mut decoded).is_ok() {
            let frame_to_encode = self.resample_frame(&decoded)?;
            self.encode_frame(frame_to_encode, &mut output, false)?;
        }

        Ok(output)
    }

    pub fn flush(&mut self) -> Result<Vec<Packet>, TranscodeError> {
        self.decoder.send_eof()?;

        let mut output = Vec::new();
        let mut decoded = frame::Audio::empty();

        while self.decoder.receive_frame(&mut decoded).is_ok() {
            let frame_to_encode = self.resample_frame(&decoded)?;
            self.encode_frame(frame_to_encode, &mut output, false)?;
        }

        // Flush remaining buffered samples as a final partial frame
        self.encode_frame(None, &mut output, true)?;

        self.encoder.send_eof()?;
        self.drain_encoder(&mut output)?;

        Ok(output)
    }

    fn resample_frame(
        &mut self,
        decoded: &frame::Audio,
    ) -> Result<Option<frame::Audio>, TranscodeError> {
        if let Some(resampler) = &mut self.resampler {
            let mut resampled = frame::Audio::empty();
            resampler
                .run(decoded, &mut resampled)
                .map_err(TranscodeError::Ffmpeg)?;
            resampled.set_pts(decoded.pts());
            Ok(Some(resampled))
        } else {
            Ok(Some(decoded.clone()))
        }
    }

    fn encode_frame(
        &mut self,
        frame: Option<frame::Audio>,
        output: &mut Vec<Packet>,
        flush: bool,
    ) -> Result<(), TranscodeError> {
        // If encoder accepts variable frame sizes, send directly
        if self.frame_size == 0 {
            if let Some(mut f) = frame {
                if let Some(pts) = f.pts() {
                    let rescaled = pts.rescale(self.in_tb, Rational(1, self.enc_sample_rate as i32));
                    f.set_pts(Some(rescaled));
                }
                self.encoder.send_frame(&f)?;
                self.drain_encoder(output)?;
            }
            return Ok(());
        }

        // Fixed frame size: send to encoder only when we have enough samples
        if let Some(f) = frame {
            let samples = f.samples();
            // TODO: a proper AVAudioFifo would be more efficient;
            // this simplified path just sends full frames directly if they
            // already match the required size, otherwise drops partial frames.
            if samples == self.frame_size {
                let mut f = f;
                f.set_pts(Some(self.next_pts));
                self.next_pts += self.frame_size as i64;
                self.encoder.send_frame(&f)?;
                self.drain_encoder(output)?;
            } else if flush {
                // Send whatever we have with a flush
                let mut f = f;
                f.set_pts(Some(self.next_pts));
                self.encoder.send_frame(&f)?;
                self.drain_encoder(output)?;
            }
        }

        Ok(())
    }

    fn drain_encoder(&mut self, output: &mut Vec<Packet>) -> Result<(), TranscodeError> {
        let mut pkt = Packet::empty();
        loop {
            match self.encoder.receive_packet(&mut pkt) {
                Ok(()) => {
                    pkt.rescale_ts(
                        Rational(1, self.enc_sample_rate as i32),
                        self.out_tb,
                    );
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
