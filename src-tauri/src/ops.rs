//! 针对单个仓库的 git 操作。只做 fetch、push（快进）、pull --ff-only、以及用户确认后的 add/commit。
//! 不使用 --force、reset、rebase、stash、clean、checkout 等任何可能改写历史或丢失改动的命令。

use std::path::Path;
use std::time::Duration;

use crate::git::{network_timeout, proxy_from_env, run_git, GitError, STATUS_TIMEOUT};
use crate::model::{CommitInfo, SyncOutcome, SyncStatus};
use crate::status::{inspect, issue_text, no_remote_reason, read_config, Inspection, DETACHED};
use crate::url::sanitize_url;

/// 只 fetch 上游分支（下载量和 pull 一样，分支、标签多的仓库不会因为下载其他分支而超时）。
/// 上游分支在本地已没有远程跟踪分支，或者按分支 fetch 失败（如云端分支已删除）时，退回 fetch 整个远程；
/// 超时不退回，免得再等一轮。`timeout` 是每次 fetch 的超时（刷新用 `refresh_timeout()`，其余用 `network_timeout()`）。
pub async fn fetch(dir: &Path, ins: &Inspection, timeout: Duration) -> Result<(), GitError> {
    let remote = ins.remote_name.as_deref().unwrap_or("origin");
    if let (Some(branch), false) = (ins.upstream_branch.as_deref(), ins.upstream_gone) {
        match run_git(dir, &["fetch", "--quiet", remote, branch], timeout).await {
            Ok(_) => return Ok(()),
            Err(e @ GitError::Timeout { .. }) => return Err(e),
            Err(_) => {}
        }
    }
    run_git(dir, &["fetch", "--quiet", remote], timeout).await.map(|_| ())
}

/// 推送当前分支到它的上游（显式写出 refspec，不加 +，远程拒绝时不会强推）
pub async fn push(dir: &Path, ins: &Inspection) -> Result<(), GitError> {
    let remote = ins.remote_name.as_deref().unwrap_or("origin");
    let target = ins.upstream_branch.as_deref().unwrap_or(&ins.branch);
    let refspec = format!("refs/heads/{}:refs/heads/{}", ins.branch, target);
    run_git(dir, &["push", "--quiet", remote, &refspec], network_timeout()).await.map(|_| ())
}

