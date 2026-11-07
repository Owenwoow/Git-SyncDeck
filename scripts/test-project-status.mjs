// 用实际前端源码验证状态判定，不写编译产物，也不访问真实仓库或配置。
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import ts from "typescript";

function moduleUrl(source, imports = {}) {
  let js = ts.transpileModule(source, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ES2022 },
  }).outputText;
  for (const [name, url] of Object.entries(imports)) {
    js = js.replaceAll(`from ${JSON.stringify(name)}`, `from ${JSON.stringify(url)}`);
  }
  return `data:text/javascript;base64,${Buffer.from(js).toString("base64")}`;
}

const timeUrl = moduleUrl(await readFile(new URL("../src/lib/time.ts", import.meta.url), "utf8"));
const statusUrl = moduleUrl(await readFile(new URL("../src/lib/status.ts", import.meta.url), "utf8"), {
  "lucide-react": import.meta.resolve("lucide-react"),
  "@/lib/time": timeUrl,
});
const status = await import(statusUrl);
const diagnostic = await import(moduleUrl(await readFile(new URL("../src/lib/diagnostic.ts", import.meta.url), "utf8"), {
  "./status": statusUrl,
  "./time": timeUrl,
}));

const now = Date.parse("2026-11-07T10:00:00+08:00");
function project(patch = {}) {
  return {
    id: "fixture", name: "测试项目", path: "C:/temporary/project", branch: "main",
    remoteUrl: "https://github.com/example/project.git", upstream: "origin/main",
    status: "synced", ahead: 0, behind: 0, changes: [], lastSyncAt: null,
    lastCheckedAt: new Date(now - 60_000).toISOString(), issue: null,
    checkError: null, commitBlockReason: null, autoCommit: false, ...patch,
  };
}

test("检查失败、遗留异常、没有联网记录和过期结果不能计为已确认同步", () => {
  const list = [
    project({ id: "fresh" }),
    project({ id: "failed", checkError: "无法连接云端" }),
    project({ id: "issue", issue: "上次推送失败" }),
    project({ id: "local", lastCheckedAt: null }),
    project({ id: "stale", lastCheckedAt: new Date(now - 30 * 60_000).toISOString() }),
  ];
  assert.deepEqual(status.summarizeProjects(list, true, now), {
    total: 5, confirmedSynced: 1, needsAction: 1, unconfirmed: 3, canAttempt: 0, manual: 1,
  });
  assert.equal(status.hasRecentCloudCheck(project({ lastCheckedAt: "invalid" }), now), false);
  assert.match(status.cloudCheckDescription(list[3], now), /仅本地状态/);
  assert.match(status.cloudCheckDescription(list[4], now), /超过 30 分钟/);
});

test("自动合并只影响可尝试处理的分类，不把分叉算作已同步", () => {
  const diverged = project({ status: "diverged", ahead: 1, behind: 1 });
  const enabled = status.summarizeProjects([diverged], true, now);
  const disabled = status.summarizeProjects([diverged], false, now);
  assert.equal(enabled.confirmedSynced, 0);
  assert.equal(enabled.needsAction, 1);
  assert.equal(enabled.canAttempt, 1);
  assert.equal(disabled.canAttempt, 0);
  assert.equal(disabled.manual, 1);
  assert.match(status.describeStatus(diverged, true), /可尝试自动处理/);
  assert.match(status.describeStatus(diverged, true), /不能保证合并成功/);
});

test("同一文件同时在暂存区和工作区只计一个文件，仍保留两组信息", () => {
  const dirty = project({ status: "dirty", changes: [
    { path: "same.txt", kind: "M", staged: true },
    { path: "same.txt", kind: "M", staged: false },
    { path: "new.txt", kind: "?", staged: false },
  ] });
  assert.equal(status.uniqueChangedFileCount(dirty.changes), 2);
  assert.match(status.describeIssue(dirty), /有 2 个未提交的文件/);
  const text = diagnostic.buildDiagnosticText(dirty, [], new Date(now));
  assert.match(text, /【已暂存的文件】1 个/);
  assert.match(text, /【未暂存的文件】1 个/);
  assert.match(text, /【未跟踪的文件】1 个/);
});

test("冲突或未完成 Git 操作始终阻止普通手动提交和自动提交的界面预期", () => {
  const reason = "存在未完成的 cherry-pick，请到外部工具处理";
  const blocked = project({ status: "dirty", autoCommit: true, commitBlockReason: reason });
  assert.equal(status.commitPushDisabledReason(blocked), reason);
  assert.equal(status.willAutoCommit(blocked), false);
  assert.equal(status.summarizeProjects([blocked], true, now).canAttempt, 0);
  assert.equal(status.commitPushDisabledReason(project({ status: "dirty" })), null);
  assert.equal(status.willAutoCommit(project({ status: "dirty", autoCommit: true })), true);
});

test("诊断文本保留完整异常和检查边界，不把本地状态承诺为所有分支已备份", () => {
  const issue = "推送失败\n远程拒绝第二行详情";
  const p = project({ issue, checkError: "fetch 超时", lastCheckedAt: null, commitBlockReason: "未完成 merge" });
  const text = diagnostic.buildDiagnosticText(p, [], new Date(now));
  assert.ok(text.includes(issue));
  assert.match(text, /【检查失败】fetch 超时/);
  assert.match(text, /【提交受阻】未完成 merge/);
  assert.match(text, /尚无成功检查记录，仅本地状态/);
  assert.match(text, /只检查当前分支，不代表所有分支均已备份/);
  assert.match(status.describeStatus(p), /云端当前状态仍待确认/);
  const prompt = diagnostic.buildAiPrompt({ project: p, conflictFiles: [], recentCommits: [], recentRemoteCommits: [] });
  assert.match(prompt, /确认并处理当前未完成的 Git 操作/);
});
