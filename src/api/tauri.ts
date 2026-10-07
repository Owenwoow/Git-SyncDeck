// 真实数据模式：调用 Rust 端的 Tauri 命令（src-tauri/src/commands.rs）。
// 函数签名必须与 src/mock/api.ts 完全一致。
import { Channel, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  CommitInfo,
  GitInfo,
  Project,
  ScannedRepo,
  Settings,
  SyncProgressEvent,
  SyncResult,
  Theme,
} from "@/types";
import { buildDiagnosticText } from "@/lib/diagnostic";

/** Rust 端返回的错误是字符串，这里统一包成 Error，界面可以直接用 e.message */
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw new Error(typeof e === "string" ? e : e instanceof Error ? e.message : String(e));
  }
}

// ---------------- 项目 ----------------

export function checkGit(): Promise<GitInfo> {
  return call("check_git");
}

/** 本地检查（不联网），启动时用 */
export function listProjects(): Promise<Project[]> {
  return call("list_projects");
}

/** git fetch + git status，最多 4 个项目并行 */
export function refreshStatus(): Promise<Project[]> {
  return call("refresh_status");
}

export function addProjects(paths: string[]): Promise<Project[]> {
  return call("add_projects", { paths });
}

export function removeProject(id: string): Promise<void> {
  return call("remove_project", { id });
}

/** 打开 / 关闭"一键同步时自动提交"，返回最新的项目 */
export function setAutoCommit(projectId: string, enabled: boolean): Promise<Project> {
  return call("set_auto_commit", { projectId, enabled });
}

// ---------------- 目录 ----------------

export async function pickDirectory(defaultPath?: string): Promise<string | null> {
  const picked = await open({ directory: true, multiple: false, defaultPath, title: "选择代码目录" });
  return typeof picked === "string" ? picked : null;
}

export function scanDirectory(dir: string): Promise<ScannedRepo[]> {
  return call("scan_directory", { dir });
}

// ---------------- 同步 ----------------

/** 进度通过 Tauri Channel 实时推送 */
export function syncAll(onProgress: (e: SyncProgressEvent) => void): Promise<SyncResult> {
  const onEvent = new Channel<SyncProgressEvent>();
  onEvent.onmessage = onProgress;
  return call("sync_all", { onEvent });
}

export function commitAndPush(projectId: string, message: string): Promise<Project> {
  return call("commit_and_push", { projectId, message });
}

export async function getDiagnosticText(projectId: string): Promise<string> {
  const info = await call<{ project: Project; recentCommits: CommitInfo[] }>("get_diagnostic_info", { projectId });
  return buildDiagnosticText(info.project, info.recentCommits);
}

// ---------------- 系统 ----------------

export function openInTerminal(projectId: string): Promise<string> {
  return call("open_in_terminal", { projectId });
}

export function openInEditor(projectId: string): Promise<string> {
  return call("open_in_editor", { projectId });
}

// ---------------- 设置（%APPDATA%\com.gitsyncdeck.desktop\config.json）----------------

function systemTheme(): Theme {
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

type StoredSettings = { defaultCodeDir: string; theme: Theme | null; commitTemplate: string };

function toSettings(s: StoredSettings): Settings {
  return { defaultCodeDir: s.defaultCodeDir, theme: s.theme ?? systemTheme(), commitTemplate: s.commitTemplate };
}

export async function getSettings(): Promise<Settings> {
  return toSettings(await call<StoredSettings>("get_settings"));
}

export async function saveSettings(patch: Partial<Settings>): Promise<Settings> {
  return toSettings(await call<StoredSettings>("save_settings", { patch }));
}
