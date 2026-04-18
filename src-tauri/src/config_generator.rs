use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub id: String,
    pub name: String,
    pub protocol: String,
    pub address: String,
    pub port: u16,
    pub uuid: Option<String>,
    pub password: Option<String>,
    pub alter_id: Option<u32>,
    pub encryption: Option<String>,
    pub flow: Option<String>,
    pub network: Option<String>,
    pub security: Option<String>,
    pub sni: Option<String>,
    pub path: Option<String>,
    pub host: Option<String>,
    pub reality_public_key: Option<String>,
    pub reality_short_id: Option<String>,
    pub fingerprint: Option<String>,
}

impl ServerConfig {
    /// Generate a full v2ray/xray config JSON from this server config
    pub fn to_v2ray_config(&self, http_port: u16, socks_port: u16, routing_mode: &str) -> Value {
        let outbound = self.to_outbound();
        let routing = Self::build_routing(routing_mode);

        serde_json::json!({
            "log": { 
                "loglevel": "info",
                "access": ""
            },
            "dns": {
                "servers": [
                    {
                        "address": "https://dns.google/dns-query",
                        "domains": ["geosite:geolocation-!cn"]
                    },
                    {
                        "address": "223.5.5.5",
                        "domains": ["geosite:cn"],
                        "expectIPs": ["geoip:cn"]
                    },
                    "localhost"
                ]
            },
            "routing": routing,
            "inbounds": [
                {
                    "tag": "http-in",
                    "protocol": "http",
                    "listen": "127.0.0.1",
                    "port": http_port,
                    "settings": {}
                },
                {
                    "tag": "socks-in",
                    "protocol": "socks",
                    "listen": "127.0.0.1",
                    "port": socks_port,
                    "settings": { "udp": true }
                }
            ],
            "outbounds": [
                outbound,
                { "tag": "direct", "protocol": "freedom" },
                { "tag": "block", "protocol": "blackhole" }
            ]
        })
    }

    fn to_outbound(&self) -> Value {
        match self.protocol.as_str() {
            "vmess" => self.to_vmess_outbound(),
            "vless" => self.to_vless_outbound(),
            "trojan" => self.to_trojan_outbound(),
            "shadowsocks" | "ss" => self.to_ss_outbound(),
            _ => self.to_vless_outbound(),
        }
    }

    fn to_vmess_outbound(&self) -> Value {
        let stream = self.build_stream_settings();
        serde_json::json!({
            "tag": "proxy",
            "protocol": "vmess",
            "settings": {
                "vnext": [{
                    "address": self.address,
                    "port": self.port,
                    "users": [{
                        "id": self.uuid.as_deref().unwrap_or(""),
                        "alterId": self.alter_id.unwrap_or(0),
                        "security": self.encryption.as_deref().unwrap_or("auto")
                    }]
                }]
            },
            "streamSettings": stream
        })
    }

    fn to_vless_outbound(&self) -> Value {
        let stream = self.build_stream_settings();
        let mut user: serde_json::Map<String, Value> = serde_json::Map::new();
        user.insert("id".into(), Value::String(self.uuid.clone().unwrap_or_default()));
        user.insert("encryption".into(), Value::String(self.encryption.clone().unwrap_or_else(|| "none".into())));
        if let Some(flow) = &self.flow {
            user.insert("flow".into(), Value::String(flow.clone()));
        }

        serde_json::json!({
            "tag": "proxy",
            "protocol": "vless",
            "settings": {
                "vnext": [{
                    "address": self.address,
                    "port": self.port,
                    "users": [user]
                }]
            },
            "streamSettings": stream
        })
    }

    fn to_trojan_outbound(&self) -> Value {
        let stream = self.build_stream_settings();
        serde_json::json!({
            "tag": "proxy",
            "protocol": "trojan",
            "settings": {
                "servers": [{
                    "address": self.address,
                    "port": self.port,
                    "password": self.password.as_deref().unwrap_or("")
                }]
            },
            "streamSettings": stream
        })
    }

