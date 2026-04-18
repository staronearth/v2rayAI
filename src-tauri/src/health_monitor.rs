/// Health monitoring and real-world latency testing for proxy connections

use serde::{Deserialize, Serialize};
use std::net::ToSocketAddrs;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatencyResult {
    pub tcp_ms: Option<u64>,
    pub http_ms: Option<u64>,
    pub reachable: bool,
    pub error: Option<String>,
}

/// Test TCP connection latency to a server (no proxy)
pub async fn test_tcp_latency(host: &str, port: u16, timeout_secs: u64) -> LatencyResult {
    let addr_str = format!("{}:{}", host, port);
    let timeout = std::time::Duration::from_secs(timeout_secs);

    let addr = match addr_str.to_socket_addrs() {
        Ok(mut addrs) => match addrs.next() {
            Some(a) => a,
            None => return LatencyResult { tcp_ms: None, http_ms: None, reachable: false,
                error: Some("DNS 解析失败".into()) },
        },
        Err(e) => return LatencyResult { tcp_ms: None, http_ms: None, reachable: false,
            error: Some(format!("地址解析失败：{}", e)) },
    };

    let start = Instant::now();
    match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(addr)).await {
        Ok(Ok(_)) => {
            let ms = start.elapsed().as_millis() as u64;
            LatencyResult { tcp_ms: Some(ms), http_ms: None, reachable: true, error: None }
        }
        Ok(Err(e)) => LatencyResult { tcp_ms: None, http_ms: None, reachable: false,
            error: Some(format!("连接失败：{}", e)) },
        Err(_) => LatencyResult { tcp_ms: None, http_ms: None, reachable: false,
            error: Some(format!("超时（{}s）", timeout_secs)) },
    }
}

/// Test HTTP latency through the local proxy (real-world check)
pub async fn test_via_proxy(http_proxy_port: u16, timeout_secs: u64) -> LatencyResult {
    let proxy = format!("http://127.0.0.1:{}", http_proxy_port);

    let client = match reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy).unwrap())
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
    {
        Ok(c) => c,
        Err(e) => return LatencyResult {
            tcp_ms: None, http_ms: None, reachable: false,
            error: Some(format!("创建 HTTP 客户端失败：{}", e)),
        },
    };

    let start = Instant::now();
    match client.get("http://www.gstatic.com/generate_204").send().await {
        Ok(resp) => {
            let ms = start.elapsed().as_millis() as u64;
            if resp.status().as_u16() == 204 {
                LatencyResult { tcp_ms: None, http_ms: Some(ms), reachable: true, error: None }
            } else {
                LatencyResult { tcp_ms: None, http_ms: Some(ms), reachable: false,
                    error: Some(format!("HTTP {}", resp.status())) }
            }
        }
        Err(e) => LatencyResult {
            tcp_ms: None, http_ms: None, reachable: false,
            error: Some(format!("代理请求失败：{}", e)),
        },
    }
}

/// Perform a complete latency test: TCP + proxy HTTP
pub async fn full_latency_test(host: &str, port: u16, http_proxy_port: u16) -> LatencyResult {
    let tcp = test_tcp_latency(host, port, 5).await;
    if !tcp.reachable {
        return tcp;
    }

    let proxy = test_via_proxy(http_proxy_port, 8).await;
    LatencyResult {
        tcp_ms: tcp.tcp_ms,
        http_ms: proxy.http_ms,
        reachable: proxy.reachable,
        error: proxy.error,
    }
}

// ── Health monitor (background task) ──────────────────────────

use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthEvent {
    pub healthy: bool,
    pub consecutive_failures: u32,
    pub last_check_ms: Option<u64>,
    pub message: String,
}

pub struct HealthMonitor {
    pub tx: broadcast::Sender<HealthEvent>,
    running: Arc<Mutex<bool>>,
}

impl HealthMonitor {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(16);
        Self {
            tx,
            running: Arc::new(Mutex::new(false)),
        }
    }

    /// Start background health check loop
    pub async fn start(&self, http_proxy_port: u16, interval_secs: u64) {
        let mut is_running = self.running.lock().await;
        if *is_running {
            return;
        }
        *is_running = true;
        drop(is_running);

        let tx = self.tx.clone();
        let running = self.running.clone();

        tokio::spawn(async move {
            let mut failures = 0u32;
            loop {
                // Check if we should stop
                if !*running.lock().await {
                    break;
                }

                let result = test_via_proxy(http_proxy_port, 8).await;
                let healthy = result.reachable;

                if healthy {
                    failures = 0;
                } else {
                    failures += 1;
                }

                let event = HealthEvent {
                    healthy,
                    consecutive_failures: failures,
                    last_check_ms: result.http_ms,
                    message: if healthy {
                        format!("连接正常 ({}ms)", result.http_ms.unwrap_or(0))
                    } else {
                        format!("连接异常 (连续{}次失败): {}",
                            failures,
                            result.error.unwrap_or_default())
                    },
                };

                tx.send(event).ok();
                tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
            }
        });
    }

    /// Stop the health monitor
    pub async fn stop(&self) {
        *self.running.lock().await = false;
    }

    /// Subscribe to health events
    pub fn subscribe(&self) -> broadcast::Receiver<HealthEvent> {
        self.tx.subscribe()
    }
}
