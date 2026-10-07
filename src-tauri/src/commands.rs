//! Tauri 命令：与前端 src/api/tauri.ts 一一对应。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tauri::ipc::Channel;
use tauri::State;

use crate::config::{AppConfig, ConfigStore, StoredProject};
use crate::model::*;
use crate::ops::{self, Checked, SyncOne};
use crate::status::{inspect, Inspection};
use crate::util::{folder_name, map_limited, normalize_path, now_iso, project_id, same_path, CONCURRENCY};
use crate::{scan, system};

pub struct AppState {
    pub config: ConfigStore,
    /// 每个项目最近一次检查的结果（内存），检查失败时用它保留上一次的状态
    cache: Mutex<HashMap<String, Project>>,
    /// 联网操作（刷新、一键同步、提交推送）互斥，避免同一个仓库被同时 fetch / push
    net_lock: tokio::sync::Mutex<()>,
    syncing: AtomicBool,
}

pub type Shared = Arc<AppState>;

impl AppState {
    pub fn new(config: ConfigStore) -> Self {
        AppState {
            config,
            cache: Mutex::new(HashMap::new()),
            net_lock: tokio::sync::Mutex::new(()),
            syncing: AtomicBool::new(false),
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
    });
    p.last_sync_at = sp.last_sync_at.clone();
    p.issue = sp.issue.clone();
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

/// 联网检查：fetch + status
async fn remote_project(st: &AppState, sp: &StoredProject) -> Project {
    let prev = st.cached(&sp.id);
    let mut p = match ops::check(Path::new(&sp.path)).await {
        Checked::Fresh(ins) => project_from(sp, &ins),
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

#[tauri::command]
pub async fn refresh_status(state: State<'_, Shared>) -> Result<Vec<Project>, String> {
    let st = state.inner().clone();
    let _net = st.net_lock.lock().await;
    let stored = st.config.read(|c| c.projects.clone());
    let st2 = st.clone();
    Ok(map_limited(stored, CONCURRENCY, move |sp| {
        let st = st2.clone();
        async move { remote_project(&st, &sp).await }
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
            let sp = StoredProject { id: project_id(&path), path, last_sync_at: None, issue: None };
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

    let started_at = now_iso();
    let mut stored = st.config.read(|c| c.projects.clone());
    // 处理顺序与界面一致：需要处理的在前
    let weight = |sp: &StoredProject| st.cached(&sp.id).map_or(6, |p| p.status.weight());
    stored.sort_by(|a, b| weight(a).cmp(&weight(b)).then_with(|| folder_name(&a.path).cmp(&folder_name(&b.path))));

    let total = stored.len();
    let started = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicUsize::new(0));
    let st2 = st.clone();
    let items = map_limited(stored, CONCURRENCY, move |sp| {
        let st = st2.clone();
        let on_event = on_event.clone();
        let started = started.clone();
        let finished = finished.clone();
        async move {
            let index = started.fetch_add(1, Ordering::SeqCst);
            let _ = on_event.send(SyncProgressEvent::Start { project_id: sp.id.clone(), index, total });
            let one = ops::sync_one(Path::new(&sp.path)).await;
            let (project, result) = record_sync(&st, &sp, one);
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

/// 记录单个项目的同步结果：成功 → 更新上次同步时间、清除异常；失败 → 保存异常原因
fn record_sync(st: &AppState, sp: &StoredProject, one: SyncOne) -> (Project, SyncItemResult) {
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

fn settings_of(c: &AppConfig) -> Settings {
    Settings {
        default_code_dir: c
            .settings
            .default_code_dir
            .clone()
            .unwrap_or_else(|| std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into())),
        theme: c.settings.theme.clone(),
    }
}

#[tauri::command]
pub async fn get_settings(state: State<'_, Shared>) -> Result<Settings, String> {
    Ok(state.config.read(settings_of))
}

#[tauri::command]
pub async fn save_settings(state: State<'_, Shared>, patch: SettingsPatch) -> Result<Settings, String> {
    if let Some(t) = &patch.theme {
        if t != "light" && t != "dark" {
            return Err(format!("未知的主题：{t}"));
        }
    }
    state.config.update(|c| {
        if let Some(dir) = &patch.default_code_dir {
            c.settings.default_code_dir = Some(normalize_path(dir));
        }
        if let Some(t) = &patch.theme {
            c.settings.theme = Some(t.clone());
        }
        settings_of(c)
    })
}
