//! 去掉 URL 里的账号、密码、token。远程地址和 git 的错误输出在显示、保存、写进诊断文本之前都要经过这里。
//! 另外负责代理地址的校验、Windows 系统代理值的解析和显示前去敏。

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

// ---------------- 代理地址 ----------------

/// 手动填写的代理地址允许的协议
const PROXY_SCHEMES: [&str; 4] = ["http", "https", "socks5", "socks5h"];

const PROXY_FORMAT_HINT: &str =
    "代理地址格式不对：要以 http://、https://、socks5:// 或 socks5h:// 开头，并写明主机和端口，如 http://127.0.0.1:7890";

/// 校验代理地址：只接受 http / https / socks5 / socks5h 协议、写明主机和端口的地址（可以带账号密码，不能带路径）。
/// 返回整理后的地址（去掉首尾空白和末尾的 /，协议名转小写）。
pub fn validate_proxy_url(s: &str) -> Result<String, String> {
    let err = || PROXY_FORMAT_HINT.to_string();
    let s = s.trim();
    let (scheme, rest) = s.split_once("://").ok_or_else(err)?;
    let scheme = scheme.to_ascii_lowercase();
    if !PROXY_SCHEMES.contains(&scheme.as_str()) {
        return Err(err());
    }
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.is_empty() || rest.contains(['/', '?', '#']) || rest.chars().any(char::is_whitespace) {
        return Err(err());
    }
    let host_port = rest.rsplit_once('@').map_or(rest, |(_, h)| h);
    let (host, port) = match host_port.strip_prefix('[') {
        // IPv6：[::1]:1080
        Some(v6) => {
            let (h, p) = v6.split_once(']').ok_or_else(err)?;
            let ok = !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
            (if ok { h } else { "" }, p.strip_prefix(':').ok_or_else(err)?)
        }
        None => {
            let (h, p) = host_port.rsplit_once(':').ok_or_else(err)?;
            let ok = h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
            (if ok { h } else { "" }, p)
        }
    };
    let port_ok = !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) && port.parse::<u16>().is_ok_and(|p| p > 0);
    if host.is_empty() || !port_ok {
        return Err(err());
    }
    Ok(format!("{scheme}://{rest}"))
}

/// 把 Windows 系统代理的 ProxyServer 值转成 git 能用的代理地址。
/// 可能是 `host:port`，也可能是 `http=h:p;https=h:p;socks=h:p`：优先 https=，其次 http=，再次不带前缀的，
/// 最后 socks=（转成 `socks5://`）；没写协议的补 `http://`。转不出合法地址时返回 None。
pub fn parse_windows_proxy_server(s: &str) -> Option<String> {
    let (mut https, mut http, mut bare, mut socks) = (None, None, None, None);
    for part in s.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        match part.split_once('=') {
            Some((key, value)) => {
                let value = value.trim();
                if value.is_empty() {
                    continue;
                }
                let slot = match key.trim().to_ascii_lowercase().as_str() {
                    "https" => &mut https,
                    "http" => &mut http,
                    "socks" => &mut socks,
                    _ => continue, // ftp 等，git 用不上
                };
                slot.get_or_insert(value);
            }
            None => {
                bare.get_or_insert(part);
            }
        }
    }
    let (addr, scheme) = https.or(http).or(bare).map(|a| (a, "http")).or(socks.map(|a| (a, "socks5")))?;
    let url = if addr.contains("://") { addr.to_string() } else { format!("{scheme}://{addr}") };
    validate_proxy_url(&url).ok()
}

/// 把 Windows 系统代理的绕过列表（注册表 ProxyOverride，如 `localhost;127.*;192.168.*;*.corp.local;<local>`）
/// 尽量转成 git（libcurl）认的 NO_PROXY（逗号分隔）：
/// - `*.corp.local` → `.corp.local`（后缀匹配）
/// - `192.168.*`、`10.*.*.*` 这类 IP 前缀 → `192.168.0.0/16`、`10.0.0.0/8`
/// - `<local>`（不带点的主机名）curl 表达不了，换成 `localhost,127.0.0.1,::1`
/// - 普通主机名、IP、CIDR 原样保留；单独一个 `*` 表示全部绕过，原样保留
/// - 其他转不了的（带端口、带协议、通配符在中间等）丢掉
///
/// 一条都转不出来时返回 None。
pub fn windows_bypass_to_no_proxy(list: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !out.contains(&s) {
            out.push(s);
        }
    };
    for entry in list.split([';', ',', ' ', '\t', '\r', '\n']).map(str::trim).filter(|e| !e.is_empty()) {
        let entry = entry.to_ascii_lowercase();
        if entry == "<local>" {
            ["localhost", "127.0.0.1", "::1"].into_iter().for_each(|s| push(s.to_string()));
        } else if let Some(converted) = convert_bypass_entry(&entry) {
            push(converted);
        }
    }
    (!out.is_empty()).then(|| out.join(","))
}

