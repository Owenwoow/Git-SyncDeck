//! Tauri 命令：与前端 src/api/tauri.ts 一一对应。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use tauri::ipc::Channel;
use tauri::State;

use crate::autosync::{self, AutoSync, Switches};
use crate::config::{AppConfig, ConfigStore, StoredProject};
use crate::model::*;
use crate::ops::{self, Checked, SyncOne};
use crate::status::{inspect, Inspection};
use crate::util::{
    commit_message_now, folder_name, map_limited, normalize_path, now_iso, project_id, same_path, CONCURRENCY,
    DEFAULT_COMMIT_TEMPLATE,
};
use crate::git::{
    proxy_from_env, refresh_timeout, set_network_timeout, set_proxy_plan, ProxyOverride, ProxyPlan,
    DEFAULT_NETWORK_TIMEOUT_SECS, NETWORK_TIMEOUT_CHOICES,
};
use crate::github::{self, GhStatus};
use crate::ops::GitProxyConfig;
use crate::util::SystemProxy;
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
    /// 最近一轮同步（一键同步或后台同步）到现在的进度：托盘显示"正在同步 3/7"，窗口中途打开时补上后台同步的进度
    round: Mutex<SyncProgressSnapshot>,
    /// 代理解析互斥：保存设置和刷新 / 同步同时解析时，保证最后生效的是按最新配置算出的结果
    proxy_lock: tokio::sync::Mutex<()>,
    /// 换电脑自动同步的调度状态（上次离开 / 到达同步的时间、正在进行的后台同步）
    pub auto: AutoSync,
    /// 项目状态、"正在同步"或暂停状态变化后调用（托盘据此更新图标）；测试里不设置
    on_change: OnceLock<Box<dyn Fn() + Send + Sync>>,
    /// 注册 / 取消开机自启（需要 AppHandle，由 lib.rs 设置）；测试里不设置
    autostart: OnceLock<Box<dyn Fn(bool) -> Result<(), String> + Send + Sync>>,
}

pub type Shared = Arc<AppState>;

/// 一键同步的进度回调（前端的 Channel，或后台同步时什么都不做）
pub type SyncSink = Arc<dyn Fn(SyncProgressEvent) + Send + Sync>;
/// 刷新状态的进度回调
pub type RefreshSink = Arc<dyn Fn(RefreshProgressEvent) + Send + Sync>;

/// 已经有一轮同步在进行时 `run_sync` 返回的错误
pub const SYNC_BUSY: &str = "正在同步中，请等这一轮结束";

impl AppState {
    pub fn new(config: ConfigStore) -> Self {
        set_network_timeout(config.read(network_timeout_of));
        util::set_host_name(config.read(|c| c.settings.host_name.clone()).as_deref());
        AppState {
            config,
            cache: Mutex::new(HashMap::new()),
            checked_at: Mutex::new(HashMap::new()),
            net_lock: tokio::sync::Mutex::new(()),
            syncing: AtomicBool::new(false),
            round: Mutex::new(SyncProgressSnapshot::default()),
            proxy_lock: tokio::sync::Mutex::new(()),
            auto: AutoSync::new(),
            on_change: OnceLock::new(),
            autostart: OnceLock::new(),
        }
    }

    /// 设置"状态变化"回调（只能设置一次）
    pub fn set_change_hook(&self, f: impl Fn() + Send + Sync + 'static) {
        let _ = self.on_change.set(Box::new(f));
    }

    /// 设置"注册 / 取消开机自启"的实现（只能设置一次）
    pub fn set_autostart_hook(&self, f: impl Fn(bool) -> Result<(), String> + Send + Sync + 'static) {
        let _ = self.autostart.set(Box::new(f));
    }

    /// 通知托盘等：状态变了。调用时不能持有任何锁
    pub fn changed(&self) {
        if let Some(f) = self.on_change.get() {
            f();
        }
    }

    /// 是否有一轮同步正在进行（窗口里的一键同步或后台自动同步）
    pub fn is_syncing(&self) -> bool {
        self.syncing.load(Ordering::SeqCst)
    }

    /// 最近一轮同步的进度
    pub fn round_snapshot(&self) -> SyncProgressSnapshot {
        self.round.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 后台同步开始时清掉上一轮的进度（还没开始联网时窗口打开，看到的是"准备中"而不是上一轮的结果）
    pub fn reset_round(&self) {
        *self.round.lock().unwrap_or_else(|e| e.into_inner()) = SyncProgressSnapshot::default();
    }

    /// 记下一条同步进度事件。项目数变了（开始、完成、等重试）时通知托盘更新悬停文字；百分比不通知，太频繁
    pub fn note_progress(&self, e: &SyncProgressEvent) {
        self.round.lock().unwrap_or_else(|e| e.into_inner()).apply(e);
        if !matches!(e, SyncProgressEvent::Progress { .. } | SyncProgressEvent::Start { .. }) {
            self.changed();
        }
    }

    /// 所有监控中的项目最近一次的检查结果（按清单顺序；还没检查过的不在里面）
    pub fn snapshot(&self) -> Vec<Project> {
        let ids: Vec<String> = self.config.read(|c| c.projects.iter().map(|p| p.id.clone()).collect());
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        ids.iter().filter_map(|id| cache.get(id).cloned()).collect()
    }

    fn cached(&self, id: &str) -> Option<Project> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned()
    }

