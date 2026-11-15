//! Lip sync with MuseTalk 1.5: the mouth of the speaking actor follows the new line.
//!
//! The work has two passes over the video:
//! 1. A scan of small frames finds the faces (YuNet) while each new line plays. The
//!    faces are tracked, and the face whose mouth moves most in the original video is
//!    the speaker of the line. Faces under 6 % of the frame height are left alone.
//! 2. While the video is encoded again, each frame of a line gets a new mouth: the face
//!    crop (256 x 256) is encoded twice with TAESD (lower half hidden, and whole), the
//!    MuseTalk UNet makes the new latents from Whisper features of the new speech, and
//!    TAESD decodes them. A soft mask blends the jaw and the mouth into the frame.
//!
//! TAESD replaces the SD VAE of MuseTalk: it uses the same latent space and is about 15
//! times faster on a CPU.

use std::{collections::BTreeMap, path::Path};

use ort::{session::Session, value::Tensor};
use rustfft::{FftPlanner, num_complex::Complex};
use verbacue_core::{Error, ErrorKind, Result};
use verbacue_media::video::{FrameEditor, Rgb, Yuv};

use crate::{run::Events, session};

/// The model files, in the voice model folder or in the `lipsync` bundle.
pub const FACES_FILE: &str = "lipsync_faces.onnx";
pub const WHISPER_FILE: &str = "lipsync_whisper.onnx";
pub const ENCODER_FILE: &str = "lipsync_vae_encoder.onnx";
pub const DECODER_FILE: &str = "lipsync_vae_decoder.onnx";
pub const UNET_FILE: &str = "lipsync_unet.onnx";
pub const FILES: [&str; 5] = [
    FACES_FILE,
    WHISPER_FILE,
    ENCODER_FILE,
    DECODER_FILE,
    UNET_FILE,
];

/// The width of the frames for face finding.
pub const SCAN_WIDTH: u32 = 640;
const DETECT_SIZE: usize = 640;
const MIN_SCORE: f32 = 0.6;
const NMS_IOU: f32 = 0.3;
/// A face must be this share of the frame height to get a new mouth.
const MIN_FACE_SHARE: f32 = 0.06;
/// A track must be in this share of the frames of a line to be the speaker.
const MIN_TRACK_SHARE: f32 = 0.4;
const TRACK_IOU: f32 = 0.3;
/// Frames on each side for the smoothing of the face boxes.
const SMOOTH: usize = 2;
const CROP: usize = 256;
const LATENT: usize = 32;
/// Pixels below the chin in the crop, as MuseTalk 1.5 (`extra_margin`).
const CHIN_MARGIN_PX: f32 = 10.0;

/// One new line: where it starts in the film, and its speech at 16 kHz.
pub struct Line {
    pub start_ms: u64,
    pub audio_16k: Vec<f32>,
}

impl Line {
    fn end_ms(&self) -> u64 {
        self.start_ms + self.audio_16k.len() as u64 / 16
    }
}

/// A face box with its nose, as shares of the frame width and height.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Face {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    nose_y: f32,
}

impl Face {
    fn iou(&self, o: &Face) -> f32 {
        let (x1, y1) = (self.x.max(o.x), self.y.max(o.y));
        let (x2, y2) = (
            (self.x + self.w).min(o.x + o.w),
            (self.y + self.h).min(o.y + o.h),
        );
        let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
        inter / (self.w * self.h + o.w * o.h - inter).max(1e-9)
    }
}

/// The ONNX graphs of lip sync.
pub struct Models {
    faces: Session,
    whisper: Session,
    encoder: Session,
    decoder: Session,
    unet: Session,
}

impl Models {
    /// Opens the five graphs in `dir`.
    ///
    /// # Errors
    /// Returns an error if a file is missing or not a valid graph.
    pub fn open(dir: &Path) -> Result<Models> {
        let open = |f: &str| session::open(&dir.join(f));
        Ok(Models {
            faces: open(FACES_FILE)?,
            whisper: open(WHISPER_FILE)?,
            encoder: open(ENCODER_FILE)?,
            decoder: open(DECODER_FILE)?,
            unet: open(UNET_FILE)?,
        })
    }

