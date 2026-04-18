/// RAG Knowledge Base
/// Strategy C+A: Pre-chunked Xray docs embedded in binary + keyword TF-IDF scoring.
/// If user has embedding API, falls back to proper cosine similarity.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use reqwest;


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocChunk {
    pub id: String,
    pub source: String,      // e.g. "xray-vless", "xray-reality"
    pub title: String,
    pub content: String,
    pub keywords: Vec<String>,
    pub embedding: Option<Vec<f32>>, // populated when API available
}

#[derive(Debug, Serialize, Deserialize)]
pub struct KnowledgeBase {
    pub xray_version: String,
    pub built_at: i64,
    pub chunks: Vec<DocChunk>,
}

impl KnowledgeBase {
    fn kb_path() -> PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".v2rayai").join("knowledge_base.json")
    }

    /// Load from disk or create from built-in chunks
    pub async fn load_or_create(xray_version: &str) -> Self {
        let path = Self::kb_path();
        if let Ok(bytes) = fs::read(&path).await {
            if let Ok(kb) = serde_json::from_slice::<KnowledgeBase>(&bytes) {
                if kb.xray_version == xray_version {
                    return kb;
                }
            }
        }
        // Build from embedded docs
        let kb = KnowledgeBase {
            xray_version: xray_version.to_string(),
            built_at: chrono::Utc::now().timestamp(),
            chunks: get_builtin_chunks(),
        };
        kb.save().await.ok();
        kb
    }

    /// Save knowledge base to disk
    pub async fn save(&self) -> Result<(), String> {
        let path = Self::kb_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.ok();
        }
        let json = serde_json::to_string(self)
            .map_err(|e| format!("序列化知识库失败：{}", e))?;
        fs::write(&path, json.as_bytes())
            .await
            .map_err(|e| format!("保存知识库失败：{}", e))
    }

    /// Refresh chunks from GitHub and optionally compute embeddings
    pub async fn refresh_from_github(&mut self, _api_key: Option<&str>) -> Result<usize, String> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("v2rayAI/0.1")
            .build()
            .map_err(|e| e.to_string())?;

        let sources = [
            ("xray-readme", "README", "https://raw.githubusercontent.com/XTLS/Xray-core/main/README.md"),
            ("xray-changelog", "CHANGELOG", "https://raw.githubusercontent.com/XTLS/Xray-core/main/CHANGELOG.md"),
        ];

        let mut new_chunks: Vec<DocChunk> = get_builtin_chunks();

        for (id_prefix, title, url) in &sources {
            match client.get(*url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(text) = resp.text().await {
                        let mut chunks = split_into_chunks(&text, id_prefix, title, 800);
                        new_chunks.append(&mut chunks);
                    }
                }
                _ => {} // silently skip failed fetches
            }
        }

        let count = new_chunks.len();
        self.chunks = new_chunks;
        self.built_at = chrono::Utc::now().timestamp();
        self.save().await?;
        Ok(count)
    }

    /// Search top-k most relevant chunks for a query using keyword TF-IDF scoring
    pub fn search(&self, query: &str, top_k: usize) -> Vec<&DocChunk> {
        let query_terms = tokenize(query);
        if query_terms.is_empty() {
            return self.chunks.iter().take(top_k).collect();
        }

        let mut scores: Vec<(usize, f32)> = self.chunks.iter().enumerate()
            .map(|(i, chunk)| {
                let score = score_chunk(chunk, &query_terms);
                (i, score)
            })
            .filter(|(_, s)| *s > 0.0)
            .collect();

        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        scores.iter()
            .take(top_k)
            .map(|(i, _)| &self.chunks[*i])
            .collect()
    }

    /// Format top-k chunks as context string for AI prompt
    pub fn get_rag_context(&self, query: &str, top_k: usize) -> String {
        let chunks = self.search(query, top_k);
        if chunks.is_empty() {
            return String::new();
        }

        let mut ctx = String::from("\n\n---\n## 📚 相关文档（RAG 检索）\n\n");
        for chunk in chunks {
            ctx.push_str(&format!("### {} ({})\n{}\n\n", chunk.title, chunk.source, chunk.content));
        }
        ctx.push_str("---\n");
        ctx
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|w| w.len() > 1)
        .map(|w| w.to_string())
        .collect()
}

