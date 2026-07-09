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

/// m4a / mp4 音声トラックのビットレート (128kbps)
const AUDIO_BPS: u64 = 128_000;

fn main() -> Result<()> {
    let args = cli::Args::parse();
    ffmpeg::check_ffmpeg()?;

    let input = resolve_input(&args)?;
    print_target_info(&input);

    let outputs = plan_outputs(&input, &args)?;
    let started = Instant::now();
    let mp = MultiProgress::new();

    // 【並列実行】動画変換は別スレッド、音声抽出→文字起こしはメインスレッド
    let convert_handle = spawn_convert(&mp, &input, &outputs, &args)?;
    let audio_result = run_audio_pipeline(&mp, &input, &outputs, &args);

    let convert_result: Result<Vec<PathBuf>> = match convert_handle {
        None => Ok(Vec::new()),
        Some(handle) => handle
            .join()
            .unwrap_or_else(|_| Err(anyhow::anyhow!("動画変換スレッドが異常終了しました"))),
    };

    let mut produced: Vec<PathBuf> = convert_result.as_deref().unwrap_or(&[]).to_vec();
    produced.push(outputs.m4a.clone());
    if let Some(txt) = &outputs.txt {
        produced.push(txt.clone());
    }
    print_summary(&produced, started.elapsed().as_secs_f64());

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
            // macOS: ~/Movies, Windows: Videos フォルダ
            None => dirs::video_dir()
                .or_else(|| dirs::home_dir().map(|home| home.join("Movies")))
                .context("動画フォルダを取得できません")?,
        };
        finder::find_latest_media(&dir, &args.ext)?
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

/// mp4 の出力先。入力が既に .mp4 の場合は `_slim.mp4` を付けて上書きを回避
/// （macOS 等の大文字小文字非区別ファイルシステムを考慮し .MP4 も同一視）
fn mp4_output_path(input: &Path) -> PathBuf {
    let is_mp4 = input
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case("mp4"))
        .unwrap_or(false);
    if !is_mp4 {
        return input.with_extension("mp4");
    }
    let stem = input
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string());
    input.with_file_name(format!("{stem}_slim.mp4"))
}

/// 大文字小文字非区別ファイルシステムを考慮したパス一致判定
fn paths_conflict(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    a.parent() == b.parent()
        && match (a.file_name(), b.file_name()) {
            (Some(name_a), Some(name_b)) => name_a
                .to_string_lossy()
                .eq_ignore_ascii_case(&name_b.to_string_lossy()),
            _ => false,
        }
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
        mp4: (!args.no_convert).then(|| mp4_output_path(input)),
        m4a: input.with_extension("m4a"),
        txt: (!args.no_transcribe).then(|| input.with_extension("txt")),
        txt_base: input.with_extension(""),
    };

    // 拡張子の付け替えで防げない衝突（.m4a 入力等）を最終ガード
    let collision = [outputs.mp4.as_deref(), Some(&outputs.m4a), outputs.txt.as_deref()]
        .into_iter()
        .flatten()
        .find(|path| paths_conflict(path, input));
    if let Some(path) = collision {
        bail!(
            "出力先が入力ファイルと同一になるため処理できません: {}\n入力には動画ファイル（mov, mp4, mkv 等）を指定してください",
            path.display()
        );
    }

    if !args.overwrite {
        let mut existing: Vec<String> = [outputs.mp4.as_deref(), Some(&outputs.m4a), outputs.txt.as_deref()]
            .into_iter()
            .flatten()
            .filter(|path| path.exists())
            .map(|path| path.display().to_string())
            .collect();
        // 分割出力（*_part000.mp4 等）の既存分も上書き対象として確認
        if let (Some(mp4), Some(dir)) = (&outputs.mp4, input.parent()) {
            if let Some(stem) = mp4.file_stem() {
                let prefix = format!("{}_part", stem.to_string_lossy());
                existing.extend(
                    collect_parts(dir, &prefix)
                        .unwrap_or_default()
                        .iter()
                        .map(|path| path.display().to_string()),
                );
            }
        }
        if !existing.is_empty() {
            bail!(
                "出力ファイルが既に存在します（--overwrite で上書き可能）:\n  {}",
                existing.join("\n  ")
            );
        }
    }
    Ok(outputs)
}

