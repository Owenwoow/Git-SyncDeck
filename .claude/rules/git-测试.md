---
paths:
  - src-tauri/tests/**
  - src-tauri/src/git.rs
  - src-tauri/src/ops.rs
  - scripts/make-test-repos.mjs
---

# 写 git 相关代码和测试时的坑

- 本机全局 git 配置（如 `http.proxy`、`merge.*`）会影响测试结果。涉及代理、合并、配置读取的测试，用 `GIT_CONFIG_GLOBAL`（指向空文件）和 `GIT_CONFIG_NOSYSTEM=1` 隔离。
- `git merge-tree --write-tree` 退出码 1 不一定是冲突：引用不存在时也是 1，但输出为空。要同时看到合法的树哈希才算冲突。
- `git merge` 会自动带上 `branch.<分支>.mergeoptions` 里的选项（如 `-s ours`、`--squash`），白名单管不到，所以自动合并前要先读这项配置，合并后还要核对结果。
- 按网址的 `http.<网址>.proxy` 比 `-c http.proxy=` 优先，要覆盖必须同时加 `-c http.<网址>.proxy=`；`-c http.proxy=`（空值）会让 git 忽略全局 `http.proxy` 和 `HTTPS_PROXY` / `ALL_PROXY`；git 对 https 地址不用 `HTTP_PROXY`。
- `git config -z --get-regexp` 的输出是"键\n值\0"，值为空时是"键\n\0"，没写值时是"键\0"；键里的 section 和变量名会转成小写，网址部分保留原样。
- 测"合并结果被改"要用 `post-merge` 钩子（`pre-merge-commit` 改暂存区不影响合并提交）；`post-merge` 运行时 MERGE_HEAD 还在，钩子里不能 `commit --amend`，要用 `commit-tree` + `update-ref`。
- 在 Git Bash 里用 `sed -i` 改文件会把 CRLF 换成 LF；改代码用编辑工具。
- 结束后台的 `npx vite` 时，node 子进程可能留下来继续占着端口，要按 PID 再结束一次。
