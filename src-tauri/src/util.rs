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
