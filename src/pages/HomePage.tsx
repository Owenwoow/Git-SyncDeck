import { useEffect, useMemo, useState } from "react";
import { FolderPlus, RefreshCw, SearchX, ArrowUpDown, Loader2, PauseCircle, Play, History, ChevronRight } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { OverviewStats } from "@/components/projects/OverviewStats";
import { ProjectToolbar, type StatusFilter } from "@/components/projects/ProjectToolbar";
import { ProjectList } from "@/components/projects/ProjectList";
import { ProjectDetailSheet } from "@/components/projects/ProjectDetailSheet";
import { sortProjects } from "@/lib/status";
import { useProjects } from "@/state/projects";
import { activePause, describePauseUntil, useSettings } from "@/state/settings";
import { headline, overall, resultSummary, shortLabel, type SyncState } from "@/state/syncProgress";
import { formatClock } from "@/lib/time";
import { useNow } from "@/lib/useNow";

/** 主页：监控中的项目 */
export function HomePage({ onAddProjects }: { onAddProjects: () => void }) {
  const { projects, loading, refreshing, checkingIds, refresh, sync, startSync, openSync, autoSync, background, openBackground } =
    useProjects();
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<StatusFilter>("all");
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return sortProjects(projects).filter(
      (p) => (filter === "all" || p.status === filter) && (!q || p.name.toLowerCase().includes(q)),
    );
  }, [projects, query, filter]);

  const selected = projects.find((p) => p.id === selectedId) ?? null;
  // 后台自动同步进行中时也不能再点一键同步 / 刷新（后端同一时间只跑一轮）
  const syncing = sync.phase === "running" || autoSync !== null;
  // 正在进行的那一轮（一键同步优先）：一键同步按钮变成进度按钮，点开看每个项目的进度
  const runningRound = sync.phase === "running" ? sync : background?.phase === "running" ? background : null;
  const lastBackground = background?.phase === "done" ? background : null;
  const now = useNow(runningRound?.retry != null);
  const nameOf = (id: string) => projects.find((p) => p.id === id)?.name;
  // 列表里正在处理的行显示百分比
  const percents = useMemo(() => {
    const map = new Map<string, number>();
    for (const a of Object.values(runningRound?.active ?? {})) {
      if (a.percent !== null && a.phase && a.phase !== "preparing") map.set(a.projectId, a.percent);
    }
    return map;
  }, [runningRound]);

  async function handleRefresh() {
    try {
      const list = await refresh();
      const failed = list.filter((p) => p.checkError).length;
      if (failed) toast.warning(`已刷新 ${list.length} 个项目，其中 ${failed} 个检查失败`);
      else toast.success(`已刷新 ${list.length} 个项目的状态`);
    } catch (e) {
      toast.error("刷新失败", { description: e instanceof Error ? e.message : String(e) });
    }
  }

  return (
    <div className="space-y-5">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">监控中的项目</h1>
          {runningRound ? (
            <p className="mt-0.5 flex min-w-0 items-center gap-1.5 text-sm text-muted-foreground" role="status">
              <Loader2 className="size-3.5 shrink-0 animate-spin" />
              <span className="truncate tabular-nums">
                {runningRound.label ?? "一键同步"} · {headline(runningRound, nameOf, now)}
              </span>
            </p>
          ) : autoSync ? (
            <p className="mt-0.5 flex items-center gap-1.5 text-sm text-muted-foreground" role="status">
              <Loader2 className="size-3.5 animate-spin" />
              正在后台{autoSync.label}…
            </p>
          ) : lastBackground ? (
            <button
              type="button"
              onClick={openBackground}
              className="mt-0.5 flex max-w-full items-center gap-1 text-left text-sm text-muted-foreground hover:text-foreground"
            >
              <History className="size-3.5 shrink-0" />
              <span className="truncate">
                上次后台同步：
                {lastBackground.finishedAt ? `${formatClock(lastBackground.finishedAt)} ` : ""}
                {shortLabel(lastBackground.label)} · {resultSummary(lastBackground)}
              </span>
              <ChevronRight className="size-3.5 shrink-0" />
            </button>
          ) : (
            <p className="mt-0.5 text-sm text-muted-foreground">
              离开和回到这台电脑时会在后台自动同步（可在设置里调整），也可以随时点"一键同步"。
            </p>
          )}
        </div>
        <div className="flex items-center gap-2">
          <Button variant="outline" onClick={handleRefresh} disabled={refreshing || syncing || loading}>
            <RefreshCw className={refreshing ? "animate-spin" : undefined} />
            {refreshing ? "正在刷新…" : "刷新状态"}
          </Button>
          {runningRound ? (
            <ProgressButton round={runningRound} onClick={runningRound === sync ? openSync : openBackground} />
          ) : (
            // 刷新途中也可以点：后端会等这一轮检查结束，再复用刚检查过的结果
            <Button onClick={startSync} disabled={syncing || loading || projects.length === 0}>
              <ArrowUpDown />
              一键同步
            </Button>
          )}
        </div>
      </header>

      <PausedNotice />

      {loading ? (
        <LoadingSkeleton />
      ) : projects.length === 0 ? (
        <EmptyState onAddProjects={onAddProjects} />
      ) : (
        <>
          <OverviewStats projects={projects} />
          <ProjectToolbar
            projects={projects}
            query={query}
            onQueryChange={setQuery}
            filter={filter}
            onFilterChange={setFilter}
          />
          {visible.length ? (
            // 刷新时不再整体变灰：每个项目检查完就更新那一行，没检查完的行在状态旁转圈
            <ProjectList
              projects={visible}
              selectedId={selectedId}
              onSelect={setSelectedId}
              checkingIds={checkingIds}
              percents={percents}
            />
          ) : (
            <div className="flex flex-col items-center gap-2 rounded-lg border border-dashed py-12 text-sm text-muted-foreground">
              <SearchX className="size-6" />
              没有匹配的项目
              <Button
                variant="link"
                size="sm"
                onClick={() => {
                  setQuery("");
                  setFilter("all");
                }}
              >
                清除筛选条件
              </Button>
            </div>
          )}
        </>
      )}

      <ProjectDetailSheet project={selected} onClose={() => setSelectedId(null)} />
    </div>
  );
}

