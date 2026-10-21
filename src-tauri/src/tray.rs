//! 托盘：图标右下角的色点表示整体状态（红：要处理；黄：本机有东西没同步；蓝：正在同步；绿：其他），
//! 悬停显示摘要；菜单：立即同步、打开主界面、暂停自动同步（到明天）/ 恢复自动同步、退出。左键单击打开主界面。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Local};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::autosync::{self, Host};
use crate::commands::{self, Shared};
use crate::model::{AutoSyncTrigger, Project, SyncStatus};

/// 前端监听的事件：托盘里暂停 / 恢复后推送最新设置
pub const EVENT_SETTINGS_CHANGED: &str = "settings-changed";

const MENU_SYNC_NOW: &str = "sync_now";
const MENU_SHOW: &str = "show";
const MENU_PAUSE: &str = "pause";
const MENU_QUIT: &str = "quit";
const PAUSE_TEXT: &str = "暂停自动同步（到明天）";
const RESUME_TEXT: &str = "恢复自动同步";

/// 托盘图标的状态色
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayColor {
    Green,
    Yellow,
    Red,
    Blue,
}

impl TrayColor {
    const ALL: [TrayColor; 4] = [TrayColor::Green, TrayColor::Yellow, TrayColor::Red, TrayColor::Blue];

    fn rgb(self) -> [u8; 3] {
        match self {
            TrayColor::Green => [0x16, 0xa3, 0x4a],
            TrayColor::Yellow => [0xf5, 0x9e, 0x0b],
            TrayColor::Red => [0xdc, 0x26, 0x26],
            TrayColor::Blue => [0x25, 0x63, 0xeb],
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }
}

/// 托盘的颜色和悬停文字（不含"Git SyncDeck · "前缀）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraySummary {
    pub color: TrayColor,
    pub text: String,
}

/// 按项目状态算托盘的颜色和摘要：
/// - 红：有项目分叉、有异常标记或检查失败
/// - 黄：有项目待推送或有未提交改动
/// - 蓝：正在同步
/// - 绿：其他（包括只是待拉取的）
pub fn summarize(projects: &[Project], syncing: bool) -> TraySummary {
    let attention = |p: &Project| p.status == SyncStatus::Diverged || p.issue.is_some() || p.check_error.is_some();
    let count = |f: &dyn Fn(&Project) -> bool| projects.iter().filter(|p| f(p)).count();
    let bad = count(&|p| attention(p));
    let pending = count(&|p| !attention(p) && matches!(p.status, SyncStatus::Ahead | SyncStatus::Dirty));
    let behind = count(&|p| !attention(p) && p.status == SyncStatus::Behind);
    let synced = count(&|p| !attention(p) && p.status == SyncStatus::Synced);
    let no_remote = count(&|p| !attention(p) && p.status == SyncStatus::NoRemote);

    let color = if syncing {
        TrayColor::Blue
    } else if bad > 0 {
        TrayColor::Red
    } else if pending > 0 {
        TrayColor::Yellow
    } else {
        TrayColor::Green
    };
    let text = if syncing {
        "正在同步…".to_string()
    } else if projects.is_empty() {
        "还没有监控项目".to_string()
    } else {
        let mut parts = Vec::new();
        if bad > 0 {
            parts.push(format!("{bad} 个项目需要处理"));
        }
        if pending > 0 {
            parts.push(format!("{pending} 个项目待推送或有改动"));
        }
        if behind > 0 {
            parts.push(format!("{behind} 个项目待拉取"));
        }
        if parts.is_empty() {
            parts.push(format!("{synced} 个项目已同步"));
            if no_remote > 0 {
                parts.push(format!("{no_remote} 个未关联云端"));
            }
        }
        parts.join("，")
    };
    TraySummary { color, text }
}

/// 在图标右下角画一个状态色点（带抗锯齿），色点外一圈挖空，和图标本体分开
pub fn with_badge(rgba: &[u8], width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    let size = width.min(height) as f32;
    let r = size * 0.28;
    let gap = (size * 0.07).max(1.0);
    let (cx, cy) = (width as f32 - r - 0.5, height as f32 - r - 0.5);
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            if i + 3 >= out.len() {
                return out;
            }
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let cut = (r + gap + 0.5 - d).clamp(0.0, 1.0);
            if cut > 0.0 {
                out[i + 3] = (f32::from(out[i + 3]) * (1.0 - cut)).round() as u8;
            }
            let fill = (r + 0.5 - d).clamp(0.0, 1.0);
            if fill > 0.0 {
                // 非预乘 alpha 的 "over" 混合
                let dst_a = f32::from(out[i + 3]) / 255.0;
                let a = fill + dst_a * (1.0 - fill);
                for c in 0..3 {
                    let v = (f32::from(rgb[c]) * fill + f32::from(out[i + c]) * dst_a * (1.0 - fill)) / a;
                    out[i + c] = v.round().clamp(0.0, 255.0) as u8;
                }
                out[i + 3] = (a * 255.0).round() as u8;
            }
        }
    }
    out
}

