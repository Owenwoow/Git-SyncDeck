# Git SyncDeck

在一个界面里统一管理多个本地 Git 项目与 GitHub 的同步状态。离开一台电脑前点一次"一键同步"把改动推上去，到另一台电脑再点一次拉下来。

## 下载

到 [Releases](https://github.com/Owenwoow/Git-SyncDeck/releases/latest) 下载以 `-setup.exe` 结尾的安装程序。使用前需要先安装 [Git for Windows](https://git-scm.com/download/win)。安装包没有签名，Windows 提示"已保护你的电脑"时，点"更多信息"→"仍要运行"。

**升级**：新版本直接覆盖安装即可，不用先卸载，监控清单和设置都会保留。0.2.0 起支持应用内更新：启动时自动检查，有新版本时提示；也可以在"设置 → 关于"里手动检查，点"下载并安装"后会自动重启到新版本。

## 功能

- **主页**：所有监控项目的同步状态（7 种），需要处理的排在前面；按状态筛选、按名称搜索；点开看详情（领先/落后、未提交文件）。
  - 启动时先显示本地状态（不联网），随后在后台联网检查（打开了"启动时同步"就由后台做一次同步）。
  - 刷新时所有项目同时检查（最多 8 个并行），每个项目检查完就显示，列表不会锁住；刷新时 fetch 最多等 30 秒。
  - 断网等原因检查失败时，保留上一次的状态，并在项目上显示"检查失败：原因"。
- **一键同步**：每个项目先 `git fetch` + `git status` 检查（1 分钟内刚检查过的不再重复 fetch），然后待推送 → `git push`，待拉取 → `git pull --ff-only`，分叉 → 先预演合并（见下）；有未提交改动（没开自动提交）/ 未关联云端的项目不做任何操作，记为异常。最多 8 个项目并行，进度实时显示，推送、拉取时细到 git 的百分比；进度弹窗随时可以关，关掉后同步照常进行，进度显示在「一键同步」按钮和列表上。
- **换电脑时自动同步**（v0.4.0 起，每项都能在设置里关）：程序常驻托盘、开机自启。
  - 离开（锁屏）：只同步"有东西要推"的项目，没有就完全不联网。关机和睡眠前不同步（v0.5.0 起去掉，Windows 给的时间太短，不可靠），没推上去的下次开机、唤醒或解锁时补上。
  - 回到这台电脑（启动、唤醒、解锁）：对所有项目跑一遍一键同步。同一种自动同步 10 分钟内只做一次。
  - 托盘图标：绿 = 都已同步，黄 = 本机有改动还没同步，红 = 有冲突或出错，蓝 = 同步中；菜单里能立即同步、暂停自动同步到明天。
  - 后台同步出错或有冲突时发系统通知（成功默认不通知）；可选每 30 分钟定时检查云端（默认关，只提示不拉取）。
  - 后台同步时打开窗口能随时看进度（v0.6.0 起，参考 GitHub Desktop）：「一键同步」按钮变成"后台同步 3/7"的进度按钮，点开看每个项目的百分比；结束后主页标题下保留"上次后台同步"的结果，点开能直接处理异常；托盘悬停显示"正在同步 3/7"。
- **自动提交**（类似 Obsidian Git，按项目开关，默认关）：在项目详情里打开后，一键同步和自动同步遇到这个项目有未提交改动时，会 `git add -A` → 按模板提交 → 推送。模板在设置里改，默认 `自动同步：{date} 来自 {host}`，`{host}` 可以在设置里填"本机名称"。先在本地提交，云端也有新提交时按分叉处理。
  - 提交前检查：有超过上限（默认 50 MB）的文件，或没被忽略的疑似密钥文件（`.env`、`*.pem`、`*.key`、`id_rsa` 等）时不提交，记为异常。
- **分叉时自动合并**（默认开）：先用 `git merge-tree` 在内存里预演，没有冲突就合并（提交信息"自动合并：来自 {host}"）并推送；有冲突就什么都不动，项目标红，可以「复制给 AI」一段现成的提示词，让 AI 教你解决。
- **代理**：设置里选自动 / 不用代理 / 手动填写，只对 SyncDeck 调用的 git 生效。自动 = git 自己配了代理就用 git 的，没配而 Windows 系统代理开着就用系统代理（带上系统的绕过列表）。
- **异常处理**：
  - 复制诊断文本：内容是项目信息、文件名、最近 3 次提交，可以直接粘贴给 AI，不含文件内容。分叉的项目还有「复制给 AI」。
  - 直接提交并推送：add -A → commit → push；提交后如果发现落后于云端，会停下来报告，不自动合并。
  - 本次跳过：异常标记保留在主页，直到下次同步成功或刷新到"已同步"。
  - 打开终端（Windows Terminal，没有就用 cmd）或 VS Code。
- **添加项目**：选择目录后递归扫描（最多 3 层），可按名称、路径、仓库搜索，勾选加入或取消监控。
- **设置**：默认代码目录、联网超时、代理、自动同步的各项开关、通知、自动提交的提交信息模板和本机名称、冲突与提交检查、浅色/深色主题、关于与检查更新。监控清单、设置、上次同步时间和遗留异常都会保存，重启后恢复。设置存在每台电脑自己的配置文件里，三台电脑可以设得不一样。

### 三台电脑怎么用

1. 每台电脑装好后保持开机自启，在设置里填好"本机名称"（如 家里笔记本 / 台式机 / 学校笔记本），把想全自动的项目打开"自动提交"，写好各项目的 `.gitignore`。
2. 离开电脑时按 Win+L 锁屏，等托盘变绿再关机或合盖（关机和睡眠前不会自动同步）。
3. 到另一台电脑，等托盘变绿再开始写。
4. 远程控制让两台同时开着时，改完一边就在托盘点「立即同步」，另一边也点一下。
5. 托盘变红：打开主界面看原因，冲突就点「复制给 AI」。

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

本地打包会同时生成应用内更新用的签名文件，需要先设置私钥环境变量（私钥在 `%USERPROFILE%\.tauri\git-syncdeck.key`，不在仓库里）：

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -Raw "$env:USERPROFILE\.tauri\git-syncdeck.key"
```

### 发布新版本

GitHub Actions（`.github/workflows/release.yml`）会在推送 `v*` 标签时先跑测试，再在 Windows 上构建安装包，并发布到 Releases，同时上传应用内更新用的 `latest.json` 和签名文件。

前提：仓库 Settings → Secrets 里有 `TAURI_SIGNING_PRIVATE_KEY`（私钥文件的内容，只需配置一次）。私钥务必备份，丢了的话，已安装的版本就再也收不到应用内更新。

1. 写这个版本的发布说明 `docs/release-notes/v0.2.0.md`：用大白话写这个版本的新功能、改进、修复和注意事项，格式参考同目录下的其他版本。下载、安装、更新这些通用说明在 `_common.md` 里，会自动接在后面。**缺了这个文件，流水线会在一开始就失败。**
2. 把 `src-tauri/tauri.conf.json`、`package.json`、`src-tauri/Cargo.toml` 里的版本号改成新版本，例如 `0.2.0`，和发布说明一起提交。
3. 打标签并推送：`git tag v0.2.0`，`git push origin main v0.2.0`。

Release 网页上显示"本版本说明 + 通用说明"；应用内"检查更新"只显示本版本说明（来自 `latest.json`）。想先看看效果：`node scripts/release-notes.mjs 0.2.0`。改已经发布的版本：`node scripts/release-notes.mjs 0.2.0 | gh release edit v0.2.0 --notes-file -`（这只改网页，应用内显示的说明要另外改 `latest.json`）。

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

假数据模式下侧边栏底部会显示"演示数据"。演示模式里把窗口切走 2 秒以上再切回来，会模拟一次"解锁后同步"（带百分比），方便看后台同步的进度；也可以在开发者工具的控制台里运行 `__syncdeckDemo.autoSync("lock")` 之类。

## 配置文件

`%APPDATA%\com.gitsyncdeck.desktop\config.json`（例如 `C:\Users\<你>\AppData\Roaming\com.gitsyncdeck.desktop\config.json`）

保存监控清单、设置、每个项目的上次同步时间和遗留异常。不保存任何凭据。开发测试时可以用环境变量 `SYNCDECK_CONFIG_DIR` 把配置放到别的目录，不影响真实配置。

## 测试

不碰真实项目，全部在临时目录里进行：

```powershell
npm run test:repos                     # 在 %TEMP%\syncdeck-test 创建测试仓库（本地裸仓库充当云端）
cd src-tauri; cargo test -- --nocapture
```

- `scripts/make-test-repos.mjs` 构造出这些状态：已同步、待推送、待拉取、有未提交改动（含中文文件名）、分叉、未关联云端（无远程 / 无上游 / detached HEAD）、检查失败（远程地址无效）、有改动且落后、有改动且落后且和云端改了同一个文件。
- `src-tauri/tests/repo_states.rs` 验证状态检测、一键同步、提交并推送，并检查异常项目的提交和工作区没被改动。
- `src-tauri/tests/auto_commit.rs` 验证自动提交：开关打开时按模板提交并推送；开关关闭时不动；落后时先拉取再提交；拉取冲突时不提交，本地改动还在。
- `src-tauri/tests/commands_flow.rs` 用 Tauri 模拟运行时调用全部命令：扫描 → 添加 → 刷新 → 同步进度 → 持久化 → 取消监控。

想在界面里亲手试：运行 `npm run test:repos`，然后在应用里"添加项目"，选择 `%TEMP%\syncdeck-test\repos`。

## 安全设计

所有 git 子进程都从 `src-tauri/src/git.rs` 的 `run_git` 创建：

- **不弹窗口、不等待输入**：`CREATE_NO_WINDOW`；`GIT_TERMINAL_PROMPT=0`；`GCM_INTERACTIVE=never`，没有已保存的凭据就直接报错，不弹登录窗口。
- **中文和超时**：加 `-c core.quotepath=false`，输出按 UTF-8 解码。状态查询超时 10 秒，fetch / push / pull / commit 的超时在设置里选（1、3、5、10 分钟，默认 3 分钟），超时后终止。fetch 只拉当前分支的上游，云端分支已删除时退回拉整个远程。
- **白名单**：只允许 status、fetch、push（显式 refspec，不带 +）、pull（`--ff-only --no-rebase --no-autostash`）、add -A、commit -m、merge-tree 预演、`merge --no-edit -m <信息> <提交>` 和 `merge --abort`、log、rev-parse、config 只读等。`--force`、reset、rebase、stash、clean、checkout、restore、`merge -s / -X / --squash` 等一律拒绝，有单元测试覆盖。
- **自动合并的防护**：分支配置了 `branch.<分支>.mergeoptions` 时不自动合并；合并后核对结果和预演一致（没有 MERGE_HEAD、两个父提交、树哈希都对）才推送。
- **代理**：只通过 `-c http.proxy=…`（和 `http.<网址>.proxy`）以及子进程的环境变量生效，不改全局 git 配置。
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
├─ ops.rs         fetch / push / pull / 预演合并与合并 / 提交前检查 / 提交并推送 / 最近提交
├─ autosync.rs    离开 / 到达自动同步的调度（10 分钟限制、暂停、通知文案）
├─ sysevents.rs   Windows 系统事件：锁屏 / 解锁、唤醒
├─ tray.rs        托盘图标与菜单
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
| 一键同步 / 后台同步的进度和结果弹窗 | `src/components/sync/SyncDialog.tsx`（进度规则在 `src/state/syncProgress.ts`） |
| 　进度按钮、"上次后台同步" | `src/pages/HomePage.tsx` |
| 添加项目 | `src/pages/AddProjectsPage.tsx` |
| 设置（左侧分类目录 + 右侧单类内容；含「退出 SyncDeck」按钮、「关于」里的检查更新） | `src/pages/SettingsPage.tsx` |
| 侧边栏 | `src/components/layout/Sidebar.tsx` |
| 状态文案 / 颜色 | `src/lib/status.ts` / `src/index.css` |
| 诊断文本格式 | `src/lib/diagnostic.ts` |

## 已知限制

- **只看当前分支**：只处理每个项目当前所在的分支；其他本地分支上没推送的提交不会提示。
- **只用已保存的凭据**：
  - HTTPS 远程需要 Git 凭据管理器里已经存有登录，没有就直接报错，不会弹登录窗口。
  - SSH 远程需要没有密码短语的密钥，或已加载到 ssh-agent 的密钥。
- **不自动解决冲突**：无冲突的分叉会自动合并；真正的冲突、正在进行的 merge / rebase 都需要手动处理，可以「复制给 AI」。
- **"dubious ownership"**：外置硬盘上的仓库，或其他用户创建的仓库，git 可能拒绝操作。界面会显示 git 的原始错误，需要按提示自己执行 `git config --global --add safe.directory <路径>`，本工具不会改你的全局 git 配置。
- **扫描范围**：最多 3 层，跳过 node_modules、.venv、target、dist、build 等目录和所有以 `.` 开头的目录，不跟随符号链接。
- **自动同步只在换电脑的时刻**：离开 / 到达时才自动同步，不会按固定间隔在后台提交；关机和睡眠前不同步，没推上去的下次开机、唤醒或解锁时补推。
- **代理只管 https 地址**：SSH 地址的仓库、仓库里单独配置的 `remote.<远程名>.proxy` 不受设置里的代理影响。
- **安装包未签名**：首次运行时 Windows SmartScreen 可能提示"未知发布者"。应用图标暂时是 Tauri 默认图标。
