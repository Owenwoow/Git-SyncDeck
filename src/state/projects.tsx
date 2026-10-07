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
import type { Project, SyncItemResult, SyncStatus } from "@/types";

export interface SyncState {
  phase: "idle" | "running" | "done";
  open: boolean;
  /** 同步开始时的处理顺序 */
  order: string[];
  /** 同步开始时各项目的原始状态（用来显示"正在推送 2 个提交"之类的文字） */
  before: Record<string, Pick<Project, "status" | "ahead" | "behind">>;
  /** 正在处理的项目（最多 4 个并行） */
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
  reload: () => Promise<void>;
  /** 联网刷新所有项目的状态，返回刷新后的列表 */
  refresh: () => Promise<Project[]>;
  commitAndPush: (id: string, message: string) => Promise<Project>;
  sync: SyncState;
  startSync: () => Promise<void>;
  closeSync: () => void;
  skipInSync: (id: string) => void;
}

const ProjectsContext = createContext<ProjectsContextValue | null>(null);

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export function ProjectsProvider({ children }: { children: ReactNode }) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [sync, setSync] = useState<SyncState>(idleSync);
  const started = useRef(false);

  const replaceProject = useCallback((next: Project) => {
    setProjects((list) => list.map((p) => (p.id === next.id ? next : p)));
  }, []);

  const setStatus = useCallback((id: string, status: SyncStatus) => {
    setProjects((list) => list.map((p) => (p.id === id ? { ...p, status } : p)));
  }, []);

  const reload = useCallback(async () => {
    try {
      setProjects(await api.listProjects());
    } catch (e) {
      toast.error("读取项目列表失败", { description: errorMessage(e) });
    } finally {
      setLoading(false);
    }
  }, []);

  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const list = await api.refreshStatus();
      setProjects(list);
      return list;
    } finally {
      setRefreshing(false);
    }
  }, []);

  // 启动：检测 git → 先显示本地状态（不联网）→ 后台自动联网刷新一次
  useEffect(() => {
    if (started.current) return; // 开发模式下 StrictMode 会执行两次 effect，避免重复 fetch
    started.current = true;
    (async () => {
      const git = await api.checkGit().catch(() => ({ installed: false, version: null }));
      if (!git.installed) {
        toast.error("未检测到 Git", {
          description: "请先安装 Git for Windows（https://git-scm.com/download/win），安装后重新打开本工具。",
          duration: Infinity,
        });
      }
      await reload();
      if (git.installed) await refresh().catch(() => undefined);
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
          setStatus(e.projectId, "syncing");
          setSync((s) => ({ ...s, activeIds: [...s.activeIds, e.projectId] }));
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

  const value = useMemo(
    () => ({
      projects,
      loading,
      refreshing,
      reload,
      refresh,
      commitAndPush,
      sync,
      startSync,
      closeSync,
      skipInSync,
    }),
    [projects, loading, refreshing, reload, refresh, commitAndPush, sync, startSync, closeSync, skipInSync],
  );

  return <ProjectsContext.Provider value={value}>{children}</ProjectsContext.Provider>;
}

export function useProjects() {
  const ctx = useContext(ProjectsContext);
  if (!ctx) throw new Error("useProjects 必须在 ProjectsProvider 内使用");
  return ctx;
}
