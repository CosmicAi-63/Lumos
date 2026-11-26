//! Media post-processing. M0 drives the `ffmpeg`/`ffprobe` command-line tools;
//! linking the FFmpeg libraries (LGPL build) replaces this later without
//! changing these signatures.
//!
//! Overrides: `LUMOS_FFMPEG`, `LUMOS_FFPROBE`, `LUMOS_TEST_VCODEC`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use tokio::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("ffmpeg not found ({0}); install it (brew install ffmpeg) or set LUMOS_FFMPEG")]
    Missing(String),
    #[error("{tool} failed: {stderr}")]
    Failed { tool: &'static str, stderr: String },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, MediaError>;

/// Where package managers put command-line tools. An app started from Finder gets only
/// `/usr/bin:/bin:/usr/sbin:/sbin` as its PATH, so `ffmpeg` from Homebrew is invisible without these.
const EXTRA_BIN_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/usr/bin"];

/// The binary to run for `name`: the `env_var` override, else the first match on PATH or in
/// [`EXTRA_BIN_DIRS`], else the bare name (so the error says what is missing).
pub fn resolve_binary(name: &str, env_var: &str) -> PathBuf {
    if let Some(p) = std::env::var_os(env_var) {
        return PathBuf::from(p);
    }
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    find_in(name, path_dirs.into_iter().chain(EXTRA_BIN_DIRS.iter().map(PathBuf::from)))
}

/// The first `dir/name` that is a file, else the bare `name`.
fn find_in(name: &str, dirs: impl IntoIterator<Item = PathBuf>) -> PathBuf {
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file()).unwrap_or_else(|| name.into())
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    #[test]
    fn finds_a_tool_outside_the_minimal_finder_path() {
        // An app started from Finder sees only these directories, not Homebrew's.
        let minimal = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(PathBuf::from);
        let homebrew = tempfile::tempdir().unwrap();
        std::fs::write(homebrew.path().join("faketool"), "").unwrap();
        assert_eq!(find_in("faketool", minimal.clone()), PathBuf::from("faketool"), "not found without the extra directory");
        let dirs = minimal.into_iter().chain([homebrew.path().to_path_buf()]);
        assert_eq!(find_in("faketool", dirs), homebrew.path().join("faketool"));
    }

    #[test]
    fn homebrew_locations_are_searched() {
        assert!(EXTRA_BIN_DIRS.contains(&"/opt/homebrew/bin") && EXTRA_BIN_DIRS.contains(&"/usr/local/bin"));
    }
}

fn ffmpeg() -> PathBuf {
    resolve_binary("ffmpeg", "LUMOS_FFMPEG")
}

fn ffprobe() -> PathBuf {
    resolve_binary("ffprobe", "LUMOS_FFPROBE")
}

/// First line of `ffmpeg -version`, or `None` if ffmpeg can't run.
pub async fn ffmpeg_version() -> Option<String> {
    let out = Command::new(ffmpeg()).arg("-version").output().await.ok()?;
    String::from_utf8_lossy(&out.stdout).lines().next().map(str::to_owned)
}

