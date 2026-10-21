//! 从 GitHub 列出自己的仓库，供「添加项目 → 从 GitHub 克隆」使用。
//!
//! - 地址解析：账号主页、用户名、单个仓库地址（`parse_source`）
//! - 私有仓库靠 GitHub CLI（gh）列出：SyncDeck 不碰令牌，只调用 gh；gh 的子进程只能从 `run_gh` 创建，有命令白名单
//! - 没装 gh / 没登录 / gh 失败：退到 GitHub 公开接口（不带任何凭据，只能看到公开仓库），用 Windows 自带的 curl.exe 请求
//! - 两种输出（gh 的 JSON、公开接口的 JSON）都转成 `GithubRepo`

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::Value;
use tokio::process::Command;

use crate::git::{self, ProxyOverride, ProxyPlan};
use crate::model::GithubRepo;
use crate::url::redact;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// gh、curl 的超时：状态查询超时的 3 倍（30 秒）
pub fn tool_timeout() -> Duration {
    git::STATUS_TIMEOUT * 3
}

// ---------------- 地址解析 ----------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// 账号（用户或组织）：列出名下的仓库
    Owner(String),
    /// 单个仓库
    Repo { owner: String, name: String },
}

impl Source {
    pub fn owner(&self) -> &str {
        match self {
            Source::Owner(o) => o,
            Source::Repo { owner, .. } => owner,
        }
    }
}

/// GitHub 用户名：字母数字和 `-`，不以 `-` 开头，最长 39 个字符
pub fn valid_owner(s: &str) -> bool {
    !s.is_empty() && s.len() <= 39 && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// 仓库名：字母数字和 `-` `_` `.`，不能是 `.` / `..`，最长 100 个字符
pub fn valid_repo_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn owner_checked(s: &str) -> Result<String, String> {
    if valid_owner(s) {
        Ok(s.to_string())
    } else {
        Err(format!("GitHub 用户名只能含字母、数字和连字符（-），且不能以 - 开头：{s}"))
    }
}

fn repo_checked(s: &str) -> Result<String, String> {
    let name = s.strip_suffix(".git").unwrap_or(s);
    if valid_repo_name(name) {
        Ok(name.to_string())
    } else {
        Err(format!("仓库名只能含字母、数字和 - _ .：{s}"))
    }
}

/// 解析用户填的 GitHub 地址：
/// 账号主页（`https://github.com/用户名`、`github.com/用户名`、末尾带 `/`）、纯用户名、
/// 单个仓库（`https://github.com/用户/仓库`、带 `.git`、`git@github.com:用户/仓库.git`）
pub fn parse_source(input: &str) -> Result<Source, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("请填写 GitHub 账号主页、用户名或仓库地址".into());
    }
    let lower = s.to_ascii_lowercase();

    let path: &str = if lower.starts_with("git@github.com:") {
        &s["git@github.com:".len()..]
    } else {
        let (had_scheme, rest) = match lower.find("://") {
            Some(i) => {
                let scheme = &lower[..i];
                if scheme != "https" && scheme != "http" {
                    return Err(format!("只支持 https://github.com/… 形式的地址：{s}"));
                }
                (true, &s[i + 3..])
            }
            None => (false, s),
        };
        let rest_lower = rest.to_ascii_lowercase();
        let stripped = ["www.github.com/", "github.com/"]
            .iter()
            .find_map(|p| rest_lower.starts_with(p).then(|| &rest[p.len()..]));
        match stripped {
            Some(p) => p,
            None if rest_lower == "github.com" || rest_lower == "www.github.com" => {
                return Err("地址里缺少用户名，例如 https://github.com/用户名".into());
            }
            None if had_scheme => return Err(format!("只支持 github.com 的地址：{s}")),
            None => rest,
        }
    };

    // 去掉 ? # 之后的部分和首尾的 /
    let path = path.split(['?', '#']).next().unwrap_or("").trim_matches('/');
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        [owner] => Ok(Source::Owner(owner_checked(owner)?)),
        [owner, name] => Ok(Source::Repo { owner: owner_checked(owner)?, name: repo_checked(name)? }),
        _ => Err(format!("看不懂这个地址，请填账号主页（github.com/用户名）或仓库地址（github.com/用户名/仓库）：{s}")),
    }
}

// ---------------- 子进程（gh、curl） ----------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// 系统里找不到这个程序
    NotInstalled,
    Timeout(u64),
    Failed(String),
    /// 被白名单拦截
    Forbidden(String),
}