    /// The faces in a frame, best first.
    fn detect(&mut self, rgb: &Rgb) -> Result<Vec<(f32, Face)>> {
        // Letterbox into 640 x 640, BGR, 0-255, as OpenCV's FaceDetectorYN.
        let scale = DETECT_SIZE as f32 / rgb.width.max(rgb.height) as f32;
        let (w, h) = (
            ((rgb.width as f32 * scale) as usize).min(DETECT_SIZE),
            ((rgb.height as f32 * scale) as usize).min(DETECT_SIZE),
        );
        let plane = DETECT_SIZE * DETECT_SIZE;
        let mut input = vec![0.0f32; 3 * plane];
        for y in 0..h {
            let sy = ((y as f32 / scale) as usize).min(rgb.height - 1);
            for x in 0..w {
                let sx = ((x as f32 / scale) as usize).min(rgb.width - 1);
                let p = (sy * rgb.width + sx) * 3;
                for c in 0..3 {
                    input[c * plane + y * DETECT_SIZE + x] = f32::from(rgb.data[p + 2 - c]);
                }
            }
        }
        let tensor = Tensor::from_array(([1, 3, DETECT_SIZE, DETECT_SIZE], input))
            .map_err(session::model_error)?;
        let out = self
            .faces
            .run(ort::inputs!["input" => tensor])
            .map_err(session::model_error)?;
        let mut found = Vec::new();
        for stride in [8usize, 16, 32] {
            let get = |name: String| -> Result<Vec<f32>> {
                let (_, v) = out[name.as_str()]
                    .try_extract_tensor::<f32>()
                    .map_err(session::model_error)?;
                Ok(v.to_vec())
            };
            let (cls, obj) = (get(format!("cls_{stride}"))?, get(format!("obj_{stride}"))?);
            let (bbox, kps) = (
                get(format!("bbox_{stride}"))?,
                get(format!("kps_{stride}"))?,
            );
            let cols = DETECT_SIZE / stride;
            for i in 0..cls.len() {
                let score = (cls[i].clamp(0.0, 1.0) * obj[i].clamp(0.0, 1.0)).sqrt();
                if score < MIN_SCORE {
                    continue;
                }
                let (r, c) = ((i / cols) as f32, (i % cols) as f32);
                let s = stride as f32;
                let (cx, cy) = ((c + bbox[i * 4]) * s, (r + bbox[i * 4 + 1]) * s);
                let (bw, bh) = (bbox[i * 4 + 2].exp() * s, bbox[i * 4 + 3].exp() * s);
                // Landmark 2 is the nose tip.
                let nose_y = (kps[i * 10 + 5] + r) * s;
                let (fw, fh) = (rgb.width as f32 * scale, rgb.height as f32 * scale);
                found.push((
                    score,
                    Face {
                        x: (cx - bw / 2.0) / fw,
                        y: (cy - bh / 2.0) / fh,
                        w: bw / fw,
                        h: bh / fh,
                        nose_y: nose_y / fh,
                    },
                ));
            }
        }
        found.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut kept: Vec<(f32, Face)> = Vec::new();
        for f in found {
            if kept.iter().all(|k| k.1.iou(&f.1) < NMS_IOU) {
                kept.push(f);
            }
        }
        Ok(kept)
    }

    /// The Whisper encoder states of a line: [1500][5][384], 50 per second.
    fn whisper_states(&mut self, audio_16k: &[f32]) -> Result<Vec<f32>> {
        let mel = whisper_mel(audio_16k);
        let input =
            Tensor::from_array(([1, MEL_BINS, MEL_FRAMES], mel)).map_err(session::model_error)?;
        let out = self
            .whisper
            .run(ort::inputs!["mel" => input])
            .map_err(session::model_error)?;
        let (_, v) = out["states"]
            .try_extract_tensor::<f32>()
            .map_err(session::model_error)?;
        Ok(v.to_vec())
    }

