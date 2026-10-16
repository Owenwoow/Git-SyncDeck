// 假数据模式下的 api 实现（第一阶段的 Demo 逻辑）。只允许 src/api/index.ts 引用。
// 函数签名必须与 src/api/tauri.ts 完全一致。
import { toast } from "sonner";
import type {
  AutoSyncEvent,
  AutoSyncState,
  CommitInfo,
  GitInfo,
  Project,
  ProxyMode,
  ScannedRepo,
  Settings,
  SyncItemResult,
  SyncProgressEvent,
  SyncResult,
  Unlisten,
  UpdateInfo,
  UpdateProgress,
} from "@/types";
import { buildDiagnosticText } from "@/lib/diagnostic";
import { DEFAULT_COMMIT_TEMPLATE, NETWORK_TIMEOUT_CHOICES, describeIssue, isAbnormal, sortProjects, willAutoCommit } from "@/lib/status";
import { formatDateTime } from "@/lib/time";
import { clone, db, delay, jitter } from "./db";
import { defaultSettings, demoHostName, demoSystemProxy, pickableDirs } from "./repos";

// ---------------- 项目 ----------------

export async function checkGit(): Promise<GitInfo> {
  await delay(50);
  return { installed: true, version: "git version 2.50.1.windows.1（演示）" };
}

export async function listProjects(): Promise<Project[]> {
  await delay(300);
  return clone(db.monitoredProjects());
}

/** 正在进行的刷新；一键同步要等它结束（和真实实现一样，联网操作互斥） */
let refreshing: Promise<unknown> | null = null;

