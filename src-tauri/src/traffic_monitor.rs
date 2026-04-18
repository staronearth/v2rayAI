use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficEvent {
    pub timestamp: u64,
    pub network: String,
    pub host: String,
    pub port: u16,
    pub route: String,
}

static TRAFFIC_REGEX: OnceLock<Regex> = OnceLock::new();

/// Parse a single Xray access log line and extract traffic event data.
/// Returns Some(TrafficEvent) if the line matches, None otherwise.
/// This is separated from `parse_and_emit_traffic` for testability.
pub fn parse_traffic_line(log_line: &str) -> Option<TrafficEvent> {
    if !log_line.contains("accepted") {
        return None;
    }

    let re = TRAFFIC_REGEX.get_or_init(|| {
        // Matches: accepted tcp:www.google.com:443 [proxy]
        // or: accepted tcp:1.1.1.1:443 [direct]
        Regex::new(r"accepted (tcp|udp):([^:\s]+):(\d+)\s+\[([^\]]+)\]").unwrap()
    });

    if let Some(captures) = re.captures(log_line) {
        let network = captures.get(1).map_or("", |m| m.as_str()).to_string();
        let host = captures.get(2).map_or("", |m| m.as_str()).to_string();
        let port = captures.get(3).and_then(|m| m.as_str().parse::<u16>().ok()).unwrap_or(0);
        let route = captures.get(4).map_or("", |m| m.as_str()).to_string();

        Some(TrafficEvent {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            network,
            host,
            port,
            route,
        })
    } else {
        None
    }
}

pub fn parse_and_emit_traffic(app: Option<&AppHandle>, log_line: &str) {
    if let Some(event) = parse_traffic_line(log_line) {
        if let Some(app) = app {
            // Emit to frontend (ignore errors if no listeners)
            let _ = app.emit("traffic-event", event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tcp_proxy() {
        let line = "2024/01/15 12:00:00 accepted tcp:www.google.com:443 [proxy]";
        let event = parse_traffic_line(line).unwrap();
        assert_eq!(event.network, "tcp");
        assert_eq!(event.host, "www.google.com");
        assert_eq!(event.port, 443);
        assert_eq!(event.route, "proxy");
    }

    #[test]
    fn test_parse_tcp_direct() {
        let line = "accepted tcp:cn.bing.com:443 [direct]";
        let event = parse_traffic_line(line).unwrap();
        assert_eq!(event.network, "tcp");
        assert_eq!(event.host, "cn.bing.com");
        assert_eq!(event.port, 443);
        assert_eq!(event.route, "direct");
    }

    #[test]
    fn test_parse_udp() {
        let line = "accepted udp:dns.google:53 [proxy]";
        let event = parse_traffic_line(line).unwrap();
        assert_eq!(event.network, "udp");
        assert_eq!(event.host, "dns.google");
        assert_eq!(event.port, 53);
    }

    #[test]
    fn test_parse_ip_address() {
        let line = "accepted tcp:1.1.1.1:443 [direct]";
        let event = parse_traffic_line(line).unwrap();
        assert_eq!(event.host, "1.1.1.1");
        assert_eq!(event.port, 443);
    }

    #[test]
    fn test_parse_block_route() {
        let line = "accepted tcp:ad.doubleclick.net:443 [block]";
        let event = parse_traffic_line(line).unwrap();
        assert_eq!(event.route, "block");
    }

    #[test]
    fn test_non_traffic_line_ignored() {
        let line = "2024/01/15 12:00:00 [Info] Xray core started";
        assert!(parse_traffic_line(line).is_none());
    }

    #[test]
    fn test_empty_line_ignored() {
        assert!(parse_traffic_line("").is_none());
    }

    #[test]
    fn test_timestamp_is_nonzero() {
        let line = "accepted tcp:test.com:80 [proxy]";
        let event = parse_traffic_line(line).unwrap();
        assert!(event.timestamp > 0);
    }
}