    /// A new mouth for a 256 x 256 RGB face crop (values 0-1, planar), from the audio
    /// features [50][384]. Returns the new crop in the same form.
    fn mouth(&mut self, crop: &[f32], audio: Vec<f32>) -> Result<Vec<f32>> {
        let plane = CROP * CROP;
        let full: Vec<f32> = crop.iter().map(|v| v * 2.0 - 1.0).collect();
        // MuseTalk hides the lower half: black, which is -1 after the normalising.
        let mut masked = full.clone();
        for c in 0..3 {
            masked[c * plane + plane / 2..(c + 1) * plane].fill(-1.0);
        }
        let encode = |enc: &mut Session, image: Vec<f32>| -> Result<Vec<f32>> {
            let t =
                Tensor::from_array(([1, 3, CROP, CROP], image)).map_err(session::model_error)?;
            crate::xtts::run_one(enc, "image", t, 4 * LATENT * LATENT)
        };
        let mut latents = encode(&mut self.encoder, masked)?;
        latents.extend(encode(&mut self.encoder, full)?);
        let unet_out = {
            let l = Tensor::from_array(([1, 8, LATENT, LATENT], latents))
                .map_err(session::model_error)?;
            let a = Tensor::from_array(([1, 50, 384], audio)).map_err(session::model_error)?;
            let out = self
                .unet
                .run(ort::inputs!["latents" => l, "audio" => a])
                .map_err(session::model_error)?;
            let (_, v) = out[0]
                .try_extract_tensor::<f32>()
                .map_err(session::model_error)?;
            v.to_vec()
        };
        let t =
            Tensor::from_array(([1, 4, LATENT, LATENT], unet_out)).map_err(session::model_error)?;
        crate::xtts::run_one(&mut self.decoder, "latents", t, 3 * plane)
    }
}

/// The face boxes of each frame of each line, with the speaking face chosen.
pub struct Plan {
    /// Frame time (ms) to the line index and the face.
    frames: BTreeMap<i64, (usize, Face)>,
}

