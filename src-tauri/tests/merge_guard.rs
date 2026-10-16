//! 自动合并的两层防护：
//! 1. 合并前：分支配置了 `branch.<分支>.mergeoptions`（git merge 会把它加进命令行，白名单管不到）就不自动合并；
//! 2. 合并后、推送前：核对没有 MERGE_HEAD、HEAD^2 是预演用的云端提交、HEAD 的树就是预演算出的树，不一致就不推送。
//! 只在 %TEMP% 里自建的临时仓库上运行；用 GIT_CONFIG_GLOBAL / GIT_CONFIG_NOSYSTEM 把 git 的全局配置换成空的临时文件。
//! 这个文件里只有一个测试，因为它要改本进程的环境变量。
//! 运行：cd src-tauri && cargo test --test merge_guard -- --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use git_syncdeck_lib::model::SyncOutcome;
use git_syncdeck_lib::ops::{self, MergePreview, SyncOptions};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "init.defaultBranch=main", "-c", "user.name=test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?} 失败：{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str) {
    std::fs::write(dir.join(file), format!("{file}\n")).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", file]);
}

fn has_merge_head(dir: &Path) -> bool {
    Command::new("git").args(["rev-parse", "-q", "--verify", "MERGE_HEAD"]).current_dir(dir).status().unwrap().success()
}

/// 一个分叉的项目：本地多了 desktop.txt，云端多了 laptop.txt（改的不同文件，预演没有冲突）。返回 (本地, 云端裸仓库)
fn diverged(root: &Path, name: &str) -> (PathBuf, PathBuf) {
    let base = root.join(name);
    std::fs::create_dir_all(&base).unwrap();
    let remote = base.join("remote.git");
    let (local, other) = (base.join("local"), base.join("other"));
    git(&base, &["init", "-q", "--bare", remote.to_str().unwrap()]);
    git(&base, &["clone", "-q", remote.to_str().unwrap(), "other"]);
    commit(&other, "README.md");
    git(&other, &["push", "-q", "origin", "HEAD:main"]);
    git(&base, &["clone", "-q", remote.to_str().unwrap(), "local"]);
    // SyncDeck 在本地合并时要用到提交身份（全局配置已换成空的）
    git(&local, &["config", "user.name", "test"]);
    git(&local, &["config", "user.email", "test@example.com"]);
    commit(&local, "desktop.txt");
    commit(&other, "laptop.txt");
    git(&other, &["push", "-q", "origin", "HEAD:main"]);
    (local, remote)
}