    fn to_ss_outbound(&self) -> Value {
        serde_json::json!({
            "tag": "proxy",
            "protocol": "shadowsocks",
            "settings": {
                "servers": [{
                    "address": self.address,
                    "port": self.port,
                    "method": self.encryption.as_deref().unwrap_or("aes-256-gcm"),
                    "password": self.password.as_deref().unwrap_or("")
                }]
            }
        })
    }

    fn build_stream_settings(&self) -> Value {
        let network = self.network.as_deref().unwrap_or("tcp");
        let security = self.security.as_deref().unwrap_or("none");

        let mut stream: serde_json::Map<String, Value> = serde_json::Map::new();
        stream.insert("network".into(), Value::String(network.into()));
        stream.insert("security".into(), Value::String(security.into()));

        // Network-specific settings
        match network {
            "ws" => {
                let mut ws: serde_json::Map<String, Value> = serde_json::Map::new();
                if let Some(path) = &self.path {
                    ws.insert("path".into(), Value::String(path.clone()));
                }
                if let Some(host) = &self.host {
                    let mut headers: serde_json::Map<String, Value> = serde_json::Map::new();
                    headers.insert("Host".into(), Value::String(host.clone()));
                    ws.insert("headers".into(), Value::Object(headers));
                }
                stream.insert("wsSettings".into(), Value::Object(ws));
            }
            "grpc" => {
                let mut grpc: serde_json::Map<String, Value> = serde_json::Map::new();
                if let Some(path) = &self.path {
                    grpc.insert("serviceName".into(), Value::String(path.clone()));
                }
                stream.insert("grpcSettings".into(), Value::Object(grpc));
            }
            _ => {}
        }

        // Security settings
        match security {
            "tls" => {
                let mut tls: serde_json::Map<String, Value> = serde_json::Map::new();
                if let Some(sni) = &self.sni {
                    tls.insert("serverName".into(), Value::String(sni.clone()));
                }
                if let Some(fp) = &self.fingerprint {
                    tls.insert("fingerprint".into(), Value::String(fp.clone()));
                }
                stream.insert("tlsSettings".into(), Value::Object(tls));
            }
            "reality" => {
                let mut reality: serde_json::Map<String, Value> = serde_json::Map::new();
                if let Some(sni) = &self.sni {
                    reality.insert("serverName".into(), Value::String(sni.clone()));
                }
                if let Some(fp) = &self.fingerprint {
                    reality.insert("fingerprint".into(), Value::String(fp.clone()));
                }
                if let Some(pk) = &self.reality_public_key {
                    reality.insert("publicKey".into(), Value::String(pk.clone()));
                }
                if let Some(sid) = &self.reality_short_id {
                    reality.insert("shortId".into(), Value::String(sid.clone()));
                }
                stream.insert("realitySettings".into(), Value::Object(reality));
            }
            _ => {}
        }

        Value::Object(stream)
    }

    fn build_routing(mode: &str) -> Value {
        match mode {
            "global" => serde_json::json!({
                "domainStrategy": "AsIs",
                "rules": []
            }),
            "direct" => serde_json::json!({
                "domainStrategy": "AsIs",
                "rules": [{
                    "type": "field",
                    "port": "0-65535",
                    "outboundTag": "direct"
                }]
            }),
            _ => serde_json::json!({
                "domainStrategy": "IPIfNonMatch",
                "rules": [
                    { "type": "field", "domain": ["geosite:category-ads-all"], "outboundTag": "block" },
                    { "type": "field", "domain": ["geosite:cn"], "outboundTag": "direct" },
                    { "type": "field", "ip": ["geoip:cn", "geoip:private"], "outboundTag": "direct" }
                ]
            }),
        }
    }
}

/// Parse a proxy share link (vmess://, vless://, trojan://, ss://)
pub fn parse_share_link(link: &str) -> Result<ServerConfig, String> {
    let link = link.trim();

    if link.starts_with("vmess://") {
        parse_vmess_link(link)
    } else if link.starts_with("vless://") {
        parse_vless_link(link)
    } else if link.starts_with("trojan://") {
        parse_trojan_link(link)
    } else if link.starts_with("ss://") {
        parse_ss_link(link)
    } else {
        Err(format!("Unsupported link format: {}", &link[..20.min(link.len())]))
    }
}

