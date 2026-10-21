//! 换电脑自动同步的调度：什么时候做（纯函数 `decide`）、做什么（离开时只推本机要推的项目，到达时对所有
//! 项目跑一遍一键同步）、定时检查云端。系统事件见 sysevents.rs，托盘见 tray.rs。
//! 这里不碰窗口和托盘：结果通过 `Host` 交给调用方（lib.rs 里发事件、发通知），方便测试。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Local, TimeZone};

use crate::commands::{self, Shared, SyncSink, SYNC_BUSY};
use crate::config::StoredSettings;
use crate::model::{
    AutoSyncEvent, AutoSyncState, AutoSyncTrigger, Project, SettingsPatch, SyncOutcome, SyncProgressEvent, SyncResult,
    SyncStatus,
};

/// 同一种自动同步（离开 / 到达）多少分钟内只做一次
pub const SAME_KIND_MINUTES: i64 = 10;
/// 定时检查云端的间隔（分钟）
pub const PERIODIC_MINUTES: i64 = 30;
/// 启动 / 唤醒后联网失败时，等多久重试一次（网络可能还没连上）
pub const RETRY_DELAY: Duration = Duration::from_secs(15);
/// 定时检查的轮询间隔：每隔这么久看一眼是否到了 30 分钟（开关随时可改，不用重启计时器）
pub const PERIODIC_TICK: Duration = Duration::from_secs(60);

/// 自动同步的种类：离开、到达、定时检查云端
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Departure,
    Arrival,
    Check,
}

impl AutoSyncTrigger {
    pub fn kind(self) -> Kind {
        use AutoSyncTrigger::*;
        match self {
            Lock => Kind::Departure,
            Startup | Resume | Unlock | Manual => Kind::Arrival,
            Periodic => Kind::Check,
        }
    }

    /// 显示用的文字（通知标题、主页提示）
    pub fn label(self) -> &'static str {
        use AutoSyncTrigger::*;
        match self {
            Lock => "锁屏时同步",
            Startup => "启动时同步",
            Resume => "唤醒后同步",
            Unlock => "解锁后同步",
            Manual => "立即同步",
            Periodic => "定时检查云端",
        }
    }
}

// ---------------- 开关 ----------------

/// 后台运行与自动同步的各个开关（读取时已套上默认值）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switches {
    pub launch_at_login: bool,
    pub close_to_tray: bool,
    pub sync_on_lock: bool,
    pub sync_on_startup: bool,
    pub sync_on_resume: bool,
    pub sync_on_unlock: bool,
    pub periodic_check: bool,
    pub notify_on_success: bool,
    pub notify_on_error: bool,
}

/// 对每个开关执行同一段代码：`$m!(字段名)`
macro_rules! each_switch {
    ($m:ident) => {
        $m!(launch_at_login);
        $m!(close_to_tray);
        $m!(sync_on_lock);
        $m!(sync_on_startup);
        $m!(sync_on_resume);
        $m!(sync_on_unlock);
        $m!(periodic_check);
        $m!(notify_on_success);
        $m!(notify_on_error);
    };
}

impl Switches {
    /// 默认值：开机自启、关窗口隐藏到托盘、锁屏和启动 / 唤醒 / 解锁都开；定时检查关；只在出错或冲突时通知
    pub const DEFAULT: Switches = Switches {
        launch_at_login: true,
        close_to_tray: true,
        sync_on_lock: true,
        sync_on_startup: true,
        sync_on_resume: true,
        sync_on_unlock: true,
        periodic_check: false,
        notify_on_success: false,
        notify_on_error: true,
    };

    /// 从配置读取；没设置过的用默认值（旧的 config.json 里没有这些字段）
    pub fn from_stored(s: &StoredSettings) -> Self {
        let mut sw = Self::DEFAULT;
        macro_rules! read {
            ($f:ident) => {
                if let Some(v) = s.$f {
                    sw.$f = v;
                }
            };
        }
        each_switch!(read);
        sw
    }

    /// 写回配置：等于默认值的不单独保存（存 None），以后改默认值时没动过的开关跟着变
    pub fn store(&self, s: &mut StoredSettings) {
        macro_rules! write {
            ($f:ident) => {
                s.$f = (self.$f != Self::DEFAULT.$f).then_some(self.$f);
            };
        }
        each_switch!(write);
    }

    /// 应用设置页的修改（patch 里没给的开关不变）
    pub fn apply(&mut self, p: &SettingsPatch) {
        macro_rules! patch {
            ($f:ident) => {
                if let Some(v) = p.$f {
                    self.$f = v;
                }
            };
        }
        each_switch!(patch);
    }