fn options() -> SyncOptions {
    SyncOptions { auto_merge: true, merge_message: ops::merge_message("测试电脑"), ..Default::default() }
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_guards() {
    let root = std::env::temp_dir().join("syncdeck-merge-guard");
    let _ = std::fs::remove_dir_all(&root); // 测试自己的临时目录
    std::fs::create_dir_all(&root).unwrap();
    let gitconfig = root.join("gitconfig");
    std::fs::write(&gitconfig, "").unwrap();
    std::env::set_var("GIT_CONFIG_GLOBAL", &gitconfig);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");

    // 对照：没有任何配置时正常合并并推送，云端两边的改动都在
    let (local, remote) = diverged(&root, "plain");
    let r = ops::sync_one(&local, &options()).await;
    assert_eq!(r.outcome, SyncOutcome::Merged, "{:?}", r.reason);
    let files = git(&remote, &["ls-tree", "-r", "--name-only", "main"]);
    assert!(files.contains("desktop.txt") && files.contains("laptop.txt"), "{files}");
    println!("对照：已合并并推送，云端文件 {:?}", files.lines().collect::<Vec<_>>());

    // ---------------- 第一层：分支配置了 mergeoptions 就不自动合并 ----------------
    for opt in ["-s ours", "--no-commit", "--squash"] {
        let (local, remote) = diverged(&root, &format!("opt{}", opt.replace([' ', '-'], "")));
        git(&local, &["config", "branch.main.mergeoptions", opt]);
        let (head, cloud) = (git(&local, &["rev-parse", "HEAD"]), git(&remote, &["rev-parse", "main"]));
        let r = ops::sync_one(&local, &options()).await;
        let reason = r.reason.clone().unwrap_or_default();
        println!("mergeoptions = {opt:<12} → {:?}：{reason}", r.outcome);
        assert_eq!(r.outcome, SyncOutcome::Failed);
        assert!(reason.contains(&format!("branch.main.mergeoptions（{opt}）")), "{reason}");
        assert_eq!(git(&local, &["rev-parse", "HEAD"]), head, "本地不应合并");
        assert_eq!(git(&remote, &["rev-parse", "main"]), cloud, "不应推送");
        assert!(!has_merge_head(&local), "不应留下 MERGE_HEAD");
        assert_eq!(git(&local, &["status", "--porcelain"]), "", "工作区和暂存区不应改动");
    }

    // ---------------- 第二层：合并结果和预演不一致就不推送 ----------------
    // 用 post-merge 钩子在 git merge 返回之前改掉合并结果（git merge 退出码仍是 0）。
    // 钩子运行时 MERGE_HEAD 还在，commit --amend 会被拒，所以用 commit-tree + update-ref 直接换掉 HEAD：
    // - amend：父提交不变（本地、云端），但多了一个文件 → 树和预演的不一样
    // - extra：在合并提交后面又接了一个普通提交 → HEAD 不再是那个合并提交
    const PREFIX: &str = "#!/bin/sh\nset -e\necho hook > hook.txt\ngit add hook.txt\ntree=$(git write-tree)\n";
    for (name, tail, want) in [
        (
            "hook-amend",
            "new=$(git commit-tree \"$tree\" -p HEAD^1 -p HEAD^2 -m amended)\ngit update-ref HEAD \"$new\"\n",
            "合并后的文件和预演算出的不一样",
        ),
        (
            "hook-extra",
            "new=$(git commit-tree \"$tree\" -p HEAD -m extra)\ngit update-ref HEAD \"$new\"\n",
            "合并提交的第一个父提交不是本地原来的提交",
        ),
    ] {
        let script = format!("{PREFIX}{tail}");
        let (local, remote) = diverged(&root, name);
        std::fs::write(local.join(".git").join("hooks").join("post-merge"), script).unwrap();
        let cloud = git(&remote, &["rev-parse", "main"]);
        let r = ops::sync_one(&local, &options()).await;
        let reason = r.reason.clone().unwrap_or_default();
        println!("{name} → {:?}：{reason}", r.outcome);
        assert_eq!(r.outcome, SyncOutcome::Failed);
        assert!(reason.starts_with(&format!("已在本地合并，但结果和预演不一致，没有推送：{want}")), "{reason}");
        assert_eq!(git(&remote, &["rev-parse", "main"]), cloud, "不一致时不应推送");
        assert!(git(&local, &["ls-files"]).lines().any(|f| f == "hook.txt"), "钩子确实改了合并结果（测的是第二层）");
        assert!(!has_merge_head(&local));
    }

    // 直接核对：手动 merge --no-commit（停在合并到一半）、merge -s ours（丢掉云端改动）都要判为不一致
    let (local, _) = diverged(&root, "verify");
    let ours = git(&local, &["rev-parse", "HEAD"]);
    git(&local, &["fetch", "-q", "origin"]);
    let theirs = git(&local, &["rev-parse", "origin/main"]);
    let Ok(MergePreview::Clean { tree }) = ops::preview_merge(&local, &ours, &theirs).await else {
        panic!("预演应当没有冲突");
    };
    git(&local, &["merge", "--no-commit", "--no-edit", &theirs]);
    let e = ops::verify_merge(&local, &ours, &theirs, &tree).await.unwrap_err();
    println!("手动 merge --no-commit → {e}");
    assert!(e.contains("MERGE_HEAD"), "{e}");
    git(&local, &["merge", "--abort"]); // 测试自己的临时仓库
    git(&local, &["merge", "-s", "ours", "--no-edit", "-m", "x", &theirs]);
    let e = ops::verify_merge(&local, &ours, &theirs, &tree).await.unwrap_err();
    println!("手动 merge -s ours → {e}");
    assert!(e.contains("预演"), "{e}");
    // 正常合并的结果核对通过
    let (local, _) = diverged(&root, "verify-ok");
    let ours = git(&local, &["rev-parse", "HEAD"]);
    git(&local, &["fetch", "-q", "origin"]);
    let theirs = git(&local, &["rev-parse", "origin/main"]);
    let Ok(MergePreview::Clean { tree }) = ops::preview_merge(&local, &ours, &theirs).await else {
        panic!("预演应当没有冲突");
    };
    git(&local, &["merge", "--no-edit", "-m", "x", &theirs]);
    assert_eq!(ops::verify_merge(&local, &ours, &theirs, &tree).await, Ok(()));
}
