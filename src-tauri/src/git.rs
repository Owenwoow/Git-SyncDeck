//! 调用系统 git 命令行的唯一入口。所有 git 子进程都从 `run_git` 创建，统一处理：
//! - 不弹出命令行窗口（CREATE_NO_WINDOW）
//! - 不等待任何凭据输入（GIT_TERMINAL_PROMPT=0、GCM_INTERACTIVE=never）
//! - 中文文件名（core.quotepath=false），输出按 UTF-8 解码
//! - 超时后终止子进程
//! - 代理设置（`ProxyOverride`）：只通过命令行 `-c` 和子进程环境变量生效，不改用户的 git 配置
//! - 安全白名单：只允许本工具需要的子命令和参数，任何可能改写历史或丢失改动的操作一律拒绝

use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::model::{GitProgress, ProgressPhase};
use crate::url::redact;

/// 状态查询类命令（status、config、log……）的超时
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
/// 联网或写入类命令（fetch、push、pull、add、commit）的默认超时，可在设置里改
pub const DEFAULT_NETWORK_TIMEOUT_SECS: u64 = 180;
/// 设置里可选的联网超时（秒）：1、3、5、10 分钟
pub const NETWORK_TIMEOUT_CHOICES: [u64; 4] = [60, 180, 300, 600];

static NETWORK_TIMEOUT_SECS: AtomicU64 = AtomicU64::new(DEFAULT_NETWORK_TIMEOUT_SECS);

/// 当前的联网超时（启动时和保存设置时由 `set_network_timeout` 更新）
pub fn network_timeout() -> Duration {
    Duration::from_secs(NETWORK_TIMEOUT_SECS.load(Ordering::Relaxed))
}

pub fn set_network_timeout(secs: u64) {
    NETWORK_TIMEOUT_SECS.store(secs, Ordering::Relaxed);
}

/// 刷新状态时 fetch 的超时上限：刷新只是看一眼云端，代理卡住时不该让整轮转好几分钟。
/// 一键同步、推送、拉取仍用设置里的联网超时。
pub const REFRESH_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// 刷新时的 fetch 超时：30 秒和设置里的联网超时取较小的
pub fn refresh_timeout() -> Duration {
    refresh_timeout_for(network_timeout())
}

fn refresh_timeout_for(network: Duration) -> Duration {
    REFRESH_FETCH_TIMEOUT.min(network)
}

// ---------------- 代理 ----------------

/// SyncDeck 调用的 git 用什么代理。只体现在子进程的命令行参数和环境变量上，不改用户的 git 配置。
/// 由 commands 在每轮刷新 / 同步开始前、保存设置时重新解析并设置。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProxyOverride {
    /// 不覆盖：git 按自己的配置（http.proxy）和环境变量决定
    #[default]
    Inherit,
    /// 用这个代理地址：`-c http.proxy=<地址>`，优先于 git 自己的配置和环境变量
    Use(String),
    /// 完全不走代理
    Disable,
}

/// SyncDeck 调用的 git 实际要加的代理设置
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProxyPlan {
    pub over: ProxyOverride,
    /// git 配置里单独设了代理的网址（`http.<网址>.proxy` 的 `<网址>`）。按网址的设置比 http.proxy 优先，
    /// 所以 Use / Disable 时对这些网址也要用 `-c http.<网址>.proxy=…` 盖住
    pub url_scopes: Vec<String>,
    /// 不走代理的地址（curl 的 NO_PROXY 格式），只在 Use 时生效；用系统代理时来自系统的绕过列表
    pub no_proxy: Option<String>,
}

impl From<ProxyOverride> for ProxyPlan {
    fn from(over: ProxyOverride) -> Self {
        ProxyPlan { over, ..Default::default() }
    }
}

static PROXY_PLAN: RwLock<ProxyPlan> =
    RwLock::new(ProxyPlan { over: ProxyOverride::Inherit, url_scopes: Vec::new(), no_proxy: None });

pub fn set_proxy_plan(p: ProxyPlan) {
    *PROXY_PLAN.write().unwrap_or_else(|e| e.into_inner()) = p;
}

pub fn proxy_plan() -> ProxyPlan {
    PROXY_PLAN.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 读取 git 代理配置用的正则：`http.proxy` 和按网址设置的 `http.<网址>.proxy`
pub const PROXY_CONFIG_REGEX: &str = r"^http\.(.+\.)?proxy$";

/// git（libcurl）认的全部代理环境变量，"不用代理"时都要去掉。
/// Windows 上环境变量本来不分大小写，两种写法都列出来是为了在别的系统上也成立。
pub const PROXY_ENV_VARS: [&str; 6] = ["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY", "http_proxy", "HTTP_PROXY"];

/// git 对 https 地址认的代理环境变量，按查找顺序排列（小写优先）。
/// HTTP_PROXY / http_proxy 只用于 http 地址，GitHub 这类 https 仓库不会用它，所以不算"git 自己的代理"。
pub const HTTPS_PROXY_ENV_VARS: [&str; 4] = ["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"];

/// 代理设置对应的 git 全局参数（放在子命令前面）
/// - Use：`-c http.proxy=<地址>`，再对每个按网址设置过代理的网址加 `-c http.<网址>.proxy=<地址>`
/// - Disable：同上，值为空（git 约定空值表示不用代理，此时也不再读环境变量）
pub fn proxy_args(p: &ProxyPlan) -> Vec<String> {
    let value = match &p.over {
        ProxyOverride::Inherit => return Vec::new(),
        ProxyOverride::Use(addr) => addr.as_str(),
        ProxyOverride::Disable => "",
    };
    let mut args = vec!["-c".to_string(), format!("http.proxy={value}")];
    for url in &p.url_scopes {
        args.push("-c".to_string());
        args.push(format!("http.{url}.proxy={value}"));
    }
    args
}

/// 代理设置要从子进程环境里去掉的变量：禁用时把环境变量里的代理也去掉，双保险
pub fn proxy_env_removals(p: &ProxyPlan) -> &'static [&'static str] {
    match p.over {
        ProxyOverride::Disable => &PROXY_ENV_VARS,
        _ => &[],
    }
}

