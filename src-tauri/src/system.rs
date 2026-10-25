//! 打开终端 / VS Code / 项目主页。这里启动的是给用户看的窗口。

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

/// 只打开固定的项目主页，不接受前端传入网址或命令。
pub fn open_project_homepage() -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
        let url: Vec<u16> = "https://github.com/Owenwoow/Git-SyncDeck\0".encode_utf16().collect();
        // ShellExecuteW 交给系统默认浏览器；返回值大于 32 表示启动成功。
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                std::ptr::null(),
                url.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        } as isize;
        if result > 32 {
            Ok(())
        } else {
            Err(format!("无法打开默认浏览器（系统错误码：{result}），请手动访问 https://github.com/Owenwoow/Git-SyncDeck"))
        }
    }
    #[cfg(not(windows))]
    Err("此功能仅支持 Windows，请手动访问 https://github.com/Owenwoow/Git-SyncDeck".into())
}

/// 终端优先用 Windows Terminal（wt -d 路径），没有则用 cmd
pub fn open_terminal(dir: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("找不到目录：{}", dir.display()));
    }
    let mut candidates = vec![PathBuf::from("wt.exe")];
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Microsoft").join("WindowsApps").join("wt.exe"));
    }
    for wt in candidates {
        if Command::new(&wt).arg("-d").arg(dir).spawn().is_ok() {
            return Ok(());
        }
    }
    let mut cmd = Command::new("cmd.exe");
    cmd.arg("/K").current_dir(dir);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NEW_CONSOLE);
    cmd.spawn().map(|_| ()).map_err(|e| format!("无法打开终端：{e}"))
}

/// 在 VS Code 中打开。直接启动 Code.exe（不经过 cmd，路径里有特殊字符也安全）
pub fn open_editor(dir: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("找不到目录：{}", dir.display()));
    }
    let exe = find_vscode()
        .ok_or_else(|| "未找到 VS Code。请先安装 VS Code（安装时勾选\"添加到 PATH\"）".to_string())?;
    Command::new(exe).arg(dir).spawn().map(|_| ()).map_err(|e| format!("无法启动 VS Code：{e}"))
}

fn find_vscode() -> Option<PathBuf> {
    // PATH 里的 ...\Microsoft VS Code\bin\code.cmd → ...\Microsoft VS Code\Code.exe
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            if dir.join("code.cmd").is_file() {
                if let Some(exe) = dir.parent().map(|d| d.join("Code.exe")) {
                    if exe.is_file() {
                        return Some(exe);
                    }
                }
            }
        }
    }
    // 常见安装位置
    let mut bases = Vec::new();
    if let Some(v) = std::env::var_os("LOCALAPPDATA") {
        bases.push(PathBuf::from(v).join("Programs"));
    }
    if let Some(v) = std::env::var_os("ProgramFiles") {
        bases.push(PathBuf::from(v));
    }
    bases
        .into_iter()
        .map(|b| b.join("Microsoft VS Code").join("Code.exe"))
        .find(|exe| exe.is_file())
}
