import { cn } from "@/lib/utils";
import { STATUS_META } from "@/lib/status";
import type { SyncStatus } from "@/types";

export function StatusBadge({ status, className }: { status: SyncStatus; className?: string }) {
  const meta = STATUS_META[status];
  const Icon = meta.icon;
  return (
    <span
      className={cn(
        "inline-flex h-6 w-fit shrink-0 items-center gap-1 rounded-full px-2 text-xs font-medium whitespace-nowrap ring-1 ring-inset",
        meta.badgeClass,
        className,
      )}
    >
      <Icon className={cn("size-3.5", status === "syncing" && "animate-spin")} />
      {meta.label}
    </span>
  );
}

export function StatusDot({ status, className }: { status: SyncStatus; className?: string }) {
  return (
    <span className={cn("inline-block size-2 shrink-0 rounded-full", STATUS_META[status].dotClass, className)} />
  );
}
