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
import { FileChangeItem } from "@/components/projects/FileChangeList";
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

  useEffect(() => {
    if (open) setMessage("");
  }, [open]);

  async function submit() {
    if (!message.trim() || submitting) return;
    setSubmitting(true);
    try {
      await commitAndPush(project.id, message.trim());
      toast.success(`${project.name} 已提交并推送`);
      onOpenChange(false);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "提交失败");
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
          <div className="mb-1.5 text-xs text-muted-foreground">将提交以下 {project.changes.length} 个文件</div>
          <ul className="max-h-36 overflow-y-auto rounded-md border bg-muted/30 px-3 py-2">
            {project.changes.map((c) => (
              <FileChangeItem key={`${c.kind}:${c.path}`} change={c} />
            ))}
          </ul>
        </div>

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
          <Button onClick={submit} disabled={!message.trim() || submitting}>
            {submitting ? <Loader2 className="animate-spin" /> : <Upload />}
            {submitting ? "正在提交…" : "提交并推送"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
