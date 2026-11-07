import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import * as api from "@/api";
import type { UpdateInfo, UpdateProgress } from "@/types";

export type UpdatePhase = "idle" | "checking" | "latest" | "available" | "downloading" | "downloaded" | "installing" | "installed" | "error";

interface UpdatesContextValue {
  version: string | null;
  update: UpdateInfo | null;
  phase: UpdatePhase;
  error: string | null;
  progress: UpdateProgress | null;
  downloaded: boolean;
  check: (silent?: boolean) => Promise<UpdateInfo | null>;
  download: () => Promise<void>;
  install: () => Promise<void>;
}

const UpdatesContext = createContext<UpdatesContextValue | null>(null);
const message = (error: unknown) => error instanceof Error ? error.message : String(error);

/** 挂在应用根部，下载、安装与失败状态不会随着设置分类或页面卸载。 */
export function UpdatesProvider({ children }: { children: ReactNode }) {
  const [version, setVersion] = useState<string | null>(null);
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [phase, setPhase] = useState<UpdatePhase>("idle");
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  const [downloaded, setDownloaded] = useState(false);
  const busy = useRef(false);
  const downloadedRef = useRef(false);
  const installedRef = useRef(false);
  const updateRef = useRef<UpdateInfo | null>(null);

  useEffect(() => { api.getAppVersion().then(setVersion, () => setVersion(null)); }, []);

  const check = useCallback(async (_silent = false): Promise<UpdateInfo | null> => {
    if (busy.current || downloadedRef.current || installedRef.current) return updateRef.current;
    busy.current = true;
    setPhase("checking");
    setError(null);
    try {
      const info = await api.checkForUpdate();
      updateRef.current = info;
      setUpdate(info);
      setPhase(info ? "available" : "latest");
      return info;
    } catch (cause) {
      updateRef.current = null;
      setUpdate(null);
      setError(`检查更新失败：${message(cause)}`);
      setPhase("error");
      return null;
    } finally { busy.current = false; }
  }, []);

  const download = useCallback(async () => {
    if (busy.current || downloadedRef.current || installedRef.current || !updateRef.current) return;
    busy.current = true;
    setPhase("downloading");
    setError(null);
    setProgress(null);
    try {
      await api.downloadUpdate(setProgress);
      downloadedRef.current = true;
      setDownloaded(true);
      setPhase("downloaded");
    } catch (cause) {
      setError(`下载更新失败：${message(cause)}`);
      setPhase("error");
    } finally { busy.current = false; }
  }, []);

  const install = useCallback(async () => {
    if (busy.current || !downloadedRef.current || installedRef.current) return;
    busy.current = true;
    setPhase("installing");
    setError(null);
    try {
      // API 内部先在后端原子取得安装资格，并阻止新的仓库任务进入。
      await api.installUpdate();
      installedRef.current = true;
      setPhase("installed");
    } catch (cause) {
      // 保留下载包，空闲后可以重试安装，无需重复下载。
      setError(`安装更新失败：${message(cause)}`);
      setPhase("error");
    } finally { busy.current = false; }
  }, []);

  return (
    <UpdatesContext.Provider value={{ version, update, phase, error, progress, downloaded, check, download, install }}>
      {children}
    </UpdatesContext.Provider>
  );
}

export function useUpdates() {
  const value = useContext(UpdatesContext);
  if (!value) throw new Error("useUpdates 必须在 UpdatesProvider 内使用");
  return value;
}
