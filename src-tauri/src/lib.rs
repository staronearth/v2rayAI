mod ai_service;
mod chat_history;
mod config_generator;
mod config_manager;
mod core_manager;
mod health_monitor;
mod knowledge_base;
mod sub_converter;
mod sys_proxy;
mod app_logger;
mod traffic_monitor;

use ai_service::{AiService, ChatMessage};
use app_logger::LogEntry;
use chat_history::{
    Conversation, ConversationMeta,
    save_conversation, load_conversation,
    list_conversations, delete_conversation, search_conversations, auto_title,
    search_history_rag, format_history_rag_context,
};
use config_generator::{parse_share_link, parse_subscription, ServerConfig};
use config_manager::{ConfigManager, SavedConfig};
use core_manager::{CoreManager, fetch_latest_xray_release, download_xray, find_xray_core, resolve_or_download_core, CoreResolveResult};
use health_monitor::{HealthMonitor, LatencyResult, full_latency_test};
use knowledge_base::KnowledgeBase;
use regex::Regex;
use sub_converter::SubConverterManager;
use sys_proxy::{enable_system_proxy, disable_system_proxy, ProxySettings};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;
use tokio::sync::Mutex;

static APP_LOGGER: std::sync::OnceLock<&'static app_logger::AppLogger> = std::sync::OnceLock::new();

/// Application state shared across Tauri commands
pub struct AppState {
    pub ai_service: AiService,
    pub core_manager: Arc<CoreManager>,
    pub config_manager: ConfigManager,
    pub health_monitor: HealthMonitor,
    pub knowledge_base: Arc<Mutex<KnowledgeBase>>,
    pub sub_converter: Arc<SubConverterManager>,
}

// ============================================================
// App Logger Commands
// ============================================================

#[tauri::command]
fn get_app_logs(count: usize, level_filter: Option<String>) -> Vec<LogEntry> {
    if let Some(logger) = APP_LOGGER.get() {
        logger.get_logs(count, level_filter.as_deref())
    } else {
        vec![]
    }
}

#[tauri::command]
fn clear_app_logs() {
    if let Some(logger) = APP_LOGGER.get() {
        logger.clear();
    }
}

// ============================================================
// Chat History Commands
// ============================================================

#[tauri::command]
async fn save_chat(conv: Conversation) -> Result<(), String> {
    save_conversation(&conv).await
}

#[tauri::command]
async fn load_chat(id: String) -> Result<Conversation, String> {
    load_conversation(&id).await
}

#[tauri::command]
async fn list_chats() -> Result<Vec<ConversationMeta>, String> {
    list_conversations().await
}

#[tauri::command]
async fn delete_chat(id: String) -> Result<(), String> {
    delete_conversation(&id).await
}

#[tauri::command]
async fn search_chats(query: String) -> Result<Vec<ConversationMeta>, String> {
    search_conversations(&query).await
}

#[tauri::command]
fn generate_conv_title(messages: Vec<ChatMessage>) -> String {
    // Convert ai_service::ChatMessage to chat_history::ChatMessage for title gen
    let hist_msgs: Vec<chat_history::ChatMessage> = messages.iter().map(|m| {
        chat_history::ChatMessage {
            role: m.role.clone(),
            content: m.content.clone(),
            timestamp: 0,
        }
    }).collect();
    auto_title(&hist_msgs)
}

// ============================================================
// AI Chat Commands (with RAG + Tool Execution Loop)
// ============================================================

