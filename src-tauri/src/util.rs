use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// 同时处理的项目数上限（联网检查主要在等网络，常见的项目数基本可以同时进行）
pub const CONCURRENCY: usize = 8;

/// 并发执行，最多同时 `limit` 个，结果按输入顺序返回
pub async fn map_limited<T, R, F, Fut>(items: Vec<T>, limit: usize, f: F) -> Vec<R>
where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> Fut,
    Fut: Future<Output = R> + Send + 'static,
{
    let n = items.len();
    let sem = Arc::new(Semaphore::new(limit.max(1)));
    let mut set = JoinSet::new();
    for (i, item) in items.into_iter().enumerate() {
        let sem = sem.clone();
        let fut = f(item);
        set.spawn(async move {
            let _permit = sem.acquire_owned().await;
            (i, fut.await)
        });
    }
    let mut out: Vec<Option<R>> = (0..n).map(|_| None).collect();
    while let Some(joined) = set.join_next().await {
        if let Ok((i, r)) = joined {
            out[i] = Some(r);
        }
    }
    out.into_iter().flatten().collect()
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// 统一路径写法：反斜杠、去掉末尾的分隔符（盘符根目录除外）
pub fn normalize_path(p: &str) -> String {
    let s = p.trim().replace('/', "\\");
    let trimmed = s.trim_end_matches('\\');
    if trimmed.ends_with(':') {
        format!("{trimmed}\\")
    } else {
        trimmed.to_string()
    }
}

pub fn same_path(a: &str, b: &str) -> bool {
    normalize_path(a).to_lowercase() == normalize_path(b).to_lowercase()
}

/// 由路径生成稳定的项目 id（FNV-1a）
pub fn project_id(path: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in normalize_path(path).to_lowercase().bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("p-{hash:016x}")
}

pub fn folder_name(path: &str) -> String {
    Path::new(&normalize_path(path))
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

// ---------------- 自动提交的提交信息 ----------------

/// 默认的提交信息模板（与 src/lib/status.ts 的 DEFAULT_COMMIT_TEMPLATE 一致）
pub const DEFAULT_COMMIT_TEMPLATE: &str = "自动同步：{date} 来自 {host}";

/// 取不到电脑名时的显示
pub const UNKNOWN_HOST: &str = "未知电脑";

/// 按模板生成提交信息：`{date}` → 传入的时间文本，`{host}` → 电脑名。
/// 模板去掉空白后为空就用默认模板。
pub fn render_commit_message(template: &str, date: &str, host: &str) -> String {
    let t = template.trim();
    let t = if t.is_empty() { DEFAULT_COMMIT_TEMPLATE } else { t };
    t.replace("{date}", date).replace("{host}", host)
}

/// 电脑名：Windows 取环境变量 COMPUTERNAME，取不到用"未知电脑"
pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| UNKNOWN_HOST.to_string())
}

/// 用本地当前时间（YYYY-MM-DD HH:mm）和电脑名生成提交信息
pub fn commit_message_now(template: &str) -> String {
    let date = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    render_commit_message(template, &date, &computer_name())
}

// ---------------- Windows 系统代理 ----------------

/// Windows 系统代理
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemProxy {
    /// git 能用的代理地址
    pub addr: String,
    /// 绕过列表（ProxyOverride）转成的 NO_PROXY；转不出来时为 None
    pub no_proxy: Option<String>,
}

/// 读取 Windows 的系统代理（HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings）；
/// 没开、用了 PAC 或读不到时返回 None
#[cfg(windows)]
pub fn windows_system_proxy() -> Option<SystemProxy> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()?;
    let enable: u32 = key.get_value("ProxyEnable").unwrap_or(0);
    let server: Option<String> = key.get_value("ProxyServer").ok();
    let pac: Option<String> = key.get_value("AutoConfigURL").ok();
    let bypass: Option<String> = key.get_value("ProxyOverride").ok();
    system_proxy_from(enable, server.as_deref(), pac.as_deref(), bypass.as_deref())
}

