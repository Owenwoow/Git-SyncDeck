import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import * as api from "@/api";
import type { Settings } from "@/types";

/** 只用于启动时避免主题闪烁（index.html 里的内联脚本会读它），真正的设置以 api 为准 */
const THEME_CACHE_KEY = "syncdeck.theme-cache";

interface SettingsContextValue {
  settings: Settings | null;
  updateSettings: (patch: Partial<Settings>) => Promise<void>;
}

const SettingsContext = createContext<SettingsContextValue | null>(null);

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [settings, setSettings] = useState<Settings | null>(null);

  useEffect(() => {
    api.getSettings().then(setSettings);
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

  return (
    <SettingsContext.Provider value={{ settings, updateSettings }}>
      {children}
    </SettingsContext.Provider>
  );
}

export function useSettings() {
  const ctx = useContext(SettingsContext);
  if (!ctx) throw new Error("useSettings 必须在 SettingsProvider 内使用");
  return ctx;
}