/// 托盘的句柄和上次显示的内容（只在变化时才更新）
pub struct Tray {
    icon: TrayIcon,
    pause_item: MenuItem<tauri::Wry>,
    icons: Vec<Image<'static>>,
    /// 已经排队等主线程更新，期间的其他更新请求合并掉
    pending: AtomicBool,
    /// 上次显示的（颜色、悬停文字、菜单是否显示"恢复自动同步"）
    last: Mutex<Option<(TrayColor, String, bool)>>,
    /// 已经安排了在这个时间（暂停到期）刷新一次托盘
    expiry_timer: Mutex<Option<DateTime<Local>>>,
}

/// 点菜单里"暂停 / 恢复"后要保存的暂停时间：按菜单上**当前显示**的文字办事——
/// 显示"恢复自动同步"就恢复（None），否则暂停到明天 00:00。
/// 不能按此刻是否还在暂停中判断：暂停刚过期、菜单还没来得及刷新时，用户点"恢复"会被反过来暂停到下一个午夜
pub fn pause_menu_target(shows_resume: bool, now: &DateTime<Local>) -> Option<DateTime<Local>> {
    (!shows_resume).then(|| autosync::next_midnight(now))
}

/// 创建托盘。`host` 用于托盘"立即同步"后发事件和通知
pub fn create(app: &AppHandle, st: Shared, host: Arc<dyn Host>) -> tauri::Result<()> {
    let sync_now = MenuItem::with_id(app, MENU_SYNC_NOW, "立即同步", true, None::<&str>)?;
    let show = MenuItem::with_id(app, MENU_SHOW, "打开主界面", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, MENU_PAUSE, PAUSE_TEXT, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &sync_now,
            &show,
            &PredefinedMenuItem::separator(app)?,
            &pause,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let base = Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
    let (w, h) = (base.width(), base.height());
    let icons: Vec<Image<'static>> =
        TrayColor::ALL.iter().map(|c| Image::new_owned(with_badge(base.rgba(), w, h, c.rgb()), w, h)).collect();

    let icon = TrayIconBuilder::with_id("main")
        .icon(icons[TrayColor::Green.index()].clone())
        .tooltip("Git SyncDeck")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| on_menu(app, event.id().as_ref(), &st, &host))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    app.manage(Tray {
        icon,
        pause_item: pause,
        icons,
        pending: AtomicBool::new(false),
        last: Mutex::new(None),
        expiry_timer: Mutex::new(None),
    });
    request_update(app);
    Ok(())
}

fn on_menu(app: &AppHandle, id: &str, st: &Shared, host: &Arc<dyn Host>) {
    match id {
        MENU_SYNC_NOW => autosync::spawn(st.clone(), host.clone(), AutoSyncTrigger::Manual),
        MENU_SHOW => show_main_window(app),
        MENU_PAUSE => {
            let shows_resume = app
                .try_state::<Tray>()
                .and_then(|t| t.last.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|l| l.2))
                .unwrap_or(false);
            let until = pause_menu_target(shows_resume, &Local::now());
            if let Err(e) = commands::set_auto_sync_pause(st, until) {
                eprintln!("保存暂停状态失败：{e}");
                return;
            }
            // 窗口开着时，主页的"已暂停"提示跟着变
            let (app, st) = (app.clone(), st.clone());
            tauri::async_runtime::spawn(async move {
                let _ = app.emit(EVENT_SETTINGS_CHANGED, commands::current_settings(&st).await);
            });
        }
        MENU_QUIT => quit(app),
        _ => {}
    }
}

/// 显示主窗口并放到最前面
/// 彻底退出 SyncDeck（托盘菜单「退出」和设置页「退出 SyncDeck」共用）
pub fn quit<R: Runtime>(app: &AppHandle<R>) {
    app.exit(0);
}

pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// 请求按最新状态更新托盘（任意线程都能调；实际更新在主线程里做，连续的请求会合并）
pub fn request_update(app: &AppHandle) {
    let Some(tray) = app.try_state::<Tray>() else { return };
    if tray.pending.swap(true, Ordering::SeqCst) {
        return;
    }
    let handle = app.clone();
    let posted = app.run_on_main_thread(move || {
        if let Some(tray) = handle.try_state::<Tray>() {
            tray.pending.store(false, Ordering::SeqCst);
            apply(&handle, &tray);
        }
    });
    if posted.is_err() {
        tray.pending.store(false, Ordering::SeqCst);
    }
}

