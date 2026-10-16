//! Tauri 命令：与前端 src/api/tauri.ts 一一对应。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::ipc::Channel;
use tauri::State;

use crate::config::{AppConfig, ConfigStore, StoredProject};
use crate::model::*;
use crate::ops::{self, Checked, SyncOne};
use crate::status::{inspect, Inspection};
use crate::util::{
    commit_message_now, folder_name, map_limited, normalize_path, now_iso, project_id, same_path, CONCURRENCY,
    DEFAULT_COMMIT_TEMPLATE,
};
use crate::git::{
    refresh_timeout, set_network_timeout, set_proxy_override, ProxyOverride, DEFAULT_NETWORK_TIMEOUT_SECS,
    NETWORK_TIMEOUT_CHOICES,
};
use crate::url::{display_proxy, validate_proxy_url};
use crate::{scan, system, util};

pub struct AppState {
    pub config: ConfigStore,
    /// 每个项目最近一次检查的结果（内存），检查失败时用它保留上一次的状态
    cache: Mutex<HashMap<String, Project>>,
    /// 每个项目最近一次联网检查（fetch）成功的时间，刷新和一键同步都会记；一键同步据此跳过刚 fetch 过的项目
    checked_at: Mutex<HashMap<String, Instant>>,
    /// 联网操作（刷新、一键同步、提交推送）互斥，避免同一个仓库被同时 fetch / push
    net_lock: tokio::sync::Mutex<()>,
    syncing: AtomicBool,
    /// 代理解析互斥：保存设置和刷新 / 同步同时解析时，保证最后生效的是按最新配置算出的结果
    proxy_lock: tokio::sync::Mutex<()>,
}

pub type Shared = Arc<AppState>;

impl AppState {
    pub fn new(config: ConfigStore) -> Self {
        set_network_timeout(config.read(network_timeout_of));
        AppState {
            config,
            cache: Mutex::new(HashMap::new()),
            checked_at: Mutex::new(HashMap::new()),
            net_lock: tokio::sync::Mutex::new(()),
            syncing: AtomicBool::new(false),
            proxy_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn cached(&self, id: &str) -> Option<Project> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned()
    }

    fn remember(&self, p: &Project) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(p.id.clone(), p.clone());
    }

    fn forget(&self, id: &str) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        self.checked_at.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    }

    /// 记下"刚刚联网检查成功"
    fn mark_checked(&self, id: &str) {
        self.checked_at.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), Instant::now());
    }

    fn last_checked(&self, id: &str) -> Option<Instant> {
        self.checked_at.lock().unwrap_or_else(|e| e.into_inner()).get(id).copied()
    }

    fn stored(&self, id: &str) -> Option<StoredProject> {
        self.config.read(|c| c.projects.iter().find(|p| p.id == id).cloned())
    }

    /// 更新某个项目的持久化字段，返回更新后的记录
    fn update_stored(&self, sp: &StoredProject, f: impl FnOnce(&mut StoredProject)) -> StoredProject {
        self.config
            .update(|c| match c.projects.iter_mut().find(|p| p.id == sp.id) {
                Some(entry) => {
                    f(entry);
                    entry.clone()
                }
                None => sp.clone(), // 处理过程中被取消监控了
            })
            .unwrap_or_else(|_| sp.clone())
    }
}

// ---------------- 组装 Project ----------------

fn project_from(sp: &StoredProject, ins: &Inspection) -> Project {
    Project {
        id: sp.id.clone(),
        name: folder_name(&sp.path),
        path: sp.path.clone(),
        branch: ins.branch.clone(),
        remote_url: ins.remote_url.clone(),
        upstream: ins.upstream.clone(),
        status: ins.status,
        ahead: ins.ahead,
        behind: ins.behind,
        changes: ins.changes.clone(),
        last_sync_at: sp.last_sync_at.clone(),
        issue: sp.issue.clone(),
        check_error: None,
        auto_commit: sp.auto_commit,
    }
}

