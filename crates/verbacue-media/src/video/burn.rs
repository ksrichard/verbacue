//! MP4 output: decodes the video, draws the subtitles into each frame, and encodes it again.

use std::path::Path;

use ffmpeg_next::{
    self as ffmpeg, Dictionary, Packet, Rational, codec, encoder,
    format::{self, Pixel, Sample, sample::Type as SampleType},
    frame, media,
    software::{resampling, scaling},
};

#[cfg(any(target_os = "macos", windows))]
use crate::hwdecode;
use verbacue_core::{
    Progress, check_cancel,
    error::{Context as _, ErrorKind, Result, WithKind as _, bail},
    subtitle::Cue,
};

use super::{
    aac::AacEncoder, audio_start_ms, duration_secs, open_input, overlay::Painter, report,
    rescale_ms,
};

/// Audio codecs that MP4 can hold. Other audio is encoded to AAC.
pub(super) const MP4_AUDIO_CODECS: &[codec::Id] = &[
    codec::Id::AAC,
    codec::Id::MP3,
    codec::Id::AC3,
    codec::Id::EAC3,
    codec::Id::OPUS,
    codec::Id::FLAC,
    codec::Id::ALAC,
];

/// Opens a decoder for a video stream. On macOS and Windows, codecs without a software
/// decoder (AV1) use the GPU. All other codecs decode in software on several threads.
pub(super) fn open_video_decoder(ist: &format::stream::Stream) -> Result<ffmpeg::decoder::Video> {
    let id = ist.parameters().id();
    let mut ctx =
        codec::context::Context::from_parameters(ist.parameters()).with_kind(ErrorKind::Other)?;
    #[cfg(any(target_os = "macos", windows))]
    let hardware = hwdecode::attach(&mut ctx, id);
    #[cfg(not(any(target_os = "macos", windows)))]
    let hardware = false;
    if !hardware {
        ctx.set_threading(codec::threading::Config {
            kind: codec::threading::Type::Frame,
            count: 0,
        });
    }
    ctx.decoder()
        .video()
        .with_kind(ErrorKind::Other)
        .with_context(|| format!("this build cannot decode {} video", id.name()))
}

pub(super) fn write_burned(
    input: &Path,
    output: &Path,
    cues: &[Cue],
    look: super::SubtitleLook,
    progress: &dyn Progress,
) -> Result<()> {
    let mut ictx = open_input(input)?;
    let mut octx = format::output_as(output, "mp4")
        .with_kind(ErrorKind::Other)
        .with_context(|| format!("cannot create {}", output.display()))?;
    let global_header = octx.format().flags().contains(format::Flags::GLOBAL_HEADER);
    let video_index = ictx
        .streams()
        .best(media::Type::Video)
        .context("the input has no video stream")?
        .index();
    let offset_ms = audio_start_ms(&ictx);

    let mut routes: Vec<Option<Route>> = Vec::with_capacity(ictx.nb_streams() as usize);
    for ist in ictx.streams() {
        let params = ist.parameters();
        let route = if ist.index() == video_index {
            Some(Route::Video(Box::new(VideoTranscoder::new(
                &ist,
                &mut octx,
                cues,
                offset_ms,
                global_header,
                look,
            )?)))
        } else if params.medium() != media::Type::Audio {
            None
        } else if MP4_AUDIO_CODECS.contains(&params.id()) {
            let mut ost = octx
                .add_stream(encoder::find(codec::Id::None))
                .with_kind(ErrorKind::Other)?;
            ost.set_parameters(ist.parameters());
            ost.set_metadata(ist.metadata().to_owned());
            Some(Route::Copy(ost.index()))
        } else {
            tracing::info!(
                "encoding {} audio (track {}) to AAC",
                params.id().name(),
                ist.index()
            );
            let transcoder = AudioTranscoder::new(&ist, &mut octx)?;
            Some(Route::Audio(Box::new(transcoder)))
        };
        routes.push(route);
    }

    let mut options = Dictionary::new();
    // Puts the index at the start of the file, so players can start before the end.
    options.set("movflags", "+faststart");
    options.set("strict", "experimental");
    octx.write_header_with(options)
        .with_kind(ErrorKind::Other)
        .context("cannot write the output header")?;
    let out_tb: Vec<Rational> = (0..octx.nb_streams() as usize)
        .map(|i| {
            octx.stream(i)
                .map_or(Rational::new(1, 1000), |s| s.time_base())
        })
        .collect();
    let total = duration_secs(&ictx);

    for (ist, mut packet) in ictx.packets() {
        check_cancel(progress)?;
        let tb = ist.time_base();
        match routes.get_mut(ist.index()).and_then(Option::as_mut) {
            None => {}
            Some(Route::Copy(oi)) => {
                packet.rescale_ts(tb, out_tb[*oi]);
                packet.set_position(-1);
                packet.set_stream(*oi);
                packet
                    .write_interleaved(&mut octx)
                    .with_kind(ErrorKind::Other)?;
            }
            Some(Route::Video(t)) => {
                if let Some(ts) = packet.pts() {
                    report(progress, rescale_ms(ts, tb), total);
                }
                t.push(Some(&packet), &mut octx, &out_tb)?;
            }
            Some(Route::Audio(t)) => t.push(Some(&packet), &mut octx, &out_tb)?,
        }
    }
    for route in routes.iter_mut().flatten() {
        match route {
            Route::Copy(_) => {}
            Route::Video(t) => t.push(None, &mut octx, &out_tb)?,
            Route::Audio(t) => t.push(None, &mut octx, &out_tb)?,
        }
    }
    octx.write_trailer().with_kind(ErrorKind::Other)?;
    Ok(())
}

