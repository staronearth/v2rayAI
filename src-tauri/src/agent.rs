/// Harness-style agentic loop for v2rayAI.
///
/// Turn model: user → [assistant + tool_results]* → final_assistant
/// Each iteration: AI responds → parse tool calls → execute → inject results → repeat.
use crate::ai_service::{AiService, ChatMessage};
use crate::config_generator::{parse_share_link, parse_subscription, ServerConfig};
use crate::sub_converter::SubConverterManager;
use regex::Regex;
use serde::Serialize;

// ── Constants ─────────────────────────────────────────────────────────────────

pub const MAX_ITERATIONS: usize = 5;

pub const ALLOWED_TOOLS: &[&str] = &[
    "fetch_subscription",
    "convert_subscription",
    "subconverter_status",
    "parse_proxy_links",
];

// ── Public Types ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentResult {
    pub message: String,
    pub parsed_servers: Vec<ServerConfig>,
}

// ── Agent Entry Point ─────────────────────────────────────────────────────────

/// Run the agentic loop: call AI, dispatch tool calls, feed results back, repeat
/// until no more tool calls or MAX_ITERATIONS is reached.
///
/// `thread` is the full conversation history including the current user turn.
pub async fn run(
    ai: &AiService,
    base_url: &str,
    api_key: &str,
    model: &str,
    thread: Vec<ChatMessage>,
    sub_converter: &SubConverterManager,
) -> Result<AgentResult, String> {
    let mut thread = thread;
    let mut parsed_servers: Vec<ServerConfig> = Vec::new();

    for iteration in 0..=MAX_ITERATIONS {
        let response = ai.complete(base_url, api_key, model, &thread).await?;
        let tool_calls = parse_tool_calls(&response);

        if tool_calls.is_empty() || iteration == MAX_ITERATIONS {
            if iteration == MAX_ITERATIONS && !tool_calls.is_empty() {
                log::warn!(
                    "[Agent] Reached max iterations ({}) with pending tool calls",
                    MAX_ITERATIONS
                );
            }
            return Ok(AgentResult {
                message: strip_tool_markers(&response),
                parsed_servers,
            });
        }

        log::info!(
            "[Agent] Iteration {}/{}: {} tool call(s)",
            iteration + 1,
            MAX_ITERATIONS,
            tool_calls.len()
        );

        // Assistant turn with the tool call text
        thread.push(ChatMessage {
            role: "assistant".to_string(),
            content: response,
        });

        // Execute each tool and collect results into a single user turn
        let mut results_text =
            String::from("工具执行结果如下，请根据结果直接回答用户（不要再调用工具）：\n");

        for (name, args) in &tool_calls {
            if !ALLOWED_TOOLS.contains(&name.as_str()) {
                log::warn!("[Agent] Blocked non-allowlisted tool: {}", name);
                results_text.push_str(&format!(
                    "\n[TOOL_RESULT: {name}]\n策略拦截：该工具不在 allowlist 中。允许的工具：{}\n[/TOOL_RESULT]\n",
                    ALLOWED_TOOLS.join(", ")
                ));
                continue;
            }

            log::info!("[Agent] Executing: {}({} chars of args)", name, args.len());
            let exec = execute_tool(name, args, sub_converter).await;
            let server_count = exec.servers.len();
            parsed_servers.extend(exec.servers);

            log::info!(
                "[Agent] Tool {} done, {} server(s) parsed",
                name,
                server_count
            );
            results_text.push_str(&format!(
                "\n[TOOL_RESULT: {name}]\n{}\n[/TOOL_RESULT]\n",
                exec.text
            ));
        }

        // Feed results back as a user turn so the next iteration sees them
        thread.push(ChatMessage {
            role: "user".to_string(),
            content: results_text,
        });
    }

    // Should be unreachable but satisfy the compiler
    Ok(AgentResult {
        message: String::new(),
        parsed_servers,
    })
}

// ── Tool Execution ────────────────────────────────────────────────────────────

struct ExecResult {
    text: String,
    servers: Vec<ServerConfig>,
}