/// "2M" / "419k" / "800000" 形式の文字列を bps に変換
fn parse_bitrate(value: &str) -> Option<u64> {
    let value = value.trim();
    let (number, unit) = match value.chars().last()? {
        'k' | 'K' => (&value[..value.len() - 1], 1_000.0),
        'm' | 'M' => (&value[..value.len() - 1], 1_000_000.0),
        _ => (value, 1.0),
    };
    let parsed: f64 = number.parse().ok()?;
    (parsed > 0.0).then_some((parsed * unit) as u64)
}

/// 変換ビットレート (bps) を決定（明示指定 > 2M と元動画ビットレートの小さい方）
fn resolve_bitrate(input: &Path, explicit: Option<&str>) -> Result<u64> {
    const DEFAULT_BPS: u64 = 2_000_000;
    const MIN_BPS: u64 = 100_000;

    if let Some(value) = explicit {
        return parse_bitrate(value)
            .with_context(|| format!("ビットレート指定を解釈できません: {value}（例: 2M, 500k）"));
    }
    Ok(match ffmpeg::probe_video_bitrate(input) {
        // 元動画が 2M 未満なら、それ以上のビットレートは肥大化するだけなので合わせる
        Some(source_bps) if source_bps < DEFAULT_BPS => source_bps.max(MIN_BPS),
        _ => DEFAULT_BPS,
    })
}

/// 分割サイズ見積りの安全係数（平均ビットレートの揺らぎを吸収）
const SPLIT_SAFETY: f64 = 0.85;

/// 推定出力サイズが上限を超える場合、上限内に収まるセグメント長（秒）を返す
fn plan_split(duration_secs: f64, total_bps: u64, max_bytes: u64) -> Option<u64> {
    if max_bytes == 0 || duration_secs <= 0.0 || total_bps == 0 {
        return None;
    }
    let estimated_bytes = total_bps as f64 / 8.0 * duration_secs;
    let target_bytes = max_bytes as f64 * SPLIT_SAFETY;
    if estimated_bytes <= target_bytes {
        return None;
    }
    // サイズ上限の保証を優先（上限が小さい場合はセグメントも短くなる）
    let segment_secs = (target_bytes * 8.0 / total_bps as f64).floor() as u64;
    Some(segment_secs.max(1))
}

/// mp4 変換を別スレッドで開始。生成したファイル一覧を返すスレッドハンドルを返す
fn spawn_convert(
    mp: &MultiProgress,
    input: &Path,
    outputs: &Outputs,
    args: &cli::Args,
) -> Result<Option<thread::JoinHandle<Result<Vec<PathBuf>>>>> {
    let Some(output) = outputs.mp4.clone() else {
        return Ok(None);
    };
    let encoder = ffmpeg::select_encoder(args.codec)?;
    let video_bps = resolve_bitrate(input, args.bitrate.as_deref())?;
    let duration = ffmpeg::probe_duration(input).unwrap_or(0.0);
    let segment_secs = plan_split(
        duration,
        video_bps + AUDIO_BPS,
        args.max_size_mb.saturating_mul(1_000_000),
    );

    let label = match segment_secs {
        Some(secs) => {
            let parts = (duration / secs as f64).ceil() as u64;
            format!(
                "動画を変換中... ({encoder}, {}k, 約{parts}ファイルに分割)",
                video_bps / 1000
            )
        }
        None => format!("動画を変換中... ({encoder}, {}k)", video_bps / 1000),
    };
    let pb = progress::spinner(mp, label);
    let input = input.to_path_buf();

    Ok(Some(thread::spawn(move || {
        let result = run_convert(&input, &output, &encoder, video_bps, segment_secs);
        match &result {
            Ok(files) if files.len() == 1 => {
                progress::finish_ok(&pb, format!("動画変換 完了: {}", files[0].display()));
            }
            Ok(files) => {
                progress::finish_ok(&pb, format!("動画変換 完了: {}ファイルに分割", files.len()));
            }
            Err(err) => progress::finish_err(&pb, format!("動画変換 失敗: {err}")),
        }
        result
    })))
}