impl ToolError {
    pub fn message(&self) -> String {
        match self {
            ToolError::NotInstalled => "未找到命令".into(),
            ToolError::Timeout(secs) => format!("超时（{secs} 秒），已终止"),
            ToolError::Failed(e) => e.clone(),
            ToolError::Forbidden(e) => format!("已拒绝执行不安全的命令：{e}"),
        }
    }
}

/// gh `repo list` / `repo view` 要的字段（固定）
pub const GH_FIELDS: &str = "name,nameWithOwner,url,isPrivate,isFork,isArchived,pushedAt,description";

/// gh 命令白名单：只允许三种完整写法
/// - `api user --jq .login`：读当前登录的账号
/// - `repo list <账号> --limit 1000 --json <固定字段>`
/// - `repo view <账号/仓库> --json <固定字段>`
///
/// 账号和仓库参数不能以 `-` 开头。auth、setup-git、repo clone、api 的别的路径等一律拒绝
pub fn validate_gh(args: &[&str]) -> Result<(), String> {
    match args {
        ["api", "user", "--jq", ".login"] => Ok(()),
        ["repo", "list", owner, "--limit", "1000", "--json", fields] if *fields == GH_FIELDS && valid_owner(owner) => {
            Ok(())
        }
        ["repo", "view", repo, "--json", fields] if *fields == GH_FIELDS && valid_full_name(repo) => Ok(()),
        _ => Err("gh 只允许 api user --jq .login、repo list <账号> --limit 1000 --json …、repo view <账号/仓库> --json …".into()),
    }
}

/// `账号/仓库`，两段都合法（因此不会以 `-` 开头）
fn valid_full_name(s: &str) -> bool {
    matches!(s.split_once('/'), Some((o, n)) if valid_owner(o) && valid_repo_name(n))
}

/// 代理设置对 gh / curl 子进程环境变量的影响：(要设置的, 要去掉的)。
/// - `Use`：`HTTPS_PROXY` / `HTTP_PROXY` 设为该地址；有绕过列表时设 `NO_PROXY`（和本进程已有的合并）
/// - `Disable`：去掉所有代理环境变量
/// - `Inherit`：不动
pub fn proxy_env(plan: &ProxyPlan, existing_no_proxy: Option<String>) -> (Vec<(&'static str, String)>, Vec<&'static str>) {
    let mut sets = Vec::new();
    if let ProxyOverride::Use(addr) = &plan.over {
        sets.push(("HTTPS_PROXY", addr.clone()));
        sets.push(("HTTP_PROXY", addr.clone()));
        sets.extend(git::proxy_env_sets(plan, existing_no_proxy));
    }
    (sets, git::proxy_env_removals(plan).to_vec())
}

fn apply_proxy_env(cmd: &mut Command, plan: &ProxyPlan) {
    let existing = std::env::var("no_proxy").or_else(|_| std::env::var("NO_PROXY")).ok();
    let (sets, removals) = proxy_env(plan, existing);
    for var in removals {
        cmd.env_remove(var);
    }
    for (k, v) in sets {
        cmd.env(k, v);
    }
}

fn base_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

async fn exec(cmd: Command, timeout: Duration) -> Result<String, ToolError> {
    let mut cmd = cmd;
    let child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ToolError::NotInstalled
        } else {
            ToolError::Failed(format!("无法执行：{e}"))
        }
    })?;
    // 超时后 future 被丢弃，kill_on_drop 会终止子进程
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Err(_) => Err(ToolError::Timeout(timeout.as_secs())),
        Ok(Err(e)) => Err(ToolError::Failed(e.to_string())),
        Ok(Ok(out)) => {
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            } else {
                let stderr = redact(&String::from_utf8_lossy(&out.stderr));
                let stderr = stderr.trim();
                Err(ToolError::Failed(if stderr.is_empty() {
                    format!("退出码 {}", out.status.code().map_or("未知".to_string(), |c| c.to_string()))
                } else {
                    stderr.chars().take(500).collect()
                }))
            }
        }
    }
}

/// 执行 `gh <args>`（只允许 `validate_gh` 里的写法）。找不到 gh → `ToolError::NotInstalled`
pub async fn run_gh(args: &[&str], timeout: Duration) -> Result<String, ToolError> {
    validate_gh(args).map_err(ToolError::Forbidden)?;
    let mut cmd = base_command("gh");
    cmd.args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1");
    apply_proxy_env(&mut cmd, &git::proxy_plan());
    exec(cmd, timeout).await
}