impl Plan {
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// Decodes the video and calls back with the small frames that the first argument
/// asks for.
pub type Scan<'a> =
    dyn FnMut(&dyn Fn(i64) -> bool, &mut dyn FnMut(i64, &Rgb) -> Result<()>) -> Result<()> + 'a;

/// A face over several frames of one line.
struct Track {
    frames: Vec<(i64, Face)>,
    /// The sum of the mouth changes between the frames of this track.
    motion: f32,
    /// The mouth area of the last frame: 16 x 8 luma samples.
    last_mouth: Vec<f32>,
}

/// Finds the speaking face of each line. `scan` decodes the video and calls back with
/// the small frames that `wants` asks for.
///
/// # Errors
/// Returns an error if the face model or the video decoding fails.
pub fn plan(
    models: &mut Models,
    lines: &[Line],
    scan: &mut Scan<'_>,
    events: &dyn Events,
) -> Result<Plan> {
    let line_at = |ms: i64| -> Option<usize> {
        let ms = u64::try_from(ms).ok()?;
        let i = lines.partition_point(|l| l.start_ms <= ms).checked_sub(1)?;
        (ms < lines[i].end_ms()).then_some(i)
    };
    let mut tracks: Vec<Vec<Track>> = (0..lines.len()).map(|_| Vec::new()).collect();
    let mut frames_of_line = vec![0usize; lines.len()];
    let total = lines.last().map_or(0, Line::end_ms);
    scan(&|ms| line_at(ms).is_some(), &mut |ms, rgb| {
        if events.cancelled() {
            return Err(Error::new(ErrorKind::Cancelled, "cancelled"));
        }
        events.progress(u64::try_from(ms).unwrap_or(0), total);
        let Some(line) = line_at(ms) else {
            return Ok(());
        };
        frames_of_line[line] += 1;
        for (_, face) in models.detect(rgb)? {
            if face.h < MIN_FACE_SHARE {
                continue;
            }
            let mouth = mouth_samples(rgb, &face);
            let tracks = &mut tracks[line];
            let best = tracks
                .iter_mut()
                .filter(|t| t.frames.last().is_some_and(|(m, _)| *m < ms))
                .map(|t| {
                    let iou = t.frames.last().map_or(0.0, |(_, f)| f.iou(&face));
                    (iou, t)
                })
                .filter(|(iou, _)| *iou >= TRACK_IOU)
                .max_by(|a, b| a.0.total_cmp(&b.0));
            match best {
                Some((_, t)) => {
                    t.motion += t
                        .last_mouth
                        .iter()
                        .zip(&mouth)
                        .map(|(a, b)| (a - b).abs())
                        .sum::<f32>()
                        / mouth.len() as f32;
                    t.frames.push((ms, face));
                    t.last_mouth = mouth;
                }
                None => tracks.push(Track {
                    frames: vec![(ms, face)],
                    motion: 0.0,
                    last_mouth: mouth,
                }),
            }
        }
        Ok(())
    })?;

    let mut frames = BTreeMap::new();
    for (line, line_tracks) in tracks.into_iter().enumerate() {
        let need = (frames_of_line[line] as f32 * MIN_TRACK_SHARE).ceil() as usize;
        let speaker = line_tracks
            .into_iter()
            .filter(|t| t.frames.len() >= need.max(2))
            .max_by(|a, b| {
                let rate = |t: &Track| t.motion / t.frames.len() as f32;
                rate(a).total_cmp(&rate(b))
            });
        if let Some(t) = speaker {
            for (ms, face) in smooth(&t.frames) {
                frames.insert(ms, (line, face));
            }
        }
    }
    Ok(Plan { frames })
}

/// The face boxes averaged over the frames around each frame, so the new mouth does
/// not shake with the small errors of the face finder.
fn smooth(frames: &[(i64, Face)]) -> Vec<(i64, Face)> {
    (0..frames.len())
        .map(|i| {
            let near = &frames[i.saturating_sub(SMOOTH)..(i + SMOOTH + 1).min(frames.len())];
            let n = near.len() as f32;
            let avg = |f: fn(&Face) -> f32| near.iter().map(|(_, x)| f(x)).sum::<f32>() / n;
            (
                frames[i].0,
                Face {
                    x: avg(|f| f.x),
                    y: avg(|f| f.y),
                    w: avg(|f| f.w),
                    h: avg(|f| f.h),
                    nose_y: avg(|f| f.nose_y),
                },
            )
        })
        .collect()
}

/// The brightness of the mouth area (the lower third of the face) on a 16 x 8 grid.
fn mouth_samples(rgb: &Rgb, face: &Face) -> Vec<f32> {
    let mut out = Vec::with_capacity(128);
    for gy in 0..8 {
        for gx in 0..16 {
            let x = face.x + face.w * (0.2 + 0.6 * (gx as f32 + 0.5) / 16.0);
            let y = face.y + face.h * (0.65 + 0.3 * (gy as f32 + 0.5) / 8.0);
            let px = ((x * rgb.width as f32) as usize).min(rgb.width - 1);
            let py = ((y * rgb.height as f32) as usize).min(rgb.height - 1);
            let p = (py * rgb.width + px) * 3;
            out.push(
                0.299 * f32::from(rgb.data[p])
                    + 0.587 * f32::from(rgb.data[p + 1])
                    + 0.114 * f32::from(rgb.data[p + 2]),
            );
        }
    }
    out
}

/// Gives the frames of the plan a new mouth while the video is encoded again.
pub struct LipSync<'a> {
    models: &'a mut Models,
    plan: Plan,
    lines: &'a [Line],
    /// The Whisper states of the line in work.
    states: Option<(usize, Vec<f32>)>,
    events: &'a dyn Events,
    done: u64,
}

impl<'a> LipSync<'a> {
    pub fn new(
        models: &'a mut Models,
        plan: Plan,
        lines: &'a [Line],
        events: &'a dyn Events,
    ) -> Self {
        LipSync {
            models,
            plan,
            lines,
            states: None,
            events,
            done: 0,
        }
    }

