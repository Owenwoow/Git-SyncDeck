---
name: release
description: Git SyncDeck 发版流程：升三处版本号、提交并推送 main、打 v* 标签触发 GitHub Actions 构建发布、等流水线跑完并核对 Release。会话里用户交代的任务全部完成后自动执行（见 CLAUDE.md「本项目例外」），或用户说"发个版""发布新版本"时使用。
---

# 发版

## 前提（任一条不满足就先停下，汇报原因）

- 本次会话的任务都已合回 `main`，没有还在干活的 worktree（`git worktree list`），`git status` 干净
- 本地刚跑过 `npm run build` 和 `cd src-tauri; cargo test`，全绿
- 上一个标签以来确实改了应用本身：`git diff --stat <上一个标签>..HEAD`，只动了 `docs/`、`README.md`、`.claude/`、`CLAUDE.md`、测试或脚本时，只推 `main`，不发版

## 步骤

1. **定版本号**：上一个标签以来的提交（`git log <上一个标签>..HEAD --oneline`）里有 `feat` 就升次版本号（0.2.0 → 0.3.0），否则升修订号（0.2.0 → 0.2.1）。用户指定了版本号就用用户的
2. **改版本号**，三处必须一致：
   - `npm version <新版本> --no-git-tag-version`（同时改 `package.json` 和 `package-lock.json`）
   - `src-tauri/tauri.conf.json` 的 `"version"`
   - `src-tauri/Cargo.toml` 的 `version`，再跑一次 `cd src-tauri; cargo test`，顺带更新 `Cargo.lock` 并确认仍然全绿
3. **提交**：`chore: 版本号升到 <新版本>`，只包含上面这几个文件
4. **推送**：先 `git push origin main`，再 `git tag v<新版本>`、`git push origin v<新版本>`
5. **等流水线**：`gh run list --workflow release.yml --limit 1` 找到这次的运行，用后台命令 `gh run watch <id> --exit-status` 等它结束（约 13 分钟），不要前台轮询
6. **核对**：`gh release view v<新版本>`，确认有 `setup.exe`、`.msi`、各自的 `.sig` 和 `latest.json`

## 汇报

一句话说明版本号、本次包含哪些改动、Release 链接（https://github.com/Owenwoow/Git-SyncDeck/releases/tag/v<新版本>）、流水线结果。

## 失败时

- 不删标签、不 `push -f`、不改写历史
- 流水线失败：看 `gh run view <id> --log-failed`，汇报原因。是代码问题就修好提交，再发下一个修订号；是 Secret、GitHub 权限等 Claude 做不了的问题，交给用户
