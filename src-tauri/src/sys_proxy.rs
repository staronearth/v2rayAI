/// System-level proxy management
/// macOS: uses `networksetup` CLI
/// Windows: uses Windows registry via `winreg`

#[derive(Debug, Clone)]
pub struct ProxySettings {
    pub http_host: String,
    pub http_port: u16,
    pub socks_host: String,
    pub socks_port: u16,
}

impl ProxySettings {
    pub fn local(http_port: u16, socks_port: u16) -> Self {
        Self {
            http_host: "127.0.0.1".into(),
            http_port,
            socks_host: "127.0.0.1".into(),
            socks_port,
        }
    }
}

/// Enable system proxy
pub async fn enable_system_proxy(settings: &ProxySettings) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    return enable_macos_proxy(settings).await;

    #[cfg(target_os = "windows")]
    return enable_windows_proxy(settings);

    #[cfg(target_os = "linux")]
    return enable_linux_proxy(settings).await;

    #[allow(unreachable_code)]
    Err("当前平台不支持自动设置系统代理".to_string())
}

/// Disable system proxy
pub async fn disable_system_proxy() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    return disable_macos_proxy().await;

    #[cfg(target_os = "windows")]
    return disable_windows_proxy();

    #[cfg(target_os = "linux")]
    return disable_linux_proxy().await;

    #[allow(unreachable_code)]
    Err("当前平台不支持自动关闭系统代理".to_string())
}

// ─── macOS ────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
async fn get_active_interfaces() -> Vec<String> {
    use tokio::process::Command;
    // Get all network services
    let output = Command::new("networksetup")
        .arg("-listallnetworkservices")
        .output()
        .await
        .unwrap_or_else(|_| std::process::Output {
            status: std::process::ExitStatus::default(),
            stdout: vec![],
            stderr: vec![],
        });
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .filter(|l| !l.starts_with('*') && !l.contains("disabled") && !l.is_empty())
        .skip(1) // skip header line "An asterisk (*) denotes..."
        .map(|l| l.trim().to_string())
        .collect()
}

#[cfg(target_os = "macos")]
async fn enable_macos_proxy(settings: &ProxySettings) -> Result<String, String> {
    let interfaces = get_active_interfaces().await;
    if interfaces.is_empty() {
        return Err("未找到可用网络接口".to_string());
    }

    let mut set_count = 0;
    let mut failures = Vec::new();
    for iface in &interfaces {
        let http_port = settings.http_port.to_string();
        let socks_port = settings.socks_port.to_string();
        let commands = [
            vec!["-setwebproxy", iface, &settings.http_host, &http_port],
            vec!["-setwebproxystate", iface, "on"],
            vec!["-setsecurewebproxy", iface, &settings.http_host, &http_port],
            vec!["-setsecurewebproxystate", iface, "on"],
            vec![
                "-setsocksfirewallproxy",
                iface,
                &settings.socks_host,
                &socks_port,
            ],
            vec!["-setsocksfirewallproxystate", iface, "on"],
        ];

        let mut iface_ok = true;
        for args in commands {
            if let Err(err) = run_networksetup(&args).await {
                iface_ok = false;
                failures.push(format!("{}: {}", iface, err));
            }
        }

        if iface_ok {
            set_count += 1;
        }
    }

    if set_count == 0 {
        return Err(format!("系统代理设置失败：{}", failures.join("; ")));
    }

    if !failures.is_empty() {
        log::warn!("部分网络接口代理设置失败：{}", failures.join("; "));
    }

    Ok(format!(
        "已在 {} 个网络接口启用系统代理（HTTP:{}, SOCKS:{}）",
        set_count, settings.http_port, settings.socks_port
    ))
}

#[cfg(target_os = "macos")]
async fn disable_macos_proxy() -> Result<String, String> {
    let interfaces = get_active_interfaces().await;
    let mut failures = Vec::new();
    for iface in &interfaces {
        let commands = [
            vec!["-setwebproxystate", iface, "off"],
            vec!["-setsecurewebproxystate", iface, "off"],
            vec!["-setsocksfirewallproxystate", iface, "off"],
        ];

        for args in commands {
            if let Err(err) = run_networksetup(&args).await {
                failures.push(format!("{}: {}", iface, err));
            }
        }
    }

    if !failures.is_empty() {
        return Err(format!(
            "关闭系统代理时部分命令失败：{}",
            failures.join("; ")
        ));
    }

    Ok(format!("已在 {} 个网络接口关闭系统代理", interfaces.len()))
}

