//! 针对单个仓库的 git 操作。只做 fetch、push（快进）、pull --ff-only、用户确认后的 add/commit，
//! 以及分叉时"预演无冲突才合并"的自动合并（merge-tree 预演 → merge，失败就 merge --abort）。
//! 不使用 --force、reset、rebase、stash、clean、checkout 等任何可能改写历史或丢失改动的命令。

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::git::{
    network_timeout, run_git, run_git_progress, GitError, GitOutput, MERGE_TREE_FLAGS, PROXY_CONFIG_REGEX,
    STATUS_TIMEOUT,
};
use crate::model::{ChangeKind, CommitInfo, GitOp, GitProgress, SyncOutcome, SyncStatus};
use crate::status::{inspect, issue_text, no_remote_reason, parse_porcelain_v2, read_config, Inspection, DETACHED};
use crate::url::sanitize_url;

/// 联网操作的进度交给谁（一键同步、后台同步把它转成进度事件）；`none()` 表示没人要看进度
#[derive(Clone, Default)]
pub struct ProgressSink(Option<Arc<dyn Fn(GitOp, GitProgress) + Send + Sync>>);

impl ProgressSink {
    pub fn new(f: impl Fn(GitOp, GitProgress) + Send + Sync + 'static) -> Self {
        ProgressSink(Some(Arc::new(f)))
    }

    pub fn none() -> Self {
        ProgressSink(None)
    }
}

impl std::fmt::Debug for ProgressSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "ProgressSink(有)" } else { "ProgressSink(无)" })
    }
}

/// 执行联网的 git 命令（fetch / push / pull）：有人要看进度时把 `--quiet` 换成 `--progress`，边运行边上报；
/// 没人看时和原来一样安静地执行
async fn run_net(
    dir: &Path,
    args: &[&str],
    timeout: Duration,
    op: GitOp,
    progress: &ProgressSink,
) -> Result<GitOutput, GitError> {
    let Some(f) = &progress.0 else {
        return run_git(dir, args, timeout).await;
    };
    let args = with_progress(args);
    run_git_progress(dir, &args, timeout, &|p| f(op, p)).await
}

/// `--quiet` 换成 `--progress`（`--quiet` 会让 push 不输出进度）；本来没有 `--quiet` 的，在子命令后面加上
pub fn with_progress<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut out: Vec<&str> = args.iter().copied().filter(|a| *a != "--quiet").collect();
    out.insert(1.min(out.len()), "--progress");
    out
}

/// 只 fetch 上游分支（下载量和 pull 一样，分支、标签多的仓库不会因为下载其他分支而超时）。
/// 上游分支在本地已没有远程跟踪分支，或者按分支 fetch 失败（如云端分支已删除）时，退回 fetch 整个远程；
/// 超时不退回，免得再等一轮。`timeout` 是每次 fetch 的超时（刷新用 `refresh_timeout()`，其余用 `network_timeout()`）。
pub async fn fetch(dir: &Path, ins: &Inspection, timeout: Duration) -> Result<(), GitError> {
    fetch_with(dir, ins, timeout, &ProgressSink::none()).await
}

/// 和 `fetch` 一样，进度交给 `progress`
pub async fn fetch_with(dir: &Path, ins: &Inspection, timeout: Duration, progress: &ProgressSink) -> Result<(), GitError> {
    let remote = ins.remote_name.as_deref().unwrap_or("origin");
    if let (Some(branch), false) = (ins.upstream_branch.as_deref(), ins.upstream_gone) {
        match run_net(dir, &["fetch", "--quiet", remote, branch], timeout, GitOp::Fetch, progress).await {
            Ok(_) => return Ok(()),
            Err(e @ GitError::Timeout { .. }) => return Err(e),
            Err(_) => {}
        }
    }
    run_net(dir, &["fetch", "--quiet", remote], timeout, GitOp::Fetch, progress).await.map(|_| ())
}

/// 推送当前分支到它的上游（显式写出 refspec，不加 +，远程拒绝时不会强推）
pub async fn push(dir: &Path, ins: &Inspection, progress: &ProgressSink) -> Result<(), GitError> {
    let remote = ins.remote_name.as_deref().unwrap_or("origin");
    let target = ins.upstream_branch.as_deref().unwrap_or(&ins.branch);
    let refspec = format!("refs/heads/{}:refs/heads/{}", ins.branch, target);
    run_net(dir, &["push", "--quiet", remote, &refspec], network_timeout(), GitOp::Push, progress).await.map(|_| ())
}