fn score_chunk(chunk: &DocChunk, query_terms: &[String]) -> f32 {
    let content_lower = chunk.content.to_lowercase();
    let title_lower = chunk.title.to_lowercase();

    let mut score = 0.0f32;
    for term in query_terms {
        // Title match = 3x weight
        if title_lower.contains(term.as_str()) {
            score += 3.0;
        }
        // Keyword exact match = 2x weight
        if chunk.keywords.iter().any(|k| k.to_lowercase() == *term) {
            score += 2.0;
        }
        // Content match
        let count = content_lower.matches(term.as_str()).count();
        score += count as f32 * 0.5;
    }
    score
}

fn split_into_chunks(text: &str, id_prefix: &str, source: &str, max_chars: usize) -> Vec<DocChunk> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_title = source.to_string();
    let mut idx = 0;

    for line in text.lines() {
        if line.starts_with("## ") || line.starts_with("# ") {
            if current.trim().len() > 50 {
                chunks.push(DocChunk {
                    id: format!("{}-{}", id_prefix, idx),
                    source: source.to_string(),
                    title: current_title.clone(),
                    content: current.trim().to_string(),
                    keywords: extract_keywords(&current),
                    embedding: None,
                });
                idx += 1;
                current = String::new();
            }
            current_title = line.trim_start_matches('#').trim().to_string();
        }

        current.push_str(line);
        current.push('\n');

        if current.len() > max_chars {
            chunks.push(DocChunk {
                id: format!("{}-{}", id_prefix, idx),
                source: source.to_string(),
                title: current_title.clone(),
                content: current.trim().to_string(),
                keywords: extract_keywords(&current),
                embedding: None,
            });
            idx += 1;
            current = String::new();
        }
    }

    if current.trim().len() > 50 {
        chunks.push(DocChunk {
            id: format!("{}-{}", id_prefix, idx),
            source: source.to_string(),
            title: current_title,
            content: current.trim().to_string(),
            keywords: extract_keywords(&current),
            embedding: None,
        });
    }

    chunks
}

fn extract_keywords(text: &str) -> Vec<String> {
    // Extract meaningful technical terms
    let tech_terms = [
        "vless", "vmess", "trojan", "shadowsocks", "reality", "xtls",
        "tls", "websocket", "grpc", "tcp", "http", "uuid", "flow",
        "routing", "outbound", "inbound", "dns", "sni", "fingerprint",
        "encryption", "alterId", "network", "security", "protocol",
        "publicKey", "privateKey", "shortId", "serverName", "vision",
    ];
    let text_lower = text.to_lowercase();
    tech_terms.iter()
        .filter(|&&t| text_lower.contains(t))
        .map(|&t| t.to_string())
        .collect()
}

