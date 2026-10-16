//! 一键同步复用刚检查过的结果：刷新后 1 分钟内的一键同步不再 fetch，直接用本地的远程跟踪分支判断；
//! 没刚检查过的项目照常 fetch。只使用 scripts/make-test-repos.mjs 生成的临时仓库和临时配置目录。
//! 运行：cd src-tauri && cargo test --test sync_reuse -- --nocapture

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::Manager;

use git_syncdeck_lib::commands::{self, AppState, Shared};
use git_syncdeck_lib::config::ConfigStore;
use git_syncdeck_lib::model::{SyncOutcome, SyncResult, SyncStatus};

fn fixtures(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(name);
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/make-test-repos.mjs");
    assert!(Command::new("node").arg(script).arg(&root).status().unwrap().success());
    root
}

fn fresh_config_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("syncdeck-reuse-config");
    let _ = std::fs::remove_dir_all(&dir); // 测试自己的临时配置目录
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 测试里直接操作临时仓库用
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

fn outcomes(result: &SyncResult, name_of: &HashMap<String, String>) -> HashMap<String, SyncOutcome> {
    result.items.iter().map(|r| (name_of[&r.project_id].clone(), r.outcome)).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_reuses_recent_check() {
    let root = fixtures("syncdeck-reuse-test");
    let repos = root.join("repos");
    let cfg_file = fresh_config_dir().join("config.json");

    let app = new_app(&cfg_file);
    let added = commands::add_projects(
        app.state(),
        ["synced", "behind"].iter().map(|n| repos.join(n).to_string_lossy().into()).collect(),
    )
    .await
    .unwrap();
    let name_of: HashMap<String, String> = added.iter().map(|p| (p.id.clone(), p.name.clone())).collect();

    // 刷新：两个项目都联网检查成功
    let list = commands::refresh_status(app.state(), Channel::new(|_| Ok(()))).await.unwrap();
    let status: HashMap<_, _> = list.iter().map(|p| (p.name.as_str(), (p.status, p.check_error.clone()))).collect();
    assert_eq!(status["synced"], (SyncStatus::Synced, None));
    assert_eq!(status["behind"], (SyncStatus::Behind, None));

    // 刷新之后"另一台电脑"又往 synced 的云端推了一个提交
    let other = root.join("other-computer").join("synced");
    std::fs::write(other.join("after-refresh.md"), "刷新之后才推的\n").unwrap();
    git(&other, &["add", "after-refresh.md"]);
    git(&other, &["commit", "--quiet", "-m", "刷新之后才推的"]);
    git(&other, &["push", "--quiet", "origin", "main"]);
    let synced_head = git(&repos.join("synced"), &["rev-parse", "HEAD"]);

    // 1 分钟内的一键同步：复用刷新的结果，不 fetch，所以看不到这个新提交
    let result = commands::sync_all(app.state(), Channel::new(|_| Ok(()))).await.unwrap();
    let got = outcomes(&result, &name_of);
    println!("刷新后立即一键同步：{got:?}");
    assert_eq!(got["synced"], SyncOutcome::UpToDate, "刚检查过的项目应跳过 fetch");
    assert_eq!(got["behind"], SyncOutcome::Pulled, "刷新时已知落后的项目照常拉取");
    assert_eq!(git(&repos.join("synced"), &["rev-parse", "HEAD"]), synced_head);
    assert_eq!(
        git(&repos.join("synced"), &["rev-parse", "origin/main"]),
        synced_head,
        "跳过 fetch 时远程跟踪分支不应变化"
    );

    // 重新打开应用（内存里没有检查记录）：一键同步照常 fetch，拉到新提交
    drop(app);
    let app = new_app(&cfg_file);
    let result = commands::sync_all(app.state(), Channel::new(|_| Ok(()))).await.unwrap();
    let got = outcomes(&result, &name_of);
    println!("没刚检查过时一键同步：{got:?}");
    assert_eq!(got["synced"], SyncOutcome::Pulled, "没刚检查过的项目应先 fetch");
    assert_eq!(got["behind"], SyncOutcome::UpToDate);
    assert!(repos.join("synced").join("after-refresh.md").exists());
}
