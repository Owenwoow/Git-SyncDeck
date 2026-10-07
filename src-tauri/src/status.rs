//! 状态检测：解析 `git status --porcelain=v2 --branch -z` 和仓库配置，按优先级判定 7 种状态之一。

use std::collections::HashMap;
use std::path::Path;

use crate::git::{run_git, GitError, STATUS_TIMEOUT};
use crate::model::{ChangeKind, FileChange, SyncStatus};
use crate::url::sanitize_url;

/// detached HEAD 时显示的分支名（与 src/lib/status.ts 的 DETACHED_BRANCH 一致）
pub const DETACHED: &str = "(detached HEAD)";

/// 一次本地检查的结果（不联网）
#[derive(Debug, Clone)]
pub struct Inspection {
    pub branch: String,
    pub detached: bool,
    /// 上游，如 origin/main
    pub upstream: Option<String>,
    /// 配置了上游，但本地没有对应的远程跟踪分支（云端分支已删除或从未 fetch）
    pub upstream_gone: bool,
    /// 上游所在的远程名，fetch / push / pull 都用它
    pub remote_name: Option<String>,
    /// 上游分支在远程上的名字（branch.<b>.merge 去掉 refs/heads/）
    pub upstream_branch: Option<String>,
    /// 显示用的远程地址（已脱敏）
    pub remote_url: Option<String>,
    pub has_remote: bool,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<FileChange>,
    /// 处于冲突状态（未合并）的文件数
    pub conflicts: u32,
    pub status: SyncStatus,
}

impl Inspection {
    /// 是否能联网检查（有上游所在的远程）
    pub fn can_fetch(&self) -> bool {
        !self.detached && self.remote_name.as_deref().is_some_and(|r| r != ".")
    }
}

/// 本地检查：git status + 读取远程 / 上游配置
pub async fn inspect(dir: &Path) -> Result<Inspection, GitError> {
    let out = run_git(dir, &["status", "--porcelain=v2", "--branch", "-z"], STATUS_TIMEOUT).await?;
    let porcelain = parse_porcelain_v2(&out.stdout);
    let config = read_config(dir).await?;
    Ok(build_inspection(porcelain, &config))
}

// ---------------- git status --porcelain=v2 ----------------

#[derive(Debug, Default, Clone)]
pub struct Porcelain {
    /// None 表示 detached HEAD
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead_behind: Option<(u32, u32)>,
    pub changes: Vec<FileChange>,
    /// 未合并（冲突中）的条目数
    pub conflicts: u32,
}

pub fn parse_porcelain_v2(out: &str) -> Porcelain {
    let mut p = Porcelain::default();
    let records: Vec<&str> = out.split('\0').collect();
    let mut i = 0;
    while i < records.len() {
        let r = records[i];
        i += 1;
        if r.is_empty() {
            continue;
        }
        if let Some(h) = r.strip_prefix("# ") {
            if let Some(v) = h.strip_prefix("branch.head ") {
                p.head = (v != "(detached)").then(|| v.to_string());
            } else if let Some(v) = h.strip_prefix("branch.upstream ") {
                p.upstream = Some(v.to_string());
            } else if let Some(v) = h.strip_prefix("branch.ab ") {
                let mut it = v.split(' ');
                let a = it.next().and_then(|s| s.trim_start_matches('+').parse().ok());
                let b = it.next().and_then(|s| s.trim_start_matches('-').parse().ok());
                if let (Some(a), Some(b)) = (a, b) {
                    p.ahead_behind = Some((a, b));
                }
            }
            continue;
        }
        match r.as_bytes()[0] {
            // 1 XY sub mH mI mW hH hI <path>
            b'1' => {
                let f: Vec<&str> = r.splitn(9, ' ').collect();
                if f.len() == 9 {
                    push_xy(&mut p.changes, f[1], f[8], None);
                }
            }
            // 2 XY sub mH mI mW hH hI Xscore <path>\0<origPath>
            b'2' => {
                let f: Vec<&str> = r.splitn(10, ' ').collect();
                let orig = records.get(i).copied();
                i += 1;
                if f.len() == 10 {
                    push_xy(&mut p.changes, f[1], f[9], orig);
                }
            }
            // u XY sub m1 m2 m3 mW h1 h2 h3 <path>（合并冲突）
            b'u' => {
                let f: Vec<&str> = r.splitn(11, ' ').collect();
                if f.len() == 11 {
                    p.changes.push(change(f[10], ChangeKind::Modified, false, None));
                    p.conflicts += 1;
                }
            }
            b'?' => {
                if let Some(path) = r.get(2..) {
                    p.changes.push(change(path, ChangeKind::Untracked, false, None));
                }
            }
            _ => {}
        }
    }
    p
}

