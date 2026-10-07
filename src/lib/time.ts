const pad = (n: number) => String(n).padStart(2, "0");

function hm(d: Date) {
  return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function startOfDay(d: Date) {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/** 相对时间：刚刚 / 3 分钟前 / 2 小时前 / 今天 09:12 / 昨天 22:14 / 3 天前 / 8月30日 / 从未同步 */
export function formatRelative(iso: string | null, now: Date = new Date()): string {
  if (!iso) return "从未同步";
  const d = new Date(iso);
  const diffMin = Math.floor((now.getTime() - d.getTime()) / 60_000);
  if (diffMin < 1) return "刚刚";
  if (diffMin < 60) return `${diffMin} 分钟前`;
  if (diffMin < 12 * 60) return `${Math.floor(diffMin / 60)} 小时前`;

  const dayDiff = Math.round((startOfDay(now) - startOfDay(d)) / 86_400_000);
  if (dayDiff === 0) return `今天 ${hm(d)}`;
  if (dayDiff === 1) return `昨天 ${hm(d)}`;
  if (dayDiff < 7) return `${dayDiff} 天前`;
  if (d.getFullYear() === now.getFullYear()) return `${d.getMonth() + 1}月${d.getDate()}日`;
  return `${d.getFullYear()}年${d.getMonth() + 1}月${d.getDate()}日`;
}

/** 完整时间：2026-10-07 14:32 */
export function formatDateTime(iso: string | Date): string {
  const d = typeof iso === "string" ? new Date(iso) : iso;
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${hm(d)}`;
}
