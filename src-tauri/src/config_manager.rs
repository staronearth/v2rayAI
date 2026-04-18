use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

/// Manage xray/v2ray config files on disk
pub struct ConfigManager {
    config_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedConfig {
    pub id: String,
    pub name: String,
    pub node_id: Option<String>,
    pub content: serde_json::Value,
    pub created_at: i64,
}

impl ConfigManager {
    pub fn new() -> Self {
        let config_dir = dirs_for_app().join("configs");
        Self { config_dir }
    }

    /// Write a config to disk and return the file path
    pub async fn write_active_config(&self, content: &serde_json::Value) -> Result<String, String> {
        fs::create_dir_all(&self.config_dir)
            .await
            .map_err(|e| format!("创建配置目录失败：{}", e))?;

        // Backup previous config
        let active_path = self.config_dir.join("active.json");
        if active_path.exists() {
            let backup = self.config_dir.join("active.json.bak");
            fs::copy(&active_path, &backup).await.ok();
        }

        let json = serde_json::to_string_pretty(content)
            .map_err(|e| format!("JSON 序列化失败：{}", e))?;

        fs::write(&active_path, json.as_bytes())
            .await
            .map_err(|e| format!("写入配置文件失败：{}", e))?;

        Ok(active_path.to_string_lossy().to_string())
    }

    /// Get the path of the active config file
    pub fn active_config_path(&self) -> String {
        self.config_dir.join("active.json").to_string_lossy().to_string()
    }

    /// Read active config
    pub async fn read_active_config(&self) -> Result<serde_json::Value, String> {
        let path = self.config_dir.join("active.json");
        let bytes = fs::read(&path)
            .await
            .map_err(|e| format!("读取配置失败：{}", e))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("解析配置失败：{}", e))
    }

    /// Save a named config
    pub async fn save_named(&self, config: &SavedConfig) -> Result<(), String> {
        fs::create_dir_all(&self.config_dir)
            .await
            .map_err(|e| format!("创建目录失败：{}", e))?;
        let path = self.config_dir.join(format!("{}.json", config.id));
        let json = serde_json::to_string_pretty(config)
            .map_err(|e| format!("序列化失败：{}", e))?;
        fs::write(&path, json.as_bytes())
            .await
            .map_err(|e| format!("写入失败：{}", e))?;
        Ok(())
    }

    /// List all saved named configs
    pub async fn list_configs(&self) -> Result<Vec<SavedConfig>, String> {
        fs::create_dir_all(&self.config_dir).await.ok();
        let mut configs = Vec::new();
        let mut dir = fs::read_dir(&self.config_dir)
            .await
            .map_err(|e| format!("读取目录失败：{}", e))?;

        while let Ok(Some(entry)) = dir.next_entry().await {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.ends_with(".json") && name_str != "active.json" && !name_str.ends_with(".bak") {
                if let Ok(bytes) = fs::read(entry.path()).await {
                    if let Ok(cfg) = serde_json::from_slice::<SavedConfig>(&bytes) {
                        configs.push(cfg);
                    }
                }
            }
        }
        configs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(configs)
    }

    /// Delete a named config
    pub async fn delete_config(&self, id: &str) -> Result<(), String> {
        let path = self.config_dir.join(format!("{}.json", id));
        fs::remove_file(&path).await.map_err(|e| format!("删除失败：{}", e))
    }
}

pub fn dirs_for_app() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".v2rayai")
}