#[cfg(target_os = "macos")]
async fn run_networksetup(args: &[&str]) -> Result<(), String> {
    use tokio::process::Command;

    let output = Command::new("networksetup")
        .args(args)
        .output()
        .await
        .map_err(|e| format!("networksetup 执行失败：{}", e))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let message = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("退出码 {}", output.status)
    };
    Err(format!("networksetup {} 失败：{}", args.join(" "), message))
}

// ─── Windows ────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn enable_windows_proxy(settings: &ProxySettings) -> Result<String, String> {
    use std::os::windows::ffi::OsStrExt;

    let proxy_server = format!(
        "http={}:{};https={}:{};socks={}:{}",
        settings.http_host,
        settings.http_port,
        settings.http_host,
        settings.http_port,
        settings.socks_host,
        settings.socks_port
    );

    set_windows_proxy_registry(true, &proxy_server)
        .map(|_| format!("已启用 Windows 系统代理：{}", proxy_server))
        .map_err(|e| format!("设置 Windows 代理失败：{}", e))
}

#[cfg(target_os = "windows")]
fn disable_windows_proxy() -> Result<String, String> {
    set_windows_proxy_registry(false, "")
        .map(|_| "已关闭 Windows 系统代理".to_string())
        .map_err(|e| format!("关闭 Windows 代理失败：{}", e))
}

#[cfg(target_os = "windows")]
fn set_windows_proxy_registry(enable: bool, proxy_server: &str) -> Result<(), String> {
    use std::process::Command;
    // Use reg.exe to avoid winreg dependency
    let enable_val = if enable { "1" } else { "0" };
    let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";

    Command::new("reg")
        .args([
            "add",
            key,
            "/v",
            "ProxyEnable",
            "/t",
            "REG_DWORD",
            "/d",
            enable_val,
            "/f",
        ])
        .output()
        .map_err(|e| e.to_string())?;

    if enable && !proxy_server.is_empty() {
        Command::new("reg")
            .args([
                "add",
                key,
                "/v",
                "ProxyServer",
                "/t",
                "REG_SZ",
                "/d",
                proxy_server,
                "/f",
            ])
            .output()
            .map_err(|e| e.to_string())?;
    }

    // Notify system of change
    Command::new("powershell")
        .args(["-Command",
            "& {Add-Type -Namespace WinAPI -Name NetHelper -MemberDefinition '[DllImport(\"wininet.dll\")]public static extern bool InternetSetOption(IntPtr h,int o,IntPtr b,int l);'; \
             [WinAPI.NetHelper]::InternetSetOption([IntPtr]::Zero,39,[IntPtr]::Zero,0); \
             [WinAPI.NetHelper]::InternetSetOption([IntPtr]::Zero,37,[IntPtr]::Zero,0)}"])
        .output()
        .ok();

    Ok(())
}

// ─── Linux ────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
async fn enable_linux_proxy(settings: &ProxySettings) -> Result<String, String> {
    // Set environment variables via gsettings (GNOME) if available
    use tokio::process::Command;

    let _ = Command::new("gsettings")
        .args(["set", "org.gnome.system.proxy", "mode", "manual"])
        .output()
        .await;
    let _ = Command::new("gsettings")
        .args([
            "set",
            "org.gnome.system.proxy.http",
            "host",
            &settings.http_host,
        ])
        .output()
        .await;
    let _ = Command::new("gsettings")
        .args([
            "set",
            "org.gnome.system.proxy.http",
            "port",
            &settings.http_port.to_string(),
        ])
        .output()
        .await;
    let _ = Command::new("gsettings")
        .args([
            "set",
            "org.gnome.system.proxy.socks",
            "host",
            &settings.socks_host,
        ])
        .output()
        .await;
    let _ = Command::new("gsettings")
        .args([
            "set",
            "org.gnome.system.proxy.socks",
            "port",
            &settings.socks_port.to_string(),
        ])
        .output()
        .await;

    Ok(format!(
        "已启用 Linux GNOME 系统代理（HTTP:{}, SOCKS:{}）",
        settings.http_port, settings.socks_port
    ))
}

#[cfg(target_os = "linux")]
async fn disable_linux_proxy() -> Result<String, String> {
    use tokio::process::Command;
    let _ = Command::new("gsettings")
        .args(["set", "org.gnome.system.proxy", "mode", "none"])
        .output()
        .await;
    Ok("已关闭 Linux GNOME 系统代理".to_string())
}