/// 変換本体。分割なしなら単一ファイル、分割ありなら `_part000.mp4` 連番を生成
fn run_convert(
    input: &Path,
    output: &Path,
    encoder: &str,
    video_bps: u64,
    segment_secs: Option<u64>,
) -> Result<Vec<PathBuf>> {
    let Some(secs) = segment_secs else {
        ffmpeg::convert_to_mp4(input, output, encoder, video_bps)?;
        return Ok(vec![output.to_path_buf()]);
    };

    let dir = output.parent().context("出力先フォルダを特定できません")?;
    let stem = output
        .file_stem()
        .context("出力ファイル名を特定できません")?
        .to_string_lossy()
        .into_owned();
    let prefix = format!("{stem}_part");

    // 前回の分割出力が残っていると新旧が混在するため先に削除
    // （既存チェックは plan_outputs 済み = ここに来る時点で上書き許可済み）
    for old in collect_parts(dir, &prefix)? {
        std::fs::remove_file(&old)
            .with_context(|| format!("既存ファイルを削除できません: {}", old.display()))?;
    }

    let pattern = dir.join(format!("{prefix}%03d.mp4"));
    ffmpeg::convert_to_mp4_segments(input, &pattern, encoder, video_bps, secs)?;

    let parts = collect_parts(dir, &prefix)?;
    if parts.is_empty() {
        bail!("分割出力が生成されませんでした");
    }
    Ok(parts)
}

/// prefix で始まる .mp4 ファイルをソート済みで収集
fn collect_parts(dir: &Path, prefix: &str) -> Result<Vec<PathBuf>> {
    let mut parts: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("フォルダを読めません: {}", dir.display()))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            let is_mp4 = path
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("mp4"))
                .unwrap_or(false);
            let matches_prefix = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().starts_with(prefix))
                .unwrap_or(false);
            is_mp4 && matches_prefix
        })
        .collect();
    parts.sort();
    Ok(parts)
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

