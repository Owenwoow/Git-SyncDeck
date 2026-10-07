import { useState } from "react";
import { AlertTriangle, CheckCircle2, ChevronRight, Clock, PartyPopper } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Progress } from "@/components/ui/progress";
import { StatusBadge } from "@/components/StatusBadge";
import { ProjectActions } from "@/components/projects/ProjectActions";
import { cn } from "@/lib/utils";
import { useProjects, type SyncState } from "@/state/projects";
import type { Project, SyncItemResult } from "@/types";

/** 一键同步对话框：进行中显示逐个进度，结束后显示结果与异常处理 */
export function SyncDialog() {
  const { sync, closeSync, projects } = useProjects();
  const running = sync.phase === "running";
  const byId = new Map(projects.map((p) => [p.id, p]));

  return (
    <Dialog open={sync.open} onOpenChange={(o) => !o && closeSync()}>
      <DialogContent
        showCloseButton={!running}
        onInteractOutside={(e) => running && e.preventDefault()}
        onEscapeKeyDown={(e) => running && e.preventDefault()}
        className="flex max-h-[85vh] flex-col gap-0 p-0 sm:max-w-2xl"
      >
        {running ? <RunningView sync={sync} byId={byId} /> : <DoneView sync={sync} byId={byId} />}
        {!running && (
          <DialogFooter className="border-t px-6 py-3.5">
            <Button onClick={closeSync}>完成</Button>
          </DialogFooter>
        )}
      </DialogContent>
    </Dialog>
  );
}

type ViewProps = { sync: SyncState; byId: Map<string, Project> };

// ---------------- 进行中 ----------------

function RunningView({ sync, byId }: ViewProps) {
  const total = sync.order.length;
  const current = Math.min(sync.doneCount + 1, total);
  return (
    <>
      <DialogHeader className="gap-3 px-6 pt-6 pb-4">
        <div className="flex items-baseline justify-between pr-2">
          <DialogTitle className="text-base">正在同步</DialogTitle>
          <span className="text-sm text-muted-foreground tabular-nums">
            {current} / {total}
          </span>
        </div>
        <DialogDescription className="sr-only">正在逐个同步所有监控中的项目</DialogDescription>
        <Progress value={(sync.doneCount / Math.max(total, 1)) * 100} />
      </DialogHeader>
      <ul className="min-h-0 flex-1 divide-y overflow-y-auto border-t px-6">
        {sync.order.map((id) => {
          const p = byId.get(id);
          if (!p) return null;
          return <ProgressRow key={id} project={p} sync={sync} />;
        })}
      </ul>
    </>
  );
}

function ProgressRow({ project: p, sync }: { project: Project; sync: SyncState }) {
  const result = sync.results[p.id];
  const active = sync.activeIds.includes(p.id) && !result;
  const before = sync.before[p.id];

  let text: React.ReactNode;
  if (result) text = <OutcomeText result={result} />;
  else if (active) text = <span className="text-status-syncing">{activeText(before, p.autoCommit)}</span>;
  else
    text = (
      <span className="inline-flex items-center gap-1 text-muted-foreground">
        <Clock className="size-3.5" />
        等待中
      </span>
    );

  return (
    <li className={cn("flex items-center gap-3 py-2.5 transition-opacity", !result && !active && "opacity-60")}>
      <div className="min-w-0 flex-1">
        <div className="truncate text-sm font-medium">{p.name}</div>
        <div className="truncate font-mono text-xs text-muted-foreground">{p.path}</div>
      </div>
      <div className="shrink-0 text-xs">{text}</div>
      <StatusBadge status={p.status} className="w-28 justify-center" />
    </li>
  );
}

function activeText(before: SyncState["before"][string] | undefined, autoCommit: boolean): string {
  switch (before?.status) {
    case "dirty":
      return autoCommit ? "正在自动提交并推送…" : "正在检查…";
    case "ahead":
      return `正在推送 ${before.ahead} 个提交…`;
    case "behind":
      return `正在拉取 ${before.behind} 个提交…`;
    case "synced":
      return "正在检查云端…";
    default:
      return "正在检查…";
  }
}

function OutcomeText({ result }: { result: SyncItemResult }) {
  switch (result.outcome) {
    case "pushed":
      return <span className="text-status-synced">已推送 {result.commits} 个提交</span>;
    case "pulled":
      return <span className="text-status-synced">已拉取 {result.commits} 个提交</span>;
    case "up-to-date":
      return <span className="text-muted-foreground">已是最新</span>;
    case "committed":
      return <span className="text-status-synced">已自动提交并推送 {result.commits} 个提交</span>;
    case "failed":
      return <span className="text-status-dirty">需要手动处理</span>;
  }
}

// ---------------- 结束后 ----------------