    /// 这种触发对应的开关是否打开（托盘"立即同步"没有开关）
    pub fn allows(&self, t: AutoSyncTrigger) -> bool {
        use AutoSyncTrigger::*;
        match t {
            Lock => self.sync_on_lock,
            Startup => self.sync_on_startup,
            Resume => self.sync_on_resume,
            Unlock => self.sync_on_unlock,
            Manual => true,
            Periodic => self.periodic_check,
        }
    }
}

// ---------------- 要不要做（纯函数） ----------------

/// 这次自动同步要不要做
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Run,
    /// 对应的开关关着
    Disabled,
    /// 自动同步暂停中
    Paused,
    /// 同一种自动同步刚做过（离开 / 到达 10 分钟内、定时检查 30 分钟内）
    TooSoon,
}

/// 判断这次要不要做：
/// - 托盘"立即同步"是用户亲手点的：总是做（不看开关、暂停和间隔）
/// - 对应开关关着 → 不做；暂停中（`now` 早于 `paused_until`）→ 不做
/// - 同一种（离开 / 到达）上次距今不到 10 分钟 → 不做；定时检查的间隔是 30 分钟
/// - `last_same_kind` 比 `now` 还晚（系统时间被往回调过）→ 不算刚做过
pub fn decide<Tz: TimeZone>(
    trigger: AutoSyncTrigger,
    now: &DateTime<Tz>,
    last_same_kind: Option<&DateTime<Tz>>,
    paused_until: Option<&DateTime<Tz>>,
    switches: &Switches,
) -> Decision {
    if trigger == AutoSyncTrigger::Manual {
        return Decision::Run;
    }
    if !switches.allows(trigger) {
        return Decision::Disabled;
    }
    if paused_until.is_some_and(|until| now < until) {
        return Decision::Paused;
    }
    let gap = match trigger {
        AutoSyncTrigger::Periodic => PERIODIC_MINUTES,
        _ => SAME_KIND_MINUTES,
    };
    if let Some(last) = last_same_kind {
        let elapsed = now.clone().signed_duration_since(last.clone());
        if elapsed >= chrono::Duration::zero() && elapsed < chrono::Duration::minutes(gap) {
            return Decision::TooSoon;
        }
    }
    Decision::Run
}

/// "暂停到明天"：本地时间次日 00:00（夏令时导致 00:00 不存在时，取 24 小时后）
pub fn next_midnight<Tz: TimeZone>(now: &DateTime<Tz>) -> DateTime<Tz> {
    let fallback = || now.clone() + chrono::Duration::hours(24);
    let Some(midnight) = now.date_naive().succ_opt().and_then(|d| d.and_hms_opt(0, 0, 0)) else {
        return fallback();
    };
    now.timezone().from_local_datetime(&midnight).earliest().unwrap_or_else(fallback)
}

/// 正在生效的暂停：配置里有暂停时间且还没到；格式不对或已过期都算没暂停
pub fn active_pause(s: &StoredSettings, now: &DateTime<Local>) -> Option<DateTime<Local>> {
    let until = DateTime::parse_from_rfc3339(s.auto_sync_paused_until.as_deref()?).ok()?.with_timezone(&Local);
    (until > *now).then_some(until)
}

/// 暂停到的时间怎么显示："今天 18:00" / "明天 00:00" / "10月20日 00:00"
pub fn describe_until<Tz: TimeZone>(until: &DateTime<Tz>, now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let (day, today) = (until.date_naive(), now.date_naive());
    let hm = until.format("%H:%M");
    if day == today {
        format!("今天 {hm}")
    } else if Some(day) == today.succ_opt() {
        format!("明天 {hm}")
    } else {
        format!("{}月{}日 {hm}", day.month(), day.day())
    }
}

/// 离开时要同步的项目（只看本地检查的结果，不联网）：待推送的；有未提交改动且打开了自动提交的；
/// 打开了自动合并时还有分叉的（本地也有没推上去的提交，预演无冲突就能合并后推送）
pub fn pick_departure(projects: &[Project], auto_merge: bool) -> Vec<String> {
    projects
        .iter()
        .filter(|p| match p.status {
            SyncStatus::Ahead => true,
            SyncStatus::Dirty => p.auto_commit,
            SyncStatus::Diverged => auto_merge,
            _ => false,
        })
        .map(|p| p.id.clone())
        .collect()
}

