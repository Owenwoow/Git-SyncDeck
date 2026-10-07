//! 一键同步时的自动提交（按项目开关）：用 Tauri 的模拟运行时调用命令，
//! 覆盖 提交信息模板设置 → 打开开关 → 一键同步自动提交并推送，以及各种不该提交的情况。
//! 只使用 scripts/make-test-repos.mjs 生成的临时仓库和临时配置目录。
//! 运行：cd src-tauri && cargo test --test auto_commit -- --nocapture

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::Manager;

use git_syncdeck_lib::commands::{self, AppState, Shared};
use git_syncdeck_lib::config::ConfigStore;
use git_syncdeck_lib::model::{Project, SettingsPatch, SyncItemResult, SyncOutcome, SyncResult, SyncStatus};
use git_syncdeck_lib::util::{computer_name, DEFAULT_COMMIT_TEMPLATE};

const TEMPLATE: &str = "测试自动同步：{date} 来自 {host}";

fn fixtures(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(name);
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/make-test-repos.mjs");
    assert!(Command::new("node").arg(script).arg(&root).status().unwrap().success());
    root
}

fn fresh_config_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("syncdeck-auto-config");
    let _ = std::fs::remove_dir_all(&dir); // 测试自己的临时配置目录
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 测试里直接读 git 状态用（只读）
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(["-c", "core.quotepath=false"]).args(args).current_dir(dir).output().expect("git");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Snapshot {
    head: String,
    remote: String,
    porcelain: String,
}

fn snapshot(root: &Path, name: &str) -> Snapshot {
    let local = root.join("repos").join(name);
    Snapshot {
        head: git(&local, &["rev-parse", "HEAD"]),
        remote: remote_main(root, name),
        porcelain: git(&local, &["status", "--porcelain"]),
    }
}

fn remote_main(root: &Path, name: &str) -> String {
    git(&root.join("remotes").join(format!("{name}.git")), &["rev-parse", "main"])
}

fn remote_subject(root: &Path, name: &str) -> String {
    git(&root.join("remotes").join(format!("{name}.git")), &["log", "-1", "--format=%s", "main"])
}

/// 提交标题是否符合模板：测试自动同步：YYYY-MM-DD HH:mm 来自 <电脑名>
fn matches_template(subject: &str) -> bool {
    let Some(rest) = subject.strip_prefix("测试自动同步：") else { return false };
    let Some(date) = rest.strip_suffix(&format!(" 来自 {}", computer_name())) else { return false };
    let b = date.as_bytes();
    date.len() == 16
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b' '
        && b[13] == b':'
        && date.chars().filter(|c| c.is_ascii_digit()).count() == 12
}

async fn run_sync(app: &tauri::App<tauri::test::MockRuntime>) -> (SyncResult, Vec<serde_json::Value>) {
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
    (result, events)
}

