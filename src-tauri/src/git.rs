//! 调用系统 git 命令行的唯一入口。所有 git 子进程都从 `run_git` 创建，统一处理：
//! - 不弹出命令行窗口（CREATE_NO_WINDOW）
//! - 不等待任何凭据输入（GIT_TERMINAL_PROMPT=0、GCM_INTERACTIVE=never）
//! - 中文文件名（core.quotepath=false），输出按 UTF-8 解码
//! - 超时后终止子进程
//! - 安全白名单：只允许本工具需要的子命令和参数，任何可能改写历史或丢失改动的操作一律拒绝

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::url::redact;

/// 状态查询类命令（status、config、log……）的超时
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(10);
/// 联网或写入类命令（fetch、push、pull、add、commit）的超时
pub const NETWORK_TIMEOUT: Duration = Duration::from_secs(60);

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
    let command = args.first().copied().unwrap_or("").to_string();

    let mut cmd = Command::new("git");
    cmd.args(["-c", "core.quotepath=false", "-c", "gc.auto=0", "-c", "maintenance.auto=false"])
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
}
