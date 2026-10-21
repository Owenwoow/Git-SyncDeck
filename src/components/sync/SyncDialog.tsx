import { useState } from "react";
import { AlertTriangle, CheckCircle2, ChevronRight, Clock, PartyPopper, WifiOff } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Progress } from "@/components/ui/progress";
import { StatusBadge } from "@/components/StatusBadge";
import { ProjectActions } from "@/components/projects/ProjectActions";
import { formatClock } from "@/lib/time";
import { useNow } from "@/lib/useNow";
import { cn } from "@/lib/utils";
import { useProjects } from "@/state/projects";
import { useSettings } from "@/state/settings";
import { activeText, headline, overall, shortLabel, type SyncState } from "@/state/syncProgress";
import type { Project, SyncItemResult } from "@/types";

/** 一键同步和后台同步的弹窗：进行中显示逐个进度（随时可以关掉，同步照常进行），结束后显示结果与异常处理 */
export function SyncDialog() {
  const { sync, closeSync, skipInSync, background, backgroundOpen, closeBackground, skipInBackground } = useProjects();
  return (
    <>
      <SyncProgressDialog state={sync} open={sync.open} onClose={closeSync} onSkip={skipInSync} />
      {background && (
        <SyncProgressDialog
          state={background}
          open={backgroundOpen}
          onClose={closeBackground}
          onSkip={skipInBackground}
        />
      )}
    </>
  );
}

type DialogProps = { state: SyncState; open: boolean; onClose: () => void; onSkip: (id: string) => void };

function SyncProgressDialog({ state, open, onClose, onSkip }: DialogProps) {
  const { projects } = useProjects();
  const running = state.phase === "running";
  const byId = new Map(projects.map((p) => [p.id, p]));

  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="flex max-h-[85vh] flex-col gap-0 p-0 sm:max-w-2xl">
        {running ? (
          <RunningView sync={state} byId={byId} />
        ) : (
          <DoneView sync={state} byId={byId} onSkip={onSkip} />
        )}
        <DialogFooter className="items-center border-t px-6 py-3.5">
          {running && (
            <span className="mr-auto text-xs text-muted-foreground">关掉这个窗口不会中断同步，进度会继续显示在主页上</span>
          )}
          <Button variant={running ? "outline" : "default"} onClick={onClose}>
            {running ? "关闭" : "完成"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

type ViewProps = { sync: SyncState; byId: Map<string, Project> };

// ---------------- 进行中 ----------------

function RunningView({ sync, byId }: ViewProps) {
  const { done, total, fraction } = overall(sync);
  const now = useNow(sync.retry !== null);
  const title = sync.label ? `正在后台${sync.label}` : "正在同步";
  return (
    <>
      <DialogHeader className="gap-3 px-6 pt-6 pb-4">
        <div className="flex items-baseline justify-between pr-6">
          <DialogTitle className="text-base">{title}</DialogTitle>
          <span className="text-sm text-muted-foreground tabular-nums">
            {done} / {total}
          </span>
        </div>
        <DialogDescription className="sr-only">逐个同步项目的进度</DialogDescription>
        <Progress value={fraction * 100} />
        {sync.retry && (
          <p className="flex items-center gap-1.5 text-sm text-muted-foreground" role="status">
            <WifiOff className="size-4" />
            {headline(sync, (id) => byId.get(id)?.name, now)}
          </p>
        )}
      </DialogHeader>
      <ul className="min-h-0 flex-1 divide-y overflow-y-auto border-t px-6">
        {sync.order.length === 0 && <li className="py-6 text-center text-sm text-muted-foreground">准备中…</li>}
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
  const progress = sync.active[p.id];
  const active = !!progress && !result;
  const waitingRetry = !result && !active && sync.retry?.projectIds.includes(p.id);
  const autoMerge = useSettings().settings?.autoMerge ?? true;

  let text: React.ReactNode;
  if (result) text = <OutcomeText result={result} />;
  else if (active)
    text = (
      <span className="text-status-syncing tabular-nums">
        {activeText(sync.before[p.id], progress, p.autoCommit, autoMerge)}
      </span>
    );
  else
    text = (
      <span className="inline-flex items-center gap-1 text-muted-foreground">
        <Clock className="size-3.5" />
        {waitingRetry ? "等待重试" : "等待中"}
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
    case "merged":
      return <span className="text-status-synced">已自动合并并推送 {result.commits} 个提交</span>;
    case "failed":
      return <span className="text-status-dirty">需要手动处理</span>;
  }
}

// ---------------- 结束后 ----------------

function DoneView({ sync, byId, onSkip }: ViewProps & { onSkip: (id: string) => void }) {
  const [showSynced, setShowSynced] = useState(false);

  const results = sync.order.map((id) => sync.results[id]).filter(Boolean);
  const failed = results.filter((r) => r.outcome === "failed");
  const succeeded = results.filter((r) => r.outcome !== "failed");
  const unresolved = failed.filter((r) => !sync.resolved.includes(r.projectId));
  const syncedCount = succeeded.length + (failed.length - unresolved.length);
  const title = sync.label
    ? `上次后台同步${sync.finishedAt ? `：${formatClock(sync.finishedAt)} ${shortLabel(sync.label)}` : ""}`
    : "同步完成";

  return (
    <>
      <DialogHeader className="gap-3 px-6 pt-6 pb-5">
        <DialogTitle className="text-base">{title}</DialogTitle>
        <DialogDescription className="sr-only">同步的结果</DialogDescription>
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
            {sync.label ? "这一轮的项目都已同步。" : "全部项目都已同步，可以放心切换电脑了。"}
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
                    onSkip={() => onSkip(r.projectId)}
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
