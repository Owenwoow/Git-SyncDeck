import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs/promises";
import ts from "typescript";
const source = await fs.readFile(new URL("../src/state/settingsQueue.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } }).outputText;
const { SettingsQueue } = await import("data:text/javascript;base64," + Buffer.from(compiled).toString("base64"));
const initial = { theme: "light", host: "电脑", enabled: false, paused: null };
function deferred() {
    let resolve;
    let reject;
    const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
    return { promise, resolve, reject };
}
test("快速连续修改串行保存，旧响应不覆盖后续选择或其他字段", async () => {
    const writes = [];
    const queue = new SettingsQueue(async () => initial, (patch) => {
        const result = deferred();
        writes.push({ patch, result });
        return result.promise;
    });
    await queue.reload();
    const one = queue.save({ theme: "dark" });
    const two = queue.save({ theme: "light", host: "笔记本" });
    await Promise.resolve();
    assert.equal(writes.length, 1);
    assert.deepEqual(queue.getSnapshot().value, { ...initial, host: "笔记本" });
    writes[0].result.resolve({ ...initial, theme: "dark" });
    await one;
    assert.equal(queue.getSnapshot().value?.theme, "light");
    assert.equal(queue.getSnapshot().value?.host, "笔记本");
    await Promise.resolve();
    assert.equal(writes.length, 2);
    writes[1].result.resolve({ ...initial, host: "笔记本" });
    await two;
    assert.deepEqual(writes.map((write) => write.patch), [{ theme: "dark" }, { theme: "light", host: "笔记本" }]);
    assert.equal(queue.getSnapshot().saving, false);
});
test("失败只回退失败修改，后续字段仍能保存，并可重试失败值", async () => {
    let disk = { ...initial };
    let fail = true;
    const queue = new SettingsQueue(async () => disk, async (patch) => {
        if (patch.theme && fail)
            throw new Error("配置目录无法写入");
        disk = { ...disk, ...patch };
        return disk;
    });
    await queue.reload();
    const one = queue.save({ theme: "dark" }).catch(() => { });
    const two = queue.save({ host: "台式机" });
    await Promise.all([one, two]);
    assert.deepEqual(queue.getSnapshot().value, { ...initial, host: "台式机" });
    assert.equal(queue.getSnapshot().error, "配置目录无法写入");
    assert.equal(queue.getSnapshot().fieldStates.theme, "error");
    fail = false;
    await queue.retry();
    assert.deepEqual(disk, { ...initial, host: "台式机", theme: "dark" });
    assert.equal(queue.getSnapshot().error, null);
});
test("失败后已重新选择的字段不会被旧重试覆盖", async () => {
    let disk = { ...initial };
    let fail = true;
    const queue = new SettingsQueue(async () => disk, async (patch) => {
        if (fail)
            throw new Error("保存失败");
        disk = { ...disk, ...patch };
        return disk;
    });
    await queue.reload();
    await queue.save({ theme: "dark" }).catch(() => { });
    fail = false;
    await queue.save({ theme: "light" });
    await queue.retry();
    assert.equal(disk.theme, "light");
    assert.equal(queue.getSnapshot().error, null);
});
test("重新读取失败可重试，且不触发保存或抹掉已有值", async () => {
    let reads = 0;
    let writes = 0;
    const queue = new SettingsQueue(async () => {
        reads++;
        if (reads === 2)
            throw new Error("无法读取配置");
        return initial;
    }, async () => { writes++; return initial; });
    await queue.reload();
    await queue.reload().catch(() => { });
    assert.equal(queue.getSnapshot().readError, "无法读取配置");
    assert.deepEqual(queue.getSnapshot().value, initial);
    await queue.reload();
    assert.equal(queue.getSnapshot().readError, null);
    assert.equal(writes, 0);
});
test("旧读取响应不会覆盖期间收到的托盘事件", async () => {
    const pending = deferred();
    const queue = new SettingsQueue(() => pending.promise, async () => initial);
    const read = queue.reload();
    await Promise.resolve();
    queue.receive({ ...initial, paused: "明天" });
    pending.resolve(initial);
    await read;
    assert.equal(queue.getSnapshot().value?.paused, "明天");
});
test("保存期间的托盘变化和后续乐观修改都保留", async () => {
    let disk = { ...initial };
    const write = deferred();
    let count = 0;
    const queue = new SettingsQueue(async () => disk, async (patch) => {
        count++;
        if (count === 1)
            return write.promise;
        disk = { ...disk, ...patch };
        return disk;
    });
    await queue.reload();
    const saving = queue.save({ theme: "dark" });
    await Promise.resolve();
    disk = { ...initial, theme: "dark", paused: "明天" };
    queue.receive({ ...initial, paused: "明天" });
    const later = queue.save({ enabled: true });
    write.resolve({ ...initial, theme: "dark" });
    await saving;
    assert.equal(queue.getSnapshot().value?.paused, "明天");
    assert.equal(queue.getSnapshot().value?.enabled, true);
    await later;
    assert.deepEqual(queue.getSnapshot().value, { ...initial, theme: "dark", paused: "明天", enabled: true });
});
test("放弃失败代理后重试不会重新应用旧代理，其他失败字段仍可重试", async () => {
    const original = { proxyMode: "auto", proxyUrl: "", host: "电脑" };
    let disk = { ...original };
    let fail = true;
    const writes = [];
    const queue = new SettingsQueue(async () => disk, async (patch) => {
        writes.push(patch);
        if (fail) throw new Error("配置无法保存");
        disk = { ...disk, ...patch };
        return disk;
    });
    await queue.reload();
    await queue.save({ proxyMode: "manual", proxyUrl: "bad-proxy" }).catch(() => {});
    await queue.save({ host: "笔记本" }).catch(() => {});
    queue.discardFailed(["proxyMode", "proxyUrl"]);
    assert.equal(queue.getSnapshot().fieldStates.proxyMode, undefined);
    assert.equal(queue.getSnapshot().fieldStates.host, "error");
    fail = false;
    await queue.retry();
    assert.deepEqual(disk, { ...original, host: "笔记本" });
    assert.deepEqual(writes.slice(2), [{ host: "笔记本" }]);
});
