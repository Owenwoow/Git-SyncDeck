pub mod autosync;
pub mod commands;
pub mod config;
pub mod git;
pub mod model;
pub mod ops;
pub mod scan;
pub mod status;
pub mod system;
pub mod sysevents;
pub mod tray;
pub mod url;
pub mod util;

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_notification::NotificationExt;

use autosync::{Host, Switches};
use model::{AutoSyncEvent, AutoSyncTrigger, Project, SyncResult};

/// 开机自启时带的参数：带着它启动时窗口不显示，只进托盘
pub const AUTOSTART_ARG: &str = "--autostart";

/// 前端监听的事件
pub const EVENT_PROJECTS_CHANGED: &str = "projects-changed";
pub const EVENT_AUTO_SYNC_STARTED: &str = "auto-sync-started";
pub const EVENT_AUTO_SYNC_FINISHED: &str = "auto-sync-finished";

/// 后台同步的结果：发给前端，按设置发系统通知
struct AppHost {
    app: AppHandle,
}

impl AppHost {
    fn event(trigger: AutoSyncTrigger, result: Option<&SyncResult>) -> AutoSyncEvent {
        AutoSyncEvent { trigger, label: trigger.label().into(), result: result.cloned() }
    }
}

impl Host for AppHost {
    fn started(&self, trigger: AutoSyncTrigger) {
        let _ = self.app.emit(EVENT_AUTO_SYNC_STARTED, Self::event(trigger, None));
    }

    fn finished(&self, trigger: AutoSyncTrigger, result: Option<&SyncResult>) {
        let _ = self.app.emit(EVENT_AUTO_SYNC_FINISHED, Self::event(trigger, result));
        // 关机前同步完就关机了，通知没人看得到
        let Some(result) = result.filter(|_| trigger != AutoSyncTrigger::Shutdown) else { return };
        let st = self.app.state::<commands::Shared>();
        let switches = st.config.read(|c| Switches::from_stored(&c.settings));
        let names = st.snapshot().into_iter().map(|p| (p.id, p.name)).collect();
        if let Some((title, body)) = autosync::notification_text(trigger, result, &names, &switches) {
            if let Err(e) = self.app.notification().builder().title(title).body(body).show() {
                eprintln!("发送通知失败：{e}");
            }
        }
    }

    fn projects_changed(&self, projects: Vec<Project>) {
        let _ = self.app.emit(EVENT_PROJECTS_CHANGED, projects);
    }
}

/// 注册 / 取消开机自启（已经是想要的状态就什么都不做）
fn set_autostart(app: &AppHandle, on: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let launcher = app.autolaunch();
    if launcher.is_enabled().ok() == Some(on) {
        return Ok(());
    }
    let r = if on { launcher.enable() } else { launcher.disable() };
    r.map_err(|e| e.to_string())
}

fn autostart_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    let builder = tauri_plugin_autostart::Builder::new().arg(AUTOSTART_ARG);
    // 调试版用另一个名字登记：在设置里手动打开开关也不会顶掉安装版的开机自启项
    #[cfg(debug_assertions)]
    let builder = builder.app_name("Git SyncDeck (dev)");
    builder.build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    // 只允许运行一个实例：再次启动时把已有窗口显示到前面（开机自启的那次除外）。
    // 调试版不启用：已安装版在托盘里常驻时，`npm run tauri dev` 也能起来
    #[cfg(not(debug_assertions))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
        if !args.iter().any(|a| a == AUTOSTART_ARG) {
            tray::show_main_window(app);
        }
    }));
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(autostart_plugin())
        .setup(|app| {
            // 默认 %APPDATA%\com.gitsyncdeck.desktop\config.json；
            // 开发测试时可用环境变量 SYNCDECK_CONFIG_DIR 指到别处，避免影响真实配置
            let dir = match std::env::var_os("SYNCDECK_CONFIG_DIR") {
                Some(d) => std::path::PathBuf::from(d),
                None => app.path().app_config_dir()?,
            };
            std::fs::create_dir_all(&dir)?;
            let store = config::ConfigStore::load(dir.join("config.json"));
            let st: commands::Shared = Arc::new(commands::AppState::new(store));
            app.manage::<commands::Shared>(st.clone());

            let handle = app.handle().clone();
            let host: Arc<dyn Host> = Arc::new(AppHost { app: handle.clone() });

            // 托盘：项目状态、"正在同步"、暂停状态一变就更新
            tray::create(&handle, st.clone(), host.clone())?;
            st.set_change_hook({
                let handle = handle.clone();
                move || tray::request_update(&handle)
            });

            // 开机自启：发布版按设置注册 / 取消；调试版不自动注册，只在设置里手动切换开关时才动
            st.set_autostart_hook({
                let handle = handle.clone();
                move |on| set_autostart(&handle, on)
            });
            #[cfg(not(debug_assertions))]
            {
                let on = st.config.read(|c| Switches::from_stored(&c.settings).launch_at_login);
                if let Err(e) = set_autostart(&handle, on) {
                    eprintln!("同步开机自启设置失败：{e}");
                }
            }

            // 开机自启时只进托盘，否则显示窗口（tauri.conf.json 里窗口默认不显示）
            if !std::env::args().any(|a| a == AUTOSTART_ARG) {
                tray::show_main_window(&handle);
            }

            // 系统事件：锁屏 / 睡眠 / 关机 → 离开；解锁 / 唤醒 → 到达
            let handlers = sysevents::Handlers {
                on_event: Box::new({
                    let (st, host) = (st.clone(), host.clone());
                    move |e| {
                        let trigger = match e {
                            sysevents::SysEvent::Lock => AutoSyncTrigger::Lock,
                            sysevents::SysEvent::Unlock => AutoSyncTrigger::Unlock,
                            sysevents::SysEvent::Suspend => AutoSyncTrigger::Sleep,
                            sysevents::SysEvent::Resume => AutoSyncTrigger::Resume,
                        };
                        autosync::spawn(st.clone(), host.clone(), trigger);
                    }
                }),
                on_end_session: Box::new({
                    let (st, host) = (st.clone(), host.clone());
                    move || autosync::shutdown_blocking(st.clone(), host.clone())
                }),
            };
            if let Err(e) = sysevents::start(handlers) {
                eprintln!("系统事件监听启动失败（离开 / 到达时不会自动同步）：{e}");
            }

            // 程序启动本身算一次"到达"；之后定时检查云端（设置里打开才做）
            autosync::on_startup(st.clone(), host.clone());
            tauri::async_runtime::spawn(autosync::periodic_loop(st, host));
            Ok(())
        })
        .on_window_event(|window, event| {
            // 关窗口时隐藏到托盘（设置里可关）
            if let WindowEvent::CloseRequested { api, .. } = event {
                let st = window.state::<commands::Shared>();
                if st.config.read(|c| Switches::from_stored(&c.settings).close_to_tray) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::check_git,
            commands::list_projects,
            commands::refresh_status,
            commands::add_projects,
            commands::remove_project,
            commands::set_auto_commit,
            commands::scan_directory,
            commands::sync_all,
            commands::commit_and_push,
            commands::get_diagnostic_info,
            commands::open_in_terminal,
            commands::open_in_editor,
            commands::get_settings,
            commands::save_settings,
            commands::pause_auto_sync,
            commands::resume_auto_sync,
            commands::get_auto_sync_state,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