fn print_items(title: &str, items: &HashMap<String, SyncItemResult>) {
    println!("\n== {title} ==");
    let mut names: Vec<_> = items.keys().collect();
    names.sort();
    for n in names {
        let r = &items[n];
        println!(
            "  {n:<15} → {:<10} 提交 {} {}",
            format!("{:?}", r.outcome),
            r.commits,
            r.reason.as_deref().map(|s| s.replace('\n', " ")).unwrap_or_default()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_commit_on_sync() {
    let root = fixtures("syncdeck-auto-test");
    let repos = root.join("repos");
    let repo = |name: &str| repos.join(name);
    let cfg_file = fresh_config_dir().join("config.json");

    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    app.manage::<Shared>(Arc::new(AppState::new(ConfigStore::load(cfg_file.clone()))));

    // ---------------- 提交信息模板设置 ----------------
    let s = commands::get_settings(app.state()).await.unwrap();
    assert_eq!(s.commit_template, DEFAULT_COMMIT_TEMPLATE, "没设置过时应为默认模板");
    let patch = |t: &str| SettingsPatch { commit_template: Some(t.into()), ..Default::default() };
    let s = commands::save_settings(app.state(), patch(&format!("  {TEMPLATE}  "))).await.unwrap();
    assert_eq!(s.commit_template, TEMPLATE);
    let s = commands::save_settings(app.state(), patch("   ")).await.unwrap();
    assert_eq!(s.commit_template, DEFAULT_COMMIT_TEMPLATE, "空模板应恢复默认");
    commands::save_settings(app.state(), patch(TEMPLATE)).await.unwrap();

    // ---------------- 添加项目、打开开关 ----------------
    let names = ["dirty", "dirty-behind", "dirty-conflict"];
    let added = commands::add_projects(app.state(), names.iter().map(|n| repo(n).to_string_lossy().into()).collect())
        .await
        .unwrap();
    assert_eq!(added.len(), 3);
    assert!(added.iter().all(|p| !p.auto_commit), "新加入的项目默认不自动提交");
    let id_of: HashMap<String, String> = added.iter().map(|p| (p.name.clone(), p.id.clone())).collect();
    let name_of: HashMap<String, String> = added.iter().map(|p| (p.id.clone(), p.name.clone())).collect();

    for n in ["dirty", "dirty-conflict"] {
        let p = commands::set_auto_commit(app.state(), id_of[n].clone(), true).await.unwrap();
        assert!(p.auto_commit);
        assert_eq!(p.status, SyncStatus::Dirty);
    }
    assert!(commands::set_auto_commit(app.state(), "p-不存在".into(), true).await.is_err());
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg_file).unwrap()).unwrap();
    let on: Vec<_> = saved["projects"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["autoCommit"] == true)
        .map(|p| p["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(on.len(), 2, "开关应写进配置文件");

    // ---------------- 第一轮一键同步 ----------------
    let before: HashMap<&str, Snapshot> = names.iter().map(|n| (*n, snapshot(&root, n))).collect();
    let (result, events) = run_sync(&app).await;
    let items: HashMap<String, SyncItemResult> =
        result.items.iter().map(|r| (name_of[&r.project_id].clone(), r.clone())).collect();
    print_items("第一轮一键同步（dirty、dirty-conflict 打开了自动提交）", &items);
    assert_eq!(result.synced_count, 1);
    assert_eq!(result.failed_count, 2);
    // 进度事件里的项目（字段名是 camelCase，含 autoCommit）
    let done_projects: HashMap<String, Project> = events
        .iter()
        .filter(|e| e["type"] == "done")
        .map(|e| {
            assert!(e["project"]["autoCommit"].is_boolean(), "进度事件里的项目应带 autoCommit");
            let p: Project = serde_json::from_value(e["project"].clone()).unwrap();
            (p.name.clone(), p)
        })
        .collect();
    let committed_event = events.iter().find(|e| e["type"] == "done" && e["result"]["outcome"] == "committed");
    assert!(committed_event.is_some(), "进度事件里 outcome 应序列化为 \"committed\"");

    // ① 打开了自动提交的 dirty：已提交并推送，提交标题符合模板，工作区干净
    let r = &items["dirty"];
    assert_eq!(r.outcome, SyncOutcome::Committed, "dirty 应自动提交并推送：{:?}", r.reason);
    assert_eq!(r.commits, 1);
    let subject = remote_subject(&root, "dirty");
    println!("  dirty 云端最新提交：{subject}");
    assert!(matches_template(&subject), "提交标题不符合模板：{subject}");
    assert_eq!(remote_main(&root, "dirty"), git(&repo("dirty"), &["rev-parse", "HEAD"]));
    assert_eq!(git(&repo("dirty"), &["rev-parse", "HEAD~1"]), before["dirty"].head, "新提交应直接接在原 HEAD 后");
    assert_eq!(git(&repo("dirty"), &["status", "--porcelain"]), "", "工作区应干净");
    let files = git(&root.join("remotes").join("dirty.git"), &["ls-tree", "-r", "--name-only", "main"]);
    for f in ["README.md", "src/新功能.ts", "中文 笔记.md"] {
        assert!(files.lines().any(|l| l == f), "云端缺少 {f}：{files}");
    }
    let p = &done_projects["dirty"];
    assert_eq!(p.status, SyncStatus::Synced);
    assert!(p.issue.is_none() && p.last_sync_at.is_some() && p.auto_commit);

    // ② 没打开开关的 dirty-behind：仍记为异常，没有新提交，工作区原样
    let r = &items["dirty-behind"];
    assert_eq!(r.outcome, SyncOutcome::Failed);
    let now = snapshot(&root, "dirty-behind");
    let was = &before["dirty-behind"];
    assert_eq!((&now.head, &now.remote, &now.porcelain), (&was.head, &was.remote, &was.porcelain), "dirty-behind 不应被改动");
    assert!(done_projects["dirty-behind"].issue.is_some());

    // ④ 打开了开关、落后且改了同一个文件的 dirty-conflict：拉取被拒绝 → 异常，没有新提交，本地改动还在
    let r = &items["dirty-conflict"];
    assert_eq!(r.outcome, SyncOutcome::Failed);
    let reason = r.reason.clone().unwrap_or_default();
    assert!(reason.contains("未提交任何内容"), "原因应说明未提交：{reason}");
    let now = snapshot(&root, "dirty-conflict");
    let was = &before["dirty-conflict"];
    assert_eq!((&now.head, &now.remote, &now.porcelain), (&was.head, &was.remote, &was.porcelain), "dirty-conflict 不应被改动");
    let readme = std::fs::read_to_string(repo("dirty-conflict").join("README.md")).unwrap();
    assert!(readme.contains("台式机上没提交的改动"), "本地改动应保留");
    assert_eq!(done_projects["dirty-conflict"].issue.as_deref(), Some(reason.as_str()));
    println!("  ✓ dirty-behind、dirty-conflict 的 HEAD、云端和工作区均未改动");

    // ---------------- 第二轮：打开 dirty-behind 的开关 ----------------
    commands::set_auto_commit(app.state(), id_of["dirty-behind"].clone(), true).await.unwrap();
    let old_remote = before["dirty-behind"].remote.clone();
    let (result, _) = run_sync(&app).await;
    let items: HashMap<String, SyncItemResult> =
        result.items.iter().map(|r| (name_of[&r.project_id].clone(), r.clone())).collect();
    print_items("第二轮一键同步（三个都打开了自动提交）", &items);
    assert_eq!(result.synced_count, 2);
    assert_eq!(result.failed_count, 1);
    assert_eq!(items["dirty"].outcome, SyncOutcome::UpToDate);

    // ③ dirty-behind（改的不是同一个文件）：先快进拉取，再提交并推送
    let r = &items["dirty-behind"];
    assert_eq!(r.outcome, SyncOutcome::Committed, "dirty-behind 应先拉取再提交推送：{:?}", r.reason);
    assert_eq!(r.commits, 1);
    let local = repo("dirty-behind");
    assert_eq!(remote_main(&root, "dirty-behind"), git(&local, &["rev-parse", "HEAD"]));
    assert_eq!(git(&local, &["rev-parse", "HEAD~1"]), old_remote, "应先快进到云端，再在其上提交（没有合并提交）");
    assert_eq!(git(&local, &["rev-list", "--parents", "-n", "1", "HEAD"]).split(' ').count(), 2);
    assert!(local.join("laptop.md").exists(), "云端的新文件应已拉下来");
    assert_eq!(git(&local, &["status", "--porcelain"]), "");
    assert!(matches_template(&remote_subject(&root, "dirty-behind")));

    // dirty-conflict 仍然是异常、仍然没动
    assert_eq!(items["dirty-conflict"].outcome, SyncOutcome::Failed);
    let now = snapshot(&root, "dirty-conflict");
    let was = &before["dirty-conflict"];
    assert_eq!((&now.head, &now.remote, &now.porcelain), (&was.head, &was.remote, &was.porcelain));

    // 模拟重启：开关和模板都在
    let reloaded = ConfigStore::load(cfg_file);
    let (count_on, template) =
        reloaded.read(|c| (c.projects.iter().filter(|p| p.auto_commit).count(), c.settings.commit_template.clone()));
    assert_eq!(count_on, 3);
    assert_eq!(template.as_deref(), Some(TEMPLATE));
    println!("  ✓ 重启后 3 个项目的自动提交开关和提交信息模板都还在");
}
