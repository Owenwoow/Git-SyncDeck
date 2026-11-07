import { summarizeProjects } from "@/lib/status";
import { useNow } from "@/lib/useNow";
import { cn } from "@/lib/utils";
import { useSettings } from "@/state/settings";
import type { Project } from "@/types";

/** 只将最近成功检查且无异常的当前分支计为已确认同步。 */
export function OverviewStats({ projects }: { projects: Project[] }) {
  const now = useNow(true);
  const autoMerge = useSettings().settings?.autoMerge ?? true;
  const stats = summarizeProjects(projects, autoMerge, now);

  return (
    <div className="grid grid-cols-3 gap-3">
      <StatCard label="项目总数" value={stats.total} hint="只检查各项目的当前分支" />
      <StatCard label="已确认同步" value={stats.confirmedSynced} hint={`${stats.unconfirmed} 个待确认 · 30 分钟内检查有效`} valueClass="text-status-synced" />
      <StatCard
        label="需处理"
        value={stats.needsAction}
        valueClass={stats.needsAction ? "text-status-dirty" : undefined}
        hint={stats.needsAction
          ? `${stats.canAttempt} 个可尝试自动处理，${stats.manual} 个需手动处理`
          : stats.unconfirmed ? "请刷新以确认当前云端状态" : stats.total ? "最近检查时当前分支均已同步" : "尚未添加项目"}
      />
    </div>
  );
}

function StatCard({
  label,
  value,
  hint,
  valueClass,
}: {
  label: string;
  value: number;
  hint: string;
  valueClass?: string;
}) {
  return (
    <div className="min-w-0 rounded-lg border bg-card px-4 py-3">
      <div className="text-xs text-muted-foreground">{label}</div>
      <div className={cn("mt-1 text-2xl font-semibold tabular-nums", valueClass)}>{value}</div>
      <div className="mt-0.5 text-xs leading-snug text-muted-foreground">
        {hint}
      </div>
    </div>
  );
}