/// 检查失败：保留上一次的状态，附上失败原因
fn with_error(prev: Option<Project>, sp: &StoredProject, error: String) -> Project {
    let mut p = prev.unwrap_or_else(|| Project {
        id: sp.id.clone(),
        name: folder_name(&sp.path),
        path: sp.path.clone(),
        branch: "—".to_string(),
        remote_url: None,
        upstream: None,
        status: SyncStatus::NoRemote,
        ahead: 0,
        behind: 0,
        changes: Vec::new(),
        last_sync_at: None,
        issue: None,
        check_error: None,
        auto_commit: false,
    });
    p.last_sync_at = sp.last_sync_at.clone();
    p.issue = sp.issue.clone();
    p.auto_commit = sp.auto_commit;
    p.check_error = Some(error);
    p
}

/// 本地检查（不联网）
async fn local_project(st: &AppState, sp: &StoredProject) -> Project {
    let prev = st.cached(&sp.id);
    let p = match inspect(Path::new(&sp.path)).await {
        Ok(ins) => {
            let mut p = project_from(sp, &ins);
            // 本地检查不涉及网络，保留上一次联网检查的失败原因
            p.check_error = prev.and_then(|x| x.check_error);
            p
        }
        Err(e) => with_error(prev, sp, e.message()),
    };
    st.remember(&p);
    p
}

/// 联网检查（刷新状态用）：fetch + status，fetch 用刷新的短超时
async fn remote_project(st: &AppState, sp: &StoredProject) -> Project {
    let prev = st.cached(&sp.id);
    let mut p = match ops::check(Path::new(&sp.path), refresh_timeout()).await {
        Checked::Fresh(ins) => {
            st.mark_checked(&sp.id);
            project_from(sp, &ins)
        }
        // fetch 失败：状态停留在上一次 fetch 时的结果（本地的远程跟踪分支没变）
        Checked::FetchFailed { local, error } => with_error(Some(project_from(sp, &local)), sp, error.message()),
        Checked::Failed(error) => with_error(prev, sp, error.message()),
    };
    // 刷新检测到已同步：清除遗留的异常标记
    if p.status == SyncStatus::Synced && p.check_error.is_none() && p.issue.is_some() {
        st.update_stored(sp, |e| e.issue = None);
        p.issue = None;
    }
    st.remember(&p);
    p
}

// ---------------- 代理 ----------------

/// 设置里的代理模式
const PROXY_MODES: [&str; 3] = ["auto", "off", "manual"];

/// 解析出的代理：交给 run_git 的覆盖方式，以及设置页显示的来源和地址（已去掉账号密码）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedProxy {
    pub over: ProxyOverride,
    pub source: ProxySource,
    pub effective: Option<String>,
}

/// 按代理模式决定 SyncDeck 调用的 git 用什么代理。纯函数，探测结果由调用方传入：
/// - `off`：完全不走代理
/// - `manual`：用手动填写的地址（`manual_url` 为已校验的地址；没有时按自动处理）
/// - `auto`：git 自己配了代理（`git_proxy`，Some("") 表示 git 配置成了不用代理）→ 不覆盖；
///   没配而系统代理开着（`system_proxy`）→ 用系统代理；都没有 → 不覆盖，也就是不用代理
pub fn decide_proxy(
    mode: &str,
    manual_url: Option<&str>,
    git_proxy: Option<&str>,
    system_proxy: Option<&str>,
) -> ResolvedProxy {
    let none = |over| ResolvedProxy { over, source: ProxySource::NoProxy, effective: None };
    match (mode, manual_url) {
        ("off", _) => none(ProxyOverride::Disable),
        ("manual", Some(url)) => ResolvedProxy {
            over: ProxyOverride::Use(url.to_string()),
            source: ProxySource::Manual,
            effective: Some(display_proxy(url)),
        },
        _ => match (git_proxy.map(str::trim), system_proxy) {
            (Some(""), _) => none(ProxyOverride::Inherit),
            (Some(own), _) => ResolvedProxy {
                over: ProxyOverride::Inherit,
                source: ProxySource::Git,
                effective: Some(display_proxy(own)),
            },
            (None, Some(sys)) => ResolvedProxy {
                over: ProxyOverride::Use(sys.to_string()),
                source: ProxySource::System,
                effective: Some(display_proxy(sys)),
            },
            (None, None) => none(ProxyOverride::Inherit),
        },
    }
}

