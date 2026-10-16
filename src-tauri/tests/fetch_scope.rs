//! fetch 只拉上游分支：其他分支的更新不下载；云端分支被删后退回 fetch 整个远程，不报错。
//! 只在 %TEMP% 里自建的临时仓库上运行。
//! 运行：cd src-tauri && cargo test --test fetch_scope -- --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;

use git_syncdeck_lib::ops;
use git_syncdeck_lib::status::inspect;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=test", "-c", "user.email=test@example.com", "-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?} 失败：{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str) {
    std::fs::write(dir.join(file), file).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", file]);
}

fn has_ref(dir: &Path, r: &str) -> bool {
    Command::new("git").args(["rev-parse", "--verify", "-q", r]).current_dir(dir).status().unwrap().success()
}

#[tokio::test]
async fn fetch_only_upstream_branch() {
    let root: PathBuf = std::env::temp_dir().join("syncdeck-fetch-scope");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let remote = root.join("remote.git");
    let (local, other) = (root.join("local"), root.join("other"));

    git(&root, &["init", "-q", "--bare", remote.to_str().unwrap()]);
    git(&root, &["clone", "-q", remote.to_str().unwrap(), "other"]);
    commit(&other, "a.txt");
    git(&other, &["push", "-q", "origin", "HEAD:main"]);
    git(&root, &["clone", "-q", remote.to_str().unwrap(), "local"]);

    // 云端 main 和另一个分支 feature 都有新提交
    commit(&other, "b.txt");
    git(&other, &["push", "-q", "origin", "HEAD:main", "HEAD:feature"]);
    let remote_main = git(&other, &["rev-parse", "HEAD"]);

    let ins = inspect(&local).await.unwrap();
    ops::fetch(&local, &ins).await.expect("fetch 上游分支应成功");
    assert_eq!(git(&local, &["rev-parse", "origin/main"]), remote_main, "上游分支应更新");
    assert!(!has_ref(&local, "refs/remotes/origin/feature"), "其他分支不应被下载");

    // 云端 main 被删：按分支 fetch 失败后退回 fetch 整个远程，不报错
    git(&remote, &["symbolic-ref", "HEAD", "refs/heads/feature"]); // 裸仓库不允许删除当前分支
    git(&other, &["push", "-q", "origin", "--delete", "main"]);
    let ins = inspect(&local).await.unwrap();
    ops::fetch(&local, &ins).await.expect("云端分支被删时应退回 fetch 整个远程");
    assert!(has_ref(&local, "refs/remotes/origin/feature"), "退回后应拉到其他分支");
}