/// 后台同步结束后的系统通知（标题、正文）；不需要通知时为 None。
/// 有项目失败 → "2 个项目需要处理：A、B"（"出错或冲突时通知"打开时）；
/// 都成功且确实推送 / 拉取 / 提交了东西 → "已同步 2 个项目：A、B"（"同步成功时通知"打开时）
pub fn notification_text(
    trigger: AutoSyncTrigger,
    result: &SyncResult,
    name_of: &HashMap<String, String>,
    switches: &Switches,
) -> Option<(String, String)> {
    let names = |pick: &dyn Fn(SyncOutcome) -> bool| -> Vec<String> {
        result
            .items
            .iter()
            .filter(|i| pick(i.outcome))
            .map(|i| name_of.get(&i.project_id).cloned().unwrap_or_else(|| i.project_id.clone()))
            .collect()
    };
    let title = format!("Git SyncDeck · {}", trigger.label());
    let failed = names(&|o| o == SyncOutcome::Failed);
    if !failed.is_empty() {
        return switches
            .notify_on_error
            .then(|| (title, format!("{} 个项目需要处理：{}", failed.len(), join_names(&failed))));
    }
    // 不是"已是最新"也不是失败，就是确实做了点什么（推送、拉取、自动提交，以及以后新增的结果类型）
    let changed = names(&|o| o != SyncOutcome::Failed && o != SyncOutcome::UpToDate);
    (switches.notify_on_success && !changed.is_empty())
        .then(|| (title, format!("已同步 {} 个项目：{}", changed.len(), join_names(&changed))))
}

/// 最多列出 3 个名字，多了加"等"
fn join_names(names: &[String]) -> String {
    let shown = names.iter().take(3).cloned().collect::<Vec<_>>().join("、");
    if names.len() > 3 {
        format!("{shown} 等")
    } else {
        shown
    }
}

// ---------------- 调度状态 ----------------

#[derive(Default)]
struct Times {
    departure: Option<DateTime<Local>>,
    arrival: Option<DateTime<Local>>,
    check: Option<DateTime<Local>>,
}

/// 自动同步的调度状态（放在 AppState 里）
pub struct AutoSync {
    times: Mutex<Times>,
    /// 正在进行的后台同步（包括唤醒后等待重试的那 15 秒）
    running: Mutex<Option<AutoSyncTrigger>>,
    /// 启动时的到达同步由后端负责
    startup_sync: AtomicBool,
    /// 最近一次真的联网同步了的后台同步（只在内存里）
    last: Mutex<Option<AutoSyncEvent>>,
}

impl Default for AutoSync {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoSync {
    pub fn new() -> Self {
        AutoSync {
            // 定时检查从启动时开始计时：启动时前端（或到达同步）已经检查过一次
            times: Mutex::new(Times { check: Some(Local::now()), ..Default::default() }),
            running: Mutex::new(None),
            startup_sync: AtomicBool::new(false),
            last: Mutex::new(None),
        }
    }

    /// 判断这次要不要做；要做就立刻记下时间（同时来的两个唤醒事件只做一次）。
    /// 到达同步会联网检查所有项目，所以也顺带重置定时检查的计时
    pub fn claim(&self, trigger: AutoSyncTrigger, now: DateTime<Local>, settings: &StoredSettings) -> Decision {
        let switches = Switches::from_stored(settings);
        let paused = active_pause(settings, &now);
        let mut t = self.times.lock().unwrap_or_else(|e| e.into_inner());
        let last = match trigger.kind() {
            Kind::Departure => t.departure,
            Kind::Arrival => t.arrival,
            Kind::Check => t.check,
        };
        let decision = decide(trigger, &now, last.as_ref(), paused.as_ref(), &switches);
        if decision == Decision::Run {
            match trigger.kind() {
                Kind::Departure => t.departure = Some(now),
                Kind::Arrival => {
                    t.arrival = Some(now);
                    t.check = Some(now);
                }
                Kind::Check => t.check = Some(now),
            }
        }
        decision
    }

    pub fn running(&self) -> Option<AutoSyncTrigger> {
        *self.running.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 开始一轮后台同步；已经有一轮时返回 None
    fn begin(&self, trigger: AutoSyncTrigger) -> Option<RunningGuard<'_>> {
        let mut r = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if r.is_some() {
            return None;
        }
        *r = Some(trigger);
        Some(RunningGuard(self, trigger))
    }

    /// 当前状态；`progress` 由 `commands::get_auto_sync_state` 补上（进度记在 AppState 里）
    pub fn state(&self) -> AutoSyncState {
        AutoSyncState {
            running: self.running().map(|t| AutoSyncEvent { trigger: t, label: t.label().into(), result: None }),
            startup_sync: self.startup_sync.load(Ordering::SeqCst),
            progress: None,
            last: self.last.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        }
    }

