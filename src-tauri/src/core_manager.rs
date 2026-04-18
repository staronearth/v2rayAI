use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tauri::AppHandle;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::process::Stdio;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreStatus {
    pub running: bool,
    pub core_type: String,
    pub pid: Option<u32>,
    pub uptime_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XrayRelease {
    pub version: String,
    pub download_url: String,
    pub published_at: String,
}

pub struct CoreManager {
    child: Arc<Mutex<Option<Child>>>,
    core_type: Arc<Mutex<String>>,
    start_time: Arc<Mutex<Option<std::time::Instant>>>,
    logs: Arc<Mutex<Vec<String>>>,
}

impl CoreManager {
    pub fn new() -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            core_type: Arc::new(Mutex::new("xray".to_string())),
            start_time: Arc::new(Mutex::new(None)),
            logs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Start the v2ray/xray core with the given config
    pub async fn start(&self, core_path: &str, config_path: &str, app: Option<AppHandle>) -> Result<String, String> {
        if !Path::new(core_path).exists() {
            return Err(format!("内核文件不存在：{}", core_path));
        }

        self.stop().await?;

        let mut cmd = Command::new(core_path);
        cmd.arg("run")
            .arg("-c")
            .arg(config_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| format!("启动内核失败：{}", e))?;

        // Capture stdout logs
        if let Some(stdout) = child.stdout.take() {
            let logs = self.logs.clone();
            let app_clone = app.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    crate::traffic_monitor::parse_and_emit_traffic(app_clone.as_ref(), &line);
                    let mut logs = logs.lock().await;
                    logs.push(line);
                    if logs.len() > 500 { logs.drain(0..100); }
                }
            });
        }

        // Capture stderr logs
        if let Some(stderr) = child.stderr.take() {
            let logs = self.logs.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut logs = logs.lock().await;
                    logs.push(format!("[ERR] {}", line));
                    if logs.len() > 500 { logs.drain(0..100); }
                }
            });
        }

        *self.child.lock().await = Some(child);
        *self.start_time.lock().await = Some(std::time::Instant::now());

        Ok("内核已启动".to_string())
    }

    /// Stop the running core
    pub async fn stop(&self) -> Result<String, String> {
        let mut child = self.child.lock().await;
        if let Some(mut c) = child.take() {
            c.kill().await.map_err(|e| format!("停止内核失败：{}", e))?;
            *self.start_time.lock().await = None;
            Ok("内核已停止".to_string())
        } else {
            Ok("内核未在运行".to_string())
        }
    }

    /// Get current status
    pub async fn status(&self) -> CoreStatus {
        let child = self.child.lock().await;
        let running = child.is_some();
        let core_type = self.core_type.lock().await.clone();
        let uptime_secs = if let Some(start_time) = *self.start_time.lock().await {
            Some(start_time.elapsed().as_secs())
        } else {
            None
        };
        CoreStatus { running, core_type, pid: None, uptime_secs }
    }

    /// Get recent logs
    pub async fn get_logs(&self, count: usize) -> Vec<String> {
        let logs = self.logs.lock().await;
        let start = if logs.len() > count { logs.len() - count } else { 0 };
        logs[start..].to_vec()
    }

    /// Test config file validity
    pub async fn test_config(&self, core_path: &str, config_path: &str) -> Result<String, String> {
        if !Path::new(core_path).exists() {
            return Err(format!("内核文件不存在：{}", core_path));
        }
        let output = Command::new(core_path)
            .arg("-test")
            .arg("-c")
            .arg(config_path)
            .output()
            .await
            .map_err(|e| format!("测试失败：{}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            Ok(format!("配置验证通过\n{}", stdout))
        } else {
            Err(format!("配置验证失败：\n{}\n{}", stdout, stderr))
        }
    }

    /// Get installed Xray version
    pub async fn get_version(&self, core_path: &str) -> Result<String, String> {
        if !Path::new(core_path).exists() {
            return Err("内核文件不存在，请先下载安装".to_string());
        }
        let output = Command::new(core_path)
            .arg("version")
            .output()
            .await
            .map_err(|e| format!("执行失败：{}", e))?;

        let text = String::from_utf8_lossy(&output.stdout).to_string();
        // First line is like: "Xray 26.3.27 (Xray, Penetrates Everything)"
        Ok(text.lines().next().unwrap_or("Unknown").to_string())
    }
}