    /// The audio features of one video frame, `ms` after the start of its line: 10
    /// Whisper frames (2 video frames before, 2 after) of 5 layers.
    fn audio_at(&mut self, line: usize, ms: u64) -> Result<Vec<f32>> {
        if self.states.as_ref().is_none_or(|(l, _)| *l != line) {
            let states = self.models.whisper_states(&self.lines[line].audio_16k)?;
            self.states = Some((line, states));
        }
        let states = &self
            .states
            .as_ref()
            .map(|(_, s)| s)
            .ok_or_else(|| Error::new(ErrorKind::Model, "no Whisper states"))?;
        let row = 5 * 384;
        let rows = states.len() / row;
        let valid = (self.lines[line].audio_16k.len() / 320).min(rows);
        let index = (ms / 20) as i64;
        let mut out = Vec::with_capacity(50 * 384);
        for r in index - 4..index + 6 {
            match usize::try_from(r).ok().filter(|&r| r < valid) {
                Some(r) => out.extend_from_slice(&states[r * row..(r + 1) * row]),
                None => out.extend(std::iter::repeat_n(0.0, row)),
            }
        }
        Ok(out)
    }
}

impl FrameEditor for LipSync<'_> {
    fn wants(&self, ms: i64) -> bool {
        self.plan.frames.contains_key(&ms)
    }

    fn edit(&mut self, ms: i64, frame: &mut Yuv) -> Result<()> {
        if self.events.cancelled() {
            return Err(Error::new(ErrorKind::Cancelled, "cancelled"));
        }
        let Some(&(line, face)) = self.plan.frames.get(&ms) else {
            return Ok(());
        };
        self.done += 1;
        self.events.progress(self.done, self.plan.len() as u64);
        let since = u64::try_from(ms)
            .unwrap_or(0)
            .saturating_sub(self.lines[line].start_ms);
        let audio = self.audio_at(line, since)?;
        let (w, h) = (frame.width as f32, frame.height as f32);
        // The MuseTalk crop: the jaw width, from the chin up to as far above the nose.
        let chin = (face.y + face.h) * h;
        let nose = face.nose_y * h;
        let x1 = (face.x * w).max(0.0) as usize;
        let x2 = ((face.x + face.w) * w).min(w - 1.0) as usize;
        let y1 = (nose - (chin - nose)).max(0.0) as usize;
        let y2 = (chin + CHIN_MARGIN_PX).min(h - 1.0) as usize;
        if x2 <= x1 + 8 || y2 <= y1 + 8 {
            return Ok(());
        }
        let crop = read_crop(frame, x1, y1, x2, y2);
        let new = self.models.mouth(&crop, audio)?;
        write_crop(frame, x1, y1, x2, y2, &with_detail(&new, &crop));
        Ok(())
    }
}

/// YUV (BT.709 for HD, else BT.601, limited range) to RGB 0-1.
fn to_rgb(y: u8, u: u8, v: u8, hd: bool) -> [f32; 3] {
    let (y, u, v) = (
        (f32::from(y) - 16.0) / 219.0,
        (f32::from(u) - 128.0) / 224.0,
        (f32::from(v) - 128.0) / 224.0,
    );
    let (kr, kb) = if hd { (0.2126, 0.0722) } else { (0.299, 0.114) };
    let kg = 1.0 - kr - kb;
    let r = y + 2.0 * (1.0 - kr) * v;
    let b = y + 2.0 * (1.0 - kb) * u;
    let g = (y - kr * r - kb * b) / kg;
    [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]
}

fn to_yuv([r, g, b]: [f32; 3], hd: bool) -> [f32; 3] {
    let (kr, kb) = if hd { (0.2126, 0.0722) } else { (0.299, 0.114) };
    let y = kr * r + (1.0 - kr - kb) * g + kb * b;
    let u = (b - y) / (2.0 * (1.0 - kb));
    let v = (r - y) / (2.0 * (1.0 - kr));
    [16.0 + 219.0 * y, 128.0 + 224.0 * u, 128.0 + 224.0 * v]
}

fn pixel(frame: &Yuv, x: usize, y: usize) -> [f32; 3] {
    let cw = frame.width.div_ceil(2);
    let c = (y / 2) * cw + x / 2;
    to_rgb(
        frame.y[y * frame.width + x],
        frame.u[c],
        frame.v[c],
        frame.height >= 720,
    )
}

