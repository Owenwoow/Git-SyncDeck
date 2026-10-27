// 假数据模式下的 api 实现（第一阶段的 Demo 逻辑）。只允许 src/api/index.ts 引用。
// 函数签名必须与 src/api/tauri.ts 完全一致。
import { toast } from "sonner";
import type {
  AutoSyncEvent,
  AutoSyncState,
  CloneDefaults,
  CloneProgressEvent,
  CloneRequest,
  CloneResult,
  CommitInfo,
  DiagnosticInfo,
  AutoSyncTrigger,
  GitInfo,
  GitOp,
  GithubListing,
  GithubRepo,
  GithubRepoEntry,
  Project,
  ProgressPhase,
  ProxyMode,
  ScannedRepo,
  Settings,
  SyncItemResult,
  SyncProgressEvent,
  SyncProgressSnapshot,
  SyncResult,
  Unlisten,
  UpdateInfo,
  UpdateProgress,
} from "@/types";
import { buildAiPrompt, buildDiagnosticText } from "@/lib/diagnostic";
import {
  DEFAULT_COMMIT_TEMPLATE,
  MAX_BIG_FILE_LIMIT_MB,
  NETWORK_TIMEOUT_CHOICES,
  describeIssue,
  isAbnormal,
  sortProjects,
  willAutoCommit,
} from "@/lib/status";
import { formatDateTime } from "@/lib/time";
import { clone, db, delay, jitter } from "./db";
import { EXISTING_FOLDERS, MOCK_OWNER, mockGithubRepos } from "./github";
import { mockConflicts } from "./projects";
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

// ---------------- 从 GitHub 克隆 ----------------

const CLONE_INPUT_KEY = "syncdeck-mock-github-input";

export async function getCloneDefaults(): Promise<CloneDefaults> {
  await delay(200);
  let lastInput: string | null = null;
  try {
    lastInput = localStorage.getItem(CLONE_INPUT_KEY);
  } catch {
    // 读不到就当没有
  }
  return { lastInput, ghLogin: MOCK_OWNER, defaultCodeDir: readSettings().defaultCodeDir };
}