/// 代理设置要给子进程设置的环境变量：使用代理且有绕过列表时设置 NO_PROXY（git 在配了 http.proxy 时也会读它）。
/// `existing` 是本进程已有的 NO_PROXY，两者合并，不丢掉用户自己设的。
pub fn proxy_env_sets(p: &ProxyPlan, existing: Option<String>) -> Vec<(&'static str, String)> {
    let (ProxyOverride::Use(_), Some(ours)) = (&p.over, &p.no_proxy) else {
        return Vec::new();
    };
    let merged = match existing.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
        Some(e) => format!("{e},{ours}"),
        None => ours.clone(),
    };
    // Windows 上环境变量不分大小写，两个名字其实是同一个；别的系统上 curl 先看小写的
    vec![("NO_PROXY", merged.clone()), ("no_proxy", merged)]
}

/// 环境变量里给 https 地址用的代理（按 `HTTPS_PROXY_ENV_VARS` 的顺序取第一个非空的）。
/// `get` 一般是 `std::env::var(k).ok()`
pub fn proxy_from_env(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    HTTPS_PROXY_ENV_VARS
        .iter()
        .filter_map(|k| get(k))
        .map(|v| v.trim().to_string())
        .find(|v| !v.is_empty())
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 错误信息最多保留的字符数，避免超长输出撑爆界面
const MAX_MESSAGE_CHARS: usize = 2000;

#[derive(Debug, Clone)]
pub struct GitOutput {
    pub stdout: String,
    #[allow(dead_code)]
    pub stderr: String,
}

#[derive(Debug, Clone)]
pub enum GitError {
    /// 系统里找不到 git
    NotInstalled,
    /// 工作目录不存在
    NoDirectory(String),
    /// 目录还在，但已经不是 Git 仓库（根目录下没有 .git）
    NotRepo(String),
    Timeout { command: String, secs: u64 },
    /// git 返回非 0 退出码；stderr 为原文（已去掉 URL 里的账号信息）。
    /// stdout 也保留着：有的命令用退出码 1 表示"有结果"（如 merge-tree 有冲突时），输出仍然有用
    Failed { command: String, code: Option<i32>, stderr: String, stdout: String },
    Io(String),
    /// 被安全白名单拦截
    Forbidden(String),
}

impl GitError {
    /// 显示给界面的错误原因：git 失败时就是 stderr 原文
    pub fn message(&self) -> String {
        let msg = match self {
            GitError::NotInstalled => {
                "未找到 git 命令，请先安装 Git for Windows：https://git-scm.com/download/win".to_string()
            }
            GitError::NoDirectory(dir) => format!("找不到目录：{dir}"),
            GitError::NotRepo(dir) => format!("这个文件夹已经不是 Git 仓库（找不到 .git）：{dir}，可以在详情里取消监控"),
            GitError::Timeout { command, secs } => format!("git {command} 超时（{secs} 秒），已终止"),
            GitError::Failed { command, code, stderr, .. } => {
                if stderr.trim().is_empty() {
                    let code = code.map_or("未知".to_string(), |c| c.to_string());
                    format!("git {command} 失败（退出码 {code}）")
                } else {
                    stderr.trim().to_string()
                }
            }
            GitError::Io(e) => format!("无法执行 git：{e}"),
            GitError::Forbidden(e) => format!("已拒绝执行不安全的 git 命令：{e}"),
        };
        truncate(&msg, MAX_MESSAGE_CHARS)
    }
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// 不需要在仓库里执行的只读查询：`git --version`，读取全局 / 系统级 git 配置里的代理
/// （`config --global|--system -z --get-regexp <PROXY_CONFIG_REGEX>`）。
/// 只认这几条完整的参数，其余命令都必须在仓库根目录下执行。
fn runs_outside_repo(args: &[&str]) -> bool {
    match args {
        ["--version"] => true,
        ["config", "--global" | "--system", "-z", "--get-regexp", re] => *re == PROXY_CONFIG_REGEX,
        // 克隆：此时 `dir` 是目标的上一级（默认代码目录），目标还不存在
        ["clone", "--progress" | "--quiet", _, _] => true,
        _ => false,
    }
}

/// 要联网、需要加代理设置的不在仓库里执行的命令（只有克隆；读代理配置本身不需要）
fn is_clone(args: &[&str]) -> bool {
    matches!(args, ["clone", ..])
}

/// 克隆的地址：`https://` 开头，或本机绝对路径（`C:\…` / `C:/…`，测试时用本地裸仓库）。
/// 不能有 `::`（禁止 `ext::` 这类传输）、不能以 `-` 开头
fn clone_url_ok(url: &str) -> bool {
    if url.is_empty() || url.starts_with('-') || url.contains("::") {
        return false;
    }
    if let Some(rest) = url.strip_prefix("https://") {
        return !rest.is_empty();
    }
    let b = url.as_bytes();
    b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// 在 `dir` 目录下执行 `git <args>`。
pub async fn run_git(dir: &Path, args: &[&str], timeout: Duration) -> Result<GitOutput, GitError> {
    let (cmd, command) = prepare(dir, args)?;
    let child = spawn(cmd)?;

    // 超时后 future 被丢弃，kill_on_drop 会终止子进程
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Err(_) => Err(GitError::Timeout { command, secs: timeout.as_secs() }),
        Ok(Err(e)) => Err(GitError::Io(e.to_string())),
        Ok(Ok(out)) => {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = redact(&String::from_utf8_lossy(&out.stderr));
            if out.status.success() {
                Ok(GitOutput { stdout, stderr })
            } else {
                Err(GitError::Failed { command, code: out.status.code(), stderr, stdout })
            }
        }
    }
}

/// 和 `run_git` 一样（白名单、不弹窗、超时、代理都相同），但边运行边读 stderr：
/// 带 `--progress` 的 fetch / push / pull / clone 输出的进度行解析成阶段和百分比，交给 `on_progress`
/// （同一阶段 200 毫秒最多一次，见 `Throttle`）。错误信息里去掉进度行，只留真正的提示
pub async fn run_git_progress(
    dir: &Path,
    args: &[&str],
    timeout: Duration,
    on_progress: &(dyn Fn(GitProgress) + Send + Sync),
) -> Result<GitOutput, GitError> {
    let (cmd, command) = prepare(dir, args)?;
    let mut child = spawn(cmd)?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();

    let work = async {
        let read_out = async {
            let mut buf = Vec::new();
            if let Some(s) = stdout.as_mut() {
                s.read_to_end(&mut buf).await?;
            }
            Ok::<_, std::io::Error>(buf)
        };
        let read_err = async {
            let mut reader = ProgressReader::new(on_progress);
            if let Some(s) = stderr.as_mut() {
                let mut chunk = [0u8; 4096];
                loop {
                    let n = s.read(&mut chunk).await?;
                    if n == 0 {
                        break;
                    }
                    reader.feed(&chunk[..n], Instant::now());
                }
            }
            Ok::<_, std::io::Error>(reader.finish())
        };
        let (out, err) = tokio::join!(read_out, read_err);
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, out?, err?))
    };

    // 超时后 future 被丢弃；函数返回时 child 被丢弃，kill_on_drop 会终止子进程
    match tokio::time::timeout(timeout, work).await {
        Err(_) => Err(GitError::Timeout { command, secs: timeout.as_secs() }),
        Ok(Err(e)) => Err(GitError::Io(e.to_string())),
        Ok(Ok((status, out, err))) => {
            let stdout = String::from_utf8_lossy(&out).into_owned();
            let stderr = redact(&err);
            if status.success() {
                Ok(GitOutput { stdout, stderr })
            } else {
                Err(GitError::Failed { command, code: status.code(), stderr, stdout })
            }
        }
    }
}

