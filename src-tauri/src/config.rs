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
}

impl ConfigStore {
    pub fn load(path: PathBuf) -> Self {
        let data = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|_| {
                // 文件损坏：改名保留下来，不删除，然后从空配置开始
                let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
                let _ = std::fs::rename(&path, path.with_file_name(format!("config.corrupt-{stamp}.json")));
                AppConfig::default()
            }),
            Err(_) => AppConfig::default(),
        };
        ConfigStore { path, data: Mutex::new(data) }
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
        let result = f(&mut data);
        data.version = 1;
        let json = serde_json::to_string_pretty(&*data).map_err(|e| e.to_string())?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("无法创建配置目录：{e}"))?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| format!("无法写入配置文件：{e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("无法保存配置文件：{e}"))?;
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
