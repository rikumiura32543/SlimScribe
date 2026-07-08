use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// モデル自動検出の対象ディレクトリ
fn model_search_dirs() -> Vec<PathBuf> {
    let mut dirs_list = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs_list.push(home.join(".cache/whisper"));
        dirs_list.push(home.join("Models/whisper"));
    }
    dirs_list.push(PathBuf::from("/opt/homebrew/share/whisper-cpp"));
    dirs_list
}

/// 精度の高い順に探すモデルファイル名
const PREFERRED_MODELS: [&str; 6] = [
    "ggml-large-v3-turbo.bin",
    "ggml-large-v3.bin",
    "ggml-medium.bin",
    "ggml-small.bin",
    "ggml-base.bin",
    "ggml-tiny.bin",
];

/// モデルパスを解決（明示指定 > 標準配置場所から精度の高い順に自動検出）
pub fn resolve_model(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Ok(path.to_path_buf());
        }
        bail!("指定されたモデルファイルが見つかりません: {}", path.display());
    }

    let search_dirs = model_search_dirs();
    for name in PREFERRED_MODELS {
        for dir in &search_dirs {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    bail!(
        "whisper モデル (.bin) が見つかりません。\n\
         --whisper-model または環境変数 SLIMSCRIBE_WHISPER_MODEL で指定してください。\n\
         ダウンロード例:\n  \
         mkdir -p ~/.cache/whisper && curl -L -o ~/.cache/whisper/ggml-small.bin \\\n    \
         https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
    )
}

/// whisper CLI が実行可能か確認
pub fn check_whisper(bin: &str) -> bool {
    Command::new(bin)
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// 16kHz WAV を文字起こしし、<output_base>.txt を生成
pub fn transcribe(
    bin: &str,
    model: &Path,
    wav: &Path,
    output_base: &Path,
    language: &str,
) -> Result<()> {
    let output = Command::new(bin)
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(wav)
        .args(["-l", language])
        .arg("-otxt")
        .arg("-of")
        .arg(output_base)
        .output()
        .with_context(|| format!("{bin} の起動に失敗しました"))?;

    if !output.status.success() {
        bail!(
            "文字起こしが異常終了しました:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}
