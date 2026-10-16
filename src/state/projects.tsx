import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { toast } from "sonner";
import * as api from "@/api";
import { sortProjects } from "@/lib/status";
import type { AutoSyncEvent, Project, SyncItemResult, SyncStatus } from "@/types";

export interface SyncState {
  phase: "idle" | "running" | "done";
  open: boolean;
  /** 同步开始时的处理顺序 */
  order: string[];
  /** 同步开始时各项目的原始状态（用来显示"正在推送 2 个提交"之类的文字） */
  before: Record<string, Pick<Project, "status" | "ahead" | "behind">>;
  /** 正在处理的项目（最多 8 个并行） */
  activeIds: string[];
  doneCount: number;
  results: Record<string, SyncItemResult>;
  /** 结果里点了"本次跳过"的项目 */
  skipped: string[];
  /** 结果里通过"提交并推送"解决了的项目 */
  resolved: string[];
}

const idleSync: SyncState = {
  phase: "idle",
  open: false,
  order: [],
  before: {},
  activeIds: [],
  doneCount: 0,
  results: {},
  skipped: [],
  resolved: [],
};

interface ProjectsContextValue {
  projects: Project[];
  loading: boolean;
  refreshing: boolean;
  /** 本轮刷新里还没检查完的项目 id（每个项目检查完就移出） */
  checkingIds: ReadonlySet<string>;
  reload: () => Promise<void>;
  /** 联网刷新所有项目的状态（每个项目检查完就更新对应行），返回刷新后的列表 */
  refresh: () => Promise<Project[]>;
  commitAndPush: (id: string, message: string) => Promise<Project>;
  /** 打开 / 关闭"一键同步时自动提交" */
  setAutoCommit: (id: string, enabled: boolean) => Promise<Project>;
  /** 取消监控：只从清单里移除，不动任何文件 */
  removeProject: (id: string) => Promise<void>;
  sync: SyncState;
  startSync: () => Promise<void>;
  closeSync: () => void;
  skipInSync: (id: string) => void;
  /** 正在进行的后台自动同步（锁屏、解锁、托盘立即同步……）；null 表示没有 */
  autoSync: AutoSyncEvent | null;
}

const ProjectsContext = createContext<ProjectsContextValue | null>(null);

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

const NO_IDS: ReadonlySet<string> = new Set();

/** 用刷新结果更新列表：只更新列表里还在的项目（刷新途中取消监控的不再加回来），
 *  一键同步已经开始处理的项目保留"同步中"，等同步结果覆盖；
 *  "自动提交"开关以界面上的为准（刷新途中刚改过的开关不会被刷新结果改回去） */
function applyChecked(list: Project[], checked: Project[]): Project[] {
  const byId = new Map(checked.map((p) => [p.id, p]));
  return list.map((p) => {
    const next = byId.get(p.id);
    return next && p.status !== "syncing" ? { ...next, autoCommit: p.autoCommit } : p;
  });
}

/** 用后台检查 / 同步推来的列表更新：只更新列表里还在的项目，一键同步正在处理的保留"同步中" */
function applyBackground(list: Project[], incoming: Project[]): Project[] {
  const byId = new Map(incoming.map((p) => [p.id, p]));
  return list.map((p) => {
    const next = byId.get(p.id);
    return next && p.status !== "syncing" ? next : p;
  });
}

