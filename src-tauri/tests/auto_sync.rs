//! 换电脑自动同步：离开时只同步有东西要推的项目（没有就完全不联网）、到达时同步所有项目、
//! 10 分钟内同一种只做一次、暂停与托盘"立即同步"、设置的读写和旧配置兼容。
//! 只使用 scripts/make-test-repos.mjs 生成的临时仓库和临时配置目录。
//! 运行：cd src-tauri && cargo test --test auto_sync -- --nocapture

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use chrono::Local;
use tauri::Manager;

use git_syncdeck_lib::autosync::{self, Decision, Host, Outcome};
use git_syncdeck_lib::commands::{self, AppState, Shared};
use git_syncdeck_lib::config::ConfigStore;
use git_syncdeck_lib::model::{AutoSyncTrigger, Project, SettingsPatch, SyncOutcome, SyncResult};
use git_syncdeck_lib::util;

fn fixtures(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(name);
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/make-test-repos.mjs");
    assert!(Command::new("node").arg(script).arg(&root).status().unwrap().success());
    root
}

fn fresh_config_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir); // 测试自己的临时配置目录
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 测试里直接读临时仓库用
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).output().expect("git");
    assert!(out.status.success(), "git {args:?} 失败：{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn new_app(cfg_file: &Path) -> tauri::App<tauri::test::MockRuntime> {
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    app.manage::<Shared>(Arc::new(AppState::new(ConfigStore::load(cfg_file.to_path_buf()))));
    app
}

/// 记下后台同步通知了什么
#[derive(Default)]
struct Recorder(Mutex<Vec<String>>);