/// 只快进拉取。--no-rebase / --no-autostash 防止全局配置（pull.rebase、autostash）让它变成变基或自动 stash
pub async fn pull_ff_only(dir: &Path, ins: &Inspection) -> Result<(), GitError> {
    let remote = ins.remote_name.as_deref().unwrap_or("origin");
    let target = ins.upstream_branch.as_deref().unwrap_or(&ins.branch);
    run_git(
        dir,
        &["pull", "--ff-only", "--no-rebase", "--no-autostash", "--quiet", remote, target],
        network_timeout(),
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

/// 联网检查：先 fetch（超时为 `fetch_timeout`），再 git status。没有上游的项目不需要联网。
/// 刷新状态时传 `refresh_timeout()`，一键同步时传 `network_timeout()`。
pub async fn check(dir: &Path, fetch_timeout: Duration) -> Checked {
    let local = match inspect(dir).await {
        Ok(i) => i,
        Err(e) => return Checked::Failed(e),
    };
    if !local.can_fetch() {
        return Checked::Fresh(local);
    }
    if let Err(error) = fetch(dir, &local, fetch_timeout).await {
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

/// 一键同步单个项目时的选项。只由 `commands::sync_options_for` 构造，以后加选项也只改那里。
#[derive(Debug, Clone, Default)]
pub struct SyncOptions {
    /// 有未提交改动时自动提交用的提交信息；None 表示不自动提交
    pub auto_commit: Option<String>,
    /// 跳过 fetch，直接用本地的远程跟踪分支判断（刚联网检查过时，复用那次的结果）
    pub skip_fetch: bool,
}

/// 一键同步单个项目：先检查状态，再按状态处理
/// - 待推送 → push；待拉取 → pull --ff-only；已同步 → 跳过
/// - 有未提交改动：打开了自动提交（`opts.auto_commit` 为提交信息）→ 自动提交并推送，见 `auto_commit_and_push`；
///   没打开 → 不做任何操作，记为异常
/// - 分叉 / 未关联云端 → 不做任何操作，记为异常
pub async fn sync_one(dir: &Path, opts: &SyncOptions) -> SyncOne {
    let checked = if opts.skip_fetch {
        match inspect(dir).await {
            Ok(i) => Checked::Fresh(i),
            Err(e) => Checked::Failed(e),
        }
    } else {
        check(dir, network_timeout()).await
    };
    let ins = match checked {
        Checked::Fresh(i) => i,
        Checked::FetchFailed { local, error } => return SyncOne::failed(error.message(), Some(local), true),
        Checked::Failed(error) => return SyncOne::failed(error.message(), None, true),
    };
    let auto_commit = opts.auto_commit.as_deref();

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
        // 有未提交改动意味着已有上游（"未关联云端"优先级更高）
        SyncStatus::Dirty if auto_commit.is_some() => {
            auto_commit_and_push(dir, ins, auto_commit.unwrap_or("")).await
        }
        _ => {
            let reason = issue_text(&ins);
            SyncOne::failed(reason, Some(ins), false)
        }
    }
}

/// 一键同步时的自动提交：
/// 1. 有冲突中的文件，或正在 merge / cherry-pick / revert → 不提交
/// 2. 领先且落后（分叉）→ 不提交
/// 3. 落后 → 先 pull --ff-only；git 拒绝（本地改动和云端改动碰到同一文件等）→ 不提交
/// 4. git add -A → git commit -m → push（不强推）；推送失败时提交保留在本地
async fn auto_commit_and_push(dir: &Path, ins: Inspection, message: &str) -> SyncOne {
    if ins.conflicts > 0 {
        let reason = format!("有 {} 个文件处于冲突状态（合并未完成），不会自动提交，请先手动解决冲突", ins.conflicts);
        return SyncOne::failed(reason, Some(ins), false);
    }
    match operation_in_progress(dir).await {
        Ok(None) => {}
        Ok(Some(op)) => {
            let reason = format!("仓库正在{op}，还没完成，不会自动提交，请先手动完成或中止");
            return SyncOne::failed(reason, Some(ins), false);
        }
        Err(e) => {
            let reason = format!("无法确认仓库是否正在合并，未提交任何内容：{}", e.message());
            return SyncOne::failed(reason, Some(ins), false);
        }
    }
    if ins.ahead > 0 && ins.behind > 0 {
        let reason = format!(
            "本地和云端各有新提交（领先 {}、落后 {}），不会自动提交，需要先合并",
            ins.ahead, ins.behind
        );
        return SyncOne::failed(reason, Some(ins), false);
    }

    let mut ins = ins;
    if ins.behind > 0 {
        if let Err(e) = pull_ff_only(dir, &ins).await {
            let reason = format!("拉取云端的 {} 个新提交失败，未提交任何内容：\n{}", ins.behind, e.message());
            return SyncOne::failed(reason, Some(ins), false);
        }
        let pulled = ins.behind;
        ins = match inspect(dir).await {
            Ok(i) => i,
            Err(e) => {
                let reason = format!("已拉取云端的新提交，但读取状态失败，未提交任何内容：{}", e.message());
                return SyncOne::failed(reason, Some(ins), false);
            }
        };
        match ins.status {
            SyncStatus::Dirty => {}
            // 拉取后工作区刚好变干净（本地改动和云端一致），不需要再提交
            SyncStatus::Synced => return SyncOne::ok(SyncOutcome::Pulled, pulled, ins),
            _ => {
                let reason = issue_text(&ins);
                return SyncOne::failed(reason, Some(ins), false);
            }
        }
    }

    if let Err(e) = run_git(dir, &["add", "-A"], network_timeout()).await {
        let reason = format!("自动提交失败（暂存改动时出错），未提交任何内容：{}", e.message());
        return SyncOne::failed(reason, Some(reinspect(dir, ins.clone()).await), false);
    }
    if let Err(e) = run_git(dir, &["commit", "--quiet", "-m", message], network_timeout()).await {
        let reason = format!("自动提交失败，未提交任何内容：{}", e.message());
        return SyncOne::failed(reason, Some(reinspect(dir, ins.clone()).await), false);
    }
    let after = match inspect(dir).await {
        Ok(i) => i,
        Err(e) => {
            let reason = format!("已提交到本地，但读取状态失败，没有推送：{}", e.message());
            return SyncOne::failed(reason, Some(ins), false);
        }
    };
    match push(dir, &after).await {
        Ok(()) => SyncOne::ok(SyncOutcome::Committed, after.ahead, reinspect(dir, after.clone()).await),
        Err(e) => {
            let reason = format!("已提交到本地，但推送失败：{}", e.message());
            SyncOne::failed(reason, Some(reinspect(dir, after.clone()).await), false)
        }
    }
}

/// 是否有未完成的 merge / cherry-pick / revert（此时提交会把它们"完成"掉，所以不自动提交）
async fn operation_in_progress(dir: &Path) -> Result<Option<&'static str>, GitError> {
    for (head, label) in [
        ("MERGE_HEAD", "合并（merge）"),
        ("CHERRY_PICK_HEAD", "拣选提交（cherry-pick）"),
        ("REVERT_HEAD", "撤销提交（revert）"),
    ] {
        match run_git(dir, &["rev-parse", "-q", "--verify", head], STATUS_TIMEOUT).await {
            Ok(_) => return Ok(Some(label)),
            // 不存在时退出码为 1
            Err(GitError::Failed { .. }) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(None)
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

    run_git(dir, &["add", "-A"], network_timeout())
        .await
        .map_err(|e| format!("暂存改动失败：{}", e.message()))?;
    run_git(dir, &["commit", "--quiet", "-m", message], network_timeout())
        .await
        .map_err(|e| format!("提交失败：{}", e.message()))?;

    fetch(dir, &ins, network_timeout())
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

/// git 自己的代理：先看全局 / 系统级 git 配置里的 http.proxy（全局优先），没配置再看环境变量。
/// 返回 Some("") 表示 git 配置里明确写了空值（git 约定为不用代理）。
pub async fn git_own_proxy() -> Option<String> {
    let (global, system) = tokio::join!(git_config_proxy("--global"), git_config_proxy("--system"));
    global.or(system).or_else(|| proxy_from_env(|k| std::env::var(k).ok()))
}

/// 读 `git config <scope> --get http.proxy`；没配置（退出码 1）或读取失败时为 None
async fn git_config_proxy(scope: &str) -> Option<String> {
    let out = run_git(&std::env::temp_dir(), &["config", scope, "--get", "http.proxy"], STATUS_TIMEOUT).await.ok()?;
    Some(out.stdout.trim().to_string())
}

/// 检测 git 是否已安装，返回版本号
pub async fn git_version() -> Result<String, GitError> {
    let out = run_git(&std::env::temp_dir(), &["--version"], STATUS_TIMEOUT).await?;
    Ok(out.stdout.trim().to_string())
}
