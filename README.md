# SlimScribe

macOS (Apple Silicon) 向け、OBS録画ファイル後処理自動化CUIツール。

指定フォルダ（デフォルト: `~/Movies`）内の最新 `.mov` を自動検出し、以下を並列実行:

1. **軽量化** — VideoToolbox ハードウェア加速で `.mp4` に変換
2. **音声抽出** — `.m4a` (AAC 128kbps) を生成
3. **文字起こし** — whisper.cpp で `.txt` を生成

出力は元動画と同じフォルダ・同じファイル名（拡張子違い）。

## 必要環境

```bash
brew install ffmpeg whisper-cpp

# whisper モデルのダウンロード（例: small）
mkdir -p ~/.cache/whisper
curl -L -o ~/.cache/whisper/ggml-small.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin
```

モデルは `~/.cache/whisper/` に置くと自動検出される（large-v3-turbo > large-v3 > medium > small > base > tiny の優先順）。

## インストール

```bash
cargo install --path .
```

`~/.cargo/bin/slimscribe` に配置され、どこからでも `slimscribe` で実行可能。
（インストールしない場合は `cargo build --release` 後に `./target/release/slimscribe` で実行）

## 通常利用

OBSで録画を停止した後、ターミナルで実行するだけ:

```bash
slimscribe
```

これだけで `~/Movies` 内の**最新の .mov** を自動検出し、同じフォルダに以下を生成:

```
~/Movies/
├── 会議_20260708.mov   # 元ファイル（そのまま残る）
├── 会議_20260708.mp4   # 軽量化された動画
├── 会議_20260708.m4a   # 音声のみ
└── 会議_20260708.txt   # 文字起こし
```

処理中はステータスと経過時間が表示される:

```
🎬 対象: /Users/xxx/Movies/会議_20260708.mov (243 MB, 5分前に更新)
✅ 動画変換 完了: /Users/xxx/Movies/会議_20260708.mp4 (8m 17s)
✅ 音声抽出 完了: /Users/xxx/Movies/会議_20260708.m4a (52s)
✅ 文字起こし 完了: /Users/xxx/Movies/会議_20260708.txt (3m 05s)

📦 出力先:
  /Users/xxx/Movies/会議_20260708.mp4 (229.4 MB)
  /Users/xxx/Movies/会議_20260708.m4a (50.7 MB)
  /Users/xxx/Movies/会議_20260708.txt (0.0 MB)
⏱️  合計時間: 497.1 秒
```

### よくある使い方

```bash
slimscribe                        # ~/Movies の最新 .mov を処理（基本形）
slimscribe --overwrite            # 前回の出力を上書きして再処理
slimscribe --file ./meeting.mov   # 特定のファイルを処理
slimscribe --dir ~/Desktop/rec    # 別フォルダの最新 .mov を処理
slimscribe --no-transcribe        # 急ぎで動画・音声だけ欲しいとき
```

### その他のオプション例

```bash
slimscribe --codec hevc           # HEVC (H.265) で変換（より高圧縮）
slimscribe --bitrate 4M           # ビットレートを固定指定
slimscribe --language en          # 英語の録画を文字起こし
slimscribe --no-convert           # mp4 変換をスキップ（音声・文字起こしのみ）
```

> **Note**: 既に出力ファイルが存在する場合は誤上書き防止のためエラーになる。再処理するときは `--overwrite` を付ける。

## 主なオプション

| オプション | デフォルト | 説明 |
|-----------|-----------|------|
| `--dir, -d` | `~/Movies` | 最新 .mov を探すフォルダ |
| `--file, -f` | — | 入力ファイル直接指定 |
| `--codec` | `h264` | `h264` / `hevc` (VideoToolbox) |
| `--bitrate` | 自動 | 動画ビットレート（未指定時は 2M と元動画ビットレートの小さい方を自動選択） |
| `--language, -l` | `ja` | 文字起こし言語 |
| `--whisper-model` | 自動検出 | モデル .bin のパス（環境変数 `SLIMSCRIBE_WHISPER_MODEL` 可） |
| `--whisper-bin` | `whisper-cli` | whisper CLI のパス（環境変数 `SLIMSCRIBE_WHISPER_BIN` 可） |
| `--no-transcribe` | — | 文字起こしをスキップ |
| `--no-convert` | — | mp4 変換をスキップ |
| `--overwrite` | — | 既存の出力ファイルを上書き |

## 設計方針

- ffmpeg / whisper-cli を `std::process::Command` で呼び出すラッパー型設計（C バインディング不使用）
- mp4 変換と「音声抽出 → 文字起こし」を別スレッドで並列実行
- 録画・文字起こし結果は `.gitignore` で除外済み（メディア・txt・モデルはコミットされない）