impl Recorder {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

impl Host for Recorder {
    fn started(&self, t: AutoSyncTrigger) {
        self.0.lock().unwrap().push(format!("started {t:?}"));
    }
    fn finished(&self, t: AutoSyncTrigger, r: Option<&SyncResult>) {
        let items = r.map_or("无".to_string(), |r| r.items.len().to_string());
        self.0.lock().unwrap().push(format!("finished {t:?} {items}"));
    }
    fn projects_changed(&self, p: Vec<Project>) {
        self.0.lock().unwrap().push(format!("projects {}", p.len()));
    }
}

fn synced(outcome: Outcome) -> SyncResult {
    match outcome {
        Outcome::Synced(r) => r,
        other => panic!("应该同步了，实际：{other:?}"),
    }
}

fn outcomes(r: &SyncResult, name_of: &HashMap<String, String>) -> HashMap<String, SyncOutcome> {
    r.items.iter().map(|i| (name_of[&i.project_id].clone(), i.outcome)).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn departure_only_pushes_what_needs_pushing() {
    let root = fixtures("syncdeck-autosync-departure");
    let repos = root.join("repos");
    let app = new_app(&fresh_config_dir("syncdeck-autosync-departure-config").join("config.json"));
    let st: Shared = app.state::<Shared>().inner().clone();

    let names = ["synced", "ahead", "behind", "dirty", "no-remote"];
    let added = commands::add_projects(app.state(), names.iter().map(|n| repos.join(n).to_string_lossy().into()).collect())
        .await
        .unwrap();
    let name_of: HashMap<String, String> = added.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
    let id_of = |n: &str| added.iter().find(|p| p.name == n).unwrap().id.clone();
    commands::set_auto_commit(app.state(), id_of("dirty"), true).await.unwrap();

    let behind = repos.join("behind");
    let behind_head = git(&behind, &["rev-parse", "HEAD"]);
    let behind_tracking = git(&behind, &["rev-parse", "origin/main"]);
    let remote_of = |n: &str| root.join("remotes").join(format!("{n}.git"));
    let ahead_local = git(&repos.join("ahead"), &["rev-parse", "HEAD"]);

    // 锁屏 = 离开：只同步待推送的和"有改动且开了自动提交"的
    let host = Recorder::default();
    let r = synced(autosync::handle(&st, &host, AutoSyncTrigger::Lock).await);
    let got = outcomes(&r, &name_of);
    println!("锁屏时同步：{got:?}");
    assert_eq!(got.len(), 2, "只挑 ahead 和 dirty：{got:?}");
    assert_eq!(got["ahead"], SyncOutcome::Pushed);
    assert_eq!(got["dirty"], SyncOutcome::Committed);
    assert_eq!(git(&remote_of("ahead"), &["rev-parse", "main"]), ahead_local, "已推送到云端");
    // 没挑中的项目完全没碰：没拉取，连 fetch 都没做（远程跟踪分支没变）
    assert_eq!(git(&behind, &["rev-parse", "HEAD"]), behind_head);
    assert_eq!(git(&behind, &["rev-parse", "origin/main"]), behind_tracking);
    assert_eq!(host.take(), ["started Lock", "projects 5", "finished Lock 2"]);

    // 10 分钟内再次锁屏：不做
    assert!(matches!(
        autosync::handle(&st, &host, AutoSyncTrigger::Lock).await,
        Outcome::Skipped(Decision::TooSoon)
    ));
    assert!(host.take().is_empty());

    // 关掉"锁屏时同步"：不做
    commands::save_settings(app.state(), SettingsPatch { sync_on_lock: Some(false), ..Default::default() })
        .await
        .unwrap();
    let outcome = autosync::handle(&st, &host, AutoSyncTrigger::Lock).await;
    assert!(matches!(outcome, Outcome::Skipped(Decision::Disabled)), "{outcome:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn arrival_syncs_everything_and_respects_pause() {
    let root = fixtures("syncdeck-autosync-arrival");
    let repos = root.join("repos");
    let app = new_app(&fresh_config_dir("syncdeck-autosync-arrival-config").join("config.json"));
    let st: Shared = app.state::<Shared>().inner().clone();

    let added = commands::add_projects(
        app.state(),
        ["synced", "behind", "ahead"].iter().map(|n| repos.join(n).to_string_lossy().into()).collect(),
    )
    .await
    .unwrap();
    let name_of: HashMap<String, String> = added.iter().map(|p| (p.id.clone(), p.name.clone())).collect();

    // 暂停到明天：解锁不做；托盘"立即同步"是手动的，照做
    let s = commands::pause_auto_sync(app.state()).await.unwrap();
    let until = s.auto_sync_paused_until.clone().expect("暂停中");
    assert!(until.contains("T00:00:00"), "暂停到次日 00:00：{until}");
    assert!(commands::get_auto_sync_state(app.state()).running.is_none());
    let host = Recorder::default();
    assert!(matches!(
        autosync::handle(&st, &host, AutoSyncTrigger::Unlock).await,
        Outcome::Skipped(Decision::Paused)
    ));
    let r = synced(autosync::handle(&st, &host, AutoSyncTrigger::Manual).await);
    let got = outcomes(&r, &name_of);
    println!("立即同步：{got:?}");
    assert_eq!(got.len(), 3, "到达类同步所有项目");
    assert_eq!(got["behind"], SyncOutcome::Pulled);
    assert_eq!(got["ahead"], SyncOutcome::Pushed);
    assert_eq!(got["synced"], SyncOutcome::UpToDate);
    assert_eq!(host.take(), ["started Manual", "projects 3", "finished Manual 3"]);

    // 恢复后：刚做过"立即同步"（同属到达），10 分钟内的解锁不再做
    let s = commands::resume_auto_sync(app.state()).await.unwrap();
    assert!(s.auto_sync_paused_until.is_none());
    assert!(matches!(
        autosync::handle(&st, &host, AutoSyncTrigger::Unlock).await,
        Outcome::Skipped(Decision::TooSoon)
    ));
    // 但离开另算
    assert!(matches!(autosync::handle(&st, &host, AutoSyncTrigger::Lock).await, Outcome::NothingToDo));

    // 新开一个进程（内存里没有上次同步的时间）：解锁照常做
    drop(app);
    let app = new_app(&fresh_config_dir("syncdeck-autosync-arrival-config").join("config.json"));
    commands::add_projects(app.state(), vec![repos.join("synced").to_string_lossy().into()]).await.unwrap();
    let st: Shared = app.state::<Shared>().inner().clone();
    let r = synced(autosync::handle(&st, &host, AutoSyncTrigger::Unlock).await);
    assert_eq!(r.items.len(), 1);
    assert_eq!(r.items[0].outcome, SyncOutcome::UpToDate);
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_sync_settings_and_old_config() {
    let dir = fresh_config_dir("syncdeck-autosync-settings-config");
    let cfg_file = dir.join("config.json");
    // 加入自动同步之前的配置文件（syncOnShutdown / syncOnSleep 是已经去掉的开关，读的时候忽略）
    std::fs::write(
        &cfg_file,
        r#"{ "version": 1, "settings": { "theme": "dark", "commitTemplate": "同步 {host}", "syncOnShutdown": false, "syncOnSleep": false }, "projects": [] }"#,
    )
    .unwrap();
    let app = new_app(&cfg_file);

    // 旧配置：新开关都是默认值
    let s = commands::get_settings(app.state()).await.unwrap();
    assert_eq!(s.theme.as_deref(), Some("dark"));
    assert!(s.launch_at_login && s.close_to_tray);
    assert!(s.sync_on_lock);
    assert!(s.sync_on_startup && s.sync_on_resume && s.sync_on_unlock);
    assert!(!s.periodic_check && !s.notify_on_success && s.notify_on_error);
    assert_eq!(s.host_name, "");
    assert_eq!(s.default_host_name, util::system_computer_name());
    assert!(s.auto_sync_paused_until.is_none());
    let state = commands::get_auto_sync_state(app.state());
    assert!(state.running.is_none() && !state.startup_sync);

    // 改开关：只保存和默认值不同的
    let s = commands::save_settings(
        app.state(),
        SettingsPatch { sync_on_lock: Some(false), periodic_check: Some(true), notify_on_error: Some(true), ..Default::default() },
    )
    .await
    .unwrap();
    assert!(!s.sync_on_lock && s.periodic_check && s.notify_on_error && s.sync_on_unlock);
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg_file).unwrap()).unwrap();
    assert_eq!(saved["settings"]["syncOnLock"], false);
    assert_eq!(saved["settings"]["periodicCheck"], true);
    assert!(saved["settings"]["notifyOnError"].is_null(), "等于默认值的不单独保存");
    assert_eq!(saved["settings"]["theme"], "dark", "旧字段原样保留");

    // 本机名称：提交信息的 {host} 用它；清空后回到电脑名；不能有换行
    let s = commands::save_settings(app.state(), SettingsPatch { host_name: Some(" 学校笔记本 ".into()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(s.host_name, "学校笔记本");
    assert_eq!(util::computer_name(), "学校笔记本");
    assert_eq!(util::commit_message_now(&s.commit_template), "同步 学校笔记本");
    assert!(commands::save_settings(app.state(), SettingsPatch { host_name: Some("a\nb".into()), ..Default::default() })
        .await
        .is_err());
    assert_eq!(commands::get_settings(app.state()).await.unwrap().host_name, "学校笔记本", "保存失败时不改");

    // 重新打开：本机名称从配置里读回来并生效
    drop(app);
    util::set_host_name(None);
    let app = new_app(&cfg_file);
    assert_eq!(util::computer_name(), "学校笔记本");
    let s = commands::save_settings(app.state(), SettingsPatch { host_name: Some("".into()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(s.host_name, "");
    assert_eq!(util::computer_name(), util::system_computer_name());

    // 暂停：保存在配置里，过期的暂停读出来是"没暂停"
    let st: Shared = app.state::<Shared>().inner().clone();
    commands::set_auto_sync_pause(&st, Some(Local::now() - chrono::Duration::minutes(1))).unwrap();
    assert!(commands::get_settings(app.state()).await.unwrap().auto_sync_paused_until.is_none());
}