#[derive(Debug, Serialize, Deserialize)]
pub struct AiSettings {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConnectionContext {
    pub server_name: Option<String>,
    pub protocol: Option<String>,
    pub is_connected: bool,
    pub latency_ms: Option<u64>,
    pub routing_mode: Option<String>,
}

/// Result of a tool execution: text description for the AI + optional parsed servers.
struct ToolResult {
    text: String,
    servers: Vec<ServerConfig>,
}

/// Parse `[[TOOL:tool_name(args)]]` markers from AI response text.
/// Returns a list of (tool_name, args) tuples.
/// Uses `[\s\S]*?` to match args across newlines (e.g. proxy links) and allows empty args.
fn parse_tool_calls(text: &str) -> Vec<(String, String)> {
    let re = Regex::new(r"(?s)\[\[TOOL:(\w+)\((.*?)\)\]\]").unwrap();
    re.captures_iter(text)
        .map(|cap| (cap[1].to_string(), cap[2].to_string()))
        .collect()
}


/// Execute a single tool call and return text result + any parsed servers.
async fn execute_tool(
    tool_name: &str,
    args: &str,
    sub_converter: &SubConverterManager,
) -> ToolResult {
    match tool_name {
        "fetch_subscription" => {
            let url = args.trim();
            if url.is_empty() {
                return ToolResult { text: "错误：未提供订阅 URL".into(), servers: vec![] };
            }
            // Fetch subscription content — use no_proxy to bypass local proxy
            let client = match reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .user_agent("v2rayN/6.31")
                .no_proxy()
                .build()
            {
                Ok(c) => c,
                Err(e) => return ToolResult {
                    text: format!("错误：构建 HTTP 客户端失败：{}", e),
                    servers: vec![],
                },
            };

            let content = match client.get(url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    match resp.text().await {
                        Ok(t) => t,
                        Err(e) => return ToolResult {
                            text: format!("错误：读取订阅内容失败：{}", e),
                            servers: vec![],
                        },
                    }
                }
                Ok(resp) => return ToolResult {
                    text: format!("错误：订阅返回 HTTP {}。如果该订阅需要直连访问，请确保关闭代理后重试", resp.status()),
                    servers: vec![],
                },
                Err(e) => return ToolResult {
                    text: format!("错误：获取订阅失败：{}", e),
                    servers: vec![],
                },
            };

            let servers = parse_subscription(&content);
            if servers.is_empty() {
                // Try subconverter fallback
                match sub_converter.convert_subscription(url).await {
                    Ok(converted) => {
                        let converted_servers = parse_subscription(&converted);
                        if converted_servers.is_empty() {
                            ToolResult {
                                text: "获取成功但未解析出任何节点，内容可能不是标准代理订阅格式".into(),
                                servers: vec![],
                            }
                        } else {
                            let text = format_servers_result(&converted_servers);
                            ToolResult { text, servers: converted_servers }
                        }
                    }
                    Err(_) => ToolResult {
                        text: "内置解析器未能解析订阅内容，subconverter 未运行。建议用户先在工具箱页面启动 subconverter".into(),
                        servers: vec![],
                    },
                }
            } else {
                let text = format_servers_result(&servers);
                ToolResult { text, servers }
            }
        }

        "convert_subscription" => {
            let url = args.trim();
            if url.is_empty() {
                return ToolResult { text: "错误：未提供订阅 URL".into(), servers: vec![] };
            }
            match sub_converter.convert_subscription(url).await {
                Ok(converted) => {
                    let servers = parse_subscription(&converted);
                    if servers.is_empty() {
                        ToolResult {
                            text: "subconverter 转换完成但未解析出可用节点".into(),
                            servers: vec![],
                        }
                    } else {
                        let text = format_servers_result(&servers);
                        ToolResult { text, servers }
                    }
                }
                Err(e) => ToolResult {
                    text: format!("subconverter 转换失败：{}", e),
                    servers: vec![],
                },
            }
        }

        "subconverter_status" => {
            let status = sub_converter.status().await;
            ToolResult {
                text: format!(
                    "已安装: {} | 运行中: {} | 路径: {}",
                    if status.installed { "✅ 是" } else { "❌ 否" },
                    if status.running { "🟢 是" } else { "⏹ 否" },
                    status.path.unwrap_or_else(|| "无".to_string()),
                ),
                servers: vec![],
            }
        }

        "parse_proxy_links" => {
            let links_text = args.trim();
            if links_text.is_empty() {
                return ToolResult { text: "错误：未提供代理链接".into(), servers: vec![] };
            }
            let mut all_servers: Vec<ServerConfig> = Vec::new();
            let mut errors: Vec<String> = Vec::new();

            for link in links_text.split(|c: char| c == '\n' || c == ' ') {
                let link = link.trim();
                if link.is_empty() { continue; }
                match parse_share_link(link) {
                    Ok(server) => all_servers.push(server),
                    Err(e) => errors.push(format!("解析失败 `{}...`: {}",
                        &link[..link.len().min(30)], e)),
                }
            }

            let mut result = String::new();
            if !all_servers.is_empty() {
                result.push_str(&format_servers_result(&all_servers));
            }
            if !errors.is_empty() {
                if !result.is_empty() { result.push_str("\n"); }
                result.push_str(&format!("解析错误 ({}):\n{}",
                    errors.len(), errors.join("\n")));
            }
            if result.is_empty() {
                ToolResult { text: "未找到任何可解析的代理链接".into(), servers: vec![] }
            } else {
                let servers = all_servers;
                ToolResult { text: result, servers }
            }
        }

        _ => ToolResult { text: format!("未知工具：{}", tool_name), servers: vec![] },
    }
}