function DoneView({ sync, byId }: ViewProps) {
  const { skipInSync } = useProjects();
  const [showSynced, setShowSynced] = useState(false);

  const results = sync.order.map((id) => sync.results[id]).filter(Boolean);
  const failed = results.filter((r) => r.outcome === "failed");
  const succeeded = results.filter((r) => r.outcome !== "failed");
  const unresolved = failed.filter((r) => !sync.resolved.includes(r.projectId));
  const syncedCount = succeeded.length + (failed.length - unresolved.length);

  return (
    <>
      <DialogHeader className="gap-3 px-6 pt-6 pb-5">
        <DialogTitle className="text-base">同步完成</DialogTitle>
        <DialogDescription className="sr-only">一键同步的结果</DialogDescription>
        <div className="flex flex-wrap gap-2.5">
          <SummaryPill
            icon={<CheckCircle2 className="size-4" />}
            className="bg-status-synced/12 text-status-synced"
            text={`${syncedCount} 个已同步`}
          />
          <SummaryPill
            icon={<AlertTriangle className="size-4" />}
            className={unresolved.length ? "bg-status-dirty/12 text-status-dirty" : "bg-muted text-muted-foreground"}
            text={`${unresolved.length} 个异常`}
          />
        </div>
      </DialogHeader>

      <div className="min-h-0 flex-1 space-y-5 overflow-y-auto border-t px-6 py-4">
        {failed.length > 0 && unresolved.length === 0 && (
          <div className="flex items-center gap-2 rounded-lg bg-status-synced/10 px-3 py-2.5 text-sm text-status-synced">
            <PartyPopper className="size-4" />
            异常项目都已处理，所有项目都同步好了。
          </div>
        )}
        {failed.length === 0 && (
          <div className="flex items-center gap-2 rounded-lg bg-status-synced/10 px-3 py-2.5 text-sm text-status-synced">
            <PartyPopper className="size-4" />
            全部项目都已同步，可以放心切换电脑了。
          </div>
        )}

        {failed.length > 0 && (
          <section>
            <h3 className="mb-2 text-sm font-medium">
              需要处理<span className="ml-1.5 text-muted-foreground">{unresolved.length}</span>
            </h3>
            <ul className="space-y-2.5">
              {failed.map((r) => {
                const p = byId.get(r.projectId);
                if (!p) return null;
                return (
                  <FailedCard
                    key={r.projectId}
                    project={p}
                    reason={r.reason}
                    skipped={sync.skipped.includes(r.projectId)}
                    resolved={sync.resolved.includes(r.projectId)}
                    onSkip={() => skipInSync(r.projectId)}
                  />
                );
              })}
            </ul>
          </section>
        )}

        {succeeded.length > 0 && (
          <Collapsible open={showSynced} onOpenChange={setShowSynced}>
            <CollapsibleTrigger className="flex items-center gap-1 text-sm font-medium hover:text-foreground/80">
              <ChevronRight className={cn("size-4 transition-transform", showSynced && "rotate-90")} />
              已同步的项目<span className="ml-0.5 text-muted-foreground">{succeeded.length}</span>
            </CollapsibleTrigger>
            <CollapsibleContent>
              <ul className="mt-2 divide-y rounded-lg border">
                {succeeded.map((r) => {
                  const p = byId.get(r.projectId);
                  if (!p) return null;
                  return (
                    <li key={r.projectId} className="flex items-center gap-3 px-3 py-2">
                      <CheckCircle2 className="size-4 shrink-0 text-status-synced" />
                      <span className="min-w-0 flex-1 truncate text-sm">{p.name}</span>
                      <span className="text-xs">
                        <OutcomeText result={r} />
                      </span>
                    </li>
                  );
                })}
              </ul>
            </CollapsibleContent>
          </Collapsible>
        )}
      </div>
    </>
  );
}

function SummaryPill({ icon, text, className }: { icon: React.ReactNode; text: string; className: string }) {
  return (
    <span className={cn("inline-flex items-center gap-1.5 rounded-md px-2.5 py-1 text-sm font-medium", className)}>
      {icon}
      {text}
    </span>
  );
}

function FailedCard({
  project: p,
  reason,
  skipped,
  resolved,
  onSkip,
}: {
  project: Project;
  reason: string | null;
  skipped: boolean;
  resolved: boolean;
  onSkip: () => void;
}) {
  return (
    <li className={cn("rounded-lg border px-3.5 py-3 transition-opacity", (skipped || resolved) && "bg-muted/30")}>
      <div className="flex items-center gap-2">
        <span className={cn("truncate text-sm font-medium", skipped && "text-muted-foreground")}>{p.name}</span>
        <StatusBadge status={p.status} />
        {skipped && !resolved && <span className="ml-auto text-xs text-muted-foreground">已跳过 · 主页会保留异常标记</span>}
        {resolved && (
          <span className="ml-auto inline-flex items-center gap-1 text-xs text-status-synced">
            <CheckCircle2 className="size-3.5" />
            已提交并推送
          </span>
        )}
      </div>
      {!resolved && (
        <p className={cn("mt-1 text-xs break-words whitespace-pre-wrap text-muted-foreground select-text", skipped && "opacity-70")}>
          {reason}
        </p>
      )}
      {!skipped && !resolved && (
        <div className="mt-2.5">
          <ProjectActions project={p} onSkip={onSkip} />
        </div>
      )}
    </li>
  );
}