    fn set_last(&self, e: AutoSyncEvent) {
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
    }
}

/// 一轮后台同步结束（包括中途返回）时清除"正在进行"
struct RunningGuard<'a>(&'a AutoSync, AutoSyncTrigger);
impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        let mut r = self.0.running.lock().unwrap_or_else(|e| e.into_inner());
        if *r == Some(self.1) {
            *r = None;
        }
    }
}

// ---------------- 执行 ----------------

/// 后台同步的结果交给谁：lib.rs 里发 Tauri 事件、发系统通知；测试里记下来
pub trait Host: Send + Sync {
    /// 开始联网同步
    fn started(&self, _trigger: AutoSyncTrigger) {}
    /// 同步进度（和一键同步的进度事件一样，另有等网络重试的 Retry）
    fn progress(&self, _trigger: AutoSyncTrigger, _event: &SyncProgressEvent) {}
    /// 一轮同步结束（定时检查不算）。`result` 为 None：没有要同步的项目（没联网），或同步没能开始
    fn finished(&self, _trigger: AutoSyncTrigger, _result: Option<&SyncResult>) {}
    /// 最新的项目列表（后台检查 / 同步之后）
    fn projects_changed(&self, _projects: Vec<Project>) {}
}

/// 什么都不做的 Host
pub struct NoHost;
impl Host for NoHost {}

/// 一次自动同步的结果
#[derive(Debug)]
pub enum Outcome {
    /// 按规则不做
    Skipped(Decision),
    /// 已经有一轮同步在跑，这次跳过
    Busy,
    /// 离开时没有要推的项目，完全没联网
    NothingToDo,
    /// 同步完了
    Synced(SyncResult),
    /// 定时检查完了，检查了几个项目
    Checked(usize),
    /// 同步没能开始
    Failed(String),
}

/// 到达类最多等多久"离开前那一轮"结束（锁屏那一轮常被睡眠打断，唤醒后接着跑完）
pub const WAIT_DEPARTURE_LEFTOVER: Duration = Duration::from_secs(120);

/// 到达类触发遇到正在跑的离开同步时，要不要先等它结束。
/// 锁屏同步常被睡眠打断、唤醒后才接着跑完：设了"唤醒需要登录"时，合盖往往先锁屏再睡眠，
/// 正在跑的就是锁屏那一轮。不等的话，唤醒和解锁的到达同步都会被它挡掉，唤醒后什么都不做
pub fn waits_for_departure(trigger: AutoSyncTrigger, running: Option<AutoSyncTrigger>) -> bool {
    trigger.kind() == Kind::Arrival && running.is_some_and(|t| t.kind() == Kind::Departure)
}

