//! 分叉时的自动合并和自动提交前检查：用 Tauri 的模拟运行时调用命令，覆盖
//! 关掉自动合并时分叉照旧记为异常 → 打开后无冲突的分叉自动合并并推送、有冲突的什么都不动 →
//! 诊断信息里的冲突文件和云端最近的提交 → 提交前检查拦下疑似密钥文件和超过上限的大文件。
//! 只使用 scripts/make-test-repos.mjs 生成的临时仓库和临时配置目录；
//! 用 GIT_CONFIG_GLOBAL / GIT_CONFIG_NOSYSTEM 把 git 的全局配置换成空的临时文件，合并结果不受本机配置影响。
//! 这个文件里只有一个测试，因为它要改本进程的环境变量。
//! 运行：cd src-tauri && cargo test --test auto_merge -- --nocapture

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::Manager;

use git_syncdeck_lib::commands::{self, AppState, Shared};
use git_syncdeck_lib::config::ConfigStore;
use git_syncdeck_lib::model::{Project, SettingsPatch, SyncItemResult, SyncOutcome, SyncStatus};
use git_syncdeck_lib::ops;
use git_syncdeck_lib::util::computer_name;

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir); // 测试自己的临时目录
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 让本进程（以及它启动的 git、node）用一份空的全局 git 配置，不读系统级配置
fn isolate_git_config() {
    let file = fresh_dir("syncdeck-merge-gitconfig").join("gitconfig");
    std::fs::write(&file, "").unwrap();
    std::env::set_var("GIT_CONFIG_GLOBAL", &file);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
}

fn fixtures(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(name);
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/make-test-repos.mjs");
    assert!(Command::new("node").arg(script).arg(&root).status().unwrap().success());
    root
}

/// 测试里直接读 git 状态用（只读）
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(["-c", "core.quotepath=false"]).args(args).current_dir(dir).output().expect("git");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    head: String,
    remote: String,
    porcelain: String,
    staged: String,
}

fn snapshot(root: &Path, name: &str) -> Snapshot {
    let local = root.join("repos").join(name);
    Snapshot {
        head: git(&local, &["rev-parse", "HEAD"]),
        remote: remote_rev(root, name, "main"),
        porcelain: git(&local, &["status", "--porcelain", "--untracked-files=all"]),
        staged: git(&local, &["diff", "--cached", "--name-only"]),
    }
}

fn remote_rev(root: &Path, name: &str, rev: &str) -> String {
    git(&root.join("remotes").join(format!("{name}.git")), &["rev-parse", rev])
}

fn remote_log(root: &Path, name: &str, format: &str) -> String {
    git(&root.join("remotes").join(format!("{name}.git")), &["log", "-1", &format!("--format={format}"), "main"])
}

fn has_merge_head(dir: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "-q", "--verify", "MERGE_HEAD"])
        .current_dir(dir)
        .status()
        .unwrap()
        .success()
}

type Done = HashMap<String, (SyncItemResult, Project)>;

/// 一键同步，返回每个项目（按名字）的结果和进度事件里的最新状态
async fn run_sync(app: &tauri::App<tauri::test::MockRuntime>, title: &str) -> Done {
    let events: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let sink = events.clone();
    let channel = Channel::new(move |body| {
        if let InvokeResponseBody::Json(s) = body {
            sink.lock().unwrap().push(serde_json::from_str(&s).unwrap());
        }
        Ok(())
    });
    commands::sync_all(app.state(), channel).await.unwrap();
    let done: Done = events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "done")
        .map(|e| {
            let r: serde_json::Value = e["result"].clone();
            let result = SyncItemResult {
                project_id: r["projectId"].as_str().unwrap().to_string(),
                outcome: match r["outcome"].as_str().unwrap() {
                    "pushed" => SyncOutcome::Pushed,
                    "pulled" => SyncOutcome::Pulled,
                    "up-to-date" => SyncOutcome::UpToDate,
                    "committed" => SyncOutcome::Committed,
                    "merged" => SyncOutcome::Merged,
                    "failed" => SyncOutcome::Failed,
                    other => panic!("不认识的 outcome：{other}"),
                },
                commits: r["commits"].as_u64().unwrap() as u32,
                reason: r["reason"].as_str().map(str::to_string),
            };
            let p: Project = serde_json::from_value(e["project"].clone()).unwrap();
            (p.name.clone(), (result, p))
        })
        .collect();
    println!("\n== {title} ==");
    let mut names: Vec<_> = done.keys().collect();
    names.sort();
    for n in names {
        let (r, p) = &done[n];
        println!(
            "  {n:<18} → {:<9} 提交 {} 之后 {:<6} {}",
            format!("{:?}", r.outcome),
            r.commits,
            p.status.label(),
            r.reason.as_deref().map(|s| s.replace('\n', " ⏎ ")).unwrap_or_default()
        );
    }
    done
}