/// 配置里的代理模式；没设置过或不认识的值都当作 auto
fn proxy_mode_of(c: &AppConfig) -> &'static str {
    match c.settings.proxy_mode.as_deref() {
        Some("off") => "off",
        Some("manual") => "manual",
        _ => "auto",
    }
}

/// 按当前配置重新解析代理并立即生效（只影响 SyncDeck 调用的 git）。
/// 每轮刷新 / 同步 / 提交推送开始前、读取和保存设置时调用：用户可能中途开关了代理软件。
async fn refresh_proxy(st: &AppState) -> ResolvedProxy {
    let _guard = st.proxy_lock.lock().await;
    let (mode, url) = st.config.read(|c| (proxy_mode_of(c), c.settings.proxy_url.clone()));
    let manual = url.as_deref().and_then(|u| validate_proxy_url(u).ok());
    let auto = mode == "auto" || (mode == "manual" && manual.is_none());
    let git_proxy = if auto { ops::git_own_proxy().await } else { None };
    let system_proxy = if auto && git_proxy.is_none() { util::windows_system_proxy() } else { None };
    let resolved = decide_proxy(mode, manual.as_deref(), git_proxy.as_deref(), system_proxy.as_deref());
    set_proxy_override(resolved.over.clone());
    resolved
}

// ---------------- 命令 ----------------

#[tauri::command]
pub async fn check_git() -> GitInfo {
    match ops::git_version().await {
        Ok(v) => GitInfo { installed: true, version: Some(v) },
        Err(_) => GitInfo { installed: false, version: None },
    }
}

#[tauri::command]
pub async fn list_projects(state: State<'_, Shared>) -> Result<Vec<Project>, String> {
    let st = state.inner().clone();
    let stored = st.config.read(|c| c.projects.clone());
    Ok(map_limited(stored, CONCURRENCY, move |sp| {
        let st = st.clone();
        async move { local_project(&st, &sp).await }
    })
    .await)
}

/// 联网刷新所有项目：每个项目检查完就通过 `on_event` 发一次 Done，最后仍返回完整列表
#[tauri::command]
pub async fn refresh_status(
    state: State<'_, Shared>,
    on_event: Channel<RefreshProgressEvent>,
) -> Result<Vec<Project>, String> {
    let st = state.inner().clone();
    let _net = st.net_lock.lock().await;
    refresh_proxy(&st).await;
    let stored = st.config.read(|c| c.projects.clone());
    let total = stored.len();
    let finished = Arc::new(AtomicUsize::new(0));
    let st2 = st.clone();
    Ok(map_limited(stored, CONCURRENCY, move |sp| {
        let st = st2.clone();
        let on_event = on_event.clone();
        let finished = finished.clone();
        async move {
            let project = remote_project(&st, &sp).await;
            let index = finished.fetch_add(1, Ordering::SeqCst);
            let _ = on_event.send(RefreshProgressEvent::Done { project: project.clone(), index, total });
            project
        }
    })
    .await)
}

#[tauri::command]
pub async fn add_projects(state: State<'_, Shared>, paths: Vec<String>) -> Result<Vec<Project>, String> {
    let st = state.inner().clone();
    let added = st.config.update(|c| {
        let mut added = Vec::new();
        for raw in &paths {
            let path = normalize_path(raw);
            if c.projects.iter().any(|p| same_path(&p.path, &path)) || !scan::is_repo(Path::new(&path)) {
                continue;
            }
            let sp = StoredProject {
                id: project_id(&path),
                path,
                last_sync_at: None,
                issue: None,
                auto_commit: false,
            };
            c.projects.push(sp.clone());
            added.push(sp);
        }
        added
    })?;
    Ok(map_limited(added, CONCURRENCY, move |sp| {
        let st = st.clone();
        async move { local_project(&st, &sp).await }
    })
    .await)
}

/// 取消监控：只从清单里移除，不碰任何文件
#[tauri::command]
pub async fn remove_project(state: State<'_, Shared>, id: String) -> Result<(), String> {
    state.config.update(|c| c.projects.retain(|p| p.id != id))?;
    state.forget(&id);
    Ok(())
}