/// 校验参数、准备好 git 子进程（还没启动）：返回命令和子命令名
fn prepare(dir: &Path, args: &[&str]) -> Result<(Command, String), GitError> {
    validate(args).map_err(GitError::Forbidden)?;
    if !dir.is_dir() {
        return Err(GitError::NoDirectory(dir.display().to_string()));
    }
    // 项目目录必须自己就是仓库根目录：否则 git 会往上层找仓库，
    // 删掉 .git 的项目会被当成上层仓库来检查甚至推送
    let outside_repo = runs_outside_repo(args);
    if !outside_repo && !dir.join(".git").exists() {
        return Err(GitError::NotRepo(dir.display().to_string()));
    }
    let command = args.first().copied().unwrap_or("").to_string();
    // 代理设置只加在仓库里的命令上（读代理配置本身不需要）
    let proxy = if outside_repo && !is_clone(args) { ProxyPlan::default() } else { proxy_plan() };

    let mut cmd = Command::new("git");
    cmd.args(["-c", "core.quotepath=false", "-c", "gc.auto=0", "-c", "maintenance.auto=false"])
        .args(proxy_args(&proxy))
        .args(args)
        .current_dir(dir)
        // 不在终端里提示输入账号密码；Git 凭据管理器也不弹登录窗口，没有已保存的凭据就直接报错
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        // status 不去抢 index.lock，避免和编辑器里的 git 冲突
        .env("GIT_OPTIONAL_LOCKS", "0")
        // 不继承可能指向别处的仓库环境变量
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for var in proxy_env_removals(&proxy) {
        cmd.env_remove(var);
    }
    let existing_no_proxy = std::env::var("no_proxy").or_else(|_| std::env::var("NO_PROXY")).ok();
    for (var, value) in proxy_env_sets(&proxy, existing_no_proxy) {
        cmd.env(var, value);
    }
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    Ok((cmd, command))
}

fn spawn(mut cmd: Command) -> Result<tokio::process::Child, GitError> {
    cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::NotInstalled
        } else {
            GitError::Io(e.to_string())
        }
    })
}

// ---------------- 进度 ----------------

/// 把 git 进度行的标题归到界面上显示的阶段。git 的提示跟随系统语言，英文、简体、繁体都认；不认识的算"准备中"
fn phase_of(title: &str) -> ProgressPhase {
    let t = title.to_lowercase();
    let has = |keys: &[&str]| keys.iter().any(|k| t.contains(k));
    if has(&["writing objects", "写入对象", "寫入物件"]) {
        ProgressPhase::Uploading
    } else if has(&["receiving objects", "unpacking objects", "接收对象", "展开对象", "解包对象", "接收物件", "展開物件"]) {
        ProgressPhase::Downloading
    } else if has(&["resolving deltas", "处理 delta", "處理 delta"]) {
        ProgressPhase::Resolving
    } else if has(&["updating files", "checking out files", "更新文件", "检出文件", "更新檔案"]) {
        ProgressPhase::Checkout
    } else {
        ProgressPhase::Preparing
    }
}

/// 解析一行 git 进度，如 `Writing objects:  45% (9/20), 1.20 MiB | 1.00 MiB/s` 或 `接收对象中: 100% (3/3), 完成.`。
/// 只认"标题: NN% (a/b)"这种格式（标题后面的冒号可以是全角）；`remote:` 开头的是云端那边的统计和压缩，算"准备中"
pub fn parse_progress(line: &str) -> Option<GitProgress> {
    let line = line.trim();
    let (remote, body) = match line.strip_prefix("remote:") {
        Some(rest) => (true, rest.trim_start()),
        None => (false, line),
    };
    let pct = body.find('%')?;
    let head = &body[..pct];
    let digits_from = head
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_ascii_digit())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let percent: u8 = head[digits_from..].parse().ok().filter(|p| *p <= 100)?;
    // 百分比后面必须是 "(已完成/总数)"
    let counter = body[pct + 1..].trim_start().strip_prefix('(')?;
    let (done, rest) = counter.split_once('/')?;
    let total = rest.split(')').next()?;
    if done.is_empty() || total.is_empty() || !done.chars().chain(total.chars()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let title = head[..digits_from].trim_end().trim_end_matches([':', '：']).trim();
    if title.is_empty() {
        return None;
    }
    let phase = if remote { ProgressPhase::Preparing } else { phase_of(title) };
    Some(GitProgress { phase, percent })
}

/// 不算错误原因的进度输出（`Total 3 (delta 0)…`、`… done.` 这类收尾行）
fn is_progress_noise(line: &str) -> bool {
    let body = line.trim();
    let body = body.strip_prefix("remote:").map_or(body, str::trim_start);
    parse_progress(line).is_some()
        || body.ends_with(", done.")
        || body.ends_with("完成.")
        || body.ends_with("完成。")
        || body.starts_with("Total ")
        || body.starts_with("Delta compression using")
}

/// 进度节流：阶段变了、到 100% 时马上报；同一阶段 200 毫秒最多报一次；和上次一样的不报
pub struct Throttle {
    last: Option<(GitProgress, Instant)>,
}

pub const PROGRESS_EVERY: Duration = Duration::from_millis(200);

impl Throttle {
    pub fn new() -> Self {
        Throttle { last: None }
    }

    pub fn admit(&mut self, p: GitProgress, now: Instant) -> bool {
        if let Some((prev, at)) = &self.last {
            if *prev == p {
                return false;
            }
            if prev.phase == p.phase && p.percent < 100 && now.saturating_duration_since(*at) < PROGRESS_EVERY {
                return false;
            }
        }
        self.last = Some((p, now));
        true
    }
}

