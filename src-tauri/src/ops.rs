//! 针对单个仓库的 git 操作。只做 fetch、push（快进）、pull --ff-only、以及用户确认后的 add/commit。
//! 不使用 --force、reset、rebase、stash、clean、checkout 等任何可能改写历史或丢失改动的命令。

use std::path::Path;

use crate::git::{run_git, GitError, NETWORK_TIMEOUT, STATUS_TIMEOUT};
use crate::model::{CommitInfo, SyncOutcome, SyncStatus};
use crate::status::{inspect, issue_text, no_remote_reason, read_config, Inspection, DETACHED};
use crate::url::sanitize_url;

pub async fn fetch(dir: &Path, ins: &Inspection) -> Result<(), GitError> {
    let remote = ins.remote_name.as_deref().unwrap_or("origin");
    run_git(dir, &["fetch", "--quiet", remote], NETWORK_TIMEOUT).await.map(|_| ())
}

/// 推送当前分支到它的上游（显式写出 refspec，不加 +，远程拒绝时不会强推）
pub async fn push(dir: &Path, ins: &Inspection) -> Result<(), GitError> {
    let remote = ins.remote_name.as_deref().unwrap_or("origin");
    let target = ins.upstream_branch.as_deref().unwrap_or(&ins.branch);
    let refspec = format!("refs/heads/{}:refs/heads/{}", ins.branch, target);
    run_git(dir, &["push", "--quiet", remote, &refspec], NETWORK_TIMEOUT).await.map(|_| ())
}

/// 只快进拉取。--no-rebase / --no-autostash 防止全局配置（pull.rebase、autostash）让它变成变基或自动 stash
pub async fn pull_ff_only(dir: &Path, ins: &Inspection) -> Result<(), GitError> {
    let remote = ins.remote_name.as_deref().unwrap_or("origin");
    let target = ins.upstream_branch.as_deref().unwrap_or(&ins.branch);
    run_git(
        dir,
        &["pull", "--ff-only", "--no-rebase", "--no-autostash", "--quiet", remote, target],
        NETWORK_TIMEOUT,
    )
    .await
    .map(|_| ())
}

/// 联网检查的结果
pub enum Checked {
    /// fetch 成功（或不需要联网）后的最新状态
    Fresh(Inspection),
    /// 本地检查成功，但 fetch 失败
    FetchFailed { local: Inspection, error: GitError },
    /// 连本地检查都失败了（目录不在、不是仓库、git 未安装……）
    Failed(GitError),
}

/// 刷新状态：先 fetch，再 git status。没有上游的项目不需要联网。
pub async fn check(dir: &Path) -> Checked {
    let local = match inspect(dir).await {
        Ok(i) => i,
        Err(e) => return Checked::Failed(e),
    };
    if !local.can_fetch() {
        return Checked::Fresh(local);
    }
    if let Err(error) = fetch(dir, &local).await {
        return Checked::FetchFailed { local, error };
    }
    match inspect(dir).await {
        Ok(i) => Checked::Fresh(i),
        Err(error) => Checked::FetchFailed { local, error },
    }
}

/// 一键同步中单个项目的结果
pub struct SyncOne {
    pub outcome: SyncOutcome,
    pub commits: u32,
    pub reason: Option<String>,
    /// 处理后的最新状态；检查失败时为检查前的本地状态，或 None
    pub inspection: Option<Inspection>,
    /// 是否在"检查"这一步就失败了（此时界面保留上一次的状态并显示"检查失败"）
    pub check_failed: bool,
}

impl SyncOne {
    fn failed(reason: String, inspection: Option<Inspection>, check_failed: bool) -> Self {
        SyncOne { outcome: SyncOutcome::Failed, commits: 0, reason: Some(reason), inspection, check_failed }
    }
    fn ok(outcome: SyncOutcome, commits: u32, inspection: Inspection) -> Self {
        SyncOne { outcome, commits, reason: None, inspection: Some(inspection), check_failed: false }
    }
}