/** 演示：输入里带 public-only 模拟没装 gh（只列公开仓库），带 not-logged 模拟 gh 没登录；写成 账号/仓库 只列那一个仓库 */
export async function listGithubRepos(input: string, parentDir?: string): Promise<GithubListing> {
  await delay(jitter(500, 900));
  const text = input.trim();
  if (!text) throw new Error("请填写 GitHub 账号主页、用户名或仓库地址");
  try {
    localStorage.setItem(CLONE_INPUT_KEY, text);
  } catch {
    // 记不住也不影响
  }
  const lower = text.toLowerCase();
  const publicOnly = lower.includes("public-only");
  const notLoggedIn = lower.includes("not-logged");
  const path = text.replace(/^https?:\/\//i, "").replace(/^(www\.)?github\.com\//i, "").replace(/[?#].*$/, "").replace(/\/+$/, "");
  const [first, second] = path.split("/");
  const isDemoAccount = publicOnly || notLoggedIn || !first || first.toLowerCase() === MOCK_OWNER.toLowerCase();
  if (!isDemoAccount) throw new Error(`找不到 GitHub 账号 ${first}（演示数据里只有 ${MOCK_OWNER}）`);

  const onlyPublic = publicOnly || notLoggedIn;
  let repos = mockGithubRepos.filter((r) => !onlyPublic || !r.private);
  if (second) {
    const name = second.replace(/\.git$/i, "").toLowerCase();
    repos = repos.filter((r) => r.name.toLowerCase() === name);
    if (!repos.length) throw new Error(`找不到 GitHub 仓库 ${MOCK_OWNER}/${second}（地址写错了，或者它是私有的、需要先用 gh 登录）`);
  }

  const entries = await inspectGithubTargets(repos, parentDir ?? readSettings().defaultCodeDir);
  return {
    owner: MOCK_OWNER,
    repos: entries,
    source: onlyPublic ? "public" : "gh",
    notice: publicOnly
      ? "装好 GitHub CLI（gh）并运行 gh auth login 后能看到私有仓库"
      : notLoggedIn
        ? "gh 还没登录：运行 gh auth login 后能看到私有仓库"
        : null,
  };
}

function directoryKey(path: string): string {
  return path.trim().replace(/\//g, "\\").replace(/[\\]+$/, "").toLowerCase();
}

function cloneDirectory(parentDir?: string): string {
  const dir = (parentDir ?? readSettings().defaultCodeDir).trim().replace(/\//g, "\\");
  if (!/^(?:[a-z]:\\|\\\\[^\\]+\\[^\\]+)/i.test(dir)) throw new Error("保存目录必须是绝对路径");
  return dir.replace(/\\+$/, "") + "\\";
}

function validCloneName(name: string): boolean {
  return name.length <= 100 && /^[a-z\d_.-]+$/i.test(name) && name !== "." && name !== "..";
}

/** 演示中的占用文件夹只存在于默认代码目录，切换目录后可重新克隆。 */
function occupiedTarget(parentDir: string, name: string): boolean {
  return (directoryKey(parentDir) === directoryKey(readSettings().defaultCodeDir) && EXISTING_FOLDERS.includes(name))
    || !!db.findRepoByPath(`${parentDir.replace(/[\\/]+$/, "")}\\${name}`);
}

export async function inspectGithubTargets(repos: GithubRepo[], parentDir: string): Promise<GithubRepoEntry[]> {
  const dir = cloneDirectory(parentDir);
  if (repos.some((r) => !validCloneName(r.name))) throw new Error("文件夹名不合法");
  await delay(150);
  const roots = [readSettings().defaultCodeDir, dir].map((p) => directoryKey(p) + "\\");
  return repos.map((repo) => {
    const target = `${dir}${repo.name}`;
    const local = db.allRepos().find((r) => r.remoteUrl?.toLowerCase() === repo.cloneUrl.toLowerCase()
      && (db.isMonitored(r.id) || roots.some((root) => directoryKey(r.path).startsWith(root))));
    return { repo: clone(repo), local: local?.path ?? null, target, targetExists: occupiedTarget(dir, repo.name) };
  });
}

/** 演示：每个仓库 2~4 秒，下载 → 处理差异 → 检出，每 200 毫秒报一次百分比；名字含 huge 的模拟超时失败；最多同时 3 个 */
export async function cloneRepos(
  items: CloneRequest[],
  onProgress: (e: CloneProgressEvent) => void,
  parentDir?: string,
): Promise<CloneResult[]> {
  const dir = cloneDirectory(parentDir);
  const results: CloneResult[] = new Array(items.length);
  const reserved = new Set<string>();

  async function one(index: number) {
    const { url, name } = items[index];
    onProgress({ type: "start", url });
    const target = `${dir}${name}`;
    let error: string | null = null;
    let project: Project | null = null;
    if (!validCloneName(name)) {
      error = `文件夹名不合法：${name}`;
    } else if (occupiedTarget(dir, name) || reserved.has(directoryKey(target))) {
      error = `文件夹已存在：${target}，没有覆盖`;
    } else {
      reserved.add(directoryKey(target));
      const total = jitter(2000, 4000);
      const failing = name.toLowerCase().includes("huge");
      const phases: [ProgressPhase, number][] = [
        ["downloading", 0.6],
        ["resolving", 0.15],
        ["checkout", 0.25],
      ];
      outer: for (const [phase, share] of phases) {
        const steps = Math.max(2, Math.round((total * share) / 200));
        for (let i = 1; i <= steps; i++) {
          await delay(200);
          const percent = Math.round((i / steps) * 100);
          // 超时：下载到一半停住
          if (failing && phase === "downloading" && percent >= 40) {
            error = "git clone 超时（180 秒），已终止";
            break outer;
          }
          onProgress({ type: "progress", url, phase, percent });
        }
      }
      if (!error) {
        project = clone(
          db.addRepo({
            id: `clone-${directoryKey(target)}`,
            name,
            path: target,
            branch: "main",
            remoteUrl: url,
            upstream: "origin/main",
            status: "synced",
            ahead: 0,
            behind: 0,
            changes: [],
            lastSyncAt: null,
            issue: null,
            checkError: null,
            autoCommit: false,
          }),
        );
        db.monitor(project.id);
      }
    }
    results[index] = { url, name, ok: !error, error, project };
    onProgress({ type: "done", url, ok: !error, error, project });
  }

  let next = 0;
  const worker = async () => {
    while (next < items.length) await one(next++);
  };
  await Promise.all(Array.from({ length: Math.min(3, items.length) }, worker));
  return results;
}

// ---------------- 同步 ----------------

/** 预演合并发现冲突时的原因（与 src-tauri/src/ops.rs 的 conflict_reason 文案一致） */
function mergeConflictReason(files: string[]): string {
  return `本地和云端改了同一处，自动合并会冲突：${files.join("、")}。没有改动任何文件，可在详情里「复制给 AI」`;
}

/** 正在进行的一轮同步（一键同步或演示的后台同步）；同一时间只跑一轮，和真实实现一样 */
let syncBusy = false;

export async function syncAll(onProgress: (e: SyncProgressEvent) => void): Promise<SyncResult> {
  if (syncBusy) throw new Error("正在同步中，请等这一轮结束");
  syncBusy = true;
  try {
    // 刷新进行中时先等它结束，再复用刚检查过的结果
    await refreshing?.catch(() => undefined);
    return await runRound(onProgress, null, 1);
  } finally {
    syncBusy = false;
  }
}

/** 演示：模拟 git 的进度，每 ~200 毫秒报一次百分比，用时约 `ms` 毫秒 */
async function simulateGit(
  projectId: string,
  op: GitOp,
  phase: ProgressPhase,
  ms: number,
  onProgress: (e: SyncProgressEvent) => void,
) {
  onProgress({ type: "progress", projectId, op, phase: "preparing", percent: 100 });
  const steps = Math.max(2, Math.round(ms / 200));
  for (let i = 1; i <= steps; i++) {
    await delay(ms / steps);
    onProgress({ type: "progress", projectId, op, phase, percent: Math.round((i / steps) * 100) });
  }
}

/** 一轮同步的演示逻辑：`only` 只同步这些项目（离开时只挑要推的），`slow` 放慢倍数（后台同步慢一点，方便看进度） */
async function runRound(
  onProgress: (e: SyncProgressEvent) => void,
  only: string[] | null,
  slow: number,
): Promise<SyncResult> {
  const startedAt = new Date().toISOString();
  const queue = sortProjects(db.monitoredProjects()).filter((p) => !only || only.includes(p.id));
  const total = queue.length;
  const items: SyncItemResult[] = [];
  const { autoMerge } = readSettings();
  onProgress({ type: "begin", projectIds: queue.map((p) => p.id), total });

  for (const [index, original] of queue.entries()) {
    db.updateRepo(original.id, { status: "syncing" });
    onProgress({ type: "start", projectId: original.id, index, total });

    const abnormal = isAbnormal(original.status);
    // 打开了自动提交的"有未提交改动"项目：演示先在本地提交，再推送（云端也有新提交时按分叉处理）
    const autoCommit = willAutoCommit(original);
    // 分叉且打开了自动合并：演示预演合并，mockConflicts 里有的会冲突，其余自动合并并推送
    const merge = autoMerge && (original.status === "diverged" || (autoCommit && original.behind > 0));
    const conflicts = merge ? (mockConflicts[original.id] ?? []) : [];
    const needsWork = original.status === "ahead" || original.status === "behind" || autoCommit || merge;
    if (original.status === "behind") {
      // 待拉取：要下载的东西在检查云端的 fetch 里下载
      await simulateGit(original.id, "fetch", "downloading", jitter(700, 1200) * slow, onProgress);
      await simulateGit(original.id, "fetch", "resolving", 300 * slow, onProgress);
    } else if (needsWork && !(merge && conflicts.length)) {
      await delay(jitter(150, 300) * slow);
      await simulateGit(original.id, "push", "uploading", jitter(700, 1400) * slow, onProgress);
    } else {
      await delay((needsWork ? jitter(800, 1200) : jitter(350, 600)) * slow);
    }

    /** 同步成功：自动提交 / 自动合并并推送后和云端一致 */
    const done = (outcome: "committed" | "merged", commits: number): [SyncItemResult, Project] => [
      { projectId: original.id, outcome, commits, reason: null },
      db.updateRepo(original.id, {
        status: "synced",
        changes: [],
        ahead: 0,
        behind: 0,
        lastSyncAt: new Date().toISOString(),
        issue: null,
        checkError: null,
      }),
    ];
    /** 记为异常；patch 是处理过程中已经发生的变化（如已经在本地提交了） */
    const fail = (reason: string, patch: Partial<Project> = {}): [SyncItemResult, Project] => [
      { projectId: original.id, outcome: "failed", commits: 0, reason },
      db.updateRepo(original.id, { status: original.status, ...patch, issue: reason }),
    ];

    let result: SyncItemResult;
    let updated: Project;
    if (autoCommit && original.behind > 0) {
      // 先在本地提交：本地多了一个提交，和云端分叉，再按分叉处理
      const committed: Partial<Project> = { status: "diverged", changes: [], ahead: original.ahead + 1 };
      if (!autoMerge) {
        [result, updated] = fail(
          `已自动提交到本地，但云端也有新提交（领先 ${original.ahead + 1}、落后 ${original.behind}），自动合并已关闭，没有推送，需要先合并`,
          committed,
        );
      } else if (conflicts.length) {
        [result, updated] = fail(`已自动提交到本地（还没推送）。${mergeConflictReason(conflicts)}`, committed);
      } else {
        [result, updated] = done("merged", original.ahead + 2);
      }
    } else if (autoCommit) {
      [result, updated] = done("committed", original.ahead + 1);
    } else if (merge) {
      [result, updated] = conflicts.length ? fail(mergeConflictReason(conflicts)) : done("merged", original.ahead + 1);
    } else if (abnormal) {
      [result, updated] = fail(describeIssue(original));
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
const FAKE_REMOTE_SUBJECTS = ["笔记本上改了 README", "调整窗口默认大小", "初始化项目"];

function fakeCommits(subjects: string[], seed: number): CommitInfo[] {
  return subjects.map((subject, i) => ({
    hash: (seed + i * 0x111111).toString(16).slice(0, 7),
    date: formatDateTime(new Date(Date.now() - (i + 1) * 26 * 3_600_000)),
    subject,
  }));
}

async function diagnosticInfo(projectId: string): Promise<DiagnosticInfo> {
  await delay(120);
  const p = db.getRepo(projectId);
  if (!p) throw new Error("项目不存在");
  return {
    project: clone(p),
    recentCommits: fakeCommits(FAKE_SUBJECTS, 0x1a2b3c4),
    // 和真实实现一样：只有分叉时才预演合并、列出冲突文件
    conflictFiles: p.status === "diverged" ? [...(mockConflicts[p.id] ?? [])] : [],
    recentRemoteCommits: p.upstream ? fakeCommits(FAKE_REMOTE_SUBJECTS, 0x5e6f7a8) : [],
  };
}

export async function getDiagnosticText(projectId: string): Promise<string> {
  const info = await diagnosticInfo(projectId);
  return buildDiagnosticText(info.project, info.recentCommits);
}

export async function getAiPromptText(projectId: string): Promise<string> {
  return buildAiPrompt(await diagnosticInfo(projectId));
}

// ---------------- 系统 ----------------

export async function openProjectHomepage(): Promise<void> {
  window.open("https://github.com/Owenwoow/Git-SyncDeck", "_blank", "noopener,noreferrer");
}

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
  const mb = patch.bigFileLimitMb;
  if (mb !== undefined && !(Number.isInteger(mb) && mb >= 1 && mb <= MAX_BIG_FILE_LIMIT_MB)) {
    throw new Error(`大文件上限要在 1 ~ ${MAX_BIG_FILE_LIMIT_MB} MB 之间：${mb} MB`);
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
    syncOnLock: patch.syncOnLock ?? cur.syncOnLock,
    syncOnStartup: patch.syncOnStartup ?? cur.syncOnStartup,
    syncOnResume: patch.syncOnResume ?? cur.syncOnResume,
    syncOnUnlock: patch.syncOnUnlock ?? cur.syncOnUnlock,
    periodicCheck: patch.periodicCheck ?? cur.periodicCheck,
    notifyOnSuccess: patch.notifyOnSuccess ?? cur.notifyOnSuccess,
    notifyOnError: patch.notifyOnError ?? cur.notifyOnError,
    // 只读：暂停只能通过 pauseAutoSync / resumeAutoSync 修改
    autoSyncPausedUntil: cur.autoSyncPausedUntil,
    autoMerge: patch.autoMerge ?? cur.autoMerge,
    precommitCheck: patch.precommitCheck ?? cur.precommitCheck,
    bigFileLimitMb: patch.bigFileLimitMb ?? cur.bigFileLimitMb,
  };
  writeSettings(next);
  return withProxy(next);
}

/** 演示：没有托盘，不会推送设置变化 */
export async function onSettingsChanged(_cb: (s: Settings) => void): Promise<Unlisten> {
  return () => {};
}

// ---------------- 换电脑自动同步（演示：切到别的窗口再切回来 = 解锁后同步）----------------

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
  return { running: clone(autoRunning), startupSync: false, progress: clone(autoProgress), last: clone(autoLast) };
}

/** 演示：不会真的退出 */
export async function quitApp(): Promise<void> {
  toast.info("演示模式不会真的退出");
}

type AutoHandlers = {
  started: (e: AutoSyncEvent) => void;
  progress: (e: SyncProgressEvent) => void;
  finished: (e: AutoSyncEvent) => void;
};

const autoListeners = new Set<AutoHandlers>();
const projectsListeners = new Set<(list: Project[]) => void>();
let autoRunning: AutoSyncEvent | null = null;
let autoProgress: SyncProgressSnapshot | null = null;
let autoLast: AutoSyncEvent | null = null;

const AUTO_LABELS: Record<AutoSyncTrigger, string> = {
  lock: "锁屏时同步",
  startup: "启动时同步",
  resume: "唤醒后同步",
  unlock: "解锁后同步",
  manual: "立即同步",
  periodic: "定时检查云端",
};

/** 和 src-tauri/src/model.rs 的 SyncProgressSnapshot::apply 一样（演示里没有重试） */
function applySnapshot(s: SyncProgressSnapshot, e: SyncProgressEvent): SyncProgressSnapshot {
  switch (e.type) {
    case "begin":
      return { projectIds: e.projectIds, active: [], results: [], retry: null };
    case "start":
      return { ...s, active: [...s.active.filter((a) => a.projectId !== e.projectId), { projectId: e.projectId, op: null, phase: null, percent: null }] };
    case "progress":
      return {
        ...s,
        active: [
          ...s.active.filter((a) => a.projectId !== e.projectId),
          { projectId: e.projectId, op: e.op, phase: e.phase, percent: e.percent },
        ],
      };
    case "done":
      return {
        ...s,
        active: s.active.filter((a) => a.projectId !== e.result.projectId),
        results: [...s.results.filter((r) => r.projectId !== e.result.projectId), e.result],
      };
    case "retry":
      return { ...s, retry: { projectIds: e.projectIds, retryAt: e.retryAt } };
  }
}

/** 演示一次后台同步：锁屏只推有东西要推的项目，其余（解锁、唤醒……）同步所有项目；比一键同步慢一些，方便看进度 */
async function simulateAutoSync(trigger: AutoSyncTrigger = "unlock") {
  if (syncBusy || autoRunning) return;
  syncBusy = true;
  const label = AUTO_LABELS[trigger];
  autoRunning = { trigger, label, result: null };
  autoProgress = { projectIds: [], active: [], results: [], retry: null };
  autoListeners.forEach((h) => h.started(clone(autoRunning!)));
  try {
    await refreshing?.catch(() => undefined);
    const only =
      trigger === "lock"
        ? db
            .monitoredProjects()
            .filter((p) => p.status === "ahead" || willAutoCommit(p))
            .map((p) => p.id)
        : null;
    const result = await runRound(
      (e) => {
        autoProgress = applySnapshot(autoProgress!, e);
        autoListeners.forEach((h) => h.progress(clone(e)));
      },
      only,
      2.5,
    );
    autoLast = { trigger, label, result };
  } finally {
    syncBusy = false;
    autoRunning = null;
    autoProgress = null;
  }
  const list = clone(db.monitoredProjects());
  projectsListeners.forEach((cb) => cb(list));
  autoListeners.forEach((h) => h.finished(clone(autoLast!)));
}

// 演示的触发方式：窗口被切走 2 秒以上再切回来，当作"解锁后同步"；调界面时也可以在控制台里调用
// window.__syncdeckDemo.autoSync("lock" | "unlock" | …)
if (typeof window !== "undefined" && typeof document !== "undefined") {
  let hiddenAt = 0;
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) hiddenAt = Date.now();
    else if (hiddenAt && Date.now() - hiddenAt > 2000) void simulateAutoSync("unlock");
  });
  (window as unknown as { __syncdeckDemo: unknown }).__syncdeckDemo = { autoSync: simulateAutoSync };
}

export async function onProjectsChanged(cb: (list: Project[]) => void): Promise<Unlisten> {
  projectsListeners.add(cb);
  return () => projectsListeners.delete(cb);
}

export async function onAutoSync(handlers: AutoHandlers): Promise<Unlisten> {
  autoListeners.add(handlers);
  return () => autoListeners.delete(handlers);
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
