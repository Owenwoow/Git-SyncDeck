import { useEffect, useState } from "react";
import { Loader2, Upload } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { FileChangeList } from "@/components/projects/FileChangeList";
import { commitPushDisabledReason, uniqueChangedFileCount } from "@/lib/status";
import { useProjects } from "@/state/projects";
import type { Project } from "@/types";

/** "直接提交并推送"弹窗：列出将提交的文件 + 填写提交说明 */
export function CommitPushDialog({
  project,
  open,
  onOpenChange,
}: {
  project: Project;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { commitAndPush } = useProjects();
  const [message, setMessage] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const disabledReason = commitPushDisabledReason(project);

  useEffect(() => {
    if (open) {
      setMessage("");
      setError(null);
    }
  }, [open]);

  async function submit() {
    if (!message.trim() || submitting || disabledReason) return;
    setSubmitting(true);
    setError(null);
    try {
      await commitAndPush(project.id, message.trim());
      toast.success(`${project.name} 已提交并推送`);
      onOpenChange(false);
    } catch (e) {
      const reason = e instanceof Error ? e.message : "提交失败";
      setError(reason);
      toast.error(reason);
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={(o) => !submitting && onOpenChange(o)}>
      <DialogContent className="gap-5 sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>提交并推送</DialogTitle>
          <DialogDescription>
            {project.name} · {project.branch}
            {project.upstream && ` → ${project.upstream}`}
          </DialogDescription>
        </DialogHeader>

        <div>
          <div className="mb-1.5 text-xs text-muted-foreground">将提交以下 {uniqueChangedFileCount(project.changes)} 个文件的全部改动</div>
          <FileChangeList changes={project.changes} className="max-h-36 overflow-y-auto rounded-md border bg-muted/30 px-3 py-2" />
        </div>

        <p className="text-xs leading-relaxed text-muted-foreground">手动提交不执行大文件、疑似密钥文件的“自动提交前检查”，也不会自动合并。请先自行核对全部改动；未解决冲突或未完成的 Git 操作始终会阻止提交，需到外部工具处理。</p>
        {(disabledReason || error) && <p role="alert" className="text-xs break-words whitespace-pre-wrap text-status-diverged select-text">{disabledReason || error}</p>}

        <div className="grid gap-1.5">
          <Label htmlFor="commit-message">提交说明</Label>
          <Textarea
            id="commit-message"
            autoFocus
            value={message}
            onChange={(e) => setMessage(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && e.ctrlKey) submit();
            }}
            placeholder="例如：更新 DC-9 靶机 writeup"
            className="min-h-20 resize-none"
          />
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={submitting}>
            取消
          </Button>
          <Button onClick={submit} disabled={!message.trim() || submitting || !!disabledReason}>
            {submitting ? <Loader2 className="animate-spin" /> : <Upload />}
            {submitting ? "正在提交…" : "提交并推送"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