async fn run(tool: &'static str, bin: PathBuf, args: Vec<OsString>) -> Result<String> {
    let out = Command::new(&bin).args(&args).output().await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound { MediaError::Missing(bin.display().to_string()) } else { MediaError::Io(e) }
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(6).collect();
        return Err(MediaError::Failed { tool, stderr: tail.into_iter().rev().collect::<Vec<_>>().join("\n") });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn ff(args: Vec<OsString>) -> Result<()> {
    let mut all: Vec<OsString> = vec!["-hide_banner".into(), "-loglevel".into(), "error".into(), "-y".into()];
    all.extend(args);
    run("ffmpeg", ffmpeg(), all).await.map(|_| ())
}

macro_rules! args {
    ($($a:expr),* $(,)?) => { vec![$(OsString::from($a)),*] };
}

fn test_vcodec() -> String {
    std::env::var("LUMOS_TEST_VCODEC").unwrap_or_else(|_| if cfg!(target_os = "macos") { "h264_videotoolbox".into() } else { "libx264".into() })
}

/// Synthetic video: moving test pattern with a frame counter. Used by the fake backend.
pub async fn generate_test_video(out: &Path, duration_us: i64, width: u32, height: u32, fps: u32) -> Result<()> {
    let secs = format!("{:.6}", duration_us.max(1) as f64 / 1e6);
    ff(args![
        "-f", "lavfi",
        "-i", format!("testsrc2=size={width}x{height}:rate={fps}:duration={secs}"),
        "-c:v", test_vcodec(),
        "-pix_fmt", "yuv420p",
        "-b:v", "2M",
        out,
    ])
    .await
}

/// Synthetic audio: a sine tone. Used by the fake backend.
pub async fn generate_test_tone(out: &Path, duration_us: i64, frequency_hz: u32, channels: u32) -> Result<()> {
    let secs = format!("{:.6}", duration_us.max(1) as f64 / 1e6);
    ff(args![
        "-f", "lavfi",
        "-i", format!("sine=frequency={frequency_hz}:sample_rate=48000:duration={secs}"),
        "-ac", channels.to_string(),
        "-c:a", "aac",
        "-b:a", "128k",
        out,
    ])
    .await
}

/// Join files with identical codecs, without re-encoding.
pub async fn concat(inputs: &[PathBuf], out: &Path) -> Result<()> {
    if inputs.len() == 1 {
        tokio::fs::copy(&inputs[0], out).await?;
        return Ok(());
    }
    let list = out.with_extension("concat.txt");
    let body: String = inputs
        .iter()
        .map(|p| format!("file '{}'\n", p.display().to_string().replace('\'', r"'\''")))
        .collect();
    tokio::fs::write(&list, body).await?;
    let res = ff(args!["-f", "concat", "-safe", "0", "-i", &list, "-c", "copy", out]).await;
    let _ = tokio::fs::remove_file(&list).await;
    res
}

/// Where the first audio sample sits on the file's own clock, in microseconds (0 if unknown).
pub async fn probe_audio_start_us(input: &Path) -> Result<i64> {
    let out = run(
        "ffprobe",
        ffprobe(),
        args!["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=start_time", "-of", "default=noprint_wrappers=1:nokey=1", input],
    )
    .await?;
    Ok(out.trim().parse::<f64>().map(|s| (s * 1e6).round() as i64).unwrap_or(0))
}

/// Re-time one audio segment onto the video clock: its first sample lands at its own start time plus
/// `shift_us`, and the result is exactly `duration_us` long (silence-padded or cut). Writes 48 kHz PCM.
pub async fn fit_audio(input: &Path, out: &Path, duration_us: i64, shift_us: i64) -> Result<()> {
    let at = probe_audio_start_us(input).await? + shift_us;
    let dur = duration_us.max(0) as f64 / 1e6;
    // Timestamps are rebuilt from the sample count so the file's own start offset can't leak into the trims.
    let mut graph = String::from("asetpts=N/SR/TB,");
    if at > 0 {
        graph.push_str(&format!("adelay={:.3}:all=1,asetpts=N/SR/TB,", at as f64 / 1000.0));
    } else if at < 0 {
        graph.push_str(&format!("atrim=start={:.6},asetpts=N/SR/TB,", (-at) as f64 / 1e6));
    }
    graph.push_str(&format!("apad=whole_dur={dur:.6},atrim=end={dur:.6},asetpts=N/SR/TB"));
    ff(args!["-i", input, "-vn", "-af", graph, "-ar", "48000", "-c:a", "pcm_s16le", out]).await
}

/// Mix several audio files into one AAC track. One AAC input is copied as is.
pub async fn mix_audio(inputs: &[PathBuf], gains_db: &[f64], out: &Path) -> Result<()> {
    let copyable = inputs.first().is_some_and(|p| p.extension().is_some_and(|e| e == "m4a"));
    if inputs.len() == 1 && copyable && gains_db.first().copied().unwrap_or(0.0) == 0.0 {
        tokio::fs::copy(&inputs[0], out).await?;
        return Ok(());
    }
    let mut a = Vec::new();
    for i in inputs {
        a.extend(args!["-i", i]);
    }
    let mut graph = String::new();
    for (i, g) in gains_db.iter().enumerate() {
        graph.push_str(&format!("[{i}:a]volume={g}dB[a{i}];"));
    }
    for i in 0..inputs.len() {
        graph.push_str(&format!("[a{i}]"));
    }
    graph.push_str(&format!("amix=inputs={}:normalize=0,alimiter=limit=0.95[out]", inputs.len()));
    a.extend(args!["-filter_complex", graph, "-map", "[out]", "-c:a", "aac", "-b:a", "160k", out]);
    ff(a).await
}

/// Put a video and an optional audio track into one MP4 that starts fast on the web.
pub async fn mux(video: &Path, audio: Option<&Path>, out: &Path) -> Result<()> {
    let mut a = args!["-i", video];
    if let Some(audio) = audio {
        a.extend(args!["-i", audio, "-map", "0:v:0", "-map", "1:a:0"]);
    } else {
        a.extend(args!["-map", "0:v:0"]);
    }
    a.extend(args!["-c", "copy", "-movflags", "+faststart", out]);
    ff(a).await
}

/// JPEG still at `at_us`, 640 px wide.
pub async fn thumbnail(input: &Path, at_us: i64, out: &Path) -> Result<()> {
    let ss = format!("{:.3}", at_us.max(0) as f64 / 1e6);
    ff(args!["-ss", ss, "-i", input, "-frames:v", "1", "-vf", "scale=640:-2", out]).await
}

/// Container duration in microseconds.
pub async fn probe_duration_us(input: &Path) -> Result<i64> {
    let out = run(
        "ffprobe",
        ffprobe(),
        args!["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1", input],
    )
    .await?;
    let secs: f64 = out.trim().parse().map_err(|_| MediaError::Failed { tool: "ffprobe", stderr: format!("unexpected duration '{}'", out.trim()) })?;
    Ok((secs * 1e6).round() as i64)
}

/// Codec names of every stream, e.g. `["video", "audio"]` (as `codec_type`).
pub async fn probe_streams(input: &Path) -> Result<Vec<String>> {
    let out = run("ffprobe", ffprobe(), args!["-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0", input]).await?;
    Ok(out.lines().map(|l| l.trim().to_owned()).filter(|l| !l.is_empty()).collect())
}

/// Options for [`transcode`].
pub struct Transcode<'a> {
    pub input: &'a Path,
    pub output: &'a Path,
    /// `mp4`, `gif` or `audio`.
    pub kind: &'a str,
    pub max_height: Option<u32>,
    pub fps: Option<u32>,
    pub video_kbps: Option<u32>,
    /// Keep only `[start, end)` seconds of the input.
    pub start_seconds: Option<f64>,
    pub end_seconds: Option<f64>,
}