/// The crop as 256 x 256 planar RGB (0-1), with bilinear sampling.
fn read_crop(frame: &Yuv, x1: usize, y1: usize, x2: usize, y2: usize) -> Vec<f32> {
    let plane = CROP * CROP;
    let mut out = vec![0.0f32; 3 * plane];
    let (cw, ch) = ((x2 - x1) as f32, (y2 - y1) as f32);
    for j in 0..CROP {
        let fy = y1 as f32 + (j as f32 + 0.5) * ch / CROP as f32 - 0.5;
        for i in 0..CROP {
            let fx = x1 as f32 + (i as f32 + 0.5) * cw / CROP as f32 - 0.5;
            let rgb = bilinear(fx, fy, frame.width, frame.height, |x, y| pixel(frame, x, y));
            for c in 0..3 {
                out[c * plane + j * CROP + i] = rgb[c];
            }
        }
    }
    out
}

fn bilinear(
    fx: f32,
    fy: f32,
    w: usize,
    h: usize,
    get: impl Fn(usize, usize) -> [f32; 3],
) -> [f32; 3] {
    let x0 = fx.floor().clamp(0.0, (w - 1) as f32) as usize;
    let y0 = fy.floor().clamp(0.0, (h - 1) as f32) as usize;
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (ax, ay) = (
        (fx - x0 as f32).clamp(0.0, 1.0),
        (fy - y0 as f32).clamp(0.0, 1.0),
    );
    let (a, b, c, d) = (get(x0, y0), get(x1, y0), get(x0, y1), get(x1, y1));
    std::array::from_fn(|k| {
        (a[k] * (1.0 - ax) + b[k] * ax) * (1.0 - ay) + (c[k] * (1.0 - ax) + d[k] * ax) * ay
    })
}

/// How much of the new crop shows at (u, v), both 0-1 in the crop: the jaw and the
/// mouth, from the nose (v = 0.5) down, with soft edges.
fn blend_weight(u: f32, v: f32) -> f32 {
    let step = |a: f32, b: f32, x: f32| {
        let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let vertical = step(0.52, 0.64, v) * (1.0 - step(0.90, 0.99, v));
    let horizontal = step(0.12, 0.28, u) * (1.0 - step(0.72, 0.88, u));
    vertical * horizontal
}

/// The share of the original fine detail (beard, skin) that goes back onto the new
/// crop. TAESD and the 256 x 256 crop lose it, so the new mouth looks soft.
const DETAIL: f32 = 0.6;

/// `new` with the fine detail of `original`: new + 0.6 * (original - blur(original)).
/// Both are 256 x 256 planar RGB (0-1).
fn with_detail(new: &[f32], original: &[f32]) -> Vec<f32> {
    let plane = CROP * CROP;
    let mut out = new.to_vec();
    for c in 0..3 {
        let src = &original[c * plane..(c + 1) * plane];
        let blurred = box_blur(src, 2);
        for i in 0..plane {
            out[c * plane + i] =
                (new[c * plane + i] + DETAIL * (src[i] - blurred[i])).clamp(0.0, 1.0);
        }
    }
    out
}

/// A box blur of radius `r` over one 256 x 256 plane, rows then columns.
fn box_blur(plane: &[f32], r: usize) -> Vec<f32> {
    let pass = |input: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0.0f32; CROP * CROP];
        for a in 0..CROP {
            for b in 0..CROP {
                let (lo, hi) = (b.saturating_sub(r), (b + r).min(CROP - 1));
                let sum: f32 = (lo..=hi)
                    .map(|k| {
                        if horizontal {
                            input[a * CROP + k]
                        } else {
                            input[k * CROP + a]
                        }
                    })
                    .sum();
                let v = sum / (hi - lo + 1) as f32;
                if horizontal {
                    out[a * CROP + b] = v;
                } else {
                    out[b * CROP + a] = v;
                }
            }
        }
        out
    };
    pass(&pass(plane, true), false)
}

