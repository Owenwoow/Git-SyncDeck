// 7 种同步状态的文案、颜色、图标、排序规则，以及各种说明文字。
// 想改某个状态的颜色：改 src/index.css 里对应的 --status-* 变量（浅色/深色各一处）。
import {
  ArrowDown,
  ArrowUp,
  CheckCircle2,
  CloudOff,
  FilePen,
  GitFork,
  Loader2,
  type LucideIcon,
} from "lucide-react";
import type { FileChangeKind, Project, SyncStatus } from "@/types";
import { formatDateTime } from "@/lib/time";

export interface StatusMeta {
  label: string;
  icon: LucideIcon;
  /** 标签样式：浅色底 + 同色文字 */
  badgeClass: string;
  /** 小圆点颜色 */
  dotClass: string;
  /** 文字颜色 */
  textClass: string;
  /** 排序权重，越小越靠前（需要处理的排在前面） */
  weight: number;
  /** 一键同步时归为异常、需要手动处理 */
  abnormal: boolean;
  /** 一键同步时可以自动处理 */
  autoSync: boolean;
}

export const STATUS_META: Record<SyncStatus, StatusMeta> = {
  diverged: {
    label: "分叉",
    icon: GitFork,
    badgeClass: "bg-status-diverged/12 text-status-diverged ring-status-diverged/25",
    dotClass: "bg-status-diverged",
    textClass: "text-status-diverged",
    weight: 0,
    abnormal: true,
    autoSync: false,
  },
  dirty: {
    label: "有未提交改动",
    icon: FilePen,
    badgeClass: "bg-status-dirty/12 text-status-dirty ring-status-dirty/25",
    dotClass: "bg-status-dirty",
    textClass: "text-status-dirty",
    weight: 1,
    abnormal: true,
    autoSync: false,
  },
  "no-remote": {
    label: "未关联云端",
    icon: CloudOff,
    badgeClass: "bg-status-no-remote/12 text-status-no-remote ring-status-no-remote/25",
    dotClass: "bg-status-no-remote",
    textClass: "text-status-no-remote",
    weight: 2,
    abnormal: true,
    autoSync: false,
  },
  behind: {
    label: "待拉取",
    icon: ArrowDown,
    badgeClass: "bg-status-behind/12 text-status-behind ring-status-behind/25",
    dotClass: "bg-status-behind",
    textClass: "text-status-behind",
    weight: 3,
    abnormal: false,
    autoSync: true,
  },
  ahead: {
    label: "待推送",
    icon: ArrowUp,
    badgeClass: "bg-status-ahead/12 text-status-ahead ring-status-ahead/25",
    dotClass: "bg-status-ahead",
    textClass: "text-status-ahead",
    weight: 4,
    abnormal: false,
    autoSync: true,
  },
  syncing: {
    label: "同步中",
    icon: Loader2,
    badgeClass: "bg-status-syncing/12 text-status-syncing ring-status-syncing/25",
    dotClass: "bg-status-syncing",
    textClass: "text-status-syncing",
    weight: 5,
    abnormal: false,
    autoSync: false,
  },
  synced: {
    label: "已同步",
    icon: CheckCircle2,
    badgeClass: "bg-status-synced/12 text-status-synced ring-status-synced/25",
    dotClass: "bg-status-synced",
    textClass: "text-status-synced",
    weight: 6,
    abnormal: false,
    autoSync: false,
  },
};

/** 主页筛选标签里出现的状态（同步中是过程态，不参与筛选） */
export const FILTERABLE_STATUSES: SyncStatus[] = [
  "diverged",
  "dirty",
  "no-remote",
  "behind",
  "ahead",
  "synced",
];

export function isAbnormal(status: SyncStatus): boolean {
  return STATUS_META[status].abnormal;
}

/** 一键同步时会自动提交并推送：有未提交改动、打开了自动提交、有上游、本地和云端没有分叉 */
export function willAutoCommit(p: Project): boolean {
  return p.status === "dirty" && p.autoCommit && !!p.upstream && !(p.ahead > 0 && p.behind > 0);
}

/** 需要处理的排在前面；同权重按名称 */
export function sortProjects(list: Project[]): Project[] {
  return [...list].sort(
    (a, b) =>
      STATUS_META[a.status].weight - STATUS_META[b.status].weight ||
      a.name.localeCompare(b.name),
  );
}

/** https://github.com/owner/repo.git → owner/repo */
export function repoShortName(url: string | null): string | null {
  if (!url) return null;
  const m = url.match(/github\.com[/:]([^/]+\/[^/]+?)(?:\.git)?\/?$/i);
  return m ? m[1] : url;
}

function countChanges(p: Project) {
  const untracked = p.changes.filter((c) => c.kind === "?").length;
  const staged = p.changes.filter((c) => c.staged).length;
  const unstaged = p.changes.length - untracked - staged;
  return { total: p.changes.length, untracked, staged, unstaged };
}

