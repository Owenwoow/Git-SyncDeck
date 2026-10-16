//! 命令层端到端测试：用 Tauri 的模拟运行时直接调用前端会调用的命令，
//! 覆盖 扫描 → 添加 → 刷新（进度通道）→ 一键同步（进度通道）→ 提交并推送 → 持久化 → 取消监控。
//! 只使用临时测试仓库和临时配置目录。
//! 运行：cd src-tauri && cargo test --test commands_flow -- --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::Manager;

use git_syncdeck_lib::commands::{self, AppState, Shared};
use git_syncdeck_lib::config::ConfigStore;
use git_syncdeck_lib::git;
use git_syncdeck_lib::model::{ProxySource, SettingsPatch, SyncStatus};

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
        SettingsPatch {
            default_code_dir: Some(repos.to_string_lossy().into()),
            theme: Some("dark".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(s.theme.as_deref(), Some("dark"));
    assert!(commands::save_settings(app.state(), SettingsPatch { theme: Some("pink".into()), ..Default::default() })
        .await
        .is_err());

    // 联网超时：默认 3 分钟；只接受固定几档；保存后立即生效
    assert_eq!(s.network_timeout_secs, 180);
    assert!(commands::save_settings(
        app.state(),
        SettingsPatch { network_timeout_secs: Some(42), ..Default::default() }
    )
    .await
    .is_err());
    let s = commands::save_settings(app.state(), SettingsPatch { network_timeout_secs: Some(600), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(s.network_timeout_secs, 600);
    assert_eq!(git::network_timeout().as_secs(), 600);

    // 代理：默认自动；手动地址要校验；保存后立即作用到 SyncDeck 调用的 git
    assert_eq!(s.proxy_mode, "auto");
    let proxy = |mode: Option<&str>, url: Option<&str>| SettingsPatch {
        proxy_mode: mode.map(str::to_string),
        proxy_url: url.map(str::to_string),
        ..Default::default()
    };
    for (mode, url) in [
        (Some("manual"), None),                        // 手动但没填地址
        (Some("manual"), Some("127.0.0.1:7890")),      // 没写协议
        (Some("manual"), Some("ftp://127.0.0.1:21")),  // 不支持的协议
        (None, Some("http://127.0.0.1")),              // 没有端口
        (Some("system"), None),                        // 不认识的模式
    ] {
        let e = commands::save_settings(app.state(), proxy(mode, url)).await.unwrap_err();
        println!("  代理设置 {mode:?} {url:?} → {e}");
    }
    assert_eq!(commands::get_settings(app.state()).await.unwrap().proxy_mode, "auto", "保存失败时什么都不改");
    let s = commands::save_settings(app.state(), proxy(Some("manual"), Some(" http://me:pw@127.0.0.1:7890/ ")))
        .await
        .unwrap();
    assert_eq!((s.proxy_mode.as_str(), s.proxy_source), ("manual", ProxySource::Manual));
    assert_eq!(s.proxy_url.as_deref(), Some("http://me:pw@127.0.0.1:7890"));
    assert_eq!(s.effective_proxy.as_deref(), Some("http://127.0.0.1:7890"), "显示时去掉账号密码");
    assert_eq!(git::proxy_plan().over, git::ProxyOverride::Use("http://me:pw@127.0.0.1:7890".into()));
    let s = commands::save_settings(app.state(), proxy(Some("off"), None)).await.unwrap();
    assert_eq!((s.proxy_source, s.effective_proxy.as_deref()), (ProxySource::NoProxy, None));
    assert_eq!(s.proxy_url.as_deref(), Some("http://me:pw@127.0.0.1:7890"), "切到别的模式后仍保留手动地址");
    assert_eq!(git::proxy_plan().over, git::ProxyOverride::Disable);
    let s = commands::save_settings(app.state(), proxy(Some("manual"), None)).await.unwrap();
    assert_eq!(s.proxy_source, ProxySource::Manual, "已有地址时可以直接切回手动");
    let s = commands::save_settings(app.state(), proxy(Some("auto"), None)).await.unwrap();
    println!("  自动模式下本机的代理：{:?} {:?}", s.proxy_source, s.effective_proxy);
    assert_eq!(s.proxy_mode, "auto");
    assert_ne!(s.proxy_source, ProxySource::Manual);
    assert!(!matches!(git::proxy_plan().over, git::ProxyOverride::Disable));
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg_file).unwrap()).unwrap();
    assert!(saved["settings"]["proxyMode"].is_null(), "自动模式不单独保存");
    assert_eq!(saved["settings"]["proxyUrl"], "http://me:pw@127.0.0.1:7890");

    // 扫描：11 个仓库，都没监控
    let scanned = commands::scan_directory(app.state(), repos.to_string_lossy().into()).await.unwrap();
    println!("扫描到 {} 个仓库", scanned.len());
    assert_eq!(scanned.len(), 11);
    assert!(scanned.iter().all(|r| !r.monitored));
    let detached = scanned.iter().find(|r| r.name == "detached").unwrap();
    assert_eq!(detached.branch, "(detached HEAD)");

    // 添加全部（重复添加会被忽略）
    let paths: Vec<String> = scanned.iter().map(|r| r.path.clone()).collect();
    let added = commands::add_projects(app.state(), paths.clone()).await.unwrap();
    assert_eq!(added.len(), 11);
    assert_eq!(commands::add_projects(app.state(), paths).await.unwrap().len(), 0);
    let rescanned = commands::scan_directory(app.state(), repos.to_string_lossy().into()).await.unwrap();
    assert!(rescanned.iter().all(|r| r.monitored && r.project_id.is_some()));

    // 刷新：fetch + status，每个项目检查完就通过 Channel 发一次 done
    let refresh_events: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let sink = refresh_events.clone();
    let channel = Channel::new(move |body| {
        if let InvokeResponseBody::Json(s) = body {
            sink.lock().unwrap().push(serde_json::from_str(&s).unwrap());
        }
        Ok(())
    });
    let list = commands::refresh_status(app.state(), channel).await.unwrap();
    let refresh_events = refresh_events.lock().unwrap().clone();
    println!("\n== 刷新状态（{} 个进度事件）==", refresh_events.len());
    assert_eq!(refresh_events.len(), 11, "每个项目各发一次");
    assert!(refresh_events.iter().all(|e| e["type"] == "done" && e["total"] == 11));
    let mut idx: Vec<u64> = refresh_events.iter().map(|e| e["index"].as_u64().unwrap()).collect();
    idx.sort();
    assert_eq!(idx, (0..11).collect::<Vec<_>>(), "index 是完成顺序 0..10");
    // 事件里的项目和最终返回的列表一一对应、内容一致
    let mut from_events: Vec<serde_json::Value> = refresh_events.iter().map(|e| e["project"].clone()).collect();
    let mut from_list: Vec<serde_json::Value> = list.iter().map(|p| serde_json::to_value(p).unwrap()).collect();
    let by_id = |a: &serde_json::Value, b: &serde_json::Value| a["id"].as_str().cmp(&b["id"].as_str());
    from_events.sort_by(by_id);
    from_list.sort_by(by_id);
    assert_eq!(from_events, from_list);
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
    assert_eq!(starts, 11);
    assert_eq!(dones.len(), 11);
    // done 的 index 是完成顺序 0..10，界面据此计算进度
    let mut idx: Vec<u64> = dones.iter().map(|e| e["index"].as_u64().unwrap()).collect();
    idx.sort();
    assert_eq!(idx, (0..11).collect::<Vec<_>>());
    // 事件里的字段名是 camelCase，和前端类型一致
    assert!(dones[0]["result"]["projectId"].is_string());
    assert!(dones[0]["project"]["checkError"].is_null() || dones[0]["project"]["checkError"].is_string());
    assert_eq!(result.synced_count, 3);
    assert_eq!(result.failed_count, 8);

    // 异常原因已持久化；成功的项目有上次同步时间
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg_file).unwrap()).unwrap();
    let saved_projects = saved["projects"].as_array().unwrap();
    let with_issue = saved_projects.iter().filter(|p| p["issue"].is_string()).count();
    let with_time = saved_projects.iter().filter(|p| p["lastSyncAt"].is_string()).count();
    println!("  配置文件：{with_issue} 个项目保存了异常原因，{with_time} 个项目有上次同步时间");
    assert_eq!(with_issue, 8);
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
    assert_eq!(count, 11);
    assert_eq!(theme.as_deref(), Some("dark"));
    assert!(dirty_issue.is_none());
    assert_eq!(reloaded.read(|c| c.settings.network_timeout_secs), Some(600));

    // 取消监控：只改清单，仓库目录还在
    commands::remove_project(app.state(), dirty_id).await.unwrap();
    assert_eq!(commands::list_projects(app.state()).await.unwrap().len(), 10);
    assert!(repos.join("dirty").join(".git").exists(), "取消监控不能删除仓库");
    println!("  取消监控后剩 10 个项目，仓库目录仍在");
}
