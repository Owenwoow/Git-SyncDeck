// 生成某个版本的 GitHub Release 说明：docs/release-notes/v<版本>.md（这个版本改了什么）+ _common.md（下载、安装、更新）。
// 发版流水线用它填 Release 正文；缺了这个版本的说明文件就报错退出，免得又发出一段千篇一律的文字。
//
// 用法：node scripts/release-notes.mjs [版本号] [--only-version]
//       不写版本号时用 package.json 里的版本；结果输出到标准输出
//       --only-version：只输出这个版本的说明，不带通用说明（用于应用内更新显示的 latest.json）
//       补改已发布的版本：node scripts/release-notes.mjs 0.4.1 | gh release edit v0.4.1 --notes-file -

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const dir = path.join(root, "docs", "release-notes");
const args = process.argv.slice(2);
const onlyVersion = args.includes("--only-version");
const given = args.find((a) => !a.startsWith("--"));
const version = (given ?? JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8")).version).replace(/^v/, "");

const read = (name) => {
  const file = path.join(dir, name);
  if (!fs.existsSync(file)) return null;
  // 去掉 Windows 换行里的 \r，免得进了 GitHub Actions 的输出变量
  return fs.readFileSync(file, "utf8").replace(/\r/g, "").trim();
};

const notes = read(`v${version}.md`);
if (!notes) {
  console.error(`缺少 docs/release-notes/v${version}.md（或内容为空）：发版前先写这个版本的发布说明，格式参考同目录下的其他文件`);
  process.exit(1);
}
if (onlyVersion) {
  process.stdout.write(`${notes}\n`);
  process.exit(0);
}
const common = read("_common.md");
if (!common) {
  console.error("缺少 docs/release-notes/_common.md（下载、安装、更新等通用说明）");
  process.exit(1);
}

process.stdout.write(`${notes}\n\n---\n\n${common}\n`);