/** 同步进行中的「一键同步」按钮（GitHub Desktop 的样子）：底色按总进度填充，文字"正在同步 3/7"，点开看详细进度 */
function ProgressButton({ round, onClick }: { round: SyncState; onClick: () => void }) {
  const { done, total, fraction } = overall(round);
  const text = `${round.label ? "后台同步" : "正在同步"} ${total ? `${done}/${total}` : "…"}`;
  return (
    <Button
      variant="outline"
      onClick={onClick}
      className="relative min-w-32 overflow-hidden tabular-nums"
      aria-label={`${text}，点击查看详细进度`}
    >
      <span
        aria-hidden
        className="absolute inset-y-0 left-0 bg-primary/20 transition-[width] duration-300"
        style={{ width: `${Math.round(fraction * 100)}%` }}
      />
      <Loader2 className="relative animate-spin" />
      <span className="relative">{text}</span>
    </Button>
  );
}

/** 自动同步暂停中："自动同步已暂停到 明天 00:00"和「恢复」；到点自动消失 */
function PausedNotice() {
  const { settings, resumeAutoSync } = useSettings();
  const [now, setNow] = useState(() => Date.now());
  const [busy, setBusy] = useState(false);
  const until = activePause(settings, now);

  // 到了暂停结束的时间重新渲染一次，提示自动消失
  useEffect(() => {
    if (!until) return;
    const timer = window.setTimeout(() => setNow(Date.now()), Math.min(until.getTime() - Date.now() + 500, 2 ** 31 - 1));
    return () => window.clearTimeout(timer);
  }, [until?.getTime()]);

  if (!until) return null;

  async function resume() {
    setBusy(true);
    try {
      await resumeAutoSync();
      toast.success("已恢复自动同步");
    } catch (e) {
      toast.error("恢复失败", { description: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex items-center justify-between gap-3 rounded-lg border bg-muted/40 px-4 py-2.5 text-sm" role="status">
      <span className="flex min-w-0 items-center gap-2">
        <PauseCircle className="size-4 shrink-0 text-muted-foreground" />
        自动同步已暂停到 {describePauseUntil(until)}
      </span>
      <Button variant="outline" size="sm" onClick={resume} disabled={busy}>
        {busy ? <Loader2 className="animate-spin" /> : <Play />}
        恢复
      </Button>
    </div>
  );
}

function EmptyState({ onAddProjects }: { onAddProjects: () => void }) {
  return (
    <div className="flex flex-col items-center gap-3 rounded-lg border border-dashed py-16 text-center">
      <FolderPlus className="size-8 text-muted-foreground" />
      <div>
        <div className="font-medium">还没有监控任何项目</div>
        <div className="mt-1 text-sm text-muted-foreground">选择你的代码目录，把要同步的 Git 项目加进来。</div>
      </div>
      <Button onClick={onAddProjects}>
        <FolderPlus />
        去添加项目
      </Button>
    </div>
  );
}

function LoadingSkeleton() {
  return (
    <div className="space-y-5">
      <div className="grid grid-cols-3 gap-3">
        {[0, 1, 2].map((i) => (
          <Skeleton key={i} className="h-[86px] rounded-lg" />
        ))}
      </div>
      <Skeleton className="h-8 w-2/3" />
      <div className="space-y-2">
        {[0, 1, 2, 3, 4].map((i) => (
          <Skeleton key={i} className="h-14 rounded-lg" />
        ))}
      </div>
    </div>
  );
}