/// Format a list of parsed servers into a summary string for the AI context.
fn format_servers_result(servers: &[ServerConfig]) -> String {
    let mut result = format!("成功解析 {} 个节点：\n", servers.len());
    for (i, s) in servers.iter().enumerate() {
        result.push_str(&format!(
            "{}. [{}] {} — {}:{}\n",
            i + 1,
            s.protocol.to_uppercase(),
            s.name,
            s.address,
            s.port,
        ));
        if i >= 19 && servers.len() > 20 {
            result.push_str(&format!("... 还有 {} 个节点\n", servers.len() - 20));
            break;
        }
    }
    result
}

/// Strip tool call markers from the AI response text.
fn strip_tool_markers(text: &str) -> String {
    let re = Regex::new(r"\[\[TOOL:(\w+)\(.+?\)\]\]").unwrap();
    re.replace_all(text, "🔧 *正在调用工具 `$1`...*").to_string()
}

/// Structured response from chat_with_ai: AI text + any servers parsed by tools.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatResult {
    pub message: String,
    pub parsed_servers: Vec<ServerConfig>,
}

const MAX_TOOL_LOOP_ITERATIONS: usize = 5;

#[tauri::command]
async fn chat_with_ai(
    message: String,
    history: Vec<ChatMessage>,
    settings: AiSettings,
    context: Option<ConnectionContext>,
    state: State<'_, AppState>,
) -> Result<ChatResult, String> {
    // ── Dual RAG: run document search and history search concurrently ──
    let doc_rag_fut = async {
        let kb = state.knowledge_base.lock().await;
        kb.get_rag_context(&message, 4)
    };
    let hist_rag_fut = search_history_rag(&message, 3, 600);

    let (doc_rag, hist_chunks) = tokio::join!(doc_rag_fut, hist_rag_fut);
    let hist_rag = format_history_rag_context(&hist_chunks);

    // ── Build enriched message with clearly labelled RAG sections ──
    let mut enriched = message.clone();
    if !doc_rag.is_empty() {
        enriched.push_str(&doc_rag);
    }
    if !hist_rag.is_empty() {
        enriched.push_str(&hist_rag);
    }

    // ── Inject current connection state ──
    if let Some(ctx) = context {
        enriched.push_str(&format!(
            "\n\n---\n**[当前连接状态]** 已连接:{} | 节点:{} | 协议:{} | 延迟:{}ms | 路由:{}",
            ctx.is_connected,
            ctx.server_name.unwrap_or("无".into()),
            ctx.protocol.unwrap_or("无".into()),
            ctx.latency_ms.unwrap_or(0),
            ctx.routing_mode.unwrap_or("smart".into()),
        ));
    }

    // ── Tool Execution Loop ──────────────────────────────────────────────
    let sub_converter = state.sub_converter.clone();
    let mut loop_history = history.clone();
    let mut all_parsed_servers: Vec<ServerConfig> = Vec::new();

    // First call uses the enriched message
    let mut ai_response = state.ai_service
        .chat(&settings.base_url, &settings.api_key, &settings.model, &enriched, &loop_history)
        .await?;

    for iteration in 0..MAX_TOOL_LOOP_ITERATIONS {
        let tool_calls = parse_tool_calls(&ai_response);
        if tool_calls.is_empty() {
            break;
        }

        log::info!(
            "[Tool Loop] Iteration {}: {} tool call(s) detected",
            iteration + 1,
            tool_calls.len()
        );

        let mut tool_results = String::new();
        for (name, args) in &tool_calls {
            log::info!("[Tool Loop] Executing: {}({})", name, args);
            let result = execute_tool(name, args, &sub_converter).await;
            // Collect servers from tool results
            if !result.servers.is_empty() {
                all_parsed_servers.extend(result.servers);
            }
            tool_results.push_str(&format!(
                "\n[[TOOL_RESULT:{}]]\n{}\n[[/TOOL_RESULT]]\n",
                name, result.text
            ));
        }

        loop_history.push(ChatMessage {
            role: "assistant".to_string(),
            content: ai_response.clone(),
        });
        loop_history.push(ChatMessage {
            role: "user".to_string(),
            content: format!(
                "以下是工具执行结果，请根据结果给用户生成最终回答。不要再调用工具，直接回答用户。\n{}",
                tool_results
            ),
        });

        ai_response = state.ai_service
            .chat(
                &settings.base_url,
                &settings.api_key,
                &settings.model,
                "请根据上面的工具执行结果回答",
                &loop_history,
            )
            .await?;
    }

    let final_response = strip_tool_markers(&ai_response);

    Ok(ChatResult {
        message: final_response,
        parsed_servers: all_parsed_servers,
    })
}

