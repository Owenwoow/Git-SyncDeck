// 在内存中加载真实 API，只替换 Tauri 边界；不下载或安装生产更新。
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";
import vm from "node:vm";
import ts from "typescript";

const source = await readFile(new URL("../src/api/tauri.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
}).outputText;

function harness() {
  const calls = [];
  const control = { busy: false, installFails: false };
  const update = {
    version: "1.0.0", currentVersion: "0.8.0", date: null, body: "测试更新",
    close: async () => { calls.push("close"); },
    download: async (progress) => {
      calls.push("download");
      progress({ event: "Started", data: { contentLength: 128 } });
      progress({ event: "Progress", data: { chunkLength: 128 } });
      progress({ event: "Finished" });
    },
    install: async () => {
      calls.push("install");
      if (control.installFails) throw new Error("安装程序启动失败");
    },
  };
  const stubs = {
    "@tauri-apps/api/core": {
      Channel: class {},
      invoke: async (command) => {
        calls.push(command);
        if (command === "begin_update_install" && control.busy) throw "刷新任务尚未结束";
      },
    },
    "@tauri-apps/api/app": { getVersion: async () => "0.8.0" },
    "@tauri-apps/api/event": { listen: async () => () => {} },
    "@tauri-apps/plugin-dialog": { open: async () => null },
    "@tauri-apps/plugin-process": { relaunch: async () => { calls.push("relaunch"); } },
    "@tauri-apps/plugin-updater": { check: async () => update },
    "@/lib/diagnostic": {},
  };
  const exports = {};
  vm.runInNewContext(compiled, {
    exports,
    require: (name) => {
      assert.ok(name in stubs, `未预期的 API 依赖：${name}`);
      return stubs[name];
    },
  });
  return { api: exports, calls, control };
}

async function downloaded(fixture) {
  await fixture.api.checkForUpdate();
  await fixture.api.downloadUpdate(() => {});
  fixture.calls.length = 0;
}

test("下载只保留安装包和进度，不取得安装闸门或重启", async () => {
  const fixture = harness();
  const progress = [];
  await fixture.api.checkForUpdate();
  await fixture.api.downloadUpdate((next) => progress.push(next));
  assert.deepEqual(fixture.calls, ["download"]);
  assert.equal(progress.at(-1)?.downloaded, 128);
  assert.equal(progress.at(-1)?.total, 128);
});

test("后端忙碌拒绝安装后，不启动安装程序，也不释放别人的闸门", async () => {
  const fixture = harness();
  await downloaded(fixture);
  fixture.control.busy = true;
  await assert.rejects(fixture.api.installUpdate(), /刷新任务尚未结束/);
  assert.deepEqual(fixture.calls, ["begin_update_install"]);
  fixture.control.busy = false;
  await fixture.api.installUpdate();
  assert.deepEqual(fixture.calls, ["begin_update_install", "begin_update_install", "install"]);
});

test("安装程序失败时释放闸门，保留下载包并允许重试", async () => {
  const fixture = harness();
  await downloaded(fixture);
  fixture.control.installFails = true;
  await assert.rejects(fixture.api.installUpdate(), /安装程序启动失败/);
  assert.deepEqual(fixture.calls, ["begin_update_install", "install", "cancel_update_install"]);
  fixture.control.installFails = false;
  await fixture.api.installUpdate();
  assert.deepEqual(fixture.calls, ["begin_update_install", "install", "cancel_update_install", "begin_update_install", "install"]);
});

test("安装成功后由 Windows 安装器重启，不释放闸门或重复安装", async () => {
  const fixture = harness();
  await downloaded(fixture);
  await fixture.api.installUpdate();
  assert.deepEqual(fixture.calls, ["begin_update_install", "install"]);
  await assert.rejects(fixture.api.installUpdate());
  assert.deepEqual(fixture.calls, ["begin_update_install", "install"]);
});
