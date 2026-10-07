import { useCallback, useEffect, useMemo, useState } from "react";
import { FolderOpen, FolderSearch, GitBranch, Loader2 } from "lucide-react";
import { toast } from "sonner";
import * as api from "@/api";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Skeleton } from "@/components/ui/skeleton";
import { repoShortName } from "@/lib/status";
import { cn } from "@/lib/utils";
import { useProjects } from "@/state/projects";
import { useSettings } from "@/state/settings";
import type { ScannedRepo } from "@/types";

/** 添加项目：选择目录 → 扫描 → 勾选要监控的仓库（也可以在这里取消监控） */
export function AddProjectsPage() {
  const { settings } = useSettings();
  const { reload, refresh } = useProjects();
  const [dir, setDir] = useState<string | null>(null);
  const [repos, setRepos] = useState<ScannedRepo[] | null>(null);
  const [scanning, setScanning] = useState(false);
  const [picking, setPicking] = useState(false);
  const [applying, setApplying] = useState(false);
  /** 勾选状态 = 希望被监控的仓库路径 */
  const [checked, setChecked] = useState<Set<string>>(new Set());

  const scan = useCallback(async (target: string) => {
    setDir(target);
    setScanning(true);
    try {
      const list = await api.scanDirectory(target);
      setRepos(list);
      setChecked(new Set(list.filter((r) => r.monitored).map((r) => r.path)));
    } catch (e) {
      toast.error("扫描失败", { description: e instanceof Error ? e.message : String(e) });
      setRepos([]);
      setChecked(new Set());
    } finally {
      setScanning(false);
    }
  }, []);

  // 进入页面自动扫描默认代码目录
  useEffect(() => {
    if (settings && dir === null) scan(settings.defaultCodeDir);
  }, [settings, dir, scan]);

  async function pick() {
    setPicking(true);
    try {
      const picked = await api.pickDirectory(dir ?? undefined);
      if (picked) await scan(picked);
    } finally {
      setPicking(false);
    }
  }

  const toAdd = useMemo(() => (repos ?? []).filter((r) => !r.monitored && checked.has(r.path)), [repos, checked]);
  const toRemove = useMemo(() => (repos ?? []).filter((r) => r.monitored && !checked.has(r.path)), [repos, checked]);
  const dirty = toAdd.length + toRemove.length > 0;

  function toggle(path: string, on: boolean) {
    setChecked((s) => {
      const next = new Set(s);
      if (on) next.add(path);
      else next.delete(path);
      return next;
    });
  }

  function reset() {
    setChecked(new Set((repos ?? []).filter((r) => r.monitored).map((r) => r.path)));
  }

  async function apply() {
    if (!dir) return;
    setApplying(true);
    try {
      if (toAdd.length) await api.addProjects(toAdd.map((r) => r.path));
      for (const r of toRemove) if (r.projectId) await api.removeProject(r.projectId);
      const parts = [toAdd.length && `已添加 ${toAdd.length} 个项目`, toRemove.length && `已取消监控 ${toRemove.length} 个`];
      toast.success(parts.filter(Boolean).join("，"));
      await Promise.all([reload(), scan(dir)]);
      // 新加入的项目只做过本地检查，后台联网刷新一次
      if (toAdd.length) refresh().catch(() => undefined);
    } catch (e) {
      toast.error("操作失败", { description: e instanceof Error ? e.message : String(e) });
    } finally {
      setApplying(false);
    }
  }

  const monitoredCount = (repos ?? []).filter((r) => r.monitored).length;

  return (
    <div className="space-y-5 pb-4">
      <header>
        <h1 className="text-xl font-semibold tracking-tight">添加项目</h1>
        <p className="mt-0.5 text-sm text-muted-foreground">选择存放代码的目录，自动找出其中的 Git 仓库，勾选后加入监控。</p>
      </header>

      <div className="flex items-center gap-3 rounded-lg border bg-card px-4 py-3">
        <FolderOpen className="size-5 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="text-xs text-muted-foreground">扫描目录（含子目录）</div>
          <div className="truncate font-mono text-sm select-text">{dir ?? "—"}</div>
        </div>
        <Button variant="outline" onClick={pick} disabled={picking || scanning || applying}>
          {picking ? <Loader2 className="animate-spin" /> : <FolderSearch />}
          选择目录…
        </Button>
      </div>

      <section>
        <div className="mb-2 flex items-baseline justify-between gap-3">
          <h2 className="text-sm font-medium">
            {scanning || !repos ? (
              <span className="inline-flex items-center gap-1.5 text-muted-foreground">
                <Loader2 className="size-3.5 animate-spin" />
                正在扫描…
              </span>
            ) : (
              <>
                在 <span className="font-mono">{dir}</span> 中找到 {repos.length} 个 Git 仓库
              </>
            )}
          </h2>
          {!scanning && repos && repos.length > 0 && (
            <span className="shrink-0 text-xs text-muted-foreground">已监控 {monitoredCount} 个</span>
          )}
        </div>

        {scanning || !repos ? (
          <div className="space-y-2">
            {[0, 1, 2, 3, 4].map((i) => (
              <Skeleton key={i} className="h-14 rounded-lg" />
            ))}
          </div>
        ) : repos.length === 0 ? (
          <div className="rounded-lg border border-dashed py-12 text-center text-sm text-muted-foreground">
            这个目录里没有找到 Git 仓库
          </div>
        ) : (
          <ul className="divide-y overflow-hidden rounded-lg border bg-card">
            {repos.map((r) => (
              <RepoRow
                key={r.path}
                repo={r}
                checked={checked.has(r.path)}
                onCheckedChange={(on) => toggle(r.path, on)}
                disabled={applying}
              />
            ))}
          </ul>
        )}
      </section>

      {dirty && (
        <div className="sticky bottom-0 flex flex-wrap items-center justify-between gap-3 rounded-lg border bg-popover px-4 py-3 shadow-lg">
          <span className="text-sm">
            将
            {toAdd.length > 0 && <b className="mx-1 text-status-synced">添加 {toAdd.length} 个</b>}
            {toAdd.length > 0 && toRemove.length > 0 && "、"}
            {toRemove.length > 0 && <b className="mx-1 text-status-diverged">取消监控 {toRemove.length} 个</b>}
            项目
          </span>
          <div className="flex gap-2">
            <Button variant="ghost" onClick={reset} disabled={applying}>
              撤销更改
            </Button>
            <Button onClick={apply} disabled={applying}>
              {applying && <Loader2 className="animate-spin" />}
              应用
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}

function RepoRow({
  repo: r,
  checked,
  onCheckedChange,
  disabled,
}: {
  repo: ScannedRepo;
  checked: boolean;
  onCheckedChange: (on: boolean) => void;
  disabled: boolean;
}) {
  const willAdd = !r.monitored && checked;
  const willRemove = r.monitored && !checked;
  const repo = repoShortName(r.remoteUrl);
  return (
    <li>
      <label
        className={cn(
          "flex cursor-pointer items-center gap-3 px-4 py-2.5 transition-colors hover:bg-accent/50",
          willAdd && "bg-status-synced/6",
          willRemove && "bg-status-diverged/6",
        )}
      >
        <Checkbox checked={checked} onCheckedChange={(v) => onCheckedChange(v === true)} disabled={disabled} />
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2">
            <span className="truncate text-sm font-medium">{r.name}</span>
            <span className="inline-flex min-w-0 shrink items-center gap-1 rounded bg-muted px-1.5 py-px font-mono text-[11px] text-muted-foreground">
              <GitBranch className="size-3 shrink-0" />
              <span className="truncate">{r.branch}</span>
            </span>
            {r.monitored && !willRemove && (
              <span className="shrink-0 rounded bg-primary/8 px-1.5 py-px text-[11px] text-foreground/70">监控中</span>
            )}
            {willAdd && <span className="shrink-0 text-[11px] text-status-synced">将添加</span>}
            {willRemove && <span className="shrink-0 text-[11px] text-status-diverged">将取消监控</span>}
          </div>
          <div className="mt-0.5 truncate font-mono text-xs text-muted-foreground">{r.path}</div>
        </div>
        <span className={cn("hidden max-w-48 shrink-0 truncate text-xs @2xl:block", repo ? "text-foreground/70" : "text-muted-foreground")}>
          {repo ?? "无远程仓库"}
        </span>
      </label>
    </li>
  );
}
