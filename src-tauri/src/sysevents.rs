//! Windows 系统事件：锁屏 / 解锁、从睡眠唤醒。
//! 起一个隐藏的顶层窗口和它自己的消息循环线程（message-only 窗口收不到广播）。
//!
//! 监听的事件：
//! - `WTSRegisterSessionNotification`：锁屏 → 离开；解锁 → 到达
//! - `WM_POWERBROADCAST`：`PBT_APMRESUMEAUTOMATIC` / `PBT_APMRESUMESUSPEND` → 到达
//!
//! 关机和睡眠前不同步（Windows 给的时间太短，实际用起来不可靠）：没推上去的提交由下次开机、唤醒或解锁时的
//! 到达同步补上。

/// 系统事件
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysEvent {
    Lock,
    Unlock,
    Resume,
}

pub struct Handlers {
    /// 锁屏、解锁、唤醒：在消息线程里调用，不能阻塞（把活儿丢到后台去做）
    pub on_event: Box<dyn Fn(SysEvent) + Send + Sync>,
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
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::RemoteDesktop::{WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage, MSG,
        PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, WM_POWERBROADCAST, WM_WTSSESSION_CHANGE, WNDCLASSW,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
    };

    use super::{Handlers, SysEvent};

    static HANDLERS: OnceLock<Handlers> = OnceLock::new();

    const CLASS_NAME: &str = "GitSyncDeckSystemEvents";

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
            // 收不到锁屏 / 解锁，唤醒照常
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
        // 隐藏的顶层窗口，能收到广播；0×0 大小、WS_EX_TOOLWINDOW 不进任务栏和 Alt+Tab、WS_EX_NOACTIVATE 不抢焦点
        let hwnd: HWND = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
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

    fn fire(h: &Handlers, e: SysEvent) {
        // panic 不能穿过 extern "system" 边界
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (h.on_event)(e)));
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
                    PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => fire(h, SysEvent::Resume),
                    _ => {}
                }
                1
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

        /// 事件窗口用户看不到：隐藏、0×0、不进任务栏、不抢焦点
        #[test]
        fn event_window_is_invisible() {
            unsafe {
                let hwnd = super::create_event_window().unwrap();
                assert!(IsWindowVisible(hwnd) == 0, "隐藏窗口");
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