impl Default for Throttle {
    fn default() -> Self {
        Self::new()
    }
}

/// 一点点读进 stderr：按 `\r` 或 `\n` 切成行（git 用 `\r` 原地刷新进度），进度行解析后上报，其余的留作错误信息
struct ProgressReader<'a> {
    on_progress: &'a (dyn Fn(GitProgress) + Send + Sync),
    pending: Vec<u8>,
    kept: String,
    throttle: Throttle,
}

impl<'a> ProgressReader<'a> {
    fn new(on_progress: &'a (dyn Fn(GitProgress) + Send + Sync)) -> Self {
        ProgressReader { on_progress, pending: Vec::new(), kept: String::new(), throttle: Throttle::new() }
    }

    fn feed(&mut self, bytes: &[u8], now: Instant) {
        self.pending.extend_from_slice(bytes);
        // \r 和 \n 都是 ASCII，在 UTF-8 里不会出现在多字节字符中间，按字节切不会切坏中文
        while let Some(i) = self.pending.iter().position(|b| *b == b'\r' || *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=i).collect();
            self.line(&line[..line.len() - 1], now);
        }
    }

    fn line(&mut self, bytes: &[u8], now: Instant) {
        let line = String::from_utf8_lossy(bytes);
        if let Some(p) = parse_progress(&line) {
            if self.throttle.admit(p, now) {
                (self.on_progress)(p);
            }
        } else if !line.trim().is_empty() && !is_progress_noise(&line) {
            self.kept.push_str(&line);
            self.kept.push('\n');
        }
    }

    fn finish(mut self) -> String {
        let rest = std::mem::take(&mut self.pending);
        if !rest.is_empty() {
            self.line(&rest, Instant::now());
        }
        self.kept
    }
}

/// 任何位置出现都会被拒绝的参数（强制推送、删除、改写历史、执行外部程序等）
const FORBIDDEN_FLAGS: &[&str] = &[
    "--force",
    "-f",
    "--force-with-lease",
    "--force-if-includes",
    "--mirror",
    "--delete",
    "-d",
    "--prune",
    "-p",
    "--prune-tags",
    "-P",
    "--hard",
    "--amend",
    "--rebase",
    "-r",
    "--autostash",
    "--output",
    "--exec",
    "--upload-pack",
    "--receive-pack",
    "--all",
    "--allow-unrelated-histories",
];

/// 预演合并（`git merge-tree`）唯一允许的选项：只在内存里算出合并结果，输出冲突文件名，不碰工作区和分支
pub const MERGE_TREE_FLAGS: [&str; 4] = ["--write-tree", "--name-only", "-z", "--no-messages"];

