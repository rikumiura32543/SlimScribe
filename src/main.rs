mod cli;
mod ffmpeg;
mod finder;
mod progress;
mod whisper;

use anyhow::{bail, Context, Result};
use clap::Parser;
use indicatif::MultiProgress;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Instant;

fn main() -> Result<()> {
    let args = cli::Args::parse();
    ffmpeg::check_ffmpeg()?;

    let input = resolve_input(&args)?;
    print_target_info(&input);

    let outputs = plan_outputs(&input, &args)?;
    let started = Instant::now();
    let mp = MultiProgress::new();

    // 【並列実行】動画変換は別スレッド、音声抽出→文字起こしはメインスレッド
    let convert_handle = spawn_convert(&mp, &input, &outputs, &args);
    let audio_result = run_audio_pipeline(&mp, &input, &outputs, &args);

    let convert_result = convert_handle
        .map(|handle| {
            handle
                .join()
                .unwrap_or_else(|_| bail_join_error())
        })
        .transpose();

    print_summary(&outputs, started.elapsed().as_secs_f64());

    // どれか失敗していたら非ゼロ終了
    audio_result?;
    convert_result?;
    Ok(())
}

/// 入力ファイルを決定（--file 指定 > フォルダ内の最新 .mov）
/// 絶対パスに正規化することで、先頭ハイフンのファイル名が
/// ffmpeg / whisper にオプションとして誤解釈されるのを防ぐ
fn resolve_input(args: &cli::Args) -> Result<PathBuf> {
    let input = if let Some(file) = &args.file {
        if !file.is_file() {
            bail!("ファイルが存在しません: {}", file.display());
        }
        file.clone()
    } else {
        let dir = match &args.dir {
            Some(dir) => dir.clone(),
            None => dirs::home_dir()
                .context("ホームディレクトリを取得できません")?
                .join("Movies"),
        };
        finder::find_latest_mov(&dir)?
    };

    input
        .canonicalize()
        .with_context(|| format!("パスを正規化できません: {}", input.display()))
}

fn print_target_info(input: &Path) {
    let age = input
        .metadata()
        .ok()
        .and_then(|meta| meta.modified().ok())
        .map(finder::age_label)
        .unwrap_or_default();
    let size_mb = input
        .metadata()
        .map(|meta| meta.len() as f64 / 1024.0 / 1024.0)
        .unwrap_or(0.0);
    println!("🎬 対象: {} ({size_mb:.0} MB, {age})", input.display());
}

/// 出力先パス一覧
struct Outputs {
    mp4: Option<PathBuf>,
    m4a: PathBuf,
    txt: Option<PathBuf>,
    /// whisper -of に渡す拡張子なしのベースパス
    txt_base: PathBuf,
}

/// 出力パスを決定し、上書き衝突を事前チェック
fn plan_outputs(input: &Path, args: &cli::Args) -> Result<Outputs> {
    let outputs = Outputs {
        mp4: (!args.no_convert).then(|| input.with_extension("mp4")),
        m4a: input.with_extension("m4a"),
        txt: (!args.no_transcribe).then(|| input.with_extension("txt")),
        txt_base: input.with_extension(""),
    };

    // --file で .mp4 / .m4a 等を渡された場合、出力が入力を破壊しないよう防止
    let collision = [outputs.mp4.as_deref(), Some(&outputs.m4a), outputs.txt.as_deref()]
        .into_iter()
        .flatten()
        .find(|path| *path == input);
    if let Some(path) = collision {
        bail!(
            "出力先が入力ファイルと同一になるため処理できません: {}\n入力には .mov ファイルを指定してください",
            path.display()
        );
    }

    if !args.overwrite {
        let existing: Vec<String> = [outputs.mp4.as_deref(), Some(&outputs.m4a), outputs.txt.as_deref()]
            .into_iter()
            .flatten()
            .filter(|path| path.exists())
            .map(|path| path.display().to_string())
            .collect();
        if !existing.is_empty() {
            bail!(
                "出力ファイルが既に存在します（--overwrite で上書き可能）:\n  {}",
                existing.join("\n  ")
            );
        }
    }
    Ok(outputs)
}

