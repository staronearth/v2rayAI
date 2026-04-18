/// Subconverter Manager
/// Downloads, installs, and manages the subconverter process for subscription format conversion.
/// subconverter repo: https://github.com/tindy2013/subconverter
///
/// Design: mirrors the pattern from core_manager.rs (find → download → start → stop).
/// Install emits realtime "sc-progress" events via Tauri AppHandle.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::process::{Child, Command};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::process::Stdio;
use tauri::{AppHandle, Emitter};

const SUBCONV_PORT: u16 = 25500;
const GITHUB_RELEASES_URL: &str =
    "https://api.github.com/repos/tindy2013/subconverter/releases/latest";

// ────────────────────────────────────────────────────────────────────────────
// Public types
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubConverterStatus {
    pub installed: bool,
    pub running: bool,
    pub version: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubConverterRelease {
    pub version: String,
    pub download_url: String,
}

/// Progress event payload emitted during install
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    /// "info" | "download" | "done" | "error"
    pub stage: String,
    pub message: String,
    /// 0..100, only meaningful when stage == "download"
    pub percent: u8,
}

// ────────────────────────────────────────────────────────────────────────────
// Manager
// ────────────────────────────────────────────────────────────────────────────

pub struct SubConverterManager {
    child: Arc<Mutex<Option<Child>>>,
    install_path: Arc<Mutex<Option<String>>>,
}

/// Helper: emit a progress event (fire-and-forget)
fn emit_progress(app: &AppHandle, stage: &str, msg: &str, percent: u8) {
    let _ = app.emit("sc-progress", ProgressEvent {
        stage: stage.to_string(),
        message: msg.to_string(),
        percent,
    });
}

/// Format bytes to human-readable
fn fmt_bytes(b: u64) -> String {
    if b < 1024 { return format!("{} B", b); }
    let kb = b as f64 / 1024.0;
    if kb < 1024.0 { return format!("{:.1} KB", kb); }
    let mb = kb / 1024.0;
    format!("{:.2} MB", mb)
}

