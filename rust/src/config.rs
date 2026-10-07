//! 設定の読み書きと同梱データ。設定ファイルは Python 版と同じ %APPDATA%\lol-lane-priority\settings.json。
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 同梱データ（exe に埋め込む）
pub const BUNDLED_CHAMPIONS: &str = include_str!("../../data/champions_ja.json");
pub const BUNDLED_TRAITS: &str = include_str!("../../data/champion_traits.json");
pub const BUNDLED_ROLES: &str = include_str!("../../data/role_rates.json");

pub fn user_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("lol-lane-priority")
}

pub fn default_settings_path() -> PathBuf {
    user_dir().join("settings.json")
}

/// 既定の実績DB: exe と同じ場所の data\matchups.sqlite
pub fn default_db_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("data")
        .join("matchups.sqlite")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 判定に使うランク帯
    pub tier: String,
    /// 実績データを満額の重みで使うのに必要な試合数
    pub min_games: i64,
    /// 実績データとして使う直近パッチ数（0 なら全パッチ）
    pub recent_patches: i64,
    /// 空なら自動検出
    pub lockfile_path: String,
    pub db_path: String,
    pub poll_interval_ms: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            tier: "ALL".into(),
            min_games: 200,
            recent_patches: 2,
            lockfile_path: String::new(),
            db_path: default_db_path().to_string_lossy().into_owned(),
            poll_interval_ms: 500,
        }
    }
}

impl Settings {
    pub fn load(path: Option<&Path>) -> Self {
        let path = path.map(Path::to_path_buf).unwrap_or_else(default_settings_path);
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: Option<&Path>) -> std::io::Result<()> {
        let path = path.map(Path::to_path_buf).unwrap_or_else(default_settings_path);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).unwrap())
    }
}