fn convert_bypass_entry(entry: &str) -> Option<String> {
    if entry == "*" {
        return Some("*".into());
    }
    // *.corp.local → .corp.local
    if let Some(suffix) = entry.strip_prefix("*.") {
        return is_host_name(suffix).then(|| format!(".{suffix}"));
    }
    // 192.168.* → 192.168.0.0/16
    if entry.contains('*') {
        let parts: Vec<&str> = entry.split('.').collect();
        let fixed = parts.iter().take_while(|p| **p != "*").count();
        let octets_ok = parts[..fixed].iter().all(|p| p.parse::<u8>().is_ok() && !p.starts_with('+'));
        let rest_all_star = parts[fixed..].iter().all(|p| *p == "*");
        if parts.len() > 4 || fixed == 0 || !octets_ok || !rest_all_star {
            return None;
        }
        let mut octets: Vec<&str> = parts[..fixed].to_vec();
        octets.resize(4, "0");
        return Some(format!("{}/{}", octets.join("."), fixed * 8));
    }
    // IPv6（可能带方括号）、IPv4、CIDR
    let bare = entry.strip_prefix('[').and_then(|e| e.strip_suffix(']')).unwrap_or(entry);
    if bare.parse::<std::net::IpAddr>().is_ok() {
        return Some(bare.to_string());
    }
    if let Some((ip, bits)) = bare.split_once('/') {
        let max = match ip.parse::<std::net::IpAddr>().ok()? {
            std::net::IpAddr::V4(_) => 32,
            std::net::IpAddr::V6(_) => 128,
        };
        return bits.parse::<u8>().ok().filter(|b| *b <= max).map(|_| bare.to_string());
    }
    // 普通主机名（.corp.local 这种前面带点的也行）
    let host = entry.strip_prefix('.').unwrap_or(entry);
    is_host_name(host).then(|| entry.to_string())
}

