use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "slimscribe",
    version,
    about = "OBS録画(.mov)の軽量化・音声抽出・文字起こしを自動化するCUIツール"
)]
pub struct Args {
    /// 対象フォルダ（この直下から最新の .mov を自動検出。デフォルト: ~/Movies）
    #[arg(short, long)]
    pub dir: Option<PathBuf>,

    /// 入力ファイルを直接指定（指定時は自動検出をスキップ）
    #[arg(short, long, conflicts_with = "dir")]
    pub file: Option<PathBuf>,

    /// 動画コーデック（VideoToolbox ハードウェア加速）
    #[arg(long, value_enum, default_value_t = Codec::H264)]
    pub codec: Codec,

    /// 動画ビットレート（未指定時: 2M と元動画の映像ビットレートの小さい方を自動選択）
    #[arg(long)]
    pub bitrate: Option<String>,

    /// 文字起こし言語
    #[arg(short, long, default_value = "ja")]
    pub language: String,

    /// whisper.cpp モデルファイル (.bin) のパス
    #[arg(long, env = "SLIMSCRIBE_WHISPER_MODEL")]
    pub whisper_model: Option<PathBuf>,

    /// whisper CLI の実行ファイル名またはパス
    #[arg(long, default_value = "whisper-cli", env = "SLIMSCRIBE_WHISPER_BIN")]
    pub whisper_bin: String,

    /// 文字起こしをスキップ
    #[arg(long)]
    pub no_transcribe: bool,

    /// mp4 変換をスキップ
    #[arg(long)]
    pub no_convert: bool,

    /// 既存の出力ファイルを上書き
    #[arg(long)]
    pub overwrite: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum Codec {
    H264,
    Hevc,
}

impl Codec {
    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            Codec::H264 => "h264_videotoolbox",
            Codec::Hevc => "hevc_videotoolbox",
        }
    }
}
