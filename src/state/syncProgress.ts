// 一轮同步（一键同步或后台同步）的进度：按进度事件更新。
// 规则和 src-tauri/src/model.rs 的 SyncProgressSnapshot::apply 一致，改一边要改另一边。
import type {
  ActiveSync,
  AutoSyncEvent,
  AutoSyncTrigger,
  GitOp,
  ProgressPhase,
  Project,
  SyncItemResult,
  SyncProgressEvent,
  SyncProgressSnapshot,
} from "@/types";

export interface ActiveProgress extends ActiveSync {
  /** 这个项目到现在的完成度（0 ~ 1，只增不减，算总进度用） */
  fraction: number;
}

export interface SyncState {
  phase: "idle" | "running" | "done";
  open: boolean;
  /** 这一轮的项目（按处理顺序） */
  order: string[];
  /** 开始处理时各项目的原始状态（用来显示"正在推送 2 个提交"之类的文字） */
  before: Record<string, Pick<Project, "status" | "ahead" | "behind">>;
  /** 正在处理的项目（最多 8 个并行）和它们的 git 进度 */
  active: Record<string, ActiveProgress>;
  results: Record<string, SyncItemResult>;
  /** 结果里点了"本次跳过"的项目 */
  skipped: string[];
  /** 结果里通过"提交并推送"解决了的项目 */
  resolved: string[];
  /** 后台同步：等网络重试 */
  retry: { projectIds: string[]; retryAt: string } | null;
  /** 后台同步的触发原因和文字（如"解锁后同步"）；一键同步为 null */
  trigger: AutoSyncTrigger | null;
  label: string | null;
  /** 结束时间（ISO）；还没结束为 null */
  finishedAt: string | null;
}

export const idleSync: SyncState = {
  phase: "idle",
  open: false,
  order: [],
  before: {},
  active: {},
  results: {},
  skipped: [],
  resolved: [],
  retry: null,
  trigger: null,
  label: null,
  finishedAt: null,
};

type Brief = SyncState["before"][string];

function brief(p: Project | undefined): Brief | null {
  return p && p.status !== "syncing" ? { status: p.status, ahead: p.ahead, behind: p.behind } : null;
}

/** 某个阶段的百分比折算成这个项目的完成度：传输占 90%，处理差异、写出文件占最后 10% */
function phaseFraction(phase: ProgressPhase | null, percent: number | null): number {
  const p = Math.min(Math.max(percent ?? 0, 0), 100) / 100;
  switch (phase) {
    case "uploading":
    case "downloading":
      return 0.05 + 0.85 * p;
    case "resolving":
    case "checkout":
      return 0.9 + 0.1 * p;
    case "preparing":
      return 0.05;
    default:
      return 0;
  }
}

/** 按一条进度事件更新。`latest` 是界面上的最新列表（记下开始时的状态） */
export function applyEvent(s: SyncState, e: SyncProgressEvent, latest: (id: string) => Project | undefined): SyncState {
  switch (e.type) {
    case "begin": {
      const before = { ...s.before };
      for (const id of e.projectIds) {
        const b = brief(latest(id));
        if (b && !before[id]) before[id] = b;
      }
      if (s.retry) {
        // 等网络重试后接着这一轮：只把要重试的项目改回没完成
        const results = { ...s.results };
        for (const id of e.projectIds) {
          delete results[id];
          before[id] = brief(latest(id)) ?? before[id];
        }
        const order = [...s.order, ...e.projectIds.filter((id) => !s.order.includes(id))];
        return { ...s, retry: null, results, before, order };
      }
      return { ...s, order: e.projectIds, before, active: {}, results: {}, retry: null };
    }
    case "start": {
      const b = brief(latest(e.projectId));
      return {
        ...s,
        order: s.order.includes(e.projectId) ? s.order : [...s.order, e.projectId],
        before: b ? { ...s.before, [e.projectId]: b } : s.before,
        active: { ...s.active, [e.projectId]: { projectId: e.projectId, op: null, phase: null, percent: null, fraction: 0 } },
      };
    }
    case "progress": {
      const prev = s.active[e.projectId];
      const fraction = Math.max(prev?.fraction ?? 0, phaseFraction(e.phase, e.percent));
      return {
        ...s,
        active: {
          ...s.active,
          [e.projectId]: { projectId: e.projectId, op: e.op, phase: e.phase, percent: e.percent, fraction },
        },
      };
    }
    case "done": {
      const active = { ...s.active };
      delete active[e.result.projectId];
      return { ...s, active, results: { ...s.results, [e.result.projectId]: e.result } };
    }
    case "retry":
      return { ...s, retry: { projectIds: e.projectIds, retryAt: e.retryAt } };
  }
}

