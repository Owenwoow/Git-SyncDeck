import { Search, X } from "lucide-react";
import { Input } from "@/components/ui/input";
import { StatusDot } from "@/components/StatusBadge";
import { FILTERABLE_STATUSES, STATUS_META } from "@/lib/status";
import { cn } from "@/lib/utils";
import type { Project, SyncStatus } from "@/types";

export type StatusFilter = SyncStatus | "all";

/** 主页工具栏：名称搜索 + 状态筛选标签（带数量） */
export function ProjectToolbar({
  projects,
  query,
  onQueryChange,
  filter,
  onFilterChange,
}: {
  projects: Project[];
  query: string;
  onQueryChange: (q: string) => void;
  filter: StatusFilter;
  onFilterChange: (f: StatusFilter) => void;
}) {
  const count = (s: SyncStatus) => projects.filter((p) => p.status === s).length;

  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
      <div className="relative w-56 shrink-0">
        <Search className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          value={query}
          onChange={(e) => onQueryChange(e.target.value)}
          placeholder="搜索项目名称"
          className="h-8 pr-8 pl-8"
        />
        {query && (
          <button
            type="button"
            onClick={() => onQueryChange("")}
            className="absolute top-1/2 right-2 -translate-y-1/2 rounded p-0.5 text-muted-foreground hover:text-foreground"
            aria-label="清空搜索"
          >
            <X className="size-3.5" />
          </button>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-1" role="radiogroup" aria-label="按状态筛选">
        <FilterChip active={filter === "all"} onClick={() => onFilterChange("all")} label="全部" count={projects.length} />
        {FILTERABLE_STATUSES.map((s) => (
          <FilterChip
            key={s}
            active={filter === s}
            onClick={() => onFilterChange(s)}
            label={STATUS_META[s].label}
            count={count(s)}
            dot={<StatusDot status={s} />}
          />
        ))}
      </div>
    </div>
  );
}

function FilterChip({
  active,
  onClick,
  label,
  count,
  dot,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
  count: number;
  dot?: React.ReactNode;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={active}
      onClick={onClick}
      className={cn(
        "inline-flex h-7 items-center gap-1.5 rounded-md border px-2 text-xs transition-colors",
        active
          ? "border-foreground/15 bg-accent font-medium text-foreground"
          : "border-transparent text-muted-foreground hover:bg-accent/60 hover:text-foreground",
        count === 0 && !active && "opacity-50",
      )}
    >
      {dot}
      {label}
      <span className="tabular-nums text-muted-foreground">{count}</span>
    </button>
  );
}