/// 一键同步单个项目：先检查状态，再按状态处理
/// - 待推送 → push；待拉取 → pull --ff-only；已同步 → 跳过
/// - 有未提交改动 / 分叉 / 未关联云端 → 不做任何操作，记为异常
pub async fn sync_one(dir: &Path) -> SyncOne {
    let ins = match check(dir).await {
        Checked::Fresh(i) => i,
        Checked::FetchFailed { local, error } => return SyncOne::failed(error.message(), Some(local), true),
        Checked::Failed(error) => return SyncOne::failed(error.message(), None, true),
    };

    match ins.status {
        SyncStatus::Ahead => match push(dir, &ins).await {
            Ok(()) => SyncOne::ok(SyncOutcome::Pushed, ins.ahead, reinspect(dir, ins.clone()).await),
            Err(e) => SyncOne::failed(e.message(), Some(ins), false),
        },
        SyncStatus::Behind => match pull_ff_only(dir, &ins).await {
            Ok(()) => SyncOne::ok(SyncOutcome::Pulled, ins.behind, reinspect(dir, ins.clone()).await),
            Err(e) => SyncOne::failed(e.message(), Some(ins), false),
        },
        SyncStatus::Synced => SyncOne::ok(SyncOutcome::UpToDate, 0, ins),
        _ => {
            let reason = issue_text(&ins);
            SyncOne::failed(reason, Some(ins), false)
        }
    }
}

/// 操作成功后重新读取状态；万一读取失败，就用操作前的结果
async fn reinspect(dir: &Path, fallback: Inspection) -> Inspection {
    inspect(dir).await.unwrap_or(fallback)
}

/// "直接提交并推送"：git add -A → git commit -m → fetch → 若落后于云端则停止并报告（不自动合并）→ git push
pub async fn commit_and_push(dir: &Path, message: &str) -> Result<Inspection, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("提交说明不能为空".into());
    }
    let ins = inspect(dir).await.map_err(|e| e.message())?;
    match ins.status {
        SyncStatus::Dirty => {}
        SyncStatus::NoRemote => return Err(no_remote_reason(&ins)),
        _ => return Err("这个项目没有需要提交的改动".into()),
    }

    run_git(dir, &["add", "-A"], NETWORK_TIMEOUT)
        .await
        .map_err(|e| format!("暂存改动失败：{}", e.message()))?;
    run_git(dir, &["commit", "--quiet", "-m", message], NETWORK_TIMEOUT)
        .await
        .map_err(|e| format!("提交失败：{}", e.message()))?;

    fetch(dir, &ins)
        .await
        .map_err(|e| format!("已提交到本地，但检查云端失败，没有推送：{}", e.message()))?;
    let after = inspect(dir)
        .await
        .map_err(|e| format!("已提交到本地，但读取状态失败，没有推送：{}", e.message()))?;
    if after.behind > 0 {
        return Err(format!(
            "已提交到本地，但云端有 {} 个新提交，已停止推送（不会自动合并）。项目现在处于分叉状态，请手动处理。",
            after.behind
        ));
    }

    push(dir, &after)
        .await
        .map_err(|e| format!("已提交到本地，但推送失败：{}", e.message()))?;
    Ok(reinspect(dir, after).await)
}

/// 最近 3 次提交（只取哈希、时间、标题，不含任何文件内容）
pub async fn recent_commits(dir: &Path) -> Vec<CommitInfo> {
    let args = ["log", "-3", "--format=%h%x1f%ad%x1f%s", "--date=format:%Y-%m-%d %H:%M"];
    let Ok(out) = run_git(dir, &args, STATUS_TIMEOUT).await else {
        return Vec::new(); // 还没有任何提交
    };
    out.stdout
        .lines()
        .filter_map(|line| {
            let mut f = line.splitn(3, '\u{1f}');
            Some(CommitInfo {
                hash: f.next()?.to_string(),
                date: f.next()?.to_string(),
                subject: f.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// 扫描结果里每个仓库的分支名和远程地址
pub async fn repo_brief(dir: &Path) -> (String, Option<String>) {
    let branch = match run_git(dir, &["symbolic-ref", "--short", "-q", "HEAD"], STATUS_TIMEOUT).await {
        Ok(out) => out.stdout.trim().to_string(),
        Err(GitError::Failed { code: Some(1), .. }) => DETACHED.to_string(),
        Err(_) => "—".to_string(),
    };
    let remote = read_config(dir).await.ok().and_then(|c| {
        let origin = c.remotes.iter().find(|(n, _)| n == "origin").or(c.remotes.first());
        origin.map(|(_, u)| sanitize_url(u))
    });
    (branch, remote)
}

/// 检测 git 是否已安装，返回版本号
pub async fn git_version() -> Result<String, GitError> {
    let out = run_git(&std::env::temp_dir(), &["--version"], STATUS_TIMEOUT).await?;
    Ok(out.stdout.trim().to_string())
}