/// Transcribe `input` with whisper.cpp. Writes `<out_base>.srt`, `.vtt` and `.txt`.
/// The binary is `whisper-cli`, or `$LUMOS_WHISPER`.
pub async fn transcribe(input: &Path, model: &Path, language: &str, out_base: &Path) -> Result<()> {
    let wav = out_base.with_extension("tmp.wav");
    ff(args!["-i", input, "-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", &wav]).await?;
    let bin = resolve_binary("whisper-cli", "LUMOS_WHISPER");
    let r = run(
        "whisper-cli",
        bin,
        args!["-m", model, "-f", &wav, "-l", language, "-osrt", "-ovtt", "-otxt", "-of", out_base],
    )
    .await;
    let _ = std::fs::remove_file(&wav);
    r.map(|_| ())
}

/// Re-encode for sharing: MP4 (H.264/AAC), GIF, or AAC audio only.
pub async fn transcode(t: Transcode<'_>) -> Result<()> {
    let h = t.max_height.map(|h| format!("min({h}\\,ih)"));
    let mut a = args![];
    let start = t.start_seconds.unwrap_or(0.0);
    if start > 0.0 {
        a.extend(args!["-ss", format!("{start:.3}")]);
    }
    if let Some(end) = t.end_seconds {
        a.extend(args!["-t", format!("{:.3}", (end - start).max(0.001))]);
    }
    a.extend(args!["-i", t.input]);
    match t.kind {
        "gif" => {
            let fps = t.fps.unwrap_or(12);
            let scale = h.map(|h| format!("scale=-2:{h}:flags=lanczos")).unwrap_or_else(|| "scale=720:-2:flags=lanczos".into());
            a.extend(args!["-an", "-vf", format!("fps={fps},{scale},split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse"), "-loop", "0"]);
        }
        "audio" => a.extend(args!["-vn", "-c:a", "aac", "-b:a", "160k"]),
        _ => {
            if let Some(h) = h {
                a.extend(args!["-vf", format!("scale=-2:{h}")]);
            }
            if let Some(f) = t.fps {
                a.extend(args!["-r", f.to_string()]);
            }
            a.extend(args!["-c:v", test_vcodec(), "-pix_fmt", "yuv420p", "-b:v", format!("{}k", t.video_kbps.unwrap_or(4000)), "-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart"]);
        }
    }
    a.push(t.output.into());
    ff(a).await
}

#[cfg(test)]
mod fit_audio_tests {
    use super::*;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime").block_on(f)
    }

    #[test]
    fn fit_audio_pads_and_trims_to_the_segment_length() {
        if block_on(ffmpeg_version()).is_none() {
            return; // no ffmpeg on this machine
        }
        let dir = tempfile::tempdir().unwrap();
        let tone = dir.path().join("tone.m4a");
        block_on(generate_test_tone(&tone, 1_000_000, 440, 1)).unwrap();

        // A 1 s tone shifted 100 ms into a 1.5 s segment: padded at both ends to exactly 1.5 s.
        let padded = dir.path().join("padded.wav");
        block_on(fit_audio(&tone, &padded, 1_500_000, 100_000)).unwrap();
        let d = block_on(probe_duration_us(&padded)).unwrap();
        assert!((d - 1_500_000).abs() < 30_000, "padded duration {d}");

        // The same tone cut to a 0.5 s segment: trimmed.
        let cut = dir.path().join("cut.wav");
        block_on(fit_audio(&tone, &cut, 500_000, 0)).unwrap();
        let d = block_on(probe_duration_us(&cut)).unwrap();
        assert!((d - 500_000).abs() < 30_000, "cut duration {d}");

        // A negative shift drops the head of the audio but keeps the segment length.
        let early = dir.path().join("early.wav");
        block_on(fit_audio(&tone, &early, 800_000, -300_000)).unwrap();
        let d = block_on(probe_duration_us(&early)).unwrap();
        assert!((d - 800_000).abs() < 30_000, "early duration {d}");
    }
}
