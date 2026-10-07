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
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncItemResult {
    pub project_id: String,
    pub outcome: SyncOutcome,
    pub commits: u32,
    pub reason: Option<String>,
}

/// 一键同步的进度事件。并行处理时 index 表示开始 / 完成的先后顺序。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SyncProgressEvent {
    #[serde(rename_all = "camelCase")]
    Start { project_id: String, index: usize, total: usize },
    #[serde(rename_all = "camelCase")]
    Done { result: SyncItemResult, project: Project, index: usize, total: usize },
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
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub default_code_dir: Option<String>,
    pub theme: Option<String>,
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
}