/** 后台同步开始：一轮新的进度（项目清单等 begin 事件或后端的进度快照） */
export function backgroundStarted(e: AutoSyncEvent): SyncState {
  return { ...idleSync, phase: "running", trigger: e.trigger, label: e.label };
}

/** 窗口中途打开：用后端记下的进度快照补上正在进行的后台同步 */
export function fromSnapshot(
  e: AutoSyncEvent,
  snap: SyncProgressSnapshot | null,
  latest: (id: string) => Project | undefined,
): SyncState {
  let s = backgroundStarted(e);
  if (!snap) return s;
  s = applyEvent(s, { type: "begin", projectIds: snap.projectIds, total: snap.projectIds.length }, latest);
  for (const r of snap.results) s = { ...s, results: { ...s.results, [r.projectId]: r } };
  for (const a of snap.active) {
    s = applyEvent(s, { type: "start", projectId: a.projectId, index: 0, total: 0 }, latest);
    if (a.op && a.phase && a.percent !== null) {
      s = applyEvent(s, { type: "progress", projectId: a.projectId, op: a.op, phase: a.phase, percent: a.percent }, latest);
    }
  }
  return snap.retry ? { ...s, retry: snap.retry } : s;
}

/** 后台同步结束：结果以后端给的为准 */
export function backgroundFinished(s: SyncState | null, e: AutoSyncEvent): SyncState | null {
  if (!e.result) return null;
  const base = s && s.trigger === e.trigger ? s : { ...idleSync, trigger: e.trigger, label: e.label };
  const results = Object.fromEntries(e.result.items.map((r) => [r.projectId, r]));
  const order = [...base.order, ...e.result.items.map((r) => r.projectId).filter((id) => !base.order.includes(id))];
  return { ...base, phase: "done", order, results, active: {}, retry: null, finishedAt: e.result.finishedAt };
}

/** 已完成的项目数、这一轮的项目数、总进度（0 ~ 1）：（已完成 + 正在处理的各自完成度）÷ 总数 */
export function overall(s: SyncState): { done: number; total: number; fraction: number } {
  const total = s.order.length;
  const done = s.order.filter((id) => s.results[id]).length;
  const partial = Object.values(s.active)
    .filter((a) => !s.results[a.projectId])
    .reduce((sum, a) => sum + a.fraction, 0);
  return { done, total, fraction: total ? Math.min((done + partial) / total, 1) : 0 };
}

const PHASE_TEXT: Record<ProgressPhase, string> = {
  preparing: "准备中",
  uploading: "上传",
  downloading: "下载",
  resolving: "处理差异",
  checkout: "写出文件",
};

/** git 进度的文字：上传 45% / 准备中；还没有进度时为空 */
export function phaseText(a: Pick<ActiveSync, "phase" | "percent"> | undefined): string {
  if (!a?.phase) return "";
  if (a.phase === "preparing" || a.percent === null) return PHASE_TEXT[a.phase];
  return `${PHASE_TEXT[a.phase]} ${a.percent}%`;
}