fn change(path: &str, kind: ChangeKind, staged: bool, old_path: Option<&str>) -> FileChange {
    FileChange { path: path.to_string(), kind, staged, old_path: old_path.map(str::to_string) }
}

/// XY：X 是暂存区，Y 是工作区。同一个文件两边都有改动时（如 MM）分别记一条。
fn push_xy(changes: &mut Vec<FileChange>, xy: &str, path: &str, orig: Option<&str>) {
    let mut chars = xy.chars();
    let x = chars.next().unwrap_or('.');
    let y = chars.next().unwrap_or('.');
    let staged = match x {
        'M' | 'T' => Some(ChangeKind::Modified),
        'A' | 'C' => Some(ChangeKind::Added),
        'D' => Some(ChangeKind::Deleted),
        'R' => Some(ChangeKind::Renamed),
        _ => None,
    };
    if let Some(kind) = staged {
        let old = if kind == ChangeKind::Renamed { orig } else { None };
        changes.push(change(path, kind, true, old));
    }
    let worktree = match y {
        'M' | 'T' => Some(ChangeKind::Modified),
        'D' => Some(ChangeKind::Deleted),
        'A' => Some(ChangeKind::Added),
        _ => None,
    };
    if let Some(kind) = worktree {
        changes.push(change(path, kind, false, None));
    }
}

// ---------------- 远程与上游配置 ----------------

#[derive(Debug, Default, Clone)]
pub struct RepoConfig {
    /// (远程名, 地址)，保持配置里的顺序
    pub remotes: Vec<(String, String)>,
    pub branch_remote: HashMap<String, String>,
    pub branch_merge: HashMap<String, String>,
}

impl RepoConfig {
    fn url_of(&self, remote: &str) -> Option<String> {
        self.remotes.iter().find(|(n, _)| n == remote).map(|(_, u)| u.clone())
    }
}

/// 一次读出所有远程地址和各分支的上游配置（只读）
pub async fn read_config(dir: &Path) -> Result<RepoConfig, GitError> {
    let args = ["config", "-z", "--get-regexp", r"^(remote\..*\.url|branch\..*\.(remote|merge))$"];
    match run_git(dir, &args, STATUS_TIMEOUT).await {
        Ok(out) => Ok(parse_config(&out.stdout)),
        // 退出码 1 表示没有任何匹配项（比如没有远程）
        Err(GitError::Failed { code: Some(1), .. }) => Ok(RepoConfig::default()),
        Err(e) => Err(e),
    }
}

pub fn parse_config(out: &str) -> RepoConfig {
    let mut c = RepoConfig::default();
    for entry in out.split('\0').filter(|e| !e.is_empty()) {
        let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
        let value = value.to_string();
        if let Some(name) = key.strip_prefix("remote.").and_then(|k| k.strip_suffix(".url")) {
            c.remotes.push((name.to_string(), value));
        } else if let Some(rest) = key.strip_prefix("branch.") {
            if let Some(b) = rest.strip_suffix(".remote") {
                c.branch_remote.insert(b.to_string(), value);
            } else if let Some(b) = rest.strip_suffix(".merge") {
                c.branch_merge.insert(b.to_string(), value);
            }
        }
    }
    c
}

// ---------------- 判定 ----------------