fn parse_vmess_link(link: &str) -> Result<ServerConfig, String> {
    let encoded = link[8..].replace(&['\n', '\r', ' ', '\t'][..], ""); // Remove "vmess://" and whitespace
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&encoded))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(&encoded))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&encoded))
        .map_err(|e| format!("Base64 decode failed: {}", e))?;

    let json_str = String::from_utf8(decoded).map_err(|e| format!("UTF-8 decode failed: {}", e))?;
    let v: Value = serde_json::from_str(&json_str).map_err(|e| format!("JSON parse failed: {}", e))?;

    Ok(ServerConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name: v["ps"].as_str().unwrap_or("VMess Node").to_string(),
        protocol: "vmess".to_string(),
        address: v["add"].as_str().unwrap_or("").to_string(),
        // Port can be a number OR a string ("443") in the wild
        port: v["port"]
            .as_u64()
            .or_else(|| v["port"].as_str().and_then(|s| s.parse::<u64>().ok()))
            .unwrap_or(443) as u16,
        uuid: v["id"].as_str().map(String::from),
        alter_id: v["aid"]
            .as_u64()
            .or_else(|| v["aid"].as_str().and_then(|s| s.parse::<u64>().ok()))
            .map(|v| v as u32),
        encryption: v["scy"].as_str().map(String::from),
        flow: None,
        network: v["net"].as_str().map(String::from),
        security: v["tls"].as_str().filter(|s| !s.is_empty()).map(String::from),
        sni: v["sni"].as_str().filter(|s| !s.is_empty()).map(String::from),
        path: v["path"].as_str().filter(|s| !s.is_empty()).map(String::from),
        host: v["host"].as_str().filter(|s| !s.is_empty()).map(String::from),
        password: None,
        reality_public_key: None,
        reality_short_id: None,
        fingerprint: None,
    })
}

fn parse_vless_link(link: &str) -> Result<ServerConfig, String> {
    // vless://uuid@address:port?params#name
    let url = Url::parse(link).map_err(|e| format!("URL parse failed: {}", e))?;
    let params: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();

    // Fragment (#name) is percent-encoded; the url crate decodes it for us
    let name = url
        .fragment()
        .map(|f| urlencoding::decode(f).unwrap_or_else(|_| f.into()).into_owned())
        .unwrap_or_else(|| "VLESS Node".to_string());

    Ok(ServerConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        protocol: "vless".to_string(),
        address: url.host_str().unwrap_or("").to_string(),
        port: url.port().unwrap_or(443),
        uuid: Some(url.username().to_string()),
        password: None,
        alter_id: None,
        encryption: params.get("encryption").cloned(),
        flow: params.get("flow").cloned(),
        network: params.get("type").cloned(),
        security: params.get("security").cloned(),
        sni: params.get("sni").cloned().or_else(|| params.get("servername").cloned()),
        path: params.get("path").cloned(),
        host: params.get("host").cloned(),
        reality_public_key: params.get("pbk").cloned(),
        reality_short_id: params.get("sid").cloned(),
        fingerprint: params.get("fp").cloned(),
    })
}

fn parse_trojan_link(link: &str) -> Result<ServerConfig, String> {
    // trojan://password@address:port?params#name
    let url = Url::parse(link).map_err(|e| format!("URL parse failed: {}", e))?;
    let params: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();

    let name = url
        .fragment()
        .map(|f| urlencoding::decode(f).unwrap_or_else(|_| f.into()).into_owned())
        .unwrap_or_else(|| "Trojan Node".to_string());

    Ok(ServerConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        protocol: "trojan".to_string(),
        address: url.host_str().unwrap_or("").to_string(),
        port: url.port().unwrap_or(443),
        uuid: None,
        password: Some(url.username().to_string()),
        alter_id: None,
        encryption: None,
        flow: None,
        network: params.get("type").cloned(),
        security: params.get("security").cloned().or(Some("tls".into())),
        sni: params.get("sni").cloned(),
        path: params.get("path").cloned(),
        host: params.get("host").cloned(),
        reality_public_key: None,
        reality_short_id: None,
        fingerprint: params.get("fp").cloned(),
    })
}