const OP_VERB: Record<GitOp, string> = {
  fetch: "正在检查云端",
  push: "正在推送",
  pull: "正在拉取",
  clone: "正在克隆",
};

/** 正在处理的项目在做什么："正在推送 3 个提交 · 上传 45%" */
export function activeText(
  before: Brief | undefined,
  a: ActiveProgress | undefined,
  autoCommit: boolean,
  autoMerge: boolean,
): string {
  if (a?.op) {
    // 待拉取的项目：要下载的东西在检查云端的 fetch 里下载，也算"正在拉取"
    const pulling = (a.op === "pull" || a.op === "fetch") && before?.status === "behind";
    const verb =
      a.op === "push" && before?.status === "ahead"
        ? `正在推送 ${before.ahead} 个提交`
        : pulling
          ? `正在拉取 ${before.behind} 个提交`
          : OP_VERB[a.op];
    const phase = phaseText(a);
    return phase ? `${verb} · ${phase}` : `${verb}…`;
  }
  switch (before?.status) {
    case "dirty":
      return autoCommit ? "正在自动提交并推送…" : "正在检查…";
    case "ahead":
      return `正在推送 ${before.ahead} 个提交…`;
    case "behind":
      return `正在拉取 ${before.behind} 个提交…`;
    case "synced":
      return "正在检查云端…";
    case "diverged":
      return autoMerge ? "正在预演合并…" : "正在检查…";
    default:
      return "正在检查…";
  }
}

/** 标题下一行："正在推送 Git-SyncDeck 上传 45% 等 3 个" / "等待网络，15 秒后重试 2 个项目" / "准备中…" */
export function headline(s: SyncState, nameOf: (id: string) => string | undefined, now: number): string {
  if (s.retry) {
    const secs = Math.max(0, Math.ceil((Date.parse(s.retry.retryAt) - now) / 1000));
    return `等待网络，${secs} 秒后重试 ${s.retry.projectIds.length} 个项目`;
  }
  const active = s.order.map((id) => s.active[id]).filter((a): a is ActiveProgress => !!a && !s.results[a.projectId]);
  if (!active.length) return s.order.length ? "正在处理…" : "准备中…";
  // 优先写有百分比的那个
  const first = active.find((a) => a.op) ?? active[0];
  const name = nameOf(first.projectId) ?? "";
  const verb = !first.op
    ? "正在处理"
    : first.op === "fetch" && s.before[first.projectId]?.status === "behind"
      ? OP_VERB.pull
      : OP_VERB[first.op];
  const phase = phaseText(first);
  const more = active.length > 1 ? ` 等 ${active.length} 个` : "";
  return `${verb} ${name}${phase ? ` ${phase}` : ""}${more}`;
}

/** 触发原因的短说法："解锁后同步" → "解锁后"，"立即同步" 不变 */
export function shortLabel(label: string | null): string {
  if (!label) return "";
  return /[后时]同步$/.test(label) ? label.slice(0, -2) : label;
}

/** 结果摘要："已推送 2 个 · 1 个需要处理" / "都已是最新" */
export function resultSummary(s: SyncState): string {
  const results = s.order.map((id) => s.results[id]).filter((r): r is SyncItemResult => !!r);
  const bad = results.filter((r) => r.outcome === "failed" && !s.resolved.includes(r.projectId)).length;
  const changed = results.filter((r) => r.outcome !== "failed" && r.outcome !== "up-to-date");
  const pushedOnly = changed.every((r) => r.outcome !== "pulled");
  const pulledOnly = changed.every((r) => r.outcome === "pulled");
  const parts: string[] = [];
  if (changed.length) {
    parts.push(`${pushedOnly ? "已推送" : pulledOnly ? "已拉取" : "推送或拉取了"} ${changed.length} 个`);
  } else if (!bad) {
    parts.push(results.length ? "都已是最新" : "没有要同步的项目");
  }
  if (bad) parts.push(`${bad} 个需要处理`);
  return parts.join(" · ");
}