    fn remember(&self, p: &Project) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(p.id.clone(), p.clone());
        self.changed();
    }

    fn forget(&self, id: &str) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        self.checked_at.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
        self.changed();
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

/// 联网检查（刷新状态用）：fetch + status，fetch 用刷新的短超时。
/// 检查要等联网，期间用户可能改了"自动提交"开关或取消了监控：检查完后重新读持久化字段再组装结果；
/// 已经取消监控的返回 None（不再记缓存和检查时间）。
async fn remote_project(st: &AppState, sp: &StoredProject) -> Option<Project> {
    let checked = ops::check(Path::new(&sp.path), refresh_timeout()).await;
    let sp = st.stored(&sp.id)?;
    let prev = st.cached(&sp.id);
    let mut p = match checked {
        Checked::Fresh(ins) => {
            st.mark_checked(&sp.id);
            project_from(&sp, &ins)
        }
        // fetch 失败：状态停留在上一次 fetch 时的结果（本地的远程跟踪分支没变）
        Checked::FetchFailed { local, error } => with_error(Some(project_from(&sp, &local)), &sp, error.message()),
        Checked::Failed(error) => with_error(prev, &sp, error.message()),
    };
    // 刷新检测到已同步：清除遗留的异常标记
    if p.status == SyncStatus::Synced && p.check_error.is_none() && p.issue.is_some() {
        st.update_stored(&sp, |e| e.issue = None);
        p.issue = None;
    }
    st.remember(&p);
    Some(p)
}

// ---------------- 代理 ----------------

/// 设置里的代理模式
const PROXY_MODES: [&str; 3] = ["auto", "off", "manual"];

/// 解析出的代理：交给 run_git 的代理设置，以及设置页显示的来源和地址（已去掉账号密码）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedProxy {
    pub plan: ProxyPlan,
    pub source: ProxySource,
    pub effective: Option<String>,
}

