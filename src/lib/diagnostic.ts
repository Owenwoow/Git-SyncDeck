// 把项目状态整理成一段可以直接粘贴给 AI 的诊断文本。
// 只包含文件名，不包含任何文件内容或 diff。
import type { CommitInfo, FileChange, Project } from "@/types";
import { CHANGE_KIND_META, STATUS_META, describeIssue } from "./status";
import { formatDateTime } from "./time";

function fileLine(c: FileChange): string {
  const name = c.kind === "R" && c.oldPath ? `${c.oldPath} → ${c.path}` : c.path;
  return c.kind === "?" ? `- ${name}` : `- ${name}（${CHANGE_KIND_META[c.kind].label}）`;
}

function fileSection(title: string, files: FileChange[]): string[] {
  return [`【${title}】${files.length ? `${files.length} 个` : "无"}`, ...files.map(fileLine)];
}

export function buildDiagnosticText(p: Project, recentCommits: CommitInfo[], now: Date = new Date()): string {
  const issue = describeIssue(p);
  const staged = p.changes.filter((c) => c.staged);
  const unstaged = p.changes.filter((c) => !c.staged && c.kind !== "?");
  const untracked = p.changes.filter((c) => c.kind === "?");

  const lines = [
    `【项目】${p.name}`,
    `【本地路径】${p.path}`,
    `【当前分支】${p.branch}`,
    `【上游分支】${p.upstream ?? "无（没有设置上游分支）"}`,
    `【远程仓库】${p.remoteUrl ?? "未配置"}`,
    `【同步状态】${STATUS_META[p.status].label}${issue ? `：${issue}` : ""}`,
    ...(p.checkError ? [`【检查失败】${p.checkError}`] : []),
    `【领先 / 落后】本地领先 ${p.ahead} 个提交，落后 ${p.behind} 个提交`,
    ...fileSection("已暂存的文件", staged),
    ...fileSection("未暂存的文件", unstaged),
    ...fileSection("未跟踪的文件", untracked),
    `【最近 ${recentCommits.length || 3} 次提交】${recentCommits.length ? "" : "无"}`,
    ...recentCommits.map((c) => `- ${c.hash}  ${c.date}  ${c.subject}`),
    `【检测时间】${formatDateTime(now)}`,
    "",
    "环境：Windows + Git 命令行，台式机和笔记本两台电脑通过 GitHub 同步同一个仓库。",
    "我想把这个项目同步到 GitHub，请告诉我该怎么处理。",
  ];
  return lines.join("\n");
}
