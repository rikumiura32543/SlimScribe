# SlimScribe

macOS (Apple Silicon) / Windows 対応、OBS録画ファイル後処理自動化CUIツール。

指定フォルダ（デフォルト: macOS=`~/Movies`, Windows=`Videos`）内の最新の動画ファイル
（`.mov` `.mp4` `.mkv` `.flv` `.ts` — OBSの録画形式に対応）を自動検出し、以下を並列実行:

1. **軽量化** — ハードウェア加速で `.mp4` に変換（200MB超の見込みなら自動分割）
2. **音声抽出** — `.m4a` (AAC 128kbps) を生成
3. **文字起こし** — whisper.cpp で `.txt` を生成

出力は元動画と同じフォルダ・同じファイル名（拡張子違い）。

使用エンコーダは環境に応じて自動選択:

| OS | H.264 | HEVC |
|----|-------|------|
| macOS | `h264_videotoolbox` | `hevc_videotoolbox` |
| Windows/その他 | NVENC > QSV > AMF > `libx264` の順で自動検出 | NVENC > QSV > AMF > `libx265` |

## 必要環境

### macOS

```bash
brew install ffmpeg whisper-cpp

# whisper モデルのダウンロード（例: small）
mkdir -p ~/.cache/whisper
curl -L -o ~/.cache/whisper/ggml-small.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin
```

### Windows

1. [ffmpeg](https://ffmpeg.org/download.html) をインストールし PATH に追加（`winget install ffmpeg` 可）
2. [whisper.cpp のリリース](https://github.com/ggerganov/whisper.cpp/releases) から Windows バイナリを取得し、`whisper-cli.exe` を PATH に追加
3. モデルを `%USERPROFILE%\.cache\whisper\` に配置

```powershell
mkdir "$env:USERPROFILE\.cache\whisper"
curl.exe -L -o "$env:USERPROFILE\.cache\whisper\ggml-small.bin" `
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin
```

リポジトリ同梱の **`slimscribe.bat` をダブルクリック**すれば、ビルド済みバイナリの検出→なければ自動ビルド→実行まで行う。

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

これだけで `~/Movies`（Windowsは `Videos`）内の**最新の動画ファイル**を自動検出し、同じフォルダに以下を生成:

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
slimscribe --max-size-mb 500      # 分割の上限サイズを変更
slimscribe --max-size-mb 0        # 分割を無効化（常に単一 mp4）
slimscribe --ext mkv,mp4          # 自動検出の対象拡張子を限定
```

> **Note**: 入力が `.mp4` の場合、変換出力は `元ファイル名_slim.mp4` になる（入力の上書き防止）。

### mp4 の自動分割

変換後の mp4 が `--max-size-mb`（デフォルト **200MB**）を超える見込みの場合、
`元ファイル名_part000.mp4`, `_part001.mp4`, … と自動で複数ファイルに分割される。

- 各ファイルが上限未満に収まるようセグメント長を自動計算（安全係数 0.85）
- 分割境界にキーフレームを強制挿入するため、各ファイルは単体で頭から再生可能
- 分割不要なサイズなら従来どおり単一の `.mp4` を出力

> **Note**: 既に出力ファイルが存在する場合は誤上書き防止のためエラーになる。再処理するときは `--overwrite` を付ける。

## 主なオプション

| オプション | デフォルト | 説明 |
|-----------|-----------|------|
| `--dir, -d` | macOS: `~/Movies`<br>Win: `Videos` | 最新の動画を探すフォルダ |
| `--ext` | `mov,mp4,mkv,flv,ts` | 自動検出の対象拡張子（カンマ区切り） |
| `--file, -f` | — | 入力ファイル直接指定 |
| `--codec` | `h264` | `h264` / `hevc`（エンコーダは環境に応じて自動選択） |
| `--bitrate` | 自動 | 動画ビットレート（未指定時は 2M と元動画ビットレートの小さい方を自動選択） |
| `--max-size-mb` | `200` | mp4 の最大サイズ (MB)。超える見込みなら分割。`0` で無効 |
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
