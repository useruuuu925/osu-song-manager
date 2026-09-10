use crate::config::{config_path, AppConfig};
use crate::detect;
use crate::library;
use crate::model::{BeatmapSetInfo, LibraryCandidate, SourceKind};
use tauri::{AppHandle, Emitter, Manager};

fn cfg_file(app: &AppHandle) -> std::path::PathBuf {
    let dir = app
        .path()
        .app_data_dir()
        .ok()
        .or_else(|| app.path().app_config_dir().ok());
    config_path(dir.as_deref())
}

#[tauri::command]
pub fn detect_libraries() -> Vec<LibraryCandidate> {
    detect::detect_libraries()
}

#[tauri::command]
pub fn validate_library(kind: SourceKind, path: String) -> Result<(), String> {
    detect::validate_library(kind, &path)
}

#[tauri::command]
pub fn get_config(app: AppHandle) -> AppConfig {
    AppConfig::load(&cfg_file(&app))
}

#[tauri::command]
pub fn set_config(app: AppHandle, config: AppConfig) -> Result<(), String> {
    config.save(&cfg_file(&app)).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn scan_library(
    app: AppHandle,
    kind: SourceKind,
    path: String,
) -> Result<Vec<BeatmapSetInfo>, String> {
    let app_for_progress = app.clone();
    let progress = move |done: u32, total: u32| {
        let _ = app_for_progress.emit("scan-progress", (done, total));
    };

    // 扫描可能是 IO 密集型的长任务，放到独立线程避免冻结 UI
    let result =
        tauri::async_runtime::spawn_blocking(move || library::scan_library(kind, &path, &progress))
            .await
            .map_err(|e| crate::errcode::ec1(crate::errcode::SCAN_INTERRUPTED, e))?;

    let result = result?;
    let _ = app.emit("scan-progress", (1u32, 1u32));
    Ok(result)
}