/// curl 的固定参数（地址之前）
const CURL_FIXED: [&str; 8] =
    ["-sS", "--fail", "--max-time", "30", "-H", "Accept: application/vnd.github+json", "-H", "User-Agent: Git-SyncDeck"];

/// 公开接口的地址只有两种：账号的仓库列表（第 1 ~ 10 页）、单个仓库
fn valid_api_url(url: &str) -> bool {
    if let Some(rest) = url.strip_prefix("https://api.github.com/users/") {
        let Some((owner, query)) = rest.split_once("/repos?per_page=100&type=owner&sort=pushed&page=") else {
            return false;
        };
        return valid_owner(owner) && matches!(query.parse::<u32>(), Ok(1..=10)) && query.chars().all(|c| c.is_ascii_digit());
    }
    if let Some(rest) = url.strip_prefix("https://api.github.com/repos/") {
        return valid_full_name(rest);
    }
    false
}

/// curl 白名单：`[-x <代理地址> | --noproxy *] -sS --fail --max-time 30 -H Accept… -H User-Agent… <公开接口地址>`
pub fn validate_curl(args: &[&str]) -> Result<(), String> {
    let rest = match args {
        ["-x", addr, rest @ ..] if !addr.is_empty() && !addr.starts_with('-') => rest,
        ["--noproxy", "*", rest @ ..] => rest,
        rest => rest,
    };
    match rest {
        [fixed @ .., url] if fixed == CURL_FIXED && valid_api_url(url) => Ok(()),
        _ => Err("curl 只允许请求 GitHub 公开接口的账号仓库列表或单个仓库".into()),
    }
}

/// 公开接口账号仓库列表的地址
pub fn api_list_url(owner: &str, page: u32) -> String {
    format!("https://api.github.com/users/{owner}/repos?per_page=100&type=owner&sort=pushed&page={page}")
}

/// 公开接口单个仓库的地址
pub fn api_repo_url(owner: &str, name: &str) -> String {
    format!("https://api.github.com/repos/{owner}/{name}")
}

/// curl 的完整参数（代理按 plan：Use → `-x`，Disable → `--noproxy *`）
pub fn curl_args(url: &str, plan: &ProxyPlan) -> Vec<String> {
    let mut args = Vec::new();
    match &plan.over {
        ProxyOverride::Use(addr) => args.extend(["-x".to_string(), addr.clone()]),
        ProxyOverride::Disable => args.extend(["--noproxy".to_string(), "*".to_string()]),
        ProxyOverride::Inherit => {}
    }
    args.extend(CURL_FIXED.iter().map(|s| s.to_string()));
    args.push(url.to_string());
    args
}

fn curl_program() -> PathBuf {
    if let Some(root) = std::env::var_os("SystemRoot") {
        let p = PathBuf::from(root).join("System32").join("curl.exe");
        if p.exists() {
            return p;
        }
    }
    PathBuf::from("curl")
}

/// 请求一个公开接口地址，返回响应正文
pub async fn run_curl(url: &str, timeout: Duration) -> Result<String, ToolError> {
    let plan = git::proxy_plan();
    let args = curl_args(url, &plan);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    validate_curl(&refs).map_err(ToolError::Forbidden)?;
    let mut cmd = base_command(curl_program());
    cmd.args(&args);
    apply_proxy_env(&mut cmd, &plan);
    exec(cmd, timeout).await
}

// ---------------- gh 状态 ----------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhStatus {
    /// 没装 gh
    Missing,
    /// 装了但没登录（或查询失败）
    NotLoggedIn,
    LoggedIn(String),
}

/// `gh api user --jq .login`
pub async fn gh_status() -> GhStatus {
    match run_gh(&["api", "user", "--jq", ".login"], tool_timeout()).await {
        Ok(out) if !out.trim().is_empty() => GhStatus::LoggedIn(out.trim().to_string()),
        Ok(_) => GhStatus::NotLoggedIn,
        Err(ToolError::NotInstalled) => GhStatus::Missing,
        Err(_) => GhStatus::NotLoggedIn,
    }
}

/// 用 gh 列出仓库（账号名下的全部，或单个仓库）
pub async fn gh_list(source: &Source) -> Result<Vec<GithubRepo>, ToolError> {
    let out = match source {
        Source::Owner(owner) => {
            run_gh(&["repo", "list", owner, "--limit", "1000", "--json", GH_FIELDS], tool_timeout()).await?
        }
        Source::Repo { owner, name } => {
            run_gh(&["repo", "view", &format!("{owner}/{name}"), "--json", GH_FIELDS], tool_timeout()).await?
        }
    };
    let mut repos = parse_gh_list(&out).map_err(ToolError::Failed)?;
    sort_repos(&mut repos);
    Ok(repos)
}

