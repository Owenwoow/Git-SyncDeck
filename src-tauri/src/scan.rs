//! 在目录里递归查找 Git 仓库：最多 3 层；找到仓库后不再进入其子目录；跳过依赖、构建产物和隐藏目录；不跟随符号链接 / 目录联接。

use std::path::{Path, PathBuf};

pub const MAX_DEPTH: usize = 3;

/// 不进入的目录（小写比较）。以 . 开头的目录（.venv、.idea、.cache……）也一律跳过。
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "venv",
    "env",
    "target",
    "dist",
    "build",
    "out",
    "vendor",
    "__pycache__",
    "bower_components",
    "appdata",
    "$recycle.bin",
    "system volume information",
];

pub fn find_repos(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(root, 0, &mut found);
    found.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    found
}

/// `.git` 可以是目录，也可以是文件（worktree、子模块）
pub fn is_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

fn walk(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if is_repo(dir) {
        found.push(dir.to_path_buf());
        return;
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // 没有权限等情况直接跳过
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        // file_type 不跟随链接：符号链接和目录联接在这里不是目录
        if !ft.is_dir() || ft.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        walk(&entry.path(), depth + 1, found);
    }
}