export function ProjectsProvider({ children }: { children: ReactNode }) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  /** 本轮刷新里已经检查完的项目；null 表示没在刷新 */
  const [checkedIds, setCheckedIds] = useState<ReadonlySet<string> | null>(null);
  /** 刷新轮次：连续触发两次刷新时，只有最后一轮结束才算刷新结束 */
  const refreshRound = useRef(0);
  const [sync, setSync] = useState<SyncState>(idleSync);
  const [autoSync, setAutoSync] = useState<AutoSyncEvent | null>(null);
  /** 最后一次后台推来的列表和序号：读列表（reload）期间收到的后台结果不能被读到的旧数据盖掉 */
  const background = useRef<{ seq: number; list: Project[] | null }>({ seq: 0, list: null });
  const started = useRef(false);
  // 一键同步开始处理某个项目时读取它的最新状态（可能是刷新途中点的一键同步）
  const projectsRef = useRef(projects);
  useEffect(() => {
    projectsRef.current = projects;
  }, [projects]);

  const checkingIds = useMemo<ReadonlySet<string>>(
    () => (checkedIds ? new Set(projects.filter((p) => !checkedIds.has(p.id)).map((p) => p.id)) : NO_IDS),
    [projects, checkedIds],
  );

  const replaceProject = useCallback((next: Project) => {
    setProjects((list) => list.map((p) => (p.id === next.id ? next : p)));
  }, []);

  const setStatus = useCallback((id: string, status: SyncStatus) => {
    setProjects((list) => list.map((p) => (p.id === id ? { ...p, status } : p)));
  }, []);

  const reload = useCallback(async () => {
    const seq = background.current.seq;
    try {
      const list = await api.listProjects();
      // 读列表期间后台推来过新列表（比如启动时的到达同步刚好结束）：以后台的为准，读到的可能是同步前的数据
      const bg = background.current;
      setProjects(bg.seq !== seq && bg.list ? applyBackground(list, bg.list) : list);
    } catch (e) {
      toast.error("读取项目列表失败", { description: errorMessage(e) });
    } finally {
      setLoading(false);
    }
  }, []);

  const refresh = useCallback(async () => {
    const round = ++refreshRound.current;
    setRefreshing(true);
    setCheckedIds(new Set());
    try {
      const list = await api.refreshStatus((p) => {
        setProjects((cur) => applyChecked(cur, [p]));
        if (refreshRound.current === round) setCheckedIds((ids) => (ids ? new Set(ids).add(p.id) : ids));
      });
      setProjects((cur) => applyChecked(cur, list));
      return list;
    } finally {
      if (refreshRound.current === round) {
        setRefreshing(false);
        setCheckedIds(null);
      }
    }
  }, []);

  // 启动：挂上后台事件的监听 → 检测 git → 先显示本地状态（不联网）→ 联网刷新一次。
  // 打开了"启动时同步"时，启动的到达同步由后端做：前端只显示本地状态，等后端推结果，避免同时跑两轮。
  // 监听在整个应用期间都有效（这个 Provider 不会卸载），所以不取消
  useEffect(() => {
    if (started.current) return; // 开发模式下 StrictMode 会执行两次 effect，避免重复 fetch
    started.current = true;
    (async () => {
      // 先挂监听再读列表：后台同步刚好在这之间结束也不会漏掉结果
      await Promise.all([
        api.onProjectsChanged((list) => {
          background.current = { seq: background.current.seq + 1, list };
          setProjects((cur) => applyBackground(cur, list));
        }),
        api.onAutoSync({
          started: setAutoSync,
          // 只清掉同一轮的（关机同步可能顶掉正在进行的那一轮）
          finished: (e) => setAutoSync((cur) => (cur && cur.trigger !== e.trigger ? cur : null)),
        }),
      ]).catch(() => undefined);
      const git = await api.checkGit().catch(() => ({ installed: false, version: null }));
      if (!git.installed) {
        toast.error("未检测到 Git", {
          description: "请先安装 Git for Windows（https://git-scm.com/download/win），安装后重新打开本工具。",
          duration: Infinity,
        });
      }
      await reload();
      const auto = await api.getAutoSyncState().catch(() => ({ running: null, startupSync: false }));
      setAutoSync((cur) => cur ?? auto.running);
      if (git.installed && !auto.startupSync) {
        await refresh().catch(() => undefined);
      } else if (auto.startupSync && !auto.running) {
        // 兜底：后端的启动同步已经结束（或还没开始），再读一次列表，不让列表停在同步前的状态；
        // 还在进行的话，结束时会推 projects-changed
        await reload();
      }
    })();
  }, [reload, refresh]);

  const startSync = useCallback(async () => {
    const ordered = sortProjects(projects);
    setSync({
      ...idleSync,
      phase: "running",
      open: true,
      order: ordered.map((p) => p.id),
      before: Object.fromEntries(
        ordered.map((p) => [p.id, { status: p.status, ahead: p.ahead, behind: p.behind }]),
      ),
    });

    try {
      await api.syncAll((e) => {
        if (e.type === "start") {
          // 刷新途中点的一键同步：后端等刷新结束才开始，这时用刷新后的状态显示"正在推送 N 个提交"
          const latest = projectsRef.current.find((p) => p.id === e.projectId);
          setStatus(e.projectId, "syncing");
          setSync((s) => ({
            ...s,
            activeIds: [...s.activeIds, e.projectId],
            before:
              latest && latest.status !== "syncing"
                ? { ...s.before, [e.projectId]: { status: latest.status, ahead: latest.ahead, behind: latest.behind } }
                : s.before,
          }));
        } else {
          replaceProject(e.project);
          setSync((s) => ({
            ...s,
            activeIds: s.activeIds.filter((id) => id !== e.result.projectId),
            doneCount: e.index + 1,
            results: { ...s.results, [e.result.projectId]: e.result },
          }));
        }
      });
    } catch (e) {
      toast.error("一键同步失败", { description: errorMessage(e) });
      await reload();
    }

    setSync((s) => ({ ...s, phase: "done", activeIds: [] }));
  }, [projects, replaceProject, setStatus, reload]);

  const closeSync = useCallback(() => {
    setSync((s) => (s.phase === "running" ? s : { ...s, open: false }));
  }, []);

  const skipInSync = useCallback((id: string) => {
    setSync((s) => (s.skipped.includes(id) ? s : { ...s, skipped: [...s.skipped, id] }));
  }, []);

  const commitAndPush = useCallback(
    async (id: string, message: string) => {
      try {
        const updated = await api.commitAndPush(id, message);
        replaceProject(updated);
        setSync((s) =>
          s.results[id] && !s.resolved.includes(id) ? { ...s, resolved: [...s.resolved, id] } : s,
        );
        return updated;
      } catch (e) {
        // 可能已经提交到本地但没推送（比如发现落后于云端），重新读取一次状态
        await reload();
        throw e;
      }
    },
    [replaceProject, reload],
  );

  const setAutoCommit = useCallback(async (id: string, enabled: boolean) => {
    const updated = await api.setAutoCommit(id, enabled);
    // 一键同步进行中时保留"同步中"的显示，结束时会被最新结果覆盖
    setProjects((list) =>
      list.map((p) => (p.id === id ? { ...updated, status: p.status === "syncing" ? p.status : updated.status } : p)),
    );
    return updated;
  }, []);

  const removeProject = useCallback(async (id: string) => {
    await api.removeProject(id);
    setProjects((list) => list.filter((p) => p.id !== id));
  }, []);

  const value = useMemo(
    () => ({
      projects,
      loading,
      refreshing,
      checkingIds,
      reload,
      refresh,
      commitAndPush,
      setAutoCommit,
      removeProject,
      sync,
      startSync,
      closeSync,
      skipInSync,
      autoSync,
    }),
    [
      projects,
      loading,
      refreshing,
      checkingIds,
      reload,
      refresh,
      commitAndPush,
      setAutoCommit,
      removeProject,
      sync,
      startSync,
      closeSync,
      skipInSync,
      autoSync,
    ],
  );

  return <ProjectsContext.Provider value={value}>{children}</ProjectsContext.Provider>;
}

export function useProjects() {
  const ctx = useContext(ProjectsContext);
  if (!ctx) throw new Error("useProjects 必须在 ProjectsProvider 内使用");
  return ctx;
}