async fn execute_tool(name: &str, args: &str, sub_converter: &SubConverterManager) -> ExecResult {
    match name {
        "fetch_subscription" => fetch_subscription(args, sub_converter).await,
        "convert_subscription" => convert_subscription(args, sub_converter).await,
        "subconverter_status" => subconverter_status(sub_converter).await,
        "parse_proxy_links" => parse_proxy_links(args),
        _ => ExecResult {
            text: format!("未知工具：{name}"),
            servers: vec![],
        },
    }
}

async fn fetch_subscription(url: &str, sub_converter: &SubConverterManager) -> ExecResult {
    let url = url.trim();
    if url.is_empty() {
        return ExecResult {
            text: "错误：未提供订阅 URL".into(),
            servers: vec![],
        };
    }

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("v2rayN/6.31")
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return ExecResult {
                text: format!("错误：构建 HTTP 客户端失败：{e}"),
                servers: vec![],
            }
        }
    };

    let content = match client.get(url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                return ExecResult {
                    text: format!("错误：读取订阅内容失败：{e}"),
                    servers: vec![],
                }
            }
        },
        Ok(resp) => {
            return ExecResult {
                text: format!(
                    "错误：订阅返回 HTTP {}。若需直连访问，请确保关闭代理后重试",
                    resp.status()
                ),
                servers: vec![],
            }
        }
        Err(e) => {
            return ExecResult {
                text: format!("错误：获取订阅失败：{e}"),
                servers: vec![],
            }
        }
    };

    let servers = parse_subscription(&content);
    if !servers.is_empty() {
        return ExecResult {
            text: format_servers_summary(&servers),
            servers,
        };
    }

    // Fallback to subconverter
    match sub_converter.convert_subscription(url).await {
        Ok(converted) => {
            let servers = parse_subscription(&converted);
            if servers.is_empty() {
                ExecResult {
                    text: "获取成功但未解析出任何节点，内容可能不是标准代理订阅格式".into(),
                    servers: vec![],
                }
            } else {
                ExecResult { text: format_servers_summary(&servers), servers }
            }
        }
        Err(_) => ExecResult {
            text: "内置解析器未能解析订阅，subconverter 未运行。请在工具箱页面启动 subconverter 后重试".into(),
            servers: vec![],
        },
    }
}

async fn convert_subscription(url: &str, sub_converter: &SubConverterManager) -> ExecResult {
    let url = url.trim();
    if url.is_empty() {
        return ExecResult {
            text: "错误：未提供订阅 URL".into(),
            servers: vec![],
        };
    }
    match sub_converter.convert_subscription(url).await {
        Ok(converted) => {
            let servers = parse_subscription(&converted);
            if servers.is_empty() {
                ExecResult {
                    text: "subconverter 转换完成但未解析出可用节点".into(),
                    servers: vec![],
                }
            } else {
                ExecResult {
                    text: format_servers_summary(&servers),
                    servers,
                }
            }
        }
        Err(e) => ExecResult {
            text: format!("subconverter 转换失败：{e}"),
            servers: vec![],
        },
    }
}

async fn subconverter_status(sub_converter: &SubConverterManager) -> ExecResult {
    let status = sub_converter.status().await;
    ExecResult {
        text: format!(
            "已安装: {} | 运行中: {} | 路径: {}",
            if status.installed {
                "✅ 是"
            } else {
                "❌ 否"
            },
            if status.running {
                "🟢 是"
            } else {
                "⏹ 否"
            },
            status.path.unwrap_or_else(|| "无".to_string()),
        ),
        servers: vec![],
    }
}