/// What happens to one input stream in [`burn`].
pub(super) enum Route<'a> {
    Copy(usize),
    Video(Box<VideoTranscoder<'a>>),
    Audio(Box<AudioTranscoder>),
}

pub(super) struct VideoTranscoder<'a> {
    decoder: ffmpeg::decoder::Video,
    encoder: encoder::video::Encoder,
    painter: Painter<'a>,
    scaler: Option<scaling::Context>,
    time_base: Rational,
    out_index: usize,
}

impl<'a> VideoTranscoder<'a> {
    pub(super) fn new(
        ist: &format::stream::Stream,
        octx: &mut format::context::Output,
        cues: &'a [Cue],
        offset_ms: i64,
        global_header: bool,
        look: super::SubtitleLook,
    ) -> Result<Self> {
        let time_base = ist.time_base();
        let decoder = open_video_decoder(ist)?;

        let (codec, name) = video_encoder()?;
        let mut enc = codec::context::Context::new_with_codec(codec)
            .encoder()
            .video()
            .with_kind(ErrorKind::Other)?;
        enc.set_width(decoder.width());
        enc.set_height(decoder.height());
        enc.set_format(Pixel::YUV420P);
        enc.set_time_base(time_base);
        enc.set_aspect_ratio(decoder.aspect_ratio());
        let rate = ist.avg_frame_rate();
        if rate.numerator() > 0 {
            enc.set_frame_rate(Some(rate));
        }
        enc.set_bit_rate(bit_rate(
            ist.parameters().bit_rate(),
            decoder.width(),
            decoder.height(),
        ));
        if global_header {
            enc.set_flags(codec::Flags::GLOBAL_HEADER);
        }
        let encoder = enc
            .open_as(codec)
            .with_kind(ErrorKind::Other)
            .with_context(|| format!("cannot open the {name} encoder"))?;
        tracing::info!("encoding video with {name}");

        let mut ost = octx.add_stream(codec).with_kind(ErrorKind::Other)?;
        ost.set_parameters(&encoder);
        ost.set_time_base(time_base);
        let out_index = ost.index();
        Ok(Self {
            decoder,
            encoder,
            painter: Painter::new(cues, offset_ms, time_base, look)?,
            scaler: None,
            time_base,
            out_index,
        })
    }

    /// Decodes a packet (or flushes with `None`), paints and encodes the frames.
    pub(super) fn push(
        &mut self,
        packet: Option<&Packet>,
        octx: &mut format::context::Output,
        out_tb: &[Rational],
    ) -> Result<()> {
        match packet {
            Some(p) => self.decoder.send_packet(p).with_kind(ErrorKind::Other)?,
            None => self.decoder.send_eof().with_kind(ErrorKind::Other)?,
        }
        let mut decoded = frame::Video::empty();
        while self.decoder.receive_frame(&mut decoded).is_ok() {
            #[cfg(any(target_os = "macos", windows))]
            let downloaded = hwdecode::download(&decoded)?;
            #[cfg(not(any(target_os = "macos", windows)))]
            let downloaded: Option<frame::Video> = None;
            let source = downloaded.as_ref().unwrap_or(&decoded);
            let frame = self.painter.paint(source, &mut self.scaler)?;
            self.encoder
                .send_frame(&frame)
                .with_kind(ErrorKind::Other)?;
            self.drain(octx, out_tb)?;
        }
        if packet.is_none() {
            self.encoder.send_eof().with_kind(ErrorKind::Other)?;
            self.drain(octx, out_tb)?;
        }
        Ok(())
    }

