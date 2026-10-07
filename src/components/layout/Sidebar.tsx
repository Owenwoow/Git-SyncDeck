import { FlaskConical, FolderPlus, FolderSync, LayoutList, Settings, type LucideIcon } from "lucide-react";
import { isDemoMode } from "@/api";
import { cn } from "@/lib/utils";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

export type Page = "home" | "add" | "settings";

const NAV: { page: Page; label: string; icon: LucideIcon }[] = [
  { page: "home", label: "主页", icon: LayoutList },
  { page: "add", label: "添加项目", icon: FolderPlus },
  { page: "settings", label: "设置", icon: Settings },
];

/** 左侧导航。窗口宽度 < 960px 时收成纯图标，悬停显示文字 */
export function Sidebar({
  page,
  onNavigate,
  updateAvailable = false,
}: {
  page: Page;
  onNavigate: (p: Page) => void;
  /** 有新版本时在"设置"上显示小圆点 */
  updateAvailable?: boolean;
}) {
  return (
    <aside className="flex w-15 shrink-0 flex-col border-r bg-sidebar px-2.5 py-4 wide:w-52 wide:px-3">
      <div className="mb-6 flex items-center gap-2.5 px-1.5">
        <div className="flex size-7 shrink-0 items-center justify-center rounded-md bg-primary text-primary-foreground">
          <FolderSync className="size-4" />
        </div>
        <span className="hidden text-[15px] font-semibold tracking-tight wide:inline">SyncDeck</span>
      </div>

      <nav className="flex flex-col gap-0.5">
        {NAV.map(({ page: p, label, icon: Icon }) => {
          const active = p === page;
          return (
            <Tooltip key={p}>
              <TooltipTrigger asChild>
                <button
                  type="button"
                  onClick={() => onNavigate(p)}
                  aria-current={active ? "page" : undefined}
                  className={cn(
                    "flex h-9 items-center gap-2.5 rounded-md px-2.5 text-sm transition-colors",
                    active
                      ? "bg-sidebar-accent font-medium text-sidebar-accent-foreground"
                      : "text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground",
                  )}
                >
                  <span className="relative shrink-0">
                    <Icon className="size-4" />
                    {p === "settings" && updateAvailable && (
                      <span
                        className="absolute -top-0.5 -right-0.5 size-2 rounded-full bg-status-ahead ring-2 ring-sidebar"
                        aria-label="有新版本"
                      />
                    )}
                  </span>
                  <span className="hidden wide:inline">{label}</span>
                </button>
              </TooltipTrigger>
              <TooltipContent side="right" sideOffset={8} className="wide:hidden">
                {label}
              </TooltipContent>
            </Tooltip>
          );
        })}
      </nav>

      {isDemoMode && (
        <Tooltip>
          <TooltipTrigger asChild>
            <div className="mt-auto flex items-center gap-2 rounded-md px-2.5 py-2 text-xs text-muted-foreground">
              <FlaskConical className="size-4 shrink-0" />
              <span className="hidden wide:inline">演示数据</span>
            </div>
          </TooltipTrigger>
          <TooltipContent side="right" sideOffset={8}>
            假数据模式（VITE_USE_MOCK=1 或在浏览器中打开）：数据来自 src/mock/，不会执行任何 git 操作
          </TooltipContent>
        </Tooltip>
      )}
    </aside>
  );
}