/// Fetch the latest Xray-core release info from GitHub API
pub async fn fetch_latest_xray_release() -> Result<XrayRelease, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("v2rayAI/0.1.0")
        .build()
        .map_err(|e| e.to_string())?;

    #[derive(Deserialize)]
    struct GitHubRelease {
        tag_name: String,
        published_at: String,
        assets: Vec<GitHubAsset>,
    }
    #[derive(Deserialize)]
    struct GitHubAsset {
        name: String,
        browser_download_url: String,
    }

    let release: GitHubRelease = client
        .get("https://api.github.com/repos/XTLS/Xray-core/releases/latest")
        .send()
        .await
        .map_err(|e| format!("获取版本信息失败：{}", e))?
        .json()
        .await
        .map_err(|e| format!("解析响应失败：{}", e))?;

    // Determine target platform asset name
    let target = get_platform_asset_name();
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == target)
        .ok_or_else(|| format!("未找到平台对应的安装包：{}", target))?;

    Ok(XrayRelease {
        version: release.tag_name,
        download_url: asset.browser_download_url.clone(),
        published_at: release.published_at,
    })
}

/// Download and install Xray-core to the given directory
pub async fn download_xray(download_url: &str, install_dir: &str) -> Result<String, String> {
    tokio::fs::create_dir_all(install_dir)
        .await
        .map_err(|e| format!("创建目录失败：{}", e))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .user_agent("v2rayAI/0.1.0")
        .build()
        .map_err(|e| e.to_string())?;

    let bytes = client
        .get(download_url)
        .send()
        .await
        .map_err(|e| format!("下载失败：{}", e))?
        .bytes()
        .await
        .map_err(|e| format!("读取数据失败：{}", e))?;

    let zip_bytes = bytes.to_vec();
    let install_dir = install_dir.to_string();

    // All zip/fs operations are synchronous — run them in a blocking thread
    // so we don't hold non-Send ZipFile across await points.
    let out_path = tokio::task::spawn_blocking(move || -> Result<String, String> {
        use std::io::Cursor;

        let xray_binary = if cfg!(target_os = "windows") { "xray.exe" } else { "xray" };
        let cursor = Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor)
            .map_err(|e| format!("解压失败：{}", e))?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            if file.name() == xray_binary {
                let out_path = format!("{}/{}", install_dir, xray_binary);
                let mut out_file = std::fs::File::create(&out_path)
                    .map_err(|e| format!("创建文件失败：{}", e))?;
                std::io::copy(&mut file, &mut out_file)
                    .map_err(|e| format!("写入文件失败：{}", e))?;

                // Set executable bit on Unix
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o755))
                        .map_err(|e| format!("设置权限失败：{}", e))?;
                }

                return Ok(out_path);
            }
        }
        Err("压缩包中未找到 xray 可执行文件".to_string())
    })
    .await
    .map_err(|e| format!("解压线程错误：{}", e))??;

    Ok(out_path)
}

