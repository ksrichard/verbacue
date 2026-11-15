//! An AAC encoder for one output stream, with FFmpeg's own `aac` encoder.
//!
//! The encoder needs frames of exactly `frame_size` samples, so the samples go
//! through a buffer for each channel first.

use crate::error::Result;
use ffmpeg_next::{
    ChannelLayout, Dictionary, Packet, Rational, codec, encoder,
    format::{self, Sample, sample::Type as SampleType},
    frame,
};

use verbacue_core::error::Context as _;

pub(super) struct AacEncoder {
    encoder: encoder::audio::Encoder,
    layout: ChannelLayout,
    rate: u32,
    /// The samples that wait for a full frame, one buffer for each channel.
    pub(super) buffer: Vec<Vec<f32>>,
    /// The position of the next frame, in samples (time base 1/rate).
    pub(super) next_pts: Option<i64>,
    out_index: usize,
}

impl AacEncoder {
    /// Adds an AAC stream to `octx`, with `metadata` as its tags.
    pub(super) fn new(
        octx: &mut format::context::Output,
        channels: u16,
        rate: u32,
        bit_rate: usize,
        metadata: Dictionary,
    ) -> Result<Self> {
        let codec = encoder::find(codec::Id::AAC).context("this build has no AAC encoder")?;
        let channels = channels.clamp(1, 8);
        let layout = ChannelLayout::default(i32::from(channels));
        let rate_i32 = i32::try_from(rate).unwrap_or(48_000);
        let mut enc = codec::context::Context::new_with_codec(codec)
            .encoder()
            .audio()?;
        enc.set_rate(rate_i32);
        enc.set_channel_layout(layout);
        enc.set_format(Sample::F32(SampleType::Planar));
        enc.set_bit_rate(bit_rate);
        enc.set_time_base((1, rate_i32));
        if octx.format().flags().contains(format::Flags::GLOBAL_HEADER) {
            enc.set_flags(codec::Flags::GLOBAL_HEADER);
        }
        let encoder = enc.open_as(codec).context("cannot open the AAC encoder")?;

        let mut ost = octx.add_stream(codec)?;
        ost.set_parameters(&encoder);
        ost.set_metadata(metadata);
        let out_index = ost.index();
        Ok(Self {
            encoder,
            layout,
            rate,
            buffer: vec![Vec::new(); usize::from(channels)],
            next_pts: None,
            out_index,
        })
    }

    pub(super) fn layout(&self) -> ChannelLayout {
        self.layout
    }

    pub(super) fn rate(&self) -> u32 {
        self.rate
    }

    /// Sends every full frame in the buffer to the encoder. With `last`, the last
    /// partial frame is padded with silence.
    pub(super) fn encode_full_frames(
        &mut self,
        octx: &mut format::context::Output,
        out_tb: &[Rational],
        last: bool,
    ) -> Result<()> {
        let size = self.encoder.frame_size().max(1) as usize;
        while self.buffer[0].len() >= size || (last && !self.buffer[0].is_empty()) {
            let mut frame = frame::Audio::new(Sample::F32(SampleType::Planar), size, self.layout);
            frame.set_rate(self.rate);
            for (ch, buf) in self.buffer.iter_mut().enumerate() {
                let n = buf.len().min(size);
                let plane = frame.plane_mut::<f32>(ch);
                plane[..n].copy_from_slice(&buf[..n]);
                plane[n..].fill(0.0);
                buf.drain(..n);
            }
            let pts = self.next_pts.unwrap_or(0);
            frame.set_pts(Some(pts));
            self.next_pts = Some(pts + size as i64);
            self.encoder.send_frame(&frame)?;
            self.drain(octx, out_tb)?;
        }
        Ok(())
    }

    /// Encodes the rest of the buffer and the frames that the encoder still holds.
    pub(super) fn finish(
        &mut self,
        octx: &mut format::context::Output,
        out_tb: &[Rational],
    ) -> Result<()> {
        self.encode_full_frames(octx, out_tb, true)?;
        self.encoder.send_eof()?;
        self.drain(octx, out_tb)
    }

    fn drain(&mut self, octx: &mut format::context::Output, out_tb: &[Rational]) -> Result<()> {
        let mut packet = Packet::empty();
        let tb = Rational::new(1, i32::try_from(self.rate).unwrap_or(48_000));
        while self.encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(self.out_index);
            packet.rescale_ts(tb, out_tb[self.out_index]);
            packet.write_interleaved(octx)?;
        }
        Ok(())
    }
}