#[cfg(not(windows))]
pub fn windows_system_proxy() -> Option<SystemProxy> {
    None
}

/// 由注册表里的几个值决定系统代理：
/// - 用了 PAC（AutoConfigURL 非空）→ 不支持，当作没有系统代理
/// - ProxyEnable 不是 1，或 ProxyServer 为空 / 解析不出地址 → 没有
/// - 否则按 `parse_windows_proxy_server` 转成代理地址，绕过列表（ProxyOverride）转成 NO_PROXY
pub fn system_proxy_from(
    enable: u32,
    server: Option<&str>,
    auto_config_url: Option<&str>,
    bypass: Option<&str>,
) -> Option<SystemProxy> {
    if auto_config_url.is_some_and(|u| !u.trim().is_empty()) || enable != 1 {
        return None;
    }
    let addr = crate::url::parse_windows_proxy_server(server?)?;
    Some(SystemProxy { addr, no_proxy: bypass.and_then(crate::url::windows_bypass_to_no_proxy) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_proxy_rules() {
        let addr = |enable, server, pac| system_proxy_from(enable, server, pac, None).map(|p| p.addr);
        let server = Some("127.0.0.1:10808");
        assert_eq!(addr(1, server, None).as_deref(), Some("http://127.0.0.1:10808"));
        assert_eq!(addr(1, server, Some("")).as_deref(), Some("http://127.0.0.1:10808"), "空的 PAC 地址不算");
        assert_eq!(addr(0, server, None), None, "系统代理没开");
        assert_eq!(addr(1, None, None), None, "开了但没填地址");
        assert_eq!(addr(1, Some(""), None), None);
        assert_eq!(addr(1, server, Some("http://127.0.0.1:7890/pac")), None, "用了 PAC 当作没有");
        assert_eq!(addr(1, Some("http=127.0.0.1:1;socks=127.0.0.1:2"), None).as_deref(), Some("http://127.0.0.1:1"));

        // 绕过列表转成 NO_PROXY；没有或转不出来时为 None
        assert_eq!(
            system_proxy_from(1, server, None, Some("localhost;192.168.*;*.corp.local;<local>")),
            Some(SystemProxy {
                addr: "http://127.0.0.1:10808".into(),
                no_proxy: Some("localhost,192.168.0.0/16,.corp.local,127.0.0.1,::1".into()),
            })
        );
        assert_eq!(system_proxy_from(1, server, None, Some("")).unwrap().no_proxy, None);
        assert_eq!(system_proxy_from(1, server, None, Some("host:8080")).unwrap().no_proxy, None);
        assert_eq!(system_proxy_from(0, server, None, Some("localhost")), None);
    }

    #[test]
    fn renders_placeholders() {
        assert_eq!(
            render_commit_message(DEFAULT_COMMIT_TEMPLATE, "2026-10-07 14:30", "DESKTOP-1"),
            "自动同步：2026-10-07 14:30 来自 DESKTOP-1"
        );
        assert_eq!(render_commit_message("{host}/{host} {date}", "D", "H"), "H/H D");
        assert_eq!(render_commit_message("  同步笔记  ", "D", "H"), "同步笔记");
    }

    #[test]
    fn empty_template_falls_back_to_default() {
        let want = "自动同步：D 来自 H";
        assert_eq!(render_commit_message("", "D", "H"), want);
        assert_eq!(render_commit_message(" \t\n ", "D", "H"), want);
    }

    #[test]
    fn message_now_has_date_format() {
        let msg = commit_message_now("{date}");
        // YYYY-MM-DD HH:mm
        assert_eq!(msg.len(), 16, "{msg}");
        assert_eq!(&msg[4..5], "-");
        assert_eq!(&msg[10..11], " ");
        assert_eq!(&msg[13..14], ":");
        assert!(!computer_name().is_empty());
    }
}