/// Blends the new 256 x 256 crop (planar RGB 0-1) back into the frame.
fn write_crop(frame: &mut Yuv, x1: usize, y1: usize, x2: usize, y2: usize, new: &[f32]) {
    let plane = CROP * CROP;
    let hd = frame.height >= 720;
    let (cw, ch) = ((x2 - x1) as f32, (y2 - y1) as f32);
    let sample = |x: usize, y: usize| -> [f32; 3] {
        let fx = (x - x1) as f32 * CROP as f32 / cw - 0.5 + 0.5 * CROP as f32 / cw;
        let fy = (y - y1) as f32 * CROP as f32 / ch - 0.5 + 0.5 * CROP as f32 / ch;
        bilinear(fx, fy, CROP, CROP, |i, j| {
            [
                new[j * CROP + i],
                new[plane + j * CROP + i],
                new[2 * plane + j * CROP + i],
            ]
        })
    };
    let cwidth = frame.width.div_ceil(2);
    // Chroma: one value for each 2 x 2 block.
    let mut chroma: BTreeMap<usize, ([f32; 2], f32)> = BTreeMap::new();
    for y in y1..y2 {
        for x in x1..x2 {
            let a = blend_weight((x - x1) as f32 / cw, (y - y1) as f32 / ch);
            if a <= 0.0 {
                continue;
            }
            let old = pixel(frame, x, y);
            let fresh = sample(x, y);
            let mixed: [f32; 3] = std::array::from_fn(|k| old[k] * (1.0 - a) + fresh[k] * a);
            let yuv = to_yuv(mixed, hd);
            frame.y[y * frame.width + x] = yuv[0].round().clamp(0.0, 255.0) as u8;
            let e = chroma
                .entry((y / 2) * cwidth + x / 2)
                .or_insert(([0.0, 0.0], 0.0));
            e.0[0] += yuv[1];
            e.0[1] += yuv[2];
            e.1 += 1.0;
        }
    }
    for (c, ([u, v], n)) in chroma {
        frame.u[c] = (u / n).round().clamp(0.0, 255.0) as u8;
        frame.v[c] = (v / n).round().clamp(0.0, 255.0) as u8;
    }
}

const MEL_BINS: usize = 80;
const MEL_FRAMES: usize = 3000;
const N_FFT: usize = 400;
const HOP: usize = 160;
const WINDOW_SAMPLES: usize = 16_000 * 30;

/// The log-mel input of Whisper for 30 s of 16 kHz audio, as the Hugging Face
/// `WhisperFeatureExtractor`: [80][3000]. Longer audio is cut, shorter is padded.
pub fn whisper_mel(audio_16k: &[f32]) -> Vec<f32> {
    let mut x = vec![0.0f32; WINDOW_SAMPLES];
    let n = audio_16k.len().min(WINDOW_SAMPLES);
    x[..n].copy_from_slice(&audio_16k[..n]);
    // Centre the frames: reflect 200 samples at each end.
    let pad = N_FFT / 2;
    let mut padded = Vec::with_capacity(WINDOW_SAMPLES + 2 * pad);
    padded.extend((1..=pad).rev().map(|i| x[i]));
    padded.extend_from_slice(&x);
    padded.extend(
        (WINDOW_SAMPLES - pad - 1..WINDOW_SAMPLES - 1)
            .rev()
            .map(|i| x[i]),
    );
    let window: Vec<f32> = (0..N_FFT)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N_FFT as f32).cos())
        .collect();
    let filters = mel_filters();
    let fft = FftPlanner::new().plan_fft_forward(N_FFT);
    let mut mel = vec![0.0f32; MEL_BINS * MEL_FRAMES];
    let mut buf = vec![Complex::new(0.0f32, 0.0); N_FFT];
    for t in 0..MEL_FRAMES {
        for (i, c) in buf.iter_mut().enumerate() {
            *c = Complex::new(padded[t * HOP + i] * window[i], 0.0);
        }
        fft.process(&mut buf);
        for (m, filter) in filters.iter().enumerate() {
            let e: f32 = filter.iter().map(|&(k, w)| w * buf[k].norm_sqr()).sum();
            mel[m * MEL_FRAMES + t] = e.max(1e-10).log10();
        }
    }
    let top = mel.iter().copied().fold(f32::MIN, f32::max);
    mel.iter_mut()
        .for_each(|v| *v = ((*v).max(top - 8.0) + 4.0) / 4.0);
    mel
}