fn parse_proxy_links(links_text: &str) -> ExecResult {
    let links_text = links_text.trim();
    if links_text.is_empty() {
        return ExecResult {
            text: "错误：未提供代理链接".into(),
            servers: vec![],
        };
    }

    let mut servers: Vec<ServerConfig> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for link in links_text.split(|c: char| c == '\n' || c == ' ') {
        let link = link.trim();
        if link.is_empty() {
            continue;
        }
        match parse_share_link(link) {
            Ok(s) => servers.push(s),
            Err(e) => errors.push(format!(
                "解析失败 `{}...`: {e}",
                &link[..link.len().min(30)]
            )),
        }
    }

    let mut text = String::new();
    if !servers.is_empty() {
        text.push_str(&format_servers_summary(&servers));
    }
    if !errors.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!(
            "解析错误 ({}):\n{}",
            errors.len(),
            errors.join("\n")
        ));
    }
    if text.is_empty() {
        ExecResult {
            text: "未找到任何可解析的代理链接".into(),
            servers: vec![],
        }
    } else {
        ExecResult { text, servers }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parse `[[TOOL:tool_name(args)]]` markers from model output.
pub fn parse_tool_calls(text: &str) -> Vec<(String, String)> {
    let re = Regex::new(r"(?s)\[\[TOOL:(\w+)\((.*?)\)\]\]").unwrap();
    re.captures_iter(text)
        .map(|cap| (cap[1].to_string(), cap[2].to_string()))
        .collect()
}

/// Remove any leftover `[[TOOL:...]]` markers from the final response text.
pub fn strip_tool_markers(text: &str) -> String {
    let re = Regex::new(r"(?s)\[\[TOOL:\w+\(.*?\)\]\]").unwrap();
    re.replace_all(text, "").to_string()
}

/// Compact server list for injection into the AI context.
pub fn format_servers_summary(servers: &[ServerConfig]) -> String {
    let mut out = format!("成功解析 {} 个节点：\n", servers.len());
    for (i, s) in servers.iter().enumerate() {
        out.push_str(&format!(
            "{}. [{}] {} — {}:{}\n",
            i + 1,
            s.protocol.to_uppercase(),
            s.name,
            s.address,
            s.port,
        ));
        if i >= 19 && servers.len() > 20 {
            out.push_str(&format!("... 还有 {} 个节点\n", servers.len() - 20));
            break;
        }
    }
    out
}

// ── System Prompt ─────────────────────────────────────────────────────────────

pub fn system_prompt() -> &'static str {
    r#"你是 v2rayAI —— 精通 Xray-core 与 V2Ray 内核的**网络安全配置专家**。

# Agent 行为规范

你是一个受控的本地配置 agent，行为必须满足：

- **上下文优先**：优先使用 `[本地环境快照]`、`[RAG 文档]`、`[历史 RAG]` 等注入内容；无法确认的信息标为假设。
- **工具受限**：只能调用当前 allowlist 中的工具，不能编造工具名或执行系统命令。
- **可审计**：需要调用工具时说明目的；工具结果回来后基于结果给出结论。
- **人类确认**：解析结果只能建议用户添加，不能声称已应用或修改系统设置。
- **最小权限**：不索要不必要的 API Key、私钥；敏感字段用占位符。

## 工具调用格式

每个工具调用独占一行，严格格式：
```
[[TOOL:工具名(参数)]]
```

## 当前 allowlist

| 工具 | 用途 |
|------|------|
| `fetch_subscription(url)` | 拉取并解析订阅链接 |
| `convert_subscription(url)` | 用 subconverter 转换 Clash 等格式订阅 |
| `subconverter_status()` | 查看 subconverter 状态 |
| `parse_proxy_links(links)` | 解析 vmess:// vless:// trojan:// ss:// 分享链接 |

## 前端动作

仅当用户**明确要求删除全部节点**时，在回复末尾输出：
```
[[APP_ACTION:clear_all_servers]]
```
这只会让前端显示二次确认按钮，不代表已执行删除。

---

# 专业领域

- **协议**：VLESS、VMess、Trojan、Shadowsocks（含 2022）、Hysteria2
- **传输**：TCP、WebSocket、gRPC、HTTPUpgrade、SplitHTTP、QUIC
- **安全**：REALITY（无证书 TLS 伪装）、TLS 1.2/1.3、XTLS Vision
- **路由**：GeoSite/GeoIP、domainStrategy、智能分流
- **DNS**：DoH、DoT、国内外 DNS 分离、expectIPs

---

# 回答规范

1. **配置生成**：给出包含 log/dns/routing/inbounds/outbounds 的完整 JSON（用 ```json 包裹）
2. **真实参数**：使用用户提供的值；未提供的用 `YOUR_XXX` 占位
3. **诊断优先**：先给诊断结论（1~2句话），再给修复步骤
4. **安全**：inbound 必须绑定 `127.0.0.1`，防止开放代理
5. **语言**：中文为主，技术术语保留英文
6. **工具优先**：需要导入订阅、解析链接等操作时，直接调用工具而非只给文字步骤"#
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_generator::ServerConfig;

    #[test]
    fn parse_single_tool_call() {
        let text = "我来帮你\n[[TOOL:fetch_subscription(https://example.com/sub)]]";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "fetch_subscription");
        assert_eq!(calls[0].1, "https://example.com/sub");
    }

    #[test]
    fn parse_multiple_tool_calls() {
        let text =
            "[[TOOL:fetch_subscription(https://a.com)]]\n文本\n[[TOOL:subconverter_status()]]";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "fetch_subscription");
        assert_eq!(calls[1].0, "subconverter_status");
    }

    #[test]
    fn parse_no_tool_calls() {
        assert!(parse_tool_calls("这是普通回复").is_empty());
    }

    #[test]
    fn parse_url_with_query_params() {
        let text =
            "[[TOOL:convert_subscription(https://sub.example.com/api?token=abc123&flag=v2ray)]]";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1.contains("token=abc123"));
    }

    #[test]
    fn parse_multiline_proxy_links() {
        let text = "[[TOOL:parse_proxy_links(vmess://abc123\nvless://def456)]]";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert!(calls[0].1.contains("vmess://"));
    }

    #[test]
    fn parse_empty_args() {
        let calls = parse_tool_calls("[[TOOL:subconverter_status()]]");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "");
    }

    #[test]
    fn strip_leaves_no_markers() {
        let text = "前\n[[TOOL:fetch_subscription(https://x.com)]]\n后";
        let result = strip_tool_markers(text);
        assert!(!result.contains("[[TOOL:"));
        assert!(result.contains("前"));
        assert!(result.contains("后"));
    }

    #[test]
    fn strip_no_op_when_no_markers() {
        let text = "普通回复";
        assert_eq!(strip_tool_markers(text), text);
    }

    #[test]
    fn format_summary_empty() {
        assert!(format_servers_summary(&[]).contains("0 个节点"));
    }

    #[test]
    fn format_summary_single() {
        let servers = vec![ServerConfig {
            id: "1".into(),
            name: "HK".into(),
            protocol: "vless".into(),
            address: "hk.example.com".into(),
            port: 443,
            uuid: None,
            password: None,
            alter_id: None,
            encryption: None,
            flow: None,
            network: None,
            security: None,
            sni: None,
            path: None,
            host: None,
            reality_public_key: None,
            reality_short_id: None,
            fingerprint: None,
            allow_insecure: None,
        }];
        let result = format_servers_summary(&servers);
        assert!(result.contains("1 个节点"));
        assert!(result.contains("VLESS"));
        assert!(result.contains("hk.example.com:443"));
    }

    #[test]
    fn format_summary_truncates_at_20() {
        let servers: Vec<ServerConfig> = (0..25)
            .map(|i| ServerConfig {
                id: i.to_string(),
                name: format!("Node-{i}"),
                protocol: "vmess".into(),
                address: format!("{i}.example.com"),
                port: 443,
                uuid: None,
                password: None,
                alter_id: None,
                encryption: None,
                flow: None,
                network: None,
                security: None,
                sni: None,
                path: None,
                host: None,
                reality_public_key: None,
                reality_short_id: None,
                fingerprint: None,
                allow_insecure: None,
            })
            .collect();
        let result = format_servers_summary(&servers);
        assert!(result.contains("25 个节点"));
        assert!(result.contains("还有 5 个节点"));
    }

    #[test]
    fn allowed_tools_are_recognized() {
        for tool in ALLOWED_TOOLS {
            assert!(ALLOWED_TOOLS.contains(tool));
        }
        assert!(!ALLOWED_TOOLS.contains(&"start_core"));
        assert!(!ALLOWED_TOOLS.contains(&"shell"));
    }
}
