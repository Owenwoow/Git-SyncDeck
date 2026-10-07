#!/usr/bin/env node
// 在临时目录里创建一组测试仓库，用本地裸仓库充当 GitHub，构造出各种同步状态。
// 不会碰你的真实项目。
//
// 用法：node scripts/make-test-repos.mjs [目标目录]
//       默认目标目录：%TEMP%\syncdeck-test
//
// 生成的目录：
//   repos\           ← 本机的测试仓库（在应用里"添加项目"时选这个目录）
//   remotes\*.git    ← 充当云端的裸仓库
//   other-computer\  ← 模拟另一台电脑，用来往云端推新提交
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const root = path.resolve(process.argv[2] ?? path.join(os.tmpdir(), "syncdeck-test"));
const MARKER = ".syncdeck-test";

// 只会清理本脚本自己创建过的目录（有标记文件），避免误删
if (fs.existsSync(root)) {
  if (!fs.existsSync(path.join(root, MARKER))) {
    console.error(`${root} 已存在，但不是本脚本创建的测试目录，为安全起见不做任何改动。`);
    process.exit(1);
  }
  fs.rmSync(root, { recursive: true, force: true });
}
const remotesDir = path.join(root, "remotes");
const reposDir = path.join(root, "repos");
const otherDir = path.join(root, "other-computer");
for (const d of [remotesDir, reposDir, otherDir]) fs.mkdirSync(d, { recursive: true });
fs.writeFileSync(path.join(root, MARKER), "由 scripts/make-test-repos.mjs 创建，可以随时删除\n");

const env = { ...process.env, GIT_TERMINAL_PROMPT: "0" };

function git(cwd, ...args) {
  return execFileSync("git", ["-c", "init.defaultBranch=main", ...args], {
    cwd,
    env,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  }).trim();
}

/** 测试仓库自己的身份配置，不依赖、也不修改你的全局 git 配置 */
function setIdentity(dir) {
  git(dir, "config", "user.name", "SyncDeck Test");
  git(dir, "config", "user.email", "test@syncdeck.invalid");
  git(dir, "config", "commit.gpgsign", "false");
}

function write(dir, file, content) {
  const full = path.join(dir, file);
  fs.mkdirSync(path.dirname(full), { recursive: true });
  fs.writeFileSync(full, content);
}

function commit(dir, file, content, message) {
  write(dir, file, content);
  git(dir, "add", "--", file);
  git(dir, "commit", "--quiet", "-m", message);
}

/** 云端（裸仓库）+ 另一台电脑 + 本机，本机与云端初始一致 */
function setup(name) {
  const remote = path.join(remotesDir, `${name}.git`);
  fs.mkdirSync(remote);
  git(remote, "init", "--quiet", "--bare");

  const other = path.join(otherDir, name);
  git(otherDir, "clone", "--quiet", remote, name);
  setIdentity(other);
  commit(other, "README.md", `# ${name}\n`, "初始提交");
  git(other, "push", "--quiet", "-u", "origin", "main");

  const local = path.join(reposDir, name);
  git(reposDir, "clone", "--quiet", remote, name);
  setIdentity(local);
  return { remote, other, local };
}

/** 模拟另一台电脑往云端推了新提交 */
function pushFromOther(other, file, message) {
  commit(other, file, `${message}\n`, message);
  git(other, "push", "--quiet", "origin", "main");
}

const expected = {};

// 1. 已同步
setup("synced");
expected["synced"] = "synced";

// 2. 待推送：本地 2 个提交没推
{
  const { local } = setup("ahead");
  commit(local, "notes/day1.md", "第一天\n", "笔记：第一天");
  commit(local, "notes/day2.md", "第二天\n", "笔记：第二天");
  expected["ahead"] = "ahead";
}

// 3. 待拉取：另一台电脑推了 1 个提交
{
  const { other } = setup("behind");
  pushFromOther(other, "from-laptop.md", "笔记本上的改动");
  expected["behind"] = "behind";
}

// 4. 有未提交改动：未暂存修改 + 已暂存新文件 + 中文文件名的未跟踪文件
{
  const { local } = setup("dirty");
  write(local, "README.md", "# dirty\n\n改了一行\n");
  write(local, "src/新功能.ts", "export const x = 1;\n");
  git(local, "add", "--", "src/新功能.ts");
  write(local, "中文 笔记.md", "未跟踪的中文文件名\n");
  expected["dirty"] = "dirty";
}

// 5. 分叉：本地和云端各有 1 个新提交
{
  const { local, other } = setup("diverged");
  commit(local, "desktop.md", "台式机\n", "台式机上的改动");
  pushFromOther(other, "laptop.md", "笔记本上的改动");
  expected["diverged"] = "diverged";
}

// 6a. 未关联云端：没有远程仓库
{
  const local = path.join(reposDir, "no-remote");
  fs.mkdirSync(local);
  git(local, "init", "--quiet");
  setIdentity(local);
  commit(local, "a.txt", "a\n", "本地仓库，从没关联过 GitHub");
  expected["no-remote"] = "no-remote";
}

// 6b. 未关联云端：有远程，但当前分支没有上游
{
  const { local } = setup("no-upstream");
  git(local, "switch", "--quiet", "-c", "feature/weather-card");
  commit(local, "weather.md", "天气卡片\n", "新分支上的提交");
  expected["no-upstream"] = "no-remote";
}

// 6c. 未关联云端：detached HEAD
{
  const { local } = setup("detached");
  git(local, "switch", "--quiet", "--detach", "HEAD");
  expected["detached"] = "no-remote";
}

// 7. 检查失败：远程地址无效，fetch 会失败
{
  const { local } = setup("fetch-fail");
  git(local, "remote", "set-url", "origin", path.join(remotesDir, "does-not-exist.git"));
  expected["fetch-fail"] = "check-failed";
}

// 8. 有改动且落后：用来验证"提交后发现落后于云端要停止，不自动合并"
{
  const { local, other } = setup("dirty-behind");
  pushFromOther(other, "laptop.md", "笔记本上的改动");
  write(local, "README.md", "# dirty-behind\n\n台式机上没提交的改动\n");
  expected["dirty-behind"] = "dirty";
}

fs.writeFileSync(path.join(root, "expected.json"), JSON.stringify(expected, null, 2));

console.log(`测试仓库已创建：${reposDir}`);
for (const [name, status] of Object.entries(expected)) {
  console.log(`  ${name.padEnd(14)} → 预期 ${status}`);
}
