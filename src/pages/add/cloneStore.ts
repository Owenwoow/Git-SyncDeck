// 克隆进度的状态：放在模块里而不是页面组件里，克隆途中切到别的页面再回来，进度还在。
import { useSyncExternalStore } from "react";
import { toast } from "sonner";
import * as api from "@/api";
import { phaseText } from "@/state/syncProgress";
import type { CloneProgressEvent, CloneRequest, CloneResult, ProgressPhase } from "@/types";

export type CloneRowStatus = "waiting" | "cloning" | "done" | "failed";

export interface CloneRow {
  url: string;
  name: string;
  status: CloneRowStatus;
  phase: ProgressPhase | null;
  percent: number | null;
  error: string | null;
}

export interface CloneState {
  /** 正在克隆（整批还没结束） */
  running: boolean;
  rows: CloneRow[];
}

const IDLE: CloneState = { running: false, rows: [] };

let state: CloneState = IDLE;
const listeners = new Set<() => void>();

function set(next: CloneState) {
  state = next;
  listeners.forEach((l) => l());
}

function patchRow(url: string, patch: Partial<CloneRow>) {
  set({ ...state, rows: state.rows.map((r) => (r.url === url ? { ...r, ...patch } : r)) });
}

function subscribe(l: () => void) {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}

export function useCloneState(): CloneState {
  return useSyncExternalStore(subscribe, () => state);
}

/** 一行的状态文字：等待 / 克隆中 · 下载 45% / 完成 / 失败 */
export function rowText(r: CloneRow): string {
  switch (r.status) {
    case "waiting":
      return "等待";
    case "cloning": {
      const p = phaseText({ phase: r.phase, percent: r.percent });
      return p ? `克隆中 · ${p}` : "克隆中";
    }
    case "done":
      return "完成";
    case "failed":
      return "失败";
  }
}

/** 清掉已结束的结果（克隆进行中不能清） */
export function clearCloneResults() {
  if (!state.running) set(IDLE);
}

function onEvent(e: CloneProgressEvent) {
  if (e.type === "start") patchRow(e.url, { status: "cloning", phase: null, percent: null });
  else if (e.type === "progress") patchRow(e.url, { status: "cloning", phase: e.phase, percent: e.percent });
  else patchRow(e.url, { status: e.ok ? "done" : "failed", error: e.error, percent: e.ok ? 100 : null });
}

function summary(results: CloneResult[]): { ok: number; failed: number } {
  const ok = results.filter((r) => r.ok).length;
  return { ok, failed: results.length - ok };
}

/**
 * 开始克隆这一批仓库。已经在克隆时不接受新的一批。
 * `setCloning` 把"正在克隆"告诉全局（退出确认要用），`onFinished` 在整批结束后调用（刷新主页和列表）
 */
export async function startClone(
  items: CloneRequest[],
  hooks: { setCloning: (on: boolean) => void; onFinished: () => void },
): Promise<void> {
  if (state.running || items.length === 0) return;
  set({
    running: true,
    rows: items.map((i) => ({ url: i.url, name: i.name, status: "waiting", phase: null, percent: null, error: null })),
  });
  hooks.setCloning(true);
  try {
    const results = await api.cloneRepos(items, onEvent);
    // 事件可能比返回值晚到或丢失，以返回的结果为准
    set({
      ...state,
      rows: state.rows.map((row) => {
        const r = results.find((x) => x.url === row.url);
        return r ? { ...row, status: r.ok ? "done" : "failed", error: r.error, percent: r.ok ? 100 : null } : row;
      }),
    });
    const { ok, failed } = summary(results);
    if (failed === 0) toast.success(`已克隆 ${ok} 个仓库并加入监控`);
    else if (ok === 0) toast.error(`${failed} 个仓库克隆失败`, { description: "原因见列表" });
    else toast.warning(`已克隆 ${ok} 个，${failed} 个失败`, { description: "失败的原因见列表" });
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    set({
      ...state,
      rows: state.rows.map((r) =>
        r.status === "done" || r.status === "failed" ? r : { ...r, status: "failed", error: message },
      ),
    });
    toast.error("克隆失败", { description: message });
  } finally {
    set({ ...state, running: false });
    hooks.setCloning(false);
    hooks.onFinished();
  }
}
