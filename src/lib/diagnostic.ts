// 把项目状态整理成一段可以直接粘贴给 AI 的诊断文本，以及分叉 / 冲突时「复制给 AI」的提示词。
// 只包含文件名和提交标题，不包含任何文件内容或 diff。
import type { CommitInfo, DiagnosticInfo, FileChange, Project } from "@/types";
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

function commitLines(commits: CommitInfo[]): string[] {
  return commits.length ? commits.map((c) => `- ${c.hash}  ${c.date}  ${c.subject}`) : ["- （无）"];
}

/**
 * 「复制给 AI」的提示词（分叉 / 自动合并有冲突时用）：项目名、路径、分支、领先 / 落后、冲突文件、
 * 两边最近的提交，并要求不用 force push、reset、rebase。
 */
export function buildAiPrompt(info: DiagnosticInfo): string {
  const { project: p, conflictFiles, recentCommits, recentRemoteCommits } = info;
  const conflict = conflictFiles.length > 0;
  const who = `我的 Git 项目「${p.name}」（${p.path}，分支 ${p.branch}）`;
  const counts = `本地比云端多 ${p.ahead} 个提交、少 ${p.behind} 个提交`;
  const lines = [
    conflict
      ? `${who}在两台电脑上改了同一处，自动合并失败。`
      : `${who}在两台电脑上都有新提交，本地和云端分叉了，需要合并。`,
    conflict ? `${counts}；冲突文件：${conflictFiles.join("、")}。` : `${counts}。`,
    "本地最近的提交：",
    ...commitLines(recentCommits),
    `云端最近的提交${p.upstream ? `（${p.upstream}）` : ""}：`,
    ...commitLines(recentRemoteCommits),
    "环境：Windows + Git 命令行。",
    conflict
      ? "请一步一步教我在这台电脑上解决冲突并推送，不要用 force push、reset 或 rebase。"
      : "请一步一步教我在这台电脑上合并并推送，不要用 force push、reset 或 rebase。",
  ];
  return lines.join("\n");
}
