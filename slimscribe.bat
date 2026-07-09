@echo off
setlocal
chcp 65001 >nul

REM =============================================================
REM SlimScribe 実行用バッチ (Windows)
REM ダブルクリックで Videos フォルダの最新 .mov を処理します。
REM 引数はそのまま slimscribe に渡されます (例: slimscribe.bat --overwrite)
REM 必要環境: ffmpeg / whisper-cli が PATH に存在すること
REM =============================================================

REM 1) インストール済みの slimscribe (cargo install) を優先
where slimscribe >nul 2>nul
if %errorlevel%==0 (
    slimscribe %*
    goto :done
)

REM 2) リポジトリ内のリリースビルドがあれば使用
if exist "%~dp0target\release\slimscribe.exe" (
    "%~dp0target\release\slimscribe.exe" %*
    goto :done
)

REM 3) どちらもなければビルドしてから実行
echo slimscribe が見つからないため、ビルドします... (初回のみ数分かかります)
where cargo >nul 2>nul
if not %errorlevel%==0 (
    echo [エラー] Rust ^(cargo^) がインストールされていません。
    echo https://rustup.rs/ からインストールしてください。
    goto :done
)
pushd "%~dp0"
cargo build --release
if not %errorlevel%==0 (
    echo [エラー] ビルドに失敗しました。
    popd
    goto :done
)
popd
"%~dp0target\release\slimscribe.exe" %*

:done
echo.
pause
endlocal
