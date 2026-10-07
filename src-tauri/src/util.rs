use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// 同时处理的项目数上限
pub const CONCURRENCY: usize = 4;

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

#[cfg(test)]
mod tests {
    use super::*;

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
