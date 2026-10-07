import { useMemo, useState } from "react";
import { FolderPlus, RefreshCw, SearchX, ArrowUpDown } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { OverviewStats } from "@/components/projects/OverviewStats";
import { ProjectToolbar, type StatusFilter } from "@/components/projects/ProjectToolbar";
import { ProjectList } from "@/components/projects/ProjectList";
import { ProjectDetailSheet } from "@/components/projects/ProjectDetailSheet";
import { sortProjects } from "@/lib/status";
import { useProjects } from "@/state/projects";

/** 主页：监控中的项目 */
export function HomePage({ onAddProjects }: { onAddProjects: () => void }) {
  const { projects, loading, refreshing, refresh, sync, startSync } = useProjects();
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
  const syncing = sync.phase === "running";

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
          <p className="mt-0.5 text-sm text-muted-foreground">离开这台电脑前点一次"一键同步"，到另一台电脑再点一次。</p>
        </div>
        <div className="flex items-center gap-2">
          <Button variant="outline" onClick={handleRefresh} disabled={refreshing || syncing || loading}>
            <RefreshCw className={refreshing ? "animate-spin" : undefined} />
            {refreshing ? "正在刷新…" : "刷新状态"}
          </Button>
          <Button onClick={startSync} disabled={syncing || refreshing || loading || projects.length === 0}>
            <ArrowUpDown />
            一键同步
          </Button>
        </div>
      </header>

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
            <div className={refreshing ? "pointer-events-none opacity-60 transition-opacity" : "transition-opacity"}>
              <ProjectList projects={visible} selectedId={selectedId} onSelect={setSelectedId} />
            </div>
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