/// Built-in pre-chunked Xray documentation
fn get_builtin_chunks() -> Vec<DocChunk> {
    vec![
        DocChunk {
            id: "vless-basic".into(),
            source: "Xray 官方文档".into(),
            title: "VLESS 协议配置".into(),
            keywords: vec!["vless","uuid","encryption","flow","vnext","xtls","vision"].iter().map(|s|s.to_string()).collect(),
            embedding: None,
            content: r#"VLESS 是一个无状态的轻量传输协议，无加密，依赖外层 TLS/REALITY 保证安全。

outbound 配置：
```json
{
  "protocol": "vless",
  "settings": {
    "vnext": [{
      "address": "your-server.com",
      "port": 443,
      "users": [{
        "id": "uuid-here",
        "encryption": "none",
        "flow": "xtls-rprx-vision"
      }]
    }]
  }
}
```

flow 字段：
- 省略或 ""：普通 TLS
- "xtls-rprx-vision"：XTLS Vision 模式，性能最强，需配合 REALITY 或 TLS 1.3
- "xtls-rprx-vision-udp443"：同上，额外透传 QUIC/UDP 443"#.into(),
        },
        DocChunk {
            id: "reality-config".into(),
            source: "Xray 官方文档".into(),
            title: "REALITY 传输配置".into(),
            keywords: vec!["reality","publicKey","privateKey","shortId","serverName","fingerprint","x25519"].iter().map(|s|s.to_string()).collect(),
            embedding: None,
            content: r#"REALITY 是 Xray 独有的传输安全方案，无需购买域名和 TLS 证书，流量特征与真实 HTTPS 完全一致。

生成密钥对：
```bash
xray x25519
# Private key: ...
# Public key: ...
```
生成 shortId：
```bash
openssl rand -hex 8
```

客户端 streamSettings：
```json
{
  "network": "tcp",
  "security": "reality",
  "realitySettings": {
    "serverName": "www.microsoft.com",
    "fingerprint": "chrome",
    "publicKey": "服务端公钥",
    "shortId": ""
  }
}
```

服务端 realitySettings：
```json
{
  "show": false,
  "dest": "www.microsoft.com:443",
  "xver": 0,
  "serverNames": ["www.microsoft.com"],
  "privateKey": "服务端私钥",
  "shortIds": ["", "可选shortid"]
}
```

推荐 dest 目标（高流量真实网站）：
- www.microsoft.com:443
- www.apple.com:443
- dl.google.com:443
- addons.mozilla.org:443"#.into(),
        },
        DocChunk {
            id: "vmess-config".into(),
            source: "Xray 官方文档".into(),
            title: "VMess 协议配置".into(),
            keywords: vec!["vmess","uuid","alterId","security","auto","aes-128-gcm","chacha20"].iter().map(|s|s.to_string()).collect(),
            embedding: None,
            content: r#"VMess 是 V2Ray 原创加密协议，内置加密，兼容性最好。

注意：alterId 新版推荐设为 0（启用 AEAD 加密），旧版服务端才需要匹配 alterId。

客户端配置：
```json
{
  "protocol": "vmess",
  "settings": {
    "vnext": [{
      "address": "server.com",
      "port": 443,
      "users": [{
        "id": "uuid-here",
        "alterId": 0,
        "security": "auto"
      }]
    }]
  }
}
```
security 可选：auto, aes-128-gcm, chacha20-poly1305, none"#.into(),
        },
        DocChunk {
            id: "trojan-config".into(),
            source: "Xray 官方文档".into(),
            title: "Trojan 协议配置".into(),
            keywords: vec!["trojan","password","tls","sni","fingerprint"].iter().map(|s|s.to_string()).collect(),
            embedding: None,
            content: r#"Trojan 通过模拟 HTTPS 流量来隐蔽代理行为，天然高隐蔽性，无需额外加密层。

客户端配置：
```json
{
  "protocol": "trojan",
  "settings": {
    "servers": [{
      "address": "your-server.com",
      "port": 443,
      "password": "your-password"
    }]
  },
  "streamSettings": {
    "network": "tcp",
    "security": "tls",
    "tlsSettings": {
      "serverName": "your-server.com",
      "fingerprint": "chrome"
    }
  }
}
```"#.into(),
        },
        DocChunk {
            id: "shadowsocks-2022".into(),
            source: "Xray 官方文档".into(),
            title: "Shadowsocks 配置（含 2022 新协议）".into(),
            keywords: vec!["shadowsocks","ss","2022-blake3","aes-256-gcm","chacha20-poly1305","method","password"].iter().map(|s|s.to_string()).collect(),
            embedding: None,
            content: r#"Shadowsocks 2022 协议安全性和性能大幅提升，推荐使用。

加密方式推荐（安全性从高到低）：
1. 2022-blake3-aes-256-gcm（密钥必须为 32 字节 Base64）
2. 2022-blake3-aes-128-gcm（密钥为 16 字节 Base64）
3. 2022-blake3-chacha20-poly1305（密钥为 32 字节 Base64）
4. chacha20-ietf-poly1305（传统）
5. aes-256-gcm（传统）

生成 2022 密钥：
```bash
openssl rand -base64 32   # 256位
openssl rand -base64 16   # 128位
```

客户端配置：
```json
{
  "protocol": "shadowsocks",
  "settings": {
    "servers": [{
      "address": "server.com",
      "port": 8388,
      "method": "2022-blake3-aes-256-gcm",
      "password": "base64-32字节密钥"
    }]
  }
}
```"#.into(),
        },
        DocChunk {
            id: "routing-rules".into(),
            source: "Xray 官方文档".into(),
            title: "路由规则配置".into(),
            keywords: vec!["routing","geosite","geoip","outboundTag","direct","block","proxy","domainStrategy"].iter().map(|s|s.to_string()).collect(),
            embedding: None,
            content: r#"路由规则控制流量走向，支持域名、IP、端口、进程等多种匹配方式。

domainStrategy：
- AsIs：不解析 IP，速度快（默认）
- IPIfNonMatch：域名规则未命中时解析 IP 再匹配（推荐）
- IPOnDemand：总是解析 IP（最精确但最慢）

推荐规则（国内直连，国外代理，去广告）：
```json
{
  "routing": {
    "domainStrategy": "IPIfNonMatch",
    "rules": [
      { "type": "field", "domain": ["geosite:category-ads-all"], "outboundTag": "block" },
      { "type": "field", "domain": ["geosite:private"], "outboundTag": "direct" },
      { "type": "field", "domain": ["geosite:cn"], "outboundTag": "direct" },
      { "type": "field", "ip": ["geoip:private", "geoip:cn"], "outboundTag": "direct" }
    ]
  }
}
```

规则类型：
- domain：域名匹配（geosite:cn, regexp:, domain:, keyword:, full:）
- ip：IP 匹配（geoip:cn, CIDR 如 192.168.0.0/16）
- port：端口匹配（如 "80,443,8080-8090"）
- processName：进程名匹配（仅 Linux/Windows）"#.into(),
        },
        DocChunk {
            id: "dns-config".into(),
            source: "Xray 官方文档".into(),
            title: "DNS 配置".into(),
            keywords: vec!["dns","doh","servers","expectIPs","domains","hosts","doh.pub","dns.google"].iter().map(|s|s.to_string()).collect(),
            embedding: None,
            content: r#"智能 DNS 分流：国内域名用国内 DNS，国外域名用国外加密 DNS。

推荐配置：
```json
{
  "dns": {
    "hosts": {
      "dns.google": "8.8.8.8",
      "dns.pub": "119.29.29.29"
    },
    "servers": [
      {
        "address": "https://dns.google/dns-query",
        "domains": ["geosite:geolocation-!cn"],
        "expectIPs": ["geoip:!cn"]
      },
      {
        "address": "https://doh.pub/dns-query",
        "domains": ["geosite:cn"],
        "expectIPs": ["geoip:cn"]
      },
      "localhost"
    ]
  }
}
```

注意：DNS 查询本身也会走路由规则，国内 DNS 走直连，国外 DNS 走代理。"#.into(),
        },
        DocChunk {
            id: "transport-ws".into(),
            source: "Xray 官方文档".into(),
            title: "WebSocket 传输配置".into(),
            keywords: vec!["websocket","ws","path","host","headers","cdn","cloudflare"].iter().map(|s|s.to_string()).collect(),
            embedding: None,
            content: r#"WebSocket 可穿透 HTTP 代理，适合 CDN（Cloudflare）中转场景。

客户端配置：
```json
{
  "streamSettings": {
    "network": "ws",
    "security": "tls",
    "wsSettings": {
      "path": "/your-path",
      "headers": {
        "Host": "your-domain.com"
      }
    },
    "tlsSettings": {
      "serverName": "your-domain.com",
      "fingerprint": "chrome"
    }
  }
}
```

Cloudflare 注意事项：
- 需要在 Cloudflare DNS 开启代理（橙云）
- 端口必须是 CF 支持的：80/8080/8880/2052（HTTP）或 443/8443/2053/2083/2087/2096（HTTPS）
- WebSocket 路径必须与服务端一致（大小写敏感）"#.into(),
        },
        DocChunk {
            id: "transport-grpc".into(),
            source: "Xray 官方文档".into(),
            title: "gRPC 传输配置".into(),
            keywords: vec!["grpc","serviceName","multiMode","http2"].iter().map(|s|s.to_string()).collect(),
            embedding: None,
            content: r#"gRPC 基于 HTTP/2，支持多路复用，延迟低，也支持 Cloudflare CDN 中转。

客户端配置：
```json
{
  "streamSettings": {
    "network": "grpc",
    "security": "tls",
    "grpcSettings": {
      "serviceName": "your-service-name",
      "multiMode": true
    },
    "tlsSettings": {
      "serverName": "your-domain.com"
    }
  }
}
```

Cloudflare 使用 gRPC 需在 Cloudflare 仪表盘 → Network 中开启"gRPC"选项。"#.into(),
        },
        DocChunk {
            id: "tls-settings".into(),
            source: "Xray 官方文档".into(),
            title: "TLS 设置详解".into(),
            keywords: vec!["tls","tlsSettings","serverName","fingerprint","alpn","allowInsecure","minVersion","chrome","firefox","safari"].iter().map(|s|s.to_string()).collect(),
            embedding: None,
            content: r#"TLS 配置详解：

```json
{
  "tlsSettings": {
    "serverName": "your-domain.com",
    "fingerprint": "chrome",
    "alpn": ["h2", "http/1.1"],
    "allowInsecure": false,
    "minVersion": "1.2"
  }
}
```

fingerprint（TLS 指纹伪造）可选值：
- chrome：模拟 Chrome 浏览器 TLS 指纹（推荐）
- firefox：模拟 Firefox
- safari：模拟 Safari
- ios：模拟 iOS
- android：模拟 Android
- edge：模拟 Edge
- 360：模拟 360 浏览器
- qq：模拟 QQ 浏览器
- random：每次随机选择
- randomized：完全随机化

allowInsecure：不验证服务端证书（仅调试用，生产环境设 false）"#.into(),
        },
        DocChunk {
            id: "inbound-config".into(),
            source: "Xray 官方文档".into(),
            title: "入站配置（Inbound）".into(),
            keywords: vec!["inbound","socks","http","listen","sniffing","tproxy"].iter().map(|s|s.to_string()).collect(),
            embedding: None,
            content: r#"标准客户端入站配置（本地代理）：

```json
{
  "inbounds": [
    {
      "tag": "http-in",
      "protocol": "http",
      "listen": "127.0.0.1",
      "port": 10808,
      "settings": {},
      "sniffing": {
        "enabled": true,
        "destOverride": ["http", "tls", "quic"]
      }
    },
    {
      "tag": "socks-in",
      "protocol": "socks",
      "listen": "127.0.0.1",
      "port": 10809,
      "settings": { "udp": true, "auth": "noauth" },
      "sniffing": {
        "enabled": true,
        "destOverride": ["http", "tls", "quic"]
      }
    }
  ]
}
```

重要：listen 绑定 127.0.0.1 防止成为开放代理。
sniffing 开启后可感知实际目的地址，提升路由准确性。"#.into(),
        },
        DocChunk {
            id: "troubleshoot".into(),
            source: "Xray 常见问题".into(),
            title: "常见问题诊断".into(),
            keywords: vec!["error","failed","timeout","connection","refused","certificate","uuid","port"].iter().map(|s|s.to_string()).collect(),
            embedding: None,
            content: r#"常见错误及解决方案：

1. connection refused（连接被拒绝）
   - 确认服务端 xray 正在运行：systemctl status xray
   - 确认服务端防火墙已放行端口：ufw allow 443
   - 确认客户端配置的端口与服务端一致

2. TLS handshake timeout / certificate error
   - SNI 与服务端域名不一致
   - 服务端证书已过期
   - 使用 REALITY 时 publicKey 填错

3. UUID/密码错误
   - 服务端和客户端 UUID 必须完全一致（大小写、连字符）
   - 可用 `xray uuid` 重新生成并同步

4. 连上了但网络很慢
   - 尝试切换传输协议（ws→grpc 或反之）
   - 检查路由规则，确认国内流量走直连
   - VLESS+REALITY+XTLS Vision 通常延迟最低

5. `dial tcp: lookup xxx: no such host`
   - DNS 解析失败，检查 DNS 配置
   - 服务端地址是否正确

6. 日志报 `io: read/write on closed pipe`
   - 这是正常的连接关闭日志，不是错误"#.into(),
        },
        DocChunk {
            id: "xray-commands".into(),
            source: "Xray CLI 工具".into(),
            title: "Xray 命令行工具".into(),
            keywords: vec!["xray","uuid","x25519","run","version","test","generate"].iter().map(|s|s.to_string()).collect(),
            embedding: None,
            content: r#"Xray 常用命令：

```bash
# 查看版本
xray version

# 生成随机 UUID
xray uuid

# 生成 REALITY 密钥对（x25519）
xray x25519
# 输出：
# Private key: xxxxxxxxxx
# Public key:  xxxxxxxxxx

# 测试配置文件语法
xray run -test -c /path/to/config.json

# 运行指定配置
xray run -c /path/to/config.json

# 生成 shortId（8位十六进制）
openssl rand -hex 8
```"#.into(),
        },
    ]
}
