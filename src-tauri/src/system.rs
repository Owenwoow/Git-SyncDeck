//! 打开终端 / VS Code。这里启动的是给用户看的窗口，所以不加 CREATE_NO_WINDOW。

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

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
