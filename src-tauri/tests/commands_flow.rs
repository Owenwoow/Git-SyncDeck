//! 命令层端到端测试：用 Tauri 的模拟运行时直接调用前端会调用的命令，
//! 覆盖 扫描 → 添加 → 刷新 → 一键同步（进度通道）→ 提交并推送 → 持久化 → 取消监控。
//! 只使用临时测试仓库和临时配置目录。
//! 运行：cd src-tauri && cargo test --test commands_flow -- --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::Manager;

use git_syncdeck_lib::commands::{self, AppState, Shared};
use git_syncdeck_lib::config::ConfigStore;
use git_syncdeck_lib::model::{SettingsPatch, SyncStatus};

fn fixtures(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(name);
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/make-test-repos.mjs");
    assert!(Command::new("node").arg(script).arg(&root).status().unwrap().success());
    root
}

fn fresh_config_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("syncdeck-cmd-config");
    let _ = std::fs::remove_dir_all(&dir); // 测试自己的临时配置目录
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn full_command_flow() {
    let root = fixtures("syncdeck-cmd-test");
    let repos = root.join("repos");
    let cfg_dir = fresh_config_dir();
    let cfg_file = cfg_dir.join("config.json");

    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    app.manage::<Shared>(Arc::new(AppState::new(ConfigStore::load(cfg_file.clone()))));

    // 设置
    let s = commands::save_settings(
        app.state(),
        SettingsPatch { default_code_dir: Some(repos.to_string_lossy().into()), theme: Some("dark".into()) },
    )
    .await
    .unwrap();
    assert_eq!(s.theme.as_deref(), Some("dark"));
    assert!(commands::save_settings(app.state(), SettingsPatch { theme: Some("pink".into()), ..Default::default() })
        .await
        .is_err());

    // 扫描：10 个仓库，都没监控
    let scanned = commands::scan_directory(app.state(), repos.to_string_lossy().into()).await.unwrap();
    println!("扫描到 {} 个仓库", scanned.len());
    assert_eq!(scanned.len(), 10);
    assert!(scanned.iter().all(|r| !r.monitored));
    let detached = scanned.iter().find(|r| r.name == "detached").unwrap();
    assert_eq!(detached.branch, "(detached HEAD)");

    // 添加全部（重复添加会被忽略）
    let paths: Vec<String> = scanned.iter().map(|r| r.path.clone()).collect();
    let added = commands::add_projects(app.state(), paths.clone()).await.unwrap();
    assert_eq!(added.len(), 10);
    assert_eq!(commands::add_projects(app.state(), paths).await.unwrap().len(), 0);
    let rescanned = commands::scan_directory(app.state(), repos.to_string_lossy().into()).await.unwrap();
    assert!(rescanned.iter().all(|r| r.monitored && r.project_id.is_some()));

    // 刷新：fetch + status
    let list = commands::refresh_status(app.state()).await.unwrap();
    println!("\n== 刷新状态 ==");
    for p in &list {
        println!(
            "  {:<13} {:<6} {}",
            p.name,
            p.status.label(),
            p.check_error.as_deref().map(|e| format!("检查失败：{}", e.lines().next().unwrap_or(""))).unwrap_or_default()
        );
    }
    let by_name = |list: &[git_syncdeck_lib::model::Project], n: &str| list.iter().find(|p| p.name == n).cloned().unwrap();
    assert_eq!(by_name(&list, "behind").status, SyncStatus::Behind);
    assert!(by_name(&list, "fetch-fail").check_error.is_some());
    assert!(by_name(&list, "synced").check_error.is_none());

    // 一键同步：通过 Channel 收进度
    let events: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let sink = events.clone();
    let channel = Channel::new(move |body| {
        if let InvokeResponseBody::Json(s) = body {
            sink.lock().unwrap().push(serde_json::from_str(&s).unwrap());
        }
        Ok(())
    });
    let result = commands::sync_all(app.state(), channel).await.unwrap();
    let events = events.lock().unwrap().clone();
    let starts = events.iter().filter(|e| e["type"] == "start").count();
    let dones: Vec<_> = events.iter().filter(|e| e["type"] == "done").collect();
    println!("\n== 一键同步 ==\n  进度事件：{starts} 个 start，{} 个 done；已同步 {}，异常 {}", dones.len(), result.synced_count, result.failed_count);
    assert_eq!(starts, 10);
    assert_eq!(dones.len(), 10);
    // done 的 index 是完成顺序 0..9，界面据此计算进度
    let mut idx: Vec<u64> = dones.iter().map(|e| e["index"].as_u64().unwrap()).collect();
    idx.sort();
    assert_eq!(idx, (0..10).collect::<Vec<_>>());
    // 事件里的字段名是 camelCase，和前端类型一致
    assert!(dones[0]["result"]["projectId"].is_string());
    assert!(dones[0]["project"]["checkError"].is_null() || dones[0]["project"]["checkError"].is_string());
    assert_eq!(result.synced_count, 3);
    assert_eq!(result.failed_count, 7);

    // 异常原因已持久化；成功的项目有上次同步时间
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg_file).unwrap()).unwrap();
    let saved_projects = saved["projects"].as_array().unwrap();
    let with_issue = saved_projects.iter().filter(|p| p["issue"].is_string()).count();
    let with_time = saved_projects.iter().filter(|p| p["lastSyncAt"].is_string()).count();
    println!("  配置文件：{with_issue} 个项目保存了异常原因，{with_time} 个项目有上次同步时间");
    assert_eq!(with_issue, 7);
    assert_eq!(with_time, 3);

    // 同步进行中不能再开一轮
    // （这里同步已结束，标记应已清除，再同步一次应该可以执行）
    let again = commands::sync_all(app.state(), Channel::new(|_| Ok(()))).await.unwrap();
    assert_eq!(again.synced_count, 3);

    // 提交并推送 dirty：成功后异常清除
    let dirty_id = by_name(&list, "dirty").id;
    let p = commands::commit_and_push(app.state(), dirty_id.clone(), "测试：命令层提交".into()).await.unwrap();
    println!("\n== 提交并推送 ==\n  dirty → {}，异常：{:?}", p.status.label(), p.issue);
    assert_eq!(p.status, SyncStatus::Synced);
    assert!(p.issue.is_none() && p.last_sync_at.is_some());

    // 诊断信息：最近 3 次提交（dirty 现在有 2 个提交）
    let diag = commands::get_diagnostic_info(app.state(), dirty_id.clone()).await.unwrap();
    println!("  诊断：最近提交 {:?}", diag.recent_commits.iter().map(|c| &c.subject).collect::<Vec<_>>());
    assert_eq!(diag.recent_commits.len(), 2);
    assert_eq!(diag.recent_commits[0].subject, "测试：命令层提交");

    // 模拟重启：重新从磁盘读配置
    let reloaded = ConfigStore::load(cfg_file.clone());
    let (count, dirty_issue, theme) = reloaded.read(|c| {
        (c.projects.len(), c.projects.iter().find(|p| p.id == dirty_id).unwrap().issue.clone(), c.settings.theme.clone())
    });
    println!("  重启后：{count} 个项目，主题 {theme:?}，dirty 的异常 {dirty_issue:?}");
    assert_eq!(count, 10);
    assert_eq!(theme.as_deref(), Some("dark"));
    assert!(dirty_issue.is_none());

    // 取消监控：只改清单，仓库目录还在
    commands::remove_project(app.state(), dirty_id).await.unwrap();
    assert_eq!(commands::list_projects(app.state()).await.unwrap().len(), 9);
    assert!(repos.join("dirty").join(".git").exists(), "取消监控不能删除仓库");
    println!("  取消监控后剩 9 个项目，仓库目录仍在");
}
