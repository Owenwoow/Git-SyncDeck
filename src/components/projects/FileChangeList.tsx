import { CHANGE_KIND_META } from "@/lib/status";
import { cn } from "@/lib/utils";
import type { FileChange } from "@/types";

/** 未提交的文件，按 已暂存 / 未暂存 / 未跟踪 分组 */
export function FileChangeList({ changes, className }: { changes: FileChange[]; className?: string }) {
  const groups = [
    { title: "已暂存", items: changes.filter((c) => c.staged) },
    { title: "未暂存", items: changes.filter((c) => !c.staged && c.kind !== "?") },
    { title: "未跟踪", items: changes.filter((c) => c.kind === "?") },
  ].filter((g) => g.items.length);

  return (
    <div className={cn("space-y-3", className)}>
      {groups.map((g) => (
        <div key={g.title}>
          <div className="mb-1 text-xs text-muted-foreground">
            {g.title} · {g.items.length}
          </div>
          <ul className="space-y-0.5">
            {g.items.map((c) => (
              <FileChangeItem key={`${c.kind}:${c.path}`} change={c} />
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}

export function FileChangeItem({ change: c }: { change: FileChange }) {
  const meta = CHANGE_KIND_META[c.kind];
  const slash = c.path.lastIndexOf("/");
  const dir = slash >= 0 ? c.path.slice(0, slash + 1) : "";
  const file = c.path.slice(slash + 1);
  return (
    <li className="flex min-w-0 items-center gap-2 py-0.5 font-mono text-xs select-text" title={meta.label}>
      <span
        className={cn(
          "inline-flex size-4.5 shrink-0 items-center justify-center rounded text-[10px] font-semibold",
          meta.className,
        )}
      >
        {c.kind === "?" ? "U" : c.kind}
      </span>
      <span className="min-w-0 truncate">
        {c.kind === "R" && c.oldPath && <span className="text-muted-foreground">{c.oldPath} → </span>}
        <span className="text-muted-foreground">{dir}</span>
        {file}
      </span>
    </li>
  );
}