/// 変換ビットレートを決定（明示指定 > 2M と元動画ビットレートの小さい方）
fn resolve_bitrate(input: &Path, explicit: Option<&str>) -> String {
    const DEFAULT_BPS: u64 = 2_000_000;

    if let Some(bitrate) = explicit {
        return bitrate.to_string();
    }
    match ffmpeg::probe_video_bitrate(input) {
        // 元動画が 2M 未満なら、それ以上のビットレートは肥大化するだけなので合わせる
        // （極端に低い値は ffmpeg の不正引数を避けるため 100k で下限クランプ）
        Some(source_bps) if source_bps < DEFAULT_BPS => {
            format!("{}k", (source_bps / 1000).max(100))
        }
        _ => "2M".to_string(),
    }
}

/// mp4 変換を別スレッドで開始
fn spawn_convert(
    mp: &MultiProgress,
    input: &Path,
    outputs: &Outputs,
    args: &cli::Args,
) -> Option<thread::JoinHandle<Result<()>>> {
    let output = outputs.mp4.clone()?;
    let codec = args.codec.ffmpeg_name();
    let bitrate = resolve_bitrate(input, args.bitrate.as_deref());
    let input = input.to_path_buf();
    let pb = progress::spinner(mp, format!("動画を変換中... ({codec}, {bitrate})"));

    Some(thread::spawn(move || {
        let result = ffmpeg::convert_to_mp4(&input, &output, codec, &bitrate);
        match &result {
            Ok(()) => progress::finish_ok(&pb, format!("動画変換 完了: {}", output.display())),
            Err(err) => progress::finish_err(&pb, format!("動画変換 失敗: {err}")),
        }
        result
    }))
}

/// 音声抽出 →（16kHz WAV 生成 → 文字起こし）を順次実行
fn run_audio_pipeline(
    mp: &MultiProgress,
    input: &Path,
    outputs: &Outputs,
    args: &cli::Args,
) -> Result<()> {
    let pb = progress::spinner(mp, "音声を抽出中... (m4a)".to_string());
    match ffmpeg::extract_audio_m4a(input, &outputs.m4a) {
        Ok(()) => progress::finish_ok(&pb, format!("音声抽出 完了: {}", outputs.m4a.display())),
        Err(err) => {
            progress::finish_err(&pb, format!("音声抽出 失敗: {err}"));
            return Err(err);
        }
    }

    if args.no_transcribe {
        return Ok(());
    }
    run_transcribe(mp, input, outputs, args)
}

fn run_transcribe(
    mp: &MultiProgress,
    input: &Path,
    outputs: &Outputs,
    args: &cli::Args,
) -> Result<()> {
    let pb = progress::spinner(mp, "文字起こしの準備中...".to_string());

    if !whisper::check_whisper(&args.whisper_bin) {
        progress::finish_skip(
            &pb,
            format!(
                "文字起こしスキップ: {} が見つかりません（brew install whisper-cpp）",
                args.whisper_bin
            ),
        );
        return Ok(());
    }
    let model = match whisper::resolve_model(args.whisper_model.as_deref()) {
        Ok(model) => model,
        Err(err) => {
            progress::finish_skip(&pb, format!("文字起こしスキップ: {err}"));
            return Ok(());
        }
    };

    // whisper.cpp は 16kHz WAV のみ受け付けるため一時ファイルを経由。
    // tempfile により推測不能な名前で作成され、スコープ離脱時に必ず削除される。
    // 再圧縮済み m4a ではなく元動画から直接抽出して音質劣化を避ける
    let wav_file = match tempfile::Builder::new()
        .prefix("slimscribe_")
        .suffix(".wav")
        .tempfile()
    {
        Ok(file) => file,
        Err(err) => {
            progress::finish_err(&pb, format!("一時ファイル作成 失敗: {err}"));
            return Err(err.into());
        }
    };
    let wav = wav_file.path().to_path_buf();
    if let Err(err) = ffmpeg::extract_wav_16k(input, &wav) {
        progress::finish_err(&pb, format!("WAV 変換 失敗: {err}"));
        return Err(err);
    }

    let model_name = model
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    pb.set_message(format!("文字起こし中... ({model_name}, 言語: {})", args.language));

    let result = whisper::transcribe(
        &args.whisper_bin,
        &model,
        &wav,
        &outputs.txt_base,
        &args.language,
    );
    drop(wav_file);

    match &result {
        Ok(()) => {
            let txt = outputs.txt.as_deref().unwrap_or(&outputs.txt_base);
            progress::finish_ok(&pb, format!("文字起こし 完了: {}", txt.display()));
        }
        Err(err) => progress::finish_err(&pb, format!("文字起こし 失敗: {err}")),
    }
    result
}