/// 安全白名单。除了下面列出的子命令和用法，一律拒绝。
/// 刻意不包含 reset、rebase、stash、clean、checkout、restore、switch、rm、gc、branch -D 等命令。
pub fn validate(args: &[&str]) -> Result<(), String> {
    let Some((&sub, rest)) = args.split_first() else {
        return Err("缺少 git 子命令".into());
    };

    let mut positional: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let a = rest[i];
        if a == "-m" && (sub == "commit" || sub == "merge") {
            i += 2; // 提交说明原样交给 git，不参与检查
            continue;
        }
        if a.starts_with('-') {
            let bad = FORBIDDEN_FLAGS.iter().any(|f| a == *f || a.starts_with(&format!("{f}=")));
            if bad {
                return Err(format!("git {sub} 不允许使用参数 {a}"));
            }
        } else {
            positional.push(a);
        }
        i += 1;
    }

    match sub {
        "--version" | "status" | "rev-parse" | "log" => Ok(()),
        "fetch" => {
            if positional.iter().any(|p| p.starts_with('+')) {
                return Err("fetch 不允许强制更新的 refspec".into());
            }
            Ok(())
        }
        "push" => {
            if positional.len() != 2 {
                return Err("push 必须显式指定远程和分支".into());
            }
            let refspec = positional[1];
            if refspec.starts_with('+') || refspec.starts_with(':') || refspec.ends_with(':') {
                return Err(format!("push 不允许强制推送或删除远程分支：{refspec}"));
            }
            Ok(())
        }
        "pull" => {
            if rest.contains(&"--ff-only") && rest.contains(&"--no-rebase") {
                Ok(())
            } else {
                Err("pull 只允许 --ff-only --no-rebase（只快进，不合并、不变基）".into())
            }
        }
        // 只允许 clone --progress|--quiet <地址> <目标路径>（见 clone_url_ok）。
        // 不许 --config、-c、--upload-pack、--template、--depth、--recurse-submodules、--mirror、--bare、-b 等其他参数
        "clone" => match rest {
            ["--progress" | "--quiet", url, target] if clone_url_ok(url) && !target.is_empty() && !target.starts_with('-') => {
                Ok(())
            }
            _ => Err("clone 只允许 clone --progress <https 地址或本机路径> <目标路径>".into()),
        },
        "add" => {
            if rest == ["-A"] {
                Ok(())
            } else {
                Err("add 只允许 add -A（仅在用户确认的\"提交并推送\"里使用）".into())
            }
        }
        "commit" => {
            // 只接受 [--quiet] -m <说明>
            let mut has_message = false;
            let mut j = 0;
            while j < rest.len() {
                match rest[j] {
                    "-m" if j + 1 < rest.len() => {
                        has_message = true;
                        j += 2;
                    }
                    "--quiet" => j += 1,
                    _ => return Err("commit 只允许 commit -m <说明>".into()),
                }
            }
            if has_message {
                Ok(())
            } else {
                Err("commit 必须带提交说明".into())
            }
        }
        "config" => {
            if rest.contains(&"--get") || rest.contains(&"--get-regexp") {
                Ok(())
            } else {
                Err("config 只允许读取（--get / --get-regexp）".into())
            }
        }
        "symbolic-ref" => {
            if rest == ["--short", "-q", "HEAD"] {
                Ok(())
            } else {
                Err("symbolic-ref 只允许读取当前分支".into())
            }
        }
        "remote" => {
            if positional.first() == Some(&"get-url") {
                Ok(())
            } else {
                Err("remote 只允许 get-url".into())
            }
        }
        // 只允许 merge-tree --write-tree --name-only -z --no-messages <本地> <云端>（预演合并，不碰工作区）
        "merge-tree" => match rest {
            [a, b, c, d, ours, theirs]
                if [*a, *b, *c, *d] == MERGE_TREE_FLAGS && !ours.starts_with('-') && !theirs.starts_with('-') =>
            {
                Ok(())
            }
            _ => Err("merge-tree 只允许 merge-tree --write-tree --name-only -z --no-messages <本地> <云端>".into()),
        },
        // 只允许两种：merge --no-edit -m <说明> <上游提交>（预演无冲突后的正式合并），merge --abort（合并失败时撤销）。
        // 不许 --squash、-s、-X、--allow-unrelated-histories、--continue 等其他选项，也不许一次合并多个提交
        "merge" => match rest {
            ["--abort"] => Ok(()),
            ["--no-edit", "-m", _, target] if !target.starts_with('-') => Ok(()),
            _ => Err("merge 只允许 merge --no-edit -m <说明> <上游提交> 或 merge --abort".into()),
        },
        other => Err(format!("不允许的 git 子命令：{other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::validate;
    use std::time::Duration;

    #[test]
    fn refresh_uses_shorter_timeout() {
        let secs = |s| Duration::from_secs(s);
        assert_eq!(super::refresh_timeout_for(secs(180)), secs(30));
        assert_eq!(super::refresh_timeout_for(secs(600)), secs(30));
        assert_eq!(super::refresh_timeout_for(secs(30)), secs(30));
        assert_eq!(super::refresh_timeout_for(secs(10)), secs(10), "联网超时比 30 秒还短时用联网超时");
    }

    #[test]
    fn allows_needed_commands() {
        assert!(validate(&["status", "--porcelain=v2", "--branch", "-z"]).is_ok());
        assert!(validate(&["fetch", "--quiet", "origin"]).is_ok());
        assert!(validate(&["push", "--quiet", "origin", "refs/heads/main:refs/heads/main"]).is_ok());
        assert!(validate(&["pull", "--ff-only", "--no-rebase", "--no-autostash", "--quiet", "origin", "main"]).is_ok());
        assert!(validate(&["add", "-A"]).is_ok());
        assert!(validate(&["commit", "--quiet", "-m", "修复 --force 和 reset 的说明文字"]).is_ok());
        assert!(validate(&["config", "-z", "--get-regexp", "^remote"]).is_ok());
        assert!(validate(&["symbolic-ref", "--short", "-q", "HEAD"]).is_ok());
        assert!(validate(&["log", "-3", "--format=%h"]).is_ok());
        assert!(validate(&["--version"]).is_ok());
    }

    #[test]
    fn rejects_dangerous_commands() {
        for args in [
            vec!["reset", "--hard"],
            vec!["rebase", "main"],
            vec!["stash"],
            vec!["clean", "-fd"],
            vec!["checkout", "--", "."],
            vec!["restore", "."],
            vec!["switch", "main"],
            vec!["rm", "a.txt"],
            vec!["gc"],
            vec!["branch", "-D", "x"],
            vec!["push", "--force", "origin", "main:main"],
            vec!["push", "-f", "origin", "main:main"],
            vec!["push", "--force-with-lease=main", "origin", "main:main"],
            vec!["push", "origin", "+main:main"],
            vec!["push", "origin", ":main"],
            vec!["push", "--delete", "origin", "main"],
            vec!["push", "origin"],
            vec!["pull", "origin", "main"],
            vec!["pull", "--rebase", "--ff-only", "--no-rebase"],
            vec!["fetch", "--prune", "origin"],
            vec!["fetch", "origin", "+refs/heads/*:refs/remotes/origin/*"],
            vec!["add", "."],
            vec!["commit", "--amend", "-m", "x"],
            vec!["commit", "-a", "-m", "x"],
            vec!["config", "user.name", "x"],
            vec!["symbolic-ref", "HEAD", "refs/heads/x"],
            vec!["remote", "add", "x", "y"],
            vec!["log", "--output=x.txt"],
        ] {
            assert!(validate(&args).is_err(), "应当拒绝：{args:?}");
        }
    }

    /// 显示进度要加 --progress：白名单允许，原来禁止的照样禁止
    #[test]
    fn progress_flag_allowed() {
        assert!(validate(&["fetch", "--progress", "origin", "main"]).is_ok());
        assert!(validate(&["push", "--progress", "origin", "refs/heads/main:refs/heads/main"]).is_ok());
        assert!(validate(&["pull", "--ff-only", "--no-rebase", "--no-autostash", "--progress", "origin", "main"]).is_ok());
        for args in [
            vec!["push", "--progress", "--force", "origin", "main:main"],
            vec!["push", "--progress", "origin", "+main:main"],
            vec!["push", "--progress", "origin"],
            vec!["fetch", "--progress", "--prune", "origin"],
            vec!["pull", "--progress", "origin", "main"],
            vec!["pull", "--progress", "--rebase", "--ff-only", "--no-rebase"],
        ] {
            assert!(validate(&args).is_err(), "应当拒绝：{args:?}");
        }
    }

    /// 克隆只允许两种完整写法，地址只能是 https 或本机绝对路径
    #[test]
    fn clone_whitelist() {
        use super::runs_outside_repo;
        for args in [
            vec!["clone", "--progress", "https://github.com/Owenwoow/Git-SyncDeck.git", r"E:\Code\Git-SyncDeck"],
            vec!["clone", "--quiet", "https://github.com/a/b.git", r"E:\Code\b"],
            vec!["clone", "--progress", r"C:\Temp\remotes\synced.git", r"C:\Temp\code\synced"],
            vec!["clone", "--quiet", "C:/Temp/remotes/synced.git", "C:/Temp/code/synced"],
        ] {
            assert!(validate(&args).is_ok(), "应当允许：{args:?}");
            assert!(runs_outside_repo(&args), "克隆不需要仓库目录：{args:?}");
        }
        for args in [
            // 其他参数
            vec!["clone", "https://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "--depth", "1", "https://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "--depth=1", "https://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "--shallow-since=2020-01-01", "https://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "--config", "core.sshCommand=calc", "https://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "-c", "core.fsmonitor=calc", "https://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "--upload-pack=calc", "https://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "-u", "calc", "https://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "--template=x", "https://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "--separate-git-dir=x", "https://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "--recurse-submodules", "https://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "--mirror", "https://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "--bare", "https://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "-b", "dev", "https://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "--quiet", "https://github.com/a/b.git", r"E:\b"],
            // 缺参数 / 多参数
            vec!["clone", "--progress"],
            vec!["clone", "--progress", "https://github.com/a/b.git"],
            vec!["clone", "--progress", "https://github.com/a/b.git", r"E:\b", "extra"],
            vec!["clone"],
            // 地址：不是 https 或本机绝对路径、传输方式、以 - 开头
            vec!["clone", "--progress", "http://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "git@github.com:a/b.git", r"E:\b"],
            vec!["clone", "--progress", "ssh://git@github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "git://github.com/a/b.git", r"E:\b"],
            vec!["clone", "--progress", "file:///C:/x.git", r"E:\b"],
            vec!["clone", "--progress", "ext::sh -c calc", r"E:\b"],
            vec!["clone", "--progress", "https://x::y/a.git", r"E:\b"],
            vec!["clone", "--progress", "fd::3", r"E:\b"],
            vec!["clone", "--progress", "https://", r"E:\b"],
            vec!["clone", "--progress", "", r"E:\b"],
            vec!["clone", "--progress", r"relative\path.git", r"E:\b"],
            vec!["clone", "--progress", r"\\server\share\a.git", r"E:\b"],
            vec!["clone", "--progress", "C:rel.git", r"E:\b"],
            vec!["clone", "--progress", "-x", r"E:\b"],
            vec!["clone", "--progress", "--upload-pack", r"E:\b"],
            // 目标：不能为空、不能以 - 开头
            vec!["clone", "--progress", "https://github.com/a/b.git", ""],
            vec!["clone", "--progress", "https://github.com/a/b.git", "-b"],
            vec!["clone", "--progress", "https://github.com/a/b.git", "--bare"],
        ] {
            assert!(validate(&args).is_err(), "应当拒绝：{args:?}");
            assert!(!runs_outside_repo(&args) || args.len() == 4, "只有完整的两种写法才不需要仓库目录：{args:?}");
        }
        // 克隆要加代理，读代理配置和 --version 不加
        assert!(super::is_clone(&["clone", "--progress", "https://github.com/a/b.git", r"E:\b"]));
        assert!(!super::is_clone(&["--version"]));
    }

    #[test]
    fn parses_progress_lines() {
        use super::parse_progress;
        use crate::model::{GitProgress, ProgressPhase::*};
        let p = |phase, percent| Some(GitProgress { phase, percent });
        // 英文
        assert_eq!(parse_progress("Writing objects:  45% (9/20), 1.20 MiB | 1.00 MiB/s"), p(Uploading, 45));
        assert_eq!(parse_progress("Writing objects: 100% (20/20), 2.1 KiB | 2.1 MiB/s, done."), p(Uploading, 100));
        assert_eq!(parse_progress("Receiving objects:   3% (1/30)"), p(Downloading, 3));
        assert_eq!(parse_progress("Unpacking objects: 100% (3/3), 250 bytes | 25.00 KiB/s, done."), p(Downloading, 100));
        assert_eq!(parse_progress("Resolving deltas:  50% (1/2)"), p(Resolving, 50));
        assert_eq!(parse_progress("Updating files:  12% (100/800)"), p(Checkout, 12));
        assert_eq!(parse_progress("Compressing objects:  66% (2/3)"), p(Preparing, 66));
        assert_eq!(parse_progress("Counting objects: 100% (5/5), done."), p(Preparing, 100));
        // 云端那边的输出都算准备中
        assert_eq!(parse_progress("remote: Counting objects:  50% (2/4)"), p(Preparing, 50));
        assert_eq!(parse_progress("remote: Compressing objects: 100% (3/3), done.        "), p(Preparing, 100));
        // 简体、繁体中文（冒号可能是全角）
        assert_eq!(parse_progress("写入对象中:  45% (9/20), 1.20 MiB | 1.00 MiB/s"), p(Uploading, 45));
        assert_eq!(parse_progress("接收对象中: 100% (30/30), 完成."), p(Downloading, 100));
        assert_eq!(parse_progress("展开对象中:  33% (1/3)"), p(Downloading, 33));
        assert_eq!(parse_progress("处理 delta 中: 100% (2/2), 完成."), p(Resolving, 100));
        assert_eq!(parse_progress("更新文件中：  7% (7/100)"), p(Checkout, 7));
        assert_eq!(parse_progress("压缩对象中: 100% (3/3), 完成."), p(Preparing, 100));
        assert_eq!(parse_progress("寫入物件中:  80% (8/10)"), p(Uploading, 80));
        assert_eq!(parse_progress("接收物件中:  10% (1/10)"), p(Downloading, 10));
        // 不是进度
        for line in [
            "",
            "To D:\\remotes\\a.git",
            "   abc123..def456  main -> main",
            "Enumerating objects: 5, done.",
            "Total 3 (delta 0), reused 0 (delta 0), pack-reused 0",
            "error: failed to push some refs to 'x'",
            "hint: 100% sure (not a counter)",
            "Writing objects: 145% (1/1)",
            "Writing objects: 45%",
            ": 45% (1/2)",
        ] {
            assert_eq!(parse_progress(line), None, "{line:?}");
        }
    }

    /// 按 \r、\n 切行（一次读到的数据可能在行中间、甚至中文字符中间断开），错误信息里不留进度行
    #[test]
    fn progress_reader_splits_cr_and_keeps_errors() {
        use super::ProgressReader;
        use crate::model::ProgressPhase;
        use std::sync::Mutex;
        let seen = Mutex::new(Vec::new());
        let on = |p: crate::model::GitProgress| seen.lock().unwrap().push((p.phase, p.percent));
        let mut r = ProgressReader::new(&on);
        let t0 = std::time::Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let text = "Enumerating objects: 5, done.\nCounting objects: 100% (5/5), done.\n写入对象中:  10% (1/10)\r写入对象中:  20% (2/10)\r";
        let bytes = text.as_bytes();
        // 从"写"字的中间切开（"写"占 3 个字节）
        let cut = text.find("写入").unwrap() + 1;
        r.feed(&bytes[..cut], ms(0));
        r.feed(&bytes[cut..], ms(0));
        r.feed("写入对象中:  60% (6/10)\r".as_bytes(), ms(250));
        r.feed("写入对象中: 100% (10/10), 完成.\nerror: failed to push some refs to 'D:\\x.git'\nhint: Updates were rejected".as_bytes(), ms(260));
        let kept = r.finish();
        // 20% 和 10% 同一时刻：节流掉；60% 隔了 250 毫秒；100% 总是报
        assert_eq!(
            *seen.lock().unwrap(),
            [(ProgressPhase::Preparing, 100), (ProgressPhase::Uploading, 10), (ProgressPhase::Uploading, 60), (ProgressPhase::Uploading, 100)]
        );
        assert_eq!(kept, "error: failed to push some refs to 'D:\\x.git'\nhint: Updates were rejected\n");
    }

    #[test]
    fn throttle_rules() {
        use super::Throttle;
        use crate::model::{GitProgress, ProgressPhase::*};
        let g = |phase, percent| GitProgress { phase, percent };
        let t0 = std::time::Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut t = Throttle::new();
        assert!(t.admit(g(Downloading, 1), ms(0)));
        assert!(!t.admit(g(Downloading, 2), ms(100)), "同一阶段 200 毫秒内不报");
        assert!(t.admit(g(Downloading, 3), ms(200)));
        assert!(!t.admit(g(Downloading, 3), ms(900)), "和上次一样不报");
        assert!(t.admit(g(Downloading, 100), ms(901)), "100% 马上报");
        assert!(t.admit(g(Resolving, 0), ms(902)), "换阶段马上报");
    }

    /// 预演合并和正式合并只允许固定的几种写法
    #[test]
    fn merge_whitelist() {
        let flags = super::MERGE_TREE_FLAGS;
        let preview = |ours: &'static str, theirs: &'static str| {
            let mut v = vec!["merge-tree"];
            v.extend(flags);
            v.extend([ours, theirs]);
            v
        };
        for args in [
            preview("HEAD", "abc123"),
            preview("0123abcd", "refs/remotes/origin/main"),
            vec!["merge", "--no-edit", "-m", "自动合并：来自 DESKTOP-1", "abc123"],
            // 提交说明里出现的危险字样不算参数
            vec!["merge", "--no-edit", "-m", "--force -s ours --squash", "abc123"],
            vec!["merge", "--abort"],
        ] {
            assert!(validate(&args).is_ok(), "应当允许：{args:?}");
        }
        for args in [
            // merge-tree：旧的三参数用法、缺选项、多选项、选项顺序不对、参数个数不对
            vec!["merge-tree", "base", "HEAD", "abc"],
            vec!["merge-tree", "--write-tree", "HEAD", "abc"],
            vec!["merge-tree", "--write-tree", "--name-only", "HEAD", "abc"],
            vec!["merge-tree", "--name-only", "--write-tree", "-z", "--no-messages", "HEAD", "abc"],
            vec!["merge-tree", "--write-tree", "--name-only", "-z", "--no-messages", "HEAD"],
            vec!["merge-tree", "--write-tree", "--name-only", "-z", "--no-messages", "HEAD", "abc", "def"],
            vec!["merge-tree", "--write-tree", "--name-only", "-z", "--no-messages", "--merge-base=x", "HEAD", "abc"],
            vec!["merge-tree", "--write-tree", "--name-only", "-z", "--no-messages", "HEAD", "--stdin"],
            vec!["merge-tree", "--write-tree", "--name-only", "-z", "--no-messages", "--allow-unrelated-histories", "HEAD"],
            vec!["merge-tree", "--stdin"],
            // merge：没有 --no-edit / -m、合并策略、压缩合并、无关历史、一次合并多个、继续 / 退出
            vec!["merge", "abc123"],
            vec!["merge", "--no-edit", "abc123"],
            vec!["merge", "-m", "x", "abc123"],
            vec!["merge", "--no-edit", "-m", "x"],
            vec!["merge", "--no-edit", "-m", "x", "abc", "def"],
            vec!["merge", "--squash", "abc123"],
            vec!["merge", "--no-edit", "-m", "x", "--squash"],
            vec!["merge", "--no-edit", "-m", "x", "-s", "ours", "abc123"],
            vec!["merge", "-s", "ours", "--no-edit", "-m", "x", "abc123"],
            vec!["merge", "--no-edit", "-m", "x", "-Xtheirs", "abc123"],
            vec!["merge", "-X", "theirs", "--no-edit", "-m", "x", "abc"],
            vec!["merge", "--no-edit", "-m", "x", "--allow-unrelated-histories", "abc123"],
            vec!["merge", "--allow-unrelated-histories", "--no-edit", "-m", "x", "abc"],
            vec!["merge", "--no-ff", "--no-edit", "-m", "x", "abc123"],
            vec!["merge", "--continue"],
            vec!["merge", "--quit"],
            vec!["merge", "--abort", "x"],
            vec!["merge"],
        ] {
            assert!(validate(&args).is_err(), "应当拒绝：{args:?}");
        }
    }

    #[test]
    fn proxy_override_args() {
        use super::{proxy_args, proxy_env_removals, ProxyOverride, ProxyPlan, PROXY_ENV_VARS};
        let plan = |over: ProxyOverride, urls: &[&str]| ProxyPlan {
            over,
            url_scopes: urls.iter().map(|u| u.to_string()).collect(),
            no_proxy: None,
        };
        let urls = ["https://github.com", "https://GitLab.Corp"];

        // 不覆盖：按网址的设置也不动
        let inherit = plan(ProxyOverride::Inherit, &urls);
        assert!(proxy_args(&inherit).is_empty());
        assert!(proxy_env_removals(&inherit).is_empty());

        let use_it: ProxyPlan = ProxyOverride::Use("socks5://127.0.0.1:10808".into()).into();
        assert_eq!(proxy_args(&use_it), ["-c", "http.proxy=socks5://127.0.0.1:10808"]);
        assert!(proxy_env_removals(&use_it).is_empty(), "-c 已经优先于环境变量，不用动环境");
        // 按网址设置过代理的网址也要盖住（它们比 http.proxy 优先），网址大小写原样保留
        assert_eq!(
            proxy_args(&plan(ProxyOverride::Use("http://127.0.0.1:7890".into()), &urls)),
            [
                "-c",
                "http.proxy=http://127.0.0.1:7890",
                "-c",
                "http.https://github.com.proxy=http://127.0.0.1:7890",
                "-c",
                "http.https://GitLab.Corp.proxy=http://127.0.0.1:7890",
            ]
        );

        let off: ProxyPlan = ProxyOverride::Disable.into();
        assert_eq!(proxy_args(&off), ["-c", "http.proxy="], "空值表示不用代理");
        assert_eq!(
            proxy_args(&plan(ProxyOverride::Disable, &urls[..1])),
            ["-c", "http.proxy=", "-c", "http.https://github.com.proxy="]
        );
        let removed = proxy_env_removals(&off);
        for var in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
            assert!(removed.contains(&var), "禁用代理时应去掉 {var}");
        }
        assert_eq!(removed.len(), PROXY_ENV_VARS.len());
    }

    #[test]
    fn no_proxy_env_only_with_system_bypass() {
        use super::{proxy_env_sets, ProxyOverride, ProxyPlan};
        let with = |over: ProxyOverride, no_proxy: Option<&str>| ProxyPlan {
            over,
            url_scopes: Vec::new(),
            no_proxy: no_proxy.map(str::to_string),
        };
        let use_sys = with(ProxyOverride::Use("http://127.0.0.1:10808".into()), Some("localhost,192.168.0.0/16"));
        assert_eq!(
            proxy_env_sets(&use_sys, None),
            [("NO_PROXY", "localhost,192.168.0.0/16".to_string()), ("no_proxy", "localhost,192.168.0.0/16".to_string())]
        );
        // 用户自己设的 NO_PROXY 不丢
        assert_eq!(
            proxy_env_sets(&use_sys, Some("git.corp".into()))[0],
            ("NO_PROXY", "git.corp,localhost,192.168.0.0/16".to_string())
        );
        assert_eq!(proxy_env_sets(&use_sys, Some("  ".into()))[0].1, "localhost,192.168.0.0/16");
        // 没有绕过列表、不覆盖、禁用时都不设
        assert!(proxy_env_sets(&with(ProxyOverride::Use("http://h:1".into()), None), None).is_empty());
        assert!(proxy_env_sets(&with(ProxyOverride::Inherit, Some("localhost")), None).is_empty());
        assert!(proxy_env_sets(&with(ProxyOverride::Disable, Some("localhost")), None).is_empty());
    }

    #[test]
    fn proxy_from_env_order() {
        use super::proxy_from_env;
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
        };
        assert_eq!(proxy_from_env(env(&[])), None);
        assert_eq!(proxy_from_env(env(&[("HTTPS_PROXY", "  ")])), None, "空值当作没设置");
        // HTTP_PROXY / http_proxy 只用于 http 地址，git 访问 https 仓库时不用它
        assert_eq!(proxy_from_env(env(&[("http_proxy", "http://h:1")])), None);
        assert_eq!(proxy_from_env(env(&[("HTTP_PROXY", "http://h:1"), ("http_proxy", "http://h:1")])), None);
        assert_eq!(proxy_from_env(env(&[("HTTPS_PROXY", "http://s:3")])).as_deref(), Some("http://s:3"));
        assert_eq!(proxy_from_env(env(&[("all_proxy", "socks5://a:2")])).as_deref(), Some("socks5://a:2"));
        assert_eq!(
            proxy_from_env(env(&[("HTTP_PROXY", "http://h:1"), ("ALL_PROXY", "socks5://a:2")])).as_deref(),
            Some("socks5://a:2"),
            "https 地址的仓库先看 https_proxy，再看 all_proxy"
        );
        assert_eq!(
            proxy_from_env(env(&[("ALL_PROXY", "socks5://a:2"), ("https_proxy", "http://s:3")])).as_deref(),
            Some("http://s:3")
        );
    }

    /// 白名单里不需要仓库的例外只有这几条完整命令
    #[test]
    fn outside_repo_queries_are_narrow() {
        use super::{runs_outside_repo, PROXY_CONFIG_REGEX};
        assert_eq!(PROXY_CONFIG_REGEX, r"^http\.(.+\.)?proxy$");
        for args in [
            vec!["--version"],
            vec!["config", "--global", "-z", "--get-regexp", PROXY_CONFIG_REGEX],
            vec!["config", "--system", "-z", "--get-regexp", PROXY_CONFIG_REGEX],
        ] {
            assert!(runs_outside_repo(&args), "{args:?}");
            assert!(validate(&args).is_ok(), "{args:?}");
        }
        for args in [
            // 不带范围会读到所在目录的仓库配置
            vec!["config", "-z", "--get-regexp", PROXY_CONFIG_REGEX],
            vec!["config", "--local", "-z", "--get-regexp", PROXY_CONFIG_REGEX],
            vec!["config", "--global", "--get-regexp", PROXY_CONFIG_REGEX],
            vec!["config", "--global", "-z", "--get-regexp", "http"],
            vec!["config", "--global", "-z", "--get-regexp", ".*"],
            vec!["config", "--global", "-z", "--get-regexp", PROXY_CONFIG_REGEX, "x"],
            vec!["config", "--global", "--get", "http.proxy"],
            vec!["config", "--global", "--get", "user.name"],
            vec!["config", "--global", "http.proxy", "x"],
            vec!["status"],
            vec!["fetch", "--quiet", "origin"],
            vec!["--version", "x"],
        ] {
            assert!(!runs_outside_repo(&args), "不应在仓库外执行：{args:?}");
        }
        // 写配置仍然被白名单拒绝
        assert!(validate(&["config", "--global", "http.proxy", "x"]).is_err());
    }

    /// 读全局代理配置不需要仓库（没配置时 git 返回退出码 1，也不能是"不是仓库"或"被拒绝"）
    #[tokio::test]
    async fn reads_global_proxy_outside_repo() {
        let dir = std::env::temp_dir();
        let args = ["config", "--global", "-z", "--get-regexp", super::PROXY_CONFIG_REGEX];
        match super::run_git(&dir, &args, std::time::Duration::from_secs(10)).await {
            Ok(_) | Err(super::GitError::Failed { code: Some(1), .. }) => {}
            Err(e) => panic!("{e:?}"),
        }
    }

    /// 删掉了 .git 的项目目录放在另一个仓库里面时，不能让 git 往上找到外层仓库
    #[tokio::test]
    async fn refuses_dir_without_own_git() {
        let outer = std::env::temp_dir().join(format!("syncdeck-notrepo-{}", std::process::id()));
        let inner = outer.join("skill");
        std::fs::create_dir_all(inner.join("src")).unwrap();
        std::fs::create_dir_all(outer.join(".git")).unwrap();
        let err = super::run_git(&inner, &["status", "--porcelain=v2"], std::time::Duration::from_secs(10))
            .await
            .unwrap_err();
        assert!(matches!(err, super::GitError::NotRepo(_)), "{err:?}");
        assert!(super::run_git(&inner, &["--version"], std::time::Duration::from_secs(10)).await.is_ok());
        let _ = std::fs::remove_dir_all(&outer);
    }
}
