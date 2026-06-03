/// Harness-style agentic loop for v2rayAI.
///
/// Turn model: user → [assistant + tool_results]* → final_assistant
/// Each iteration: AI responds → parse tool calls → execute → inject results → repeat.
use crate::ai_service::{AiService, ChatMessage};
use crate::config_generator::{parse_share_link, parse_subscription, ServerConfig};
use crate::sub_converter::SubConverterManager;
use crate::types::ToolSpec;
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

/// Run the agentic loop. Tries native function calling first and transparently
/// falls back to the text `[[TOOL:...]]` protocol when the endpoint rejects
/// `tools` (4xx) or the model answers in text-protocol form instead of emitting
/// native tool calls.
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
    match run_native(ai, base_url, api_key, model, thread.clone(), sub_converter).await {
        Ok(result) => Ok(result),
        Err(RunErr::Fallback(reason)) => {
            log::warn!("[Agent] Falling back to text protocol: {reason}");
            run_text(ai, base_url, api_key, model, thread, sub_converter).await
        }
        Err(RunErr::Fatal(e)) => Err(e),
    }
}

enum RunErr {
    /// Endpoint/model doesn't do native tools — retry with the text protocol.
    Fallback(String),
    /// A genuine error that retrying wouldn't fix.
    Fatal(String),
}

/// Native function-calling loop: advertise tools, consume `tool_calls`, and feed
/// each result back as a `tool` message keyed by its `tool_call_id`.
async fn run_native(
    ai: &AiService,
    base_url: &str,
    api_key: &str,
    model: &str,
    mut thread: Vec<ChatMessage>,
    sub_converter: &SubConverterManager,
) -> Result<AgentResult, RunErr> {
    let specs = tool_specs();
    let mut parsed_servers: Vec<ServerConfig> = Vec::new();

    for iteration in 0..=MAX_ITERATIONS {
        let completion = ai
            .complete_with_tools(base_url, api_key, model, &thread, Some(&specs))
            .await
            .map_err(|e| {
                if should_fallback_to_text(&e) {
                    RunErr::Fallback(e)
                } else {
                    RunErr::Fatal(e)
                }
            })?;

        let has_calls = completion.has_tool_calls();
        if !has_calls || iteration == MAX_ITERATIONS {
            if iteration == MAX_ITERATIONS && has_calls {
                log::warn!(
                    "[Agent] Reached max iterations ({}) with pending tool calls",
                    MAX_ITERATIONS
                );
            }
            // Model ignored native tools and answered in text-protocol form —
            // hand off to the text loop (only safe before any tool has run).
            if !has_calls
                && parsed_servers.is_empty()
                && !parse_tool_calls(&completion.content).is_empty()
            {
                return Err(RunErr::Fallback(
                    "model used text [[TOOL]] protocol".to_string(),
                ));
            }
            return Ok(AgentResult {
                message: strip_tool_markers(&completion.content),
                parsed_servers,
            });
        }

        let calls = completion.tool_calls.clone().unwrap_or_default();
        log::info!(
            "[Agent] (native) Iteration {}/{}: {} tool call(s)",
            iteration + 1,
            MAX_ITERATIONS,
            calls.len()
        );

        // Assistant turn carrying the native tool calls.
        thread.push(ChatMessage::assistant_tool_calls(
            completion.content.clone(),
            calls.clone(),
        ));

        // One `tool` reply per call, keyed by tool_call_id.
        for call in &calls {
            let name = call.function.name.as_str();
            if !ALLOWED_TOOLS.contains(&name) {
                log::warn!("[Agent] Blocked non-allowlisted tool: {}", name);
                thread.push(ChatMessage::tool(
                    call.id.clone(),
                    format!(
                        "策略拦截：该工具不在 allowlist 中。允许的工具：{}",
                        ALLOWED_TOOLS.join(", ")
                    ),
                ));
                continue;
            }
            let arg_text = native_args_to_text(name, &call.function.arguments);
            log::info!("[Agent] (native) Executing: {}", name);
            let exec = execute_tool(name, &arg_text, sub_converter).await;
            parsed_servers.extend(exec.servers);
            thread.push(ChatMessage::tool(call.id.clone(), exec.text));
        }
    }

    Ok(AgentResult {
        message: String::new(),
        parsed_servers,
    })
}

