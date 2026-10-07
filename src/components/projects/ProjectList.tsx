import { AlertTriangle, GitBranch } from "lucide-react";
import { StatusBadge } from "@/components/StatusBadge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { repoShortName } from "@/lib/status";
import { formatDateTime, formatRelative } from "@/lib/time";
import { cn } from "@/lib/utils";
import type { Project } from "@/types";

// 列宽：窄窗口时隐藏"GitHub 仓库"列（容器查询，见 App 里 main 的 @container）
const GRID =
  "grid grid-cols-[minmax(0,1fr)_7.5rem_5.5rem] @3xl:grid-cols-[minmax(0,1fr)_minmax(0,12rem)_7.5rem_5.5rem] items-center gap-4";

export function ProjectList({
  projects,
  selectedId,
  onSelect,
}: {
  projects: Project[];
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  return (
    <div className="overflow-hidden rounded-lg border bg-card">
      <div className={cn(GRID, "border-b bg-muted/40 px-4 py-2 text-xs text-muted-foreground")}>
        <span>项目</span>
        <span className="hidden @3xl:block">GitHub 仓库</span>
        <span>状态</span>
        <span className="text-right">上次同步</span>
      </div>
      <ul className="divide-y">
        {projects.map((p) => (
          <li key={p.id}>
            <ProjectRow project={p} selected={p.id === selectedId} onClick={() => onSelect(p.id)} />
          </li>
        ))}
      </ul>
    </div>
  );
}

function ProjectRow({
  project: p,
  selected,
  onClick,
}: {
  project: Project;
  selected: boolean;
  onClick: () => void;
}) {
  const repo = repoShortName(p.remoteUrl);
  // 检查失败优先于上次同步遗留的异常
  const notice = p.checkError ? `检查失败：${p.checkError}` : p.issue ? `上次同步已跳过：${p.issue}` : null;
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        GRID,
        "w-full px-4 py-3 text-left transition-colors hover:bg-accent/50",
        selected && "bg-accent/70 hover:bg-accent/70",
      )}
    >
      <div className="min-w-0">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate font-medium">{p.name}</span>
          <span className="inline-flex min-w-0 shrink items-center gap-1 rounded bg-muted px-1.5 py-px font-mono text-[11px] text-muted-foreground">
            <GitBranch className="size-3 shrink-0" />
            <span className="truncate">{p.branch}</span>
          </span>
        </div>
        <div className="mt-0.5 truncate font-mono text-xs text-muted-foreground">{p.path}</div>
        {notice && (
          <div className="mt-1 flex min-w-0 items-center gap-1 text-xs text-status-dirty" title={notice}>
            <AlertTriangle className="size-3.5 shrink-0" />
            <span className="truncate">{notice.replace(/\s*\n\s*/g, " ")}</span>
          </div>
        )}
      </div>

      <div className="hidden min-w-0 @3xl:block">
        {repo ? (
          <Tooltip>
            <TooltipTrigger asChild>
              <span className="block truncate text-sm text-foreground/80">{repo}</span>
            </TooltipTrigger>
            <TooltipContent>{p.remoteUrl}</TooltipContent>
          </Tooltip>
        ) : (
          <span className="text-sm text-muted-foreground">—</span>
        )}
      </div>

      <StatusBadge status={p.status} />

      <span
        className="text-right text-xs text-muted-foreground tabular-nums"
        title={p.lastSyncAt ? formatDateTime(p.lastSyncAt) : undefined}
      >
        {formatRelative(p.lastSyncAt)}
      </span>
    </button>
  );
}