/// 打开 / 关闭某个项目的"一键同步时自动提交"，返回最新的项目（本地检查，不联网）
#[tauri::command]
pub async fn set_auto_commit(state: State<'_, Shared>, project_id: String, enabled: bool) -> Result<Project, String> {
    let st = state.inner().clone();
    let sp = st
        .config
        .update(|c| {
            c.projects.iter_mut().find(|p| p.id == project_id).map(|p| {
                p.auto_commit = enabled;
                p.clone()
            })
        })?
        .ok_or("项目不存在")?;
    Ok(local_project(&st, &sp).await)
}

#[tauri::command]
pub async fn scan_directory(state: State<'_, Shared>, dir: String) -> Result<Vec<ScannedRepo>, String> {
    let root = PathBuf::from(normalize_path(&dir));
    if !root.is_dir() {
        return Err(format!("目录不存在：{dir}"));
    }
    let repos = tokio::task::spawn_blocking(move || scan::find_repos(&root))
        .await
        .map_err(|e| format!("扫描失败：{e}"))?;
    let monitored = Arc::new(state.config.read(|c| c.projects.clone()));
    Ok(map_limited(repos, CONCURRENCY, move |path| {
        let monitored = monitored.clone();
        async move {
            let path_str = path.to_string_lossy().into_owned();
            let (branch, remote_url) = ops::repo_brief(&path).await;
            let hit = monitored.iter().find(|m| same_path(&m.path, &path_str));
            ScannedRepo {
                name: folder_name(&path_str),
                branch,
                remote_url,
                monitored: hit.is_some(),
                project_id: hit.map(|m| m.id.clone()),
                path: path_str,
            }
        }
    })
    .await)
}

/// 同步结束（包括出错返回）时清除"正在同步"标记
struct SyncingFlag(Shared);
impl Drop for SyncingFlag {
    fn drop(&mut self) {
        self.0.syncing.store(false, Ordering::SeqCst);
    }
}

#[tauri::command]
pub async fn sync_all(
    state: State<'_, Shared>,
    on_event: Channel<SyncProgressEvent>,
) -> Result<SyncResult, String> {
    let st = state.inner().clone();
    if st.syncing.swap(true, Ordering::SeqCst) {
        return Err("正在同步中，请等这一轮结束".into());
    }
    let _flag = SyncingFlag(st.clone());
    let _net = st.net_lock.lock().await;
    refresh_proxy(&st).await;

    let started_at = now_iso();
    let mut stored = st.config.read(|c| c.projects.clone());
    // 处理顺序与界面一致：需要处理的在前
    let weight = |sp: &StoredProject| st.cached(&sp.id).map_or(6, |p| p.status.weight());
    stored.sort_by(|a, b| weight(a).cmp(&weight(b)).then_with(|| folder_name(&a.path).cmp(&folder_name(&b.path))));

    // 自动提交的提交信息：本轮所有项目共用同一个时间
    let commit_message = Arc::new(commit_message_now(&st.config.read(commit_template_of)));

    let total = stored.len();
    let started = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicUsize::new(0));
    let st2 = st.clone();
    let items = map_limited(stored, CONCURRENCY, move |sp| {
        let st = st2.clone();
        let on_event = on_event.clone();
        let started = started.clone();
        let finished = finished.clone();
        let commit_message = commit_message.clone();
        async move {
            let index = started.fetch_add(1, Ordering::SeqCst);
            let _ = on_event.send(SyncProgressEvent::Start { project_id: sp.id.clone(), index, total });
            let opts = sync_options_for(&st, &sp, &commit_message);
            let one = ops::sync_one(Path::new(&sp.path), &opts).await;
            let (project, result) = record_sync(&st, &sp, &opts, one);
            let index = finished.fetch_add(1, Ordering::SeqCst);
            let _ = on_event.send(SyncProgressEvent::Done { result: result.clone(), project, index, total });
            result
        }
    })
    .await;

    let failed_count = items.iter().filter(|i| i.outcome == SyncOutcome::Failed).count();
    Ok(SyncResult {
        started_at,
        finished_at: now_iso(),
        synced_count: items.len() - failed_count,
        failed_count,
        items,
    })
}

