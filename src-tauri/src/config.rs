use crate::model::SourceKind;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// 应用配置（JSON 持久化）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub mode: Option<SourceKind>,
    pub lazer_dir: Option<String>,
    pub stable_songs_dir: Option<String>,
    pub osz_dir: Option<String>,
    pub last_export_dir: Option<String>,
    pub last_download_dir: Option<String>,
}

impl AppConfig {
    pub fn load(path: &Path) -> AppConfig {
        match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => AppConfig::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)
    }

    /// 仅测试使用（生产路径检测走 detect::detect_libraries）
    #[cfg(test)]
    pub fn active_path(&self) -> Option<(SourceKind, String)> {
        match self.mode? {
            SourceKind::Lazer => self.lazer_dir.clone().map(|p| (SourceKind::Lazer, p)),
            SourceKind::Stable => self
                .stable_songs_dir
                .clone()
                .map(|p| (SourceKind::Stable, p)),
            SourceKind::OszFolder => self.osz_dir.clone().map(|p| (SourceKind::OszFolder, p)),
        }
    }
}

/// 解析配置文件路径：优先使用 Tauri appdata 目录，失败时退回 %APPDATA%
pub fn config_path(app_data: Option<&Path>) -> PathBuf {
    if let Some(dir) = app_data {
        return dir.join("config.json");
    }
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(appdata)
        .join("osu-song-manager")
        .join("config.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let cfg = AppConfig {
            mode: Some(SourceKind::Lazer),
            lazer_dir: Some("C:/somewhere/osu!".into()),
            ..AppConfig::default()
        };
        let dir = std::env::temp_dir().join("osm-test-cfg");
        let p = dir.join("config.json");
        cfg.save(&p).unwrap();
        let loaded = AppConfig::load(&p);
        assert_eq!(loaded.mode, Some(SourceKind::Lazer));
        assert_eq!(loaded.lazer_dir.as_deref(), Some("C:/somewhere/osu!"));
        assert_eq!(
            loaded.active_path(),
            Some((SourceKind::Lazer, "C:/somewhere/osu!".to_string()))
        );
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn corrupt_file_falls_back_to_default() {
        let dir = std::env::temp_dir().join("osm-test-cfg2");
        let p = dir.join("config.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&p, "{ not json !!").unwrap();
        let loaded = AppConfig::load(&p);
        assert!(loaded.mode.is_none());
    }
}