/// Text-protocol loop (fallback): the original `[[TOOL:...]]` regex protocol with
/// results injected as a single `user` turn. Used when the endpoint can't do
/// native function calling.
async fn run_text(
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
            "[Agent] (text) Iteration {}/{}: {} tool call(s)",
            iteration + 1,
            MAX_ITERATIONS,
            tool_calls.len()
        );

        // Assistant turn with the tool call text
        thread.push(ChatMessage::assistant(response));

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
            parsed_servers.extend(exec.servers);
            results_text.push_str(&format!(
                "\n[TOOL_RESULT: {name}]\n{}\n[/TOOL_RESULT]\n",
                exec.text
            ));
        }

        // Feed results back as a user turn so the next iteration sees them
        thread.push(ChatMessage::user(results_text));
    }

    // Should be unreachable but satisfy the compiler
    Ok(AgentResult {
        message: String::new(),
        parsed_servers,
    })
}

// ── Native-mode helpers ───────────────────────────────────────────────────────

/// JSON-Schema tool definitions advertised to the model in native mode. Mirrors
/// the text-protocol allowlist in `ALLOWED_TOOLS`.
fn tool_specs() -> Vec<ToolSpec> {
    let url_schema = serde_json::json!({
        "type": "object",
        "properties": { "url": { "type": "string", "description": "订阅链接 URL" } },
        "required": ["url"]
    });
    vec![
        ToolSpec::function(
            "fetch_subscription",
            "拉取并解析订阅链接，返回解析出的节点列表",
            url_schema.clone(),
        ),
        ToolSpec::function(
            "convert_subscription",
            "用 subconverter 转换 Clash 等格式的订阅链接",
            url_schema,
        ),
        ToolSpec::function(
            "subconverter_status",
            "查看本地 subconverter 的安装与运行状态",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        ToolSpec::function(
            "parse_proxy_links",
            "解析 vmess:// vless:// trojan:// ss:// 等代理分享链接",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "links": {
                        "type": "string",
                        "description": "一个或多个分享链接，使用空格或换行分隔"
                    }
                },
                "required": ["links"]
            }),
        ),
    ]
}

/// Map a native tool call's JSON `arguments` to the single string argument the
/// `execute_tool` dispatcher expects. Falls back to the raw (trimmed) string when
/// `arguments` isn't valid JSON, so a model that passes a bare value still works.
fn native_args_to_text(name: &str, arguments: &str) -> String {
    let value: serde_json::Value = match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(_) => return arguments.trim().to_string(),
    };
    let key = match name {
        "fetch_subscription" | "convert_subscription" => "url",
        "parse_proxy_links" => "links",
        _ => return String::new(),
    };
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Heuristic for the native→text fallback: a 4xx API error almost always means
/// the endpoint rejected the `tools` field. Network errors and 5xx are *not*
/// fallback-worthy — retrying in text mode wouldn't help.
fn should_fallback_to_text(err: &str) -> bool {
    err.contains("API error (4")
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

    // ── Native-mode helpers ─────────────────────────────────────────────────

    #[test]
    fn tool_specs_cover_the_allowlist() {
        let specs = tool_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.function.name.as_str()).collect();
        assert_eq!(specs.len(), ALLOWED_TOOLS.len());
        for t in ALLOWED_TOOLS {
            assert!(names.contains(t), "missing tool spec for {t}");
        }
    }

    #[test]
    fn tool_specs_have_object_schema() {
        for spec in tool_specs() {
            assert_eq!(spec.spec_type, "function");
            assert_eq!(spec.function.parameters["type"], "object");
        }
    }

    #[test]
    fn native_args_extracts_url_param() {
        assert_eq!(
            native_args_to_text("fetch_subscription", r#"{"url":"https://x.com/sub"}"#),
            "https://x.com/sub"
        );
        assert_eq!(
            native_args_to_text("convert_subscription", r#"{"url":"https://y.com"}"#),
            "https://y.com"
        );
    }

    #[test]
    fn native_args_extracts_links_param() {
        assert_eq!(
            native_args_to_text("parse_proxy_links", r#"{"links":"vmess://a\nvless://b"}"#),
            "vmess://a\nvless://b"
        );
    }

    #[test]
    fn native_args_status_tool_takes_no_args() {
        assert_eq!(native_args_to_text("subconverter_status", "{}"), "");
    }

    #[test]
    fn native_args_missing_key_yields_empty() {
        assert_eq!(native_args_to_text("fetch_subscription", "{}"), "");
    }

    #[test]
    fn native_args_non_json_falls_back_to_raw() {
        // Some models pass a bare value instead of a JSON object.
        assert_eq!(
            native_args_to_text("fetch_subscription", "https://raw.example.com"),
            "https://raw.example.com"
        );
    }

    #[test]
    fn fallback_triggers_on_4xx_only() {
        assert!(should_fallback_to_text("API error (400): 'tools' not supported"));
        assert!(should_fallback_to_text("API error (404): not found"));
        assert!(should_fallback_to_text("API error (422): unprocessable"));
        assert!(!should_fallback_to_text("API error (500): internal error"));
        assert!(!should_fallback_to_text("Request failed: connection refused"));
    }
}
