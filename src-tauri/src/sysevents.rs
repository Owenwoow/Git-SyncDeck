//! Windows 系统事件：锁屏 / 解锁、睡眠 / 唤醒、关机 / 重启 / 注销。
//! 起一个看不见的顶层窗口和它自己的消息循环线程（message-only 窗口收不到广播和关机消息）。
//! 这个窗口必须"可见"（WS_VISIBLE，但 0×0、不进任务栏和 Alt+Tab、不抢焦点）：Windows 不允许没有可见窗口的
//! 程序阻止关机，5 秒内不回复 WM_QUERYENDSESSION 就直接结束进程。开机自启（从没弹窗口）或主窗口关到托盘时，
//! 这是整个进程唯一的可见窗口。
//!
//! 监听的事件：
//! - `WTSRegisterSessionNotification`：锁屏 → 离开；解锁 → 到达
//! - `WM_POWERBROADCAST`：`PBT_APMSUSPEND` → 离开（Windows 只给几秒，尽力而为，不阻塞）；
//!   `PBT_APMRESUMEAUTOMATIC` / `PBT_APMRESUMESUSPEND` → 到达
//! - `WM_QUERYENDSESSION` / `WM_ENDSESSION`：关机、重启、注销 → 离开。用 `ShutdownBlockReasonCreate` 显示
//!   "Git SyncDeck 正在同步…"，阻塞到同步完成或超时（最长时间由 `on_end_session` 自己控制），然后放行，
//!   绝不拒绝关机（总是返回 TRUE）

/// 不需要阻塞的系统事件
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysEvent {
    Lock,
    Unlock,
    Suspend,
    Resume,
}

pub struct Handlers {
    /// 锁屏、解锁、睡眠、唤醒：在消息线程里调用，不能阻塞（把活儿丢到后台去做）
    pub on_event: Box<dyn Fn(SysEvent) + Send + Sync>,
    /// 关机 / 重启 / 注销：阻塞到同步完成或超时，返回后放行关机
    pub on_end_session: Box<dyn Fn() + Send + Sync>,
}

/// 启动监听（只能调用一次）。失败时返回原因，程序其他功能照常
#[cfg(windows)]
pub fn start(handlers: Handlers) -> Result<(), String> {
    imp::start(handlers)
}