/// 只快进拉取。--no-rebase / --no-autostash 防止全局配置（pull.rebase、autostash）让它变成变基或自动 stash
pub async fn pull_ff_only(dir: &Path, ins: &Inspection, progress: &ProgressSink) -> Result<(), GitError> {
    let remote = ins.remote_name.as_deref().unwrap_or("origin");
    let target = ins.upstream_branch.as_deref().unwrap_or(&ins.branch);
    run_net(
        dir,
        &["pull", "--ff-only", "--no-rebase", "--no-autostash", "--quiet", remote, target],
        network_timeout(),
        GitOp::Pull,
        progress,
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
    check_with(dir, fetch_timeout, &ProgressSink::none()).await
}

/// 和 `check` 一样，fetch 的进度交给 `progress`
pub async fn check_with(dir: &Path, fetch_timeout: Duration, progress: &ProgressSink) -> Checked {
    let local = match inspect(dir).await {
        Ok(i) => i,
        Err(e) => return Checked::Failed(e),
    };
    if !local.can_fetch() {
        return Checked::Fresh(local);
    }
    if let Err(error) = fetch_with(dir, &local, fetch_timeout, progress).await {
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
    /// 分叉时先用 merge-tree 预演合并，没有冲突就正式合并并推送；false 时分叉照旧记为异常
    pub auto_merge: bool,
    /// 自动合并的提交信息（`{host}` 已换好，见 `merge_message`）；为空时用不带电脑名的默认说明
    pub merge_message: String,
    /// 自动提交前的检查（大文件、疑似密钥文件）；None 表示不检查
    pub precommit: Option<PrecommitCheck>,
    /// fetch / push / pull 的进度交给谁；默认没人看
    pub progress: ProgressSink,
}

/// 自动提交前检查的设置
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrecommitCheck {
    /// 单个文件的大小上限（MB）
    pub big_file_limit_mb: u64,
}

/// 自动合并的提交信息模板；`{host}` 和自动提交信息里的 `{host}` 用同一个来源
pub const MERGE_MESSAGE_TEMPLATE: &str = "自动合并：来自 {host}";
/// 合并提交信息为空时的兜底（git 不接受空的合并说明）
const FALLBACK_MERGE_MESSAGE: &str = "自动合并";

/// 按模板生成自动合并的提交信息
pub fn merge_message(host: &str) -> String {
    MERGE_MESSAGE_TEMPLATE.replace("{host}", host)
}

/// 提交前检查的大文件上限：默认 50 MB，可设 1 ~ 2048 MB
pub const DEFAULT_BIG_FILE_LIMIT_MB: u64 = 50;
pub const MAX_BIG_FILE_LIMIT_MB: u64 = 2048;

/// 一键同步单个项目：先检查状态，再按状态处理
/// - 待推送 → push；待拉取 → pull --ff-only；已同步 → 跳过
/// - 有未提交改动：打开了自动提交（`opts.auto_commit` 为提交信息）→ 先在本地提交，再推送或按分叉处理，
///   见 `auto_commit_and_push`；没打开 → 不做任何操作，记为异常
/// - 分叉：打开了自动合并 → 预演无冲突才合并并推送，见 `merge_and_push`；没打开 → 不做任何操作，记为异常
/// - 未关联云端 → 不做任何操作，记为异常
pub async fn sync_one(dir: &Path, opts: &SyncOptions) -> SyncOne {
    let checked = if opts.skip_fetch {
        match inspect(dir).await {
            Ok(i) => Checked::Fresh(i),
            Err(e) => Checked::Failed(e),
        }
    } else {
        check_with(dir, network_timeout(), &opts.progress).await
    };
    let ins = match checked {
        Checked::Fresh(i) => i,
        Checked::FetchFailed { local, error } => return SyncOne::failed(error.message(), Some(local), true),
        Checked::Failed(error) => return SyncOne::failed(error.message(), None, true),
    };
    let auto_commit = opts.auto_commit.as_deref();

    match ins.status {
        SyncStatus::Ahead => match push(dir, &ins, &opts.progress).await {
            Ok(()) => SyncOne::ok(SyncOutcome::Pushed, ins.ahead, reinspect(dir, ins.clone()).await),
            Err(e) => SyncOne::failed(e.message(), Some(ins), false),
        },
        SyncStatus::Behind => match pull_ff_only(dir, &ins, &opts.progress).await {
            Ok(()) => SyncOne::ok(SyncOutcome::Pulled, ins.behind, reinspect(dir, ins.clone()).await),
            Err(e) => SyncOne::failed(e.message(), Some(ins), false),
        },
        SyncStatus::Synced => SyncOne::ok(SyncOutcome::UpToDate, 0, ins),
        // 分叉意味着工作区干净（"有未提交改动"优先级更高）
        SyncStatus::Diverged if opts.auto_merge => merge_and_push(dir, ins, &opts.merge_message, &opts.progress).await,
        // 有未提交改动意味着已有上游（"未关联云端"优先级更高）
        SyncStatus::Dirty if auto_commit.is_some() => {
            auto_commit_and_push(dir, ins, auto_commit.unwrap_or(""), opts).await
        }
        _ => {
            let reason = issue_text(&ins);
            SyncOne::failed(reason, Some(ins), false)
        }
    }
}

/// 一键同步时的自动提交（先在本地提交，再看云端）：
/// 1. 有冲突中的文件，或正在 merge / cherry-pick / revert → 不提交
/// 2. 提交前检查（`opts.precommit`，见 `precommit_issues`）：有超过上限的大文件或疑似密钥文件 → 不提交
/// 3. git add -A → git commit -m（不管云端有没有新提交，都先提交到本地）
/// 4. 云端没有新提交 → push（不强推）；云端也有新提交（现在是分叉）→ 打开了自动合并就预演，
///    无冲突再合并并推送（见 `merge_upstream`），否则提交留在本地、记为异常。推送失败时提交保留在本地
async fn auto_commit_and_push(dir: &Path, ins: Inspection, message: &str, opts: &SyncOptions) -> SyncOne {
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
    if let Some(check) = opts.precommit {
        match files_to_commit(dir).await {
            Ok(files) => {
                let issues = precommit_issues(&files, check.big_file_limit_mb);
                if !issues.is_empty() {
                    return SyncOne::failed(precommit_reason(&issues, check.big_file_limit_mb), Some(ins), false);
                }
            }
            Err(e) => {
                let reason = format!("提交前检查失败（列出要提交的文件时出错），未提交任何内容：{}", e.message());
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

    if after.behind == 0 {
        return match push(dir, &after, &opts.progress).await {
            Ok(()) => SyncOne::ok(SyncOutcome::Committed, after.ahead, reinspect(dir, after.clone()).await),
            Err(e) => {
                let reason = format!("已提交到本地，但推送失败：{}", e.message());
                SyncOne::failed(reason, Some(reinspect(dir, after.clone()).await), false)
            }
        };
    }

    // 云端也有新提交：现在本地和云端分叉了，按分叉处理
    if !opts.auto_merge {
        let reason = format!(
            "已自动提交到本地，但云端也有新提交（领先 {}、落后 {}），自动合并已关闭，没有推送，需要先合并",
            after.ahead, after.behind
        );
        return SyncOne::failed(reason, Some(after), false);
    }
    match merge_upstream(dir, &after, &opts.merge_message).await {
        Ok(merged) => push_merged(dir, merged, &opts.progress).await,
        Err(reason) => {
            let reason = format!("已自动提交到本地（还没推送）。{reason}");
            SyncOne::failed(reason, Some(reinspect(dir, after.clone()).await), false)
        }
    }
}

// ---------------- 分叉时的自动合并 ----------------

/// 分叉时的自动合并：预演无冲突才合并（见 `merge_upstream`），合并后推送
async fn merge_and_push(dir: &Path, ins: Inspection, message: &str, progress: &ProgressSink) -> SyncOne {
    match merge_upstream(dir, &ins, message).await {
        Ok(merged) => push_merged(dir, merged, progress).await,
        Err(reason) => SyncOne::failed(reason, Some(reinspect(dir, ins.clone()).await), false),
    }
}

/// 推送刚在本地合并好的结果；推送失败时合并提交保留在本地
async fn push_merged(dir: &Path, merged: Inspection, progress: &ProgressSink) -> SyncOne {
    match push(dir, &merged, progress).await {
        Ok(()) => SyncOne::ok(SyncOutcome::Merged, merged.ahead, reinspect(dir, merged.clone()).await),
        Err(e) => {
            let reason = format!("已在本地自动合并，但推送失败：{}", e.message());
            SyncOne::failed(reason, Some(reinspect(dir, merged.clone()).await), false)
        }
    }
}

/// 把上游合并进当前分支（只在本地，不推送），返回合并后的状态：
/// 1. 工作区必须干净，且没有未完成的 merge / cherry-pick / revert
/// 2. 分支配置了 `branch.<分支>.mergeoptions` → 不自动合并（git merge 会把它加进命令行，白名单管不到，
///    比如 `-s ours` 会悄悄丢掉云端的改动，`--no-commit` / `--squash` 会停在合并到一半）
/// 3. `merge-tree --write-tree` 在内存里预演（git 要不低于 2.38）；有冲突或预演失败 → 什么都不动，返回原因
/// 4. 无冲突 → `merge --no-edit -m <说明> <上游提交>`；万一失败 → 有 MERGE_HEAD 就 `merge --abort`，返回原因
/// 5. 合并后、推送前核对结果和预演一致（见 `verify_merge`）；不一致就不推送（有 MERGE_HEAD 就 `merge --abort`），返回原因
///
/// 预演和正式合并用的是同一个上游提交（先解析成哈希），中途远程跟踪分支变了也不会合进没预演过的提交
async fn merge_upstream(dir: &Path, ins: &Inspection, message: &str) -> Result<Inspection, String> {
    if !ins.changes.is_empty() {
        return Err(format!("工作区有 {} 个未提交的改动，不会自动合并", ins.changes.len()));
    }
    match operation_in_progress(dir).await {
        Ok(None) => {}
        Ok(Some(op)) => return Err(format!("仓库正在{op}，还没完成，不会自动合并，请先手动完成或中止")),
        Err(e) => return Err(format!("无法确认仓库是否正在合并，没有自动合并：{}", e.message())),
    }
    match branch_merge_options(dir, &ins.branch).await {
        Ok(None) => {}
        Ok(Some(value)) => {
            return Err(format!(
                "分支配置了 branch.{}.mergeoptions（{value}），git merge 会自动带上这些选项，合并结果可能和预演不一样，\
                 没有自动合并，也没有改动任何文件。可以去掉这个配置后再同步，或者手动合并",
                ins.branch
            ))
        }
        Err(e) => return Err(format!("读取分支的合并配置失败，没有自动合并：{}", e.message())),
    }
    let (ours, theirs) = merge_sides(dir)
        .await
        .map_err(|e| format!("读取本地和云端的最新提交失败，没有自动合并：{}", e.message()))?;
    let tree = match preview_merge(dir, &ours, &theirs).await? {
        MergePreview::Conflicts(files) => return Err(conflict_reason(&files)),
        MergePreview::Clean { tree } => tree,
    };

    let message = if message.trim().is_empty() { FALLBACK_MERGE_MESSAGE } else { message };
    if let Err(e) = run_git(dir, &["merge", "--no-edit", "-m", message, &theirs], network_timeout()).await {
        let undo = abort_if_merging(dir).await;
        return Err(match undo {
            Abort::NotMerging => format!("自动合并失败，没有合并：{}", e.message()),
            Abort::Aborted => {
                format!("自动合并失败，已撤销这次合并（merge --abort），项目恢复到合并前的样子：{}", e.message())
            }
            Abort::Failed(e2) => {
                format!("自动合并失败，撤销合并（merge --abort）也失败了，请手动检查这个项目：{}\n{e2}", e.message())
            }
        });
    }
    // git merge 退出码是 0 也不代表合并结果就是预演的那样：核对一遍，不一致就不推送
    if let Err(detail) = verify_merge(dir, &ours, &theirs, &tree).await {
        let undo = match abort_if_merging(dir).await {
            Abort::NotMerging => "合并结果留在本地，请先检查这个项目，确认没问题再手动推送".to_string(),
            Abort::Aborted => "已撤销这次合并（merge --abort），项目恢复到合并前的样子".to_string(),
            Abort::Failed(e) => format!("撤销合并（merge --abort）也失败了，请手动检查这个项目：{e}"),
        };
        return Err(format!("已在本地合并，但结果和预演不一致，没有推送：{detail}。{undo}"));
    }
    inspect(dir)
        .await
        .map_err(|e| format!("已在本地自动合并，但读取状态失败，没有推送：{}", e.message()))
}

/// 分支配置的 `branch.<分支>.mergeoptions`（所有层级的 git 配置都算）；没配或是空值时为 None
async fn branch_merge_options(dir: &Path, branch: &str) -> Result<Option<String>, GitError> {
    let key = format!("branch.{branch}.mergeoptions");
    match run_git(dir, &["config", "--get", &key], STATUS_TIMEOUT).await {
        Ok(out) => Ok(Some(out.stdout.trim().to_string()).filter(|v| !v.is_empty())),
        // 退出码 1 表示没有这个配置
        Err(GitError::Failed { code: Some(1), .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// 合并后、推送前核对结果和预演一致，返回第一处不一致的说明：
/// 没有 MERGE_HEAD（合并已完成）；HEAD^1 是本地原来的提交；HEAD^2 是预演用的云端提交；HEAD 的树就是预演算出的树
pub async fn verify_merge(dir: &Path, ours: &str, theirs: &str, tree: &str) -> Result<(), String> {
    if merge_in_progress(dir).await {
        return Err("合并没有完成（留下了 MERGE_HEAD）".into());
    }
    for (rev, want, what) in [
        ("HEAD^1", ours, "合并提交的第一个父提交不是本地原来的提交"),
        ("HEAD^2", theirs, "合并提交的第二个父提交不是预演用的云端提交"),
        ("HEAD^{tree}", tree, "合并后的文件和预演算出的不一样"),
    ] {
        let got = run_git(dir, &["rev-parse", "-q", "--verify", rev], STATUS_TIMEOUT).await;
        if got.as_ref().ok().map(|o| o.stdout.trim()) != Some(want) {
            return Err(what.to_string());
        }
    }
    Ok(())
}

/// 是否有进行中的合并（MERGE_HEAD）
async fn merge_in_progress(dir: &Path) -> bool {
    run_git(dir, &["rev-parse", "-q", "--verify", "MERGE_HEAD"], STATUS_TIMEOUT).await.is_ok()
}

/// `abort_if_merging` 的结果
enum Abort {
    /// 没有进行中的合并，不需要撤销
    NotMerging,
    /// 已 `merge --abort`
    Aborted,
    /// `merge --abort` 失败（git 的报错）
    Failed(String),
}

/// 有进行中的合并（MERGE_HEAD）就 `merge --abort`，把项目恢复到合并前的样子
async fn abort_if_merging(dir: &Path) -> Abort {
    if !merge_in_progress(dir).await {
        return Abort::NotMerging;
    }
    match run_git(dir, &["merge", "--abort"], network_timeout()).await {
        Ok(_) => Abort::Aborted,
        Err(e) => Abort::Failed(e.message()),
    }
}

/// 本地（HEAD）和云端（上游分支）最新提交的哈希
async fn merge_sides(dir: &Path) -> Result<(String, String), GitError> {
    let ours = run_git(dir, &["rev-parse", "-q", "--verify", "HEAD"], STATUS_TIMEOUT).await?;
    let theirs = run_git(dir, &["rev-parse", "-q", "--verify", "@{upstream}"], STATUS_TIMEOUT).await?;
    Ok((ours.stdout.trim().to_string(), theirs.stdout.trim().to_string()))
}

/// 预演合并的结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergePreview {
    /// 没有冲突，可以自动合并；tree 是预演算出的合并结果（树的哈希），正式合并后用它核对
    Clean { tree: String },
    /// 会冲突的文件
    Conflicts(Vec<String>),
}

/// 支持 `merge-tree --write-tree` 的最低 git 版本
pub const MIN_MERGE_TREE_VERSION: (u32, u32) = (2, 38);

/// 从 `git --version` 的输出（如 "git version 2.50.1.windows.1"）取出主、次版本号
pub fn parse_git_version(s: &str) -> Option<(u32, u32)> {
    let v = s.trim().strip_prefix("git version ")?;
    let mut parts = v.split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// 这个版本的 git 能不能预演合并。认不出版本号时照常尝试：不支持的话 merge-tree 会报错，同样什么都不动
pub fn supports_merge_tree(version: &str) -> bool {
    parse_git_version(version).map_or(true, |v| v >= MIN_MERGE_TREE_VERSION)
}

/// 解析 `git merge-tree --write-tree --name-only -z --no-messages` 的结果：
/// 输出是 `<树的哈希>\0<冲突文件>\0<冲突文件>\0…`；退出码 0 = 没有冲突，1 = 有冲突。
/// 其他情况（如退出码 1 但没有树的哈希，那是参数错了）返回 None
pub fn parse_merge_tree(exit_code: i32, stdout: &str) -> Option<MergePreview> {
    let mut records = stdout.split('\0');
    let tree = records.next()?.trim();
    if !matches!(tree.len(), 40 | 64) || !tree.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match exit_code {
        0 => Some(MergePreview::Clean { tree: tree.to_string() }),
        1 => {
            let mut seen = HashSet::new();
            let files = records
                .filter(|f| !f.is_empty() && seen.insert(f.to_string()))
                .map(str::to_string)
                .collect();
            Some(MergePreview::Conflicts(files))
        }
        _ => None,
    }
}

/// 用 `git merge-tree --write-tree` 在内存里预演合并：不碰工作区、暂存区和分支，只把算出来的树对象写进对象库。
/// git 低于 2.38 时不支持，返回需要升级 git 的说明
pub async fn preview_merge(dir: &Path, ours: &str, theirs: &str) -> Result<MergePreview, String> {
    let version = git_version().await.map_err(|e| e.message())?;
    if !supports_merge_tree(&version) {
        return Err(format!(
            "当前的 git（{version}）低于 2.38，不支持预演合并，没有自动合并。请升级 Git for Windows：https://git-scm.com/download/win"
        ));
    }
    let mut args = vec!["merge-tree"];
    args.extend(MERGE_TREE_FLAGS);
    args.extend([ours, theirs]);
    let failed = |detail: String| format!("预演合并失败，没有自动合并，也没有改动任何文件：{detail}");
    let result = run_git(dir, &args, network_timeout()).await;
    // 有冲突时 git 的退出码是 1，输出照样有用
    let (code, stdout) = match &result {
        Ok(out) => (0, out.stdout.as_str()),
        Err(GitError::Failed { code: Some(1), stdout, .. }) => (1, stdout.as_str()),
        Err(e) => return Err(failed(e.message())),
    };
    parse_merge_tree(code, stdout).ok_or_else(|| match &result {
        Err(e) => failed(e.message()),
        Ok(_) => failed(format!("看不懂 git 的输出 {stdout:?}")),
    })
}

/// 分叉时预演合并算出的冲突文件（不联网，只读本地的远程跟踪分支）；不能预演或没有冲突时为空
pub async fn preview_conflicts(dir: &Path) -> Vec<String> {
    let Ok((ours, theirs)) = merge_sides(dir).await else {
        return Vec::new();
    };
    match preview_merge(dir, &ours, &theirs).await {
        Ok(MergePreview::Conflicts(files)) => files,
        _ => Vec::new(),
    }
}

/// 原因里最多列出的文件数，多出来的写"等 N 个文件"
const MAX_LISTED_FILES: usize = 10;

fn list_files(files: &[String]) -> String {
    let shown = files.iter().take(MAX_LISTED_FILES).map(String::as_str).collect::<Vec<_>>().join("、");
    if files.len() > MAX_LISTED_FILES {
        format!("{shown} 等 {} 个文件", files.len())
    } else {
        shown
    }
}

/// 预演发现冲突时的异常原因
pub fn conflict_reason(files: &[String]) -> String {
    let files = if files.is_empty() { "（git 没有列出文件名）".to_string() } else { list_files(files) };
    format!("本地和云端改了同一处，自动合并会冲突：{files}。没有改动任何文件，可在详情里「复制给 AI」")
}

// ---------------- 提交前检查 ----------------

/// 提交前检查命中的问题
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrecommitIssue {
    /// 文件名像密钥（.env、*.pem、id_rsa……）
    Secret { path: String },
    /// 单个文件超过大小上限
    TooBig { path: String, bytes: u64 },
}

/// 文件名是否像密钥文件（只看文件名，不分大小写）：
/// `.env`、`.env.*`（`.env.example` / `.env.sample` / `.env.template` 除外）、`*.pem`、`*.key`、`*.p12`、`*.pfx`、
/// `id_rsa`、`id_dsa`、`id_ecdsa`、`id_ed25519`（`.pub` 公钥不算）
pub fn looks_like_secret(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_lowercase();
    if name == ".env" {
        return true;
    }
    if let Some(suffix) = name.strip_prefix(".env.") {
        return !matches!(suffix, "example" | "sample" | "template");
    }
    matches!(name.as_str(), "id_rsa" | "id_dsa" | "id_ecdsa" | "id_ed25519")
        || [".pem", ".key", ".p12", ".pfx"].iter().any(|ext| name.ends_with(ext))
}

/// 提交前检查（纯函数）：`files` 是将要提交、磁盘上还在的文件（路径, 字节数），`limit_mb` 是单个文件上限
pub fn precommit_issues(files: &[(String, u64)], limit_mb: u64) -> Vec<PrecommitIssue> {
    let limit = limit_mb.saturating_mul(1024 * 1024);
    let mut issues = Vec::new();
    for (path, bytes) in files {
        if looks_like_secret(path) {
            issues.push(PrecommitIssue::Secret { path: path.clone() });
        }
        if *bytes > limit {
            issues.push(PrecommitIssue::TooBig { path: path.clone(), bytes: *bytes });
        }
    }
    issues
}

fn format_size(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= MB {
        format!("{:.1} MB", bytes as f64 / MB)
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

/// 原因里最多列出的问题数
const MAX_LISTED_ISSUES: usize = 20;

/// 提交前检查没通过时的异常原因
pub fn precommit_reason(issues: &[PrecommitIssue], limit_mb: u64) -> String {
    let mut lines = vec!["提交前检查没通过，这次没有自动提交：".to_string()];
    for issue in issues.iter().take(MAX_LISTED_ISSUES) {
        lines.push(match issue {
            PrecommitIssue::Secret { path } => format!("- {path}：疑似密钥文件"),
            PrecommitIssue::TooBig { path, bytes } => {
                format!("- {path}：{}，超过 {limit_mb} MB 的上限", format_size(*bytes))
            }
        });
    }
    if issues.len() > MAX_LISTED_ISSUES {
        lines.push(format!("- ……共 {} 处", issues.len()));
    }
    lines.push(
        "不该上传的文件请加进 .gitignore；确实要提交的，可以手动\"提交并推送\"，或在设置里关掉提交前检查".to_string(),
    );
    lines.join("\n")
}

/// 将要提交的文件（`git status --porcelain=v2 -z --untracked-files=all`，被 .gitignore 忽略的已由 git 排除），
/// 只保留磁盘上还在的普通文件（删除的不用检查），附上大小
async fn files_to_commit(dir: &Path) -> Result<Vec<(String, u64)>, GitError> {
    let out = run_git(dir, &["status", "--porcelain=v2", "-z", "--untracked-files=all"], network_timeout()).await?;
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for c in parse_porcelain_v2(&out.stdout).changes {
        if c.kind == ChangeKind::Deleted || !seen.insert(c.path.clone()) {
            continue;
        }
        // 被删除的文件、子模块 / 嵌套仓库的目录都不用检查
        if let Ok(meta) = std::fs::metadata(dir.join(&c.path)) {
            if meta.is_file() {
                files.push((c.path, meta.len()));
            }
        }
    }
    Ok(files)
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

    push(dir, &after, &ProgressSink::none())
        .await
        .map_err(|e| format!("已提交到本地，但推送失败：{}", e.message()))?;
    Ok(reinspect(dir, after).await)
}

/// 最近 3 次提交（只取哈希、时间、标题，不含任何文件内容）
pub async fn recent_commits(dir: &Path) -> Vec<CommitInfo> {
    recent_commits_of(dir, "HEAD").await
}

/// 云端（上游分支）最近 3 次提交，取的是本地的远程跟踪分支（不联网）；没有上游时为空
pub async fn recent_upstream_commits(dir: &Path) -> Vec<CommitInfo> {
    recent_commits_of(dir, "@{upstream}").await
}

async fn recent_commits_of(dir: &Path, rev: &str) -> Vec<CommitInfo> {
    // 末尾的 -- 表示 rev 一定是提交而不是文件名（仓库里有叫 HEAD 的文件时也不会报"有歧义"）
    let args = ["log", "-3", "--format=%h%x1f%ad%x1f%s", "--date=format:%Y-%m-%d %H:%M", rev, "--"];
    let Ok(out) = run_git(dir, &args, STATUS_TIMEOUT).await else {
        return Vec::new(); // 还没有任何提交，或者没有上游
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

/// git 配置里的代理（全局 / 系统级）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitProxyConfig {
    /// `http.proxy`；Some("") 表示明确写了空值（git 约定为不用代理）
    pub proxy: Option<String>,
    /// 按网址设置的代理 `http.<网址>.proxy`：(网址, 值)，值为空表示这个网址明确不用代理
    pub url_proxies: Vec<(String, String)>,
}

impl GitProxyConfig {
    /// 按网址设置过代理的网址（不管值是不是空）
    pub fn url_scopes(&self) -> Vec<String> {
        self.url_proxies.iter().map(|(url, _)| url.clone()).collect()
    }
}

/// 读取 git 自己配置的代理：全局和系统级都读，同一个配置项全局优先（和 git 自己一样）
pub async fn git_proxy_config() -> GitProxyConfig {
    let (global, system) = tokio::join!(read_proxy_config("--global"), read_proxy_config("--system"));
    merge_proxy_config(global, system)
}

/// `git config <scope> -z --get-regexp <PROXY_CONFIG_REGEX>`；没配置（退出码 1）或读取失败时为空
async fn read_proxy_config(scope: &str) -> GitProxyConfig {
    let args = ["config", scope, "-z", "--get-regexp", PROXY_CONFIG_REGEX];
    match run_git(&std::env::temp_dir(), &args, STATUS_TIMEOUT).await {
        Ok(out) => parse_proxy_config(&out.stdout),
        Err(_) => GitProxyConfig::default(),
    }
}

/// 解析 `git config -z --get-regexp` 的输出：每条是 `键\n值\0`（没有值时是 `键\0`）。
/// 键里的 section 和变量名已被 git 转成小写，网址部分保留原样。同一个键出现多次时取最后一个（和 git 一样）。
pub fn parse_proxy_config(out: &str) -> GitProxyConfig {
    let mut cfg = GitProxyConfig::default();
    for record in out.split('\0').filter(|r| !r.is_empty()) {
        let (key, value) = record.split_once('\n').unwrap_or((record, ""));
        let value = value.trim().to_string();
        if key == "http.proxy" {
            cfg.proxy = Some(value);
        } else if let Some(url) = key.strip_prefix("http.").and_then(|k| k.strip_suffix(".proxy")) {
            match cfg.url_proxies.iter_mut().find(|(u, _)| u == url) {
                Some(entry) => entry.1 = value,
                None => cfg.url_proxies.push((url.to_string(), value)),
            }
        }
    }
    cfg
}

/// 合并全局和系统级的代理配置：同一个配置项以全局为准
pub fn merge_proxy_config(global: GitProxyConfig, system: GitProxyConfig) -> GitProxyConfig {
    let mut url_proxies = global.url_proxies;
    for (url, value) in system.url_proxies {
        if !url_proxies.iter().any(|(u, _)| *u == url) {
            url_proxies.push((url, value));
        }
    }
    GitProxyConfig { proxy: global.proxy.or(system.proxy), url_proxies }
}

/// 检测 git 是否已安装，返回版本号
pub async fn git_version() -> Result<String, GitError> {
    let out = run_git(&std::env::temp_dir(), &["--version"], STATUS_TIMEOUT).await?;
    Ok(out.stdout.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(u: &str, v: &str) -> (String, String) {
        (u.to_string(), v.to_string())
    }

    #[test]
    fn parses_proxy_config() {
        assert_eq!(parse_proxy_config(""), GitProxyConfig::default());
        // 实测的 git 输出格式：键\n值\0；空值是 键\n\0；没写值是 键\0
        let out = "http.proxy\nsocks5://127.0.0.1:10808\0\
                   http.https://github.com.proxy\nsocks5://127.0.0.1:1080\0\
                   http.https://GitLab.Corp.proxy\n\0\
                   http.https://novalue.example.proxy\0\
                   http.https://github.com.proxy\nhttp://127.0.0.1:7890\0";
        assert_eq!(
            parse_proxy_config(out),
            GitProxyConfig {
                proxy: Some("socks5://127.0.0.1:10808".into()),
                url_proxies: vec![
                    url("https://github.com", "http://127.0.0.1:7890"), // 同一个键取最后一个
                    url("https://GitLab.Corp", ""),
                    url("https://novalue.example", ""),
                ],
            }
        );
        // 明确写了空的 http.proxy
        assert_eq!(parse_proxy_config("http.proxy\n\0").proxy.as_deref(), Some(""));
    }

    #[test]
    fn progress_args() {
        assert_eq!(with_progress(&["fetch", "--quiet", "origin", "main"]), ["fetch", "--progress", "origin", "main"]);
        assert_eq!(
            with_progress(&["push", "--quiet", "origin", "refs/heads/a:refs/heads/a"]),
            ["push", "--progress", "origin", "refs/heads/a:refs/heads/a"]
        );
        assert_eq!(
            with_progress(&["pull", "--ff-only", "--no-rebase", "--no-autostash", "--quiet", "origin", "main"]),
            ["pull", "--progress", "--ff-only", "--no-rebase", "--no-autostash", "origin", "main"]
        );
        for args in [
            with_progress(&["fetch", "--quiet", "origin"]),
            with_progress(&["pull", "--ff-only", "--no-rebase", "--no-autostash", "--quiet", "origin", "main"]),
        ] {
            assert!(crate::git::validate(&args).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn git_version_for_merge_tree() {
        assert_eq!(parse_git_version("git version 2.50.1.windows.1"), Some((2, 50)));
        assert_eq!(parse_git_version("git version 2.38.0\n"), Some((2, 38)));
        assert_eq!(parse_git_version("git version 2.9.5"), Some((2, 9)));
        assert_eq!(parse_git_version("git version 2.39.3 (Apple Git-146)"), Some((2, 39)));
        assert_eq!(parse_git_version("不是版本号"), None);
        assert!(supports_merge_tree("git version 2.38.0.windows.1"));
        assert!(supports_merge_tree("git version 3.0.0"));
        assert!(!supports_merge_tree("git version 2.37.3.windows.1"));
        assert!(!supports_merge_tree("git version 2.9.5"), "2.9 比 2.38 旧（按数字比较，不是按字符串）");
        assert!(supports_merge_tree("git version 不认识"), "认不出版本号时照常尝试");
    }

    #[test]
    fn parses_merge_tree_output() {
        let tree = "58017f32555d8cced875cce658b2ffd75d47a68b";
        // 实测的输出：没有冲突时只有树的哈希；有冲突时后面跟着冲突文件名
        assert_eq!(parse_merge_tree(0, &format!("{tree}\0")), Some(MergePreview::Clean { tree: tree.into() }));
        assert_eq!(
            parse_merge_tree(1, &format!("{tree}\0README.md\0中文 文件.md\0README.md\0")),
            Some(MergePreview::Conflicts(vec!["README.md".into(), "中文 文件.md".into()]))
        );
        // 退出码 1 但没有树的哈希：参数错了（如 "not something we can merge"），不能当成"有冲突"
        assert_eq!(parse_merge_tree(1, ""), None);
        assert_eq!(parse_merge_tree(1, "fatal: x\0"), None);
        assert_eq!(parse_merge_tree(128, &format!("{tree}\0")), None);
        // SHA-256 仓库的哈希是 64 位
        let tree256 = "a".repeat(64);
        assert_eq!(parse_merge_tree(0, &format!("{tree256}\0")), Some(MergePreview::Clean { tree: tree256 }));
    }

    #[test]
    fn conflict_reason_lists_files() {
        let files: Vec<String> = ["README.md", "src/App.tsx"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            conflict_reason(&files),
            "本地和云端改了同一处，自动合并会冲突：README.md、src/App.tsx。没有改动任何文件，可在详情里「复制给 AI」"
        );
        let many: Vec<String> = (1..=12).map(|i| format!("f{i}.txt")).collect();
        let reason = conflict_reason(&many);
        assert!(reason.contains("f10.txt 等 12 个文件") && !reason.contains("f11.txt"), "{reason}");
        assert_eq!(merge_message("家里台式机"), "自动合并：来自 家里台式机");
    }

    #[test]
    fn secret_file_names() {
        for p in [
            ".env",
            ".env.local",
            ".env.production",
            "config/.ENV",
            "server/.env.dev",
            "certs/server.pem",
            "a/b/private.KEY",
            "cert.p12",
            "cert.pfx",
            "id_rsa",
            ".ssh/id_ed25519",
            "keys\\id_ecdsa",
            "id_dsa",
        ] {
            assert!(looks_like_secret(p), "应当算疑似密钥：{p}");
        }
        for p in [
            ".env.example",
            ".env.sample",
            ".env.template",
            "web/.env.EXAMPLE",
            "id_rsa.pub",
            "README.md",
            "env.ts",
            ".envrc",
            "src/keyboard.ts",
            "monkey.json",
            "docs/pem-format.md",
        ] {
            assert!(!looks_like_secret(p), "不应算疑似密钥：{p}");
        }
    }

    #[test]
    fn precommit_rules() {
        const MB: u64 = 1024 * 1024;
        let files = vec![
            ("README.md".to_string(), 100),
            (".env".to_string(), 30),
            ("assets/video.bin".to_string(), 2 * MB),
            ("exact.bin".to_string(), MB), // 刚好等于上限：不算超
            ("big.key".to_string(), 3 * MB),
        ];
        assert_eq!(
            precommit_issues(&files, 1),
            vec![
                PrecommitIssue::Secret { path: ".env".into() },
                PrecommitIssue::TooBig { path: "assets/video.bin".into(), bytes: 2 * MB },
                PrecommitIssue::Secret { path: "big.key".into() },
                PrecommitIssue::TooBig { path: "big.key".into(), bytes: 3 * MB },
            ]
        );
        // 默认 50 MB 时只剩密钥文件
        assert_eq!(precommit_issues(&files, DEFAULT_BIG_FILE_LIMIT_MB).len(), 2);
        assert!(precommit_issues(&[("ok.txt".into(), 10)], 1).is_empty());

        let reason = precommit_reason(&precommit_issues(&files, 1), 1);
        println!("{reason}");
        assert!(reason.starts_with("提交前检查没通过，这次没有自动提交"));
        assert!(reason.contains("- .env：疑似密钥文件"));
        assert!(reason.contains("- assets/video.bin：2.0 MB，超过 1 MB 的上限"));
        assert!(reason.contains(".gitignore"));
        assert_eq!(format_size(1500), "2 KB");
    }

    #[test]
    fn merges_global_over_system() {
        let global = GitProxyConfig { proxy: None, url_proxies: vec![url("https://github.com", "g")] };
        let system = GitProxyConfig {
            proxy: Some("s".into()),
            url_proxies: vec![url("https://github.com", "s"), url("https://gitlab.com", "s")],
        };
        let merged = merge_proxy_config(global, system);
        assert_eq!(merged.proxy.as_deref(), Some("s"), "全局没配就用系统级的");
        assert_eq!(merged.url_proxies, vec![url("https://github.com", "g"), url("https://gitlab.com", "s")]);
        assert_eq!(merged.url_scopes(), ["https://github.com", "https://gitlab.com"]);
        let both = merge_proxy_config(
            GitProxyConfig { proxy: Some("".into()), url_proxies: vec![] },
            GitProxyConfig { proxy: Some("s".into()), url_proxies: vec![] },
        );
        assert_eq!(both.proxy.as_deref(), Some(""), "全局写了空值（不用代理）也优先");
    }
}
