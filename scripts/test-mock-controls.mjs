// 执行真实 mock 源码，仅替换浏览器环境、提示和计时器；不访问网络、真实配置或 Git 仓库。
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";

const SETTINGS_KEY = "syncdeck.settings";
let harnessId = 0;

async function loadMock(seed) {
  const key = `syncdeckMockTest${++harnessId}`;
  const timers = [];
  const records = new Map(seed === undefined ? [] : [[SETTINGS_KEY, typeof seed === "string" ? seed : JSON.stringify(seed)]]);
  const notices = [];
  let now = new Date(2026, 10, 7, 15, 20, 0).getTime();
  let failRead = false;
  let failWrite = false;
  let writes = 0;
  const storage = {
    getItem(name) { if (failRead) throw new Error("模拟读取失败"); return records.get(name) ?? null; },
    setItem(name, value) { if (failWrite) throw new Error("模拟写入失败"); writes++; records.set(name, value); },
  };
  class ControlledDate extends Date {
    constructor(...args) { super(...(args.length ? args : [now])); }
    static now() { return now; }
  }
  const environment = {
    window: { matchMedia: () => ({ matches: false }), open: () => { throw new Error("测试不允许打开外部页面"); } },
    document: { hidden: false, addEventListener: () => {} },
    localStorage: storage,
    Date: ControlledDate,
    setTimeout(callback, ms) { timers.push({ callback, ms }); return timers.length; },
    toast: new Proxy({}, { get: () => (...args) => notices.push(args) }),
  };
  globalThis[key] = environment;
  const prelude = `const { window, document, localStorage, Date, setTimeout } = globalThis[${JSON.stringify(key)}];\n`;
  function moduleUrl(source, imports = {}) {
    let js = ts.transpileModule(prelude + source, {
      compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 },
    }).outputText;
    for (const [name, url] of Object.entries(imports)) {
      js = js.replaceAll(`from ${JSON.stringify(name)}`, `from ${JSON.stringify(url)}`);
    }
    return `data:text/javascript;base64,${Buffer.from(js).toString("base64")}`;
  }
  async function source(path) { return readFile(new URL(`../src/${path}`, import.meta.url), "utf8"); }
  const timeUrl = moduleUrl(await source("lib/time.ts"));
  const statusUrl = moduleUrl(await source("lib/status.ts"), {
    "lucide-react": import.meta.resolve("lucide-react"), "@/lib/time": timeUrl,
  });
  const diagnosticUrl = moduleUrl(await source("lib/diagnostic.ts"), { "./status": statusUrl, "./time": timeUrl });
  const projectsUrl = moduleUrl(await source("mock/projects.ts"));
  const reposUrl = moduleUrl(await source("mock/repos.ts"), { "@/lib/status": statusUrl, "./projects": projectsUrl });
  const githubUrl = moduleUrl(await source("mock/github.ts"), { "./projects": projectsUrl });
  const dbUrl = moduleUrl(await source("mock/db.ts"), { "./projects": projectsUrl, "./repos": reposUrl });
  const toastUrl = `data:text/javascript;base64,${Buffer.from(`export const toast = globalThis[${JSON.stringify(key)}].toast;`).toString("base64")}`;
  const api = await import(moduleUrl(await source("mock/api.ts"), {
    sonner: toastUrl, "@/lib/diagnostic": diagnosticUrl, "@/lib/status": statusUrl,
    "@/lib/time": timeUrl, "./db": dbUrl, "./github": githubUrl, "./projects": projectsUrl, "./repos": reposUrl,
  }));
  const { db } = await import(dbUrl);
  async function flush() { for (let i = 0; i < 8; i++) await Promise.resolve(); }
  async function run(promise) {
    let settled = false;
    let value;
    let error;
    Promise.resolve(promise).then((result) => { settled = true; value = result; }, (cause) => { settled = true; error = cause; });
    for (let step = 0; !settled && step < 2000; step++) {
      await flush();
      if (!settled) timers.splice(0).forEach((timer) => timer.callback());
    }
    if (!settled) throw new Error("mock 操作未结束或出现未覆盖的异步依赖");
    if (error) throw error;
    return value;
  }
  return {
    api, db, run, flush, notices, timers, window: environment.window,
    stored: () => records.get(SETTINGS_KEY), writes: () => writes,
    failRead: (value) => { failRead = value; }, failWrite: (value) => { failWrite = value; },
    advance: (ms) => { now += ms; }, now: () => now,
    releaseDuration: async (ms) => {
      await flush();
      const selected = timers.filter((timer) => timer.ms === ms);
      for (const timer of selected) timers.splice(timers.indexOf(timer), 1);
      selected.forEach((timer) => timer.callback());
      await flush();
    },
    close: () => { delete globalThis[key]; },
  };
}

