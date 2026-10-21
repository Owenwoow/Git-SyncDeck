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
  /** 一键同步时自动提交并推送（每个项目单独开关，默认关） */
  autoCommit: boolean;
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
  | "committed" // 已自动提交并推送（项目打开了"自动提交"）
  | "merged" // 分叉时预演合并没有冲突，已自动合并并推送（设置里打开了"无冲突时自动合并"）
  | "failed"; // 异常，需要手动处理

export interface SyncItemResult {
  projectId: string;
  outcome: SyncOutcome;
  /** 推送或拉取的提交数（committed 时为推送的提交数，含自动提交的那一个；merged 时含合并提交） */
  commits: number;
  /** 异常原因，仅 outcome 为 failed 时有值 */
  reason: string | null;
}

/** 正在进行的联网 git 操作 */
export type GitOp = "fetch" | "push" | "pull" | "clone";

/** git 进度的阶段：准备中（统计、压缩）/ 上传 / 下载 / 处理差异 / 写出文件（克隆后检出） */
export type ProgressPhase = "preparing" | "uploading" | "downloading" | "resolving" | "checkout";

/** 一键同步（和后台同步）的进度事件：
 *  begin 这一轮要处理的项目（等网络重试时会再来一次，只列要重试的）；start 开始处理某项目；
 *  progress 该项目 git 的阶段和百分比（每个项目最多 200 毫秒一次）；done 处理完毕（附带最新状态）；
 *  retry 后台同步时联网失败的项目等一会儿再试 */
export type SyncProgressEvent =
  | { type: "begin"; projectIds: string[]; total: number }
  | { type: "start"; projectId: string; index: number; total: number }
  | { type: "progress"; projectId: string; op: GitOp; phase: ProgressPhase; percent: number }
  | { type: "done"; result: SyncItemResult; project: Project; index: number; total: number }
  | { type: "retry"; projectIds: string[]; retryAt: string };

/** 正在处理的项目和它最近一次的 git 进度 */
export interface ActiveSync {
  projectId: string;
  op: GitOp | null;
  phase: ProgressPhase | null;
  percent: number | null;
}

/** 一轮同步到现在的进度（后端按进度事件累积，窗口中途打开时用它补上） */
export interface SyncProgressSnapshot {
  projectIds: string[];
  active: ActiveSync[];
  results: SyncItemResult[];
  retry: { projectIds: string[]; retryAt: string } | null;
}

/** 刷新状态的进度事件：每个项目检查完立即发一次，index 为完成顺序（从 0 开始） */
export type RefreshProgressEvent = { type: "done"; project: Project; index: number; total: number };

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

/** 生成诊断文本、「复制给 AI」提示词用的信息（不含任何文件内容） */
export interface DiagnosticInfo {
  project: Project;
  /** 本地最近 3 次提交 */
  recentCommits: CommitInfo[];
  /** 分叉时预演合并算出的冲突文件；不是分叉、没有冲突或无法预演时为空 */
  conflictFiles: string[];
  /** 云端（上游分支）最近 3 次提交；没有上游时为空 */
  recentRemoteCommits: CommitInfo[];
}

export type Theme = "light" | "dark";

