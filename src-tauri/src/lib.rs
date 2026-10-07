pub mod commands;
pub mod config;
pub mod git;
pub mod model;
pub mod ops;
pub mod scan;
pub mod status;
pub mod system;
pub mod url;
pub mod util;

use std::sync::Arc;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // 默认 %APPDATA%\com.gitsyncdeck.desktop\config.json；
            // 开发测试时可用环境变量 SYNCDECK_CONFIG_DIR 指到别处，避免影响真实配置
            let dir = match std::env::var_os("SYNCDECK_CONFIG_DIR") {
                Some(d) => std::path::PathBuf::from(d),
                None => app.path().app_config_dir()?,
            };
            std::fs::create_dir_all(&dir)?;
            let store = config::ConfigStore::load(dir.join("config.json"));
            app.manage::<commands::Shared>(Arc::new(commands::AppState::new(store)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::check_git,
            commands::list_projects,
            commands::refresh_status,
            commands::add_projects,
            commands::remove_project,
            commands::scan_directory,
            commands::sync_all,
            commands::commit_and_push,
            commands::get_diagnostic_info,
            commands::open_in_terminal,
            commands::open_in_editor,
            commands::get_settings,
            commands::save_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
