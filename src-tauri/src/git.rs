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
use std::time::Duration;

use tokio::process::Command;

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
}

impl From<ProxyOverride> for ProxyPlan {
    fn from(over: ProxyOverride) -> Self {
        ProxyPlan { over, ..Default::default() }
    }
}

static PROXY_PLAN: RwLock<ProxyPlan> = RwLock::new(ProxyPlan { over: ProxyOverride::Inherit, url_scopes: Vec::new() });

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
    /// git 返回非 0 退出码；stderr 为原文（已去掉 URL 里的账号信息）
    Failed { command: String, code: Option<i32>, stderr: String },
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
            GitError::Failed { command, code, stderr } => {
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
        _ => false,
    }
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
    let proxy = if outside_repo { ProxyPlan::default() } else { proxy_plan() };

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
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);

    let child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            GitError::NotInstalled
        } else {
            GitError::Io(e.to_string())
        }
    })?;

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
                Err(GitError::Failed { command, code: out.status.code(), stderr })
            }
        }
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
        if a == "-m" && sub == "commit" {
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

    #[test]
    fn proxy_override_args() {
        use super::{proxy_args, proxy_env_removals, ProxyOverride, ProxyPlan, PROXY_ENV_VARS};
        let plan = |over: ProxyOverride, urls: &[&str]| ProxyPlan {
            over,
            url_scopes: urls.iter().map(|u| u.to_string()).collect(),
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