// ============================================================
// History RAG Command (for frontend inspection / debugging)
// ============================================================

#[derive(Debug, Serialize)]
pub struct HistoryRagResult {
    pub conv_title: String,
    pub conv_id: String,
    pub snippet: String,
    pub score: f32,
}

#[tauri::command]
async fn search_history_knowledge(
    query: String,
    top_k: Option<usize>,
) -> Result<Vec<HistoryRagResult>, String> {
    let chunks = search_history_rag(&query, top_k.unwrap_or(5), 600).await;
    Ok(chunks
        .into_iter()
        .map(|c| HistoryRagResult {
            conv_title: c.conv_title,
            conv_id: c.conv_id,
            snippet: c.snippet,
            score: c.score,
        })
        .collect())
}

// ============================================================
// Knowledge Base Commands
// ============================================================

#[tauri::command]
async fn refresh_knowledge_base(
    xray_version: String,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let mut kb = state.knowledge_base.lock().await;
    kb.refresh_from_github(None).await?;
    kb.xray_version = xray_version;
    kb.save().await?;
    Ok(kb.chunks.len())
}

#[tauri::command]
async fn search_knowledge(
    query: String,
    top_k: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let kb = state.knowledge_base.lock().await;
    let chunks = kb.search(&query, top_k.unwrap_or(5));
    let results: Vec<serde_json::Value> = chunks.iter().map(|c| serde_json::json!({
        "id": c.id,
        "title": c.title,
        "source": c.source,
        "content": c.content,
    })).collect();
    Ok(results)
}

// ============================================================
// Config Generation & Management Commands
// ============================================================

#[tauri::command]
fn parse_proxy_link(link: String) -> Result<ServerConfig, String> {
    parse_share_link(&link)
}

#[tauri::command]
fn parse_subscription_content(content: String) -> Vec<ServerConfig> {
    parse_subscription(&content)
}

#[tauri::command]
fn generate_config(
    server: ServerConfig,
    http_port: u16,
    socks_port: u16,
    routing_mode: String,
) -> serde_json::Value {
    server.to_v2ray_config(http_port, socks_port, &routing_mode)
}

#[tauri::command]
async fn apply_config(
    server: ServerConfig,
    http_port: u16,
    socks_port: u16,
    routing_mode: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let config = server.to_v2ray_config(http_port, socks_port, &routing_mode);
    state.config_manager.write_active_config(&config).await
}

#[tauri::command]
async fn get_active_config_path(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.config_manager.active_config_path())
}

#[tauri::command]
async fn list_saved_configs(state: State<'_, AppState>) -> Result<Vec<SavedConfig>, String> {
    state.config_manager.list_configs().await
}

#[tauri::command]
async fn delete_saved_config(id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.config_manager.delete_config(&id).await
}

// ============================================================
// Core Management Commands
// ============================================================

#[tauri::command]
async fn start_core(
    core_path: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    // Use the active config file automatically
    let config_path = state.config_manager.active_config_path();
    if !std::path::Path::new(&config_path).exists() {
        return Err("请先连接一个节点以生成配置文件".to_string());
    }
    state.core_manager.start(&core_path, &config_path, Some(app)).await
}

#[tauri::command]
async fn stop_core(state: State<'_, AppState>) -> Result<String, String> {
    state.core_manager.stop().await
}

