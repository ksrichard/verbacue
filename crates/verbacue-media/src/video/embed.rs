//! MKV output: copies all streams and adds the subtitles as SRT tracks.

use std::path::Path;

use ffmpeg_next::{
    Dictionary, Packet, Rational, codec, encoder,
    format::{self},
    media,
};

use verbacue_core::{
    Progress,
    error::{Context as _, ErrorKind, Result, WithKind as _},
};

use super::{
    StreamMap, SubtitleTrack, audio_start_ms, duration_secs, open_input, report, rescale_ms,
    write_header,
};

/// Input subtitle codecs that MKV can hold. Other input subtitle streams are dropped.
pub(super) const MKV_SUBTITLE_CODECS: &[codec::Id] = &[
    codec::Id::SUBRIP,
    codec::Id::SSA,
    codec::Id::ASS,
    codec::Id::HDMV_PGS_SUBTITLE,
    codec::Id::DVD_SUBTITLE,
    codec::Id::DVB_SUBTITLE,
    codec::Id::WEBVTT,
];

pub(super) fn write_embedded(
    input: &Path,
    output: &Path,
    tracks: &[SubtitleTrack],
    progress: &dyn Progress,
) -> Result<()> {
    let mut ictx = open_input(input)?;
    let mut octx = format::output_as(output, "matroska")
        .with_kind(ErrorKind::Other)
        .with_context(|| format!("cannot create {}", output.display()))?;
    let mut map = StreamMap::new(&ictx);

    // Audio and video first, then the new subtitle tracks, then the input subtitles.
    // This keeps the new tracks first among the subtitles, so they become the default.
    for ist in ictx.streams() {
        let medium = ist.parameters().medium();
        if matches!(medium, media::Type::Video | media::Type::Audio) {
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
    let offset_ms = audio_start_ms(&ictx);
    let mut subs = SubtitleQueue::new(&octx, tracks, &sub_streams, offset_ms);
    let total = duration_secs(&ictx);

    for (ist, mut packet) in ictx.packets() {
        let Some(oi) = map.output_index(ist.index()) else {
            continue;
        };
        let tb = ist.time_base();
        if let Some(ts) = packet.dts().or(packet.pts()) {
            let ms = rescale_ms(ts, tb);
            subs.write_until(ms, &mut octx)?;
            report(progress, ms, total);
        }
        packet.rescale_ts(tb, octx.stream(oi).context("missing stream")?.time_base());
        packet.set_position(-1);
        packet.set_stream(oi);
        packet
            .write_interleaved(&mut octx)
            .with_kind(ErrorKind::Other)?;
    }
    subs.write_until(i64::MAX, &mut octx)?;
    octx.write_trailer().with_kind(ErrorKind::Other)?;
    Ok(())
}

pub(super) fn add_subtitle_streams(
    octx: &mut format::context::Output,
    tracks: &[SubtitleTrack],
) -> Result<Vec<usize>> {
    tracks
        .iter()
        .map(|track| {
            let mut params = codec::Parameters::new();
            params.set_medium(media::Type::Subtitle);
            params.set_id(codec::Id::SUBRIP);
            let mut ost = octx
                .add_stream(encoder::find(codec::Id::None))
                .with_kind(ErrorKind::Other)?;
            ost.set_parameters(params);
            ost.set_time_base((1, 1000));
            let mut metadata = Dictionary::new();
            metadata.set("language", iso639_2(track.language));
            metadata.set(
                "title",
                &verbacue_core::languages::language_name(track.language),
            );
            ost.set_metadata(metadata);
            Ok(ost.index())
        })
        .collect()
}

/// Subtitle packets for all tracks, sorted by start time. They are written between the
/// media packets, so the muxer gets every stream in time order.
pub(super) struct SubtitleQueue {
    packets: Vec<(i64, usize, i64, String)>,
    next: usize,
    time_bases: Vec<Rational>,
}

impl SubtitleQueue {
    pub(super) fn new(
        octx: &format::context::Output,
        tracks: &[SubtitleTrack],
        streams: &[usize],
        offset_ms: i64,
    ) -> Self {
        let mut packets: Vec<(i64, usize, i64, String)> = tracks
            .iter()
            .zip(streams)
            .flat_map(|(track, &stream)| {
                track.cues.iter().map(move |c| {
                    let start = c.start_ms as i64 + offset_ms;
                    (
                        start,
                        stream,
                        (c.end_ms - c.start_ms) as i64,
                        c.text.clone(),
                    )
                })
            })
            .collect();
        packets.sort_by_key(|p| (p.0, p.1));
        let time_bases = (0..octx.nb_streams() as usize)
            .map(|i| {
                octx.stream(i)
                    .map_or(Rational::new(1, 1000), |s| s.time_base())
            })
            .collect();
        Self {
            packets,
            next: 0,
            time_bases,
        }
    }

    pub(super) fn write_until(
        &mut self,
        ms: i64,
        octx: &mut format::context::Output,
    ) -> Result<()> {
        while let Some((start, stream, duration, text)) = self.packets.get(self.next) {
            if *start > ms {
                break;
            }
            let mut packet = Packet::copy(text.as_bytes());
            packet.set_pts(Some(*start));
            packet.set_dts(Some(*start));
            packet.set_duration(*duration);
            packet.set_stream(*stream);
            packet.rescale_ts(Rational::new(1, 1000), self.time_bases[*stream]);
            packet.write_interleaved(octx).with_kind(ErrorKind::Other)?;
            self.next += 1;
        }
        Ok(())
    }
}

/// MKV language tags use ISO 639-2 codes.
pub(super) fn iso639_2(code: &str) -> &str {
    match code {
        "hu" => "hun",
        "en" => "eng",
        "de" => "ger",
        "fr" => "fre",
        "es" => "spa",
        "it" => "ita",
        "pt" => "por",
        "nl" => "dut",
        "pl" => "pol",
        "cs" => "cze",
        "sk" => "slo",
        "ro" => "rum",
        "ru" => "rus",
        "uk" => "ukr",
        "sv" => "swe",
        "da" => "dan",
        "no" => "nor",
        "fi" => "fin",
        "tr" => "tur",
        "el" => "gre",
        "ja" => "jpn",
        "ko" => "kor",
        "zh" => "chi",
        "ar" => "ara",
        "he" => "heb",
        "hi" => "hin",
        other => other,
    }
}
