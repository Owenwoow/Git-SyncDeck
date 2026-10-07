import { useEffect, useState } from "react";
import { Check, FolderSearch, Loader2, RotateCcw } from "lucide-react";
import { toast } from "sonner";
import * as api from "@/api";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { DEFAULT_COMMIT_TEMPLATE, renderCommitMessage } from "@/lib/status";
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
        <h2 className="text-sm font-medium text-muted-foreground">自动提交</h2>
        <CommitTemplateCard />
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

/** 预览里电脑名的占位文字（实际提交时由后端取这台电脑的名字） */
const HOST_PREVIEW = "本机电脑名";

/** 自动提交的提交信息模板：失焦或回车保存，空值恢复默认 */
function CommitTemplateCard() {
  const { settings, updateSettings } = useSettings();
  const saved = settings?.commitTemplate ?? DEFAULT_COMMIT_TEMPLATE;
  const [draft, setDraft] = useState(saved);
  useEffect(() => setDraft(saved), [saved]);

  async function save(value: string) {
    const next = value.trim();
    if (next === saved) {
      setDraft(saved);
      return;
    }
    try {
      await updateSettings({ commitTemplate: next });
      toast.success(next && next !== DEFAULT_COMMIT_TEMPLATE ? "提交信息模板已保存" : "已恢复默认模板");
    } catch (e) {
      setDraft(saved);
      toast.error("保存失败", { description: e instanceof Error ? e.message : String(e) });
    }
  }

  return (
    <div className="rounded-lg border bg-card p-4">
      <div className="text-sm font-medium">提交信息模板</div>
      <p className="mt-0.5 text-xs text-muted-foreground">
        在项目详情里打开"一键同步时自动提交"的项目，一键同步时会用这个格式提交所有改动并推送。
      </p>
      <div className="mt-3 flex gap-2">
        <Input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => save(draft)}
          onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
          placeholder={DEFAULT_COMMIT_TEMPLATE}
          disabled={!settings}
          className="font-mono"
          aria-label="提交信息模板"
        />
        <Button
          variant="outline"
          // 不让输入框先失焦保存一次草稿
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => save(DEFAULT_COMMIT_TEMPLATE)}
          disabled={!settings || (saved === DEFAULT_COMMIT_TEMPLATE && draft === saved)}
        >
          <RotateCcw />
          恢复默认
        </Button>
      </div>
      <ul className="mt-3 space-y-1 text-xs text-muted-foreground">
        <li>
          <code className="rounded bg-muted px-1 py-px font-mono text-foreground/80">{"{date}"}</code>{" "}
          同步时的本地时间，如 2026-10-07 14:30
        </li>
        <li>
          <code className="rounded bg-muted px-1 py-px font-mono text-foreground/80">{"{host}"}</code>{" "}
          这台电脑的名字（预览里用"{HOST_PREVIEW}"代替）
        </li>
      </ul>
      <div className="mt-3 rounded-md bg-muted/50 px-3 py-2 text-xs">
        <span className="text-muted-foreground">预览：</span>
        <span className="font-mono break-all select-text">
          {renderCommitMessage(draft, new Date(), HOST_PREVIEW)}
        </span>
      </div>
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
