//! 与前端 src/types/index.ts 一一对应的数据结构（字段名转成 camelCase）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyncStatus {
    Synced,
    Ahead,
    Behind,
    Dirty,
    Diverged,
    NoRemote,
    Syncing,
}

impl SyncStatus {
    /// 排序权重，与 src/lib/status.ts 的 STATUS_META.weight 一致（需要处理的在前）
    pub fn weight(self) -> u8 {
        match self {
            SyncStatus::Diverged => 0,
            SyncStatus::Dirty => 1,
            SyncStatus::NoRemote => 2,
            SyncStatus::Behind => 3,
            SyncStatus::Ahead => 4,
            SyncStatus::Syncing => 5,
            SyncStatus::Synced => 6,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SyncStatus::Synced => "已同步",
            SyncStatus::Ahead => "待推送",
            SyncStatus::Behind => "待拉取",
            SyncStatus::Dirty => "有未提交改动",
            SyncStatus::Diverged => "分叉",
            SyncStatus::NoRemote => "未关联云端",
            SyncStatus::Syncing => "同步中",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKind {
    #[serde(rename = "M")]
    Modified,
    #[serde(rename = "A")]
    Added,
    #[serde(rename = "D")]
    Deleted,
    #[serde(rename = "R")]
    Renamed,
    #[serde(rename = "?")]
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
    pub staged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub branch: String,
    pub remote_url: Option<String>,
    pub upstream: Option<String>,
    pub status: SyncStatus,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<FileChange>,
    pub last_sync_at: Option<String>,
    pub issue: Option<String>,
    /// 检查失败（如断网时 fetch 失败）的原因；此时 status 保留上一次的结果
    pub check_error: Option<String>,
    /// 一键同步时是否自动提交并推送
    pub auto_commit: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannedRepo {
    pub path: String,
    pub name: String,
    pub branch: String,
    pub remote_url: Option<String>,
    pub monitored: bool,
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyncOutcome {
    Pushed,
    Pulled,
    UpToDate,
    /// 已自动提交并推送
    Committed,
    Failed,
    /// 分叉时预演合并没有冲突，已自动合并并推送（commits 为推送的提交数，含合并提交）
    Merged,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncItemResult {
    pub project_id: String,
    pub outcome: SyncOutcome,
    pub commits: u32,
    pub reason: Option<String>,
}

/// 正在进行的联网 git 操作
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GitOp {
    Fetch,
    Push,
    Pull,
    Clone,
}

/// git 进度的阶段（只分用户关心的几种）：
/// 准备中（统计、压缩、枚举对象，以及云端那边的输出）、上传、下载、处理差异、写出文件（克隆后检出）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProgressPhase {
    Preparing,
    Uploading,
    Downloading,
    Resolving,
    Checkout,
}

/// 从 git 的 `--progress` 输出解析出的一条进度
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GitProgress {
    pub phase: ProgressPhase,
    /// 0 ~ 100
    pub percent: u8,
}

/// 一键同步的进度事件。并行处理时 index 表示开始 / 完成的先后顺序。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SyncProgressEvent {
    /// 这一轮要处理的项目（按处理顺序）。等网络重试时会再来一次，只列要重试的
    #[serde(rename_all = "camelCase")]
    Begin { project_ids: Vec<String>, total: usize },
    #[serde(rename_all = "camelCase")]
    Start { project_id: String, index: usize, total: usize },
    /// 正在处理的项目里 git 的阶段和百分比（每个项目最多 200 毫秒一次）
    #[serde(rename_all = "camelCase")]
    Progress { project_id: String, op: GitOp, phase: ProgressPhase, percent: u8 },
    #[serde(rename_all = "camelCase")]
    Done { result: SyncItemResult, project: Project, index: usize, total: usize },
    /// 后台同步：联网失败的项目等一会儿再试（`retry_at` 为 RFC 3339 时间）
    #[serde(rename_all = "camelCase")]
    Retry { project_ids: Vec<String>, retry_at: String },
}

/// 正在处理的项目和它最近一次的 git 进度（还没有进度时三项都是 None）
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveSync {
    pub project_id: String,
    pub op: Option<GitOp>,
    pub phase: Option<ProgressPhase>,
    pub percent: Option<u8>,
}

/// 等网络重试
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryInfo {
    pub project_ids: Vec<String>,
    pub retry_at: String,
}

/// 一轮同步到现在的进度（后端按进度事件累积，窗口中途打开时用它补上）。前端 src/state/syncProgress.ts 的 applyEvent 是同一套规则
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProgressSnapshot {
    /// 这一轮的项目（按处理顺序）
    pub project_ids: Vec<String>,
    pub active: Vec<ActiveSync>,
    /// 已完成的项目的结果（按完成顺序）
    pub results: Vec<SyncItemResult>,
    pub retry: Option<RetryInfo>,
}

impl SyncProgressSnapshot {
    /// 按一条进度事件更新。Begin：等重试时是接着这一轮（只把要重试的项目改回没完成），否则从头开始新的一轮
    pub fn apply(&mut self, e: &SyncProgressEvent) {
        match e {
            SyncProgressEvent::Begin { project_ids, .. } => {
                if self.retry.take().is_some() {
                    self.results.retain(|r| !project_ids.contains(&r.project_id));
                    for id in project_ids {
                        if !self.project_ids.contains(id) {
                            self.project_ids.push(id.clone());
                        }
                    }
                } else {
                    *self = SyncProgressSnapshot { project_ids: project_ids.clone(), ..Default::default() };
                }
            }
            SyncProgressEvent::Start { project_id, .. } => {
                self.active.retain(|a| a.project_id != *project_id);
                self.active.push(ActiveSync { project_id: project_id.clone(), op: None, phase: None, percent: None });
            }
            SyncProgressEvent::Progress { project_id, op, phase, percent } => {
                let entry = match self.active.iter_mut().position(|a| a.project_id == *project_id) {
                    Some(i) => &mut self.active[i],
                    None => {
                        self.active.push(ActiveSync { project_id: project_id.clone(), op: None, phase: None, percent: None });
                        self.active.last_mut().expect("刚放进去")
                    }
                };
                entry.op = Some(*op);
                entry.phase = Some(*phase);
                entry.percent = Some(*percent);
            }
            SyncProgressEvent::Done { result, .. } => {
                self.active.retain(|a| a.project_id != result.project_id);
                self.results.retain(|r| r.project_id != result.project_id);
                self.results.push(result.clone());
            }
            SyncProgressEvent::Retry { project_ids, retry_at } => {
                self.retry = Some(RetryInfo { project_ids: project_ids.clone(), retry_at: retry_at.clone() });
            }
        }
    }

    /// （已完成的项目数, 这一轮的项目数）
    pub fn counts(&self) -> (usize, usize) {
        (self.results.len(), self.project_ids.len())
    }
}

/// 刷新状态的进度事件：每个项目检查完立即发一次，index 为完成顺序（0 起）
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum RefreshProgressEvent {
    #[serde(rename_all = "camelCase")]
    Done { project: Project, index: usize, total: usize },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub started_at: String,
    pub finished_at: String,
    pub items: Vec<SyncItemResult>,
    pub synced_count: usize,
    pub failed_count: usize,
}

/// 返回给前端的设置；theme 为 null 表示还没选过，由前端跟随系统
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub default_code_dir: String,
    pub theme: Option<String>,
    /// 自动提交的提交信息模板；没设置过时为默认模板
    pub commit_template: String,
    /// 联网操作（fetch、push、pull）的超时秒数
    pub network_timeout_secs: u64,
    /// 代理模式："auto"（默认）/ "off"（不用代理）/ "manual"（手动填写）
    pub proxy_mode: String,
    /// 手动填写的代理地址（切到其他模式后仍保留，方便切回来）
    pub proxy_url: Option<String>,
    /// 只读：SyncDeck 调用的 git 实际用的代理地址（去掉了账号密码）；None 表示不用代理
    pub effective_proxy: Option<String>,
    /// 只读：实际生效的代理来自哪里
    pub proxy_source: ProxySource,
    /// 本机名称，提交信息模板里的 `{host}` 用它；空字符串表示没填，用 `default_host_name`
    pub host_name: String,
    /// 只读：系统的电脑名（COMPUTERNAME）
    pub default_host_name: String,
    /// 开机自启（开机后只进托盘，不弹窗口）
    pub launch_at_login: bool,
    /// 关窗口时隐藏到托盘，不退出
    pub close_to_tray: bool,
    /// 离开时同步：锁屏
    pub sync_on_lock: bool,
    /// 到达时同步：程序启动（含开机自启）
    pub sync_on_startup: bool,
    /// 到达时同步：从睡眠唤醒
    pub sync_on_resume: bool,
    /// 到达时同步：解锁
    pub sync_on_unlock: bool,
    /// 定时检查云端：每 30 分钟 fetch + status，只更新列表和托盘，不拉取
    pub periodic_check: bool,
    /// 后台同步成功且确实推送或拉取了东西时发系统通知
    pub notify_on_success: bool,
    /// 后台同步有项目出错或冲突时发系统通知
    pub notify_on_error: bool,
    /// 只读：自动同步暂停到什么时候（RFC 3339，带时区）；None 表示没暂停。用 pause_auto_sync / resume_auto_sync 修改
    pub auto_sync_paused_until: Option<String>,
    /// 分叉时先预演合并，没有冲突就自动合并并推送（默认开）
    pub auto_merge: bool,
    /// 自动提交前检查大文件和疑似密钥文件（默认开）
    pub precommit_check: bool,
    /// 提交前检查的单个文件大小上限（MB，默认 50）
    pub big_file_limit_mb: u64,
}

/// 实际生效的代理来自哪里
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxySource {
    /// git 自己配置的代理（http.proxy 或环境变量），SyncDeck 不覆盖
    Git,
    /// Windows 系统代理
    System,
    /// 设置里手动填写的代理
    Manual,
    /// 不使用代理
    #[default]
    #[serde(rename = "none")]
    NoProxy,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub default_code_dir: Option<String>,
    pub theme: Option<String>,
    /// 去掉空白后为空（或等于默认模板）时恢复默认
    pub commit_template: Option<String>,
    /// 只能是 git::NETWORK_TIMEOUT_CHOICES 之一
    pub network_timeout_secs: Option<u64>,
    /// "auto" / "off" / "manual"；切到 manual 时必须已有（或同时给出）合法的代理地址
    pub proxy_mode: Option<String>,
    /// 手动填写的代理地址；空字符串表示清除
    pub proxy_url: Option<String>,
    /// 本机名称；空字符串表示清除（用电脑名）
    pub host_name: Option<String>,
    pub launch_at_login: Option<bool>,
    pub close_to_tray: Option<bool>,
    pub sync_on_lock: Option<bool>,
    pub sync_on_startup: Option<bool>,
    pub sync_on_resume: Option<bool>,
    pub sync_on_unlock: Option<bool>,
    pub periodic_check: Option<bool>,
    pub notify_on_success: Option<bool>,
    pub notify_on_error: Option<bool>,
    pub auto_merge: Option<bool>,
    pub precommit_check: Option<bool>,
    /// 只能在 1 ~ ops::MAX_BIG_FILE_LIMIT_MB 之间
    pub big_file_limit_mb: Option<u64>,
}

/// 自动同步的触发原因。离开：锁屏；到达：启动 / 唤醒 / 解锁 / 托盘"立即同步"；另有定时检查云端
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutoSyncTrigger {
    Lock,
    Startup,
    Resume,
    Unlock,
    /// 托盘菜单"立即同步"
    Manual,
    /// 定时检查云端（只 fetch + status，不拉取）
    Periodic,
}

/// 后台自动同步的事件（auto-sync-started / auto-sync-finished），也用于查询当前状态
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoSyncEvent {
    pub trigger: AutoSyncTrigger,
    /// 显示用的文字，如"解锁后同步"
    pub label: String,
    /// 结束时的同步结果；开始时、或者没有要同步的项目（没联网）时为 None
    pub result: Option<SyncResult>,
}

/// 后台自动同步的当前状态（前端启动时查询一次，之后靠事件更新）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoSyncState {
    /// 正在进行的后台同步；None 表示没有
    pub running: Option<AutoSyncEvent>,
    /// 启动时的到达同步由后端负责（正在做或已做完）：前端启动时不用再自己刷新一次
    pub startup_sync: bool,
    /// 正在进行的后台同步到现在的进度；没有后台同步时为 None
    pub progress: Option<SyncProgressSnapshot>,
    /// 最近一次后台同步的结果（只在内存里，重启后没有）；还没有过时为 None
    pub last: Option<AutoSyncEvent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitInfo {
    pub installed: bool,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitInfo {
    pub hash: String,
    pub date: String,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticInfo {
    pub project: Project,
    pub recent_commits: Vec<CommitInfo>,
    /// 分叉时用 merge-tree 预演合并算出的冲突文件；不是分叉、没有冲突或无法预演时为空
    pub conflict_files: Vec<String>,
    /// 云端（上游分支）最近 3 次提交；没有上游时为空
    pub recent_remote_commits: Vec<CommitInfo>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(id: &str, outcome: SyncOutcome) -> SyncProgressEvent {
        let project = Project {
            id: id.into(),
            name: id.into(),
            path: String::new(),
            branch: "main".into(),
            remote_url: None,
            upstream: None,
            status: SyncStatus::Synced,
            ahead: 0,
            behind: 0,
            changes: Vec::new(),
            last_sync_at: None,
            issue: None,
            check_error: None,
            auto_commit: false,
        };
        let result = SyncItemResult { project_id: id.into(), outcome, commits: 1, reason: None };
        SyncProgressEvent::Done { result, project, index: 0, total: 0 }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn snapshot_follows_events_and_retry() {
        let mut s = SyncProgressSnapshot::default();
        s.apply(&SyncProgressEvent::Begin { project_ids: ids(&["a", "b", "c"]), total: 3 });
        s.apply(&SyncProgressEvent::Start { project_id: "a".into(), index: 0, total: 3 });
        s.apply(&SyncProgressEvent::Progress {
            project_id: "a".into(),
            op: GitOp::Push,
            phase: ProgressPhase::Uploading,
            percent: 45,
        });
        assert_eq!(s.active[0].percent, Some(45));
        assert_eq!(s.active[0].op, Some(GitOp::Push));
        s.apply(&done("a", SyncOutcome::Pushed));
        s.apply(&SyncProgressEvent::Start { project_id: "b".into(), index: 1, total: 3 });
        s.apply(&done("b", SyncOutcome::Failed));
        s.apply(&done("c", SyncOutcome::Failed));
        assert!(s.active.is_empty());
        assert_eq!(s.counts(), (3, 3));

        // 等网络重试：接着这一轮，只把要重试的改回没完成
        s.apply(&SyncProgressEvent::Retry { project_ids: ids(&["b"]), retry_at: "t".into() });
        assert!(s.retry.is_some());
        s.apply(&SyncProgressEvent::Begin { project_ids: ids(&["b"]), total: 1 });
        assert!(s.retry.is_none());
        assert_eq!(s.counts(), (2, 3));
        s.apply(&done("b", SyncOutcome::Pulled));
        assert_eq!(s.counts(), (3, 3));
        assert_eq!(s.results.iter().filter(|r| r.project_id == "b").count(), 1);

        // 没在等重试时的 Begin：新的一轮
        s.apply(&SyncProgressEvent::Begin { project_ids: ids(&["x"]), total: 1 });
        assert_eq!(s.counts(), (0, 1));
    }
}
