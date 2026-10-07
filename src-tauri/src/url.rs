//! 去掉 URL 里的账号、密码、token。远程地址和 git 的错误输出在显示、保存、写进诊断文本之前都要经过这里。

/// 远程地址脱敏：
/// - `https://user:token@github.com/a/b.git` → `https://github.com/a/b.git`（http/https 的账号部分全部去掉）
/// - `ssh://git:pass@host/a/b.git` → `ssh://git@host/a/b.git`（只去掉密码）
/// - `git@github.com:a/b.git` 保持不变（这里的 git 只是 SSH 用户名，不是凭据）
pub fn sanitize_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let rest_start = scheme_end + 3;
    let rest = &url[rest_start..];
    let host_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..host_end];
    let Some(at) = authority.rfind('@') else {
        return url.to_string();
    };
    let scheme = url[..scheme_end].to_ascii_lowercase();
    let userinfo = &authority[..at];
    if scheme.starts_with("http") || userinfo.is_empty() {
        format!("{}{}", &url[..rest_start], &rest[at + 1..])
    } else if let Some(colon) = userinfo.find(':') {
        format!("{}{}@{}", &url[..rest_start], &userinfo[..colon], &rest[at + 1..])
    } else {
        url.to_string()
    }
}

/// 把一段文字里所有 `scheme://userinfo@host` 的 userinfo 去掉（用于 git 的错误输出）
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("://") {
        let (before, after) = rest.split_at(pos + 3);
        out.push_str(before);
        let end = after
            .find(|c: char| c == '/' || c.is_whitespace() || c == '\'' || c == '"')
            .unwrap_or(after.len());
        let authority = &after[..end];
        match authority.rfind('@') {
            Some(at) => out.push_str(&authority[at + 1..]),
            None => out.push_str(authority),
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_credentials() {
        assert_eq!(sanitize_url("https://ghp_abc123@github.com/o/r.git"), "https://github.com/o/r.git");
        assert_eq!(sanitize_url("https://user:pw@github.com/o/r.git"), "https://github.com/o/r.git");
        assert_eq!(sanitize_url("ssh://git:pw@github.com/o/r.git"), "ssh://git@github.com/o/r.git");
        assert_eq!(sanitize_url("ssh://git@github.com/o/r.git"), "ssh://git@github.com/o/r.git");
        assert_eq!(sanitize_url("git@github.com:o/r.git"), "git@github.com:o/r.git");
        assert_eq!(sanitize_url("https://github.com/o/r.git"), "https://github.com/o/r.git");
        assert_eq!(sanitize_url("D:/remotes/r.git"), "D:/remotes/r.git");
    }

    #[test]
    fn redacts_text() {
        let s = "fatal: unable to access 'https://ghp_x@github.com/o/r.git/': 403";
        assert_eq!(redact(s), "fatal: unable to access 'https://github.com/o/r.git/': 403");
        assert_eq!(redact("没有 URL 的中文错误"), "没有 URL 的中文错误");
    }
}