/** 演示：每个项目各自"联网"一段时间，检查完就回调一次 */
export async function refreshStatus(onProgress?: (p: Project) => void): Promise<Project[]> {
  const run = (async () => {
    await Promise.all(
      db.monitoredProjects().map(async (p) => {
        await delay(jitter(400, 2600));
        const latest = db.getRepo(p.id);
        if (latest && db.isMonitored(p.id)) onProgress?.(clone(latest));
      }),
    );
    return clone(db.monitoredProjects());
  })();
  refreshing = run;
  try {
    return await run;
  } finally {
    if (refreshing === run) refreshing = null;
  }
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

export async function setAutoCommit(projectId: string, enabled: boolean): Promise<Project> {
  await delay(150);
  if (!db.getRepo(projectId)) throw new Error("项目不存在");
  return clone(db.updateRepo(projectId, { autoCommit: enabled }));
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
  // 刷新进行中时先等它结束，再复用刚检查过的结果
  await refreshing?.catch(() => undefined);
  const queue = sortProjects(db.monitoredProjects());
  const total = queue.length;
  const items: SyncItemResult[] = [];

  for (const [index, original] of queue.entries()) {
    db.updateRepo(original.id, { status: "syncing" });
    onProgress({ type: "start", projectId: original.id, index, total });

    const abnormal = isAbnormal(original.status);
    // 打开了自动提交的"有未提交改动"项目：演示自动提交并推送
    const autoCommit = willAutoCommit(original);
    const needsWork = original.status === "ahead" || original.status === "behind" || autoCommit;
    await delay(needsWork ? jitter(800, 1200) : jitter(350, 600));

    let result: SyncItemResult;
    let updated: Project;
    if (autoCommit) {
      updated = db.updateRepo(original.id, {
        status: "synced",
        changes: [],
        ahead: 0,
        behind: 0,
        lastSyncAt: new Date().toISOString(),
        issue: null,
        checkError: null,
      });
      result = { projectId: original.id, outcome: "committed", commits: original.ahead + 1, reason: null };
    } else if (abnormal) {
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

/** 保存在 localStorage 里的部分（实际生效的代理、系统电脑名是每次读取时算出来的） */
type StoredSettings = Omit<Settings, "effectiveProxy" | "proxySource" | "defaultHostName">;

const PROXY_MODES: readonly ProxyMode[] = ["auto", "off", "manual"];
const PROXY_FORMAT_HINT =
  "代理地址格式不对：要以 http://、https://、socks5:// 或 socks5h:// 开头，并写明主机和端口，如 http://127.0.0.1:7890";

/** 和真实实现（src-tauri/src/url.rs 的 validate_proxy_url）规则一致：协议、主机、端口都要有，不能带路径 */
function normalizeProxyUrl(input: string): string {
  const m = /^(https?|socks5h?):\/\/([^/?#\s]+?)\/?$/i.exec(input.trim());
  const hostPort = m ? m[2].slice(m[2].lastIndexOf("@") + 1) : "";
  const hp = /^(\[[0-9a-f:.]+\]|[A-Za-z0-9._-]+):(\d{1,5})$/i.exec(hostPort);
  const port = hp ? Number(hp[2]) : 0;
  if (!m || port < 1 || port > 65535) throw new Error(PROXY_FORMAT_HINT);
  return `${m[1].toLowerCase()}://${m[2]}`;
}

/** 演示：按代理模式算出"当前实际使用的代理"（自动模式假装探测到了系统代理）；补上系统电脑名，去掉已过期的暂停 */
function withProxy(stored: StoredSettings): Settings {
  const paused = stored.autoSyncPausedUntil && Date.parse(stored.autoSyncPausedUntil) > Date.now();
  const s = { ...stored, defaultHostName: demoHostName, autoSyncPausedUntil: paused ? stored.autoSyncPausedUntil : null };
  if (s.proxyMode === "off") return { ...s, effectiveProxy: null, proxySource: "none" };
  if (s.proxyMode === "manual" && s.proxyUrl) {
    return { ...s, effectiveProxy: s.proxyUrl.replace(/:\/\/[^@/]*@/, "://"), proxySource: "manual" };
  }
  return { ...s, effectiveProxy: demoSystemProxy, proxySource: "system" };
}

/** 和真实实现（commands.rs 的 validate_host_name）规则一致 */
function normalizeHostName(input: string): string {
  const name = input.trim();
  if (/[\u0000-\u001f\u007f]/.test(name)) throw new Error("本机名称不能包含换行等控制字符");
  if ([...name].length > 40) throw new Error("本机名称最多 40 个字");
  return name;
}

function writeSettings(s: StoredSettings) {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(s));
  } catch {
    // 存储不可用时只在本次会话生效
  }
}

function systemTheme(): Settings["theme"] {
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function readSettings(): StoredSettings {
  const fallback: StoredSettings = { ...defaultSettings, theme: systemTheme() };
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    return raw ? { ...fallback, ...JSON.parse(raw) } : fallback;
  } catch {
    return fallback;
  }
}

export async function getSettings(): Promise<Settings> {
  await delay(50);
  return withProxy(readSettings());
}

export async function saveSettings(patch: Partial<Settings>): Promise<Settings> {
  await delay(100);
  if (patch.networkTimeoutSecs !== undefined && !(NETWORK_TIMEOUT_CHOICES as readonly number[]).includes(patch.networkTimeoutSecs)) {
    throw new Error(`不支持的联网超时：${patch.networkTimeoutSecs} 秒`);
  }
  const cur = readSettings();
  // 代理：和真实实现一致，先校验，不合法时什么都不改
  if (patch.proxyMode !== undefined && !PROXY_MODES.includes(patch.proxyMode)) {
    throw new Error(`未知的代理模式：${patch.proxyMode}`);
  }
  const proxyMode = patch.proxyMode ?? cur.proxyMode;
  let proxyUrl = cur.proxyUrl;
  if (patch.proxyUrl !== undefined) {
    const url = (patch.proxyUrl ?? "").trim();
    proxyUrl = url ? normalizeProxyUrl(url) : null;
  }
  if (proxyMode === "manual" && !proxyUrl) throw new Error("选择手动填写时，请先填好代理地址");
  const hostName = patch.hostName !== undefined ? normalizeHostName(patch.hostName) : cur.hostName;

  const next: StoredSettings = {
    defaultCodeDir: patch.defaultCodeDir ?? cur.defaultCodeDir,
    theme: patch.theme ?? cur.theme,
    // 和真实实现一致：模板为空时恢复默认
    commitTemplate: (patch.commitTemplate ?? cur.commitTemplate).trim() || DEFAULT_COMMIT_TEMPLATE,
    networkTimeoutSecs: patch.networkTimeoutSecs ?? cur.networkTimeoutSecs,
    proxyMode,
    proxyUrl,
    hostName,
    launchAtLogin: patch.launchAtLogin ?? cur.launchAtLogin,
    closeToTray: patch.closeToTray ?? cur.closeToTray,
    syncOnShutdown: patch.syncOnShutdown ?? cur.syncOnShutdown,
    syncOnLock: patch.syncOnLock ?? cur.syncOnLock,
    syncOnSleep: patch.syncOnSleep ?? cur.syncOnSleep,
    syncOnStartup: patch.syncOnStartup ?? cur.syncOnStartup,
    syncOnResume: patch.syncOnResume ?? cur.syncOnResume,
    syncOnUnlock: patch.syncOnUnlock ?? cur.syncOnUnlock,
    periodicCheck: patch.periodicCheck ?? cur.periodicCheck,
    notifyOnSuccess: patch.notifyOnSuccess ?? cur.notifyOnSuccess,
    notifyOnError: patch.notifyOnError ?? cur.notifyOnError,
    // 只读：暂停只能通过 pauseAutoSync / resumeAutoSync 修改
    autoSyncPausedUntil: cur.autoSyncPausedUntil,
  };
  writeSettings(next);
  return withProxy(next);
}

/** 演示：没有托盘，不会推送设置变化 */
export async function onSettingsChanged(_cb: (s: Settings) => void): Promise<Unlisten> {
  return () => {};
}

// ---------------- 换电脑自动同步（演示：没有后台同步，只能暂停 / 恢复）----------------

/** 演示：暂停到本地时间明天 00:00 */
export async function pauseAutoSync(): Promise<Settings> {
  await delay(100);
  const midnight = new Date();
  midnight.setHours(24, 0, 0, 0);
  const next = { ...readSettings(), autoSyncPausedUntil: midnight.toISOString() };
  writeSettings(next);
  return withProxy(next);
}

export async function resumeAutoSync(): Promise<Settings> {
  await delay(100);
  const next = { ...readSettings(), autoSyncPausedUntil: null };
  writeSettings(next);
  return withProxy(next);
}

export async function getAutoSyncState(): Promise<AutoSyncState> {
  await delay(30);
  return { running: null, startupSync: false };
}

/** 演示：没有后台同步，不会推送 */
export async function onProjectsChanged(_cb: (list: Project[]) => void): Promise<Unlisten> {
  return () => {};
}

export async function onAutoSync(_handlers: {
  started: (e: AutoSyncEvent) => void;
  finished: (e: AutoSyncEvent) => void;
}): Promise<Unlisten> {
  return () => {};
}

// ---------------- 应用更新（演示）----------------

export async function getAppVersion(): Promise<string> {
  await delay(30);
  return "0.1.0";
}

/** 演示：假装有个新版本 */
export async function checkForUpdate(): Promise<UpdateInfo | null> {
  await delay(900);
  return {
    version: "0.2.0",
    currentVersion: "0.1.0",
    date: new Date().toISOString(),
    notes: ["- 新增应用内更新", "- 修复若干问题", "（演示数据）"].join("\n"),
  };
}

/** 演示：模拟下载进度，最后不会真的安装或重启 */
export async function installUpdate(onProgress: (p: UpdateProgress) => void): Promise<void> {
  const total = 24_000_000;
  onProgress({ downloaded: 0, total });
  for (let downloaded = 0; downloaded < total; ) {
    await delay(150);
    downloaded = Math.min(total, downloaded + Math.round(total * (0.06 + Math.random() * 0.08)));
    onProgress({ downloaded, total });
  }
  await delay(400);
  toast.info("演示模式：不会真的更新", { description: "真实环境下这里会安装新版本并自动重启应用" });
}
