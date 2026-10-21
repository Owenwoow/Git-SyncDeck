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
import type { AutoSyncEvent, Project, SyncProgressEvent, SyncResult, SyncStatus } from "@/types";
import {
  applyEvent,
  backgroundFinished,
  backgroundStarted,
  fromSnapshot,
  idleSync,
  type SyncState,
} from "./syncProgress";

export type { SyncState } from "./syncProgress";

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
  /** 窗口里的一键同步 */
  sync: SyncState;
  startSync: () => Promise<void>;
  /** 打开 / 关掉一键同步的进度或结果弹窗（进行中也能关，同步照常进行） */
  openSync: () => void;
  closeSync: () => void;
  skipInSync: (id: string) => void;
  /** 正在进行的后台自动同步（锁屏、解锁、托盘立即同步……）；null 表示没有 */
  autoSync: AutoSyncEvent | null;
  /** 后台同步：正在进行的那一轮；没在进行时是最近一次的结果（只在内存里）；都没有为 null */
  background: SyncState | null;
  backgroundOpen: boolean;
  openBackground: () => void;
  closeBackground: () => void;
  skipInBackground: (id: string) => void;
  /** 正在从 GitHub 克隆仓库；由「添加项目」页的克隆标签页设置 */
  cloning: boolean;
  setCloning: (on: boolean) => void;
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
  /** 一键同步的弹窗现在是否开着（同步结束时决定要不要提示"查看结果"） */
  const syncOpenRef = useRef(false);
  useEffect(() => {
    syncOpenRef.current = sync.open;
  }, [sync.open]);
  const [autoSync, setAutoSync] = useState<AutoSyncEvent | null>(null);
  /** 「添加项目 → 从 GitHub 克隆」正在克隆（退出前要确认） */
  const [cloning, setCloning] = useState(false);
  /** 正在进行的后台同步的进度、最近一次后台同步的结果、后台同步弹窗是否打开 */
  const [bgRun, setBgRun] = useState<SyncState | null>(null);
  const [bgLast, setBgLast] = useState<SyncState | null>(null);
  const [backgroundOpen, setBackgroundOpen] = useState(false);
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

  const latest = useCallback((id: string) => projectsRef.current.find((p) => p.id === id), []);

  /** 进度事件里和列表有关的部分：开始处理的那一行显示"同步中"，处理完换成最新状态 */
  const applyToList = useCallback(
    (e: SyncProgressEvent) => {
      if (e.type === "start") setStatus(e.projectId, "syncing");
      else if (e.type === "done") replaceProject(e.project);
    },
    [setStatus, replaceProject],
  );

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
          started: (e) => {
            setAutoSync(e);
            setBgRun(backgroundStarted(e));
          },
          progress: (e) => {
            // 先记下开始时的状态，再把那一行改成"同步中"
            setBgRun((cur) => (cur ? applyEvent(cur, e, latest) : cur));
            applyToList(e);
          },
          finished: (e) => {
            // 只清掉同一轮的
            setAutoSync((cur) => (cur && cur.trigger !== e.trigger ? cur : null));
            setBgRun((cur) => {
              const done = backgroundFinished(cur, e);
              if (done) setBgLast(done);
              return null;
            });
          },
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
      const auto = await api
        .getAutoSyncState()
        .catch(() => ({ running: null, startupSync: false, progress: null, last: null }));
      setAutoSync((cur) => cur ?? auto.running);
      // 窗口中途打开：补上正在进行的后台同步的进度和最近一次的结果（已经从事件收到的为准）
      const running = auto.running;
      if (running) setBgRun((cur) => (cur && cur.order.length ? cur : fromSnapshot(running, auto.progress, latest)));
      const last = auto.last;
      if (last) setBgLast((cur) => cur ?? backgroundFinished(null, last));
      if (git.installed && !auto.startupSync) {
        await refresh().catch(() => undefined);
      } else if (auto.startupSync && !auto.running) {
        // 兜底：后端的启动同步已经结束（或还没开始），再读一次列表，不让列表停在同步前的状态；
        // 还在进行的话，结束时会推 projects-changed
        await reload();
      }
    })();
  }, [reload, refresh, latest, applyToList]);

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

    let result: SyncResult | null = null;
    try {
      // 刷新途中点的一键同步：后端等刷新结束才开始，这时用刷新后的状态显示"正在推送 N 个提交"
      result = await api.syncAll((e) => {
        setSync((s) => applyEvent(s, e, latest));
        applyToList(e);
      });
    } catch (e) {
      toast.error("一键同步失败", { description: errorMessage(e) });
      await reload();
    }

    setSync((s) => ({ ...s, phase: "done", active: {}, finishedAt: new Date().toISOString() }));
    // 弹窗关着时同步做完了：提示一下，可以点开看结果
    if (result && !syncOpenRef.current) {
      const bad = result.failedCount;
      const text = `一键同步完成：${result.syncedCount} 个已同步${bad ? `，${bad} 个需要处理` : ""}`;
      const action = { label: "查看结果", onClick: () => setSync((cur) => ({ ...cur, open: true })) };
      if (bad) toast.warning(text, { action, duration: 10000 });
      else toast.success(text, { action });
    }
  }, [projects, latest, applyToList, reload]);

  const openSync = useCallback(() => {
    setSync((s) => (s.phase === "idle" ? s : { ...s, open: true }));
  }, []);

  const closeSync = useCallback(() => {
    setSync((s) => ({ ...s, open: false }));
  }, []);

  const openBackground = useCallback(() => setBackgroundOpen(true), []);
  const closeBackground = useCallback(() => setBackgroundOpen(false), []);
  const skipInBackground = useCallback((id: string) => {
    setBgLast((s) => (!s || s.skipped.includes(id) ? s : { ...s, skipped: [...s.skipped, id] }));
  }, []);

  const skipInSync = useCallback((id: string) => {
    setSync((s) => (s.skipped.includes(id) ? s : { ...s, skipped: [...s.skipped, id] }));
  }, []);

  const commitAndPush = useCallback(
    async (id: string, message: string) => {
      try {
        const updated = await api.commitAndPush(id, message);
        replaceProject(updated);
        const markResolved = (s: SyncState) =>
          s.results[id] && !s.resolved.includes(id) ? { ...s, resolved: [...s.resolved, id] } : s;
        setSync(markResolved);
        setBgLast((s) => (s ? markResolved(s) : s));
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
      openSync,
      closeSync,
      skipInSync,
      autoSync,
      background: bgRun ?? bgLast,
      backgroundOpen,
      openBackground,
      closeBackground,
      skipInBackground,
      cloning,
      setCloning,
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
      openSync,
      closeSync,
      skipInSync,
      autoSync,
      bgRun,
      bgLast,
      backgroundOpen,
      openBackground,
      closeBackground,
      skipInBackground,
      cloning,
    ],
  );

  return <ProjectsContext.Provider value={value}>{children}</ProjectsContext.Provider>;
}

export function useProjects() {
  const ctx = useContext(ProjectsContext);
  if (!ctx) throw new Error("useProjects 必须在 ProjectsProvider 内使用");
  return ctx;
}