#[tauri::command]
async fn get_core_status(state: State<'_, AppState>) -> Result<core_manager::CoreStatus, String> {
    Ok(state.core_manager.status().await)
}

#[tauri::command]
async fn get_core_logs(count: usize, state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(state.core_manager.get_logs(count).await)
}

#[tauri::command]
async fn test_config(
    core_path: String,
    config_path: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    state.core_manager.test_config(&core_path, &config_path).await
}

#[tauri::command]
async fn get_core_version(
    core_path: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    state.core_manager.get_version(&core_path).await
}

#[tauri::command]
async fn fetch_latest_xray() -> Result<core_manager::XrayRelease, String> {
    fetch_latest_xray_release().await
}

#[tauri::command]
async fn download_xray_core(
    download_url: String,
    install_dir: String,
) -> Result<String, String> {
    download_xray(&download_url, &install_dir).await
}

#[tauri::command]
async fn auto_detect_core() -> Result<String, String> {
    find_xray_core().await
}

/// Smart command: find existing core first, download if not found.
/// Returns path + source ("existing" / "downloaded") + description.
#[tauri::command]
async fn resolve_core() -> Result<CoreResolveResult, String> {
    resolve_or_download_core().await
}

// ============================================================
// System Proxy Commands
// ============================================================

#[tauri::command]
async fn enable_proxy(http_port: u16, socks_port: u16) -> Result<String, String> {
    let settings = ProxySettings::local(http_port, socks_port);
    enable_system_proxy(&settings).await
}

#[tauri::command]
async fn disable_proxy() -> Result<String, String> {
    disable_system_proxy().await
}

// ============================================================
// Latency Test Commands
// ============================================================

#[tauri::command]
async fn test_latency(
    host: String,
    port: u16,
    http_proxy_port: Option<u16>,
) -> Result<LatencyResult, String> {
    if let Some(proxy_port) = http_proxy_port {
        Ok(full_latency_test(&host, port, proxy_port).await)
    } else {
        Ok(health_monitor::test_tcp_latency(&host, port, 5).await)
    }
}

#[tauri::command]
async fn start_health_monitor(
    http_proxy_port: u16,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.health_monitor.start(http_proxy_port, 30).await;
    Ok(())
}

#[tauri::command]
async fn stop_health_monitor(state: State<'_, AppState>) -> Result<(), String> {
    state.health_monitor.stop().await;
    Ok(())
}

// ============================================================
// Subscription Commands
// ============================================================

#[tauri::command]
async fn fetch_subscription(
    url: String,
    state: State<'_, AppState>,
) -> Result<Vec<ServerConfig>, String> {
    // Use a widely recognized v2ray client User-Agent so proxy panels return
    // base64 configuration instead of a dashboard HTML page or Clash YAML.
    // no_proxy() ensures we don't route through the local proxy we set up.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("v2rayN/6.31")
        .no_proxy()
        .build()
        .map_err(|e| format!("构建请求客户端失败: {}", e))?;

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("获取订阅失败：{}", e))?;

    if !response.status().is_success() {
        return Err(format!("订阅服务器返回 HTTP {}，请检查链接是否正确", response.status()));
    }

    let content = response
        .text()
        .await
        .map_err(|e| format!("读取订阅内容失败：{}", e))?;

    // ── Try native parser first ────────────────────────────────────────────
    let servers = parse_subscription(&content);
    if !servers.is_empty() {
        return Ok(servers);
    }

    // ── Native parse failed → try subconverter fallback ────────────────────
    log::info!("内置解析器未能解析订阅，尝试 subconverter 转换...");
    match state.sub_converter.convert_subscription(&url).await {
        Ok(converted_text) => {
            let converted_servers = parse_subscription(&converted_text);
            if !converted_servers.is_empty() {
                log::info!("subconverter 成功转换 {} 个节点", converted_servers.len());
                return Ok(converted_servers);
            }
            // subconverter returned something but we still couldn't parse it
            let preview = if converted_text.len() > 100 {
                format!("{}...", &converted_text[..100])
            } else {
                converted_text.clone()
            };
            Err(format!(
                "subconverter 已转换但仍无法解析节点。\n转换结果前缀: {}",
                preview.replace('\n', " ")
            ))
        }
        Err(converter_err) => {
            // subconverter not available → give user a clear diagnosis
            let preview = if content.len() > 100 {
                format!("{}...", &content[..100])
            } else {
                content.clone()
            };
            let is_clash = content.trim_start().starts_with("proxies:")
                || content.trim_start().starts_with("proxy-groups:")
                || (content.contains("proxies:") && content.contains("server:"));

            if is_clash {
                Err(format!(
                    "检测到 Clash 格式订阅，内置解析器不支持此格式。\n请在「设置 → 订阅转换工具」中安装并启动 subconverter，即可自动转换。\n\nsubconverter 状态：{}",
                    converter_err
                ))
            } else {
                Err(format!(
                    "订阅解析结果为空，未发现支持的节点格式。\n获取到的内容前缀: {}\n\n如果是 Clash/Surge 专属格式，请安装订阅转换工具。\nsubconverter 状态：{}",
                    preview.replace('\n', " "),
                    converter_err
                ))
            }
        }
    }
}

