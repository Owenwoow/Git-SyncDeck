import { STATUS_META, willAutoCommit } from "@/lib/status";
import { cn } from "@/lib/utils";
import type { Project } from "@/types";

/** 主页顶部的三张概览卡：项目总数 / 已同步 / 需处理 */
export function OverviewStats({ projects }: { projects: Project[] }) {
  const total = projects.length;
  const synced = projects.filter((p) => p.status === "synced").length;
  // 打开了自动提交的"有未提交改动"项目也算可自动同步
  const autoSync = projects.filter((p) => STATUS_META[p.status].autoSync || willAutoCommit(p)).length;
  const manual = projects.filter((p) => STATUS_META[p.status].abnormal && !willAutoCommit(p)).length;
  const pending = total - synced;

  return (
    <div className="grid grid-cols-3 gap-3">
      <StatCard label="项目总数" value={total} hint="正在监控" />
      <StatCard label="已同步" value={synced} hint="本地与云端一致" valueClass="text-status-synced" />
      <StatCard
        label="需处理"
        value={pending}
        valueClass={pending ? "text-status-dirty" : undefined}
        hint={pending ? `其中 ${autoSync} 个可自动同步，${manual} 个需手动处理` : "全部都是最新的"}
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