/// Helper: scan well-known locations for an existing xray / v2ray executable.
/// Search order:
///   1. App private install dir  (~/.v2rayai/xray/)
///   2. Homebrew (macOS: /opt/homebrew, /usr/local)
///   3. Common system directories (/usr/bin, /usr/local/bin, …)
///   4. `which` / `where` (system PATH)
pub async fn find_xray_core() -> Result<String, String> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());

    let binary_name = if cfg!(target_os = "windows") { "xray.exe" } else { "xray" };

    // --- 1. App private install directory (highest priority) ---
    let private_path = Path::new(&home).join(".v2rayai").join("xray").join(binary_name);
    if private_path.exists() {
        return Ok(private_path.to_string_lossy().to_string());
    }

    // --- 2 & 3. Static candidate paths (macOS / Linux / Windows) ---
    #[cfg(target_os = "macos")]
    let candidates: &[&str] = &[
        "/opt/homebrew/bin/xray",      // Homebrew ARM
        "/usr/local/bin/xray",         // Homebrew Intel / manual
        "/opt/homebrew/bin/v2ray",
        "/usr/local/bin/v2ray",
        "/usr/bin/xray",
        "/usr/bin/v2ray",
    ];

    #[cfg(target_os = "linux")]
    let candidates: &[&str] = &[
        "/usr/local/bin/xray",
        "/usr/bin/xray",
        "/opt/xray/xray",
        "/usr/local/bin/v2ray",
        "/usr/bin/v2ray",
    ];

    #[cfg(target_os = "windows")]
    let candidates: &[&str] = &[
        r"C:\Program Files\Xray\xray.exe",
        r"C:\xray\xray.exe",
        r"C:\v2ray\v2ray.exe",
    ];

    for path in candidates {
        if Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }

    // --- 4. System PATH (which / where) ---
    // Try both "xray" and "v2ray" names
    for bin in &["xray", "v2ray"] {
        let cmd = if cfg!(target_os = "windows") {
            Command::new("where").arg(bin).output().await
        } else {
            Command::new("which").arg(bin).output().await
        };

        if let Ok(output) = cmd {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(first_line) = stdout.lines().next() {
                    let trimmed = first_line.trim();
                    if !trimmed.is_empty() {
                        return Ok(trimmed.to_string());
                    }
                }
            }
        }
    }

    Err("未在系统中找到 xray/v2ray 内核".to_string())
}

/// Source how a core path was resolved
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreResolveResult {
    /// Resolved absolute path to the executable
    pub path: String,
    /// Where it was found: "existing" | "downloaded"
    pub source: String,
    /// Human-readable description of where it was found
    pub description: String,
}

/// High-level helper used by the frontend:
/// 1. Try to find an existing xray in the system.
/// 2. If none found, download the latest release into `~/.v2rayai/xray/`.
pub async fn resolve_or_download_core() -> Result<CoreResolveResult, String> {
    // Step 1 — look for an existing installation
    match find_xray_core().await {
        Ok(path) => {
            let description = if path.contains(".v2rayai") {
                "应用私有目录".to_string()
            } else if path.contains("homebrew") || path.contains("Homebrew") {
                "Homebrew".to_string()
            } else {
                "系统 PATH".to_string()
            };
            return Ok(CoreResolveResult {
                path,
                source: "existing".to_string(),
                description: format!("发现已安装的内核（{}）", description),
            });
        }
        Err(_) => { /* fall through to download */ }
    }

    // Step 2 — nothing found, download latest Xray release
    let release = fetch_latest_xray_release().await?;

    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    let install_dir = Path::new(&home)
        .join(".v2rayai")
        .join("xray")
        .to_string_lossy()
        .to_string();

    let path = download_xray(&release.download_url, &install_dir).await?;

    Ok(CoreResolveResult {
        path,
        source: "downloaded".to_string(),
        description: format!("已自动下载 Xray {}", release.version),
    })
}


fn get_platform_asset_name() -> String {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };

    let arch = if cfg!(target_arch = "x86_64") {
        "64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64-v8a"
    } else {
        "64"
    };

    if cfg!(target_os = "windows") {
        format!("Xray-{}-{}.zip", os, arch)
    } else if cfg!(target_os = "macos") {
        format!("Xray-{}-{}.zip", os, arch)
    } else {
        format!("Xray-{}-{}.zip", os, arch)
    }
}