/// 只由字母、数字、点、连字符、下划线组成的主机名（不带端口、通配符、协议）
fn is_host_name(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && !s.ends_with('.')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// 显示用的代理地址：去掉账号密码（带不带协议都处理）
pub fn display_proxy(addr: &str) -> String {
    let s = redact(addr.trim());
    if s.contains("://") {
        return s;
    }
    match s.rsplit_once('@') {
        Some((_, host)) => host.to_string(),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_proxy_url() {
        for (input, want) in [
            ("http://127.0.0.1:7890", "http://127.0.0.1:7890"),
            ("  socks5://127.0.0.1:10808/ ", "socks5://127.0.0.1:10808"),
            ("SOCKS5H://proxy.lan:1080", "socks5h://proxy.lan:1080"),
            ("https://proxy.example.com:443", "https://proxy.example.com:443"),
            ("http://user:pw@127.0.0.1:8080", "http://user:pw@127.0.0.1:8080"),
            ("http://[::1]:7890", "http://[::1]:7890"),
        ] {
            assert_eq!(validate_proxy_url(input).as_deref(), Ok(want), "{input}");
        }
        for bad in [
            "",
            "127.0.0.1:7890",            // 没写协议
            "ftp://127.0.0.1:21",        // 不支持的协议
            "socks4://127.0.0.1:1080",   // 不支持的协议
            "http://127.0.0.1",          // 没有端口
            "http://:7890",              // 没有主机
            "http://127.0.0.1:0",        // 端口不对
            "http://127.0.0.1:70000",    // 端口不对
            "http://127.0.0.1:+80",      // 端口不对
            "http://127.0.0.1:7890/pac", // 带路径
            "http://127.0.0.1 :7890",    // 有空白
            "http://::1:7890",           // IPv6 要加方括号
            "http://代理:7890",          // 主机名不合法
        ] {
            let e = validate_proxy_url(bad).unwrap_err();
            assert!(e.contains("代理地址格式不对"), "{bad} → {e}");
        }
    }

    #[test]
    fn parses_windows_proxy_server() {
        let p = |s: &str| parse_windows_proxy_server(s);
        assert_eq!(p("127.0.0.1:10808").as_deref(), Some("http://127.0.0.1:10808"));
        assert_eq!(p(" 127.0.0.1:10808 ").as_deref(), Some("http://127.0.0.1:10808"));
        assert_eq!(
            p("http=127.0.0.1:10809;https=127.0.0.1:10810;socks=127.0.0.1:10808").as_deref(),
            Some("http://127.0.0.1:10810"),
            "优先 https="
        );
        assert_eq!(p("ftp=1.1.1.1:21;HTTP=127.0.0.1:8080").as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(p("socks=127.0.0.1:10808;127.0.0.1:8888").as_deref(), Some("http://127.0.0.1:8888"), "不带前缀的优先于 socks=");
        assert_eq!(p("socks=127.0.0.1:10808").as_deref(), Some("socks5://127.0.0.1:10808"));
        assert_eq!(p("https=http://127.0.0.1:7890").as_deref(), Some("http://127.0.0.1:7890"), "写了协议就不再补");
        assert_eq!(p("https=;http=127.0.0.1:1;").as_deref(), Some("http://127.0.0.1:1"), "空值跳过");
        for none in ["", " ; ", "ftp=1.1.1.1:21", "127.0.0.1", "https=代理"] {
            assert_eq!(p(none), None, "{none:?}");
        }
    }

    #[test]
    fn converts_windows_bypass_list() {
        let c = |s: &str| windows_bypass_to_no_proxy(s);
        // 常见代理软件写进注册表的默认绕过列表
        assert_eq!(
            c("localhost;127.*;10.*;172.16.*;172.31.*;192.168.*;*.local;<local>").as_deref(),
            Some("localhost,127.0.0.0/8,10.0.0.0/8,172.16.0.0/16,172.31.0.0/16,192.168.0.0/16,.local,127.0.0.1,::1")
        );
        assert_eq!(c("*.corp.local").as_deref(), Some(".corp.local"));
        assert_eq!(c("*.Corp.Local").as_deref(), Some(".corp.local"), "统一小写");
        assert_eq!(c("192.168.1.*").as_deref(), Some("192.168.1.0/24"));
        assert_eq!(c("10.*.*.*").as_deref(), Some("10.0.0.0/8"));
        assert_eq!(c("<LOCAL>").as_deref(), Some("localhost,127.0.0.1,::1"));
        assert_eq!(c("git.corp.local; 10.1.2.3 ;.intra").as_deref(), Some("git.corp.local,10.1.2.3,.intra"));
        assert_eq!(c("10.0.0.0/8;[::1];fd00::/8").as_deref(), Some("10.0.0.0/8,::1,fd00::/8"));
        assert_eq!(c("*").as_deref(), Some("*"));
        assert_eq!(c("localhost;127.0.0.1;<local>").as_deref(), Some("localhost,127.0.0.1,::1"), "去重");
        // 转不了的丢掉
        for bad in ["", " ; ", "*corp*", "foo.*.com", "*.*.*.*", "192.168.*.1", "300.*", "1.2.3.4.*", "host:8080", "http://git.corp", "*.", "代理.local", "10.0.0.0/33"] {
            assert_eq!(c(bad), None, "{bad:?}");
        }
        assert_eq!(c("host:8080;*.ok.com;192.168.*.1").as_deref(), Some(".ok.com"), "只丢掉转不了的那几条");
    }

    #[test]
    fn displays_proxy_without_credentials() {
        assert_eq!(display_proxy("http://user:pw@127.0.0.1:7890"), "http://127.0.0.1:7890");
        assert_eq!(display_proxy("socks5://u:p@127.0.0.1:1080"), "socks5://127.0.0.1:1080");
        assert_eq!(display_proxy("user:pw@127.0.0.1:7890"), "127.0.0.1:7890");
        assert_eq!(display_proxy("127.0.0.1:7890"), "127.0.0.1:7890");
    }

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
