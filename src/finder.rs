use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

/// dir 直下（サブフォルダは走査しない）から最終更新が最も新しい .mov を返す
pub fn find_latest_mov(dir: &Path) -> Result<PathBuf> {
    if !dir.is_dir() {
        bail!("フォルダが存在しません: {}", dir.display());
    }

    let latest = WalkDir::new(dir)
        .max_depth(1)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("mov"))
                .unwrap_or(false)
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.into_path()))
        })
        .max_by_key(|(modified, _)| *modified);

    match latest {
        Some((_, path)) => Ok(path),
        None => bail!(".mov ファイルが見つかりません: {}", dir.display()),
    }
}

/// SystemTime を経過表示用に変換（何分前に録画されたか）
pub fn age_label(modified: SystemTime) -> String {
    match SystemTime::now().duration_since(modified) {
        Ok(age) => {
            let mins = age.as_secs() / 60;
            if mins < 60 {
                format!("{mins}分前に更新")
            } else if mins < 60 * 24 {
                format!("{}時間{}分前に更新", mins / 60, mins % 60)
            } else {
                format!("{}日前に更新", mins / (60 * 24))
            }
        }
        Err(_) => "更新時刻不明".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::time::Duration;

    /// 指定した mtime のファイルを作成
    fn create_file_with_mtime(dir: &Path, name: &str, age: Duration) {
        let path = dir.join(name);
        let file = File::create(&path).expect("テストファイル作成失敗");
        let mtime = SystemTime::now() - age;
        file.set_times(fs::FileTimes::new().set_modified(mtime))
            .expect("mtime 設定失敗");
    }

    #[test]
    fn 空フォルダはエラー() {
        let dir = tempfile::tempdir().expect("tempdir 作成失敗");
        assert!(find_latest_mov(dir.path()).is_err());
    }

    #[test]
    fn 存在しないフォルダはエラー() {
        assert!(find_latest_mov(Path::new("/nonexistent/dir")).is_err());
    }

    #[test]
    fn 最新のmovを選択() {
        let dir = tempfile::tempdir().expect("tempdir 作成失敗");
        create_file_with_mtime(dir.path(), "old.mov", Duration::from_secs(3600));
        create_file_with_mtime(dir.path(), "newest.mov", Duration::from_secs(60));
        create_file_with_mtime(dir.path(), "middle.mov", Duration::from_secs(1800));

        let found = find_latest_mov(dir.path()).expect("検出失敗");
        assert_eq!(found.file_name().unwrap(), "newest.mov");
    }

    #[test]
    fn 拡張子は大文字小文字を区別しない() {
        let dir = tempfile::tempdir().expect("tempdir 作成失敗");
        create_file_with_mtime(dir.path(), "video.MOV", Duration::from_secs(60));

        let found = find_latest_mov(dir.path()).expect("検出失敗");
        assert_eq!(found.file_name().unwrap(), "video.MOV");
    }

    #[test]
    fn mov以外は無視() {
        let dir = tempfile::tempdir().expect("tempdir 作成失敗");
        create_file_with_mtime(dir.path(), "newer.mp4", Duration::from_secs(10));
        create_file_with_mtime(dir.path(), "target.mov", Duration::from_secs(3600));

        let found = find_latest_mov(dir.path()).expect("検出失敗");
        assert_eq!(found.file_name().unwrap(), "target.mov");
    }

    #[test]
    fn サブフォルダは走査しない() {
        let dir = tempfile::tempdir().expect("tempdir 作成失敗");
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).expect("サブフォルダ作成失敗");
        create_file_with_mtime(&sub, "inner.mov", Duration::from_secs(10));
        create_file_with_mtime(dir.path(), "top.mov", Duration::from_secs(3600));

        let found = find_latest_mov(dir.path()).expect("検出失敗");
        assert_eq!(found.file_name().unwrap(), "top.mov");
    }

    #[test]
    fn 経過時間の表示単位() {
        let now = SystemTime::now();
        assert!(age_label(now - Duration::from_secs(5 * 60)).starts_with("5分前"));
        assert!(age_label(now - Duration::from_secs(90 * 60)).starts_with("1時間30分前"));
        assert!(age_label(now - Duration::from_secs(3 * 24 * 3600)).starts_with("3日前"));
        // 未来の時刻（時計ずれ）でも panic しない
        assert_eq!(age_label(now + Duration::from_secs(3600)), "更新時刻不明");
    }
}
