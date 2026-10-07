import { useEffect, useState } from "react";
import { Check, CircleAlert, Download, FolderSearch, Loader2, RefreshCw } from "lucide-react";
import { toast } from "sonner";
import * as api from "@/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { cn } from "@/lib/utils";
import { useSettings } from "@/state/settings";
import type { Theme, UpdateInfo, UpdateProgress } from "@/types";

/** 设置：默认代码目录、浅色/深色主题、关于与检查更新。pendingUpdate 是启动时已发现的新版本 */
export function SettingsPage({ pendingUpdate = null }: { pendingUpdate?: UpdateInfo | null }) {
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

      <AboutSection pendingUpdate={pendingUpdate} />
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


// ---------------- 关于与应用更新 ----------------

type UpdatePhase = "idle" | "checking" | "latest" | "available" | "installing" | "error";

function formatBytes(n: number): string {
  if (n >= 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(n / 1024))} KB`;
}

function AboutSection({ pendingUpdate }: { pendingUpdate: UpdateInfo | null }) {
  const [version, setVersion] = useState<string | null>(null);
  const [phase, setPhase] = useState<UpdatePhase>(pendingUpdate ? "available" : "idle");
  const [update, setUpdate] = useState<UpdateInfo | null>(pendingUpdate);
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<UpdateProgress | null>(null);

  useEffect(() => {
    api.getAppVersion().then(setVersion, () => setVersion(null));
  }, []);

  // 启动时的静默检查可能在打开设置页之后才完成
  useEffect(() => {
    if (pendingUpdate) {
      setUpdate(pendingUpdate);
      setPhase((p) => (p === "idle" || p === "latest" ? "available" : p));
    }
  }, [pendingUpdate]);

  async function check() {
    setPhase("checking");
    setError(null);
    try {
      const info = await api.checkForUpdate();
      setUpdate(info);
      setPhase(info ? "available" : "latest");
    } catch (e) {
      setUpdate(null);
      setError(e instanceof Error ? e.message : String(e));
      setPhase("error");
    }
  }

  async function install() {
    setPhase("installing");
    setError(null);
    setProgress(null);
    try {
      await api.installUpdate(setProgress);
      // 真实环境下这里已经重启；演示模式不会真的更新，回到"有新版本"
      setPhase("available");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setPhase("error");
    }
  }

  const percent = progress?.total ? Math.min(100, Math.round((progress.downloaded / progress.total) * 100)) : null;
  const busy = phase === "checking" || phase === "installing";
  const showUpdate = update !== null && (phase === "available" || phase === "installing" || phase === "error");

  return (
    <section className="space-y-3">
      <h2 className="text-sm font-medium text-muted-foreground">关于</h2>
      <div className="rounded-lg border bg-card p-4">
        <div className="flex items-center justify-between gap-4">
          <div>
            <div className="text-sm font-medium">Git SyncDeck</div>
            <p className="mt-0.5 text-xs text-muted-foreground">当前版本 {version ? `v${version}` : "未知"}</p>
          </div>
          <Button variant="outline" onClick={check} disabled={busy}>
            {phase === "checking" ? <Loader2 className="animate-spin" /> : <RefreshCw />}
            {phase === "checking" ? "检查中…" : "检查更新"}
          </Button>
        </div>

        {phase === "latest" && (
          <p className="mt-3 flex items-center gap-1.5 text-sm text-muted-foreground">
            <Check className="size-4" />
            已是最新版本
          </p>
        )}

        {phase === "error" && error && (
          <p className="mt-3 flex items-start gap-1.5 text-sm text-destructive">
            <CircleAlert className="mt-0.5 size-4 shrink-0" />
            <span className="min-w-0 break-words">检查或安装更新失败：{error}</span>
          </p>
        )}

        {showUpdate && update && (
          <div className="mt-4 space-y-3 border-t pt-4">
            <div className="flex items-center justify-between gap-4">
              <div>
                <div className="text-sm font-medium">发现新版本 v{update.version}</div>
                {update.date && (
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    发布于 {new Date(update.date).toLocaleDateString("zh-CN")}
                  </p>
                )}
              </div>
              <Button onClick={install} disabled={busy}>
                {phase === "installing" ? <Loader2 className="animate-spin" /> : <Download />}
                {phase === "installing" ? "正在下载…" : "下载并安装"}
              </Button>
            </div>

            {update.notes && (
              <div>
                <div className="text-xs font-medium text-muted-foreground">更新说明</div>
                <p className="mt-1 max-h-40 overflow-y-auto text-sm whitespace-pre-wrap select-text">{update.notes}</p>
              </div>
            )}

            {phase === "installing" && (
              <div className="space-y-1.5">
                <Progress value={percent ?? 0} aria-label="下载进度" />
                <p className="text-xs text-muted-foreground">
                  {progress
                    ? progress.total
                      ? `已下载 ${formatBytes(progress.downloaded)} / ${formatBytes(progress.total)}（${percent}%），完成后会自动重启`
                      : `已下载 ${formatBytes(progress.downloaded)}，完成后会自动重启`
                    : "正在连接…"}
                </p>
              </div>
            )}
          </div>
        )}
      </div>
    </section>
  );
}