/// 一键同步时复用检查结果的时间窗口：这段时间内联网检查成功过的项目不再 fetch
pub const REUSE_CHECK_WITHIN: Duration = Duration::from_secs(60);

/// 一键同步要不要跳过 fetch：最近一次联网检查成功距现在不到 `REUSE_CHECK_WITHIN`
pub fn should_skip_fetch(last_checked: Option<Instant>, now: Instant) -> bool {
    last_checked.is_some_and(|t| now.saturating_duration_since(t) < REUSE_CHECK_WITHIN)
}

/// 一键同步时单个项目的选项。**唯一的构造入口**：`sync_all` 只通过这里拿选项，以后加选项也只改这里。
/// `commit_message` 是本轮共用的自动提交信息，只给打开了自动提交的项目用。
fn sync_options_for(st: &AppState, sp: &StoredProject, commit_message: &str) -> ops::SyncOptions {
    ops::SyncOptions {
        auto_commit: sp.auto_commit.then(|| commit_message.to_string()),
        skip_fetch: should_skip_fetch(st.last_checked(&sp.id), Instant::now()),
    }
}

/// 记录单个项目的同步结果：成功（含自动提交并推送）→ 更新上次同步时间、清除异常；失败 → 保存异常原因。
/// 这次真的 fetch 成功了，就记下检查时间（跳过 fetch 时不记，免得一直沿用旧结果）。
fn record_sync(st: &AppState, sp: &StoredProject, opts: &ops::SyncOptions, one: SyncOne) -> (Project, SyncItemResult) {
    if !opts.skip_fetch && !one.check_failed {
        st.mark_checked(&sp.id);
    }
    let success = one.outcome != SyncOutcome::Failed;
    let reason = one.reason.clone();
    let now = now_iso();
    let updated = st.update_stored(sp, |e| {
        if success {
            e.last_sync_at = Some(now);
            e.issue = None;
        } else {
            e.issue = reason.clone();
        }
    });
    let project = match (&one.inspection, one.check_failed) {
        (Some(ins), false) => project_from(&updated, ins),
        (Some(ins), true) => with_error(Some(project_from(&updated, ins)), &updated, reason.clone().unwrap_or_default()),
        (None, _) => with_error(st.cached(&sp.id), &updated, reason.clone().unwrap_or_default()),
    };
    st.remember(&project);
    let result = SyncItemResult { project_id: sp.id.clone(), outcome: one.outcome, commits: one.commits, reason };
    (project, result)
}

#[tauri::command]
pub async fn commit_and_push(
    state: State<'_, Shared>,
    project_id: String,
    message: String,
) -> Result<Project, String> {
    let st = state.inner().clone();
    let _net = st.net_lock.lock().await;
    refresh_proxy(&st).await;
    let sp = st.stored(&project_id).ok_or("项目不存在")?;
    let dir = PathBuf::from(&sp.path);
    match ops::commit_and_push(&dir, &message).await {
        Ok(ins) => {
            let updated = st.update_stored(&sp, |e| {
                e.last_sync_at = Some(now_iso());
                e.issue = None;
            });
            let p = project_from(&updated, &ins);
            st.remember(&p);
            Ok(p)
        }
        Err(msg) => {
            // 可能已经提交到本地，刷新一下缓存里的状态
            if let Ok(ins) = inspect(&dir).await {
                st.remember(&project_from(&sp, &ins));
            }
            Err(msg)
        }
    }
}

#[tauri::command]
pub async fn get_diagnostic_info(state: State<'_, Shared>, project_id: String) -> Result<DiagnosticInfo, String> {
    let sp = state.stored(&project_id).ok_or("项目不存在")?;
    let dir = PathBuf::from(&sp.path);
    let prev = state.cached(&sp.id);
    let project = match inspect(&dir).await {
        Ok(ins) => {
            let mut p = project_from(&sp, &ins);
            p.check_error = prev.and_then(|x| x.check_error);
            p
        }
        Err(e) => with_error(prev, &sp, e.message()),
    };
    let recent_commits = ops::recent_commits(&dir).await;
    Ok(DiagnosticInfo { project, recent_commits })
}

