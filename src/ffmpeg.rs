use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

use crate::cli::Codec;

/// プラットフォームに応じたエンコーダを選択
/// macOS は VideoToolbox 固定、それ以外は利用可能なハードウェアエンコーダ
/// (NVENC > QSV > AMF) を探し、なければソフトウェアエンコーダにフォールバック
pub fn select_encoder(codec: Codec) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        Ok(match codec {
            Codec::H264 => "h264_videotoolbox",
            Codec::Hevc => "hevc_videotoolbox",
        }
        .to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let output = Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .output()
            .context("ffmpeg の起動に失敗しました")?;
        let available = String::from_utf8_lossy(&output.stdout).into_owned();

        let candidates: &[&str] = match codec {
            Codec::H264 => &["h264_nvenc", "h264_qsv", "h264_amf", "libx264"],
            Codec::Hevc => &["hevc_nvenc", "hevc_qsv", "hevc_amf", "libx265"],
        };
        for name in candidates {
            if available.contains(name) {
                return Ok(name.to_string());
            }
        }
        bail!("利用可能な {codec:?} エンコーダが見つかりません（ffmpeg のビルド構成を確認してください）");
    }
}

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

/// ハードウェア加速で軽量 mp4 に変換（単一ファイル）
pub fn convert_to_mp4(input: &Path, output: &Path, encoder: &str, video_bps: u64) -> Result<()> {
    run_ffmpeg(|cmd| {
        cmd.arg("-i")
            .arg(input)
            .args(["-vcodec", encoder])
            .args(["-b:v", &video_bps.to_string()])
            .args(["-c:a", "aac", "-b:a", "128k"])
            // ストリーミング再生向けに moov atom を先頭へ
            .args(["-movflags", "+faststart"])
            .arg(output);
    })
}

/// segment_secs ごとに分割しながら mp4 に変換
/// pattern には `_part%03d.mp4` 形式の出力テンプレートを渡す
pub fn convert_to_mp4_segments(
    input: &Path,
    pattern: &Path,
    encoder: &str,
    video_bps: u64,
    segment_secs: u64,
) -> Result<()> {
    run_ffmpeg(|cmd| {
        cmd.arg("-i")
            .arg(input)
            .args(["-vcodec", encoder])
            .args(["-b:v", &video_bps.to_string()])
            .args(["-c:a", "aac", "-b:a", "128k"])
            // 分割境界で確実にキーフレームを打ち、セグメント長のずれを防ぐ
            .arg("-force_key_frames")
            .arg(format!("expr:gte(t,n_forced*{segment_secs})"))
            .args(["-f", "segment"])
            .args(["-segment_time", &segment_secs.to_string()])
            .args(["-reset_timestamps", "1"])
            .args(["-segment_format_options", "movflags=+faststart"])
            .arg(pattern);
    })
}

/// 入力動画の長さ（秒）を取得
pub fn probe_duration(input: &Path) -> Option<f64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(input)
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
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
