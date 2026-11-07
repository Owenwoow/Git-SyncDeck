import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { toast } from "sonner";
import * as api from "@/api";
import type { TaskState } from "@/types";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";

const IDLE: TaskState = { active: null, waiting: [], installingUpdate: false };
const TasksContext = createContext<{ tasks: TaskState; requestQuit: () => Promise<void> } | null>(null);

export function TasksProvider({ children }: { children: ReactNode }) {
  const [tasks, setTasks] = useState<TaskState>(IDLE);
  const [confirmation, setConfirmation] = useState<TaskState | null>(null);
  const [quitting, setQuitting] = useState(false);
  const revision = useRef(0);

  useEffect(() => {
    let disposed = false;
    const offs: (() => void)[] = [];
    const accept = (next: TaskState) => {
      revision.current++;
      if (!disposed) setTasks(next);
    };
    (async () => {
      try {
        for (const registration of [
          api.onTasksChanged(accept),
          api.onQuitRequested((next) => { if (!disposed) setConfirmation(next); }),
        ]) {
          const off = await registration;
          if (disposed) off(); else offs.push(off);
        }
        const seq = revision.current;
        const next = await api.getTaskState();
        if (!disposed && revision.current === seq) setTasks(next);
      } catch (e) {
        if (!disposed) toast.error("读取后台任务状态失败", { description: String(e) });
      }
    })();
    return () => { disposed = true; offs.forEach((off) => off()); };
  }, []);

  const requestQuit = useCallback(async () => {
    try {
      const busy = await api.quitApp();
      if (busy) setConfirmation(busy);
    } catch (e) {
      toast.error("无法退出", { description: e instanceof Error ? e.message : String(e) });
    }
  }, []);

  async function confirmQuit() {
    setQuitting(true);
    try {
      await api.quitApp(true);
      setConfirmation(null);
    } catch (e) {
      toast.error("退出失败", { description: e instanceof Error ? e.message : String(e) });
    } finally { setQuitting(false); }
  }

  const current = tasks.active || tasks.waiting.length || tasks.installingUpdate ? tasks : confirmation;
  return (
    <TasksContext.Provider value={{ tasks, requestQuit }}>
      {children}
      <Dialog open={confirmation !== null} onOpenChange={(open) => { if (!open && !quitting) setConfirmation(null); }}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>退出会中断当前任务</DialogTitle>
            <DialogDescription>
              {current?.installingUpdate ? "正在安装更新。" : `正在进行：${current?.active?.label ?? "等待执行的仓库任务"}。`}
              {current?.waiting.length ? `另有 ${current.waiting.length} 项任务等待执行。` : ""}
              立即退出会中断操作，尚未推送的提交会保留在本地。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setConfirmation(null)} disabled={quitting}>取消退出</Button>
            <Button variant="destructive" onClick={confirmQuit} disabled={quitting || current?.installingUpdate}>立即退出</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </TasksContext.Provider>
  );
}

export function useTasks() {
  const value = useContext(TasksContext);
  if (!value) throw new Error("useTasks 必须在 TasksProvider 内使用");
  return value;
}
