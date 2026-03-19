use std::ffi::c_void;

use ffmpeg_next::{
    codec, format, frame, rescale::Rescale, software::resampling, ChannelLayout, Dictionary, Error,
    Packet, Rational,
};
use libc::c_int;

use super::error::TranscodeError;

// AVAudioFifo is not exposed by ffmpeg-sys-next, so we declare the opaque type
// and the functions we need ourselves.
#[repr(C)]
struct AVAudioFifo {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    fn av_audio_fifo_alloc(
        sample_fmt: ffmpeg_next::ffi::AVSampleFormat,
        channels: c_int,
        nb_samples: c_int,
    ) -> *mut AVAudioFifo;

    fn av_audio_fifo_free(af: *mut AVAudioFifo);

    fn av_audio_fifo_write(
        af: *mut AVAudioFifo,
        data: *const *mut c_void,
        nb_samples: c_int,
    ) -> c_int;

    fn av_audio_fifo_read(
        af: *mut AVAudioFifo,
        data: *const *mut c_void,
        nb_samples: c_int,
    ) -> c_int;

    fn av_audio_fifo_size(af: *const AVAudioFifo) -> c_int;
}

/// Safe wrapper around AVAudioFifo.
struct AudioFifo {
    ptr: *mut AVAudioFifo,
}

unsafe impl Send for AudioFifo {}

impl AudioFifo {
    fn new(
        format: ffmpeg_next::ffi::AVSampleFormat,
        channels: i32,
        initial_size: i32,
    ) -> Result<Self, TranscodeError> {
        let ptr = unsafe { av_audio_fifo_alloc(format, channels, initial_size) };
        if ptr.is_null() {
            return Err(TranscodeError::Ffmpeg(ffmpeg_next::Error::Unknown));
        }
        Ok(Self { ptr })
    }

    fn write(&mut self, frame: &frame::Audio) -> Result<(), TranscodeError> {
        let planes = frame.planes();
        let ptrs: Vec<*mut c_void> = (0..planes)
            .map(|i| unsafe { (*frame.as_ptr()).data[i] as *mut c_void })
            .collect();

        let result =
            unsafe { av_audio_fifo_write(self.ptr, ptrs.as_ptr(), frame.samples() as c_int) };
        if result < 0 {
            return Err(TranscodeError::Ffmpeg(ffmpeg_next::Error::from(result)));
        }
        Ok(())
    }

    fn read(&mut self, frame: &mut frame::Audio, nb_samples: i32) -> Result<i32, TranscodeError> {
        let planes = frame.planes();
        let ptrs: Vec<*mut c_void> = (0..planes)
            .map(|i| unsafe { (*frame.as_mut_ptr()).data[i] as *mut c_void })
            .collect();

        let result = unsafe { av_audio_fifo_read(self.ptr, ptrs.as_ptr(), nb_samples) };
        if result < 0 {
            return Err(TranscodeError::Ffmpeg(ffmpeg_next::Error::from(result)));
        }
        Ok(result)
    }

    fn size(&self) -> i32 {
        unsafe { av_audio_fifo_size(self.ptr) }
    }
}

impl Drop for AudioFifo {
    fn drop(&mut self) {
        unsafe { av_audio_fifo_free(self.ptr) };
    }
}

pub struct AudioTranscoder {
    pub decoder: codec::decoder::Audio,
    pub encoder: codec::encoder::audio::Encoder,
    pub resampler: Option<resampling::Context>,
    fifo: AudioFifo,
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

        let channels = enc_channel_layout.channels() as i32;
        let fifo = AudioFifo::new(
            enc_format.into(),
            channels,
            frame_size.max(1) as i32,
        )?;

        Ok(Self {
            decoder,
            encoder,
            resampler,
            fifo,
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
        match self.decoder.send_packet(packet) {
            // TrueHD (and some other codecs) can return AVERROR_INVALIDDATA for
            // initial sync / header-only packets; skip them rather than aborting.
            Err(Error::InvalidData) => Ok(()),
            result => Ok(result?),
        }
    }

    /// Drain decoded frames, resample if needed, buffer into the FIFO, and
    /// encode whenever enough samples are available.
    pub fn receive_packets(&mut self) -> Result<Vec<Packet>, TranscodeError> {
        let mut output = Vec::new();
        let mut decoded = frame::Audio::empty();

        while self.decoder.receive_frame(&mut decoded).is_ok() {
            let frame = self.resample_frame(&decoded)?;
            self.buffer_and_encode(Some(frame), &mut output, false)?;
        }

        Ok(output)
    }

    pub fn flush(&mut self) -> Result<Vec<Packet>, TranscodeError> {
        self.decoder.send_eof()?;

        let mut output = Vec::new();
        let mut decoded = frame::Audio::empty();

        while self.decoder.receive_frame(&mut decoded).is_ok() {
            let frame = self.resample_frame(&decoded)?;
            self.buffer_and_encode(Some(frame), &mut output, false)?;
        }

        // Drain any remaining samples in the FIFO
        self.buffer_and_encode(None, &mut output, true)?;

        self.encoder.send_eof()?;
        self.drain_encoder(&mut output)?;

        Ok(output)
    }

    fn resample_frame(&mut self, decoded: &frame::Audio) -> Result<frame::Audio, TranscodeError> {
        if let Some(resampler) = &mut self.resampler {
            let mut resampled = frame::Audio::empty();
            resampler
                .run(decoded, &mut resampled)
                .map_err(TranscodeError::Ffmpeg)?;
            resampled.set_pts(decoded.pts());
            Ok(resampled)
        } else {
            Ok(decoded.clone())
        }
    }

    fn buffer_and_encode(
        &mut self,
        frame: Option<frame::Audio>,
        output: &mut Vec<Packet>,
        flush: bool,
    ) -> Result<(), TranscodeError> {
        // Variable frame size: bypass the FIFO and send directly
        if self.frame_size == 0 {
            if let Some(mut f) = frame {
                if let Some(pts) = f.pts() {
                    let rescaled =
                        pts.rescale(self.in_tb, Rational(1, self.enc_sample_rate as i32));
                    f.set_pts(Some(rescaled));
                }
                self.encoder.send_frame(&f)?;
                self.drain_encoder(output)?;
            }
            return Ok(());
        }

        // Write the incoming frame into the FIFO
        if let Some(f) = frame {
            self.fifo.write(&f)?;
        }

        // Drain the FIFO in frame_size chunks
        loop {
            let available = self.fifo.size();
            let enough = if flush {
                available > 0
            } else {
                available >= self.frame_size as i32
            };
            if !enough {
                break;
            }

            let samples = available.min(self.frame_size as i32);
            let mut out_frame =
                frame::Audio::new(self.enc_format, samples as usize, self.enc_channel_layout);
            self.fifo.read(&mut out_frame, samples)?;
            out_frame.set_pts(Some(self.next_pts));
            self.next_pts += samples as i64;

            self.encoder.send_frame(&out_frame)?;
            self.drain_encoder(output)?;
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
