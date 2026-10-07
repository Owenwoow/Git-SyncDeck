# Git SyncDeck

在一个界面里统一管理多个本地 Git 项目与 GitHub 的同步状态。离开一台电脑前点一次"一键同步"把改动推上去，到另一台电脑再点一次拉下来。

## 下载

到 [Releases](https://github.com/Owenwoow/Git-SyncDeck/releases/latest) 下载以 `-setup.exe` 结尾的安装程序。使用前需要先安装 [Git for Windows](https://git-scm.com/download/win)。安装包没有签名，Windows 提示"已保护你的电脑"时，点"更多信息"→"仍要运行"。

## 功能

- **主页**：所有监控项目的同步状态（7 种），需要处理的排在前面；按状态筛选、按名称搜索；点开看详情（领先/落后、未提交文件）。
  - 启动时先显示本地状态（不联网），随后自动在后台联网刷新一次。
  - 断网等原因检查失败时，保留上一次的状态，并在项目上显示"检查失败：原因"。
- **一键同步**：每个项目先 `git fetch` + `git status` 检查，然后待推送 → `git push`，待拉取 → `git pull --ff-only`；有未提交改动 / 分叉 / 未关联云端的项目不做任何操作，记为异常。最多 4 个项目并行，进度实时显示。
- **异常处理**：
  - 复制诊断文本：内容是项目信息、文件名、最近 3 次提交，可以直接粘贴给 AI，不含文件内容。
  - 直接提交并推送：add -A → commit → push；提交后如果发现落后于云端，会停下来报告，不自动合并。
  - 本次跳过：异常标记保留在主页，直到下次同步成功或刷新到"已同步"。
  - 打开终端（Windows Terminal，没有就用 cmd）或 VS Code。
- **添加项目**：选择目录后递归扫描（最多 3 层），勾选加入或取消监控。
- **设置**：默认代码目录、浅色/深色主题。监控清单、设置、上次同步时间和遗留异常都会保存，重启后恢复。

## 环境要求

| 依赖 | 说明 |
|---|---|
| Git for Windows | 所有 git 操作都调用系统的 git 命令行，直接沿用你已保存的 GitHub 登录 |
| Node.js 20.19+ / 22.12+ | 前端构建 |
| MSVC 构建工具 | VS 2022 Build Tools，"使用 C++ 的桌面开发" |
| Rust（stable-msvc） | Tauri 桌面壳 |
| WebView2 Runtime | Windows 10/11 一般自带 |

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install --id Rustlang.Rustup -e
```

## 启动与打包

```powershell
npm install
npm run tauri dev     # 桌面窗口，真实数据
npm run tauri build   # 打包，安装包在 src-tauri\target\release\bundle\（nsis\*.exe、msi\*.msi）
```

### 发布新版本

GitHub Actions（`.github/workflows/release.yml`）会在推送 `v*` 标签时先跑测试，再在 Windows 上构建安装包，并发布到 Releases。

1. 把 `src-tauri/tauri.conf.json`、`package.json`、`src-tauri/Cargo.toml` 里的版本号改成新版本，例如 `0.2.0`，然后提交。
2. 打标签并推送：`git tag v0.2.0`，`git push origin main v0.2.0`。

也可以在 GitHub 网页的 Actions 页面手动运行 "Release (Windows)"。

## 假数据模式（继续调界面用）

假数据在 `src/mock/`，和真实实现的函数签名完全一致。满足以下任一条件就用假数据：

- 环境变量 `VITE_USE_MOCK=1`
- 在普通浏览器里打开（不在 Tauri 窗口内）

```powershell
npm run tauri:mock    # 桌面窗口 + 假数据
npm run dev:mock      # 浏览器 + 假数据：http://127.0.0.1:1420
$env:VITE_USE_MOCK = "1"; npm run tauri dev   # 也可以直接设环境变量
```

假数据模式下侧边栏底部会显示"演示数据"。

## 配置文件

`%APPDATA%\com.gitsyncdeck.desktop\config.json`（例如 `C:\Users\<你>\AppData\Roaming\com.gitsyncdeck.desktop\config.json`）

保存监控清单、设置、每个项目的上次同步时间和遗留异常。不保存任何凭据。开发测试时可以用环境变量 `SYNCDECK_CONFIG_DIR` 把配置放到别的目录，不影响真实配置。

## 测试

不碰真实项目，全部在临时目录里进行：

```powershell
npm run test:repos                     # 在 %TEMP%\syncdeck-test 创建测试仓库（本地裸仓库充当云端）
cd src-tauri; cargo test -- --nocapture
```

- `scripts/make-test-repos.mjs` 构造出这些状态：已同步、待推送、待拉取、有未提交改动（含中文文件名）、分叉、未关联云端（无远程 / 无上游 / detached HEAD）、检查失败（远程地址无效）、有改动且落后。
- `src-tauri/tests/repo_states.rs` 验证状态检测、一键同步、提交并推送，并检查异常项目的提交和工作区没被改动。
- `src-tauri/tests/commands_flow.rs` 用 Tauri 模拟运行时调用全部命令：扫描 → 添加 → 刷新 → 同步进度 → 持久化 → 取消监控。

想在界面里亲手试：运行 `npm run test:repos`，然后在应用里"添加项目"，选择 `%TEMP%\syncdeck-test\repos`。

## 安全设计

所有 git 子进程都从 `src-tauri/src/git.rs` 的 `run_git` 创建：

- **不弹窗口、不等待输入**：`CREATE_NO_WINDOW`；`GIT_TERMINAL_PROMPT=0`；`GCM_INTERACTIVE=never`，没有已保存的凭据就直接报错，不弹登录窗口。
- **中文和超时**：加 `-c core.quotepath=false`，输出按 UTF-8 解码。状态查询超时 10 秒，fetch / push / pull / commit 超时 60 秒，超时后终止。
- **白名单**：只允许 status、fetch、push（显式 refspec，不带 +）、pull（`--ff-only --no-rebase --no-autostash`）、add -A、commit -m、log、config 只读等。`--force`、reset、rebase、stash、clean、checkout、restore 等一律拒绝，有单元测试覆盖。
- **不碰凭据**：远程地址和错误输出里的账号或 token 一律去掉后才显示或保存。
- **不动文件**：取消监控只改清单，不删除任何文件或仓库。

## 目录结构

```
src/
├─ api/
│  ├─ index.ts    ★ 界面唯一的数据入口，按开关选择真实 / 假数据实现
│  └─ tauri.ts    真实实现：调用 Tauri 命令
├─ mock/          假数据 + 假实现（api.ts），只允许 src/api/ 引用
├─ types/         类型定义（与 src-tauri/src/model.rs 一一对应）
├─ lib/           状态文案与排序、诊断文本、时间、剪贴板
├─ state/         项目列表与一键同步进度、设置与主题
├─ components/    界面组件（ui/ 为 shadcn/ui）
└─ pages/         三个页面
src-tauri/src/
├─ commands.rs    Tauri 命令（与 src/api/tauri.ts 对应）、缓存、持久化
├─ git.rs         调用 git 的唯一入口：不弹窗、超时、安全白名单
├─ status.rs      解析 porcelain v2，判定 7 种状态
├─ ops.rs         fetch / push / pull / 提交并推送 / 最近提交
├─ scan.rs        目录扫描
├─ config.rs      config.json 读写
├─ system.rs      打开终端 / VS Code
└─ url.rs         远程地址脱敏
scripts/make-test-repos.mjs   测试仓库生成脚本
```

## 页面与文件对照

| 界面 | 文件 |
|---|---|
| 主页 | `src/pages/HomePage.tsx` |
| 　概览 / 搜索筛选 / 列表 | `src/components/projects/OverviewStats.tsx` / `ProjectToolbar.tsx` / `ProjectList.tsx` |
| 项目详情抽屉 | `src/components/projects/ProjectDetailSheet.tsx` |
| 异常操作按钮 / 提交弹窗 | `src/components/projects/ProjectActions.tsx` / `CommitPushDialog.tsx` |
| 一键同步对话框 | `src/components/sync/SyncDialog.tsx` |
| 添加项目 | `src/pages/AddProjectsPage.tsx` |
| 设置 | `src/pages/SettingsPage.tsx` |
| 侧边栏 | `src/components/layout/Sidebar.tsx` |
| 状态文案 / 颜色 | `src/lib/status.ts` / `src/index.css` |
| 诊断文本格式 | `src/lib/diagnostic.ts` |

## 已知限制

- **只看当前分支**：只处理每个项目当前所在的分支；其他本地分支上没推送的提交不会提示。
- **只用已保存的凭据**：
  - HTTPS 远程需要 Git 凭据管理器里已经存有登录，没有就直接报错，不会弹登录窗口。
  - SSH 远程需要没有密码短语的密钥，或已加载到 ssh-agent 的密钥。
- **不自动解决冲突**：分叉、合并冲突、正在进行的 merge / rebase 都需要手动处理，可以复制诊断文本交给 AI。
- **"dubious ownership"**：外置硬盘上的仓库，或其他用户创建的仓库，git 可能拒绝操作。界面会显示 git 的原始错误，需要按提示自己执行 `git config --global --add safe.directory <路径>`，本工具不会改你的全局 git 配置。
- **扫描范围**：最多 3 层，跳过 node_modules、.venv、target、dist、build 等目录和所有以 `.` 开头的目录，不跟随符号链接。
- **自动刷新只有一次**：只在启动时自动联网刷新一次，之后要点"刷新状态"。
- **安装包未签名**：首次运行时 Windows SmartScreen 可能提示"未知发布者"。应用图标暂时是 Tauri 默认图标。
