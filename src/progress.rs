use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::time::Duration;

const TEMPLATE_ERR: &str = "テンプレートは固定文字列のため必ず有効";

/// 経過時間付きスピナーを作成
pub fn spinner(mp: &MultiProgress, msg: String) -> ProgressBar {
    let pb = mp.add(ProgressBar::new_spinner());
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg} ({elapsed})").expect(TEMPLATE_ERR),
    );
    pb.set_message(msg);
    pb.enable_steady_tick(Duration::from_millis(120));
    pb
}

/// 成功で完了（経過時間を残す）
pub fn finish_ok(pb: &ProgressBar, msg: String) {
    pb.set_style(ProgressStyle::with_template("✅ {msg} ({elapsed})").expect(TEMPLATE_ERR));
    pb.finish_with_message(msg);
}

/// 失敗で完了
pub fn finish_err(pb: &ProgressBar, msg: String) {
    pb.set_style(ProgressStyle::with_template("❌ {msg}").expect(TEMPLATE_ERR));
    pb.finish_with_message(msg);
}

/// スキップで完了
pub fn finish_skip(pb: &ProgressBar, msg: String) {
    pb.set_style(ProgressStyle::with_template("⏭️  {msg}").expect(TEMPLATE_ERR));
    pb.finish_with_message(msg);
}