/// 用公开接口列出仓库（账号最多翻 10 页，不足 100 条就停）
pub async fn public_list(source: &Source) -> Result<Vec<GithubRepo>, ToolError> {
    let mut repos = Vec::new();
    match source {
        Source::Owner(owner) => {
            for page in 1..=10 {
                let out = run_curl(&api_list_url(owner, page), tool_timeout()).await?;
                let batch = parse_api_list(&out).map_err(ToolError::Failed)?;
                let n = batch.len();
                repos.extend(batch);
                if n < 100 {
                    break;
                }
            }
        }
        Source::Repo { owner, name } => {
            let out = run_curl(&api_repo_url(owner, name), tool_timeout()).await?;
            repos = parse_api_list(&out).map_err(ToolError::Failed)?;
        }
    }
    sort_repos(&mut repos);
    Ok(repos)
}

/// 公开接口失败的原因翻成人话
pub fn public_error_text(e: &ToolError, source: &Source) -> String {
    let m = e.message();
    let what = match source {
        Source::Owner(o) => format!("账号 {o}"),
        Source::Repo { owner, name } => format!("仓库 {owner}/{name}"),
    };
    match e {
        ToolError::NotInstalled => "系统里找不到 curl.exe，无法访问 GitHub 公开接口".into(),
        ToolError::Failed(_) if m.contains("404") => format!("找不到 GitHub {what}（地址写错了，或者它是私有的、需要先用 gh 登录）"),
        ToolError::Failed(_) if m.contains("403") || m.contains("429") => {
            "GitHub 公开接口访问太频繁（被限流），请稍后再试，或装好 GitHub CLI（gh）并运行 gh auth login 后重试".into()
        }
        _ => format!("无法从 GitHub 获取仓库列表：{m}"),
    }
}

// ---------------- JSON 解析与排序 ----------------

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn with_git_suffix(url: &str) -> String {
    let u = url.trim_end_matches('/');
    if u.ends_with(".git") {
        u.to_string()
    } else {
        format!("{u}.git")
    }
}

fn items(json: &str) -> Result<Vec<Value>, String> {
    match serde_json::from_str::<Value>(json).map_err(|e| format!("无法解析 GitHub 返回的内容：{e}"))? {
        Value::Array(a) => Ok(a),
        v @ Value::Object(_) => Ok(vec![v]),
        _ => Err("GitHub 返回的内容格式不对".into()),
    }
}

/// 解析 gh 的 `--json` 输出（字段名 camelCase）；`repo view` 返回单个对象，也接受
pub fn parse_gh_list(json: &str) -> Result<Vec<GithubRepo>, String> {
    items(json)?
        .iter()
        .map(|v| {
            let name = text(v, "name").ok_or("仓库缺少 name 字段")?;
            let url = text(v, "url").ok_or("仓库缺少 url 字段")?;
            Ok(GithubRepo {
                full_name: text(v, "nameWithOwner").unwrap_or_else(|| name.clone()),
                name,
                clone_url: with_git_suffix(&url),
                html_url: url.trim_end_matches('/').trim_end_matches(".git").to_string(),
                private: flag(v, "isPrivate"),
                fork: flag(v, "isFork"),
                archived: flag(v, "isArchived"),
                pushed_at: text(v, "pushedAt"),
                description: text(v, "description"),
            })
        })
        .collect()
}

/// 解析 GitHub 公开接口的输出（字段 snake_case）；`/repos/<账号>/<仓库>` 返回单个对象，也接受
pub fn parse_api_list(json: &str) -> Result<Vec<GithubRepo>, String> {
    items(json)?
        .iter()
        .map(|v| {
            let name = text(v, "name").ok_or("仓库缺少 name 字段")?;
            let html_url = text(v, "html_url").ok_or("仓库缺少 html_url 字段")?;
            let clone_url = text(v, "clone_url").unwrap_or_else(|| with_git_suffix(&html_url));
            Ok(GithubRepo {
                full_name: text(v, "full_name").unwrap_or_else(|| name.clone()),
                name,
                clone_url: with_git_suffix(&clone_url),
                html_url,
                private: flag(v, "private"),
                fork: flag(v, "fork"),
                archived: flag(v, "archived"),
                pushed_at: text(v, "pushed_at"),
                description: text(v, "description"),
            })
        })
        .collect()
}

