//! 刷新途中用户改了设置：刷新结果要用检查完那一刻的持久化字段（自动提交开关等），
//! 不能用刷新开始时的旧快照把用户刚改的值盖回去；刷新途中取消监控的项目不再出现在结果里。
//! 只使用 scripts/make-test-repos.mjs 生成的临时仓库和临时配置目录。
//! 运行：cd src-tauri && cargo test --test refresh_race -- --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::Manager;

use git_syncdeck_lib::commands::{self, AppState, Shared};
use git_syncdeck_lib::config::ConfigStore;
use git_syncdeck_lib::util::CONCURRENCY;

fn fixtures(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(name);
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/make-test-repos.mjs");
    assert!(Command::new("node").arg(script).arg(&root).status().unwrap().success());
    root
}

fn fresh_config_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("syncdeck-race-config");
    let _ = std::fs::remove_dir_all(&dir); // 测试自己的临时配置目录
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_keeps_changes_made_during_refresh() {
    let root = fixtures("syncdeck-race-test");
    let repos = root.join("repos");
    let cfg_file = fresh_config_dir().join("config.json");

    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let st: Shared = Arc::new(AppState::new(ConfigStore::load(cfg_file)));
    app.manage::<Shared>(st.clone());

    let scanned = commands::scan_directory(app.state(), repos.to_string_lossy().into()).await.unwrap();
    let added = commands::add_projects(app.state(), scanned.iter().map(|r| r.path.clone()).collect()).await.unwrap();
    assert!(added.len() > CONCURRENCY, "项目数要多于并行数，才能保证有项目在第一个结果出来时还没开始检查");
    assert!(added.iter().all(|p| !p.auto_commit));
    // 按加入顺序排在最后的项目要等前面的检查完才开始，第一个结果出来时它一定还没检查
    let removed_id = added.last().unwrap().id.clone();

    // 第一个项目检查完时（这时排在后面的项目还在等），模拟用户：
    // 打开所有项目的"自动提交"，并取消监控最后一个项目（和 remove_project 一样只改清单）
    let done = Arc::new(AtomicBool::new(false));
    let events: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let (sink, st2, done2, removed2) = (events.clone(), st.clone(), done.clone(), removed_id.clone());
    let channel = Channel::new(move |body| {
        if let InvokeResponseBody::Json(s) = body {
            sink.lock().unwrap().push(serde_json::from_str(&s).unwrap());
        }
        if !done2.swap(true, Ordering::SeqCst) {
            st2.config
                .update(|c| {
                    c.projects.iter_mut().for_each(|p| p.auto_commit = true);
                    c.projects.retain(|p| p.id != removed2);
                })
                .unwrap();
        }
        Ok(())
    });
    let list = commands::refresh_status(app.state(), channel).await.unwrap();
    let events = events.lock().unwrap().clone();

    let turned_on = list.iter().filter(|p| p.auto_commit).count();
    println!("刷新结果 {} 个项目，其中 {turned_on} 个带上了刷新途中打开的自动提交", list.len());
    // 至少"第一个结果出来时还没开始检查"的那几个（不含被取消监控的）一定是新值；旧代码这里是 0
    let waiting = added.len() - CONCURRENCY;
    assert!(turned_on >= waiting - 1, "刷新途中打开的自动提交被改回去了：{turned_on} < {}", waiting - 1);
    assert!(list.iter().all(|p| p.id != removed_id), "刷新途中取消监控的项目不应出现在结果里");
    assert!(events.iter().all(|e| e["project"]["id"] != removed_id.as_str()), "也不应为它发进度事件");
    assert_eq!(events.len(), list.len());

    // 配置里的开关没被刷新改回去
    let on_in_config = st.config.read(|c| c.projects.iter().filter(|p| p.auto_commit).count());
    assert_eq!(on_in_config, added.len() - 1);
    // 本地检查（读配置 + 缓存）也是新值
    let local = commands::list_projects(app.state()).await.unwrap();
    assert_eq!(local.len(), added.len() - 1);
    assert!(local.iter().all(|p| p.auto_commit));
}