pub fn build_inspection(p: Porcelain, cfg: &RepoConfig) -> Inspection {
    let detached = p.head.is_none();
    let branch = p.head.clone().unwrap_or_else(|| DETACHED.to_string());
    let has_remote = !cfg.remotes.is_empty();

    let (remote_name, upstream_branch) = if detached {
        (None, None)
    } else {
        (
            cfg.branch_remote.get(&branch).cloned(),
            cfg.branch_merge
                .get(&branch)
                .map(|m| m.strip_prefix("refs/heads/").unwrap_or(m).to_string()),
        )
    };

    // 显示用的远程：上游所在的远程 > origin > 第一个远程
    let remote_url = remote_name
        .as_deref()
        .and_then(|n| cfg.url_of(n))
        .or_else(|| cfg.url_of("origin"))
        .or_else(|| cfg.remotes.first().map(|(_, u)| u.clone()))
        .map(|u| sanitize_url(&u));

    // 上游是本地分支（remote = "."）时不算关联了云端
    let upstream = if remote_name.as_deref() == Some(".") { None } else { p.upstream.clone() };
    let upstream_gone = upstream.is_some() && p.ahead_behind.is_none();
    let (ahead, behind) = p.ahead_behind.unwrap_or((0, 0));

    let status = determine(has_remote, detached, upstream.is_some(), upstream_gone, !p.changes.is_empty(), ahead, behind);

    Inspection {
        branch,
        detached,
        upstream,
        upstream_gone,
        remote_name,
        upstream_branch,
        remote_url,
        has_remote,
        ahead,
        behind,
        changes: p.changes,
        conflicts: p.conflicts,
        status,
    }
}

/// 状态优先级：
/// 1. 没有远程 / 没有上游 / detached HEAD / 上游已不存在 → 未关联云端
/// 2. 工作区或暂存区有改动（含未跟踪文件）→ 有未提交改动
/// 3. 领先且落后 → 分叉；4. 只领先 → 待推送；5. 只落后 → 待拉取；6. 其余 → 已同步
pub fn determine(
    has_remote: bool,
    detached: bool,
    has_upstream: bool,
    upstream_gone: bool,
    has_changes: bool,
    ahead: u32,
    behind: u32,
) -> SyncStatus {
    if !has_remote || detached || !has_upstream || upstream_gone {
        SyncStatus::NoRemote
    } else if has_changes {
        SyncStatus::Dirty
    } else if ahead > 0 && behind > 0 {
        SyncStatus::Diverged
    } else if ahead > 0 {
        SyncStatus::Ahead
    } else if behind > 0 {
        SyncStatus::Behind
    } else {
        SyncStatus::Synced
    }
}

/// 异常原因（与 src/lib/status.ts 的 describeIssue 文案一致）
pub fn issue_text(ins: &Inspection) -> String {
    match ins.status {
        SyncStatus::Dirty => {
            let untracked = ins.changes.iter().filter(|c| c.kind == ChangeKind::Untracked).count();
            let staged = ins.changes.iter().filter(|c| c.staged).count();
            let total = ins.changes.len();
            let unstaged = total - untracked - staged;
            let parts: Vec<String> = [(staged, "已暂存"), (unstaged, "未暂存"), (untracked, "未跟踪")]
                .iter()
                .filter(|(n, _)| *n > 0)
                .map(|(n, label)| format!("{n} 个{label}"))
                .collect();
            format!("有 {total} 个未提交的改动（{}），需要先提交", parts.join("、"))
        }
        SyncStatus::Diverged => {
            format!("本地和云端各有新提交（领先 {}、落后 {}），需要先合并", ins.ahead, ins.behind)
        }
        SyncStatus::NoRemote => no_remote_reason(ins),
        _ => String::new(),
    }
}

