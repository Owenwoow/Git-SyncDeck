//! 从 GitHub 克隆的后端流程：用 Tauri 的模拟运行时调用 `clone_repos`，
//! 克隆 scripts/make-test-repos.mjs 生成的本地裸仓库（充当云端），不连真实 GitHub。
//! 覆盖克隆成功与事件、默认及自定义目录、设置不变、本地目标检查、目录校验、已有目标保护、并发同名与失败清理。
//! 用 GIT_CONFIG_GLOBAL / GIT_CONFIG_NOSYSTEM 把 git 的全局配置换成空的临时文件；这个文件里只有一个测试，因为它要改本进程的环境变量。
//! 运行：cd src-tauri && cargo test --test clone_flow -- --nocapture

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::Manager;

use git_syncdeck_lib::commands::{self, AppState, Shared};
use git_syncdeck_lib::config::ConfigStore;
use git_syncdeck_lib::model::{CloneRequest, CloneResult, GithubRepo, SettingsPatch, SyncStatus};

fn fixtures(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(name);
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/make-test-repos.mjs");
    assert!(Command::new("node").arg(script).arg(&root).status().unwrap().success());
    root
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir); // 测试自己的临时目录
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn clone(
    app: &tauri::App<tauri::test::MockRuntime>,
    items: &[(&str, &str)],
) -> (Vec<CloneResult>, Vec<serde_json::Value>) {
    clone_in(app, items, None).await
}

async fn clone_in(
    app: &tauri::App<tauri::test::MockRuntime>,
    items: &[(&str, &str)],
    parent: Option<&Path>,
) -> (Vec<CloneResult>, Vec<serde_json::Value>) {
    let events: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let sink = events.clone();
    let channel = Channel::new(move |body| {
        if let InvokeResponseBody::Json(s) = body {
            sink.lock().unwrap().push(serde_json::from_str(&s).unwrap());
        }
        Ok(())
    });
    let items = items.iter().map(|(url, name)| CloneRequest { url: url.to_string(), name: name.to_string() }).collect();
    let results = commands::clone_repos(app.state(), items, channel, parent.map(|p| p.to_string_lossy().into_owned())).await.unwrap();
    let events = events.lock().unwrap().clone();
    (results, events)
}