test("全新演示配置默认不自动同步；读取不会写设置或改变项目", async () => {
  const h = await loadMock();
  try {
    const before = structuredClone(h.db.monitoredProjects());
    const settings = await h.run(h.api.getSettings());
    assert.equal(settings.autoSyncEnabled, false);
    assert.equal(settings.periodicCheck, false);
    assert.equal(settings.launchAtLogin, true);
    assert.equal(settings.closeToTray, true);
    assert.equal(h.writes(), 0);
    await h.window.__syncdeckDemo.autoSync("startup");
    await h.window.__syncdeckDemo.autoSync("unlock");
    await h.window.__syncdeckDemo.autoSync("periodic");
    assert.deepEqual(h.db.monitoredProjects(), before);
    assert.deepEqual(await h.api.getTaskState(), { active: null, waiting: [], installingUpdate: false });
  } finally { h.close(); }
});

test("旧配置缺总开关时保留原有效触发及主题、代理、自启和暂停", async () => {
  const until = new Date(2026, 10, 9, 0, 0, 0).toISOString();
  const old = { theme: "dark", hostName: "台式机", launchAtLogin: false, closeToTray: true,
    proxyMode: "manual", proxyUrl: "http://127.0.0.1:7890", syncOnLock: false, syncOnStartup: true,
    syncOnResume: false, syncOnUnlock: true, periodicCheck: true, autoSyncPausedUntil: until };
  const h = await loadMock(old);
  try {
    const settings = await h.run(h.api.getSettings());
    assert.equal(settings.autoSyncEnabled, true);
    for (const [key, value] of Object.entries(old)) assert.deepEqual(settings[key], value, key);
    const projects = structuredClone(h.db.monitoredProjects());
    await h.run(h.api.saveSettings({ theme: "light" }));
    assert.deepEqual(h.db.monitoredProjects(), projects);
    const reloaded = await h.run(h.api.getSettings());
    assert.equal(reloaded.autoSyncEnabled, true);
    assert.equal(reloaded.autoSyncPausedUntil, until);
    assert.equal(reloaded.proxyUrl, old.proxyUrl);
    assert.equal(reloaded.launchAtLogin, false);
  } finally { h.close(); }
});

test("暂停1小时和本地午夜持久化，总开关反复切换不清暂停或触发选择", async () => {
  const h = await loadMock();
  try {
    await h.run(h.api.saveSettings({ syncOnLock: false, syncOnResume: false }));
    const paused = await h.run(h.api.pauseAutoSync("hour"));
    assert.equal(Date.parse(paused.autoSyncPausedUntil), h.now() + 3_600_000);
    await h.run(h.api.saveSettings({ autoSyncEnabled: true }));
    await h.run(h.api.saveSettings({ autoSyncEnabled: false }));
    let read = await h.run(h.api.getSettings());
    assert.equal(read.autoSyncPausedUntil, paused.autoSyncPausedUntil);
    assert.equal(read.syncOnLock, false);
    assert.equal(read.syncOnResume, false);
    assert.equal(JSON.parse(h.stored()).autoSyncPausedUntil, paused.autoSyncPausedUntil);
    const tomorrow = await h.run(h.api.pauseAutoSync("tomorrow"));
    const target = new Date(h.now()); target.setHours(24, 0, 0, 0);
    assert.equal(Date.parse(tomorrow.autoSyncPausedUntil), target.getTime());
    h.advance(target.getTime() - h.now() + 1000);
    read = await h.run(h.api.getSettings());
    assert.equal(read.autoSyncPausedUntil, null);
    assert.equal(read.autoSyncEnabled, false);
    assert.equal((await h.run(h.api.getAutoSyncState())).last, null);
    await h.run(h.api.resumeAutoSync());
    assert.equal((await h.run(h.api.getSettings())).autoSyncEnabled, false);
  } finally { h.close(); }
});

