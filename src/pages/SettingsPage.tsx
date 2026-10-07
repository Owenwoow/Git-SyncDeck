import { useState } from "react";
import { Check, FolderSearch, Loader2 } from "lucide-react";
import { toast } from "sonner";
import * as api from "@/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { useSettings } from "@/state/settings";
import type { Theme } from "@/types";

/** 设置：默认代码目录、浅色/深色主题 */
export function SettingsPage() {
  const { settings, updateSettings } = useSettings();
  const [picking, setPicking] = useState(false);

  async function pickDir() {
    setPicking(true);
    try {
      const dir = await api.pickDirectory(settings?.defaultCodeDir);
      if (dir) {
        await updateSettings({ defaultCodeDir: dir });
        toast.success("默认代码目录已保存", { description: dir });
      }
    } finally {
      setPicking(false);
    }
  }

  return (
    <div className="max-w-2xl space-y-8">
      <header>
        <h1 className="text-xl font-semibold tracking-tight">设置</h1>
      </header>

      <section className="space-y-3">
        <h2 className="text-sm font-medium text-muted-foreground">常规</h2>
        <div className="rounded-lg border bg-card p-4">
          <div className="text-sm font-medium">默认代码目录</div>
          <p className="mt-0.5 text-xs text-muted-foreground">"添加项目"页面会默认扫描这个目录。</p>
          <div className="mt-3 flex gap-2">
            <Input readOnly value={settings?.defaultCodeDir ?? ""} className="font-mono select-text" />
            <Button variant="outline" onClick={pickDir} disabled={picking || !settings}>
              {picking ? <Loader2 className="animate-spin" /> : <FolderSearch />}
              选择…
            </Button>
          </div>
        </div>
      </section>

      <section className="space-y-3">
        <h2 className="text-sm font-medium text-muted-foreground">外观</h2>
        <div className="rounded-lg border bg-card p-4">
          <div className="text-sm font-medium">主题</div>
          <div className="mt-3 grid grid-cols-2 gap-3" role="radiogroup" aria-label="主题">
            <ThemeCard
              theme="light"
              label="浅色"
              active={settings?.theme === "light"}
              onSelect={() => updateSettings({ theme: "light" })}
            />
            <ThemeCard
              theme="dark"
              label="深色"
              active={settings?.theme === "dark"}
              onSelect={() => updateSettings({ theme: "dark" })}
            />
          </div>
        </div>
      </section>
    </div>
  );
}

function ThemeCard({
  theme,
  label,
  active,
  onSelect,
}: {
  theme: Theme;
  label: string;
  active: boolean;
  onSelect: () => void;
}) {
  const dark = theme === "dark";
  return (
    <button
      type="button"
      role="radio"
      aria-checked={active}
      onClick={onSelect}
      className={cn(
        "group rounded-lg border p-2 text-left transition-colors",
        active ? "border-foreground/40 ring-2 ring-foreground/15" : "hover:border-foreground/20",
      )}
    >
      {/* 迷你预览：固定颜色，不随当前主题变化 */}
      <div
        className={cn(
          "flex h-20 overflow-hidden rounded-md border",
          dark ? "border-neutral-700 bg-neutral-900" : "border-neutral-200 bg-white",
        )}
      >
        <div className={cn("w-1/4 space-y-1 p-1.5", dark ? "bg-neutral-800" : "bg-neutral-100")}>
          {[0, 1, 2].map((i) => (
            <div key={i} className={cn("h-1.5 rounded-full", dark ? "bg-neutral-600" : "bg-neutral-300")} />
          ))}
        </div>
        <div className="flex-1 space-y-1.5 p-2">
          <div className={cn("h-2 w-1/2 rounded-full", dark ? "bg-neutral-500" : "bg-neutral-300")} />
          {["bg-emerald-500", "bg-blue-500", "bg-amber-500"].map((c) => (
            <div key={c} className="flex items-center gap-1.5">
              <div className={cn("h-1.5 flex-1 rounded-full", dark ? "bg-neutral-700" : "bg-neutral-200")} />
              <div className={cn("h-1.5 w-4 rounded-full opacity-70", c)} />
            </div>
          ))}
        </div>
      </div>
      <div className="mt-2 flex items-center justify-between px-0.5">
        <span className="text-sm">{label}</span>
        {active && <Check className="size-4" />}
      </div>
    </button>
  );
}
