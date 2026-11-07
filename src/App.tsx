import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Sidebar, type Page } from "@/components/layout/Sidebar";
import { SyncDialog } from "@/components/sync/SyncDialog";
import { HomePage } from "@/pages/HomePage";
import { AddProjectsPage } from "@/pages/AddProjectsPage";
import { SettingsPage, type SettingsCategory } from "@/pages/SettingsPage";
import { ProjectsProvider } from "@/state/projects";
import { SettingsProvider, useSettings } from "@/state/settings";
import { TasksProvider, useTasks } from "@/state/tasks";
import { UpdatesProvider, useUpdates } from "@/state/updates";

export default function App() {
  return (
    <SettingsProvider>
      <TasksProvider>
      <UpdatesProvider>
      <ProjectsProvider>
        <TooltipProvider delayDuration={300}>
          <Shell />
        </TooltipProvider>
      </ProjectsProvider>
      </UpdatesProvider>
      </TasksProvider>
    </SettingsProvider>
  );
}

function Shell() {
  const [page, setPage] = useState<Page>("home");
  // 设置页停在哪一类：只记在内存里，切到别的页面再回来还是上次看的；第一次是「常规」
  const [settingsCategory, setSettingsCategory] = useState<SettingsCategory>("general");
  const { settings } = useSettings();
  const { update, check } = useUpdates();
  const { tasks } = useTasks();
  const checkedRef = useRef(false);

  // 启动后静默检查一次更新：有新版本时提示，失败不打扰用户
  useEffect(() => {
    if (checkedRef.current) return;
    checkedRef.current = true;
    check(true).then(
      (info) => {
        if (!info) return;
        toast(`发现新版本 v${info.version}`, {
          description: `当前版本 v${info.currentVersion}，可在设置页先下载，任务空闲后安装。`,
          duration: 12000,
          action: {
            label: "去更新",
            onClick: () => {
              setSettingsCategory("about");
              setPage("settings");
            },
          },
        });
      },
      () => {},
    );
  }, [check]);

  return (
    <div className="flex h-full overflow-hidden">
      <Sidebar page={page} onNavigate={setPage} updateAvailable={update !== null} />
      {/* @container：内容区按自身宽度调整列表列数，不依赖窗口宽度 */}
      <main className="@container min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-5xl px-6 py-6 wide:px-8">
          {(tasks.active || tasks.waiting.length > 0 || tasks.installingUpdate) && (
            <div className="mb-4 rounded-md border bg-muted/40 px-3 py-2 text-sm" role="status">
              {tasks.installingUpdate ? "正在安装更新，仓库任务已暂停进入" : tasks.active ? `正在进行：${tasks.active.label}` : "仓库任务正在等待执行"}
              {tasks.waiting.length > 0 && ` · ${tasks.waiting.length} 项等待中`}
            </div>
          )}
          {page === "home" && <HomePage onAddProjects={() => setPage("add")} />}
          {page === "add" && <AddProjectsPage />}
          {page === "settings" && (
            <SettingsPage
              category={settingsCategory}
              onCategoryChange={setSettingsCategory}
            />
          )}
        </div>
      </main>
      <SyncDialog />
      <Toaster theme={settings?.theme ?? "light"} position="bottom-right" richColors={false} />
    </div>
  );
}