fn print_summary(produced: &[PathBuf], elapsed_secs: f64) {
    println!("\n📦 出力先:");
    for path in produced.iter().filter(|path| path.exists()) {
        let size_mb = path
            .metadata()
            .map(|meta| meta.len() as f64 / 1024.0 / 1024.0)
            .unwrap_or(0.0);
        println!("  {} ({size_mb:.1} MB)", path.display());
    }
    println!("⏱️  合計時間: {elapsed_secs:.1} 秒");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_args() -> cli::Args {
        cli::Args {
            dir: None,
            file: None,
            ext: ["mov", "mp4", "mkv", "flv", "ts"].map(String::from).to_vec(),
            codec: cli::Codec::H264,
            max_size_mb: 200,
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
    fn mp4入力はslimサフィックスで上書きを回避() {
        assert_eq!(
            mp4_output_path(Path::new("/tmp/video.mp4")),
            Path::new("/tmp/video_slim.mp4")
        );
        // 大文字拡張子も同一ファイル扱い（大文字小文字非区別FS対策）
        assert_eq!(
            mp4_output_path(Path::new("/tmp/video.MP4")),
            Path::new("/tmp/video_slim.mp4")
        );
        assert_eq!(
            mp4_output_path(Path::new("/tmp/video.mkv")),
            Path::new("/tmp/video.mp4")
        );

        let outputs = plan_outputs(Path::new("/tmp/video.mp4"), &test_args()).expect("失敗");
        assert_eq!(outputs.mp4.as_deref(), Some(Path::new("/tmp/video_slim.mp4")));
    }

    #[test]
    fn 入力と出力が衝突する場合はエラー() {
        // .m4a 入力 → 音声出力パスと同一になり入力破壊のリスク
        assert!(plan_outputs(Path::new("/tmp/audio.m4a"), &test_args()).is_err());
        // 大文字 .M4A も同一ファイル扱いで検出
        assert!(plan_outputs(Path::new("/tmp/audio.M4A"), &test_args()).is_err());
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
        let bitrate = resolve_bitrate(Path::new("/nonexistent.mov"), Some("4M")).unwrap();
        assert_eq!(bitrate, 4_000_000);
    }

    #[test]
    fn ビットレート検出不能時は既定の2m() {
        let bitrate = resolve_bitrate(Path::new("/nonexistent.mov"), None).unwrap();
        assert_eq!(bitrate, 2_000_000);
    }

    #[test]
    fn 不正なビットレート指定はエラー() {
        assert!(resolve_bitrate(Path::new("/nonexistent.mov"), Some("abc")).is_err());
        assert!(resolve_bitrate(Path::new("/nonexistent.mov"), Some("-1M")).is_err());
    }

    #[test]
    fn ビットレート文字列の解釈() {
        assert_eq!(parse_bitrate("2M"), Some(2_000_000));
        assert_eq!(parse_bitrate("419k"), Some(419_000));
        assert_eq!(parse_bitrate("800000"), Some(800_000));
        assert_eq!(parse_bitrate("0.5M"), Some(500_000));
        assert_eq!(parse_bitrate("abc"), None);
        assert_eq!(parse_bitrate(""), None);
    }

    #[test]
    fn 上限内なら分割しない() {
        // 547kbps × 30分 ≈ 123MB < 200MB
        assert_eq!(plan_split(1800.0, 547_000, 200_000_000), None);
    }

    #[test]
    fn 上限超過なら分割する() {
        // 2.128Mbps × 55分 ≈ 877MB > 200MB → 各セグメントが安全係数込みで上限未満になる長さ
        let secs = plan_split(3300.0, 2_128_000, 200_000_000).expect("分割されるべき");
        let segment_bytes = 2_128_000.0 / 8.0 * secs as f64;
        assert!(segment_bytes < 200_000_000.0 * 0.9);
        assert!(secs >= 1);
    }

    #[test]
    fn 上限が小さくてもセグメントサイズ保証を優先() {
        // 868kbps で上限 3MB → セグメントは約23秒（60秒に切り上げない）
        let secs = plan_split(120.0, 868_000, 3_000_000).expect("分割されるべき");
        let segment_bytes = 868_000.0 / 8.0 * secs as f64;
        assert!(segment_bytes < 3_000_000.0);
    }

    #[test]
    fn 分割無効化はゼロ指定() {
        assert_eq!(plan_split(36000.0, 2_128_000, 0), None);
    }

    #[test]
    fn 分割ファイルの収集は接頭辞と拡張子で絞る() {
        let dir = tempfile::tempdir().expect("tempdir 作成失敗");
        for name in ["rec_part000.mp4", "rec_part001.mp4", "rec.mp4", "other.mp4", "rec_part000.txt"] {
            std::fs::write(dir.path().join(name), b"x").expect("書き込み失敗");
        }
        let parts = collect_parts(dir.path(), "rec_part").expect("収集失敗");
        let names: Vec<_> = parts
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["rec_part000.mp4", "rec_part001.mp4"]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macosはvideotoolboxを選択() {
        assert_eq!(
            ffmpeg::select_encoder(cli::Codec::H264).unwrap(),
            "h264_videotoolbox"
        );
        assert_eq!(
            ffmpeg::select_encoder(cli::Codec::Hevc).unwrap(),
            "hevc_videotoolbox"
        );
    }
}