fn print_summary(outputs: &Outputs, elapsed_secs: f64) {
    println!("\n📦 出力先:");
    for path in [outputs.mp4.as_deref(), Some(&outputs.m4a), outputs.txt.as_deref()]
        .into_iter()
        .flatten()
        .filter(|path| path.exists())
    {
        let size_mb = path
            .metadata()
            .map(|meta| meta.len() as f64 / 1024.0 / 1024.0)
            .unwrap_or(0.0);
        println!("  {} ({size_mb:.1} MB)", path.display());
    }
    println!("⏱️  合計時間: {elapsed_secs:.1} 秒");
}

fn bail_join_error() -> Result<()> {
    Err(anyhow::anyhow!("動画変換スレッドが異常終了しました"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_args() -> cli::Args {
        cli::Args {
            dir: None,
            file: None,
            codec: cli::Codec::H264,
            bitrate: None,
            language: "ja".to_string(),
            whisper_model: None,
            whisper_bin: "whisper-cli".to_string(),
            no_transcribe: false,
            no_convert: false,
            overwrite: false,
        }
    }

    #[test]
    fn 出力パスは入力と同じフォルダ_拡張子違い() {
        let input = Path::new("/tmp/録画_20260708.mov");
        let outputs = plan_outputs(input, &test_args()).expect("plan_outputs 失敗");

        assert_eq!(outputs.mp4.as_deref(), Some(Path::new("/tmp/録画_20260708.mp4")));
        assert_eq!(outputs.m4a, Path::new("/tmp/録画_20260708.m4a"));
        assert_eq!(outputs.txt.as_deref(), Some(Path::new("/tmp/録画_20260708.txt")));
        assert_eq!(outputs.txt_base, Path::new("/tmp/録画_20260708"));
    }

    #[test]
    fn 入力と出力が衝突する場合はエラー() {
        // .m4a 入力 → 音声出力パスと同一になり入力破壊のリスク
        assert!(plan_outputs(Path::new("/tmp/audio.m4a"), &test_args()).is_err());
        // .mp4 入力 → 変換出力パスと同一
        assert!(plan_outputs(Path::new("/tmp/video.mp4"), &test_args()).is_err());
    }

    #[test]
    fn 既存出力があればエラー_overwriteで許可() {
        let dir = tempfile::tempdir().expect("tempdir 作成失敗");
        let input = dir.path().join("rec.mov");
        std::fs::write(dir.path().join("rec.mp4"), b"existing").expect("書き込み失敗");

        assert!(plan_outputs(&input, &test_args()).is_err());

        let args = cli::Args {
            overwrite: true,
            ..test_args()
        };
        assert!(plan_outputs(&input, &args).is_ok());
    }

    #[test]
    fn スキップ指定で出力パスがnoneになる() {
        let args = cli::Args {
            no_convert: true,
            no_transcribe: true,
            ..test_args()
        };
        let outputs = plan_outputs(Path::new("/tmp/rec.mov"), &args).expect("plan_outputs 失敗");
        assert!(outputs.mp4.is_none());
        assert!(outputs.txt.is_none());
    }

    #[test]
    fn ビットレート明示指定はそのまま使用() {
        let bitrate = resolve_bitrate(Path::new("/nonexistent.mov"), Some("4M"));
        assert_eq!(bitrate, "4M");
    }

    #[test]
    fn ビットレート検出不能時は既定の2m() {
        let bitrate = resolve_bitrate(Path::new("/nonexistent.mov"), None);
        assert_eq!(bitrate, "2M");
    }

    #[test]
    fn コーデック名はvideotoolbox() {
        assert_eq!(cli::Codec::H264.ffmpeg_name(), "h264_videotoolbox");
        assert_eq!(cli::Codec::Hevc.ffmpeg_name(), "hevc_videotoolbox");
    }
}
