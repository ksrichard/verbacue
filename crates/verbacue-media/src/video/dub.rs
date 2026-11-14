//! MKV output with a new audio track: copies all streams, adds the dubbed audio as
//! AAC, and adds the subtitles as SRT tracks.

use std::{
    fs::File,
    io::{BufReader, Read as _},
    path::Path,
};

use ffmpeg_next::{Dictionary, Rational, format, media};

use verbacue_core::{
    Progress, check_cancel,
    error::{Context as _, ErrorKind, Result, WithKind as _},
};

use super::{
    StreamMap, SubtitleTrack,
    aac::AacEncoder,
    audio_start_ms, duration_secs,
    embed::{MKV_SUBTITLE_CODECS, SubtitleQueue, add_subtitle_streams, iso639_2},
    open_input, report, rescale_ms, write_header,
};
use crate::audio::RawAudio;

/// The bit rate of the new track.
const BIT_RATE: usize = 192_000;
/// The frames read from the raw file at once.
const CHUNK_FRAMES: usize = 4096;

pub(super) fn write_dubbed(
    input: &Path,
    output: &Path,
    audio: &RawAudio,
    language: &str,
    title: &str,
    tracks: &[SubtitleTrack],
    progress: &dyn Progress,
) -> Result<()> {
    let mut ictx = open_input(input)?;
    let mut octx = format::output_as(output, "matroska")
        .with_kind(ErrorKind::Other)
        .with_context(|| format!("cannot create {}", output.display()))?;
    let mut map = StreamMap::new(&ictx);

    // Video, then the new audio, then the input audio. MKV with `default_mode=infer`
    // marks the first audio track as the default, and the copied streams have no
    // default flag, so the new track is the only default one.
    for ist in ictx.streams() {
        if ist.parameters().medium() == media::Type::Video {
            map.copy_stream(&ist, &mut octx)?;
        }
    }
    let mut metadata = Dictionary::new();
    metadata.set("language", iso639_2(language));
    metadata.set("title", title);
    let mut aac = AacEncoder::new(&mut octx, audio.channels, audio.rate, BIT_RATE, metadata)?;
    for ist in ictx.streams() {
        if ist.parameters().medium() == media::Type::Audio {
            map.copy_stream(&ist, &mut octx)?;
        }
    }
    let sub_streams = add_subtitle_streams(&mut octx, tracks)?;
    for ist in ictx.streams() {
        let params = ist.parameters();
        if params.medium() == media::Type::Subtitle && MKV_SUBTITLE_CODECS.contains(&params.id()) {
            map.copy_stream(&ist, &mut octx)?;
        }
    }

    write_header(&mut octx)?;
    let out_tb: Vec<Rational> = (0..octx.nb_streams() as usize)
        .map(|i| {
            octx.stream(i)
                .map_or(Rational::new(1, 1000), |s| s.time_base())
        })
        .collect();
    let offset_ms = audio_start_ms(&ictx);
    let mut subs = SubtitleQueue::new(&octx, tracks, &sub_streams, offset_ms);
    // The dubbed audio starts where the transcript starts: at the first input audio.
    aac.next_pts = Some(offset_ms.max(0) * i64::from(audio.rate) / 1000);
    let mut raw = RawReader::open(audio)?;
    let total = duration_secs(&ictx);

    for (ist, mut packet) in ictx.packets() {
        check_cancel(progress)?;
        let Some(oi) = map.output_index(ist.index()) else {
            continue;
        };
        let tb = ist.time_base();
        if let Some(ts) = packet.dts().or(packet.pts()) {
            let ms = rescale_ms(ts, tb);
            raw.feed_until(ms - offset_ms, &mut aac, &mut octx, &out_tb)?;
            subs.write_until(ms, &mut octx)?;
            report(progress, ms, total);
        }
        packet.rescale_ts(tb, out_tb[oi]);
        packet.set_position(-1);
        packet.set_stream(oi);
        packet
            .write_interleaved(&mut octx)
            .with_kind(ErrorKind::Other)?;
    }
    raw.feed_until(i64::MAX, &mut aac, &mut octx, &out_tb)?;
    aac.finish(&mut octx, &out_tb)?;
    subs.write_until(i64::MAX, &mut octx)?;
    octx.write_trailer().with_kind(ErrorKind::Other)?;
    Ok(())
}

/// Reads the raw file in chunks and gives the samples to the encoder.
struct RawReader {
    reader: BufReader<File>,
    channels: usize,
    rate: u32,
    /// The frames read so far.
    frames: u64,
    done: bool,
}

impl RawReader {
    fn open(audio: &RawAudio) -> Result<Self> {
        let file = File::open(&audio.path)
            .with_kind(ErrorKind::Other)
            .with_context(|| format!("cannot read {}", audio.path.display()))?;
        Ok(Self {
            reader: BufReader::new(file),
            channels: usize::from(audio.channels.max(1)),
            rate: audio.rate.max(1),
            frames: 0,
            done: false,
        })
    }

    /// Encodes the audio up to `ms` (from the start of the raw file).
    fn feed_until(
        &mut self,
        ms: i64,
        aac: &mut AacEncoder,
        octx: &mut format::context::Output,
        out_tb: &[Rational],
    ) -> Result<()> {
        let mut bytes = vec![0u8; CHUNK_FRAMES * self.channels * 4];
        while !self.done && (self.frames * 1000 / u64::from(self.rate)) as i64 <= ms {
            let n = read_full(&mut self.reader, &mut bytes)?;
            let frames = n / (self.channels * 4);
            if frames < CHUNK_FRAMES {
                self.done = true;
            }
            for frame in bytes[..frames * self.channels * 4].chunks_exact(self.channels * 4) {
                for (ch, buf) in aac.buffer.iter_mut().enumerate() {
                    // The encoder has as many channels as the raw file.
                    let at = ch.min(self.channels - 1) * 4;
                    buf.push(f32::from_le_bytes([
                        frame[at],
                        frame[at + 1],
                        frame[at + 2],
                        frame[at + 3],
                    ]));
                }
            }
            self.frames += frames as u64;
            aac.encode_full_frames(octx, out_tb, false)?;
        }
        Ok(())
    }
}

/// Reads until `buf` is full or the file ends. Returns the bytes read.
fn read_full(reader: &mut BufReader<File>, buf: &mut [u8]) -> Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match reader.read(&mut buf[n..]).with_kind(ErrorKind::Other)? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}