#[tauri::command]
pub async fn open_in_terminal(state: State<'_, Shared>, project_id: String) -> Result<String, String> {
    let sp = state.stored(&project_id).ok_or("项目不存在")?;
    system::open_terminal(Path::new(&sp.path))?;
    Ok(sp.path)
}

#[tauri::command]
pub async fn open_in_editor(state: State<'_, Shared>, project_id: String) -> Result<String, String> {
    let sp = state.stored(&project_id).ok_or("项目不存在")?;
    system::open_editor(Path::new(&sp.path))?;
    Ok(sp.path)
}

fn settings_of(c: &AppConfig, proxy: &ResolvedProxy) -> Settings {
    Settings {
        default_code_dir: c
            .settings
            .default_code_dir
            .clone()
            .unwrap_or_else(|| std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into())),
        theme: c.settings.theme.clone(),
        commit_template: commit_template_of(c),
        network_timeout_secs: network_timeout_of(c),
        proxy_mode: proxy_mode_of(c).to_string(),
        proxy_url: c.settings.proxy_url.clone(),
        effective_proxy: proxy.effective.clone(),
        proxy_source: proxy.source,
    }
}

/// 联网超时：没设置过或配置文件里是不认识的值时用默认值
fn network_timeout_of(c: &AppConfig) -> u64 {
    c.settings
        .network_timeout_secs
        .filter(|s| NETWORK_TIMEOUT_CHOICES.contains(s))
        .unwrap_or(DEFAULT_NETWORK_TIMEOUT_SECS)
}

/// 提交信息模板：没设置过或为空时用默认模板
fn commit_template_of(c: &AppConfig) -> String {
    c.settings
        .commit_template
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(DEFAULT_COMMIT_TEMPLATE)
        .to_string()
}

/// 读取设置；顺带重新解析一次代理，设置页显示的"当前代理"是此刻的实际情况
#[tauri::command]
pub async fn get_settings(state: State<'_, Shared>) -> Result<Settings, String> {
    let proxy = refresh_proxy(&state).await;
    Ok(state.config.read(|c| settings_of(c, &proxy)))
}

