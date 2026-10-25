---
name: release
description: Git SyncDeck 发版流程：写这个版本自己的发布说明、升三处版本号、提交并推送 main、打 v* 标签触发 GitHub Actions 构建发布、等流水线跑完并核对 Release。会话里用户交代的任务全部完成后自动执行（见 CLAUDE.md「本项目例外」），或用户说"发个版""发布新版本"时使用。
---

# 发版

## 前提（任一条不满足就先停下，汇报原因）

- 本次会话的任务都已合回 `main`，没有还在干活的 worktree（`git worktree list`），`git status` 干净
- 本地刚跑过 `npm run build` 和 `cd src-tauri; cargo test`，全绿
- 上一个标签以来确实改了应用本身：`git diff --stat <上一个标签>..HEAD`，只动了 `docs/`、`README.md`、`.Codex/`、`AGENTS.md`、测试或脚本时，只推 `main`，不发版

## 步骤

1. **定版本号**：上一个标签以来的提交（`git log <上一个标签>..HEAD --oneline`）里有 `feat` 就升次版本号（0.2.0 → 0.3.0），否则升修订号（0.2.0 → 0.2.1）。用户指定了版本号就用用户的
2. **写发布说明** `docs/release-notes/v<新版本>.md`（**每个版本都要写自己的，不许照抄上一版**；缺了这个文件流水线会在第一步失败）：
   - 依据：上一个**发布成功的**标签以来的 `feat` / `fix` / `perf` 提交（`git log <标签>..HEAD --no-merges --format="%s%n%b"`），以及 `docs/requirements.md` 里这次新增的决定。上一个标签的流水线失败、没发出安装包的，它的改动也要写进这一版，并在"注意"里说一句（例子见 `v0.4.1.md`）
   - 写给使用者看的大白话：说用户能感觉到的变化，不写文件名、函数名、提交号，不写纯内部的重构和测试
   - 按需要用这几个小节：`## 新功能`、`## 改进`、`## 修复`、`## 注意`（默认值变化、升级要做的事、已知问题）。每条以加粗的短标题开头，后面一两句说清楚
   - 下载、安装、更新这些通用说明已经在 `_common.md` 里，流水线会自动接在网页版后面，这里不要重复
   - 写完用 `node scripts/release-notes.mjs <新版本>` 看一眼拼好的效果
3. **改版本号**，三处必须一致：
   - `npm version <新版本> --no-git-tag-version`（同时改 `package.json` 和 `package-lock.json`）
   - `src-tauri/tauri.conf.json` 的 `"version"`
   - `src-tauri/Cargo.toml` 的 `version`，再跑一次 `cd src-tauri; cargo test`，顺带更新 `Cargo.lock` 并确认仍然全绿
4. **提交**：`chore: 版本号升到 <新版本>`，只包含上面这几个文件和这个版本的发布说明
5. **推送**：先 `git push origin main`，再 `git tag v<新版本>`、`git push origin v<新版本>`
6. **等流水线**：`gh run list --workflow release.yml --limit 1` 找到这次的运行，用后台命令 `gh run watch <id> --exit-status` 等它结束（约 13 分钟），不要前台轮询
7. **核对**：
   - `gh release view v<新版本>`：有 `setup.exe`、`.msi`、各自的 `.sig` 和 `latest.json`；正文是这个版本的说明，后面接着通用说明
   - `latest.json` 的 `notes`（应用内"检查更新"显示的内容）是这个版本的说明：`gh release download v<新版本> --pattern latest.json --dir <临时目录>` 后看一眼

## 汇报

一句话说明版本号、本次包含哪些改动、Release 链接（https://github.com/Owenwoow/Git-SyncDeck/releases/tag/v<新版本>）、流水线结果。

## 失败时

- 不删标签、不 `push -f`、不改写历史
- 流水线失败：看 `gh run view <id> --log-failed`，汇报原因。是代码问题就修好提交，再发下一个修订号（它的发布说明要包含失败那一版的改动）；是 Secret、GitHub 权限等 Codex 做不了的问题，交给用户
- 发布说明写错了：改 `docs/release-notes/v<版本>.md` 并提交，再 `node scripts/release-notes.mjs <版本> | gh release edit v<版本> --notes-file -` 更新网页。如果还要改应用内显示的说明，下载 `latest.json`，**只改 `notes` 字段**（签名和下载地址一个字都不能动），再 `gh release upload v<版本> latest.json --clobber`，然后从 `https://github.com/Owenwoow/Git-SyncDeck/releases/latest/download/latest.json` 下载回来核对