// ============================================================
// SubConverter Commands
// ============================================================

#[tauri::command]
async fn install_subconverter(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    state.sub_converter.install(&app).await
}

#[tauri::command]
async fn start_subconverter(
    state: State<'_, AppState>,
) -> Result<String, String> {
    state.sub_converter.start().await
}

#[tauri::command]
async fn stop_subconverter(
    state: State<'_, AppState>,
) -> Result<String, String> {
    state.sub_converter.stop().await
}

#[tauri::command]
async fn get_subconverter_status(
    state: State<'_, AppState>,
) -> Result<sub_converter::SubConverterStatus, String> {
    Ok(state.sub_converter.status().await)
}

#[tauri::command]
async fn convert_subscription_via_tool(
    url: String,
    state: State<'_, AppState>,
) -> Result<Vec<ServerConfig>, String> {
    let converted = state.sub_converter.convert_subscription(&url).await?;
    let servers = parse_subscription(&converted);
    if servers.is_empty() {
        return Err("subconverter 转换完成但未解析出节点".to_string());
    }
    Ok(servers)
}

// ============================================================
// App Entry Point
// ============================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize logger
    let logger = app_logger::AppLogger::init();
    APP_LOGGER.set(logger).ok();

    // Build initial knowledge base synchronously using blocking runtime
    let kb = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async { KnowledgeBase::load_or_create("unknown").await });

    let app_state = AppState {
        ai_service: AiService::new(),
        core_manager: Arc::new(CoreManager::new()),
        config_manager: ConfigManager::new(),
        health_monitor: HealthMonitor::new(),
        knowledge_base: Arc::new(Mutex::new(kb)),
        sub_converter: Arc::new(SubConverterManager::new()),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            // Logs
            get_app_logs,
            clear_app_logs,
            // History
            save_chat,
            load_chat,
            list_chats,
            delete_chat,
            search_chats,
            generate_conv_title,
            // AI + RAG
            chat_with_ai,
            search_history_knowledge,
            refresh_knowledge_base,
            search_knowledge,
            // Config
            parse_proxy_link,
            parse_subscription_content,
            generate_config,
            apply_config,
            get_active_config_path,
            list_saved_configs,
            delete_saved_config,
            // Core
            start_core,
            stop_core,
            get_core_status,
            get_core_logs,
            test_config,
            get_core_version,
            fetch_latest_xray,
            download_xray_core,
            auto_detect_core,
            resolve_core,
            // System Proxy
            enable_proxy,
            disable_proxy,
            // Latency & Health
            test_latency,
            start_health_monitor,
            stop_health_monitor,
            // Subscription
            fetch_subscription,
            // SubConverter
            install_subconverter,
            start_subconverter,
            stop_subconverter,
            get_subconverter_status,
            convert_subscription_via_tool,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    // ================================================================
    // parse_tool_calls
    // ================================================================

    #[test]
    fn test_parse_tool_calls_single() {
        let text = "我来帮你获取订阅\n[[TOOL:fetch_subscription(https://example.com/sub)]]";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "fetch_subscription");
        assert_eq!(calls[0].1, "https://example.com/sub");
    }

    #[test]
    fn test_parse_tool_calls_multiple() {
        let text = "\
[[TOOL:fetch_subscription(https://a.com/sub)]]\n\
一些中间文本\n\
[[TOOL:subconverter_status()]]";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "fetch_subscription");
        assert_eq!(calls[1].0, "subconverter_status");
    }

    #[test]
    fn test_parse_tool_calls_none() {
        let text = "这是一段普通回复，没有任何工具调用。";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 0);
    }

    #[test]
    fn test_parse_tool_calls_url_with_query_params() {
        let text = "[[TOOL:convert_subscription(https://sub.example.com/api/v1/client/subscribe?token=abc123&flag=v2ray)]]";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "convert_subscription");
        assert!(calls[0].1.contains("token=abc123"));
    }

    #[test]
    fn test_parse_tool_calls_proxy_links_multiline() {
        let text = "[[TOOL:parse_proxy_links(vmess://abc123\nvless://def456)]]";
        let calls = parse_tool_calls(text);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "parse_proxy_links");
        assert!(calls[0].1.contains("vmess://"));
    }

    #[test]
    fn test_parse_tool_calls_empty_args() {
        let text = "[[TOOL:subconverter_status()]]";
        let calls = parse_tool_calls(text);
        // After regex fix: `.*?` matches empty args too
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "subconverter_status");
        assert_eq!(calls[0].1, "");
    }


    // ================================================================
    // format_servers_result
    // ================================================================

    #[test]
    fn test_format_servers_result_empty() {
        let result = format_servers_result(&[]);
        assert!(result.contains("0 个节点"));
    }

    #[test]
    fn test_format_servers_result_single() {
        let servers = vec![ServerConfig {
            id: "1".into(),
            name: "HK-Node".into(),
            protocol: "vless".into(),
            address: "hk.example.com".into(),
            port: 443,
            uuid: None, password: None, alter_id: None, encryption: None,
            flow: None, network: None, security: None, sni: None,
            path: None, host: None, reality_public_key: None,
            reality_short_id: None, fingerprint: None,
        }];
        let result = format_servers_result(&servers);
        assert!(result.contains("1 个节点"));
        assert!(result.contains("VLESS"));
        assert!(result.contains("HK-Node"));
        assert!(result.contains("hk.example.com:443"));
    }

    #[test]
    fn test_format_servers_result_truncates_at_20() {
        let servers: Vec<ServerConfig> = (0..25)
            .map(|i| ServerConfig {
                id: format!("{}", i),
                name: format!("Node-{}", i),
                protocol: "vmess".into(),
                address: format!("{}.example.com", i),
                port: 443,
                uuid: None, password: None, alter_id: None, encryption: None,
                flow: None, network: None, security: None, sni: None,
                path: None, host: None, reality_public_key: None,
                reality_short_id: None, fingerprint: None,
            })
            .collect();

        let result = format_servers_result(&servers);
        assert!(result.contains("25 个节点"));
        assert!(result.contains("还有 5 个节点")); // 25 - 20 = 5
    }

    // ================================================================
    // strip_tool_markers
    // ================================================================

    #[test]
    fn test_strip_tool_markers_single() {
        let text = "我来帮你获取\n[[TOOL:fetch_subscription(https://example.com)]]\n等一下";
        let result = strip_tool_markers(text);
        assert!(!result.contains("[[TOOL:"));
        assert!(result.contains("🔧"));
        assert!(result.contains("fetch_subscription"));
    }

    #[test]
    fn test_strip_tool_markers_multiple() {
        let text = "[[TOOL:fetch_subscription(url1)]]\n文本\n[[TOOL:convert_subscription(url2)]]";
        let result = strip_tool_markers(text);
        assert!(!result.contains("[[TOOL:"));
        // Both should be replaced
        assert!(result.contains("fetch_subscription"));
        assert!(result.contains("convert_subscription"));
    }

    #[test]
    fn test_strip_tool_markers_no_markers() {
        let text = "这是一段完全正常的回复，没有工具标记。";
        let result = strip_tool_markers(text);
        assert_eq!(result, text);
    }

    #[test]
    fn test_strip_tool_markers_preserves_surrounding_text() {
        let text = "开始文本\n[[TOOL:subconverter_status(check)]]\n结束文本";
        let result = strip_tool_markers(text);
        assert!(result.contains("开始文本"));
        assert!(result.contains("结束文本"));
    }
}