test("旧配置中的null自动规则回落默认值，总开关保留旧版有效行为", async () => {
  const h = await loadMock({ autoSyncEnabled: null, syncOnLock: null, syncOnStartup: null, periodicCheck: null, launchAtLogin: null });
  try {
    const settings = await h.run(h.api.getSettings());
    assert.equal(settings.configReadError, null);
    assert.equal(settings.autoSyncEnabled, true);
    assert.equal(settings.syncOnLock, true);
    assert.equal(settings.syncOnStartup, true);
    assert.equal(settings.periodicCheck, false);
    assert.equal(settings.launchAtLogin, true);
  } finally { h.close(); }
});

test("总开关开启但所有触发规则关闭时不会自动操作仓库", async () => {
  const h = await loadMock();
  try {
    await h.run(h.api.saveSettings({ autoSyncEnabled: true, syncOnLock: false, syncOnStartup: false, syncOnResume: false, syncOnUnlock: false }));
    const before = structuredClone(h.db.monitoredProjects());
    for (const trigger of ["lock", "startup", "resume", "unlock"]) await h.window.__syncdeckDemo.autoSync(trigger);
    assert.deepEqual(h.db.monitoredProjects(), before);
    assert.deepEqual(await h.api.getTaskState(), { active: null, waiting: [], installingUpdate: false });
  } finally { h.close(); }
});

test("存储写入失败和代理校验失败不改变原设置，失败后可重试", async () => {
  const h = await loadMock();
  try {
    await h.run(h.api.saveSettings({ theme: "dark", hostName: "旧名称" }));
    const before = h.stored();
    h.failWrite(true);
    await h.run(assert.rejects(h.api.saveSettings({ theme: "light", hostName: "新名称" }), /无法保存设置/));
    await h.run(assert.rejects(h.api.pauseAutoSync("hour"), /无法保存设置/));
    assert.equal(h.stored(), before);
    assert.equal((await h.run(h.api.getSettings())).hostName, "旧名称");
    h.failWrite(false);
    await h.run(assert.rejects(h.api.saveSettings({ proxyMode: "manual", proxyUrl: "bad-proxy" }), /代理地址格式不对/));
    assert.equal(h.stored(), before);
    await h.run(h.api.saveSettings({ hostName: "新名称" }));
    assert.equal((await h.run(h.api.getSettings())).hostName, "新名称");
  } finally { h.close(); }
});

test("配置读取失败保留原内容并阻止保存，错误排除后可重新读取", async () => {
  const h = await loadMock({ theme: "dark", autoSyncEnabled: false });
  try {
    const before = h.stored();
    h.failRead(true);
    assert.match((await h.run(h.api.getSettings())).configReadError, /无法读取设置/);
    await h.run(assert.rejects(h.api.saveSettings({ theme: "light" }), /无法读取设置/));
    assert.equal(h.stored(), before);
    h.failRead(false);
    const settings = await h.run(h.api.getSettings());
    assert.equal(settings.configReadError, null);
    assert.equal(settings.theme, "dark");
  } finally { h.close(); }
});