/// 按代理模式决定 SyncDeck 调用的 git 用什么代理。纯函数，探测结果由调用方传入：
/// - `off`：完全不走代理；git 配置里按网址设置的代理（`http.<网址>.proxy`）也一并盖住
/// - `manual`：用手动填写的地址（`manual_url` 为已校验的地址；没有时按自动处理），按网址设置的代理也盖住
/// - `auto`：git 自己配了代理 → 不覆盖。"git 自己的代理"按 git 的优先级看：`http.proxy`（写成空值表示
///   明确不用代理）、https 用的代理环境变量（`env_proxy`）、按网址设置的非空代理。
///   都没有而系统代理开着（`system_proxy`）→ 用系统代理（连同它的绕过列表）；都没有 → 不覆盖，也就是不用代理
pub fn decide_proxy(
    mode: &str,
    manual_url: Option<&str>,
    git: &GitProxyConfig,
    env_proxy: Option<&str>,
    system_proxy: Option<&SystemProxy>,
) -> ResolvedProxy {
    let plan = |over| ProxyPlan { over, url_scopes: git.url_scopes(), no_proxy: None };
    let none = |plan| ResolvedProxy { plan, source: ProxySource::NoProxy, effective: None };
    let own = |effective: String| ResolvedProxy {
        plan: ProxyOverride::Inherit.into(),
        source: ProxySource::Git,
        effective: Some(effective),
    };
    match (mode, manual_url) {
        ("off", _) => return none(plan(ProxyOverride::Disable)),
        ("manual", Some(url)) => {
            return ResolvedProxy {
                plan: plan(ProxyOverride::Use(url.to_string())),
                source: ProxySource::Manual,
                effective: Some(display_proxy(url)),
            }
        }
        _ => {}
    }

    // 只给某些网址配的代理：显示成"地址（仅 网址）"
    let scoped: Vec<_> = git.url_proxies.iter().filter(|(_, v)| !v.is_empty()).collect();
    let scoped_display = scoped.first().map(|(url, value)| match scoped.len() {
        1 => format!("{}（仅 {url}）", display_proxy(value)),
        n => format!("{}（仅 {url} 等 {n} 个网址）", display_proxy(value)),
    });
    let env_proxy = env_proxy.map(str::trim).filter(|v| !v.is_empty());
    match git.proxy.as_deref().map(str::trim) {
        Some("") => scoped_display.map_or_else(|| none(ProxyOverride::Inherit.into()), own),
        Some(p) => own(display_proxy(p)),
        None => match (env_proxy, scoped_display, system_proxy) {
            (Some(e), _, _) => own(display_proxy(e)),
            (None, Some(d), _) => own(d),
            // 用系统代理时不碰按网址的设置：能走到这里，它们都是"这个网址不用代理"
            (None, None, Some(sys)) => ResolvedProxy {
                plan: ProxyPlan {
                    over: ProxyOverride::Use(sys.addr.clone()),
                    url_scopes: Vec::new(),
                    no_proxy: sys.no_proxy.clone(),
                },
                source: ProxySource::System,
                effective: Some(display_proxy(&sys.addr)),
            },
            (None, None, None) => none(ProxyOverride::Inherit.into()),
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
    // git 自己的代理配置每次都读：不用代理 / 手动模式也要知道哪些网址单独配了代理，好把它们盖住
    let git = ops::git_proxy_config().await;
    let env_proxy = if auto { proxy_from_env(|k| std::env::var(k).ok()) } else { None };
    let system_proxy = if auto { util::windows_system_proxy() } else { None };
    let resolved = decide_proxy(mode, manual.as_deref(), &git, env_proxy.as_deref(), system_proxy.as_ref());
    set_proxy_plan(resolved.plan.clone());
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
    Ok(local_projects(state.inner()).await)
}

/// 本地检查所有项目（不联网）：启动时的列表、离开时挑选要同步的项目都用它
pub async fn local_projects(st: &Shared) -> Vec<Project> {
    let stored = st.config.read(|c| c.projects.clone());
    let st = st.clone();
    map_limited(stored, CONCURRENCY, move |sp| {
        let st = st.clone();
        async move { local_project(&st, &sp).await }
    })
    .await
}

/// 联网刷新所有项目：每个项目检查完就通过 `on_event` 发一次 Done，最后仍返回完整列表。
/// 刷新途中被取消监控的项目不发事件，也不在返回的列表里。
#[tauri::command]
pub async fn refresh_status(
    state: State<'_, Shared>,
    on_event: Channel<RefreshProgressEvent>,
) -> Result<Vec<Project>, String> {
    Ok(run_refresh(state.inner(), Arc::new(move |e| {
        let _ = on_event.send(e);
    }))
    .await)
}

/// 刷新状态的主体（fetch + status，不拉取）：窗口里的"刷新状态"和后台的定时检查云端都用它
pub async fn run_refresh(st: &Shared, on_event: RefreshSink) -> Vec<Project> {
    let _net = st.net_lock.lock().await;
    refresh_proxy(st).await;
    let stored = st.config.read(|c| c.projects.clone());
    let total = stored.len();
    let finished = Arc::new(AtomicUsize::new(0));
    let st2 = st.clone();
    let checked = map_limited(stored, CONCURRENCY, move |sp| {
        let st = st2.clone();
        let on_event = on_event.clone();
        let finished = finished.clone();
        async move {
            let project = remote_project(&st, &sp).await?;
            let index = finished.fetch_add(1, Ordering::SeqCst);
            on_event(RefreshProgressEvent::Done { project: project.clone(), index, total });
            Some(project)
        }
    })
    .await;
    checked.into_iter().flatten().collect()
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
        self.0.changed();
    }
}

/// 窗口里的"一键同步"：进度通过 Channel 推给前端
#[tauri::command]
pub async fn sync_all(
    state: State<'_, Shared>,
    on_event: Channel<SyncProgressEvent>,
) -> Result<SyncResult, String> {
    run_sync(state.inner(), None, Arc::new(move |e| {
        let _ = on_event.send(e);
    }))
    .await
}

/// 同步引擎：窗口里的一键同步和后台自动同步都用它。
/// `only` 为 None 时同步所有项目，否则只同步这些 id 的项目（离开时只挑有东西要推的）。
/// 同一时间只允许一轮（"正在同步"标记），已经有一轮在跑时返回 `SYNC_BUSY`；联网操作之间用 net_lock 互斥。
pub async fn run_sync(st: &Shared, only: Option<Vec<String>>, on_event: SyncSink) -> Result<SyncResult, String> {
    if st.syncing.swap(true, Ordering::SeqCst) {
        return Err(SYNC_BUSY.into());
    }
    st.changed();
    let _flag = SyncingFlag(st.clone());
    let _net = st.net_lock.lock().await;
    refresh_proxy(st).await;

    let started_at = now_iso();
    let mut stored = st.config.read(|c| c.projects.clone());
    if let Some(ids) = &only {
        stored.retain(|sp| ids.contains(&sp.id));
    }
    // 处理顺序与界面一致：需要处理的在前
    let weight = |sp: &StoredProject| st.cached(&sp.id).map_or(6, |p| p.status.weight());
    stored.sort_by(|a, b| weight(a).cmp(&weight(b)).then_with(|| folder_name(&a.path).cmp(&folder_name(&b.path))));

    // 自动提交的提交信息：本轮所有项目共用同一个时间
    let commit_message = Arc::new(commit_message_now(&st.config.read(commit_template_of)));

    // 每条进度事件先记进这一轮的进度（托盘、窗口中途打开时用），再交给调用方
    let on_event: SyncSink = {
        let st = st.clone();
        Arc::new(move |e| {
            st.note_progress(&e);
            on_event(e);
        })
    };
    let total = stored.len();
    on_event(SyncProgressEvent::Begin { project_ids: stored.iter().map(|sp| sp.id.clone()).collect(), total });
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
            on_event(SyncProgressEvent::Start { project_id: sp.id.clone(), index, total });
            let progress = {
                let (on_event, project_id) = (on_event.clone(), sp.id.clone());
                ops::ProgressSink::new(move |op, p| {
                    on_event(SyncProgressEvent::Progress {
                        project_id: project_id.clone(),
                        op,
                        phase: p.phase,
                        percent: p.percent,
                    })
                })
            };
            let opts = sync_options_for(&st, &sp, &commit_message, progress);
            let one = ops::sync_one(Path::new(&sp.path), &opts).await;
            let (project, result) = record_sync(&st, &sp, &opts, one);
            let index = finished.fetch_add(1, Ordering::SeqCst);
            on_event(SyncProgressEvent::Done { result: result.clone(), project, index, total });
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
/// `commit_message` 是本轮共用的自动提交信息，只给打开了自动提交的项目用；`progress` 接收这个项目的 git 进度。
/// 自动合并的提交信息"自动合并：来自 {host}"里的 `{host}` 和自动提交信息用同一个取电脑名的来源。
fn sync_options_for(
    st: &AppState,
    sp: &StoredProject,
    commit_message: &str,
    progress: ops::ProgressSink,
) -> ops::SyncOptions {
    let (auto_merge, precommit_check, big_file_limit_mb) =
        st.config.read(|c| (auto_merge_of(c), precommit_check_of(c), big_file_limit_of(c)));
    ops::SyncOptions {
        auto_commit: sp.auto_commit.then(|| commit_message.to_string()),
        skip_fetch: should_skip_fetch(st.last_checked(&sp.id), Instant::now()),
        auto_merge,
        merge_message: ops::merge_message(&util::computer_name()),
        precommit: precommit_check.then_some(ops::PrecommitCheck { big_file_limit_mb }),
        progress,
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
    // 分叉时预演一次合并（只在内存里算，不联网、不碰工作区），列出会冲突的文件，给「复制给 AI」用
    let conflict_files = if project.status == SyncStatus::Diverged {
        ops::preview_conflicts(&dir).await
    } else {
        Vec::new()
    };
    let recent_remote_commits =
        if project.upstream.is_some() { ops::recent_upstream_commits(&dir).await } else { Vec::new() };
    Ok(DiagnosticInfo { project, recent_commits, conflict_files, recent_remote_commits })
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
    let sw = Switches::from_stored(&c.settings);
    Settings {
        default_code_dir: code_dir_of(c),
        theme: c.settings.theme.clone(),
        commit_template: commit_template_of(c),
        network_timeout_secs: network_timeout_of(c),
        proxy_mode: proxy_mode_of(c).to_string(),
        proxy_url: c.settings.proxy_url.clone(),
        effective_proxy: proxy.effective.clone(),
        proxy_source: proxy.source,
        host_name: c.settings.host_name.clone().unwrap_or_default(),
        default_host_name: util::system_computer_name(),
        launch_at_login: sw.launch_at_login,
        close_to_tray: sw.close_to_tray,
        sync_on_lock: sw.sync_on_lock,
        sync_on_startup: sw.sync_on_startup,
        sync_on_resume: sw.sync_on_resume,
        sync_on_unlock: sw.sync_on_unlock,
        periodic_check: sw.periodic_check,
        notify_on_success: sw.notify_on_success,
        notify_on_error: sw.notify_on_error,
        auto_sync_paused_until: autosync::active_pause(&c.settings, &Local::now()).map(|d| d.to_rfc3339()),
        auto_merge: auto_merge_of(c),
        precommit_check: precommit_check_of(c),
        big_file_limit_mb: big_file_limit_of(c),
    }
}

/// 分叉时无冲突就自动合并：没设置过时默认开
pub(crate) fn auto_merge_of(c: &AppConfig) -> bool {
    c.settings.auto_merge.unwrap_or(true)
}

/// 自动提交前检查：没设置过时默认开
fn precommit_check_of(c: &AppConfig) -> bool {
    c.settings.precommit_check.unwrap_or(true)
}

/// 提交前检查的大文件上限（MB）：没设置过或配置文件里的值超出范围时用默认值
fn big_file_limit_of(c: &AppConfig) -> u64 {
    c.settings
        .big_file_limit_mb
        .filter(|mb| (1..=ops::MAX_BIG_FILE_LIMIT_MB).contains(mb))
        .unwrap_or(ops::DEFAULT_BIG_FILE_LIMIT_MB)
}

/// 本机名称最长多少个字
const HOST_NAME_MAX_CHARS: usize = 40;

/// 校验本机名称：去掉首尾空白；空 → None（用电脑名）；不能有换行等控制字符，不能太长
fn validate_host_name(raw: &str) -> Result<Option<String>, String> {
    let name = raw.trim();
    if name.chars().any(char::is_control) {
        return Err("本机名称不能包含换行等控制字符".into());
    }
    if name.chars().count() > HOST_NAME_MAX_CHARS {
        return Err(format!("本机名称最多 {HOST_NAME_MAX_CHARS} 个字"));
    }
    Ok((!name.is_empty()).then(|| name.to_string()))
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
    if let Some(mb) = patch.big_file_limit_mb {
        if !(1..=ops::MAX_BIG_FILE_LIMIT_MB).contains(&mb) {
            return Err(format!("大文件上限要在 1 ~ {} MB 之间：{mb} MB", ops::MAX_BIG_FILE_LIMIT_MB));
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
    let host_name = patch.host_name.as_deref().map(validate_host_name).transpose()?;
    // 开机自启：先注册 / 取消，成功了再保存，免得设置显示的和实际不一致
    if let (Some(on), Some(apply)) = (patch.launch_at_login, state.autostart.get()) {
        apply(on).map_err(|e| format!("设置开机自启失败：{e}"))?;
    }
    state.config.update(|c| {
        if let Some(name) = &host_name {
            c.settings.host_name = name.clone();
            util::set_host_name(name.as_deref());
        }
        let mut sw = Switches::from_stored(&c.settings);
        sw.apply(&patch);
        sw.store(&mut c.settings);
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
        // 和其他设置一样，等于默认值时不单独保存
        if let Some(on) = patch.auto_merge {
            c.settings.auto_merge = (!on).then_some(false);
        }
        if let Some(on) = patch.precommit_check {
            c.settings.precommit_check = (!on).then_some(false);
        }
        if let Some(mb) = patch.big_file_limit_mb {
            c.settings.big_file_limit_mb = (mb != ops::DEFAULT_BIG_FILE_LIMIT_MB).then_some(mb);
        }
        set_network_timeout(network_timeout_of(c));
    })?;
    let proxy = refresh_proxy(&state).await;
    Ok(state.config.read(|c| settings_of(c, &proxy)))
}

/// 当前设置（托盘里暂停 / 恢复后，用它把最新设置推给前端）
pub async fn current_settings(st: &AppState) -> Settings {
    let proxy = refresh_proxy(st).await;
    st.config.read(|c| settings_of(c, &proxy))
}

/// 暂停自动同步到某个时间（None = 恢复）。托盘菜单和设置页都用它
pub fn set_auto_sync_pause(st: &AppState, until: Option<DateTime<Local>>) -> Result<(), String> {
    st.config.update(|c| c.settings.auto_sync_paused_until = until.map(|d| d.to_rfc3339()))?;
    st.changed();
    Ok(())
}

/// 暂停自动同步到明天 00:00（本地时间）
#[tauri::command]
pub async fn pause_auto_sync(state: State<'_, Shared>) -> Result<Settings, String> {
    set_auto_sync_pause(&state, Some(autosync::next_midnight(&Local::now())))?;
    Ok(current_settings(&state).await)
}

/// 恢复自动同步
#[tauri::command]
pub async fn resume_auto_sync(state: State<'_, Shared>) -> Result<Settings, String> {
    set_auto_sync_pause(&state, None)?;
    Ok(current_settings(&state).await)
}

/// 后台自动同步的当前状态（前端启动时查一次，之后靠 auto-sync-started / -progress / -finished 事件）：
/// 正在进行的后台同步和它到现在的进度、最近一次后台同步的结果
#[tauri::command]
pub fn get_auto_sync_state(state: State<'_, Shared>) -> AutoSyncState {
    let mut s = state.auto.state();
    s.progress = s.running.as_ref().map(|_| state.round_snapshot());
    s
}

/// 退出 SyncDeck（设置页「退出 SyncDeck」按钮；和托盘菜单「退出」同一个逻辑）
#[tauri::command]
pub fn quit_app<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    crate::tray::quit(&app);
}

// ---------------- 从 GitHub 克隆 ----------------

/// 默认代码目录（没设置过时是用户目录）
fn code_dir_of(c: &AppConfig) -> String {
    c.settings
        .default_code_dir
        .clone()
        .unwrap_or_else(|| std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into()))
}

/// 打开「从 GitHub 克隆」时的默认值：上次输入的地址、gh 登录的账号、默认代码目录
#[tauri::command]
pub async fn get_clone_defaults(state: State<'_, Shared>) -> Result<CloneDefaults, String> {
    let st = state.inner().clone();
    refresh_proxy(&st).await;
    let gh_login = match github::gh_status().await {
        GhStatus::LoggedIn(login) => Some(login),
        _ => None,
    };
    Ok(st.config.read(|c| CloneDefaults {
        last_input: c.settings.github_source.clone(),
        gh_login,
        default_code_dir: code_dir_of(c),
    }))
}

/// 本地已有的仓库：默认代码目录下扫描到的 + 监控清单里的，按规范化后的远程地址对应到路径
async fn local_remotes(st: &AppState, code_dir: &str) -> HashMap<String, String> {
    let mut paths: Vec<String> = st.config.read(|c| c.projects.iter().map(|p| p.path.clone()).collect());
    let root = PathBuf::from(normalize_path(code_dir));
    if root.is_dir() {
        let found = tokio::task::spawn_blocking(move || scan::find_repos(&root)).await.unwrap_or_default();
        for p in found {
            let p = p.to_string_lossy().into_owned();
            if !paths.iter().any(|x| same_path(x, &p)) {
                paths.push(p);
            }
        }
    }
    let briefs = map_limited(paths, CONCURRENCY, |path| async move {
        let (_, remote) = ops::repo_brief(Path::new(&path)).await;
        (path, remote)
    })
    .await;
    let mut map = HashMap::new();
    for (path, remote) in briefs {
        if let Some(url) = remote {
            map.entry(github::normalize_remote(&url)).or_insert(path);
        }
    }
    map
}

/// 公开接口列仓库，失败翻成人话
async fn list_public(source: &github::Source) -> Result<Vec<GithubRepo>, String> {
    github::public_list(source).await.map_err(|e| github::public_error_text(&e, source))
}

/// 列出 GitHub 账号名下的仓库（或单个仓库）。优先用 gh（能看到私有仓库），没装 / 没登录 / 失败时退到公开接口
#[tauri::command]
pub async fn list_github_repos(state: State<'_, Shared>, input: String) -> Result<GithubListing, String> {
    let st = state.inner().clone();
    let source = github::parse_source(&input)?;
    refresh_proxy(&st).await;
    let saved = input.trim().to_string();
    st.config.update(|c| c.settings.github_source = Some(saved))?;

    let (repos, kind, notice) = match github::gh_status().await {
        GhStatus::LoggedIn(_) => match github::gh_list(&source).await {
            Ok(repos) => (repos, "gh", None),
            Err(e) => {
                let notice = format!("gh 列出仓库失败（{}），已改用公开接口，只能看到公开仓库", e.message());
                (list_public(&source).await?, "public", Some(notice))
            }
        },
        GhStatus::NotLoggedIn => (
            list_public(&source).await?,
            "public",
            Some("gh 还没登录：运行 gh auth login 后能看到私有仓库".to_string()),
        ),
        GhStatus::Missing => (
            list_public(&source).await?,
            "public",
            Some("装好 GitHub CLI（gh）并运行 gh auth login 后能看到私有仓库".to_string()),
        ),
    };

    let code_dir = st.config.read(code_dir_of);
    let locals = local_remotes(&st, &code_dir).await;
    let repos = repos
        .into_iter()
        .map(|repo| {
            let target = PathBuf::from(normalize_path(&code_dir)).join(&repo.name);
            GithubRepoEntry {
                local: locals.get(&github::normalize_remote(&repo.clone_url)).cloned(),
                target: target.to_string_lossy().into_owned(),
                target_exists: target.exists(),
                repo,
            }
        })
        .collect();
    Ok(GithubListing { owner: source.owner().to_string(), repos, source: kind.to_string(), notice })
}

/// 克隆进度的回调（前端的 Channel）
pub type CloneSink = Arc<dyn Fn(CloneProgressEvent) + Send + Sync>;

/// 同时克隆的仓库数上限
const CLONE_CONCURRENCY: usize = 3;

/// 私有仓库没有凭据时的提示
const CLONE_AUTH_HINT: &str = "如果是私有仓库，请先在命令行里用 git 登录一次 GitHub（比如 git clone 一次或 gh auth login 后按提示配置 git），SyncDeck 不会替你改 git 的全局配置";

/// 克隆失败的原因（已去掉账号）；认证失败 / 找不到仓库时补一句私有仓库的提示
pub fn clone_error_text(err: &crate::git::GitError) -> String {
    let msg = err.message();
    if let crate::git::GitError::Failed { stderr, .. } = err {
        let lower = stderr.to_lowercase();
        if ["authentication failed", "could not read username", "repository not found"].iter().any(|k| lower.contains(k)) {
            return format!("{msg}\n{CLONE_AUTH_HINT}");
        }
    }
    msg
}

/// 克隆到默认代码目录，完成后加入监控并联网检查一次。进度通过 Channel 推给前端
#[tauri::command]
pub async fn clone_repos(
    state: State<'_, Shared>,
    items: Vec<CloneRequest>,
    on_event: Channel<CloneProgressEvent>,
) -> Result<Vec<CloneResult>, String> {
    run_clone(state.inner(), items, Arc::new(move |e| {
        let _ = on_event.send(e);
    }))
    .await
}

pub async fn run_clone(st: &Shared, items: Vec<CloneRequest>, sink: CloneSink) -> Result<Vec<CloneResult>, String> {
    // 和刷新、一键同步互斥
    let _net = st.net_lock.lock().await;
    refresh_proxy(st).await;
    let code_dir = PathBuf::from(normalize_path(&st.config.read(code_dir_of)));
    if !code_dir.is_dir() {
        return Err(format!("默认代码目录不存在：{}，请先在设置里改一下", code_dir.display()));
    }
    let st = st.clone();
    Ok(map_limited(items, CLONE_CONCURRENCY, move |req| {
        let (st, sink, dir) = (st.clone(), sink.clone(), code_dir.clone());
        async move {
            sink(CloneProgressEvent::Start { url: req.url.clone() });
            let outcome = clone_one(&st, &dir, &req, &sink).await;
            let (ok, error, project) = match outcome {
                Ok(p) => (true, None, Some(p)),
                Err(e) => (false, Some(e), None),
            };
            sink(CloneProgressEvent::Done { url: req.url.clone(), ok, error: error.clone(), project: project.clone() });
            CloneResult { url: req.url, name: req.name, ok, error, project }
        }
    })
    .await)
}

async fn clone_one(st: &AppState, dir: &Path, req: &CloneRequest, sink: &CloneSink) -> Result<Project, String> {
    if !github::valid_repo_name(&req.name) {
        return Err(format!("文件夹名不合法：{}", req.name));
    }
    let target = dir.join(&req.name);
    if target.exists() {
        return Err(format!("文件夹已存在：{}，没有覆盖", target.display()));
    }
    let target_str = target.to_string_lossy().into_owned();
    let (cb_sink, cb_url) = (sink.clone(), req.url.clone());
    let on_progress = move |p: GitProgress| {
        cb_sink(CloneProgressEvent::Progress { url: cb_url.clone(), phase: p.phase, percent: p.percent })
    };
    let args = ["clone", "--progress", req.url.as_str(), target_str.as_str()];
    if let Err(e) = crate::git::run_git_progress(dir, &args, crate::git::network_timeout(), &on_progress).await {
        // 超时或中途失败可能留下半截文件夹：目标是这次新建的（开始前不存在），只删这一个
        let _ = std::fs::remove_dir_all(&target);
        return Err(clone_error_text(&e));
    }

    let path = normalize_path(&target_str);
    let sp = st.config.update(|c| {
        if let Some(existing) = c.projects.iter().find(|p| same_path(&p.path, &path)) {
            return existing.clone();
        }
        let sp = StoredProject { id: project_id(&path), path, last_sync_at: None, issue: None, auto_commit: false };
        c.projects.push(sp.clone());
        sp
    })?;
    match remote_project(st, &sp).await {
        Some(p) => Ok(p),
        None => Ok(local_project(st, &sp).await),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_error_hint_only_for_auth_failures() {
        use crate::git::GitError;
        let failed = |stderr: &str| GitError::Failed {
            command: "clone".into(),
            code: Some(128),
            stderr: stderr.into(),
            stdout: String::new(),
        };
        for s in [
            "remote: Repository not found.\nfatal: repository 'https://github.com/a/b.git/' not found",
            "fatal: Authentication failed for 'https://github.com/a/b.git/'",
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
        ] {
            let text = clone_error_text(&failed(s));
            assert!(text.contains("如果是私有仓库") && text.contains(s.lines().next().unwrap()), "{text}");
        }
        let text = clone_error_text(&failed("fatal: unable to access 'https://github.com/a/b.git/': Could not resolve host"));
        assert!(!text.contains("私有仓库"), "{text}");
        assert!(!clone_error_text(&GitError::Timeout { command: "clone".into(), secs: 180 }).contains("私有仓库"));
    }

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
            plan: over.into(),
            source,
            effective: effective.map(str::to_string),
        };
        let cfg = |proxy: Option<&str>, urls: &[(&str, &str)]| GitProxyConfig {
            proxy: proxy.map(str::to_string),
            url_proxies: urls.iter().map(|(u, v)| (u.to_string(), v.to_string())).collect(),
        };
        let nothing = cfg(None, &[]);
        let git = cfg(Some("socks5://127.0.0.1:10808"), &[]);
        let system = |addr: &str, no_proxy: Option<&str>| SystemProxy {
            addr: addr.to_string(),
            no_proxy: no_proxy.map(str::to_string),
        };
        let sys_proxy = system("http://127.0.0.1:10809", None);
        let sys = Some(&sys_proxy);

        // 自动：git 自己配了代理 → 不覆盖
        assert_eq!(
            decide_proxy("auto", None, &git, None, sys),
            r(ProxyOverride::Inherit, ProxySource::Git, Some("socks5://127.0.0.1:10808"))
        );
        // 自动：git 没配，系统代理开着 → 用系统代理
        assert_eq!(
            decide_proxy("auto", None, &nothing, None, sys),
            r(ProxyOverride::Use("http://127.0.0.1:10809".into()), ProxySource::System, Some("http://127.0.0.1:10809"))
        );
        // 自动：都没有 → 不覆盖、不用代理
        assert_eq!(decide_proxy("auto", None, &nothing, None, None), r(ProxyOverride::Inherit, ProxySource::NoProxy, None));
        // 自动：git 配置成空值（明确不用代理）→ 尊重 git 的配置，不套系统代理，也不看环境变量
        assert_eq!(
            decide_proxy("auto", None, &cfg(Some(""), &[]), Some("http://127.0.0.1:3"), sys),
            r(ProxyOverride::Inherit, ProxySource::NoProxy, None)
        );
        // 自动：环境变量里给 https 用的代理也算 git 自己的
        assert_eq!(
            decide_proxy("auto", None, &nothing, Some("http://127.0.0.1:3"), sys),
            r(ProxyOverride::Inherit, ProxySource::Git, Some("http://127.0.0.1:3"))
        );
        // 显示的地址去掉账号密码，交给 git 的保留原样
        assert_eq!(
            decide_proxy("auto", None, &nothing, None, Some(&system("http://u:p@127.0.0.1:1", None))),
            r(ProxyOverride::Use("http://u:p@127.0.0.1:1".into()), ProxySource::System, Some("http://127.0.0.1:1"))
        );
        assert_eq!(
            decide_proxy("auto", None, &nothing, Some("user:pw@127.0.0.1:2"), None),
            r(ProxyOverride::Inherit, ProxySource::Git, Some("127.0.0.1:2"))
        );

        // 自动：只按网址配了代理（http.https://github.com.proxy）→ 也算 git 自己配了代理，不覆盖
        let github = cfg(None, &[("https://github.com", "socks5://127.0.0.1:10808")]);
        assert_eq!(
            decide_proxy("auto", None, &github, None, sys),
            r(ProxyOverride::Inherit, ProxySource::Git, Some("socks5://127.0.0.1:10808（仅 https://github.com）"))
        );
        let two = cfg(Some(""), &[("https://a.example", ""), ("https://github.com", "http://h:1"), ("https://gitlab.com", "http://h:2")]);
        assert_eq!(
            decide_proxy("auto", None, &two, None, sys),
            r(ProxyOverride::Inherit, ProxySource::Git, Some("http://h:1（仅 https://github.com 等 2 个网址）"))
        );
        // 自动：按网址的设置只有"这个网址不用代理"→ 不算，照常用系统代理，而且不去盖这些网址的设置
        let direct_only = cfg(None, &[("https://git.corp.local", "")]);
        assert_eq!(
            decide_proxy("auto", None, &direct_only, None, sys),
            r(ProxyOverride::Use("http://127.0.0.1:10809".into()), ProxySource::System, Some("http://127.0.0.1:10809"))
        );

        // 自动 + 系统代理：系统的绕过列表带进 NO_PROXY
        let with_bypass = system("http://127.0.0.1:10809", Some("localhost,192.168.0.0/16"));
        let r_sys = decide_proxy("auto", None, &nothing, None, Some(&with_bypass));
        assert_eq!(r_sys.plan.over, ProxyOverride::Use("http://127.0.0.1:10809".into()));
        assert_eq!(r_sys.plan.no_proxy.as_deref(), Some("localhost,192.168.0.0/16"));
        assert!(r_sys.plan.url_scopes.is_empty());
        // 手动 / 不用代理 / git 自己的代理时不带系统的绕过列表
        for r in [
            decide_proxy("manual", Some("http://127.0.0.1:7890"), &nothing, None, Some(&with_bypass)),
            decide_proxy("off", None, &nothing, None, Some(&with_bypass)),
            decide_proxy("auto", None, &git, None, Some(&with_bypass)),
        ] {
            assert_eq!(r.plan.no_proxy, None, "{r:?}");
        }

        // 不用代理：不管 git 和系统怎么配，都完全不走代理，按网址配的代理也盖住
        assert_eq!(decide_proxy("off", None, &git, None, sys), r(ProxyOverride::Disable, ProxySource::NoProxy, None));
        let off = decide_proxy("off", None, &two, None, sys);
        assert_eq!(off.plan.over, ProxyOverride::Disable);
        assert_eq!(off.plan.url_scopes, ["https://a.example", "https://github.com", "https://gitlab.com"]);

        // 手动：用填写的地址，按网址配的代理也盖住
        assert_eq!(
            decide_proxy("manual", Some("http://127.0.0.1:7890"), &git, None, sys),
            r(ProxyOverride::Use("http://127.0.0.1:7890".into()), ProxySource::Manual, Some("http://127.0.0.1:7890"))
        );
        let manual = decide_proxy("manual", Some("http://127.0.0.1:7890"), &github, None, sys);
        assert_eq!(manual.plan.url_scopes, ["https://github.com"]);
        // 手动但没有可用的地址（配置被手动改坏）→ 按自动处理
        assert_eq!(
            decide_proxy("manual", None, &nothing, None, sys),
            decide_proxy("auto", None, &nothing, None, sys)
        );
    }
}