/// The Slaney mel filters of librosa (80 bins, 0-8000 Hz, Slaney norm) as (bin, weight).
fn mel_filters() -> Vec<Vec<(usize, f32)>> {
    let to_mel = |hz: f64| {
        if hz < 1000.0 {
            hz * 3.0 / 200.0
        } else {
            15.0 + (hz / 1000.0).ln() / (6.4f64.ln() / 27.0)
        }
    };
    let to_hz = |mel: f64| {
        if mel < 15.0 {
            mel * 200.0 / 3.0
        } else {
            1000.0 * ((mel - 15.0) * 6.4f64.ln() / 27.0).exp()
        }
    };
    let (lo, hi) = (to_mel(0.0), to_mel(8000.0));
    let points: Vec<f64> = (0..MEL_BINS + 2)
        .map(|i| to_hz(lo + (hi - lo) * i as f64 / (MEL_BINS + 1) as f64))
        .collect();
    let bins = N_FFT / 2 + 1;
    (0..MEL_BINS)
        .map(|m| {
            let (left, centre, right) = (points[m], points[m + 1], points[m + 2]);
            let norm = 2.0 / (right - left);
            (0..bins)
                .filter_map(|k| {
                    let f = 8000.0 * k as f64 / (bins - 1) as f64;
                    let w = ((f - left) / (centre - left)).min((right - f) / (right - centre));
                    (w > 0.0).then_some((k, (w * norm) as f32))
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_blend_covers_the_mouth_only() {
        assert_eq!(blend_weight(0.5, 0.3), 0.0);
        assert!(blend_weight(0.5, 0.75) > 0.99);
        assert_eq!(blend_weight(0.1, 0.75), 0.0);
        assert!(blend_weight(0.5, 0.97) < 0.5);
    }

    #[test]
    fn detail_comes_back_and_flat_areas_stay() {
        let plane = CROP * CROP;
        let flat = vec![0.5f32; 3 * plane];
        assert_eq!(with_detail(&flat, &flat), flat);
        // A fine stripe pattern in the original shows in the soft new crop.
        let stripes: Vec<f32> = (0..3 * plane)
            .map(|i| if i % 2 == 0 { 0.6 } else { 0.4 })
            .collect();
        let out = with_detail(&flat, &stripes);
        assert!((out[0] - out[1]).abs() > 0.05, "{} {}", out[0], out[1]);
    }

    #[test]
    fn colours_survive_the_round_trip() {
        for rgb in [[0.2f32, 0.5, 0.8], [0.9, 0.1, 0.3], [0.5, 0.5, 0.5]] {
            for hd in [false, true] {
                let yuv = to_yuv(rgb, hd);
                let back = to_rgb(yuv[0] as u8, yuv[1] as u8, yuv[2] as u8, hd);
                for k in 0..3 {
                    assert!((back[k] - rgb[k]).abs() < 0.02, "{rgb:?} {back:?}");
                }
            }
        }
    }

    #[test]
    fn faces_overlap() {
        let a = Face {
            x: 0.1,
            y: 0.1,
            w: 0.2,
            h: 0.2,
            nose_y: 0.2,
        };
        assert!((a.iou(&a) - 1.0).abs() < 1e-6);
        let b = Face { x: 0.5, ..a };
        assert_eq!(a.iou(&b), 0.0);
    }

    #[test]
    fn the_mel_has_whisper_shape() {
        let audio: Vec<f32> = (0..16_000).map(|i| (i as f32 * 0.1).sin() * 0.3).collect();
        let mel = whisper_mel(&audio);
        assert_eq!(mel.len(), MEL_BINS * MEL_FRAMES);
        let top = mel.iter().copied().fold(f32::MIN, f32::max);
        let bottom = mel.iter().copied().fold(f32::MAX, f32::min);
        assert!(top - bottom <= 2.0 + 1e-4, "{top} {bottom}");
    }
}
