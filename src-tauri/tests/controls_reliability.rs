//! 同步控制回归：真实临时仓库、写盘失败、运行时补偿、手动冲突保护与只读检查。
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use chrono::Local;
use tauri::Manager;
use git_syncdeck_lib::autosync::{self, Decision, NoHost, Outcome};
use git_syncdeck_lib::commands::{self, AppState, Shared};
use git_syncdeck_lib::config::ConfigStore;
use git_syncdeck_lib::model::{AutoSyncTrigger, SettingsPatch};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().into()
}

fn new_app(path: &Path) -> tauri::App<tauri::test::MockRuntime> {
    let app = tauri::test::mock_builder().build(tauri::test::mock_context(tauri::test::noop_assets())).unwrap();
    app.manage::<Shared>(Arc::new(AppState::new(ConfigStore::load(path.to_path_buf()))));
    app
}

fn snapshot(dir: &Path) -> (String, Vec<u8>, Vec<u8>, String) {
    (git(dir, &["rev-parse", "HEAD"]), std::fs::read(dir.join(".git/index")).unwrap(),
        std::fs::read(dir.join("README.md")).unwrap(), git(dir, &["status", "--porcelain=v2"]))
}

#[tokio::test(flavor = "multi_thread")]
async fn controls_reliability() {
    let root = std::env::temp_dir().join(format!("syncdeck-controls-{}", std::process::id()));
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/make-test-repos.mjs");
    assert!(Command::new("node").arg(script).arg(&root).status().unwrap().success());
    let config = root.join("config.json");
    let app = new_app(&config);
    let st = app.state::<Shared>().inner().clone();
    let defaults = commands::get_settings(app.state()).await.unwrap();
    assert!(!defaults.auto_sync_enabled && !defaults.periodic_check);
    assert!(defaults.launch_at_login && defaults.close_to_tray);
    let changes = Arc::new(Mutex::new(Vec::new()));
    let observed = changes.clone();
    st.set_autostart_hook(move |enabled| { observed.lock().unwrap().push(enabled); Ok(()) });
    commands::save_settings(app.state(), SettingsPatch { theme: Some("light".into()), ..Default::default() }).await.unwrap();
    let original = std::fs::read(&config).unwrap();
    let original_timeout = git_syncdeck_lib::git::network_timeout();
    let original_name = git_syncdeck_lib::util::computer_name();
    std::fs::create_dir(config.with_extension("json.tmp")).unwrap();
    let error = commands::save_settings(app.state(), SettingsPatch {
        launch_at_login: Some(false), host_name: Some("不能生效".into()), network_timeout_secs: Some(600),
        theme: Some("dark".into()), ..Default::default()
    }).await.unwrap_err();
    assert!(error.contains("配置文件"), "{error}");
    assert_eq!(*changes.lock().unwrap(), [false, true], "写盘失败补偿开机自启");
    assert_eq!(std::fs::read(&config).unwrap(), original);
    assert_eq!(git_syncdeck_lib::git::network_timeout(), original_timeout);
    assert_eq!(git_syncdeck_lib::util::computer_name(), original_name);
    assert_eq!(commands::get_settings(app.state()).await.unwrap().theme.as_deref(), Some("light"));
    std::fs::remove_dir(config.with_extension("json.tmp")).unwrap();
    // 修复写盘条件后重试，真实保存与运行时同步生效。
    commands::save_settings(app.state(), SettingsPatch { theme: Some("dark".into()), ..Default::default() }).await.unwrap();
    let pause = commands::pause_auto_sync(app.state(), Some("hour".into())).await.unwrap();
    let until = pause.auto_sync_paused_until.clone().unwrap();
    let parsed = chrono::DateTime::parse_from_rfc3339(&until).unwrap();
    assert!((parsed.with_timezone(&Local) - Local::now()).num_seconds() > 3590);
    commands::save_settings(app.state(), SettingsPatch { auto_sync_enabled: Some(true), sync_on_unlock: Some(false), ..Default::default() }).await.unwrap();
    let off = commands::save_settings(app.state(), SettingsPatch { auto_sync_enabled: Some(false), ..Default::default() }).await.unwrap();
    assert_eq!(off.auto_sync_paused_until.as_deref(), Some(until.as_str()));
    assert!(!off.sync_on_unlock);
    let reopened = ConfigStore::load(config.clone());
    assert_eq!(reopened.read(|c| c.settings.auto_sync_enabled), Some(false));
    assert_eq!(reopened.read(|c| c.settings.auto_sync_paused_until.clone()), Some(until));
    commands::resume_auto_sync(app.state()).await.unwrap();

    // 外部自启调用失败会明确返回，不落盘；补偿也失败时提供独立警告。
    let external_config = root.join("external-config.json");
    let external_app = new_app(&external_config);
    external_app.state::<Shared>().set_autostart_hook(|_| Err("模拟系统拒绝自启修改".into()));
    let failed = commands::save_settings(external_app.state(), SettingsPatch { launch_at_login: Some(false), ..Default::default() }).await.unwrap_err();
    assert!(failed.contains("模拟系统拒绝"));
    assert!(!external_config.exists());
    assert!(commands::get_settings(external_app.state()).await.unwrap().launch_at_login);
    let compensation_config = root.join("compensation.json");
    let compensation_app = new_app(&compensation_config);
    let compensation_state = compensation_app.state::<Shared>().inner().clone();
    compensation_state.set_autostart_hook(|enabled| if enabled { Err("模拟恢复失败".into()) } else { Ok(()) });
    compensation_state.config.update(|_| ()).unwrap();
    std::fs::create_dir(compensation_config.with_extension("json.tmp")).unwrap();
    let failed = commands::save_settings(compensation_app.state(), SettingsPatch { launch_at_login: Some(false), ..Default::default() }).await.unwrap_err();
    assert!(failed.contains("恢复开机自启也失败"));
    let recovered = commands::get_settings(compensation_app.state()).await.unwrap();
    assert!(recovered.config_read_error.is_none() && recovered.runtime_warning.is_some());

    let system_config = root.join("system-state.json");
    let system_app = new_app(&system_config);
    let system_state = system_app.state::<Shared>().inner().clone();
    let applied = Arc::new(Mutex::new(Vec::new()));
    let actual = applied.clone();
    system_state.set_autostart_hook(move |enabled| { actual.lock().unwrap().push(enabled); Ok(()) });
    system_state.set_autostart_query(|| Ok(false)); // 系统实际状态与配置默认值不同。
    system_state.config.update(|_| ()).unwrap();
    std::fs::create_dir(system_config.with_extension("json.tmp")).unwrap();
    assert!(commands::save_settings(system_app.state(), SettingsPatch { launch_at_login: Some(false), ..Default::default() }).await.is_err());
    assert_eq!(*applied.lock().unwrap(), [false, false], "补偿恢复系统实际原值，而不是配置默认值");

    let repos = root.join("repos");
    let names = ["dirty", "behind", "ahead", "no-remote", "diverged-conflict"];
    let added = commands::add_projects(app.state(), names.iter().map(|name| repos.join(name).to_string_lossy().into()).collect()).await.unwrap();
    assert!(added.iter().all(|p| p.last_checked_at.is_none()), "本地读取不能宣称已验证云端");
    let id = |name: &str| added.iter().find(|p| p.name == name).unwrap().id.clone();
    let behind = repos.join("behind");
    let ahead_remote = root.join("remotes/ahead.git");
    let before_head = git(&behind, &["rev-parse", "HEAD"]);
    let before_remote = git(&ahead_remote, &["rev-parse", "main"]);
    let before_dirty = snapshot(&repos.join("dirty"));
    for trigger in [AutoSyncTrigger::Startup, AutoSyncTrigger::Lock, AutoSyncTrigger::Resume, AutoSyncTrigger::Unlock] {
        assert!(matches!(autosync::handle(&st, &NoHost, trigger).await, Outcome::Skipped(Decision::Disabled)));
    }
    assert_eq!(git(&behind, &["rev-parse", "HEAD"]), before_head);
    commands::save_settings(app.state(), SettingsPatch { periodic_check: Some(true), ..Default::default() }).await.unwrap();
    let checked = commands::run_refresh_for(&st, Arc::new(|_| {}), Some(AutoSyncTrigger::Periodic)).await.unwrap();
    assert_eq!(git(&behind, &["rev-parse", "HEAD"]), before_head, "只读检查不拉取工作区");
    assert_eq!(git(&ahead_remote, &["rev-parse", "main"]), before_remote, "只读检查不推送");
    assert_eq!(snapshot(&repos.join("dirty")), before_dirty, "只读检查不提交");
    assert!(checked.iter().filter(|p| p.name != "no-remote").all(|p| p.last_checked_at.is_some()));
    assert!(checked.iter().find(|p| p.name == "no-remote").unwrap().last_checked_at.is_none());

    // 真正未解决的冲突：保护 HEAD、索引三个 stage 和工作区内容。
    let conflict = repos.join("diverged-conflict");
    let output = Command::new("git").args(["merge", "--no-edit", "origin/main"]).current_dir(&conflict).output().unwrap();
    assert!(!output.status.success());
    assert!(!git(&conflict, &["ls-files", "-u"]).is_empty());
    let before = snapshot(&conflict);
    let error = commands::commit_and_push(app.state(), id("diverged-conflict"), "禁止提交冲突".into()).await.unwrap_err();
    assert!(error.contains("冲突"), "{error}");
    assert_eq!(snapshot(&conflict), before);
    let local = commands::list_projects(app.state()).await.unwrap();
    assert!(local.iter().find(|p| p.name == "diverged-conflict").unwrap().commit_block_reason.is_some());
    // 即使用户先把文件暂存，MERGE_HEAD 仍要求去外部工具完成合并。
    git(&conflict, &["add", "-A"]);
    let before = snapshot(&conflict);
    let error = commands::commit_and_push(app.state(), id("diverged-conflict"), "禁止完成半途合并".into()).await.unwrap_err();
    assert!(error.contains("未完成"));
    assert_eq!(snapshot(&conflict), before);
    git(&conflict, &["merge", "--abort"]);

    let dirty = repos.join("dirty");
    for marker in ["CHERRY_PICK_HEAD", "REVERT_HEAD", "sequencer", "rebase-merge", "rebase-apply"] {
        let path = dirty.join(".git").join(marker);
        if marker.ends_with("HEAD") { std::fs::write(&path, git(&dirty, &["rev-parse", "HEAD"])).unwrap(); }
        else { std::fs::create_dir(&path).unwrap(); }
        let before = snapshot(&dirty);
        let error = commands::commit_and_push(app.state(), id("dirty"), "操作未完成".into()).await.unwrap_err();
        assert!(error.contains("未完成"), "{marker}: {error}");
        assert_eq!(snapshot(&dirty), before, "{marker} 不能被普通提交入口改变");
        if path.is_dir() { std::fs::remove_dir(path).unwrap(); } else { std::fs::remove_file(path).unwrap(); }
    }
    // 手动提交延续旧规则，不做自动提交检查；成功的 fetch 记录验证时间。
    let submitted = commands::commit_and_push(app.state(), id("dirty"), "手动提交".into()).await.unwrap();
    assert!(submitted.last_checked_at.is_some());
    assert!(submitted.last_sync_at.is_some());
    // 外部工具切换当前分支后不能沿用另一分支的验证时间或 1 分钟 fetch 复用。
    git(&dirty, &["checkout", "-q", "-b", "feature", "--track", "origin/main"]);
    let local = commands::list_projects(app.state()).await.unwrap();
    assert!(local.iter().find(|p| p.name == "dirty").unwrap().last_checked_at.is_none());
    let other = root.join("other-computer/dirty");
    git(&other, &["pull", "--ff-only", "origin", "main"]);
    std::fs::write(other.join("new-after-check.txt"), "new cloud commit").unwrap();
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-q", "-m", "new cloud commit"]);
    git(&other, &["push", "-q", "origin", "main"]);
    let cloud_head = git(&other, &["rev-parse", "HEAD"]);
    commands::run_sync(&st, Some(vec![id("dirty")]), Arc::new(|_| {})).await.unwrap();
    assert_eq!(git(&dirty, &["rev-parse", "HEAD"]), cloud_head, "切分支后必须重新 fetch 并拉取最新提交");
    assert!(commands::list_projects(app.state()).await.unwrap().iter().find(|p| p.name == "dirty").unwrap().last_checked_at.is_some());

}
