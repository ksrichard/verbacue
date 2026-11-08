//! End-to-end tests that run the real binary. They download models on first use, so
//! they are ignored by default. Run them with:
//!
//!     cargo test --release -- --ignored
//!
//! On macOS the test audio is real speech from the `say` command. Elsewhere it is a
//! sine tone, which only checks that the pipeline runs.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const SENTENCE: &str = "Hello John. Where were you last night? I waited for you at the station.";

fn work_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("verbacue-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes test audio to `dir/input.<ext>` and says if it contains real speech.
fn make_audio(dir: &Path) -> (PathBuf, bool) {
    let aiff = dir.join("input.aiff");
    // The default voice follows the system language, so ask for an English voice.
    let say = |voice: Option<&str>| {
        let mut cmd = Command::new("say");
        if let Some(v) = voice {
            cmd.args(["-v", v]);
        }
        cmd.arg("-o")
            .arg(&aiff)
            .arg(SENTENCE)
            .status()
            .is_ok_and(|s| s.success())
    };
    let spoken = say(Some("Samantha")) || say(None);
    if spoken {
        return (aiff, true);
    }
    let wav = dir.join("input.wav");
    write_sine_wav(&wav, 3.0);
    (wav, false)
}

/// A 16-bit mono PCM WAV with a 440 Hz tone.
fn write_sine_wav(path: &Path, seconds: f32) {
    let rate = 16_000u32;
    let samples: Vec<i16> = (0..(rate as f32 * seconds) as u32)
        .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 8000.0) as i16)
        .collect();
    let data_len = (samples.len() * 2) as u32;
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((36 + data_len).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(rate.to_le_bytes());
    bytes.extend((rate * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data_len.to_le_bytes());
    for s in samples {
        bytes.extend(s.to_le_bytes());
    }
    fs::write(path, bytes).unwrap();
}

fn verbacue(args: &[&str]) {
    let status = Command::new(env!("CARGO_BIN_EXE_verbacue"))
        .args(args)
        .status()
        .expect("verbacue did not start");
    assert!(status.success(), "verbacue {args:?} failed");
}

#[test]
#[ignore = "downloads the tiny Whisper model"]
fn transcribes_audio_to_srt_with_the_tiny_model() {
    let dir = work_dir("transcribe");
    let (input, spoken) = make_audio(&dir);

    verbacue(&[
        input.to_str().unwrap(),
        "--model",
        "tiny",
        "--translator",
        "none",
    ]);

    let srt_path = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "srt"))
        .expect("no SRT written");
    let srt = fs::read_to_string(&srt_path).unwrap();
    if spoken {
        assert!(srt_path.ends_with("input.en.srt"), "{srt_path:?}");
        assert!(srt.starts_with("1\n00:00:00,"), "{srt}");
        let lower = srt.to_lowercase();
        assert!(lower.contains("john") && lower.contains("station"), "{srt}");
    }
}

#[test]
#[ignore = "downloads the tiny Whisper model and the 7 GB translation model"]
fn full_pipeline_translates_and_writes_videos() {
    let dir = work_dir("full");
    let (input, spoken) = make_audio(&dir);
    if !spoken {
        // A tone has no words to translate.
        return;
    }
    let video = dir.join("movie.mp4");
    // The ffmpeg command line tool makes the test video (a black picture plus the audio).
    // verbacue itself does not need it. The test skips if ffmpeg is not installed.
    let made = Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=320x240:r=10",
        ])
        .arg("-i")
        .arg(&input)
        .args(["-shortest", "-c:v", "mpeg4", "-c:a", "aac"])
        .arg(&video)
        .status()
        .is_ok_and(|s| s.success());
    if !made {
        eprintln!("skipped: ffmpeg is needed to make the test video");
        return;
    }

    verbacue(&[
        video.to_str().unwrap(),
        "--model",
        "tiny",
        "--to",
        "hu",
        "--embed",
        "--burn",
    ]);

    let hu = fs::read_to_string(dir.join("movie.hu.srt")).unwrap();
    assert!(hu.contains("John"), "{hu}");
    assert!(dir.join("movie.en.srt").is_file());
    assert!(dir.join("movie.hu.mkv").metadata().unwrap().len() > 0);
    assert!(dir.join("movie.hu.mp4").metadata().unwrap().len() > 0);

    // A second run skips the file because the outputs exist.
    verbacue(&[
        video.to_str().unwrap(),
        "--model",
        "tiny",
        "--to",
        "hu",
        "--embed",
        "--burn",
    ]);
}