impl SubConverterManager {
    pub fn new() -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            install_path: Arc::new(Mutex::new(None)),
        }
    }

    /// Return the canonical install directory: ~/.v2rayai/subconverter/
    fn install_dir() -> PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".v2rayai").join("subconverter")
    }

    fn binary_name() -> &'static str {
        if cfg!(target_os = "windows") {
            "subconverter.exe"
        } else {
            "subconverter"
        }
    }

    /// Try to locate an existing subconverter installation
    pub async fn detect(&self) -> Option<String> {
        let dir = Self::install_dir();
        let bin = dir.join(Self::binary_name());
        if bin.exists() {
            let p = bin.to_string_lossy().to_string();
            *self.install_path.lock().await = Some(p.clone());
            return Some(p);
        }

        // Also check the nested subconverter/subconverter path
        let nested = dir.join("subconverter").join(Self::binary_name());
        if nested.exists() {
            let p = nested.to_string_lossy().to_string();
            *self.install_path.lock().await = Some(p.clone());
            return Some(p);
        }

        // Check PATH
        let cmd_name = if cfg!(target_os = "windows") { "where" } else { "which" };
        if let Ok(output) = Command::new(cmd_name).arg("subconverter").output().await {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(first) = stdout.lines().next() {
                    let trimmed = first.trim().to_string();
                    if !trimmed.is_empty() {
                        *self.install_path.lock().await = Some(trimmed.clone());
                        return Some(trimmed);
                    }
                }
            }
        }

        None
    }

    /// Get current status
    pub async fn status(&self) -> SubConverterStatus {
        let path = self.detect().await;
        let running = self.child.lock().await.is_some();
        SubConverterStatus {
            installed: path.is_some(),
            running,
            version: None,
            path,
        }
    }

    /// Fetch the latest release info from GitHub
    pub async fn fetch_latest_release() -> Result<SubConverterRelease, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("v2rayAI/0.1.0")
            .build()
            .map_err(|e| e.to_string())?;

        #[derive(Deserialize)]
        struct GHRelease {
            tag_name: String,
            assets: Vec<GHAsset>,
        }
        #[derive(Deserialize)]
        struct GHAsset {
            name: String,
            browser_download_url: String,
        }

        let release: GHRelease = client
            .get(GITHUB_RELEASES_URL)
            .send()
            .await
            .map_err(|e| format!("获取版本失败：{}", e))?
            .json()
            .await
            .map_err(|e| format!("解析响应失败：{}", e))?;

        let target = get_platform_asset_name();
        let asset = release
            .assets
            .iter()
            .find(|a| a.name == target)
            .ok_or_else(|| format!("未找到当前平台的安装包：{}", target))?;

        Ok(SubConverterRelease {
            version: release.tag_name,
            download_url: asset.browser_download_url.clone(),
        })
    }

    /// Download and install subconverter with real-time progress events
    pub async fn install(&self, app: &AppHandle) -> Result<String, String> {
        // ── Step 1: Fetch release info ──
        emit_progress(app, "info", "正在获取最新版本信息...", 0);
        let release = Self::fetch_latest_release().await?;
        emit_progress(app, "info", &format!("找到版本 {}，准备下载...", release.version), 0);

        let install_dir = Self::install_dir();
        tokio::fs::create_dir_all(&install_dir)
            .await
            .map_err(|e| format!("创建目录失败：{}", e))?;

        // ── Step 2: Download with streaming progress ──
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .user_agent("v2rayAI/0.1.0")
            .build()
            .map_err(|e| e.to_string())?;

        let resp = client
            .get(&release.download_url)
            .send()
            .await
            .map_err(|e| format!("下载失败：{}", e))?;

        let total_size = resp.content_length().unwrap_or(0);
        let total_str = if total_size > 0 { fmt_bytes(total_size) } else { "未知大小".into() };
        emit_progress(app, "download", &format!("开始下载 ({})...", total_str), 0);

        // Stream download
        let mut downloaded: u64 = 0;
        let mut raw: Vec<u8> = Vec::with_capacity(total_size as usize);
        let mut stream = resp.bytes_stream();
        use futures_util::StreamExt;
        let mut last_percent: u8 = 0;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("下载中断：{}", e))?;
            downloaded += chunk.len() as u64;
            raw.extend_from_slice(&chunk);

            let percent = if total_size > 0 {
                ((downloaded * 100) / total_size).min(100) as u8
            } else {
                0
            };

            // Only emit if percent actually changed (avoid flooding)
            if percent != last_percent {
                last_percent = percent;
                emit_progress(
                    app,
                    "download",
                    &format!("下载中 {} / {} ({}%)", fmt_bytes(downloaded), total_str, percent),
                    percent,
                );
            }
        }

        emit_progress(app, "info", &format!("下载完成 ({})，正在解压...", fmt_bytes(downloaded)), 100);

        // ── Step 3: Extract ──
        let dir_str = install_dir.to_string_lossy().to_string();
        let binary_name = Self::binary_name().to_string();

        let out_path = tokio::task::spawn_blocking(move || -> Result<String, String> {
            use std::io::Cursor;

            let cursor = Cursor::new(&raw);
            let gz = flate2::read::GzDecoder::new(cursor);
            let mut archive = tar::Archive::new(gz);

            archive
                .unpack(&dir_str)
                .map_err(|e| format!("解压失败：{}", e))?;

            // subconverter tar.gz extracts into a subconverter/ subdirectory
            let extracted_bin = Path::new(&dir_str)
                .join("subconverter")
                .join(&binary_name);

            if extracted_bin.exists() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        &extracted_bin,
                        std::fs::Permissions::from_mode(0o755),
                    )
                    .map_err(|e| format!("设置权限失败：{}", e))?;
                }
                return Ok(extracted_bin.to_string_lossy().to_string());
            }

            // Fallback: binary directly in directory
            let direct_bin = Path::new(&dir_str).join(&binary_name);
            if direct_bin.exists() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        &direct_bin,
                        std::fs::Permissions::from_mode(0o755),
                    )
                    .ok();
                }
                return Ok(direct_bin.to_string_lossy().to_string());
            }

            Err(format!("解压完成但未找到 {} 二进制文件", binary_name))
        })
        .await
        .map_err(|e| format!("解压线程错误：{}", e))??;

        emit_progress(app, "done", &format!("✅ 安装完成：{}", out_path), 100);

        *self.install_path.lock().await = Some(out_path.clone());
        Ok(out_path)
    }

    /// Start subconverter as a background process
    pub async fn start(&self) -> Result<String, String> {
        // Stop existing instance first
        self.stop().await.ok();

        let bin_path = match self.detect().await {
            Some(p) => p,
            None => return Err("subconverter 未安装，请先安装".to_string()),
        };

        // subconverter needs to be started from its own directory (for config files)
        let work_dir = Path::new(&bin_path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());

        let mut cmd = Command::new(&bin_path);
        cmd.current_dir(&work_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let child = cmd
            .spawn()
            .map_err(|e| format!("启动 subconverter 失败：{}", e))?;

        *self.child.lock().await = Some(child);

        // Wait a moment for the service to start
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;

        // Verify it's actually listening
        let check = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .ok();

        if let Some(client) = check {
            match client
                .get(format!("http://127.0.0.1:{}/version", SUBCONV_PORT))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    return Ok("subconverter 已启动".to_string());
                }
                _ => {
                    // Not responding yet, wait more
                    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
                }
            }
        }

        Ok("subconverter 已启动（服务启动可能需要几秒）".to_string())
    }

    /// Stop the running subconverter process
    pub async fn stop(&self) -> Result<String, String> {
        let mut child = self.child.lock().await;
        if let Some(mut c) = child.take() {
            c.kill()
                .await
                .map_err(|e| format!("停止 subconverter 失败：{}", e))?;
            Ok("subconverter 已停止".to_string())
        } else {
            Ok("subconverter 未在运行".to_string())
        }
    }

    /// Convert a subscription URL via the locally running subconverter service.
    /// Returns the raw Base64 text that can be fed into `parse_subscription()`.
    pub async fn convert_subscription(&self, url: &str) -> Result<String, String> {
        // Ensure subconverter is running
        if self.child.lock().await.is_none() {
            return Err(
                "subconverter 未运行，请先在「工具」页面安装并启动"
                    .to_string(),
            );
        }

        let encoded_url = urlencoding::encode(url);
        let api_url = format!(
            "http://127.0.0.1:{}/sub?target=v2ray&url={}&emoji=true&list=false",
            SUBCONV_PORT, encoded_url
        );

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?;

        let resp = client
            .get(&api_url)
            .send()
            .await
            .map_err(|e| format!("subconverter 请求失败：{}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "subconverter 返回错误 HTTP {}：{}",
                status,
                body.chars().take(200).collect::<String>()
            ));
        }

        let text = resp
            .text()
            .await
            .map_err(|e| format!("读取转换结果失败：{}", e))?;

        if text.trim().is_empty() {
            return Err("subconverter 返回空结果，订阅可能无效".to_string());
        }

        Ok(text)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Helpers
// ────────────────────────────────────────────────────────────────────────────

fn get_platform_asset_name() -> String {
    if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "subconverter_darwinarm.tar.gz".to_string()
    } else if cfg!(target_os = "macos") {
        "subconverter_darwin64.tar.gz".to_string()
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        "subconverter_aarch64.tar.gz".to_string()
    } else if cfg!(target_os = "linux") {
        "subconverter_linux64.tar.gz".to_string()
    } else if cfg!(target_os = "windows") {
        "subconverter_win64.7z".to_string()
    } else {
        "subconverter_linux64.tar.gz".to_string()
    }
}
