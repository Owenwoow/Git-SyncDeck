//! 用 scripts/make-test-repos.mjs 生成的临时仓库验证：状态检测、一键同步、提交并推送，以及安全红线。
//! 运行：cd src-tauri && cargo test --test repo_states -- --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use git_syncdeck_lib::model::{ChangeKind, SyncOutcome, SyncStatus};
use git_syncdeck_lib::ops::{self, Checked};
use git_syncdeck_lib::status::inspect;

/// 每次都重新生成一套干净的测试仓库（与手动体验用的 %TEMP%\syncdeck-test 分开）
fn fixtures() -> PathBuf {
    let root = std::env::temp_dir().join("syncdeck-cargo-test");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/make-test-repos.mjs");
    let status = Command::new("node").arg(script).arg(&root).status().expect("需要 node 来生成测试仓库");
    assert!(status.success(), "生成测试仓库失败");
    root
}

/// 测试里直接读 git 状态用（只读）
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).output().expect("git");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn head(dir: &Path) -> String {
    git(dir, &["rev-parse", "HEAD"])
}

fn remote_main(root: &Path, name: &str) -> String {
    git(&root.join("remotes").join(format!("{name}.git")), &["rev-parse", "main"])
}

#[tokio::test(flavor = "multi_thread")]
async fn states_sync_and_commit() {
    let root = fixtures();
    let repo = |name: &str| root.join("repos").join(name);

    // ---------------- 1. 状态检测（fetch + status）----------------
    println!("\n== 状态检测（刷新状态：fetch + status）==");
    let expected = [
        ("synced", SyncStatus::Synced),
        ("ahead", SyncStatus::Ahead),
        ("behind", SyncStatus::Behind),
        ("dirty", SyncStatus::Dirty),
        ("diverged", SyncStatus::Diverged),
        ("no-remote", SyncStatus::NoRemote),
        ("no-upstream", SyncStatus::NoRemote),
        ("detached", SyncStatus::NoRemote),
        ("dirty-behind", SyncStatus::Dirty),
    ];
    for (name, want) in expected {
        match ops::check(&repo(name)).await {
            Checked::Fresh(ins) => {
                println!(
                    "  {name:<13} → {:<6} 分支 {:<22} 领先 {} 落后 {} 改动 {}",
                    ins.status.label(),
                    ins.branch,
                    ins.ahead,
                    ins.behind,
                    ins.changes.len()
                );
                assert_eq!(ins.status, want, "{name} 的状态不对");
            }
            Checked::FetchFailed { error, .. } => panic!("{name} 不应该检查失败：{error}"),
            Checked::Failed(error) => panic!("{name} 不应该检查失败：{error}"),
        }
    }
    match ops::check(&repo("fetch-fail")).await {
        Checked::FetchFailed { local, error } => {
            println!("  {:<13} → 检查失败（保留上次状态：{}）：{}", "fetch-fail", local.status.label(), first_line(&error.message()));
            assert!(!error.message().is_empty());
        }
        _ => panic!("fetch-fail 应该检查失败"),
    }

    // 中文文件名、已暂存 / 未暂存 / 未跟踪分组
    let dirty = inspect(&repo("dirty")).await.unwrap();
    let names: Vec<_> = dirty.changes.iter().map(|c| (c.path.as_str(), c.kind, c.staged)).collect();
    println!("  dirty 的改动：{names:?}");
    assert!(names.contains(&("README.md", ChangeKind::Modified, false)));
    assert!(names.contains(&("src/新功能.ts", ChangeKind::Added, true)));
    assert!(names.contains(&("中文 笔记.md", ChangeKind::Untracked, false)));

    // ---------------- 2. 一键同步 ----------------
    println!("\n== 一键同步 ==");
    let untouchable = ["dirty", "diverged", "no-remote", "no-upstream", "detached", "fetch-fail", "dirty-behind"];
    let before: Vec<(&str, String, String)> = untouchable
        .iter()
        .map(|n| (*n, head(&repo(n)), git(&repo(n), &["status", "--porcelain"])))
        .collect();

    let cases = [
        ("synced", SyncOutcome::UpToDate),
        ("ahead", SyncOutcome::Pushed),
        ("behind", SyncOutcome::Pulled),
        ("dirty", SyncOutcome::Failed),
        ("diverged", SyncOutcome::Failed),
        ("no-remote", SyncOutcome::Failed),
        ("no-upstream", SyncOutcome::Failed),
        ("detached", SyncOutcome::Failed),
        ("fetch-fail", SyncOutcome::Failed),
        ("dirty-behind", SyncOutcome::Failed),
    ];
    for (name, want) in cases {
        let r = ops::sync_one(&repo(name)).await;
        let after = r.inspection.as_ref().map_or("—", |i| i.status.label());
        println!(
            "  {name:<13} → {:<10} {}{}",
            format!("{:?}", r.outcome),
            if r.outcome == SyncOutcome::Failed { "原因：" } else { "之后：" },
            r.reason.as_deref().map(first_line).unwrap_or_else(|| after.to_string())
        );
        assert_eq!(r.outcome, want, "{name} 的同步结果不对");
    }

    assert_eq!(remote_main(&root, "ahead"), head(&repo("ahead")), "ahead 应该已推送到云端");
    assert_eq!(remote_main(&root, "behind"), head(&repo("behind")), "behind 应该已快进到云端最新");
    for name in ["ahead", "behind", "synced"] {
        assert_eq!(inspect(&repo(name)).await.unwrap().status, SyncStatus::Synced, "{name} 同步后应为已同步");
    }

    // 红线：异常项目的提交和工作区完全没被动过
    for (name, h, st) in &before {
        assert_eq!(&head(&repo(name)), h, "{name} 的 HEAD 被改动了");
        assert_eq!(&git(&repo(name), &["status", "--porcelain"]), st, "{name} 的工作区被改动了");
    }
    println!("  ✓ 7 个异常项目的 HEAD 和工作区均未改动");

    // ---------------- 3. 提交并推送 ----------------
    println!("\n== 提交并推送 ==");
    let ins = ops::commit_and_push(&repo("dirty"), "测试：提交全部改动").await.expect("dirty 应该能提交并推送");
    println!("  dirty         → 成功，之后：{}", ins.status.label());
    assert_eq!(ins.status, SyncStatus::Synced);
    assert_eq!(remote_main(&root, "dirty"), head(&repo("dirty")));
    assert_eq!(git(&repo("dirty"), &["log", "-1", "--format=%s"]), "测试：提交全部改动");

    let remote_before = remote_main(&root, "dirty-behind");
    let head_before = head(&repo("dirty-behind"));
    let err = ops::commit_and_push(&repo("dirty-behind"), "测试：落后时提交").await.unwrap_err();
    println!("  dirty-behind  → 停止：{err}");
    assert!(err.contains("已停止推送"));
    assert_eq!(remote_main(&root, "dirty-behind"), remote_before, "落后时不应该推送");
    // 新提交的父提交就是原来的 HEAD：没有合并、没有变基
    assert_eq!(git(&repo("dirty-behind"), &["rev-parse", "HEAD~1"]), head_before);
    assert_eq!(git(&repo("dirty-behind"), &["rev-list", "--parents", "-n", "1", "HEAD"]).split(' ').count(), 2);
    let after = inspect(&repo("dirty-behind")).await.unwrap();
    assert_eq!(after.status, SyncStatus::Diverged);
    println!("  dirty-behind  → 现在是 {}（领先 {}、落后 {}），云端未改动，没有自动合并", after.status.label(), after.ahead, after.behind);

    assert!(ops::commit_and_push(&repo("diverged"), "x").await.is_err(), "分叉项目不能提交并推送");
    assert!(ops::commit_and_push(&repo("dirty-behind"), "   ").await.is_err(), "空说明应被拒绝");
    println!("  ✓ 分叉项目、空提交说明均被拒绝");
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").to_string()
}
