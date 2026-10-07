import { useState } from "react";
import { Toaster } from "@/components/ui/sonner";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Sidebar, type Page } from "@/components/layout/Sidebar";
import { SyncDialog } from "@/components/sync/SyncDialog";
import { HomePage } from "@/pages/HomePage";
import { AddProjectsPage } from "@/pages/AddProjectsPage";
import { SettingsPage } from "@/pages/SettingsPage";
import { ProjectsProvider } from "@/state/projects";
import { SettingsProvider, useSettings } from "@/state/settings";

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

  return (
    <div className="flex h-full overflow-hidden">
      <Sidebar page={page} onNavigate={setPage} />
      {/* @container：内容区按自身宽度调整列表列数，不依赖窗口宽度 */}
      <main className="@container min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-5xl px-6 py-6 wide:px-8">
          {page === "home" && <HomePage onAddProjects={() => setPage("add")} />}
          {page === "add" && <AddProjectsPage />}
          {page === "settings" && <SettingsPage />}
        </div>
      </main>
      <SyncDialog />
      <Toaster theme={settings?.theme ?? "light"} position="bottom-right" richColors={false} />
    </div>
  );
}
