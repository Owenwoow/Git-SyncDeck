import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import * as api from "@/api";
import type { Settings } from "@/types";

/** 只用于启动时避免主题闪烁（index.html 里的内联脚本会读它），真正的设置以 api 为准 */
const THEME_CACHE_KEY = "syncdeck.theme-cache";

interface SettingsContextValue {
  settings: Settings | null;
  updateSettings: (patch: Partial<Settings>) => Promise<void>;
  /** 暂停自动同步到明天 00:00 */
  pauseAutoSync: () => Promise<void>;
  /** 恢复自动同步 */
  resumeAutoSync: () => Promise<void>;
}

const SettingsContext = createContext<SettingsContextValue | null>(null);

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [settings, setSettings] = useState<Settings | null>(null);

  useEffect(() => {
    api.getSettings().then(setSettings);
  }, []);

  // 托盘菜单里暂停 / 恢复自动同步后，后端推送最新设置
  useEffect(() => {
    let disposed = false;
    let off: (() => void) | null = null;
    api.onSettingsChanged(setSettings).then(
      (unlisten) => (disposed ? unlisten() : (off = unlisten)),
      () => undefined,
    );
    return () => {
      disposed = true;
      off?.();
    };
  }, []);

  useEffect(() => {
    if (!settings) return;
    document.documentElement.classList.toggle("dark", settings.theme === "dark");
    try {
      localStorage.setItem(THEME_CACHE_KEY, settings.theme);
    } catch {
      // 忽略
    }
  }, [settings?.theme]);

  const updateSettings = useCallback(async (patch: Partial<Settings>) => {
    setSettings((s) => (s ? { ...s, ...patch } : s)); // 先乐观更新，主题切换无延迟
    setSettings(await api.saveSettings(patch));
  }, []);

  const pauseAutoSync = useCallback(async () => {
    setSettings(await api.pauseAutoSync());
  }, []);

  const resumeAutoSync = useCallback(async () => {
    setSettings(await api.resumeAutoSync());
  }, []);

  return (
    <SettingsContext.Provider value={{ settings, updateSettings, pauseAutoSync, resumeAutoSync }}>
      {children}
    </SettingsContext.Provider>
  );
}

export function useSettings() {
  const ctx = useContext(SettingsContext);
  if (!ctx) throw new Error("useSettings 必须在 SettingsProvider 内使用");
  return ctx;
}

/** 正在生效的暂停：返回暂停到的时间；没暂停或已过期返回 null */
export function activePause(settings: Settings | null, now = Date.now()): Date | null {
  const until = settings?.autoSyncPausedUntil ? new Date(settings.autoSyncPausedUntil) : null;
  return until && until.getTime() > now ? until : null;
}

/** 暂停到的时间怎么显示："今天 18:00" / "明天 00:00" / "10月20日 00:00"（和托盘的写法一致） */
export function describePauseUntil(until: Date, now = new Date()): string {
  const hm = `${String(until.getHours()).padStart(2, "0")}:${String(until.getMinutes()).padStart(2, "0")}`;
  const day = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const tomorrow = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1).getTime();
  if (day(until) === day(now)) return `今天 ${hm}`;
  if (day(until) === tomorrow) return `明天 ${hm}`;
  return `${until.getMonth() + 1}月${until.getDate()}日 ${hm}`;
}
