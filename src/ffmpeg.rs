use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// ffmpeg がインストールされているか確認
pub fn check_ffmpeg() -> Result<()> {
    let status = Command::new("ffmpeg")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() => Ok(()),
        _ => bail!("ffmpeg が見つかりません。`brew install ffmpeg` でインストールしてください"),
    }
}

/// ffmpeg 実行の共通ラッパー。失敗時は stderr をエラーメッセージに含める
fn run_ffmpeg(configure: impl FnOnce(&mut Command)) -> Result<()> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
    configure(&mut cmd);

    let output = cmd.output().context("ffmpeg の起動に失敗しました")?;

    if !output.status.success() {
        bail!(
            "ffmpeg が異常終了しました:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// VideoToolbox ハードウェア加速で軽量 mp4 に変換
pub fn convert_to_mp4(input: &Path, output: &Path, codec: &str, bitrate: &str) -> Result<()> {
    run_ffmpeg(|cmd| {
        cmd.arg("-i")
            .arg(input)
            .args(["-vcodec", codec, "-b:v", bitrate])
            .args(["-c:a", "aac", "-b:a", "128k"])
            // ストリーミング再生向けに moov atom を先頭へ
            .args(["-movflags", "+faststart"])
            .arg(output);
    })
}

/// 音声のみを .m4a (AAC) として抽出
pub fn extract_audio_m4a(input: &Path, output: &Path) -> Result<()> {
    run_ffmpeg(|cmd| {
        cmd.arg("-i")
            .arg(input)
            .args(["-vn", "-c:a", "aac", "-b:a", "128k"])
            .arg(output);
    })
}

/// 入力動画の映像ストリームのビットレート (bps) を取得
pub fn probe_video_bitrate(input: &Path) -> Option<u64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=bit_rate",
            "-of",
            "csv=p=0",
        ])
        .arg(input)
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// whisper.cpp 入力用の 16kHz モノラル WAV を生成
pub fn extract_wav_16k(input: &Path, output: &Path) -> Result<()> {
    run_ffmpeg(|cmd| {
        cmd.arg("-i")
            .arg(input)
            .args(["-vn", "-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le"])
            .arg(output);
    })
}