/** 异常原因（一键同步结果、主页"已跳过"提示、诊断文本里用） */
export function describeIssue(p: Project): string {
  switch (p.status) {
    case "dirty": {
      const c = countChanges(p);
      const parts = [
        c.staged && `${c.staged} 个已暂存`,
        c.unstaged && `${c.unstaged} 个未暂存`,
        c.untracked && `${c.untracked} 个未跟踪`,
      ].filter(Boolean);
      return `有 ${c.total} 个未提交的改动（${parts.join("、")}），需要先提交`;
    }
    case "diverged":
      return `本地和云端各有新提交（领先 ${p.ahead}、落后 ${p.behind}），需要先合并`;
    case "no-remote":
      return noRemoteReason(p);
    default:
      return "";
  }
}

/** 详情抽屉里的状态说明：这个状态是什么意思、一键同步会怎么做 */
export function describeStatus(p: Project): string {
  switch (p.status) {
    case "synced":
      return "本地与 GitHub 完全一致，不需要任何操作。";
    case "ahead":
      return `本地有 ${p.ahead} 个提交还没推送到 GitHub。一键同步会自动推送。`;
    case "behind":
      return `GitHub 上有 ${p.behind} 个新提交还没拉到本地。一键同步会自动拉取。`;
    case "dirty":
      if (willAutoCommit(p))
        return `工作区有 ${p.changes.length} 个文件改动还没提交。已打开自动提交，一键同步会自动提交并推送${p.behind ? "（先拉取云端的新提交）" : ""}。`;
      if (p.autoCommit && p.ahead > 0 && p.behind > 0)
        return `工作区有 ${p.changes.length} 个文件改动还没提交，而且本地和云端各有新提交。即使打开了自动提交，一键同步也不会处理，需要先合并。`;
      return `工作区有 ${p.changes.length} 个文件改动还没提交。一键同步不会处理，可以在下方直接提交并推送。`;
    case "diverged":
      return `本地领先 ${p.ahead} 个提交，同时落后 ${p.behind} 个提交，两边都有对方没有的改动。一键同步不会处理，需要先合并。`;
    case "no-remote":
      if (!p.remoteUrl)
        return "这个仓库还没有关联任何远程仓库，一键同步无法处理，需要先在 GitHub 上建仓库并关联。";
      if (p.branch === DETACHED_BRANCH)
        return "当前不在任何分支上（detached HEAD），一键同步无法处理，需要先切回一个分支。";
      if (p.upstream)
        return `上游分支 ${p.upstream} 在云端已不存在，一键同步无法处理，需要重新设置上游分支。`;
      return `当前分支 ${p.branch} 没有设置上游分支，一键同步不知道该推到哪里，需要先推送并建立跟踪。`;
    case "syncing":
      return "正在与 GitHub 同步……";
  }
}

/** "提交并推送"按钮不可用时的原因；可用时返回 null */
export function commitPushDisabledReason(p: Project): string | null {
  if (p.status === "dirty") {
    if (!p.remoteUrl) return "没有远程仓库，提交后也无处可推";
    if (!p.upstream) return "当前分支没有上游分支，无法直接推送";
    return null;
  }
  if (p.status === "diverged") return "分叉需要先合并，建议复制诊断文本交给 AI 处理";
  if (p.status === "no-remote") return noRemoteReason(p);
  return "没有需要提交的改动";
}

/** detached HEAD 时的分支名（与 src-tauri/src/status.rs 的 DETACHED 一致） */
export const DETACHED_BRANCH = "(detached HEAD)";

/** "未关联云端"的具体原因（与 src-tauri/src/status.rs 的 no_remote_reason 文案一致） */
function noRemoteReason(p: Project): string {
  if (!p.remoteUrl) return "没有配置远程仓库，无法与 GitHub 同步";
  if (p.branch === DETACHED_BRANCH) return "处于 detached HEAD 状态（不在任何分支上），无法同步";
  if (p.upstream) return `上游分支 ${p.upstream} 在云端已不存在`;
  return `分支 ${p.branch} 没有设置上游分支，不知道该推到哪里`;
}

// ---------------- 自动提交 ----------------

/** 默认的自动提交信息模板（与 src-tauri/src/util.rs 的 DEFAULT_COMMIT_TEMPLATE 一致） */
export const DEFAULT_COMMIT_TEMPLATE = "自动同步：{date} 来自 {host}";

/** 按模板生成提交信息（与 src-tauri/src/util.rs 的 render_commit_message 一致）：
 *  {date} → 本地时间 YYYY-MM-DD HH:mm，{host} → 电脑名；模板为空时用默认模板 */
export function renderCommitMessage(template: string, date: Date, host: string): string {
  const t = template.trim() || DEFAULT_COMMIT_TEMPLATE;
  return t.split("{date}").join(formatDateTime(date)).split("{host}").join(host);
}

/** 文件变更类型的显示 */
export const CHANGE_KIND_META: Record<FileChangeKind, { label: string; className: string }> = {
  M: { label: "修改", className: "text-status-dirty bg-status-dirty/12" },
  A: { label: "新增", className: "text-status-synced bg-status-synced/12" },
  D: { label: "删除", className: "text-status-diverged bg-status-diverged/12" },
  R: { label: "重命名", className: "text-status-ahead bg-status-ahead/12" },
  "?": { label: "未跟踪", className: "text-status-no-remote bg-status-no-remote/12" },
};