export interface Settings {
  /** 默认代码目录，添加项目页默认扫描这里 */
  defaultCodeDir: string;
  theme: Theme;
  /** 自动提交的提交信息模板，占位符 {date}、{host}；保存空值会恢复默认模板 */
  commitTemplate: string;
  /** 联网操作（fetch、push、pull）的超时秒数，只能是 NETWORK_TIMEOUT_CHOICES 之一 */
  networkTimeoutSecs: number;
  /** 代理模式：自动（默认）/ 不用代理 / 手动填写。只影响 SyncDeck 调用的 git，不改全局 git 配置 */
  proxyMode: ProxyMode;
  /** 手动填写的代理地址（切到其他模式后仍保留）；保存空字符串表示清除 */
  proxyUrl: string | null;
  /** 只读：实际生效的代理地址（去掉了账号密码）；null 表示不用代理 */
  effectiveProxy: string | null;
  /** 只读：实际生效的代理来自哪里 */
  proxySource: ProxySource;
  /** 本机名称（如"家里台式机"），提交信息模板里的 {host} 用它；空字符串表示用电脑名 defaultHostName */
  hostName: string;
  /** 只读：这台电脑的系统名称（COMPUTERNAME） */
  defaultHostName: string;
  /** 开机自启（开机后只进托盘，不弹窗口），默认开 */
  launchAtLogin: boolean;
  /** 关窗口时隐藏到托盘，不退出，默认开 */
  closeToTray: boolean;
  /** 离开时同步：锁屏，默认开 */
  syncOnLock: boolean;
  /** 到达时同步：程序启动（含开机自启），默认开 */
  syncOnStartup: boolean;
  /** 到达时同步：从睡眠唤醒，默认开 */
  syncOnResume: boolean;
  /** 到达时同步：解锁，默认开 */
  syncOnUnlock: boolean;
  /** 定时检查云端：每 30 分钟 fetch + status，只更新列表和托盘，不拉取，默认关 */
  periodicCheck: boolean;
  /** 后台同步成功且确实推送或拉取了东西时发系统通知，默认关 */
  notifyOnSuccess: boolean;
  /** 后台同步有项目出错或冲突时发系统通知，默认开 */
  notifyOnError: boolean;
  /** 只读：自动同步暂停到的时间（ISO，带时区）；null 表示没暂停。用 pauseAutoSync / resumeAutoSync 修改 */
  autoSyncPausedUntil: string | null;
  /** 分叉时先预演合并，没有冲突就自动合并并推送（默认开） */
  autoMerge: boolean;
  /** 自动提交前检查大文件和疑似密钥文件（默认开；手动"提交并推送"不检查） */
  precommitCheck: boolean;
  /** 提交前检查的单个文件大小上限（MB），1 ~ MAX_BIG_FILE_LIMIT_MB，默认 50 */
  bigFileLimitMb: number;
}

/** 自动同步的触发原因。离开：锁屏；到达：启动 / 唤醒 / 解锁 / 托盘"立即同步"；另有定时检查云端 */
export type AutoSyncTrigger = "lock" | "startup" | "resume" | "unlock" | "manual" | "periodic";

/** 后台自动同步的事件：开始时 result 为 null；结束时为同步结果（没有要同步的项目、定时检查时为 null） */
export interface AutoSyncEvent {
  trigger: AutoSyncTrigger;
  /** 显示用的文字，如"解锁后同步" */
  label: string;
  result: SyncResult | null;
}

/** 后台自动同步的当前状态 */
export interface AutoSyncState {
  /** 正在进行的后台同步；null 表示没有 */
  running: AutoSyncEvent | null;
  /** 启动时的到达同步由后端负责（正在做或已做完）：前端启动时不用再自己刷新 */
  startupSync: boolean;
  /** 正在进行的后台同步到现在的进度；没有后台同步时为 null */
  progress: SyncProgressSnapshot | null;
  /** 最近一次后台同步的结果（只在内存里，重启后没有） */
  last: AutoSyncEvent | null;
}

/** 取消事件监听 */
export type Unlisten = () => void;

/** 代理模式：auto 自动（git 自己配了代理就用 git 的，否则用 Windows 系统代理）/ off 不用代理 / manual 手动填写 */
export type ProxyMode = "auto" | "off" | "manual";

/** 实际生效的代理来自：git 自己的配置或环境变量 / Windows 系统代理 / 手动填写 / 不用代理 */
export type ProxySource = "git" | "system" | "manual" | "none";

/** 应用内更新：发现的新版本 */
export interface UpdateInfo {
  version: string;
  currentVersion: string;
  date: string | null;
  notes: string | null;
}

/** 更新包下载进度；total 为 null 表示服务器没给出总大小 */
export interface UpdateProgress {
  downloaded: number;
  total: number | null;
}
