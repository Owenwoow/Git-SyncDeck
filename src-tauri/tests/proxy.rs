//! 代理覆盖真的作用到 SyncDeck 调用的 git 上：远程地址用一个解析不了的 https 地址（.invalid），
//! 代理指向本机一个没人监听的端口，看 git 报的错是"连不上这个代理"还是"解析不了主机"（说明没走代理）。
//! 不访问任何真实服务器；用 GIT_CONFIG_GLOBAL / GIT_CONFIG_NOSYSTEM 隔离本机的 git 配置，不读也不改它。
//! 这个文件里只有一个测试，因为它要改本进程的环境变量。
//! 运行：cd src-tauri && cargo test --test proxy -- --nocapture

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use git_syncdeck_lib::git::{run_git, set_proxy_override, GitError, ProxyOverride};

const REMOTE: &str = "https://syncdeck-test.invalid/r.git";

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").args(args).current_dir(dir).output().expect("git");
    assert!(out.status.success(), "git {args:?} 失败：{}", String::from_utf8_lossy(&out.stderr));
}

async fn fetch_error(dir: &Path) -> String {
    match run_git(dir, &["fetch", "--quiet", "origin"], Duration::from_secs(60)).await {
        Ok(_) => panic!("fetch 一个 .invalid 地址不应该成功"),
        Err(e @ GitError::Failed { .. }) => e.message(),
        Err(e) => panic!("应该是 git 报错：{e:?}"),
    }
}

#[tokio::test]
async fn proxy_override_reaches_git() {
    let root = std::env::temp_dir().join("syncdeck-proxy-test");
    let _ = std::fs::remove_dir_all(&root); // 测试自己的临时目录
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let empty_global = root.join("empty.gitconfig");
    std::fs::write(&empty_global, "").unwrap();

    // 隔离本机的 git 配置（比如全局 http.proxy），只留下这个测试设的环境变量代理
    std::env::set_var("GIT_CONFIG_GLOBAL", &empty_global);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    for var in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"] {
        std::env::remove_var(var);
    }
    std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:9");

    git(&repo, &["init", "--quiet"]);
    git(&repo, &["remote", "add", "origin", REMOTE]);

    // 不覆盖：git 用环境变量里的代理（证明下面"禁用"确实盖过了环境变量）
    set_proxy_override(ProxyOverride::Inherit);
    let e = fetch_error(&repo).await;
    println!("不覆盖：{e}");
    assert!(e.contains("127.0.0.1 port 9"), "应该走环境变量里的代理：{e}");

    // 使用指定代理：-c http.proxy 优先于环境变量
    set_proxy_override(ProxyOverride::Use("http://127.0.0.1:8".into()));
    let e = fetch_error(&repo).await;
    println!("使用 127.0.0.1:8：{e}");
    assert!(e.contains("127.0.0.1 port 8"), "应该走设置里的代理：{e}");

    // socks5 也能用
    set_proxy_override(ProxyOverride::Use("socks5://127.0.0.1:8".into()));
    let e = fetch_error(&repo).await;
    println!("使用 socks5://127.0.0.1:8：{e}");
    assert!(e.contains("127.0.0.1 port 8"), "应该走 socks5 代理：{e}");

    // 禁用：连环境变量里的代理也不走，直接去解析那个（解析不了的）主机
    set_proxy_override(ProxyOverride::Disable);
    let e = fetch_error(&repo).await;
    println!("禁用：{e}");
    assert!(!e.contains("127.0.0.1"), "不应该走任何代理：{e}");
    assert!(!e.to_lowercase().contains("resolve proxy"), "不应该走任何代理：{e}");

    set_proxy_override(ProxyOverride::Inherit);
    let _ = std::fs::remove_dir_all(&root);
}
