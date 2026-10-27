// 真实数据模式：调用 Rust 端的 Tauri 命令（src-tauri/src/commands.rs）。
// 函数签名必须与 src/mock/api.ts 完全一致。
import { Channel, invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import type {
  AutoSyncEvent,
  AutoSyncState,
  CloneDefaults,
  CloneProgressEvent,
  CloneRequest,
  CloneResult,
  DiagnosticInfo,
  GitInfo,
  GithubListing,
  GithubRepo,
  GithubRepoEntry,
  Project,
  RefreshProgressEvent,
  ScannedRepo,
  Settings,
  SyncProgressEvent,
  SyncResult,
  Theme,
  Unlisten,
  UpdateInfo,
  UpdateProgress,
} from "@/types";
import { buildAiPrompt, buildDiagnosticText } from "@/lib/diagnostic";

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

/** git fetch + git status，最多 8 个项目并行；每个项目检查完就通过 onProgress 回调一次，最后返回完整列表 */
export function refreshStatus(onProgress?: (p: Project) => void): Promise<Project[]> {
  const onEvent = new Channel<RefreshProgressEvent>();
  if (onProgress) onEvent.onmessage = (e) => onProgress(e.project);
  return call("refresh_status", { onEvent });
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

// ---------------- 从 GitHub 克隆 ----------------

/** 打开「从 GitHub 克隆」时的默认值：上次输入的地址、gh 登录的账号、默认代码目录 */
export function getCloneDefaults(): Promise<CloneDefaults> {
  return call("get_clone_defaults");
}

/** 列出 GitHub 账号名下的仓库（或单个仓库）；输入可以是账号主页、用户名或仓库地址 */
export function listGithubRepos(input: string, parentDir?: string): Promise<GithubListing> {
  return call("list_github_repos", { input, parentDir });
}

/** 只读更新本地状态和目标占用情况，不重新请求 GitHub。 */
export function inspectGithubTargets(repos: GithubRepo[], parentDir: string): Promise<GithubRepoEntry[]> {
  return call("inspect_github_targets", { repos, parentDir });
}

/** 克隆到本批保存目录并加入监控；未指定时用默认目录，一个失败不影响其他 */
export function cloneRepos(
  items: CloneRequest[],
  onProgress: (e: CloneProgressEvent) => void,
  parentDir?: string,
): Promise<CloneResult[]> {
  const onEvent = new Channel<CloneProgressEvent>();
  onEvent.onmessage = onProgress;
  return call("clone_repos", { items, onEvent, parentDir });
}

export function commitAndPush(projectId: string, message: string): Promise<Project> {
  return call("commit_and_push", { projectId, message });
}

export async function getDiagnosticText(projectId: string): Promise<string> {
  const info = await call<DiagnosticInfo>("get_diagnostic_info", { projectId });
  return buildDiagnosticText(info.project, info.recentCommits);
}

/** 「复制给 AI」的提示词：分叉时后端会预演一次合并（不联网、不碰工作区），列出会冲突的文件 */
export async function getAiPromptText(projectId: string): Promise<string> {
  return buildAiPrompt(await call<DiagnosticInfo>("get_diagnostic_info", { projectId }));
}

// ---------------- 系统 ----------------

export function openInTerminal(projectId: string): Promise<string> {
  return call("open_in_terminal", { projectId });
}

export function openInEditor(projectId: string): Promise<string> {
  return call("open_in_editor", { projectId });
}

export function openProjectHomepage(): Promise<void> {
  return call("open_project_homepage");
}

// ---------------- 设置（%APPDATA%\com.gitsyncdeck.desktop\config.json）----------------

function systemTheme(): Theme {
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

type StoredSettings = Omit<Settings, "theme"> & { theme: Theme | null };

function toSettings(s: StoredSettings): Settings {
  return { ...s, theme: s.theme ?? systemTheme() };
}

export async function getSettings(): Promise<Settings> {
  return toSettings(await call<StoredSettings>("get_settings"));
}

export async function saveSettings(patch: Partial<Settings>): Promise<Settings> {
  return toSettings(await call<StoredSettings>("save_settings", { patch }));
}

/** 托盘菜单里暂停 / 恢复自动同步后，后端推送最新设置 */
export function onSettingsChanged(cb: (s: Settings) => void): Promise<Unlisten> {
  return listen<StoredSettings>("settings-changed", (e) => cb(toSettings(e.payload)));
}

// ---------------- 换电脑自动同步（后台进行，结果通过事件推送）----------------

/** 暂停自动同步到明天 00:00，返回最新设置 */
export async function pauseAutoSync(): Promise<Settings> {
  return toSettings(await call<StoredSettings>("pause_auto_sync"));
}

/** 恢复自动同步，返回最新设置 */
export async function resumeAutoSync(): Promise<Settings> {
  return toSettings(await call<StoredSettings>("resume_auto_sync"));
}

/** 后台自动同步的当前状态（启动时查一次，之后靠 onAutoSync） */
export function getAutoSyncState(): Promise<AutoSyncState> {
  return call("get_auto_sync_state");
}

/** 彻底退出 SyncDeck（和托盘菜单「退出」一样） */
export function quitApp(): Promise<void> {
  return call("quit_app");
}

/** 后台检查 / 同步之后的最新项目列表 */
export function onProjectsChanged(cb: (list: Project[]) => void): Promise<Unlisten> {
  return listen<Project[]>("projects-changed", (e) => cb(e.payload));
}

/** 后台自动同步开始 / 进度 / 结束 */
export async function onAutoSync(handlers: {
  started: (e: AutoSyncEvent) => void;
  progress: (e: SyncProgressEvent) => void;
  finished: (e: AutoSyncEvent) => void;
}): Promise<Unlisten> {
  const offs = await Promise.all([
    listen<AutoSyncEvent>("auto-sync-started", (e) => handlers.started(e.payload)),
    listen<SyncProgressEvent>("auto-sync-progress", (e) => handlers.progress(e.payload)),
    listen<AutoSyncEvent>("auto-sync-finished", (e) => handlers.finished(e.payload)),
  ]);
  return () => offs.forEach((off) => off());
}

// ---------------- 应用更新（tauri-plugin-updater，读取 GitHub Releases 的 latest.json）----------------

/** 最近一次 checkForUpdate 发现的新版本，installUpdate 用它下载安装 */
let pendingUpdate: Update | null = null;

export function getAppVersion(): Promise<string> {
  return getVersion();
}

/** 有新版本返回版本信息，已是最新返回 null；网络或签名问题抛出 Error */
export async function checkForUpdate(): Promise<UpdateInfo | null> {
  try {
    const update = await check();
    pendingUpdate = update;
    if (!update) return null;
    return {
      version: update.version,
      currentVersion: update.currentVersion,
      date: update.date ?? null,
      notes: update.body ?? null,
    };
  } catch (e) {
    pendingUpdate = null;
    throw new Error(typeof e === "string" ? e : e instanceof Error ? e.message : String(e));
  }
}

/** 下载并安装最近发现的新版本，完成后重启应用 */
export async function installUpdate(onProgress: (p: UpdateProgress) => void): Promise<void> {
  const update = pendingUpdate;
  if (!update) throw new Error("没有可安装的更新，请先检查更新");
  let downloaded = 0;
  let total: number | null = null;
  try {
    await update.downloadAndInstall((event) => {
      if (event.event === "Started") {
        total = event.data.contentLength ?? null;
        onProgress({ downloaded: 0, total });
      } else if (event.event === "Progress") {
        downloaded += event.data.chunkLength;
        onProgress({ downloaded, total });
      }
    });
  } catch (e) {
    throw new Error(typeof e === "string" ? e : e instanceof Error ? e.message : String(e));
  }
  await relaunch();
}