pub fn no_remote_reason(ins: &Inspection) -> String {
    if !ins.has_remote {
        "没有配置远程仓库，无法与 GitHub 同步".to_string()
    } else if ins.detached {
        "处于 detached HEAD 状态（不在任何分支上），无法同步".to_string()
    } else if ins.upstream_gone {
        format!("上游分支 {} 在云端已不存在", ins.upstream.as_deref().unwrap_or(""))
    } else {
        format!("分支 {} 没有设置上游分支，不知道该推到哪里", ins.branch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_and_changes() {
        let out = "# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 M. N... 100644 100644 100644 aaa bbb src/a.rs\0\
1 MM N... 100644 100644 100644 aaa bbb 中文 文件.md\0\
1 .D N... 100644 100644 000000 aaa aaa old.txt\0\
2 R. N... 100644 100644 100644 aaa bbb R100 new name.txt\0old name.txt\0\
? 未跟踪.png\0";
        let p = parse_porcelain_v2(out);
        assert_eq!(p.head.as_deref(), Some("main"));
        assert_eq!(p.upstream.as_deref(), Some("origin/main"));
        assert_eq!(p.ahead_behind, Some((2, 1)));
        assert_eq!(p.changes.len(), 6);
        assert_eq!(p.changes[1].path, "中文 文件.md");
        assert!(p.changes[1].staged && !p.changes[2].staged);
        assert_eq!(p.changes[4].kind, ChangeKind::Renamed);
        assert_eq!(p.changes[4].path, "new name.txt");
        assert_eq!(p.changes[4].old_path.as_deref(), Some("old name.txt"));
        assert_eq!(p.changes[5].kind, ChangeKind::Untracked);
    }

    #[test]
    fn counts_conflicts() {
        let out = "# branch.head main\0u UU N... 100644 100644 100644 100644 aaa bbb ccc 冲突 文件.md\0\
1 M. N... 100644 100644 100644 aaa bbb ok.txt\0";
        let p = parse_porcelain_v2(out);
        assert_eq!(p.conflicts, 1);
        assert_eq!(p.changes.len(), 2);
        assert_eq!(p.changes[0].path, "冲突 文件.md");
        assert_eq!(parse_porcelain_v2("# branch.head main\0? a.txt\0").conflicts, 0);
    }

    #[test]
    fn detached_and_priority() {
        let p = parse_porcelain_v2("# branch.oid abc\0# branch.head (detached)\0");
        assert!(p.head.is_none());
        use SyncStatus::*;
        assert_eq!(determine(false, false, true, false, true, 1, 1), NoRemote);
        assert_eq!(determine(true, true, true, false, false, 0, 0), NoRemote);
        assert_eq!(determine(true, false, false, false, false, 0, 0), NoRemote);
        assert_eq!(determine(true, false, true, true, false, 0, 0), NoRemote);
        assert_eq!(determine(true, false, true, false, true, 1, 1), Dirty);
        assert_eq!(determine(true, false, true, false, false, 1, 1), Diverged);
        assert_eq!(determine(true, false, true, false, false, 1, 0), Ahead);
        assert_eq!(determine(true, false, true, false, false, 0, 1), Behind);
        assert_eq!(determine(true, false, true, false, false, 0, 0), Synced);
    }

    #[test]
    fn parses_config() {
        let out = "remote.origin.url\nhttps://tok@github.com/o/r.git\0branch.feature/x.y.remote\norigin\0branch.feature/x.y.merge\nrefs/heads/feature/x.y\0";
        let c = parse_config(out);
        assert_eq!(c.remotes, vec![("origin".to_string(), "https://tok@github.com/o/r.git".to_string())]);
        assert_eq!(c.branch_remote.get("feature/x.y").map(String::as_str), Some("origin"));
        let ins = build_inspection(
            Porcelain { head: Some("feature/x.y".into()), upstream: Some("origin/feature/x.y".into()), ahead_behind: Some((0, 0)), changes: vec![], conflicts: 0 },
            &c,
        );
        assert_eq!(ins.remote_url.as_deref(), Some("https://github.com/o/r.git"));
        assert_eq!(ins.upstream_branch.as_deref(), Some("feature/x.y"));
        assert_eq!(ins.status, SyncStatus::Synced);
    }
}
