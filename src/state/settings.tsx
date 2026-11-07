import { createContext, useCallback, useContext, useEffect, useState, useSyncExternalStore, type ReactNode } from "react";
import * as api from "@/api";
import type { Settings } from "@/types";
import { SettingsQueue, type SaveFieldState } from "./settingsQueue";

const THEME_CACHE_KEY = "syncdeck.theme-cache";

interface SettingsContextValue {
  settings: Settings | null;
  loading: boolean;
  saving: boolean;
  saveStatus: "idle" | "saving" | "saved" | "error";
  error: string | null;
  readError: string | null;
  fieldStates: Partial<Record<keyof Settings, SaveFieldState>>;
  updateSettings: (patch: Partial<Settings>) => Promise<void>;
  reloadSettings: () => Promise<void>;
  retryFailedSave: () => Promise<void>;
  discardFailedSave: (fields: (keyof Settings)[]) => void;
  pauseAutoSync: (duration?: "hour" | "tomorrow") => Promise<void>;
  resumeAutoSync: () => Promise<void>;
}

const SettingsContext = createContext<SettingsContextValue | null>(null);

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [queue] = useState(() => new SettingsQueue<Settings>(api.getSettings, api.saveSettings));
  const snapshot = useSyncExternalStore(queue.subscribe, queue.getSnapshot);
  const settings = snapshot.value;

  useEffect(() => { void queue.reload().catch(() => {}); }, [queue]);

  useEffect(() => {
    let disposed = false;
    let off: (() => void) | null = null;
    api.onSettingsChanged(queue.receive).then(
      (unlisten) => (disposed ? unlisten() : (off = unlisten)),
      () => undefined,
    );
    return () => { disposed = true; off?.(); };
  }, [queue]);

  useEffect(() => {
    if (!settings) return;
    document.documentElement.classList.toggle("dark", settings.theme === "dark");
    try { localStorage.setItem(THEME_CACHE_KEY, settings.theme); } catch { /* 主题缓存不影响设置写盘。 */ }
  }, [settings?.theme]);

  const pauseAutoSync = useCallback((duration: "hour" | "tomorrow" = "tomorrow") => {
    const until = new Date();
    if (duration === "hour") until.setTime(until.getTime() + 60 * 60 * 1000);
    else until.setHours(24, 0, 0, 0);
    return queue.change({ autoSyncPausedUntil: until.toISOString() }, () => api.pauseAutoSync(duration));
  }, [queue]);
  const resumeAutoSync = useCallback(() => queue.change({ autoSyncPausedUntil: null }, api.resumeAutoSync), [queue]);

  const saveStatus = snapshot.saving ? "saving" : snapshot.error ? "error"
    : Object.values(snapshot.fieldStates).includes("saved") ? "saved" : "idle";
  return (
    <SettingsContext.Provider value={{
      settings, loading: snapshot.loading, saving: snapshot.saving, saveStatus,
      error: snapshot.error, readError: snapshot.readError, fieldStates: snapshot.fieldStates,
      updateSettings: queue.save, reloadSettings: queue.reload, retryFailedSave: queue.retry, discardFailedSave: queue.discardFailed,
      pauseAutoSync, resumeAutoSync,
    }}>
      {children}
    </SettingsContext.Provider>
  );
}

export function useSettings() {
  const ctx = useContext(SettingsContext);
  if (!ctx) throw new Error("useSettings 必须在 SettingsProvider 内使用");
  return ctx;
}

/** 正在生效的暂停：返回暂停到的本地时间；没暂停或已过期返回 null。 */
export function activePause(settings: Settings | null, now = Date.now()): Date | null {
  const until = settings?.autoSyncPausedUntil ? new Date(settings.autoSyncPausedUntil) : null;
  return until && until.getTime() > now ? until : null;
}

export function describePauseUntil(until: Date, now = new Date()): string {
  const hm = `${String(until.getHours()).padStart(2, "0")}:${String(until.getMinutes()).padStart(2, "0")}`;
  const day = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const tomorrow = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1).getTime();
  if (day(until) === day(now)) return `今天 ${hm}`;
  if (day(until) === tomorrow) return `明天 ${hm}`;
  return `${until.getMonth() + 1}月${until.getDate()}日 ${hm}`;
}