test("损坏或错误形状配置不能被当成正常配置覆盖", async () => {
  for (const raw of ["{broken", "null", "42", "[]", '"text"', '{"autoSyncEnabled":"false"}', '{"syncOnLock":1}']) {
    const h = await loadMock(raw);
    try {
      assert.ok((await h.run(h.api.getSettings())).configReadError, raw);
      await h.run(assert.rejects(h.api.saveSettings({ theme: "dark" }), /无法读取设置/));
      assert.equal(h.stored(), raw);
    } finally { h.close(); }
  }
});

test("定时检查独立于总开关且仅刷新检查信息，暂停优先", async () => {
  const h = await loadMock();
  try {
    await h.run(h.api.saveSettings({ autoSyncEnabled: false, periodicCheck: true }));
    const before = structuredClone(h.db.monitoredProjects());
    await h.run(h.window.__syncdeckDemo.autoSync("periodic"));
    const after = h.db.monitoredProjects();
    for (let i = 0; i < before.length; i++) {
      const { lastCheckedAt: previousCheck, ...previous } = before[i];
      const { lastCheckedAt, ...current } = after[i];
      assert.deepEqual(current, previous, previous.name);
      assert.equal(lastCheckedAt, previous.remoteUrl && previous.upstream && !previous.checkError
        ? new Date(h.now()).toISOString() : previousCheck);
    }
    await h.run(h.api.pauseAutoSync("hour"));
    const paused = structuredClone(after);
    h.advance(60_000);
    await h.window.__syncdeckDemo.autoSync("periodic");
    assert.deepEqual(h.db.monitoredProjects(), paused);
    assert.equal((await h.run(h.api.getAutoSyncState())).last, null);
  } finally { h.close(); }
});

test("手动刷新、提交、一键同步和克隆不被总开关或临时暂停封死", async () => {
  const h = await loadMock();
  try {
    await h.run(h.api.pauseAutoSync("hour"));
    await h.run(h.api.refreshStatus());
    const dirty = h.db.monitoredProjects().find((project) => project.status === "dirty" && !project.behind && project.upstream);
    const committed = await h.run(h.api.commitAndPush(dirty.id, "手动测试提交"));
    assert.equal(committed.status, "synced");
    const result = await h.run(h.api.syncAll(() => {}));
    assert.ok(result.items.length > 0);
    const [cloned] = await h.run(h.api.cloneRepos([{ name: "mock-regression-clone", url: "https://github.com/example/mock-regression-clone.git" }], () => {}, "E:\\mock-only"));
    assert.equal(cloned.ok, true);
    assert.equal(h.db.isMonitored(cloned.project.id), true);
    const settings = await h.run(h.api.getSettings());
    assert.equal(settings.autoSyncEnabled, false);
    assert.ok(settings.autoSyncPausedUntil);
  } finally { h.close(); }
});

test("活动和等待任务统一保护退出；安装取得资格后拒绝所有新仓库任务", async () => {
  const h = await loadMock();
  try {
    await h.run(h.api.downloadUpdate(() => {}));
    const refresh = h.api.refreshStatus();
    const sync = h.api.syncAll(() => {});
    await h.flush();
    const state = await h.api.getTaskState();
    assert.equal(state.active.kind, "refresh");
    assert.equal(state.waiting.length, 1);
    assert.equal(state.waiting[0].kind, "sync");
    assert.deepEqual(await h.api.quitApp(), state);
    assert.equal(await h.api.quitApp(true), null);
    await assert.rejects(h.api.installUpdate(), /仓库任务仍在进行/);
    await h.run(Promise.all([refresh, sync]));
    const installation = h.api.installUpdate();
    assert.equal((await h.api.getTaskState()).installingUpdate, true);
    await assert.rejects(h.api.refreshStatus(), /正在安装更新/);
    await assert.rejects(h.api.syncAll(() => {}), /正在安装更新/);
    await assert.rejects(h.api.commitAndPush("p-blog", "测试"), /正在安装更新/);
    await assert.rejects(h.api.cloneRepos([], () => {}), /正在安装更新/);
    await assert.rejects(h.api.quitApp(true), /正在安装更新/);
    await h.run(installation);
    assert.deepEqual(await h.api.getTaskState(), { active: null, waiting: [], installingUpdate: false });
  } finally { h.close(); }
});

