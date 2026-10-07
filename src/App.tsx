import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import * as api from "@/api";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Sidebar, type Page } from "@/components/layout/Sidebar";
import { SyncDialog } from "@/components/sync/SyncDialog";
import { HomePage } from "@/pages/HomePage";
import { AddProjectsPage } from "@/pages/AddProjectsPage";
import { SettingsPage } from "@/pages/SettingsPage";
import { ProjectsProvider } from "@/state/projects";
import { SettingsProvider, useSettings } from "@/state/settings";
import type { UpdateInfo } from "@/types";

export default function App() {
  return (
    <SettingsProvider>
      <ProjectsProvider>
        <TooltipProvider delayDuration={300}>
          <Shell />
        </TooltipProvider>
      </ProjectsProvider>
    </SettingsProvider>
  );
}

function Shell() {
  const [page, setPage] = useState<Page>("home");
  const { settings } = useSettings();
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const checkedRef = useRef(false);

  // 启动后静默检查一次更新：有新版本时提示，失败不打扰用户
  useEffect(() => {
    if (checkedRef.current) return;
    checkedRef.current = true;
    api.checkForUpdate().then(
      (info) => {
        if (!info) return;
        setUpdate(info);
        toast(`发现新版本 v${info.version}`, {
          description: `当前版本 v${info.currentVersion}，可以在设置页下载并安装。`,
          duration: 12000,
          action: { label: "去更新", onClick: () => setPage("settings") },
        });
      },
      () => {},
    );
  }, []);

  return (
    <div className="flex h-full overflow-hidden">
      <Sidebar page={page} onNavigate={setPage} updateAvailable={update !== null} />
      {/* @container：内容区按自身宽度调整列表列数，不依赖窗口宽度 */}
      <main className="@container min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-5xl px-6 py-6 wide:px-8">
          {page === "home" && <HomePage onAddProjects={() => setPage("add")} />}
          {page === "add" && <AddProjectsPage />}
          {page === "settings" && <SettingsPage pendingUpdate={update} />}
        </div>
      </main>
      <SyncDialog />
      <Toaster theme={settings?.theme ?? "light"} position="bottom-right" richColors={false} />
    </div>
  );
}