fn repo(name: &str, url: &str) -> GithubRepo {
    GithubRepo {
        name: name.into(), full_name: format!("test/{name}"), clone_url: url.into(),
        html_url: "https://github.com/test/repo".into(), private: false, fork: false, archived: false,
        pushed_at: None, description: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn clone_flow() {
    let cfg = fresh_dir("syncdeck-clone-gitconfig");
    let gitconfig = cfg.join("gitconfig");
    std::fs::write(&gitconfig, "").unwrap();
    std::env::set_var("GIT_CONFIG_GLOBAL", &gitconfig);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");

    let root = fixtures("syncdeck-clone-test");
    let remote = root.join("remotes").join("synced.git").to_string_lossy().into_owned();
    let missing = root.join("remotes").join("does-not-exist.git").to_string_lossy().into_owned();
    let code_dir = fresh_dir("syncdeck-clone-code");
    let cfg_dir = fresh_dir("syncdeck-clone-config");

    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    app.manage::<Shared>(Arc::new(AppState::new(ConfigStore::load(cfg_dir.join("config.json")))));

    // 默认代码目录不存在 → 整个命令报错，不克隆
    let not_there = code_dir.join("nope");
    commands::save_settings(
        app.state(),
        SettingsPatch { default_code_dir: Some(not_there.to_string_lossy().into()), ..Default::default() },
    )
    .await
    .unwrap();
    let channel = Channel::new(|_| Ok(()));
    let err = commands::clone_repos(
        app.state(),
        vec![CloneRequest { url: remote.clone(), name: "synced".into() }],
        channel,
        None,
    )
    .await
    .unwrap_err();
    assert!(err.contains("默认代码目录不存在"), "{err}");
    assert!(!not_there.exists());

    commands::save_settings(
        app.state(),
        SettingsPatch { default_code_dir: Some(code_dir.to_string_lossy().into()), ..Default::default() },
    )
    .await
    .unwrap();
    assert!(commands::list_projects(app.state()).await.unwrap().is_empty());

    // 1. 克隆成功：收到 Start / Done 事件，加入监控清单，状态已同步
    let (results, events) = clone(&app, &[(&remote, "synced")]).await;
    println!("  克隆结果：{results:?}");
    assert_eq!(results.len(), 1);
    assert!(results[0].ok, "{:?}", results[0].error);
    let target = code_dir.join("synced");
    assert!(target.join(".git").exists());
    let types: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
    assert_eq!(types.first(), Some(&"start"), "{types:?}");
    assert_eq!(types.last(), Some(&"done"), "{types:?}");
    assert!(types.iter().all(|t| ["start", "progress", "done"].contains(t)), "{types:?}");
    let done = events.last().unwrap();
    assert_eq!(done["url"], remote.as_str());
    assert_eq!(done["ok"], true);
    assert_eq!(done["project"]["status"], "synced");
    let project = results[0].project.as_ref().expect("成功时带着项目");
    assert_eq!(project.status, SyncStatus::Synced);
    assert_eq!(project.name, "synced");
    let listed = commands::list_projects(app.state()).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, project.id);
    assert_eq!(listed[0].status, SyncStatus::Synced);

    // 2. 再克隆同名：失败，原因含"文件夹已存在"，已有文件夹原样不动；清单里仍然只有一个
    std::fs::write(target.join("keep.txt"), "不能被覆盖").unwrap();
    let (results, events) = clone(&app, &[(&remote, "synced")]).await;
    assert!(!results[0].ok);
    let reason = results[0].error.as_deref().unwrap();
    assert!(reason.contains("文件夹已存在") && reason.contains("没有覆盖"), "{reason}");
    assert_eq!(std::fs::read_to_string(target.join("keep.txt")).unwrap(), "不能被覆盖");
    assert!(target.join(".git").exists());
    assert_eq!(events.last().unwrap()["ok"], false);
    assert_eq!(commands::list_projects(app.state()).await.unwrap().len(), 1);

    // 3. 克隆不存在的地方：失败，目标文件夹不存在（不留半截的）；同一批里成功的不受影响
    let (results, _) = clone(&app, &[(&missing, "broken"), (&remote, "synced-2")]).await;
    assert_eq!(results.len(), 2);
    assert!(!results[0].ok && results[0].error.is_some());
    assert!(!code_dir.join("broken").exists(), "失败后不能留下半截文件夹");
    assert!(results[1].ok, "{:?}", results[1].error);
    assert!(code_dir.join("synced-2").join(".git").exists());
    assert_eq!(commands::list_projects(app.state()).await.unwrap().len(), 2);

    // 4. 不合法的文件夹名、不合法的地址：被拒绝，不创建任何东西
    let (results, _) = clone(&app, &[(&remote, "..\\escape"), ("http://github.com/a/b.git", "plain-http"), ("ext::sh -c calc", "ext")]).await;
    assert!(results.iter().all(|r| !r.ok), "{results:?}");
    assert!(!code_dir.parent().unwrap().join("escape").exists());
    assert!(!code_dir.join("plain-http").exists() && !code_dir.join("ext").exists());
    assert!(results[1].error.as_deref().unwrap().contains("已拒绝"), "{:?}", results[1].error);
    assert_eq!(commands::list_projects(app.state()).await.unwrap().len(), 2);

    // 5. 自定义目录含空格和中文，预览按该目录判断占用；不改变设置。
    let custom_root = fresh_dir("syncdeck-clone-custom");
    let custom = custom_root.join("本次 代码目录");
    std::fs::create_dir(&custom).unwrap();
    let custom_text = custom.to_string_lossy().into_owned();
    let preview = vec![repo("occupied", &missing), repo("custom-repo", &remote)];
    let before = commands::inspect_github_targets(app.state(), preview.clone(), custom_text.clone()).await.unwrap();
    assert!(!before[0].target_exists);
    assert_eq!(PathBuf::from(&before[0].target), custom.join("occupied"));
    assert!(before[1].local.is_some(), "监控中的远程地址相同仓库仍应识别");
    std::fs::create_dir(custom.join("occupied")).unwrap();
    std::fs::write(custom.join("occupied").join("keep.txt"), "保留").unwrap();
    let after = commands::inspect_github_targets(app.state(), preview, custom_text.clone()).await.unwrap();
    assert!(after[0].target_exists);
    let (results, _) = clone_in(&app, &[(&remote, "custom-repo"), (&missing, "broken-custom"), (&remote, "occupied")], Some(&custom)).await;
    assert!(results[0].ok, "{:?}", results[0].error);
    assert_eq!(PathBuf::from(&results[0].project.as_ref().unwrap().path), custom.join("custom-repo"));
    assert!(custom.join("custom-repo").join(".git").exists());
    assert!(!code_dir.join("custom-repo").exists());
    assert!(!results[1].ok && !custom.join("broken-custom").exists());
    assert!(!results[2].ok);
    assert_eq!(std::fs::read_to_string(custom.join("occupied").join("keep.txt")).unwrap(), "保留");
    assert_eq!(commands::get_settings(app.state()).await.unwrap().default_code_dir, code_dir.to_string_lossy());

    // 6. 不传本批目录仍回退到默认目录；同批同名目标只有一个成功，不清理成功者。
    let (results, _) = clone(&app, &[(&remote, "default-again")]).await;
    assert!(results[0].ok && code_dir.join("default-again").join(".git").exists());
    let (results, _) = clone_in(&app, &[(&remote, "same-name"), (&remote, "same-name")], Some(&custom)).await;
    assert_eq!(results.iter().filter(|r| r.ok).count(), 1, "{results:?}");
    assert!(custom.join("same-name").join(".git").exists());

    // 7. 默认目录与本次目录里未监控的仓库也能识别，切换目录只读、不改监控清单。
    let custom_remote = root.join("remotes").join("ahead.git").to_string_lossy().into_owned();
    let (unique, _) = clone_in(&app, &[(&custom_remote, "custom-only")], Some(&custom)).await;
    assert!(unique[0].ok);
    let empty = fresh_dir("syncdeck-clone-empty");
    let monitored = commands::inspect_github_targets(app.state(), vec![repo("other-name", &custom_remote)], empty.to_string_lossy().into()).await.unwrap();
    assert_eq!(PathBuf::from(monitored[0].local.as_ref().unwrap()), custom.join("custom-only"));
    commands::remove_project(app.state(), unique[0].project.as_ref().unwrap().id.clone()).await.unwrap();
    let custom_entries = commands::inspect_github_targets(app.state(), vec![repo("other-name", &custom_remote)], custom_text.clone()).await.unwrap();
    assert_eq!(PathBuf::from(custom_entries[0].local.as_ref().unwrap()), custom.join("custom-only"));
    let monitored_ids: Vec<_> = commands::list_projects(app.state()).await.unwrap().into_iter().map(|p| p.id).collect();
    // 切换到没有本地仓库的目录，也要识别默认目录中未监控的远程仓库。
    for p in commands::list_projects(app.state()).await.unwrap() {
        commands::remove_project(app.state(), p.id).await.unwrap();
    }
    let default_entries = commands::inspect_github_targets(app.state(), vec![repo("other-name", &remote)], empty.to_string_lossy().into()).await.unwrap();
    assert!(default_entries[0].local.as_ref().unwrap().starts_with(code_dir.to_str().unwrap()));
    assert!(commands::list_projects(app.state()).await.unwrap().is_empty());
    assert!(!monitored_ids.is_empty());

    // 8. 预览与实际执行都拒绝非法父目录；预览仓库名同样防止目录穿越。
    let ordinary_file = custom_root.join("file.txt");
    std::fs::write(&ordinary_file, "file").unwrap();
    for dir in ["".to_string(), "relative".into(), "C:relative".into(), custom_root.join("missing").to_string_lossy().into(), ordinary_file.to_string_lossy().into()] {
        assert!(commands::inspect_github_targets(app.state(), vec![repo("safe", &remote)], dir.clone()).await.is_err(), "{dir}");
        let channel = Channel::new(|_| Ok(()));
        assert!(commands::clone_repos(app.state(), vec![CloneRequest { url: remote.clone(), name: "safe".into() }], channel, Some(dir.clone())).await.is_err(), "{dir}");
    }
    assert!(commands::inspect_github_targets(app.state(), vec![repo("..\\escape", &remote)], custom_text).await.is_err());
    assert!(!custom_root.join("escape").exists());
}
