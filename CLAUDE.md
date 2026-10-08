@~/.claude/workflows/dev-collab.md

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# Git SyncDeck

Windows 桌面工具（Tauri 2 + React 19 + TypeScript + Rust）：在一个界面里查看多个本地 Git 项目与 GitHub 的同步状态，并一键推送/拉取。界面文案全部是简体中文。需求和第二阶段的实现决定见 `docs/requirements.md`，用户文档见 `README.md`。

## 常用命令

- 安装依赖：`npm install`
- 启动（桌面窗口 + 真实数据）：`npm run tauri dev`
- 启动（假数据）：`npm run tauri:mock`（桌面窗口），`npm run dev:mock`（浏览器，http://127.0.0.1:1420）
- 类型检查 + 前端构建：`npm run build`（没有 ESLint，`tsc` 就是代码检查）
- 全量测试：`cd src-tauri; cargo test`（CI 跑的就是 `npm run build` + `cargo test`）
- 单个集成测试：`cd src-tauri; cargo test --test repo_states -- --nocapture`，或 `--test commands_flow`
- 单元测试（例如 git 白名单）：`cd src-tauri; cargo test --lib git::`
- 打包：`npm run tauri build`，产物在 `src-tauri\target\release\bundle\`。因为开了 `createUpdaterArtifacts`，必须先设置环境变量 `TAURI_SIGNING_PRIVATE_KEY`（私钥在 `%USERPROFILE%\.tauri\git-syncdeck.key`，绝不进仓库），否则打包最后一步会失败
- 手动体验用测试仓库：`npm run test:repos`，生成在 `%TEMP%\syncdeck-test`，再在应用里"添加项目"选 `repos` 子目录

集成测试会自己调用 `node scripts/make-test-repos.mjs`，在 `%TEMP%` 里重新生成测试仓库（本地裸仓库充当云端），所以 PATH 里要有 `node` 和 `git`。`commands_flow` 用 `tauri::test::mock_builder()` 调全部命令，并用 `SYNCDECK_CONFIG_DIR` 把配置写到临时目录。

## 架构

### 前后端契约（改接口时四处一起改）

- `src/api/index.ts` 是界面拿数据的**唯一入口**，组件只能 `import from "@/api"`，不能引用 `src/mock/`。
- 有两套实现，签名必须一致：真实实现是 `src/api/tauri.ts`，调用 Tauri 命令；假数据实现是 `src/mock/api.ts`。`const impl: typeof real = ...` 这行会在编译期检查两边签名是否一致，所以新增或修改一个 API 时，两边都要改。
- `VITE_USE_MOCK=1`，或者不在 Tauri 窗口里（`__TAURI_INTERNALS__` 不存在）时，自动用假数据，侧边栏显示"演示数据"。
- 类型 `src/types/index.ts` 和 `src-tauri/src/model.rs`（serde）一一对应；命令 `src-tauri/src/commands.rs` 和 `src/api/tauri.ts` 一一对应，新命令还要在 `src-tauri/src/lib.rs` 的 `generate_handler!` 里注册。

### Rust 端

- **所有 git 子进程只能从 `git.rs` 的 `run_git` 创建**。它负责：不弹窗口（`CREATE_NO_WINDOW`、`GIT_TERMINAL_PROMPT=0`、`GCM_INTERACTIVE=never`）、超时（状态查询 10 秒，联网 60 秒）、UTF-8 加 `core.quotepath=false`，还有**命令白名单**。force push、reset、rebase、stash、clean、checkout、restore 等一律拒绝，有单元测试覆盖。新增 git 操作要先扩白名单、再补测试。
- `status.rs` 解析 `git status --porcelain=v2`，按固定优先级判定 7 种状态：未关联云端 > 有未提交改动 > 分叉 > 待推送 > 待拉取 > 已同步（"同步中"只在前端出现）。
- `ops.rs`：fetch / push（显式 refspec，不带 `+`）/ `pull --ff-only --no-rebase --no-autostash` / 提交并推送 / 最近提交。拉取不是快进时判为分叉，绝不自动合并。
- `commands.rs` 的 `AppState`：
  - `config` 持久化到 `config.json`（`config.rs`，先写临时文件再改名）
  - `cache` 在内存里保存每个项目最近一次的检查结果，检查失败时保留上一次的状态，另外附上"检查失败：原因"
  - `net_lock` 让联网操作（刷新、一键同步、提交推送）互斥
  - `util::map_limited` 最多 4 个项目并行
- 一键同步的进度由 Rust 端通过 Tauri 2 `Channel<SyncProgressEvent>` 推给前端，并行时 `index` 是完成顺序。
- `url.rs` 负责把远程地址和 git 错误输出里的账号、token 去掉，之后才能显示或保存。

### 前端

- `src/state/` 管理项目列表、一键同步进度、设置和主题；`src/lib/status.ts` 管状态文案和排序；颜色在 `src/index.css`，浅色和深色各一套。
- `src/components/ui/` 是 shadcn/ui 生成的组件；路径别名 `@` → `src`。
- 页面和文件的对照表见 `README.md` 的「页面与文件对照」。

## 项目约束

- **测试只用临时仓库**（`make-test-repos.mjs` 生成的），不要对用户真实项目跑 push / pull / commit。
- 不修改用户的全局 git 配置（包括 `safe.directory`），不读写凭据；取消监控只改清单，不删除任何文件。
- 开发服务器固定监听 `127.0.0.1:1420`（`strictPort`）：部分 Windows 机器上 `localhost` 只解析到 `::1`，Tauri CLI 会一直等不到开发服务器。
- 调试时用环境变量 `SYNCDECK_CONFIG_DIR` 指定配置目录，以免改到真实配置 `%APPDATA%\com.gitsyncdeck.desktop\config.json`。
- 发布：先把 `src-tauri/tauri.conf.json`、`package.json`、`src-tauri/Cargo.toml` 三处的版本号改成同一个并提交，再推 `v*` 标签。`.github/workflows/release.yml` 会先跑测试，再构建 Windows 安装包并发布到 Releases，同时上传应用内更新用的 `latest.json` 和 `.sig`（需要仓库 Secret `TAURI_SIGNING_PRIVATE_KEY`）。`tauri.conf.json` 里的 updater 公钥和这把私钥是一对，换私钥等于让所有已安装版本收不到更新。
- 自动提交只在一键同步时、对打开了开关的项目进行；不做后台定时提交。

## 本项目例外

- **每次会话收尾自动发版**（覆盖 dev-collab.md 里"不许 push"那条）：用户在本次会话里交代的所有任务都做完、合回 `main` 并且测试全绿之后，Claude 自己推送 `main`，再按 `.claude/skills/release/SKILL.md` 升版本号、打 `v*` 标签发布新版本，不用再问。
  - 用户在当前会话里另有要求（"这次先别发""攒着下次一起发""只推不发"等）时，以用户为准。
  - 本次改动没碰应用本身（只改了 `docs/`、`README.md`、`.claude/`、`CLAUDE.md`、测试或脚本）时只推 `main`，不发版，汇报时说一声。
