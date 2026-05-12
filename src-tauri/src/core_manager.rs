use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tauri::AppHandle;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

const MAX_CORE_LOG_LINES: usize = 2000;

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
    log_path: PathBuf,
}

impl CoreManager {
    pub fn new() -> Self {
        let log_path = core_log_path();
        Self {
            child: Arc::new(Mutex::new(None)),
            core_type: Arc::new(Mutex::new("xray".to_string())),
            start_time: Arc::new(Mutex::new(None)),
            logs: Arc::new(Mutex::new(load_recent_core_logs(&log_path, 500))),
            log_path,
        }
    }

    /// Start the v2ray/xray core with the given config
    pub async fn start(
        &self,
        core_path: &str,
        config_path: &str,
        app: Option<AppHandle>,
    ) -> Result<String, String> {
        if !Path::new(core_path).exists() {
            return Err(format!("内核文件不存在：{}", core_path));
        }

        self.stop().await?;
        self.stop_stale_cores(core_path, config_path).await;

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
            let log_path = self.log_path.clone();
            let app_clone = app.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    crate::traffic_monitor::parse_and_emit_traffic(app_clone.as_ref(), &line);
                    append_core_log(&log_path, &line).await;
                    let mut logs = logs.lock().await;
                    logs.push(line);
                    if logs.len() > MAX_CORE_LOG_LINES {
                        let drain_count = logs.len() - MAX_CORE_LOG_LINES;
                        logs.drain(0..drain_count);
                    }
                }
            });
        }

        // Capture stderr logs
        if let Some(stderr) = child.stderr.take() {
            let logs = self.logs.clone();
            let log_path = self.log_path.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let line = format!("[ERR] {}", line);
                    append_core_log(&log_path, &line).await;
                    let mut logs = logs.lock().await;
                    logs.push(line);
                    if logs.len() > MAX_CORE_LOG_LINES {
                        let drain_count = logs.len() - MAX_CORE_LOG_LINES;
                        logs.drain(0..drain_count);
                    }
                }
            });
        }

        tokio::time::sleep(Duration::from_millis(300)).await;
        if let Some(status) = child
            .try_wait()
            .map_err(|e| format!("检查内核状态失败：{}", e))?
        {
            let logs = self.logs.lock().await;
            let recent_logs = logs.iter().rev().take(20).cloned().collect::<Vec<_>>();
            let detail = if recent_logs.is_empty() {
                "未捕获到内核输出".to_string()
            } else {
                recent_logs.into_iter().rev().collect::<Vec<_>>().join("\n")
            };
            return Err(format!("内核启动后立即退出（{}）：\n{}", status, detail));
        }

        *self.child.lock().await = Some(child);
        *self.start_time.lock().await = Some(std::time::Instant::now());

        Ok("内核已启动".to_string())
    }

    #[cfg(unix)]
    async fn stop_stale_cores(&self, core_path: &str, config_path: &str) {
        let Ok(output) = Command::new("pgrep").arg("-f").arg(core_path).output().await else {
            return;
        };
        if !output.status.success() {
            return;
        }

        let current_pid = std::process::id();
        let stdout = String::from_utf8_lossy(&output.stdout);
        for pid in stdout.lines().filter_map(|line| line.trim().parse::<u32>().ok()) {
            if pid == current_pid {
                continue;
            }

            let Ok(args_output) = Command::new("ps")
                .arg("-p")
                .arg(pid.to_string())
                .arg("-o")
                .arg("args=")
                .output()
                .await
            else {
                continue;
            };
            let args = String::from_utf8_lossy(&args_output.stdout);
            if args.contains(core_path) && args.contains(config_path) {
                append_core_log(
                    &self.log_path,
                    &format!("[Info] 停止残留内核进程 PID {}", pid),
                )
                .await;
                let _ = Command::new("kill").arg(pid.to_string()).output().await;
            }
        }
    }

    #[cfg(not(unix))]
    async fn stop_stale_cores(&self, _core_path: &str, _config_path: &str) {}

    /// Stop the running core
    pub async fn stop(&self) -> Result<String, String> {
        let mut child = self.child.lock().await;
        if let Some(mut c) = child.take() {
            if let Ok(Some(_)) = c.try_wait() {
                *self.start_time.lock().await = None;
                return Ok("内核已停止".to_string());
            }
            c.kill().await.map_err(|e| format!("停止内核失败：{}", e))?;
            *self.start_time.lock().await = None;
            Ok("内核已停止".to_string())
        } else {
            Ok("内核未在运行".to_string())
        }
    }

    /// Get current status
    pub async fn status(&self) -> CoreStatus {
        let mut child = self.child.lock().await;
        let mut pid = child.as_ref().and_then(|c| c.id());
        let running = if let Some(c) = child.as_mut() {
            match c.try_wait() {
                Ok(Some(_)) => {
                    *child = None;
                    *self.start_time.lock().await = None;
                    pid = None;
                    false
                }
                Ok(None) => true,
                Err(e) => {
                    let line = format!("[ERR] 检查内核状态失败：{}", e);
                    append_core_log(&self.log_path, &line).await;
                    let mut logs = self.logs.lock().await;
                    logs.push(line);
                    false
                }
            }
        } else {
            false
        };
        let core_type = self.core_type.lock().await.clone();
        let uptime_secs = if let Some(start_time) = *self.start_time.lock().await {
            Some(start_time.elapsed().as_secs())
        } else {
            None
        };
        CoreStatus {
            running,
            core_type,
            pid,
            uptime_secs,
        }
    }

    /// Get recent logs
    pub async fn get_logs(&self, count: usize) -> Vec<String> {
        match tokio::fs::read_to_string(&self.log_path).await {
            Ok(text) => tail_lines(&text, count),
            Err(_) => {
                let logs = self.logs.lock().await;
                let start = if logs.len() > count {
                    logs.len() - count
                } else {
                    0
                };
                logs[start..].to_vec()
            }
        }
    }

    pub async fn clear_logs(&self) -> Result<(), String> {
        self.logs.lock().await.clear();
        match tokio::fs::remove_file(&self.log_path).await {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("清空内核日志失败：{}", e)),
        }
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

        let xray_binary = if cfg!(target_os = "windows") {
            "xray.exe"
        } else {
            "xray"
        };
        let cursor = Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("解压失败：{}", e))?;

        let mut binary_path: Option<String> = None;
        let data_files = ["geoip.dat", "geosite.dat"];

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            if file.name() == xray_binary {
                let out_path = format!("{}/{}", install_dir, xray_binary);
                let mut out_file =
                    std::fs::File::create(&out_path).map_err(|e| format!("创建文件失败：{}", e))?;
                std::io::copy(&mut file, &mut out_file)
                    .map_err(|e| format!("写入文件失败：{}", e))?;

                // Set executable bit on Unix
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o755))
                        .map_err(|e| format!("设置权限失败：{}", e))?;
                }

                binary_path = Some(out_path);
            } else if data_files.contains(&file.name()) {
                let out_path = format!("{}/{}", install_dir, file.name());
                let mut out_file =
                    std::fs::File::create(&out_path).map_err(|e| format!("创建数据文件失败：{}", e))?;
                std::io::copy(&mut file, &mut out_file)
                    .map_err(|e| format!("写入数据文件失败：{}", e))?;
            }
        }

        binary_path.ok_or_else(|| "压缩包中未找到 xray 可执行文件".to_string())
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

    let binary_name = if cfg!(target_os = "windows") {
        "xray.exe"
    } else {
        "xray"
    };

    // --- 1. App private install directory (highest priority) ---
    let private_path = Path::new(&home)
        .join(".v2rayai")
        .join("xray")
        .join(binary_name);
    if private_path.exists() {
        return Ok(private_path.to_string_lossy().to_string());
    }

    // --- 2 & 3. Static candidate paths (macOS / Linux / Windows) ---
    #[cfg(target_os = "macos")]
    let candidates: &[&str] = &[
        "/opt/homebrew/bin/xray", // Homebrew ARM
        "/usr/local/bin/xray",    // Homebrew Intel / manual
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

    // Xray release assets use the same `Xray-{os}-{arch}.zip` naming
    // on all three platforms (see https://github.com/XTLS/Xray-core/releases).
    format!("Xray-{}-{}.zip", os, arch)
}

fn core_log_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("logs")
        .join("core.log")
}

fn load_recent_core_logs(path: &Path, count: usize) -> Vec<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => tail_lines(&text, count),
        Err(_) => Vec::new(),
    }
}

fn tail_lines(text: &str, count: usize) -> Vec<String> {
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    if lines.len() > count {
        lines.drain(0..lines.len() - count);
    }
    lines
}

async fn append_core_log(path: &Path, line: &str) {
    use tokio::io::AsyncWriteExt;

    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    if let Ok(mut file) = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
    {
        let _ = file.write_all(line.as_bytes()).await;
        let _ = file.write_all(b"\n").await;
    }
}
