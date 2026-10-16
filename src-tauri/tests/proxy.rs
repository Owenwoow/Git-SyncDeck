//! 代理设置真的作用到 SyncDeck 调用的 git 上：远程地址用一个解析不了的 https 地址（.invalid），
//! 代理指向本机没人监听的端口，看 git 报的错是"连不上这个代理"还是"解析不了主机"（说明没走代理）。
//! 不访问任何真实服务器；用 GIT_CONFIG_GLOBAL / GIT_CONFIG_NOSYSTEM 把 git 的全局配置换成测试自己的临时文件，
//! 不读也不改本机的 git 配置。这个文件里只有一个测试，因为它要改本进程的环境变量。
//! 运行：cd src-tauri && cargo test --test proxy -- --nocapture

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use git_syncdeck_lib::git::{run_git, set_proxy_plan, GitError, ProxyOverride, ProxyPlan};
use git_syncdeck_lib::ops;

const HOST: &str = "https://syncdeck-test.invalid";

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

/// 按这个代理设置 fetch 一次，返回 git 的报错
async fn fetch_with(dir: &Path, label: &str, plan: ProxyPlan) -> String {
    set_proxy_plan(plan);
    let e = fetch_error(dir).await;
    println!("{label}：{e}");
    e
}

fn no_proxy_used(e: &str) -> bool {
    !e.contains("127.0.0.1") && !e.to_lowercase().contains("resolve proxy")
}

#[tokio::test]
async fn proxy_override_reaches_git() {
    let root = std::env::temp_dir().join("syncdeck-proxy-test");
    let _ = std::fs::remove_dir_all(&root); // 测试自己的临时目录
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let fake_global = root.join("fake.gitconfig");
    std::fs::write(&fake_global, "").unwrap();

    // 隔离本机的 git 配置（比如全局 http.proxy），只留下这个测试设的环境变量代理
    std::env::set_var("GIT_CONFIG_GLOBAL", &fake_global);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    for var in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy", "NO_PROXY", "no_proxy"] {
        std::env::remove_var(var);
    }
    std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:9");

    git(&repo, &["init", "--quiet"]);
    git(&repo, &["remote", "add", "origin", &format!("{HOST}/r.git")]);

    // ---------------- 只有环境变量里的代理 ----------------
    // 不覆盖：git 用环境变量里的代理（证明下面"禁用"确实盖过了环境变量）
    let e = fetch_with(&repo, "不覆盖", ProxyOverride::Inherit.into()).await;
    assert!(e.contains("127.0.0.1 port 9"), "应该走环境变量里的代理：{e}");

    // 使用指定代理：-c http.proxy 优先于环境变量
    let e = fetch_with(&repo, "使用 127.0.0.1:8", ProxyOverride::Use("http://127.0.0.1:8".into()).into()).await;
    assert!(e.contains("127.0.0.1 port 8"), "应该走设置里的代理：{e}");

    // socks5 也能用
    let e = fetch_with(&repo, "使用 socks5://127.0.0.1:8", ProxyOverride::Use("socks5://127.0.0.1:8".into()).into()).await;
    assert!(e.contains("127.0.0.1 port 8"), "应该走 socks5 代理：{e}");

    // 禁用：连环境变量里的代理也不走，直接去解析那个（解析不了的）主机
    let e = fetch_with(&repo, "禁用", ProxyOverride::Disable.into()).await;
    assert!(no_proxy_used(&e), "不应该走任何代理：{e}");

    // ---------------- git 全局配置里按网址设置了代理（http.<网址>.proxy）----------------
    std::fs::write(&fake_global, format!("[http \"{HOST}\"]\n\tproxy = http://127.0.0.1:7\n")).unwrap();
    let cfg = ops::git_proxy_config().await;
    assert_eq!(cfg.proxy, None);
    assert_eq!(cfg.url_proxies, vec![(HOST.to_string(), "http://127.0.0.1:7".to_string())], "应读到按网址的代理");

    // 按网址的设置比 http.proxy 优先：只加 -c http.proxy 盖不住它
    let e = fetch_with(&repo, "只禁用 http.proxy", ProxyOverride::Disable.into()).await;
    assert!(e.contains("127.0.0.1 port 7"), "只盖 http.proxy 时仍会走按网址的代理：{e}");

    // 把这些网址也盖住
    let scoped = |over| ProxyPlan { over, url_scopes: cfg.url_scopes() };
    let e = fetch_with(&repo, "禁用（含按网址的设置）", scoped(ProxyOverride::Disable)).await;
    assert!(no_proxy_used(&e), "不应该走任何代理：{e}");
    let e = fetch_with(&repo, "使用 127.0.0.1:8（含按网址的设置）", scoped(ProxyOverride::Use("http://127.0.0.1:8".into()))).await;
    assert!(e.contains("127.0.0.1 port 8"), "应该走设置里的代理：{e}");

    set_proxy_plan(ProxyPlan::default());
    let _ = std::fs::remove_dir_all(&root);
}
