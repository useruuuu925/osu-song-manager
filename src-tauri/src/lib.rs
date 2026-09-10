mod commands;
mod config;
mod convert;
mod detect;
mod downloader;
mod errcode;
mod export_csv;
mod library;
mod manage;
mod model;
mod online;
mod osu_parser;
mod realm_db;
mod thumbs;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // M5：自定义 thumb 协议，GET http://thumb.localhost/<key>（或 thumb://<key>）
        // 仅服务 app_data_dir/thumbs 下、通过严格键校验的缓存文件；未知键一律 404。
        .register_uri_scheme_protocol("thumb", |ctx, request| {
            let app = ctx.app_handle().clone();
            thumbs::serve(&app, &request.uri().to_string())
        })
        .invoke_handler(tauri::generate_handler![
            commands::detect_libraries,
            commands::validate_library,
            commands::get_config,
            commands::set_config,
            commands::scan_library,
            online::online_fetch,
            online::online_cache,
            online::online_status,
            online::set_oauth_credentials,
            online::osu_login_begin,
            online::osu_login_status,
            online::osu_login_manual,
            online::osu_logout,
            online::get_oauth_config,
            thumbs::prepare_thumbnails,
            thumbs::fetch_online_background,
            thumbs::pick_export_folder,
            thumbs::export_backgrounds,
            manage::detect_duplicates,
            manage::delete_to_trash,
            manage::move_sets,
            manage::pack_archives,
            manage::scan_empty_folders,
            downloader::download_osz,
            downloader::cancel_downloads,
            export_csv::export_library_csv,
            convert::convert_lazer_to_stable,
            convert::cancel_convert,
        ])
        .setup(|app| {
            // 启动清扫：删除旧键形态缩略图缓存与 .tmpwrite 残留（后台，不阻塞启动）
            let handle = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                thumbs::prune_stale_thumbs(&thumbs::cache_dir(&handle));
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
mod slider_path;