test("等待锁的自动任务取得执行资格时重新检查暂停或总开关", async () => {
  for (const action of ["pause", "disable"]) {
    const h = await loadMock();
    try {
      await h.run(h.api.saveSettings({ autoSyncEnabled: true, syncOnUnlock: true }));
      let started = 0;
      await h.api.onAutoSync({ started: () => started++, progress: () => {}, finished: () => {} });
      const refresh = h.api.refreshStatus();
      await h.flush();
      const waiting = h.window.__syncdeckDemo.autoSync("unlock");
      assert.equal((await h.api.getTaskState()).waiting.length, 1);
      const changed = action === "pause" ? h.api.pauseAutoSync("hour") : h.api.saveSettings({ autoSyncEnabled: false });
      await h.releaseDuration(100);
      await changed;
      await h.run(Promise.all([refresh, waiting]));
      assert.equal(started, 0, action);
      assert.equal((await h.api.getTaskState()).active, null);
    } finally { h.close(); }
  }
});

test("冲突保护拒绝手动提交且项目完全不变", async () => {
  const h = await loadMock();
  try {
    h.db.updateRepo("p-blog", { commitBlockReason: "存在未完成的 merge，请到外部工具处理" });
    const before = structuredClone(h.db.getRepo("p-blog"));
    await h.run(assert.rejects(h.api.commitAndPush("p-blog", "不可提交冲突"), /未完成的 merge/));
    assert.deepEqual(h.db.getRepo("p-blog"), before);
  } finally { h.close(); }
});

test("已开始的自动同步在暂停后正常收尾，恢复只恢复原有规则", async () => {
  const h = await loadMock();
  try {
    await h.run(h.api.saveSettings({ autoSyncEnabled: true, syncOnUnlock: true }));
    let started = 0;
    let finished = 0;
    await h.api.onAutoSync({ started: () => started++, progress: () => {}, finished: () => finished++ });
    const running = h.window.__syncdeckDemo.autoSync("unlock");
    await h.flush();
    assert.equal(started, 1);
    assert.equal((await h.api.getTaskState()).active.automatic, true);
    const pause = h.api.pauseAutoSync("hour");
    await h.releaseDuration(100);
    await pause;
    assert.equal(finished, 0);
    await h.run(running);
    assert.equal(finished, 1);
    const state = await h.run(h.api.getAutoSyncState());
    assert.equal(state.running, null);
    assert.ok(state.last.result.items.length > 0);
    await h.run(h.api.resumeAutoSync());
    assert.equal(started, 1);
    assert.equal(finished, 1);
    assert.deepEqual((await h.run(h.api.getAutoSyncState())).last, state.last);
    const settings = await h.run(h.api.getSettings());
    assert.equal(settings.autoSyncEnabled, true);
    assert.equal(settings.autoSyncPausedUntil, null);
  } finally { h.close(); }
});

test("手动提交遇到云端领先时保留本地提交，不自动合并或推送", async () => {
  const h = await loadMock();
  try {
    const before = structuredClone(h.db.getRepo("p-blog"));
    h.db.updateRepo(before.id, { behind: 2 });
    await h.run(assert.rejects(h.api.commitAndPush(before.id, "先留在本地"), /已提交到本地/));
    const after = h.db.getRepo(before.id);
    assert.equal(after.status, "diverged");
    assert.equal(after.ahead, before.ahead + 1);
    assert.equal(after.behind, 2);
    assert.deepEqual(after.changes, []);
    assert.equal(after.lastSyncAt, before.lastSyncAt);
  } finally { h.close(); }
});