/// 排序：非 fork 非归档的在前，各自按最近推送时间倒序
pub fn sort_repos(repos: &mut [GithubRepo]) {
    repos.sort_by(|a, b| {
        let rank = |r: &GithubRepo| r.fork || r.archived;
        rank(a).cmp(&rank(b)).then_with(|| b.pushed_at.cmp(&a.pushed_at)).then_with(|| a.name.cmp(&b.name))
    });
}

// ---------------- 远程地址规范化 ----------------

/// 把远程地址规范成可比较的形式：去账号、小写、去协议、去 `.git` 和末尾 `/`，
/// `git@github.com:a/b` 转成 `github.com/a/b`
pub fn normalize_remote(url: &str) -> String {
    let cleaned = crate::url::sanitize_url(url.trim());
    let mut s = cleaned.to_lowercase();
    if let Some(i) = s.find("://") {
        s = s[i + 3..].to_string();
        if let Some(rest) = s.strip_prefix("git@") {
            s = rest.to_string();
        }
    } else if let Some(rest) = s.strip_prefix("git@") {
        s = rest.replacen(':', "/", 1);
    }
    let s = s.trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s);
    s.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(s: &str) -> Source {
        Source::Owner(s.into())
    }
    fn repo(o: &str, n: &str) -> Source {
        Source::Repo { owner: o.into(), name: n.into() }
    }

    #[test]
    fn parses_owner_inputs() {
        for input in [
            "https://github.com/Owenwoow",
            "https://github.com/Owenwoow/",
            "github.com/Owenwoow",
            "  http://www.github.com/Owenwoow/  ",
            "HTTPS://GitHub.com/Owenwoow",
            "Owenwoow",
            "https://github.com/Owenwoow?tab=repositories",
        ] {
            assert_eq!(parse_source(input), Ok(owner("Owenwoow")), "{input}");
        }
        assert_eq!(parse_source("a-b-1"), Ok(owner("a-b-1")));
    }

    #[test]
    fn parses_repo_inputs() {
        for input in [
            "https://github.com/Owenwoow/Git-SyncDeck",
            "https://github.com/Owenwoow/Git-SyncDeck/",
            "https://github.com/Owenwoow/Git-SyncDeck.git",
            "github.com/Owenwoow/Git-SyncDeck",
            "git@github.com:Owenwoow/Git-SyncDeck.git",
            "git@github.com:Owenwoow/Git-SyncDeck",
            "Owenwoow/Git-SyncDeck",
        ] {
            assert_eq!(parse_source(input), Ok(repo("Owenwoow", "Git-SyncDeck")), "{input}");
        }
        assert_eq!(parse_source("github.com/a/b_c.d"), Ok(repo("a", "b_c.d")));
    }

    #[test]
    fn rejects_bad_inputs() {
        for input in [
            "",
            "   ",
            "github.com",
            "https://github.com/",
            "https://gitlab.com/someone",
            "ftp://github.com/someone",
            "ssh://git@github.com/a/b",
            "-leading",
            "https://github.com/-leading",
            "bad_name",
            "中文用户",
            "a b",
            "https://github.com/a/b/tree/main",
            "https://github.com/a/../b",
            "https://github.com/a/..",
            "https://github.com/a/b c",
            "https://github.com/a/中文仓库",
            "git@gitlab.com:a/b.git",
            "https://example.com/a",
            &"x".repeat(40),
        ] {
            assert!(parse_source(input).is_err(), "应当拒绝：{input:?}");
        }
        let e = parse_source("bad_name").unwrap_err();
        assert!(e.contains("用户名"), "{e}");
    }

    #[test]
    fn gh_whitelist() {
        let list = |o: &str| ["repo", "list", o, "--limit", "1000", "--json", GH_FIELDS].map(str::to_string).to_vec();
        let view = |r: &str| ["repo", "view", r, "--json", GH_FIELDS].map(str::to_string).to_vec();
        let ok = |v: Vec<String>| validate_gh(&v.iter().map(String::as_str).collect::<Vec<_>>()).is_ok();
        assert!(validate_gh(&["api", "user", "--jq", ".login"]).is_ok());
        assert!(ok(list("Owenwoow")));
        assert!(ok(view("Owenwoow/Git-SyncDeck")));

        assert!(!ok(list("-bad")));
        assert!(!ok(list("a/b")));
        assert!(!ok(list("")));
        assert!(!ok(view("-x/y")));
        assert!(!ok(view("a")));
        assert!(!ok(view("a/b/c")));
        let mut long = list("a");
        long[4] = "100".into();
        assert!(!ok(long));
        let mut other = list("a");
        other[6] = "name,url,viewerPermission".into();
        assert!(!ok(other));
        let mut extra = list("a");
        extra.push("--web".into());
        assert!(!ok(extra));

        for args in [
            vec!["auth", "login"],
            vec!["auth", "status"],
            vec!["auth", "setup-git"],
            vec!["auth", "token"],
            vec!["repo", "clone", "a/b"],
            vec!["repo", "delete", "a/b", "--yes"],
            vec!["api", "user"],
            vec!["api", "user", "--jq", ".email"],
            vec!["api", "/user/repos"],
            vec!["api", "user", "--jq", ".login", "-X", "DELETE"],
            vec!["api", "-X", "DELETE", "repos/a/b"],
            vec!["config", "set", "x", "y"],
            vec!["--version"],
            vec![],
        ] {
            assert!(validate_gh(&args).is_err(), "应当拒绝：{args:?}");
        }
    }

    #[test]
    fn curl_whitelist() {
        let plan = |o: ProxyOverride| ProxyPlan::from(o);
        let check = |url: &str, p: ProxyOverride| {
            let args = curl_args(url, &plan(p));
            validate_curl(&args.iter().map(String::as_str).collect::<Vec<_>>())
        };
        for url in [
            api_list_url("Owenwoow", 1),
            api_list_url("Owenwoow", 10),
            api_repo_url("Owenwoow", "Git-SyncDeck"),
            api_repo_url("a", "b.c_d"),
        ] {
            assert!(check(&url, ProxyOverride::Inherit).is_ok(), "{url}");
            assert!(check(&url, ProxyOverride::Use("http://127.0.0.1:7890".into())).is_ok(), "{url}");
            assert!(check(&url, ProxyOverride::Disable).is_ok(), "{url}");
        }
        // 参数里的代理写法
        let args = curl_args(&api_repo_url("a", "b"), &plan(ProxyOverride::Use("http://h:1".into())));
        assert_eq!(&args[..2], ["-x", "http://h:1"]);
        let args = curl_args(&api_repo_url("a", "b"), &plan(ProxyOverride::Disable));
        assert_eq!(&args[..2], ["--noproxy", "*"]);

        for url in [
            "https://api.github.com/users/a/repos?per_page=100&type=owner&sort=pushed&page=0",
            "https://api.github.com/users/a/repos?per_page=100&type=owner&sort=pushed&page=11",
            "https://api.github.com/users/a/repos?per_page=100&type=owner&sort=pushed&page=x",
            "https://api.github.com/users/a/repos?per_page=100&type=all&sort=pushed&page=1",
            "https://api.github.com/users/-a/repos?per_page=100&type=owner&sort=pushed&page=1",
            "https://api.github.com/users/a/starred",
            "https://api.github.com/user/repos",
            "https://api.github.com/repos/a",
            "https://api.github.com/repos/a/b/contents",
            "https://api.github.com/repos/a/b?x=1",
            "https://api.github.com/repos/-a/b",
            "http://api.github.com/repos/a/b",
            "https://evil.example/repos/a/b",
            "https://api.github.com.evil.example/repos/a/b",
            "file:///C:/Windows/win.ini",
            "-o",
        ] {
            assert!(check(url, ProxyOverride::Inherit).is_err(), "应当拒绝：{url}");
        }
        let good = api_repo_url("a", "b");
        let fixed: Vec<String> = CURL_FIXED.iter().map(|s| s.to_string()).collect();
        let with = |extra: &[&str], url: &str| {
            let mut v: Vec<&str> = extra.to_vec();
            v.extend(fixed.iter().map(String::as_str));
            v.push(url);
            validate_curl(&v)
        };
        assert!(with(&[], &good).is_ok());
        // 其他选项（写文件、带凭据、跟随跳转、换证书……）一律拒绝
        for extra in [
            vec!["-o", "x.txt"],
            vec!["-u", "a:b"],
            vec!["-L"],
            vec!["-k"],
            vec!["-x"],
            vec!["-x", "--output"],
            vec!["--noproxy", "x"],
            vec!["-H", "Authorization: Bearer x"],
            vec!["-x", "http://h:1", "-o", "x"],
        ] {
            assert!(with(&extra, &good).is_err(), "应当拒绝：{extra:?}");
        }
        assert!(validate_curl(&["-sS", &good]).is_err());
        assert!(validate_curl(&[]).is_err());
    }

    #[test]
    fn proxy_env_rules() {
        let use_plan = ProxyPlan {
            over: ProxyOverride::Use("http://127.0.0.1:7890".into()),
            url_scopes: vec![],
            no_proxy: Some("localhost,10.0.0.0/8".into()),
        };
        let (sets, removed) = proxy_env(&use_plan, None);
        assert!(removed.is_empty());
        let get = |k: &str| sets.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("HTTPS_PROXY"), Some("http://127.0.0.1:7890"));
        assert_eq!(get("HTTP_PROXY"), Some("http://127.0.0.1:7890"));
        assert_eq!(get("NO_PROXY"), Some("localhost,10.0.0.0/8"));

        let no_bypass = ProxyPlan::from(ProxyOverride::Use("http://h:1".into()));
        let (sets, _) = proxy_env(&no_bypass, None);
        assert!(sets.iter().all(|(k, _)| !k.eq_ignore_ascii_case("no_proxy")));

        let (sets, removed) = proxy_env(&ProxyPlan::from(ProxyOverride::Disable), None);
        assert!(sets.is_empty());
        assert_eq!(removed.len(), git::PROXY_ENV_VARS.len());
        assert!(removed.contains(&"HTTPS_PROXY") && removed.contains(&"all_proxy"));

        let (sets, removed) = proxy_env(&ProxyPlan::default(), None);
        assert!(sets.is_empty() && removed.is_empty());
    }

    const GH_SAMPLE: &str = r#"[
      {"name":"old-fork","nameWithOwner":"Owenwoow/old-fork","url":"https://github.com/Owenwoow/old-fork","isPrivate":false,"isFork":true,"isArchived":false,"pushedAt":"2026-09-01T00:00:00Z","description":""},
      {"name":"secret","nameWithOwner":"Owenwoow/secret","url":"https://github.com/Owenwoow/secret","isPrivate":true,"isFork":false,"isArchived":false,"pushedAt":"2026-01-02T03:04:05Z","description":"私有的东西"},
      {"name":"Git-SyncDeck","nameWithOwner":"Owenwoow/Git-SyncDeck","url":"https://github.com/Owenwoow/Git-SyncDeck","isPrivate":false,"isFork":false,"isArchived":false,"pushedAt":"2026-10-20T10:00:00Z","description":"多项目同步"},
      {"name":"archived","nameWithOwner":"Owenwoow/archived","url":"https://github.com/Owenwoow/archived","isPrivate":false,"isFork":false,"isArchived":true,"pushedAt":"2026-12-01T00:00:00Z","description":null},
      {"name":"empty","nameWithOwner":"Owenwoow/empty","url":"https://github.com/Owenwoow/empty","isPrivate":false,"isFork":false,"isArchived":false,"pushedAt":null,"description":null}
    ]"#;

    #[test]
    fn parses_gh_output() {
        let mut repos = parse_gh_list(GH_SAMPLE).unwrap();
        assert_eq!(repos.len(), 5);
        let r = &repos[2];
        assert_eq!(r.name, "Git-SyncDeck");
        assert_eq!(r.full_name, "Owenwoow/Git-SyncDeck");
        assert_eq!(r.clone_url, "https://github.com/Owenwoow/Git-SyncDeck.git");
        assert_eq!(r.html_url, "https://github.com/Owenwoow/Git-SyncDeck");
        assert!(!r.private && !r.fork && !r.archived);
        assert_eq!(r.pushed_at.as_deref(), Some("2026-10-20T10:00:00Z"));
        assert_eq!(r.description.as_deref(), Some("多项目同步"));
        assert!(repos[1].private && repos[0].fork && repos[3].archived);
        assert_eq!(repos[0].description, None, "空字符串当作没有");
        assert_eq!(repos[4].pushed_at, None);

        // 非 fork 非归档在前（按推送时间倒序，没有推送时间的最后），然后才是 fork 和归档（同样按推送时间倒序）
        sort_repos(&mut repos);
        let names: Vec<_> = repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Git-SyncDeck", "secret", "empty", "archived", "old-fork"]);
    }

    #[test]
    fn parses_gh_single_object_and_errors() {
        let one = r#"{"name":"x","nameWithOwner":"a/x","url":"https://github.com/a/x","isPrivate":true,"isFork":false,"isArchived":false,"pushedAt":"2026-01-01T00:00:00Z","description":"d"}"#;
        let repos = parse_gh_list(one).unwrap();
        assert_eq!(repos.len(), 1);
        assert!(repos[0].private);
        assert_eq!(parse_gh_list("[]").unwrap().len(), 0);
        assert!(parse_gh_list("not json").is_err());
        assert!(parse_gh_list("42").is_err());
        assert!(parse_gh_list(r#"[{"url":"https://github.com/a/x"}]"#).is_err());
    }

    #[test]
    fn parses_public_api_output() {
        let json = r#"[
          {"id":1,"name":"blog","full_name":"Owenwoow/blog","html_url":"https://github.com/Owenwoow/blog","clone_url":"https://github.com/Owenwoow/blog.git","private":false,"fork":false,"archived":false,"pushed_at":"2026-05-01T00:00:00Z","description":"博客"},
          {"id":2,"name":"lib","full_name":"Owenwoow/lib","html_url":"https://github.com/Owenwoow/lib","clone_url":"https://github.com/Owenwoow/lib.git","private":false,"fork":true,"archived":false,"pushed_at":"2026-08-01T00:00:00Z","description":null},
          {"id":3,"name":"old","full_name":"Owenwoow/old","html_url":"https://github.com/Owenwoow/old","private":false,"fork":false,"archived":true,"pushed_at":null}
        ]"#;
        let mut repos = parse_api_list(json).unwrap();
        assert_eq!(repos.len(), 3);
        assert_eq!(repos[0].full_name, "Owenwoow/blog");
        assert_eq!(repos[0].clone_url, "https://github.com/Owenwoow/blog.git");
        assert_eq!(repos[0].description.as_deref(), Some("博客"));
        assert!(repos[1].fork && repos[2].archived);
        assert_eq!(repos[2].clone_url, "https://github.com/Owenwoow/old.git", "没有 clone_url 时由 html_url 推出");
        assert_eq!(repos[2].pushed_at, None);
        sort_repos(&mut repos);
        let names: Vec<_> = repos.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["blog", "lib", "old"]);

        // 单个仓库返回对象
        let one = r#"{"name":"blog","full_name":"Owenwoow/blog","html_url":"https://github.com/Owenwoow/blog","clone_url":"https://github.com/Owenwoow/blog.git","private":false,"fork":false,"archived":false,"pushed_at":"2026-05-01T00:00:00Z"}"#;
        assert_eq!(parse_api_list(one).unwrap().len(), 1);
        // GitHub 出错时返回的是 {"message": …}，不当作仓库
        assert!(parse_api_list(r#"{"message":"Not Found"}"#).is_err());
        assert!(parse_api_list("<html>").is_err());
    }

    #[test]
    fn normalizes_remotes() {
        let want = "github.com/owenwoow/git-syncdeck";
        for url in [
            "https://github.com/Owenwoow/Git-SyncDeck.git",
            "https://github.com/Owenwoow/Git-SyncDeck",
            "https://github.com/Owenwoow/Git-SyncDeck/",
            "https://github.com/Owenwoow/Git-SyncDeck.git/",
            "HTTPS://GitHub.com/owenwoow/git-syncdeck.GIT",
            "https://user:ghp_token@github.com/Owenwoow/Git-SyncDeck.git",
            "https://owenwoow@github.com/Owenwoow/Git-SyncDeck.git",
            "git@github.com:Owenwoow/Git-SyncDeck.git",
            "git@github.com:Owenwoow/Git-SyncDeck",
            "ssh://git@github.com/Owenwoow/Git-SyncDeck.git",
            "  https://github.com/Owenwoow/Git-SyncDeck.git  ",
        ] {
            assert_eq!(normalize_remote(url), want, "{url}");
        }
        assert_ne!(normalize_remote("https://github.com/Owenwoow/other.git"), want);
        assert_ne!(normalize_remote("https://gitlab.com/Owenwoow/Git-SyncDeck.git"), want);
        // 本地路径只做小写和去 .git
        assert_eq!(normalize_remote(r"C:\Remotes\Synced.git"), r"c:\remotes\synced");
    }

    #[test]
    fn public_error_texts() {
        let src = owner("ghost");
        assert!(public_error_text(&ToolError::Failed("curl: (22) The requested URL returned error: 404".into()), &src)
            .contains("找不到"));
        assert!(public_error_text(&ToolError::Failed("curl: (22) … error: 403".into()), &src).contains("限流"));
        assert!(public_error_text(&ToolError::Timeout(30), &src).contains("超时"));
        assert!(public_error_text(&ToolError::NotInstalled, &src).contains("curl"));
    }
}
