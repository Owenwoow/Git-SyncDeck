// 假数据模式下的 api 实现（第一阶段的 Demo 逻辑）。只允许 src/api/index.ts 引用。
// 函数签名必须与 src/api/tauri.ts 完全一致。
import type {
  CommitInfo,
  GitInfo,
  Project,
  ScannedRepo,
  Settings,
  SyncItemResult,
  SyncProgressEvent,
  SyncResult,
} from "@/types";
import { buildDiagnosticText } from "@/lib/diagnostic";
import { DEFAULT_COMMIT_TEMPLATE, describeIssue, isAbnormal, sortProjects } from "@/lib/status";
import { formatDateTime } from "@/lib/time";
import { clone, db, delay, jitter } from "./db";
import { defaultSettings, pickableDirs } from "./repos";

// ---------------- 项目 ----------------

export async function checkGit(): Promise<GitInfo> {
  await delay(50);
  return { installed: true, version: "git version 2.50.1.windows.1（演示）" };
}

export async function listProjects(): Promise<Project[]> {
  await delay(300);
  return clone(db.monitoredProjects());
}

export async function refreshStatus(): Promise<Project[]> {
  await delay(800);
  return clone(db.monitoredProjects());
}

export async function addProjects(paths: string[]): Promise<Project[]> {
  await delay(400);
  const added: Project[] = [];
  for (const path of paths) {
    const repo = db.findRepoByPath(path);
    if (!repo || db.isMonitored(repo.id)) continue;
    db.monitor(repo.id);
    added.push(repo);
  }
  return clone(added);
}

export async function removeProject(id: string): Promise<void> {
  await delay(200);
  db.unmonitor(id);
}

// ---------------- 目录 ----------------

/** 不弹窗，返回一个与 defaultPath 不同的假目录 */
export async function pickDirectory(defaultPath?: string): Promise<string | null> {
  await delay(250);
  const others = pickableDirs.filter((d) => d.toLowerCase() !== defaultPath?.toLowerCase());
  return others[db.nextPickCount() % others.length];
}

export async function scanDirectory(dir: string): Promise<ScannedRepo[]> {
  await delay(900);
  const prefix = dir.replace(/[\\/]+$/, "").toLowerCase() + "\\";
  return db
    .allRepos()
    .filter((r) => r.path.toLowerCase().startsWith(prefix))
    .sort((a, b) => a.path.localeCompare(b.path))
    .map((r) => {
      const monitored = db.isMonitored(r.id);
      return {
        path: r.path,
        name: r.name,
        branch: r.branch,
        remoteUrl: r.remoteUrl,
        monitored,
        projectId: monitored ? r.id : null,
      };
    });
}

// ---------------- 同步 ----------------

export async function syncAll(onProgress: (e: SyncProgressEvent) => void): Promise<SyncResult> {
  const startedAt = new Date().toISOString();
  const queue = sortProjects(db.monitoredProjects());
  const total = queue.length;
  const items: SyncItemResult[] = [];

  for (const [index, original] of queue.entries()) {
    db.updateRepo(original.id, { status: "syncing" });
    onProgress({ type: "start", projectId: original.id, index, total });

    const abnormal = isAbnormal(original.status);
    const needsWork = original.status === "ahead" || original.status === "behind";
    await delay(needsWork ? jitter(800, 1200) : jitter(350, 600));

    let result: SyncItemResult;
    let updated: Project;
    if (abnormal) {
      const reason = describeIssue(original);
      updated = db.updateRepo(original.id, { status: original.status, issue: reason });
      result = { projectId: original.id, outcome: "failed", commits: 0, reason };
    } else {
      updated = db.updateRepo(original.id, {
        status: "synced",
        ahead: 0,
        behind: 0,
        lastSyncAt: new Date().toISOString(),
        issue: null,
        checkError: null,
      });
      result = {
        projectId: original.id,
        outcome:
          original.status === "ahead" ? "pushed" : original.status === "behind" ? "pulled" : "up-to-date",
        commits: original.status === "ahead" ? original.ahead : original.behind,
        reason: null,
      };
    }

    items.push(result);
    onProgress({ type: "done", result, project: clone(updated), index, total });
  }

  const failedCount = items.filter((i) => i.outcome === "failed").length;
  return { startedAt, finishedAt: new Date().toISOString(), items, syncedCount: items.length - failedCount, failedCount };
}

export async function commitAndPush(projectId: string, message: string): Promise<Project> {
  await delay(1200);
  const p = db.getRepo(projectId);
  if (!p) throw new Error("项目不存在");
  if (p.status !== "dirty") throw new Error("这个项目没有需要提交的改动");
  if (!message.trim()) throw new Error("提交说明不能为空");
  const updated = db.updateRepo(projectId, {
    status: "synced",
    changes: [],
    ahead: 0,
    behind: 0,
    lastSyncAt: new Date().toISOString(),
    issue: null,
    checkError: null,
  });
  return clone(updated);
}

const FAKE_SUBJECTS = ["更新 README", "修复路径里含空格时的问题", "初始化项目"];

export async function getDiagnosticText(projectId: string): Promise<string> {
  await delay(120);
  const p = db.getRepo(projectId);
  if (!p) throw new Error("项目不存在");
  const commits: CommitInfo[] = FAKE_SUBJECTS.map((subject, i) => ({
    hash: (0x1a2b3c4 + i * 0x111111).toString(16).slice(0, 7),
    date: formatDateTime(new Date(Date.now() - (i + 1) * 26 * 3_600_000)),
    subject,
  }));
  return buildDiagnosticText(p, commits);
}

// ---------------- 系统 ----------------

export async function openInTerminal(projectId: string): Promise<string> {
  await delay(100);
  return db.getRepo(projectId)?.path ?? "";
}

export async function openInEditor(projectId: string): Promise<string> {
  await delay(100);
  return db.getRepo(projectId)?.path ?? "";
}

// ---------------- 设置（存在 WebView 的 localStorage 里）----------------

const SETTINGS_KEY = "syncdeck.settings";

function systemTheme(): Settings["theme"] {
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function readSettings(): Settings {
  const fallback: Settings = { ...defaultSettings, theme: systemTheme() };
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    return raw ? { ...fallback, ...JSON.parse(raw) } : fallback;
  } catch {
    return fallback;
  }
}

export async function getSettings(): Promise<Settings> {
  await delay(50);
  return readSettings();
}

export async function saveSettings(patch: Partial<Settings>): Promise<Settings> {
  await delay(100);
  const next = { ...readSettings(), ...patch };
  // 和真实实现一致：模板为空时恢复默认
  next.commitTemplate = next.commitTemplate.trim() || DEFAULT_COMMIT_TEMPLATE;
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(next));
  } catch {
    // 存储不可用时只在本次会话生效
  }
  return next;
}
