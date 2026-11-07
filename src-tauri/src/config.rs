//! 持久化：监控的项目列表、设置、每个项目的上次同步时间和遗留异常，保存在应用配置目录下的 config.json。
//! Windows 上的位置：%APPDATA%\com.gitsyncdeck.desktop\config.json

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppConfig {
    pub version: u32,
    pub settings: StoredSettings,
    pub projects: Vec<StoredProject>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StoredSettings {
    /// None 仅用于旧配置迁移；首次创建时显式关闭。
    pub auto_sync_enabled: Option<bool>,
    pub default_code_dir: Option<String>,
    pub theme: Option<String>,
    /// 自动提交的提交信息模板；None 表示用默认模板
    pub commit_template: Option<String>,
    /// 联网超时（秒）；None 表示用默认值
    pub network_timeout_secs: Option<u64>,
    /// 代理模式："off" / "manual"；None（或不认识的值）表示 "auto"
    pub proxy_mode: Option<String>,
    /// 手动填写的代理地址
    pub proxy_url: Option<String>,
    /// 本机名称（提交信息里的 {host}）；None 表示用电脑名
    pub host_name: Option<String>,
    // 以下开关：None 表示用默认值（默认值见 autosync::Switches::DEFAULT）
    pub launch_at_login: Option<bool>,
    pub close_to_tray: Option<bool>,
    pub sync_on_lock: Option<bool>,
    pub sync_on_startup: Option<bool>,
    pub sync_on_resume: Option<bool>,
    pub sync_on_unlock: Option<bool>,
    pub periodic_check: Option<bool>,
    pub notify_on_success: Option<bool>,
    pub notify_on_error: Option<bool>,
    /// 自动同步暂停到的时间（RFC 3339，带时区）；None 表示没暂停
    pub auto_sync_paused_until: Option<String>,
    /// 分叉时无冲突就自动合并；None 表示默认（开）
    pub auto_merge: Option<bool>,
    /// 自动提交前检查大文件和疑似密钥文件；None 表示默认（开）
    pub precommit_check: Option<bool>,
    /// 提交前检查的大文件上限（MB）；None 表示默认值
    pub big_file_limit_mb: Option<u64>,
    /// 「添加项目 → 从 GitHub 克隆」上次输入的账号主页 / 用户名 / 仓库地址
    pub github_source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredProject {
    pub id: String,
    pub path: String,
    #[serde(default)]
    pub last_sync_at: Option<String>,
    /// 一键同步时的异常原因，直到下次同步成功（或刷新检测到已同步）才清除
    #[serde(default)]
    pub issue: Option<String>,
    /// 一键同步时自动提交并推送（默认关；旧配置里没有这个字段时读出来为 false）
    #[serde(default)]
    pub auto_commit: bool,
}

pub struct ConfigStore {
    path: PathBuf,
    data: Mutex<AppConfig>,
    load_error: Mutex<Option<String>>,
}

impl ConfigStore {
    pub fn load(path: PathBuf) -> Self {
        let fresh = || {
            let mut c = AppConfig::default();
            c.settings.auto_sync_enabled = Some(false);
            c
        };
        let (data, load_error) = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<AppConfig>(&text) {
                Ok(mut c) => {
                    // 文件确实存在才视作升级：保留旧版本有效的自动触发行为。
                    if c.settings.auto_sync_enabled.is_none() {
                        c.settings.auto_sync_enabled = Some(true);
                    }
                    (c, None)
                }
                Err(e) => (fresh(), Some(format!("配置文件无法解析，已保留原文件并停止保存：{e}。请先备份并修复 {} 后重新读取设置或重启应用", path.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (fresh(), None),
            Err(e) => (fresh(), Some(format!("读取配置失败，已停止保存以保护原数据：{e}。请检查 {} 的权限后重新读取设置或重启应用", path.display()))),
        };
        ConfigStore { path, data: Mutex::new(data), load_error: Mutex::new(load_error) }
    }

    pub fn load_error(&self) -> Option<String> {
        self.load_error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 显式重新读取设置时允许从已修复的文件恢复；失败期间始终禁止覆盖。
    pub fn reload_if_failed(&self) -> bool {
        if self.load_error().is_none() { return false; }
        let loaded = Self::load(self.path.clone());
        let error = loaded.load_error();
        if error.is_none() {
            let restored = loaded.read(Clone::clone);
            let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
            *data = restored;
            *self.load_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
            true
        } else {
            *self.load_error.lock().unwrap_or_else(|e| e.into_inner()) = error;
            false
        }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn read<R>(&self, f: impl FnOnce(&AppConfig) -> R) -> R {
        f(&self.data.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// 修改并立即写盘（先写临时文件再改名，中途崩溃不会写坏原文件）
    pub fn update<R>(&self, f: impl FnOnce(&mut AppConfig) -> R) -> Result<R, String> {
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(error) = self.load_error() { return Err(error); }
        let mut candidate = data.clone();
        let result = f(&mut candidate);
        candidate.version = 2;
        let json = serde_json::to_string_pretty(&candidate).map_err(|e| e.to_string())?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("无法创建配置目录：{e}"))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        use std::io::Write;
        let mut file = std::fs::File::create(&tmp).map_err(|e| format!("无法写入配置文件：{e}"))?;
        file.write_all(json.as_bytes()).and_then(|_| file.sync_all()).map_err(|e| format!("无法写入配置文件：{e}"))?;
        drop(file);
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("无法保存配置文件：{e}"))?;
        *data = candidate;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_config_without_new_fields_still_loads() {
        // 加入"自动提交"之前的配置文件：没有 autoCommit、commitTemplate
        let old = r#"{
          "version": 1,
          "settings": { "defaultCodeDir": "D:\\code", "theme": "dark" },
          "projects": [
            { "id": "p-1", "path": "D:\\code\\blog", "lastSyncAt": null, "issue": "有改动" }
          ]
        }"#;
        let c: AppConfig = serde_json::from_str(old).unwrap();
        assert_eq!(c.settings.theme.as_deref(), Some("dark"));
        assert!(c.settings.commit_template.is_none());
        assert!(c.settings.network_timeout_secs.is_none());
        assert!(c.settings.proxy_mode.is_none() && c.settings.proxy_url.is_none());
        // 自动同步相关的新字段：都是 None，读取时回落为默认值
        assert!(c.settings.host_name.is_none() && c.settings.auto_sync_paused_until.is_none());
        assert!(c.settings.launch_at_login.is_none() && c.settings.periodic_check.is_none());
        assert_eq!(crate::autosync::Switches::from_stored(&c.settings), crate::autosync::Switches::DEFAULT);
        // 没有"冲突与提交检查"的设置时都是默认值
        assert!(c.settings.auto_merge.is_none());
        assert!(c.settings.precommit_check.is_none());
        assert!(c.settings.big_file_limit_mb.is_none());
        assert_eq!(c.projects.len(), 1);
        assert!(!c.projects[0].auto_commit);
        assert_eq!(c.projects[0].issue.as_deref(), Some("有改动"));

        // 写回去再读，新字段能往返
        let mut c = c;
        c.projects[0].auto_commit = true;
        c.settings.commit_template = Some("同步 {date}".into());
        c.settings.host_name = Some("家里台式机".into());
        c.settings.sync_on_lock = Some(false);
        c.settings.periodic_check = Some(true);
        c.settings.auto_merge = Some(false);
        c.settings.big_file_limit_mb = Some(10);
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"syncOnLock\":false") && json.contains("\"hostName\""), "{json}");
        assert!(json.contains("\"autoMerge\":false") && json.contains("\"bigFileLimitMb\":10"), "{json}");
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert!(back.projects[0].auto_commit);
        assert_eq!(back.settings.commit_template.as_deref(), Some("同步 {date}"));
        assert_eq!(back.settings.host_name.as_deref(), Some("家里台式机"));
        let sw = crate::autosync::Switches::from_stored(&back.settings);
        assert!(!sw.sync_on_lock && sw.periodic_check && sw.sync_on_unlock);
        assert_eq!(back.settings.auto_merge, Some(false));
        assert_eq!(back.settings.big_file_limit_mb, Some(10));
    }
}

#[cfg(test)]
mod reliability_tests {
    use super::*;

    fn temporary(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("syncdeck-config-{name}-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn fresh_default_and_legacy_migration_preserve_choices() {
        let dir = temporary("migration");
        let path = dir.join("config.json");
        let fresh = ConfigStore::load(path.clone());
        assert_eq!(fresh.read(|c| c.settings.auto_sync_enabled), Some(false));
        assert!(!path.exists(), "读取本身不能保存默认配置");
        std::fs::write(&path, r#"{"version":1,"settings":{"theme":"dark","proxyMode":"off","launchAtLogin":false,"syncOnUnlock":false,"autoSyncPausedUntil":"2030-01-01T00:00:00+08:00"},"projects":[{"id":"p","path":"D:/code/test","autoCommit":true}]}"#).unwrap();
        let old = ConfigStore::load(path.clone());
        old.read(|c| {
            assert_eq!(c.settings.auto_sync_enabled, Some(true));
            assert_eq!(c.settings.theme.as_deref(), Some("dark"));
            assert_eq!(c.settings.proxy_mode.as_deref(), Some("off"));
            assert_eq!(c.settings.launch_at_login, Some(false));
            assert_eq!(c.settings.sync_on_unlock, Some(false));
            assert!(c.settings.auto_sync_paused_until.is_some());
            assert!(c.projects[0].auto_commit);
        });
        old.update(|c| c.settings.auto_sync_enabled = Some(false)).unwrap();
        let reopened = ConfigStore::load(path);
        assert_eq!(reopened.read(|c| c.settings.auto_sync_enabled), Some(false));
        assert_eq!(reopened.read(|c| c.projects.len()), 1);
    }

    #[test]
    fn failed_write_and_replace_never_publish_candidate() {
        let dir = temporary("write");
        let path = dir.join("config.json");
        let store = ConfigStore::load(path.clone());
        store.update(|c| c.settings.theme = Some("light".into())).unwrap();
        let original = std::fs::read(&path).unwrap();
        let tmp = path.with_extension("json.tmp");
        std::fs::create_dir(&tmp).unwrap();
        assert!(store.update(|c| c.settings.theme = Some("dark".into())).is_err());
        assert_eq!(store.read(|c| c.settings.theme.clone()).as_deref(), Some("light"));
        assert_eq!(std::fs::read(&path).unwrap(), original);

        let target = dir.join("replace-target.json");
        let replacement = ConfigStore::load(target.clone());
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("preserve"), "existing directory").unwrap();
        assert!(replacement.update(|c| c.settings.theme = Some("dark".into())).is_err());
        assert!(replacement.read(|c| c.settings.theme.is_none()));
        assert_eq!(std::fs::read_to_string(target.join("preserve")).unwrap(), "existing directory");
    }

    #[test]
    fn unreadable_or_corrupt_config_is_preserved_and_can_be_reread() {
        let dir = temporary("read");
        let path = dir.join("config.json");
        std::fs::write(&path, "invalid configuration").unwrap();
        let store = ConfigStore::load(path.clone());
        assert!(store.load_error().is_some());
        assert!(store.update(|c| c.projects.clear()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "invalid configuration");
        assert!(!store.reload_if_failed());
        std::fs::write(&path, r#"{"settings":{"theme":"dark"},"projects":[]}"#).unwrap();
        assert!(store.reload_if_failed());
        assert!(store.load_error().is_none());
        assert_eq!(store.read(|c| c.settings.theme.clone()).as_deref(), Some("dark"));
        assert!(store.update(|c| c.settings.theme = Some("light".into())).is_ok());

        let directory = dir.join("directory.json");
        std::fs::create_dir(&directory).unwrap();
        let unreadable = ConfigStore::load(directory);
        assert!(unreadable.load_error().is_some());
        assert!(unreadable.update(|_| ()).is_err());
    }
}