fn apply(app: &AppHandle, tray: &Tray) {
    let st = app.state::<Shared>();
    let now = Local::now();
    let paused = st.config.read(|c| autosync::active_pause(&c.settings, &now));
    let summary = summarize(&st.snapshot(), st.is_syncing());
    let mut tooltip = format!("Git SyncDeck · {}", summary.text);
    if let Some(until) = &paused {
        tooltip.push_str(&format!("\n自动同步已暂停到 {}", autosync::describe_until(until, &now)));
    }
    let next = (summary.color, tooltip, paused.is_some());
    let mut last = tray.last.lock().unwrap_or_else(|e| e.into_inner());
    let prev = last.replace(next.clone());
    drop(last);
    if prev.as_ref().map(|p| p.0) != Some(next.0) {
        let _ = tray.icon.set_icon(Some(tray.icons[next.0.index()].clone()));
    }
    if prev.as_ref().map(|p| &p.1) != Some(&next.1) {
        let _ = tray.icon.set_tooltip(Some(&next.1));
    }
    if prev.as_ref().map(|p| p.2) != Some(next.2) {
        let _ = tray.pause_item.set_text(if next.2 { RESUME_TEXT } else { PAUSE_TEXT });
    }
    if let Some(until) = paused {
        schedule_expiry_refresh(app, tray, until);
    }
}

/// 暂停到期时刷新一次托盘（菜单文字变回"暂停自动同步"、悬停文字去掉暂停那一行）。
/// 同一个到期时间只安排一次；睡过了头也没关系，唤醒后的同步会触发刷新，而且菜单按显示的文字办事
fn schedule_expiry_refresh(app: &AppHandle, tray: &Tray, until: DateTime<Local>) {
    let mut scheduled = tray.expiry_timer.lock().unwrap_or_else(|e| e.into_inner());
    if *scheduled == Some(until) {
        return;
    }
    *scheduled = Some(until);
    drop(scheduled);
    let wait = (until - Local::now()).to_std().unwrap_or_default() + Duration::from_secs(1);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(wait).await;
        request_update(&app);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str, status: SyncStatus) -> Project {
        Project {
            id: name.into(),
            name: name.into(),
            path: String::new(),
            branch: "main".into(),
            remote_url: None,
            upstream: None,
            status,
            ahead: 0,
            behind: 0,
            changes: Vec::new(),
            last_sync_at: None,
            issue: None,
            check_error: None,
            auto_commit: false,
        }
    }

    #[test]
    fn colors_and_tooltips() {
        let s = |list: &[Project], syncing| summarize(list, syncing);
        assert_eq!(s(&[], false), TraySummary { color: TrayColor::Green, text: "还没有监控项目".into() });

        let synced: Vec<_> = (0..7).map(|i| project(&format!("p{i}"), SyncStatus::Synced)).collect();
        assert_eq!(s(&synced, false), TraySummary { color: TrayColor::Green, text: "7 个项目已同步".into() });
        assert_eq!(s(&synced, true).color, TrayColor::Blue, "正在同步优先");
        assert_eq!(s(&synced, true).text, "正在同步…");

        // 只是待拉取 / 未关联云端：仍是绿色，悬停文字里说明
        let mut list = synced.clone();
        list.push(project("b", SyncStatus::Behind));
        assert_eq!(s(&list, false), TraySummary { color: TrayColor::Green, text: "1 个项目待拉取".into() });
        let mut list = synced.clone();
        list.push(project("n", SyncStatus::NoRemote));
        assert_eq!(s(&list, false).text, "7 个项目已同步，1 个未关联云端");

        // 待推送 / 有未提交改动 → 黄
        let mut list = synced.clone();
        list.push(project("a", SyncStatus::Ahead));
        list.push(project("d", SyncStatus::Dirty));
        assert_eq!(s(&list, false), TraySummary { color: TrayColor::Yellow, text: "2 个项目待推送或有改动".into() });

        // 分叉、异常标记、检查失败 → 红
        let mut issue = project("i", SyncStatus::Synced);
        issue.issue = Some("推送被拒".into());
        let mut failed = project("f", SyncStatus::Ahead);
        failed.check_error = Some("连不上 GitHub".into());
        list.extend([project("x", SyncStatus::Diverged), issue, failed]);
        assert_eq!(
            s(&list, false),
            TraySummary { color: TrayColor::Red, text: "3 个项目需要处理，2 个项目待推送或有改动".into() }
        );
    }

    #[test]
    fn pause_menu_follows_what_it_shows() {
        let now = Local::now();
        // 菜单显示"恢复自动同步"：不管此刻暂停是否已经过期，点了就是恢复
        assert_eq!(pause_menu_target(true, &now), None);
        // 菜单显示"暂停自动同步（到明天）"：暂停到明天 00:00
        let until = pause_menu_target(false, &now).unwrap();
        assert_eq!(until, autosync::next_midnight(&now));
        assert!(until > now);
    }

    #[test]
    fn badge_paints_bottom_right_only() {
        let (w, h) = (32u32, 32u32);
        let base: Vec<u8> = std::iter::repeat([10u8, 20, 30, 255]).take((w * h) as usize).flatten().collect();
        let out = with_badge(&base, w, h, [255, 0, 0]);
        let px = |x: u32, y: u32| &out[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize];
        assert_eq!(px(0, 0), [10, 20, 30, 255], "左上角不变");
        assert_eq!(px(23, 23), [255, 0, 0, 255], "色点中心是状态色");
        // 色点和图标之间挖空了一圈（有透明像素）
        assert!((0..w).any(|x| px(x, 14)[3] < 255 || px(14, x)[3] < 255));
        assert_eq!(out.len(), base.len());
    }
}