#[cfg(not(windows))]
pub fn start(_handlers: Handlers) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::RemoteDesktop::{WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION};
    use windows_sys::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage,
        ENDSESSION_CLOSEAPP, MSG, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND, WM_ENDSESSION,
        WM_POWERBROADCAST, WM_QUERYENDSESSION, WM_WTSSESSION_CHANGE, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_POPUP, WS_VISIBLE, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
    };

    use super::{Handlers, SysEvent};

    static HANDLERS: OnceLock<Handlers> = OnceLock::new();
    /// 这次关机已经处理过（WM_QUERYENDSESSION 之后还会来 WM_ENDSESSION）；关机被取消时清除，下次关机再做
    static ENDING: AtomicBool = AtomicBool::new(false);

    const CLASS_NAME: &str = "GitSyncDeckSystemEvents";
    const BLOCK_REASON: &str = "Git SyncDeck 正在同步…";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn start(handlers: Handlers) -> Result<(), String> {
        if HANDLERS.set(handlers).is_err() {
            return Err("系统事件监听已经启动过了".into());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("syncdeck-sysevents".into())
            .spawn(move || {
                // SAFETY: 窗口在这个线程里创建，消息循环也在这个线程里跑
                match unsafe { create_window() } {
                    Ok(()) => {
                        let _ = tx.send(Ok(()));
                        unsafe { message_loop() };
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                    }
                }
            })
            .map_err(|e| format!("无法创建系统事件线程：{e}"))?;
        rx.recv().map_err(|e| format!("系统事件线程意外退出：{e}"))?
    }

    unsafe fn create_window() -> Result<(), String> {
        let hwnd = create_event_window()?;
        if WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) == 0 {
            // 收不到锁屏 / 解锁，睡眠和关机照常
            eprintln!("注册锁屏通知失败：{}", std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// 注册窗口类并创建接收系统事件的窗口（在调用线程里，消息也要在这个线程里取）
    pub(super) unsafe fn create_event_window() -> Result<HWND, String> {
        let hinstance = GetModuleHandleW(std::ptr::null());
        let class = wide(CLASS_NAME);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            lpszClassName: class.as_ptr(),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(format!("注册窗口类失败：{}", std::io::Error::last_os_error()));
        }
        let title = wide("Git SyncDeck 系统事件");
        // 顶层窗口，能收到广播和关机消息。带 WS_VISIBLE 让系统把它当作可见窗口（这样关机时才允许它阻止关机，
        // 见文件开头），但 0×0 大小、WS_EX_TOOLWINDOW 不进任务栏和 Alt+Tab、WS_EX_NOACTIVATE 不抢焦点，用户看不到
        let hwnd: HWND = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return Err(format!("创建系统事件窗口失败：{}", std::io::Error::last_os_error()));
        }
        Ok(hwnd)
    }

    unsafe fn message_loop() {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    /// 只是安装程序要关掉本程序（Restart Manager），不是关机或注销
    fn closes_app_only(lparam: LPARAM) -> bool {
        (lparam as u32) & ENDSESSION_CLOSEAPP != 0
    }

    fn fire(h: &Handlers, e: SysEvent) {
        // panic 不能穿过 extern "system" 边界
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (h.on_event)(e)));
    }

    /// 关机前同步：显示阻止关机的原因，阻塞到同步完成或超时，然后撤掉原因
    unsafe fn block_for_sync(hwnd: HWND, h: &Handlers) {
        let reason = wide(BLOCK_REASON);
        let blocked = ShutdownBlockReasonCreate(hwnd, reason.as_ptr()) != 0;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (h.on_end_session)()));
        if blocked {
            ShutdownBlockReasonDestroy(hwnd);
        }
    }

    unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let Some(h) = HANDLERS.get() else {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        match msg {
            WM_WTSSESSION_CHANGE => {
                match wparam as u32 {
                    WTS_SESSION_LOCK => fire(h, SysEvent::Lock),
                    WTS_SESSION_UNLOCK => fire(h, SysEvent::Unlock),
                    _ => {}
                }
                0
            }
            WM_POWERBROADCAST => {
                match wparam as u32 {
                    PBT_APMSUSPEND => fire(h, SysEvent::Suspend),
                    PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => fire(h, SysEvent::Resume),
                    _ => {}
                }
                1
            }
            WM_QUERYENDSESSION => {
                if !closes_app_only(lparam) && !ENDING.swap(true, Ordering::SeqCst) {
                    block_for_sync(hwnd, h);
                }
                1 // 总是放行
            }
            WM_ENDSESSION => {
                if wparam == 0 {
                    // 关机被取消了（别的程序拦下或用户取消）：下次关机还要同步
                    ENDING.store(false, Ordering::SeqCst);
                } else if !closes_app_only(lparam) && !ENDING.swap(true, Ordering::SeqCst) {
                    // 没收到 WM_QUERYENDSESSION 就直接结束会话：尽力同步一下
                    block_for_sync(hwnd, h);
                }
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    #[cfg(test)]
    mod tests {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DestroyWindow, GetWindowLongW, GetWindowRect, IsWindowVisible, GWL_EXSTYLE, WS_EX_APPWINDOW,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        };

        /// 系统只允许有可见窗口的程序阻止关机：事件窗口要"可见"，但 0×0、不进任务栏、不抢焦点
        #[test]
        fn event_window_counts_as_visible_but_takes_no_space() {
            unsafe {
                let hwnd = super::create_event_window().unwrap();
                assert!(IsWindowVisible(hwnd) != 0, "必须是可见窗口，否则关机时 5 秒就被结束");
                let mut rect: RECT = std::mem::zeroed();
                assert!(GetWindowRect(hwnd, &mut rect) != 0);
                assert_eq!((rect.right - rect.left, rect.bottom - rect.top), (0, 0), "0×0，用户看不到");
                let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
                assert!(ex & WS_EX_TOOLWINDOW != 0 && ex & WS_EX_APPWINDOW == 0, "不进任务栏和 Alt+Tab");
                assert!(ex & WS_EX_NOACTIVATE != 0, "不抢焦点");
                DestroyWindow(hwnd);
            }
        }
    }
}
