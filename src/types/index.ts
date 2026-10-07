// 全部类型定义集中在这里。第二阶段接入 Tauri 命令时，Rust 端返回的数据结构应与这里保持一致。

/** 同步状态（共 7 种） */
export type SyncStatus =
  | "synced" // 已同步：本地与云端一致
  | "ahead" // 待推送：本地有新提交
  | "behind" // 待拉取：云端有新提交
  | "dirty" // 有未提交改动：工作区或暂存区有改动
  | "diverged" // 分叉：本地和云端都有新提交
  | "no-remote" // 未关联云端：没有远程仓库或没有上游分支
  | "syncing"; // 同步中：仅在一键同步过程中出现

/** 文件变更类型：M 修改 / A 新增 / D 删除 / R 重命名 / ? 未跟踪 */
export type FileChangeKind = "M" | "A" | "D" | "R" | "?";

export interface FileChange {
  path: string;
  kind: FileChangeKind;
  /** 是否已暂存（未跟踪文件恒为 false） */
  staged: boolean;
  /** 重命名前的路径，仅 kind 为 R 时有值 */
  oldPath?: string;
}

export interface Project {
  id: string;
  name: string;
  /** 本地路径，如 D:\code\blog */
  path: string;
  /** 当前分支 */
  branch: string;
  /** 远程仓库地址；null 表示没有远程仓库 */
  remoteUrl: string | null;
  /** 上游分支，如 origin/main；null 表示没有上游 */
  upstream: string | null;
  status: SyncStatus;
  /** 本地领先上游的提交数 */
  ahead: number;
  /** 本地落后上游的提交数 */
  behind: number;
  /** 未提交的改动（工作区 + 暂存区） */
  changes: FileChange[];
  /** 上次成功同步的时间（ISO 字符串）；null 表示从未同步 */
  lastSyncAt: string | null;
  /** 一键同步时被判为异常并跳过后保留的原因；null 表示没有遗留问题 */
  issue: string | null;
  /** 检查失败（如断网时 fetch 失败）的原因；此时 status 保留上一次的结果 */
  checkError: string | null;
}

/** 扫描目录时发现的 Git 仓库 */
export interface ScannedRepo {
  path: string;
  name: string;
  branch: string;
  remoteUrl: string | null;
  /** 是否已在监控列表中 */
  monitored: boolean;
  /** 已监控时对应的项目 id */
  projectId: string | null;
}

/** 一键同步过程中，单个项目的处理结果 */
export type SyncOutcome =
  | "pushed" // 已推送
  | "pulled" // 已拉取
  | "up-to-date" // 已是最新
  | "failed"; // 异常，需要手动处理

export interface SyncItemResult {
  projectId: string;
  outcome: SyncOutcome;
  /** 推送或拉取的提交数 */
  commits: number;
  /** 异常原因，仅 outcome 为 failed 时有值 */
  reason: string | null;
}

/** 一键同步的进度事件：start 表示开始处理某项目，done 表示该项目处理完毕（附带最新状态） */
export type SyncProgressEvent =
  | { type: "start"; projectId: string; index: number; total: number }
  | { type: "done"; result: SyncItemResult; project: Project; index: number; total: number };

export interface SyncResult {
  startedAt: string;
  finishedAt: string;
  items: SyncItemResult[];
  syncedCount: number;
  failedCount: number;
}

/** 启动时检测系统里的 git */
export interface GitInfo {
  installed: boolean;
  version: string | null;
}

/** 诊断文本里的最近提交（只有哈希、时间、标题，不含文件内容） */
export interface CommitInfo {
  hash: string;
  date: string;
  subject: string;
}

export type Theme = "light" | "dark";

export interface Settings {
  /** 默认代码目录，添加项目页默认扫描这里 */
  defaultCodeDir: string;
  theme: Theme;
}
