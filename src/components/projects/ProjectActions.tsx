import { useState } from "react";
import { ChevronDown, Code2, Copy, ExternalLink, SkipForward, SquareTerminal, Upload } from "lucide-react";
import { toast } from "sonner";
import * as api from "@/api";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { CommitPushDialog } from "@/components/projects/CommitPushDialog";
import { copyText } from "@/lib/clipboard";
import { commitPushDisabledReason } from "@/lib/status";
import type { Project } from "@/types";

/**
 * 异常项目的操作按钮组。一键同步结果和详情抽屉共用。
 * 详情抽屉里不传 onSkip，就不显示"本次跳过"。
 */
export function ProjectActions({ project, onSkip }: { project: Project; onSkip?: () => void }) {
  const [commitOpen, setCommitOpen] = useState(false);
  const disabledReason = commitPushDisabledReason(project);

  async function copyDiagnostic() {
    try {
      await copyText(await api.getDiagnosticText(project.id));
      toast.success("诊断文本已复制", { description: "可以直接粘贴给 AI" });
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "复制失败");
    }
  }

  async function open(target: "terminal" | "editor") {
    const name = target === "terminal" ? "终端" : "VS Code";
    try {
      const path =
        target === "terminal" ? await api.openInTerminal(project.id) : await api.openInEditor(project.id);
      if (api.isDemoMode) {
        toast.info(`演示模式：不会真的打开${name}`, { description: path });
      } else {
        toast.success(`已在${name}中打开`, { description: path });
      }
    } catch (e) {
      toast.error(`无法打开${name}`, { description: e instanceof Error ? e.message : String(e) });
    }
  }

  return (
    <div className="flex flex-wrap items-center gap-2">
      <Button variant="outline" size="sm" onClick={copyDiagnostic}>
        <Copy />
        复制诊断文本
      </Button>

      {disabledReason ? (
        <Tooltip>
          <TooltipTrigger asChild>
            {/* 禁用的按钮收不到鼠标事件，外面包一层让提示能显示 */}
            <span tabIndex={0} className="inline-flex rounded-md">
              <Button variant="outline" size="sm" disabled>
                <Upload />
                提交并推送
              </Button>
            </span>
          </TooltipTrigger>
          <TooltipContent>{disabledReason}</TooltipContent>
        </Tooltip>
      ) : (
        <Button variant="outline" size="sm" onClick={() => setCommitOpen(true)}>
          <Upload />
          提交并推送
        </Button>
      )}

      {onSkip && (
        <Button variant="outline" size="sm" onClick={onSkip}>
          <SkipForward />
          本次跳过
        </Button>
      )}

      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button variant="outline" size="sm">
            <ExternalLink />
            打开
            <ChevronDown className="opacity-60" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          <DropdownMenuItem onSelect={() => open("terminal")}>
            <SquareTerminal />
            在终端中打开
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => open("editor")}>
            <Code2 />
            在 VS Code 中打开
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      {!disabledReason && <CommitPushDialog project={project} open={commitOpen} onOpenChange={setCommitOpen} />}
    </div>
  );
}