fn patch() -> SettingsPatch {
    SettingsPatch::default()
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_merge_and_precommit_check() {
    isolate_git_config();
    let root = fixtures("syncdeck-merge-test");
    let repo = |name: &str| root.join("repos").join(name);
    let cfg_file = fresh_dir("syncdeck-merge-config").join("config.json");

    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    app.manage::<Shared>(Arc::new(AppState::new(ConfigStore::load(cfg_file.clone()))));

    // ---------------- 设置：默认值、校验、保存 ----------------
    let s = commands::get_settings(app.state()).await.unwrap();
    assert!(s.auto_merge && s.precommit_check, "自动合并、提交前检查默认开");
    assert_eq!(s.big_file_limit_mb, 50);
    for mb in [0, 2049, 100_000] {
        let e = commands::save_settings(app.state(), SettingsPatch { big_file_limit_mb: Some(mb), ..patch() })
            .await
            .unwrap_err();
        println!("  大文件上限 {mb} MB → {e}");
    }
    let s = commands::save_settings(
        app.state(),
        SettingsPatch { auto_merge: Some(false), big_file_limit_mb: Some(1), ..patch() },
    )
    .await
    .unwrap();
    assert!(!s.auto_merge && s.precommit_check);
    assert_eq!(s.big_file_limit_mb, 1);
    let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&cfg_file).unwrap()).unwrap();
    assert_eq!(saved["settings"]["autoMerge"], false);
    assert_eq!(saved["settings"]["bigFileLimitMb"], 1);
    assert!(saved["settings"]["precommitCheck"].is_null(), "默认值不单独保存");

    // ---------------- 添加项目，给两个有改动的项目打开自动提交 ----------------
    let names = ["diverged", "diverged-conflict", "dirty-env", "dirty-bigfile"];
    let added = commands::add_projects(app.state(), names.iter().map(|n| repo(n).to_string_lossy().into()).collect())
        .await
        .unwrap();
    assert_eq!(added.len(), names.len());
    let id_of: HashMap<String, String> = added.iter().map(|p| (p.name.clone(), p.id.clone())).collect();
    for n in ["dirty-env", "dirty-bigfile"] {
        commands::set_auto_commit(app.state(), id_of[n].clone(), true).await.unwrap();
    }
    let before: HashMap<&str, Snapshot> = names.iter().map(|n| (*n, snapshot(&root, n))).collect();

    // ---------------- ⑦ 关掉自动合并：分叉照旧记为异常；⑤⑥ 提交前检查不通过 ----------------
    let done = run_sync(&app, "第一轮：关掉自动合并，大文件上限 1 MB").await;
    for n in names {
        assert_eq!(done[n].0.outcome, SyncOutcome::Failed, "{n} 应记为异常");
        assert_eq!(snapshot(&root, n), before[n], "{n} 的提交、云端、工作区和暂存区都不应改动");
    }
    assert_eq!(
        done["diverged"].0.reason.as_deref(),
        Some("本地和云端各有新提交（领先 1、落后 1），需要先合并"),
        "关掉自动合并时和以前一样"
    );
    assert_eq!(done["diverged"].1.status, SyncStatus::Diverged);
    // ⑤ 没被忽略的 .env：不提交；.env.example 和被 .gitignore 忽略的 local.key 不算
    let reason = done["dirty-env"].0.reason.clone().unwrap();
    assert!(reason.contains("提交前检查没通过") && reason.contains(".env：疑似密钥文件"), "{reason}");
    assert!(!reason.contains(".env.example") && !reason.contains("local.key"), "{reason}");
    assert!(reason.contains(".gitignore"), "应提示加进 .gitignore：{reason}");
    assert_eq!(done["dirty-env"].1.issue.as_deref(), Some(reason.as_str()));
    // ⑥ 2 MB 的文件超过 1 MB 的上限：不提交
    let reason = done["dirty-bigfile"].0.reason.clone().unwrap();
    assert!(reason.contains("assets/video.bin：2.0 MB，超过 1 MB 的上限"), "{reason}");
    assert!(!reason.contains("notes.md"), "普通文件不应列出：{reason}");
    println!("  ✓ 4 个项目都没有被改动");

    // ---------------- ① ② 打开自动合并 ----------------
    let s = commands::save_settings(app.state(), SettingsPatch { auto_merge: Some(true), ..patch() }).await.unwrap();
    assert!(s.auto_merge);
    let done = run_sync(&app, "第二轮：打开自动合并").await;

    // ① 分叉、改的不同文件 → 已合并并推送；云端最新提交是合并提交；状态已同步
    let (r, p) = &done["diverged"];
    assert_eq!(r.outcome, SyncOutcome::Merged, "diverged 应自动合并：{:?}", r.reason);
    assert_eq!(r.commits, 2, "推送本地的 1 个提交和 1 个合并提交");
    let local = repo("diverged");
    let head = git(&local, &["rev-parse", "HEAD"]);
    assert_eq!(remote_rev(&root, "diverged", "main"), head, "合并结果应已推送");
    let parents: Vec<String> = remote_log(&root, "diverged", "%P").split(' ').map(str::to_string).collect();
    assert_eq!(parents, [before["diverged"].head.clone(), before["diverged"].remote.clone()], "云端最新提交是合并提交");
    assert_eq!(remote_log(&root, "diverged", "%s"), ops::merge_message(&computer_name()));
    println!("  diverged 云端最新提交：{}", remote_log(&root, "diverged", "%s"));
    assert!(local.join("desktop.md").exists() && local.join("laptop.md").exists(), "两边的改动都在");
    assert_eq!(git(&local, &["status", "--porcelain"]), "");
    assert_eq!(p.status, SyncStatus::Synced);
    assert!(p.issue.is_none() && p.last_sync_at.is_some());

    // ② 分叉、改了同一处 → 记为异常，原因含冲突文件名；本地和云端提交都没变，工作区干净，没有 MERGE_HEAD
    let (r, p) = &done["diverged-conflict"];
    assert_eq!(r.outcome, SyncOutcome::Failed);
    let reason = r.reason.clone().unwrap();
    assert_eq!(
        reason,
        "本地和云端改了同一处，自动合并会冲突：README.md。没有改动任何文件，可在详情里「复制给 AI」"
    );
    let local = repo("diverged-conflict");
    assert_eq!(snapshot(&root, "diverged-conflict"), before["diverged-conflict"]);
    assert_eq!(git(&local, &["status", "--porcelain"]), "", "工作区应干净");
    assert!(!has_merge_head(&local), "不应留下进行中的合并");
    assert!(std::fs::read_to_string(local.join("README.md")).unwrap().contains("台式机上改了这一行"));
    assert_eq!((p.status, p.issue.as_deref()), (SyncStatus::Diverged, Some(reason.as_str())));

    // 提交前检查没通过的仍然没动
    for n in ["dirty-env", "dirty-bigfile"] {
        assert_eq!(done[n].0.outcome, SyncOutcome::Failed);
        assert_eq!(snapshot(&root, n), before[n]);
    }

    // ---------------- 诊断信息：冲突文件、两边最近的提交 ----------------
    let diag = commands::get_diagnostic_info(app.state(), id_of["diverged-conflict"].clone()).await.unwrap();
    println!(
        "  诊断 diverged-conflict：冲突 {:?}，本地 {:?}，云端 {:?}",
        diag.conflict_files,
        diag.recent_commits.iter().map(|c| &c.subject).collect::<Vec<_>>(),
        diag.recent_remote_commits.iter().map(|c| &c.subject).collect::<Vec<_>>()
    );
    assert_eq!(diag.conflict_files, ["README.md"]);
    assert_eq!(diag.recent_commits[0].subject, "台式机上改了 README");
    assert_eq!(diag.recent_remote_commits[0].subject, "笔记本上改了 README");
    assert_eq!(diag.recent_remote_commits.len(), 2);
    assert!(!has_merge_head(&local) && git(&local, &["status", "--porcelain"]).is_empty(), "预演不碰工作区");
    let diag = commands::get_diagnostic_info(app.state(), id_of["diverged"].clone()).await.unwrap();
    assert!(diag.conflict_files.is_empty(), "不是分叉时没有冲突文件");
    assert_eq!(diag.recent_remote_commits[0].subject, ops::merge_message(&computer_name()));
    let json = serde_json::to_value(&diag).unwrap();
    assert!(json["conflictFiles"].is_array() && json["recentRemoteCommits"].is_array(), "字段名是 camelCase");

    // ---------------- 上限改回 50 MB：大文件可以提交了，.env 仍然拦着 ----------------
    commands::save_settings(app.state(), SettingsPatch { big_file_limit_mb: Some(50), ..patch() }).await.unwrap();
    let done = run_sync(&app, "第三轮：大文件上限 50 MB").await;
    assert_eq!(done["dirty-bigfile"].0.outcome, SyncOutcome::Committed, "{:?}", done["dirty-bigfile"].0.reason);
    let files = git(&root.join("remotes").join("dirty-bigfile.git"), &["ls-tree", "-r", "--name-only", "main"]);
    assert!(files.lines().any(|l| l == "assets/video.bin"), "{files}");
    assert_eq!(done["dirty-env"].0.outcome, SyncOutcome::Failed);
    assert_eq!(snapshot(&root, "dirty-env"), before["dirty-env"]);

    // ---------------- 关掉提交前检查：.env 也照常提交（被忽略的 local.key 仍然不会提交）----------------
    let s = commands::save_settings(app.state(), SettingsPatch { precommit_check: Some(false), ..patch() })
        .await
        .unwrap();
    assert!(!s.precommit_check);
    let done = run_sync(&app, "第四轮：关掉提交前检查").await;
    assert_eq!(done["dirty-env"].0.outcome, SyncOutcome::Committed, "{:?}", done["dirty-env"].0.reason);
    let files = git(&root.join("remotes").join("dirty-env.git"), &["ls-tree", "-r", "--name-only", "main"]);
    assert!(files.lines().any(|l| l == ".env") && !files.lines().any(|l| l == "local.key"), "{files}");

    // 模拟重启：设置都在
    let reloaded = ConfigStore::load(cfg_file);
    let (auto_merge, precommit, limit) = reloaded
        .read(|c| (c.settings.auto_merge, c.settings.precommit_check, c.settings.big_file_limit_mb));
    assert_eq!((auto_merge, precommit, limit), (None, Some(false), None), "等于默认值的不保存");
}