    pub(super) fn drain(
        &mut self,
        octx: &mut format::context::Output,
        out_tb: &[Rational],
    ) -> Result<()> {
        let mut packet = Packet::empty();
        while self.encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(self.out_index);
            packet.rescale_ts(self.time_base, out_tb[self.out_index]);
            packet.write_interleaved(octx).with_kind(ErrorKind::Other)?;
        }
        Ok(())
    }
}

/// Decodes audio that MP4 cannot hold and encodes it to AAC.
pub(super) struct AudioTranscoder {
    decoder: ffmpeg::decoder::Audio,
    resampler: Option<(resampling::Context, Sample, u64, u32)>,
    aac: AacEncoder,
    in_time_base: Rational,
}

impl AudioTranscoder {
    pub(super) fn new(
        ist: &format::stream::Stream,
        octx: &mut format::context::Output,
    ) -> Result<Self> {
        let decoder = codec::context::Context::from_parameters(ist.parameters())
            .with_kind(ErrorKind::Other)?
            .decoder()
            .audio()
            .with_kind(ErrorKind::Other)
            .with_context(|| {
                format!(
                    "this build cannot decode {} audio",
                    ist.parameters().id().name()
                )
            })?;
        let channels = decoder.channels().clamp(1, 8);
        let rate = if decoder.rate() > 0 {
            decoder.rate()
        } else {
            48_000
        };
        let aac = AacEncoder::new(
            octx,
            channels,
            rate,
            64_000 * usize::from(channels),
            ist.metadata().to_owned(),
        )?;
        Ok(Self {
            decoder,
            resampler: None,
            aac,
            in_time_base: ist.time_base(),
        })
    }

    pub(super) fn push(
        &mut self,
        packet: Option<&Packet>,
        octx: &mut format::context::Output,
        out_tb: &[Rational],
    ) -> Result<()> {
        match packet {
            Some(p) => self.decoder.send_packet(p).with_kind(ErrorKind::Other)?,
            None => self.decoder.send_eof().with_kind(ErrorKind::Other)?,
        }
        let mut decoded = frame::Audio::empty();
        while self.decoder.receive_frame(&mut decoded).is_ok() {
            if self.aac.next_pts.is_none() {
                let start = decoded.timestamp().unwrap_or(0);
                self.aac.next_pts =
                    Some(rescale_ms(start, self.in_time_base) * i64::from(self.aac.rate()) / 1000);
            }
            self.convert(&decoded)?;
            self.aac.encode_full_frames(octx, out_tb, false)?;
        }
        if packet.is_none() {
            self.aac.finish(octx, out_tb)?;
        }
        Ok(())
    }

    fn convert(&mut self, decoded: &frame::Audio) -> Result<()> {
        let decoded = crate::audio::with_known_layout(decoded);
        let decoded = decoded.as_ref();
        let layout = decoded.channel_layout();
        let (out_layout, rate) = (self.aac.layout(), self.aac.rate());
        let key = (decoded.format(), layout.bits(), decoded.rate());
        if self
            .resampler
            .as_ref()
            .is_none_or(|r| (r.1, r.2, r.3) != key)
        {
            let ctx = resampling::Context::get(
                decoded.format(),
                layout,
                decoded.rate(),
                Sample::F32(SampleType::Planar),
                out_layout,
                rate,
            )
            .with_kind(ErrorKind::Other)?;
            self.resampler = Some((ctx, key.0, key.1, key.2));
        }
        let capacity = decoded.samples() * rate as usize / decoded.rate().max(1) as usize + 256;
        let mut out = frame::Audio::new(Sample::F32(SampleType::Planar), capacity, out_layout);
        out.set_rate(rate);
        if let Some((ctx, ..)) = self.resampler.as_mut() {
            ctx.run(decoded, &mut out).with_kind(ErrorKind::Other)?;
        }
        for (ch, buf) in self.aac.buffer.iter_mut().enumerate() {
            buf.extend_from_slice(&out.plane::<f32>(ch)[..out.samples()]);
        }
        Ok(())
    }
}

pub(super) fn video_encoder() -> Result<(ffmpeg::Codec, &'static str)> {
    for name in ["h264_videotoolbox", "mpeg4"] {
        if let Some(codec) = encoder::find_by_name(name) {
            return Ok((codec, name));
        }
    }
    bail!("this build has no usable video encoder")
}

/// Keeps the input bit rate if known. Otherwise it uses about 0.1 bit per pixel at
/// 30 frames per second (6 Mbit/s for 1080p).
pub(super) fn bit_rate(input: i64, width: u32, height: u32) -> usize {
    if input > 0 {
        return usize::try_from(input).unwrap_or(6_000_000);
    }
    (f64::from(width) * f64::from(height) * 30.0 * 0.1) as usize
}
