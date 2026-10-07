import { AlertTriangle, ArrowDown, ArrowUp, Info } from "lucide-react";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { StatusBadge } from "@/components/StatusBadge";
import { FileChangeList } from "@/components/projects/FileChangeList";
import { ProjectActions } from "@/components/projects/ProjectActions";
import { STATUS_META, describeStatus } from "@/lib/status";
import { formatDateTime, formatRelative } from "@/lib/time";
import { cn } from "@/lib/utils";
import type { Project } from "@/types";

/** 点击主页某一行后，从右侧滑出的项目详情 */
export function ProjectDetailSheet({
  project,
  onClose,
}: {
  project: Project | null;
  onClose: () => void;
}) {
  return (
    <Sheet open={!!project} onOpenChange={(o) => !o && onClose()}>
      <SheetContent
        className="w-[420px] max-w-[90vw] gap-0 p-0 data-[side=right]:sm:max-w-[420px]"
        onOpenAutoFocus={(e) => e.preventDefault()}
      >
        {project && <DetailBody project={project} />}
      </SheetContent>
    </Sheet>
  );
}

function DetailBody({ project: p }: { project: Project }) {
  const meta = STATUS_META[p.status];
  const Icon = meta.icon;

  return (
    <>
      <SheetHeader className="gap-2 border-b px-5 pt-5 pb-4">
        <div className="flex items-center gap-2 pr-8">
          <SheetTitle className="truncate text-base font-semibold">{p.name}</SheetTitle>
          <StatusBadge status={p.status} />
        </div>
        <SheetDescription className="truncate font-mono text-xs select-text">{p.path}</SheetDescription>
      </SheetHeader>

      <div className="flex-1 space-y-5 overflow-y-auto px-5 py-4">
        {/* 状态说明 */}
        <div className={cn("flex gap-2.5 rounded-lg px-3 py-2.5 text-sm", meta.badgeClass, "ring-1 ring-inset")}>
          <Icon className={cn("mt-0.5 size-4 shrink-0", p.status === "syncing" && "animate-spin")} />
          <p className="leading-relaxed text-foreground/90">{describeStatus(p)}</p>
        </div>

        {p.checkError && (
          <div className="flex gap-2 text-xs text-status-dirty">
            <AlertTriangle className="size-3.5 shrink-0 translate-y-px" />
            <span className="min-w-0 break-words whitespace-pre-wrap select-text">
              检查失败（显示的是上一次的状态）：{p.checkError}
            </span>
          </div>
        )}

        {p.issue && (
          <div className="flex gap-2 text-xs text-status-dirty">
            <AlertTriangle className="size-3.5 shrink-0 translate-y-px" />
            <span>上次一键同步时被跳过，仍待处理。</span>
          </div>
        )}

        {/* 领先 / 落后 */}
        <div className="grid grid-cols-2 gap-2">
          <Counter icon={<ArrowUp className="size-3.5" />} label="领先" value={p.ahead} hint="本地有、云端没有" />
          <Counter icon={<ArrowDown className="size-3.5" />} label="落后" value={p.behind} hint="云端有、本地没有" />
        </div>

        {/* 基本信息 */}
        <dl className="grid grid-cols-[4.5rem_minmax(0,1fr)] gap-x-3 gap-y-2 text-sm">
          <dt className="text-muted-foreground">分支</dt>
          <dd className="truncate font-mono text-xs leading-5">
            {p.branch}
            {p.upstream ? (
              <span className="text-muted-foreground"> → {p.upstream}</span>
            ) : (
              <span className="font-sans text-muted-foreground">（未设置上游分支）</span>
            )}
          </dd>
          <dt className="text-muted-foreground">远程仓库</dt>
          <dd className="truncate font-mono text-xs leading-5 select-text" title={p.remoteUrl ?? undefined}>
            {p.remoteUrl ?? <span className="font-sans text-muted-foreground">未配置</span>}
          </dd>
          <dt className="text-muted-foreground">上次同步</dt>
          <dd className="text-xs leading-5">
            {formatRelative(p.lastSyncAt)}
            {p.lastSyncAt && <span className="text-muted-foreground">（{formatDateTime(p.lastSyncAt)}）</span>}
          </dd>
        </dl>

        {/* 未提交的文件 */}
        <section>
          <h3 className="mb-2 text-sm font-medium">
            未提交的文件
            {p.changes.length > 0 && <span className="ml-1.5 text-muted-foreground">{p.changes.length}</span>}
          </h3>
          {p.changes.length ? (
            <FileChangeList changes={p.changes} className="rounded-lg border px-3 py-2.5" />
          ) : (
            <div className="flex items-center gap-2 rounded-lg border border-dashed px-3 py-3 text-xs text-muted-foreground">
              <Info className="size-3.5" />
              工作区干净，没有未提交的改动
            </div>
          )}
        </section>
      </div>

      {meta.abnormal && (
        <div className="border-t bg-muted/30 px-5 py-3.5">
          <div className="mb-2 text-xs text-muted-foreground">处理这个项目</div>
          <ProjectActions project={p} />
        </div>
      )}
    </>
  );
}

function Counter({
  icon,
  label,
  value,
  hint,
}: {
  icon: React.ReactNode;
  label: string;
  value: number;
  hint: string;
}) {
  return (
    <div className="rounded-lg border px-3 py-2">
      <div className="flex items-center gap-1 text-xs text-muted-foreground">
        {icon}
        {label}
      </div>
      <div className="mt-0.5 flex items-baseline gap-1">
        <span className={cn("text-xl font-semibold tabular-nums", value === 0 && "text-muted-foreground")}>
          {value}
        </span>
        <span className="text-xs text-muted-foreground">个提交</span>
      </div>
      <div className="text-[11px] text-muted-foreground">{hint}</div>
    </div>
  );
}