/// 处理一次触发（锁屏、解锁、唤醒、托盘立即同步、定时检查……）。
/// 已经有一轮同步在跑时直接跳过，也不记时间。例外：到达类遇到正在跑的离开同步，先等它结束（最多 2 分钟）
/// 再做，见 `waits_for_departure`
pub async fn handle(st: &Shared, host: &dyn Host, trigger: AutoSyncTrigger) -> Outcome {
    if waits_for_departure(trigger, st.auto.running()) {
        let deadline = Instant::now() + WAIT_DEPARTURE_LEFTOVER;
        while waits_for_departure(trigger, st.auto.running()) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    if st.is_syncing() || st.auto.running().is_some() {
        return Outcome::Busy;
    }
    match st.config.read(|c| st.auto.claim(trigger, Local::now(), &c.settings)) {
        Decision::Run => execute(st, host, trigger).await,
        d => Outcome::Skipped(d),
    }
}

/// 程序启动 = 一次"到达"。要做的话由后端做（前端启动时就不再自己刷新），不做的话只做一次本地检查给托盘用
pub fn on_startup(st: Shared, host: Arc<dyn Host>) {
    let decision = st.config.read(|c| st.auto.claim(AutoSyncTrigger::Startup, Local::now(), &c.settings));
    if decision == Decision::Run {
        st.auto.startup_sync.store(true, Ordering::SeqCst);
        tauri::async_runtime::spawn(async move {
            execute(&st, &*host, AutoSyncTrigger::Startup).await;
        });
    } else {
        tauri::async_runtime::spawn(async move {
            commands::local_projects(&st).await;
            host.projects_changed(st.snapshot());
        });
    }
}

/// 在后台处理一次触发，不等结果（系统事件、托盘菜单用）
pub fn spawn(st: Shared, host: Arc<dyn Host>, trigger: AutoSyncTrigger) {
    tauri::async_runtime::spawn(async move {
        handle(&st, &*host, trigger).await;
    });
}

/// 定时检查云端：每分钟看一眼，打开了开关、没暂停、距上次检查（或到达同步）满 30 分钟就刷新一次
pub async fn periodic_loop(st: Shared, host: Arc<dyn Host>) {
    loop {
        tokio::time::sleep(PERIODIC_TICK).await;
        handle(&st, &*host, AutoSyncTrigger::Periodic).await;
    }
}

/// 已经决定要做之后的执行部分
async fn execute(st: &Shared, host: &dyn Host, trigger: AutoSyncTrigger) -> Outcome {
    if trigger.kind() == Kind::Check {
        // 定时检查只是看一眼（不拉取、不推送），不占"正在后台同步"：
        // 期间来的到达同步照常开始，靠 net_lock 排在它后面
        let checked = commands::run_refresh(st, Arc::new(|_| {})).await.len();
        host.projects_changed(st.snapshot());
        return Outcome::Checked(checked);
    }
    let Some(_running) = st.auto.begin(trigger) else {
        return Outcome::Busy;
    };
    match trigger.kind() {
        Kind::Departure => departure(st, host, trigger).await,
        // 开机、唤醒后网络可能还没连上：联网失败的项目等一会儿再试一次
        _ => {
            let retry = matches!(trigger, AutoSyncTrigger::Startup | AutoSyncTrigger::Resume);
            sync_and_report(st, host, trigger, None, retry).await
        }
    }
}

/// 离开：先本地检查所有项目（不联网），只同步有东西要推的；一个都没有就完全不联网
async fn departure(st: &Shared, host: &dyn Host, trigger: AutoSyncTrigger) -> Outcome {
    let local = commands::local_projects(st).await;
    let ids = pick_departure(&local, st.config.read(commands::auto_merge_of));
    if ids.is_empty() {
        host.projects_changed(st.snapshot());
        host.finished(trigger, None);
        return Outcome::NothingToDo;
    }
    sync_and_report(st, host, trigger, Some(ids), false).await
}

async fn sync_and_report(
    st: &Shared,
    host: &dyn Host,
    trigger: AutoSyncTrigger,
    only: Option<Vec<String>>,
    retry: bool,
) -> Outcome {
    st.reset_round();
    host.started(trigger);
    // 进度事件从各个项目的任务里发出来，经过通道交给 host（host 只借用到这一轮结束）
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SyncProgressEvent>();
    let sink: SyncSink = Arc::new(move |e| {
        let _ = tx.send(e);
    });
    let work = async move {
        let mut result = commands::run_sync(st, only, sink.clone()).await;
        if let (true, Ok(first)) = (retry, &mut result) {
            let again = network_failures(st, first);
            if !again.is_empty() {
                let retry_at = (Local::now() + chrono::Duration::from_std(RETRY_DELAY).unwrap_or_default()).to_rfc3339();
                let notice = SyncProgressEvent::Retry { project_ids: again.clone(), retry_at };
                st.note_progress(&notice);
                sink(notice);
                tokio::time::sleep(RETRY_DELAY).await;
                if let Ok(second) = commands::run_sync(st, Some(again), sink.clone()).await {
                    merge_retry(first, second);
                }
            }
        }
        drop(sink); // 发送端都丢掉后，下面的转发就结束了
        result
    };
    let forward = async {
        while let Some(e) = rx.recv().await {
            host.progress(trigger, &e);
        }
    };
    let (result, ()) = tokio::join!(work, forward);
    if let Ok(r) = &result {
        st.auto.set_last(AutoSyncEvent { trigger, label: trigger.label().into(), result: Some(r.clone()) });
    }
    host.projects_changed(st.snapshot());
    host.finished(trigger, result.as_ref().ok());
    match result {
        Ok(r) => Outcome::Synced(r),
        Err(e) if e == SYNC_BUSY => Outcome::Busy,
        Err(e) => Outcome::Failed(e),
    }
}

/// 失败在联网检查这一步的项目（界面上显示"检查失败"的）：值得等网络好了再试
fn network_failures(st: &Shared, r: &SyncResult) -> Vec<String> {
    let check_failed: HashMap<String, bool> =
        st.snapshot().into_iter().map(|p| (p.id, p.check_error.is_some())).collect();
    r.items
        .iter()
        .filter(|i| i.outcome == SyncOutcome::Failed && check_failed.get(&i.project_id).copied().unwrap_or(false))
        .map(|i| i.project_id.clone())
        .collect()
}

/// 重试的结果覆盖第一次的同名项目
fn merge_retry(first: &mut SyncResult, second: SyncResult) {
    for item in second.items {
        if let Some(slot) = first.items.iter_mut().find(|i| i.project_id == item.project_id) {
            *slot = item;
        }
    }
    first.failed_count = first.items.iter().filter(|i| i.outcome == SyncOutcome::Failed).count();
    first.synced_count = first.items.len() - first.failed_count;
    first.finished_at = second.finished_at;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SyncItemResult;
    use chrono::FixedOffset;
    use AutoSyncTrigger::*;

    fn at(h: u32, m: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(8 * 3600).unwrap().with_ymd_and_hms(2026, 10, 16, h, m, 0).unwrap()
    }

    const ALL: [AutoSyncTrigger; 6] = [Lock, Startup, Resume, Unlock, Manual, Periodic];

    #[test]
    fn kinds() {
        assert_eq!(Lock.kind(), Kind::Departure);
        for t in [Startup, Resume, Unlock, Manual] {
            assert_eq!(t.kind(), Kind::Arrival);
        }
        assert_eq!(Periodic.kind(), Kind::Check);
    }

    #[test]
    fn runs_by_default_except_periodic() {
        let sw = Switches::DEFAULT;
        for t in ALL {
            let want = if t == Periodic { Decision::Disabled } else { Decision::Run };
            assert_eq!(decide(t, &at(9, 0), None, None, &sw), want, "{t:?}");
        }
    }

    #[test]
    fn same_kind_only_once_in_ten_minutes() {
        let sw = Switches { periodic_check: true, ..Switches::DEFAULT };
        let now = at(9, 30);
        for t in [Lock, Startup, Resume, Unlock] {
            assert_eq!(decide(t, &now, Some(&at(9, 21)), None, &sw), Decision::TooSoon, "{t:?} 9 分钟前做过");
            assert_eq!(decide(t, &now, Some(&at(9, 20)), None, &sw), Decision::Run, "{t:?} 满 10 分钟");
            assert_eq!(decide(t, &now, Some(&at(8, 0)), None, &sw), Decision::Run);
        }
        // 托盘"立即同步"不受 10 分钟限制
        assert_eq!(decide(Manual, &now, Some(&at(9, 29)), None, &sw), Decision::Run);
        // 定时检查：30 分钟一次
        assert_eq!(decide(Periodic, &now, Some(&at(9, 1)), None, &sw), Decision::TooSoon);
        assert_eq!(decide(Periodic, &now, Some(&at(9, 0)), None, &sw), Decision::Run);
        // 系统时间被往回调过（上次时间在未来）：不算刚做过
        assert_eq!(decide(Unlock, &now, Some(&at(10, 0)), None, &sw), Decision::Run);
    }

    #[test]
    fn switches_turn_off_each_trigger() {
        let off = Switches {
            sync_on_lock: false,
            sync_on_startup: false,
            sync_on_resume: false,
            sync_on_unlock: false,
            periodic_check: false,
            ..Switches::DEFAULT
        };
        for t in ALL {
            let want = if t == Manual { Decision::Run } else { Decision::Disabled };
            assert_eq!(decide(t, &at(9, 0), None, None, &off), want, "{t:?}");
        }
        // 只关锁屏：解锁照常
        let no_lock = Switches { sync_on_lock: false, ..Switches::DEFAULT };
        assert_eq!(decide(Lock, &at(9, 0), None, None, &no_lock), Decision::Disabled);
        assert_eq!(decide(Unlock, &at(9, 0), None, None, &no_lock), Decision::Run);
    }

    #[test]
    fn pause_until_tomorrow() {
        let sw = Switches { periodic_check: true, ..Switches::DEFAULT };
        let until = next_midnight(&at(15, 20));
        assert_eq!(until.to_rfc3339(), "2026-10-17T00:00:00+08:00");
        for t in ALL {
            let want = if t == Manual { Decision::Run } else { Decision::Paused };
            assert_eq!(decide(t, &at(23, 59), None, Some(&until), &sw), want, "{t:?} 暂停中");
        }
        // 到了次日 00:00 自动恢复
        let tomorrow = until.clone();
        assert_eq!(decide(Unlock, &tomorrow, None, Some(&until), &sw), Decision::Run);
        // 刚过午夜暂停：暂停到再下一个午夜
        assert_eq!(next_midnight(&at(0, 0)).to_rfc3339(), "2026-10-17T00:00:00+08:00");
        // 月底、年底
        let tz = FixedOffset::east_opt(8 * 3600).unwrap();
        let eom = tz.with_ymd_and_hms(2026, 10, 31, 22, 0, 0).unwrap();
        assert_eq!(next_midnight(&eom).to_rfc3339(), "2026-11-01T00:00:00+08:00");
        let eoy = tz.with_ymd_and_hms(2026, 12, 31, 23, 59, 59).unwrap();
        assert_eq!(next_midnight(&eoy).to_rfc3339(), "2027-01-01T00:00:00+08:00");
    }

    #[test]
    fn describes_pause_time() {
        assert_eq!(describe_until(&next_midnight(&at(15, 0)), &at(15, 0)), "明天 00:00");
        assert_eq!(describe_until(&at(18, 30), &at(15, 0)), "今天 18:30");
        let tz = FixedOffset::east_opt(8 * 3600).unwrap();
        let later = tz.with_ymd_and_hms(2026, 10, 20, 0, 0, 0).unwrap();
        assert_eq!(describe_until(&later, &at(15, 0)), "10月20日 00:00");
    }

    #[test]
    fn active_pause_reads_config() {
        let now = Local::now();
        let mut s = StoredSettings::default();
        assert!(active_pause(&s, &now).is_none());
        s.auto_sync_paused_until = Some(next_midnight(&now).to_rfc3339());
        assert!(active_pause(&s, &now).is_some());
        s.auto_sync_paused_until = Some((now - chrono::Duration::minutes(1)).to_rfc3339());
        assert!(active_pause(&s, &now).is_none(), "已过期");
        s.auto_sync_paused_until = Some("明天".into());
        assert!(active_pause(&s, &now).is_none(), "格式不对当作没暂停");
    }

    #[test]
    fn claim_records_time_only_when_running() {
        let auto = AutoSync::new();
        let s = StoredSettings::default();
        let t0 = Local::now();
        assert_eq!(auto.claim(Unlock, t0, &s), Decision::Run);
        // 唤醒的两个事件、解锁紧接着来：10 分钟内同属"到达"，只做一次
        assert_eq!(auto.claim(Resume, t0 + chrono::Duration::seconds(5), &s), Decision::TooSoon);
        // "离开"另算
        assert_eq!(auto.claim(Lock, t0 + chrono::Duration::minutes(1), &s), Decision::Run);
        assert_eq!(auto.claim(Unlock, t0 + chrono::Duration::minutes(11), &s), Decision::Run);
        // 关着开关的不记时间：打开后马上能做
        let off = StoredSettings { sync_on_lock: Some(false), ..Default::default() };
        let auto = AutoSync::new();
        assert_eq!(auto.claim(Lock, t0, &off), Decision::Disabled);
        assert_eq!(auto.claim(Lock, t0, &s), Decision::Run);
    }

    #[test]
    fn switches_round_trip_through_config() {
        let mut s = StoredSettings::default();
        assert_eq!(Switches::from_stored(&s), Switches::DEFAULT);
        let mut sw = Switches::DEFAULT;
        sw.apply(&SettingsPatch { sync_on_lock: Some(false), periodic_check: Some(true), ..Default::default() });
        sw.store(&mut s);
        assert_eq!(s.sync_on_lock, Some(false));
        assert_eq!(s.periodic_check, Some(true));
        assert_eq!(s.sync_on_unlock, None, "等于默认值的不单独保存");
        assert_eq!(Switches::from_stored(&s), sw);
        // 改回默认值：配置里又变回 None
        sw.apply(&SettingsPatch { sync_on_lock: Some(true), ..Default::default() });
        sw.store(&mut s);
        assert_eq!(s.sync_on_lock, None);
    }

    fn project(id: &str, status: SyncStatus, auto_commit: bool) -> Project {
        Project {
            id: id.into(),
            name: id.into(),
            path: format!("D:\\code\\{id}"),
            branch: "main".into(),
            remote_url: None,
            upstream: Some("origin/main".into()),
            status,
            ahead: 0,
            behind: 0,
            changes: Vec::new(),
            last_sync_at: None,
            issue: None,
            check_error: None,
            auto_commit,
        }
    }

    #[test]
    fn departure_picks_only_what_needs_pushing() {
        let list = vec![
            project("synced", SyncStatus::Synced, true),
            project("ahead", SyncStatus::Ahead, false),
            project("behind", SyncStatus::Behind, true),
            project("dirty-auto", SyncStatus::Dirty, true),
            project("dirty-manual", SyncStatus::Dirty, false),
            project("diverged", SyncStatus::Diverged, true),
            project("no-remote", SyncStatus::NoRemote, true),
        ];
        assert_eq!(pick_departure(&list, false), ["ahead", "dirty-auto"]);
        assert_eq!(pick_departure(&list, true), ["ahead", "dirty-auto", "diverged"], "打开自动合并时分叉的也要试着推");
        assert!(pick_departure(&[project("a", SyncStatus::Synced, false)], true).is_empty(), "没有要推的就不联网");
    }

    fn result(items: &[(&str, SyncOutcome)]) -> SyncResult {
        let items: Vec<SyncItemResult> = items
            .iter()
            .map(|(id, o)| SyncItemResult { project_id: id.to_string(), outcome: *o, commits: 1, reason: None })
            .collect();
        let failed_count = items.iter().filter(|i| i.outcome == SyncOutcome::Failed).count();
        SyncResult {
            started_at: String::new(),
            finished_at: String::new(),
            synced_count: items.len() - failed_count,
            failed_count,
            items,
        }
    }

    #[test]
    fn notifications() {
        let names: HashMap<String, String> =
            ["a", "b", "c", "d", "e"].iter().map(|n| (n.to_string(), format!("项目{}", n.to_uppercase()))).collect();
        let sw = Switches::DEFAULT;
        let both = Switches { notify_on_success: true, ..sw };
        let quiet = Switches { notify_on_error: false, notify_on_success: false, ..sw };
        use SyncOutcome::*;

        // 出错：默认通知，一轮只发一条
        let bad = result(&[("a", Failed), ("b", Pushed), ("c", Failed)]);
        let (title, body) = notification_text(Unlock, &bad, &names, &sw).unwrap();
        assert_eq!(title, "Git SyncDeck · 解锁后同步");
        assert_eq!(body, "2 个项目需要处理：项目A、项目C");
        assert!(notification_text(Unlock, &bad, &names, &quiet).is_none());
        let many = result(&[("a", Failed), ("b", Failed), ("c", Failed), ("d", Failed), ("e", Failed)]);
        assert_eq!(notification_text(Lock, &many, &names, &sw).unwrap().1, "5 个项目需要处理：项目A、项目B、项目C 等");

        // 成功：默认不通知；打开后只在确实推送 / 拉取了东西时通知
        let good = result(&[("a", Pushed), ("b", UpToDate), ("c", Pulled), ("d", Committed)]);
        assert!(notification_text(Manual, &good, &names, &sw).is_none());
        assert_eq!(notification_text(Manual, &good, &names, &both).unwrap().1, "已同步 3 个项目：项目A、项目C、项目D");
        let nothing = result(&[("a", UpToDate)]);
        assert!(notification_text(Manual, &nothing, &names, &both).is_none());
    }

    #[test]
    fn retry_result_replaces_first_attempt() {
        use SyncOutcome::*;
        let mut first = result(&[("a", Failed), ("b", Pushed), ("c", Failed)]);
        merge_retry(&mut first, result(&[("a", Pulled)]));
        assert_eq!(first.items[0].outcome, Pulled);
        assert_eq!((first.synced_count, first.failed_count), (2, 1));
    }

    #[test]
    fn arrival_waits_only_for_departure_leftovers() {
        // 正在跑的是离开类（锁屏）：到达类先等它结束
        for t in [Resume, Unlock, Startup, Manual] {
            assert!(waits_for_departure(t, Some(Lock)), "{t:?}");
        }
        // 正在跑的是到达类、或者没在跑：不等（到达类遇到到达类直接跳过）
        for running in [None, Some(Unlock), Some(Manual), Some(Resume)] {
            assert!(!waits_for_departure(Resume, running), "{running:?}");
        }
        // 离开类、定时检查不等
        assert!(!waits_for_departure(Lock, Some(Lock)));
        assert!(!waits_for_departure(Periodic, Some(Lock)));
    }

    fn empty_state(name: &str) -> Shared {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir); // 测试自己的临时配置目录
        Arc::new(crate::commands::AppState::new(crate::config::ConfigStore::load(dir.join("config.json"))))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn resume_after_lock_leftover_still_syncs() {
        // 合盖时先锁屏再睡眠：锁屏那一轮被睡眠打断，唤醒后还没跑完
        let st = empty_state("syncdeck-autosync-unit-leftover");
        let leftover = st.auto.begin(Lock).unwrap();
        let st2 = st.clone();
        let resume = tokio::spawn(async move { handle(&st2, &NoHost, Resume).await });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!resume.is_finished(), "唤醒的到达同步在等锁屏那一轮");
        drop(leftover);
        let outcome = resume.await.unwrap();
        assert!(matches!(outcome, Outcome::Synced(_)), "锁屏那一轮结束后照常做到达同步：{outcome:?}");
        // 同属到达的解锁紧接着来：10 分钟内不再做
        assert!(matches!(handle(&st, &NoHost, Unlock).await, Outcome::Skipped(Decision::TooSoon)));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn arrival_skips_when_another_arrival_is_running() {
        let st = empty_state("syncdeck-autosync-unit-busy");
        let _running = st.auto.begin(Manual).unwrap();
        assert!(matches!(handle(&st, &NoHost, Resume).await, Outcome::Busy));
    }
}