fn parse_ss_link(link: &str) -> Result<ServerConfig, String> {
    // ss://base64(method:password)@address:port#name
    // or ss://base64(method:password@address:port)#name
    let without_prefix = &link[5..]; // Remove "ss://"
    let (main_part, name) = if let Some(hash_pos) = without_prefix.rfind('#') {
        (&without_prefix[..hash_pos], urlencoding_decode(&without_prefix[hash_pos + 1..]))
    } else {
        (without_prefix, "Shadowsocks Node".to_string())
    };

    if let Some(at_pos) = main_part.rfind('@') {
        let encoded = &main_part[..at_pos];
        let server_part = &main_part[at_pos + 1..];

        let clean_encoded = encoded.replace(&['\n', '\r', ' ', '\t'][..], "");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&clean_encoded)
            .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&clean_encoded))
            .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(&clean_encoded))
            .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&clean_encoded))
            .map_err(|e| format!("Base64 decode failed: {}", e))?;
        let method_pass = String::from_utf8(decoded).map_err(|e| format!("UTF-8 error: {}", e))?;

        let (method, password) = method_pass.split_once(':')
            .ok_or("Invalid ss format: missing ':'".to_string())?;

        let (address, port) = parse_host_port(server_part)?;

        Ok(ServerConfig {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            protocol: "shadowsocks".to_string(),
            address,
            port,
            uuid: None,
            password: Some(password.to_string()),
            alter_id: None,
            encryption: Some(method.to_string()),
            flow: None,
            network: None,
            security: None,
            sni: None,
            path: None,
            host: None,
            reality_public_key: None,
            reality_short_id: None,
            fingerprint: None,
        })
    } else {
        Err("Invalid shadowsocks link format".to_string())
    }
}

fn parse_host_port(s: &str) -> Result<(String, u16), String> {
    if let Some(colon_pos) = s.rfind(':') {
        let host = s[..colon_pos].to_string();
        let port: u16 = s[colon_pos + 1..].parse().map_err(|e| format!("Invalid port: {}", e))?;
        Ok((host, port))
    } else {
        Err("Missing port in address".to_string())
    }
}

fn urlencoding_decode(s: &str) -> String {
    url::form_urlencoded::parse(s.as_bytes())
        .map(|(k, v)| if v.is_empty() { k.to_string() } else { format!("{}={}", k, v) })
        .collect::<Vec<_>>()
        .join("")
}