#[tauri::command]
pub async fn save_settings(state: State<'_, Shared>, patch: SettingsPatch) -> Result<Settings, String> {
    if let Some(t) = &patch.theme {
        if t != "light" && t != "dark" {
            return Err(format!("未知的主题：{t}"));
        }
    }
    if let Some(secs) = patch.network_timeout_secs {
        if !NETWORK_TIMEOUT_CHOICES.contains(&secs) {
            return Err(format!("不支持的联网超时：{secs} 秒"));
        }
    }
    // 代理：先算出保存后的模式和地址并校验，不合法时什么都不改
    let proxy_patch = if patch.proxy_mode.is_some() || patch.proxy_url.is_some() {
        let (cur_mode, cur_url) = state.config.read(|c| (proxy_mode_of(c), c.settings.proxy_url.clone()));
        let mode = match patch.proxy_mode.as_deref() {
            None => cur_mode,
            Some(m) => *PROXY_MODES.iter().find(|x| **x == m).ok_or_else(|| format!("未知的代理模式：{m}"))?,
        };
        let url = match patch.proxy_url.as_deref().map(str::trim) {
            None => cur_url,
            Some("") => None,
            Some(u) => Some(validate_proxy_url(u)?),
        };
        if mode == "manual" {
            match &url {
                None => return Err("选择手动填写时，请先填好代理地址".into()),
                Some(u) => {
                    validate_proxy_url(u)?;
                }
            }
        }
        Some((mode, url))
    } else {
        None
    };
    state.config.update(|c| {
        if let Some((mode, url)) = &proxy_patch {
            c.settings.proxy_mode = (*mode != "auto").then(|| mode.to_string());
            c.settings.proxy_url = url.clone();
        }
        if let Some(dir) = &patch.default_code_dir {
            c.settings.default_code_dir = Some(normalize_path(dir));
        }
        if let Some(t) = &patch.theme {
            c.settings.theme = Some(t.clone());
        }
        if let Some(t) = &patch.commit_template {
            // 空模板或等于默认模板时不单独保存，读取时回落为默认模板
            let t = t.trim();
            c.settings.commit_template =
                if t.is_empty() || t == DEFAULT_COMMIT_TEMPLATE { None } else { Some(t.to_string()) };
        }
        if let Some(secs) = patch.network_timeout_secs {
            c.settings.network_timeout_secs = (secs != DEFAULT_NETWORK_TIMEOUT_SECS).then_some(secs);
        }
        set_network_timeout(network_timeout_of(c));
    })?;
    let proxy = refresh_proxy(&state).await;
    Ok(state.config.read(|c| settings_of(c, &proxy)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_fetch_only_right_after_successful_check() {
        // 用"基准时间 + 偏移"构造，避免 Instant 减法在刚开机时下溢
        let base = Instant::now();
        let now = base + Duration::from_secs(600);
        let ago = |secs: u64| Some(now - Duration::from_secs(secs));
        assert!(!should_skip_fetch(None, now), "从没检查成功过 → 要 fetch");
        assert!(should_skip_fetch(ago(0), now));
        assert!(should_skip_fetch(ago(10), now));
        assert!(should_skip_fetch(ago(59), now));
        assert!(!should_skip_fetch(ago(60), now), "满 1 分钟 → 要 fetch");
        assert!(!should_skip_fetch(ago(300), now));
        // 检查时间比 now 还晚（并行时先取了 now）：算刚检查过
        assert!(should_skip_fetch(Some(now + Duration::from_secs(1)), now));
    }

    #[test]
    fn proxy_decisions() {
        let r = |over: ProxyOverride, source: ProxySource, effective: Option<&str>| ResolvedProxy {
            over,
            source,
            effective: effective.map(str::to_string),
        };
        let git = Some("socks5://127.0.0.1:10808");
        let sys = Some("http://127.0.0.1:10809");

        // 自动：git 自己配了代理 → 不覆盖
        assert_eq!(
            decide_proxy("auto", None, git, sys),
            r(ProxyOverride::Inherit, ProxySource::Git, Some("socks5://127.0.0.1:10808"))
        );
        // 自动：git 没配，系统代理开着 → 用系统代理
        assert_eq!(
            decide_proxy("auto", None, None, sys),
            r(ProxyOverride::Use("http://127.0.0.1:10809".into()), ProxySource::System, Some("http://127.0.0.1:10809"))
        );
        // 自动：都没有 → 不覆盖、不用代理
        assert_eq!(decide_proxy("auto", None, None, None), r(ProxyOverride::Inherit, ProxySource::NoProxy, None));
        // 自动：git 配置成空值（明确不用代理）→ 尊重 git 的配置，不套系统代理
        assert_eq!(decide_proxy("auto", None, Some(""), sys), r(ProxyOverride::Inherit, ProxySource::NoProxy, None));
        // 显示的地址去掉账号密码，交给 git 的保留原样
        assert_eq!(
            decide_proxy("auto", None, None, Some("http://u:p@127.0.0.1:1")),
            r(ProxyOverride::Use("http://u:p@127.0.0.1:1".into()), ProxySource::System, Some("http://127.0.0.1:1"))
        );
        assert_eq!(
            decide_proxy("auto", None, Some("user:pw@127.0.0.1:2"), None),
            r(ProxyOverride::Inherit, ProxySource::Git, Some("127.0.0.1:2"))
        );

        // 不用代理：不管 git 和系统怎么配，都完全不走代理
        assert_eq!(decide_proxy("off", None, git, sys), r(ProxyOverride::Disable, ProxySource::NoProxy, None));

        // 手动：用填写的地址
        assert_eq!(
            decide_proxy("manual", Some("http://127.0.0.1:7890"), git, sys),
            r(ProxyOverride::Use("http://127.0.0.1:7890".into()), ProxySource::Manual, Some("http://127.0.0.1:7890"))
        );
        // 手动但没有可用的地址（配置被手动改坏）→ 按自动处理
        assert_eq!(decide_proxy("manual", None, None, sys), decide_proxy("auto", None, None, sys));
    }
}