/// Parse a subscription content.
/// Handles:
/// 1. Base64-encoded list of share links (most common)
/// 2. Plain-text newline-separated share links
/// 3. Clash YAML (detected and rejected with clear error — caller handles)
pub fn parse_subscription(content: &str) -> Vec<ServerConfig> {
    let trimmed = content.trim();

    // ── Fast path: plain text links (starts with a known scheme) ──────────────
    let first_line = trimmed.lines().next().unwrap_or("").trim();
    let known_schemes = ["vmess://", "vless://", "trojan://", "ss://", "hy2://", "hysteria2://"];
    let looks_like_plain = known_schemes.iter().any(|s| first_line.starts_with(s));

    if looks_like_plain {
        return trimmed
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| parse_share_link(line.trim()).ok())
            .collect();
    }

    // ── Base64 decode path ────────────────────────────────────────────────────
    let clean_b64 = trimmed.replace(&['\n', '\r', ' ', '\t'][..], "");
    let decoded_opt = base64::engine::general_purpose::STANDARD
        .decode(&clean_b64)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&clean_b64))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(&clean_b64))
        .or_else(|_| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&clean_b64))
        .ok();

    if let Some(decoded_bytes) = decoded_opt {
        let text = String::from_utf8_lossy(&decoded_bytes);
        let decoded_trimmed = text.trim();

        // Check if the decoded result looks like share links
        let decoded_first = decoded_trimmed.lines().next().unwrap_or("").trim();
        let decoded_looks_like = known_schemes.iter().any(|s| decoded_first.starts_with(s));

        if decoded_looks_like {
            return decoded_trimmed
                .lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| parse_share_link(line.trim()).ok())
                .collect();
        }
    }

    // ── Last resort: try each raw line as-is ──────────────────────────────────
    trimmed
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| parse_share_link(line.trim()).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ================================================================
    // VMess Link Parsing
    // ================================================================

    #[test]
    fn test_vmess_basic() {
        let link = "vmess://eyJhZGQiOiIxLjIuMy40IiwiYWlkIjoiMCIsImhvc3QiOiIiLCJpZCI6IjAwMDAwMDAwLTAwMDAtMDAwMC0wMDAwLTAwMDAwMDAwMDAwMCIsIm5ldCI6InRjcCIsInBhdGgiOiIiLCJwb3J0IjoiNDQzIiwicHMiOiJ0ZXN0X3ZtZXNzIiwic2N5IjoiYXV0byIsInNuaSI6IiIsInRscyI6IiIsInR5cGUiOiJub25lIiwidiI6IjIifQ";
        let node = parse_share_link(link).unwrap();
        assert_eq!(node.protocol, "vmess");
        assert_eq!(node.name, "test_vmess");
        assert_eq!(node.address, "1.2.3.4");
        assert_eq!(node.port, 443);
        assert_eq!(node.uuid.as_deref().unwrap(), "00000000-0000-0000-0000-000000000000");
        assert_eq!(node.alter_id, Some(0));
    }

    #[test]
    fn test_vmess_with_chinese_name() {
        // Build a VMess JSON with Chinese name, port as string
        let json = serde_json::json!({
            "v": "2", "ps": "🇭🇰 香港节点 01",
            "add": "hk1.example.com", "port": "8443",
            "id": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            "aid": "0", "scy": "auto", "net": "ws",
            "type": "none", "host": "cdn.example.com",
            "path": "/ws", "tls": "tls", "sni": "cdn.example.com"
        });
        let encoded = base64::engine::general_purpose::STANDARD.encode(json.to_string());
        let link = format!("vmess://{}", encoded);

        let node = parse_share_link(&link).unwrap();
        assert_eq!(node.name, "🇭🇰 香港节点 01");
        assert_eq!(node.address, "hk1.example.com");
        assert_eq!(node.port, 8443); // String port "8443" → u16
        assert_eq!(node.network.as_deref(), Some("ws"));
        assert_eq!(node.security.as_deref(), Some("tls"));
        assert_eq!(node.path.as_deref(), Some("/ws"));
        assert_eq!(node.host.as_deref(), Some("cdn.example.com"));
        assert_eq!(node.sni.as_deref(), Some("cdn.example.com"));
    }

    #[test]
    fn test_vmess_invalid_base64() {
        let link = "vmess://not-valid-base64!!!";
        let result = parse_share_link(link);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Base64"));
    }

    // ================================================================
    // VLESS Link Parsing
    // ================================================================

    #[test]
    fn test_vless_basic_tls() {
        let link = "vless://00000000-0000-0000-0000-000000000000@1.2.3.4:443?encryption=none&security=tls&sni=example.com&type=tcp#test_vless";
        let node = parse_share_link(link).unwrap();
        assert_eq!(node.protocol, "vless");
        assert_eq!(node.name, "test_vless");
        assert_eq!(node.address, "1.2.3.4");
        assert_eq!(node.port, 443);
        assert_eq!(node.uuid.as_deref().unwrap(), "00000000-0000-0000-0000-000000000000");
        assert_eq!(node.security.as_deref(), Some("tls"));
        assert_eq!(node.sni.as_deref(), Some("example.com"));
    }

    #[test]
    fn test_vless_reality() {
        let link = "vless://uuid-test@reality.example.com:443?\
            encryption=none&flow=xtls-rprx-vision&security=reality&\
            sni=www.microsoft.com&fp=chrome&pbk=AAABBBCCCDDD&\
            sid=abcdef&type=tcp#REALITY节点";
        let node = parse_share_link(link).unwrap();
        assert_eq!(node.protocol, "vless");
        assert_eq!(node.name, "REALITY节点");
        assert_eq!(node.security.as_deref(), Some("reality"));
        assert_eq!(node.flow.as_deref(), Some("xtls-rprx-vision"));
        assert_eq!(node.fingerprint.as_deref(), Some("chrome"));
        assert_eq!(node.reality_public_key.as_deref(), Some("AAABBBCCCDDD"));
        assert_eq!(node.reality_short_id.as_deref(), Some("abcdef"));
    }

    #[test]
    fn test_vless_websocket() {
        let link = "vless://test-uuid@ws.example.com:443?encryption=none&security=tls&sni=ws.example.com&type=ws&host=ws.example.com&path=%2Fvless-ws#WS节点";
        let node = parse_share_link(link).unwrap();
        assert_eq!(node.network.as_deref(), Some("ws"));
        assert_eq!(node.path.as_deref(), Some("/vless-ws")); // URL-decoded
        assert_eq!(node.host.as_deref(), Some("ws.example.com"));
    }

    // ================================================================
    // Trojan Link Parsing
    // ================================================================

    #[test]
    fn test_trojan_basic() {
        let link = "trojan://my-password@trojan.example.com:443?security=tls&sni=trojan.example.com&fp=chrome#Trojan节点";
        let node = parse_share_link(link).unwrap();
        assert_eq!(node.protocol, "trojan");
        assert_eq!(node.name, "Trojan节点");
        assert_eq!(node.password.as_deref(), Some("my-password"));
        assert_eq!(node.address, "trojan.example.com");
        assert_eq!(node.port, 443);
        assert_eq!(node.security.as_deref(), Some("tls"));
        assert_eq!(node.fingerprint.as_deref(), Some("chrome"));
    }

    #[test]
    fn test_trojan_websocket() {
        let link = "trojan://pass123@ws-trojan.com:443?type=ws&security=tls&path=%2Ftrojan-ws&sni=ws-trojan.com#Trojan-WS";
        let node = parse_share_link(link).unwrap();
        assert_eq!(node.network.as_deref(), Some("ws"));
        assert_eq!(node.path.as_deref(), Some("/trojan-ws"));
    }

    // ================================================================
    // Shadowsocks Link Parsing
    // ================================================================

    #[test]
    fn test_ss_sip002_format() {
        // ss://base64(method:password)@address:port#name
        let method_pass = base64::engine::general_purpose::STANDARD.encode("aes-256-gcm:my-secret-password");
        let link = format!("ss://{}@ss.example.com:8388#SS节点", method_pass);
        let node = parse_share_link(&link).unwrap();
        assert_eq!(node.protocol, "shadowsocks");
        assert_eq!(node.name, "SS节点");
        assert_eq!(node.address, "ss.example.com");
        assert_eq!(node.port, 8388);
        assert_eq!(node.encryption.as_deref(), Some("aes-256-gcm"));
        assert_eq!(node.password.as_deref(), Some("my-secret-password"));
    }

    // ================================================================
    // Invalid / Unknown Links
    // ================================================================

    #[test]
    fn test_unsupported_protocol() {
        let link = "hy2://password@example.com:443#Hysteria2";
        let result = parse_share_link(link);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unsupported"));
    }

    #[test]
    fn test_empty_link() {
        let result = parse_share_link("");
        assert!(result.is_err());
    }

    // ================================================================
    // Subscription Parsing
    // ================================================================

    #[test]
    fn test_subscription_base64_decode() {
        let raw_vmess = "vmess://eyJhZGQiOiIxLjIuMy40IiwiYWlkIjoiMCIsImhvc3QiOiIiLCJpZCI6IjAwMDAwMDAwLTAwMDAtMDAwMC0wMDAwLTAwMDAwMDAwMDAwMCIsIm5ldCI6InRjcCIsInBhdGgiOiIiLCJwb3J0IjoiNDQzIiwicHMiOiJ0ZXN0X3ZtZXNzIiwic2N5IjoiYXV0byIsInNuaSI6IiIsInRscyI6IiIsInR5cGUiOiJub25lIiwidiI6IjIifQ";
        let sub_content = base64::engine::general_purpose::STANDARD_NO_PAD.encode(raw_vmess);

        let nodes = parse_subscription(&sub_content);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "test_vmess");
        assert_eq!(nodes[0].address, "1.2.3.4");
        assert_eq!(nodes[0].port, 443);
    }

    #[test]
    fn test_subscription_plaintext_multi_nodes() {
        let content = "\
vless://uuid1@a.com:443?encryption=none&security=tls&type=tcp#Node1\n\
vless://uuid2@b.com:443?encryption=none&security=tls&type=tcp#Node2\n\
vless://uuid3@c.com:443?encryption=none&security=tls&type=tcp#Node3";

        let nodes = parse_subscription(content);
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].name, "Node1");
        assert_eq!(nodes[1].name, "Node2");
        assert_eq!(nodes[2].name, "Node3");
    }

    #[test]
    fn test_subscription_empty_content() {
        let nodes = parse_subscription("");
        assert_eq!(nodes.len(), 0);
    }

    #[test]
    fn test_subscription_garbage_content() {
        let nodes = parse_subscription("this is not a subscription at all\nrandom text\n123");
        assert_eq!(nodes.len(), 0);
    }

    #[test]
    fn test_subscription_base64_multi_protocols() {
        let links = "\
vless://uuid@a.com:443?encryption=none&security=tls&type=tcp#VLESS-Node\n\
trojan://pass@b.com:443?security=tls#Trojan-Node";
        let encoded = base64::engine::general_purpose::STANDARD.encode(links);

        let nodes = parse_subscription(&encoded);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].protocol, "vless");
        assert_eq!(nodes[1].protocol, "trojan");
    }

    // ================================================================
    // Config Generation — to_v2ray_config
    // ================================================================

    fn make_test_server(protocol: &str) -> ServerConfig {
        ServerConfig {
            id: "test-id".to_string(),
            name: "Test Node".to_string(),
            protocol: protocol.to_string(),
            address: "1.2.3.4".to_string(),
            port: 443,
            uuid: Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string()),
            password: Some("test-password".to_string()),
            alter_id: Some(0),
            encryption: Some("none".to_string()),
            flow: None,
            network: Some("tcp".to_string()),
            security: Some("tls".to_string()),
            sni: Some("example.com".to_string()),
            path: None,
            host: None,
            reality_public_key: None,
            reality_short_id: None,
            fingerprint: Some("chrome".to_string()),
        }
    }

    #[test]
    fn test_config_vless_structure() {
        let server = make_test_server("vless");
        let config = server.to_v2ray_config(10808, 10809, "rule");

        // Has required top-level keys
        assert!(config.get("log").is_some());
        assert!(config.get("dns").is_some());
        assert!(config.get("routing").is_some());
        assert!(config.get("inbounds").is_some());
        assert!(config.get("outbounds").is_some());

        // Inbounds: HTTP + SOCKS
        let inbounds = config["inbounds"].as_array().unwrap();
        assert_eq!(inbounds.len(), 2);
        assert_eq!(inbounds[0]["protocol"], "http");
        assert_eq!(inbounds[0]["port"], 10808);
        assert_eq!(inbounds[1]["protocol"], "socks");
        assert_eq!(inbounds[1]["port"], 10809);

        // Outbounds: proxy + direct + block
        let outbounds = config["outbounds"].as_array().unwrap();
        assert_eq!(outbounds.len(), 3);
        assert_eq!(outbounds[0]["protocol"], "vless");
        assert_eq!(outbounds[0]["tag"], "proxy");
        assert_eq!(outbounds[1]["protocol"], "freedom");
        assert_eq!(outbounds[2]["protocol"], "blackhole");
    }

    #[test]
    fn test_config_vmess_structure() {
        let server = make_test_server("vmess");
        let config = server.to_v2ray_config(1080, 1081, "global");
        let outbound = &config["outbounds"][0];
        assert_eq!(outbound["protocol"], "vmess");
        let user = &outbound["settings"]["vnext"][0]["users"][0];
        assert_eq!(user["id"], "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
        assert_eq!(user["alterId"], 0);
    }

    #[test]
    fn test_config_trojan_structure() {
        let server = make_test_server("trojan");
        let config = server.to_v2ray_config(1080, 1081, "rule");
        let outbound = &config["outbounds"][0];
        assert_eq!(outbound["protocol"], "trojan");
        assert_eq!(outbound["settings"]["servers"][0]["password"], "test-password");
        assert_eq!(outbound["settings"]["servers"][0]["address"], "1.2.3.4");
    }

    #[test]
    fn test_config_shadowsocks_structure() {
        let server = make_test_server("shadowsocks");
        let config = server.to_v2ray_config(1080, 1081, "rule");
        let outbound = &config["outbounds"][0];
        assert_eq!(outbound["protocol"], "shadowsocks");
        assert_eq!(outbound["settings"]["servers"][0]["method"], "none");
        assert_eq!(outbound["settings"]["servers"][0]["password"], "test-password");
    }

    // ================================================================
    // Routing Modes
    // ================================================================

    #[test]
    fn test_routing_rule_mode() {
        let routing = ServerConfig::build_routing("rule");
        let rules = routing["rules"].as_array().unwrap();
        assert!(rules.len() >= 3); // ads block, cn direct, cn ip direct
        // Check ad blocking rule
        assert_eq!(rules[0]["outboundTag"], "block");
        // Check CN direct rules
        assert_eq!(rules[1]["outboundTag"], "direct");
    }

    #[test]
    fn test_routing_global_mode() {
        let routing = ServerConfig::build_routing("global");
        let rules = routing["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 0); // No rules = all traffic to proxy
    }

    #[test]
    fn test_routing_direct_mode() {
        let routing = ServerConfig::build_routing("direct");
        let rules = routing["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["outboundTag"], "direct");
    }

    // ================================================================
    // Stream Settings
    // ================================================================

    #[test]
    fn test_stream_settings_ws_tls() {
        let mut server = make_test_server("vless");
        server.network = Some("ws".to_string());
        server.security = Some("tls".to_string());
        server.path = Some("/ws-path".to_string());
        server.host = Some("cdn.example.com".to_string());

        let stream = server.build_stream_settings();
        assert_eq!(stream["network"], "ws");
        assert_eq!(stream["security"], "tls");
        assert_eq!(stream["wsSettings"]["path"], "/ws-path");
        assert_eq!(stream["wsSettings"]["headers"]["Host"], "cdn.example.com");
        assert!(stream["tlsSettings"]["serverName"].as_str().is_some());
    }

    #[test]
    fn test_stream_settings_grpc() {
        let mut server = make_test_server("vless");
        server.network = Some("grpc".to_string());
        server.path = Some("my-grpc-service".to_string());

        let stream = server.build_stream_settings();
        assert_eq!(stream["network"], "grpc");
        assert_eq!(stream["grpcSettings"]["serviceName"], "my-grpc-service");
    }

    #[test]
    fn test_stream_settings_reality() {
        let mut server = make_test_server("vless");
        server.security = Some("reality".to_string());
        server.reality_public_key = Some("test-public-key".to_string());
        server.reality_short_id = Some("abcd1234".to_string());

        let stream = server.build_stream_settings();
        assert_eq!(stream["security"], "reality");
        assert_eq!(stream["realitySettings"]["publicKey"], "test-public-key");
        assert_eq!(stream["realitySettings"]["shortId"], "abcd1234");
    }
}
