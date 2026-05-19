mod agent;
mod ai_service;
mod app_logger;
mod chat_history;
mod config_generator;
mod config_manager;
mod core_manager;
mod health_monitor;
mod knowledge_base;
mod secure_store;
mod sub_converter;
mod sys_proxy;
mod traffic_monitor;

use agent::AgentResult;
use ai_service::{AiService, ChatMessage};
use app_logger::LogEntry;
use chat_history::{
    auto_title, delete_conversation, format_history_rag_context, list_conversations,
    load_conversation, save_conversation, search_conversations, search_history_rag, Conversation,
    ConversationMeta,
};
use config_generator::{parse_share_link, parse_subscription, ServerConfig};
use config_manager::{ConfigManager, SavedConfig};
use core_manager::{
    download_xray, fetch_latest_xray_release, find_xray_core, resolve_or_download_core,
    CoreManager, CoreResolveResult,
};
use health_monitor::{full_latency_test, HealthMonitor, LatencyResult};
use knowledge_base::KnowledgeBase;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use sub_converter::SubConverterManager;
use sys_proxy::{disable_system_proxy, enable_system_proxy, ProxySettings};
use tauri::State;
use tokio::sync::Mutex;

static APP_LOGGER: std::sync::OnceLock<&'static app_logger::AppLogger> = std::sync::OnceLock::new();

pub struct AppState {
    pub ai_service: AiService,
    pub core_manager: Arc<CoreManager>,
    pub config_manager: ConfigManager,
    pub health_monitor: HealthMonitor,
    pub knowledge_base: Arc<Mutex<KnowledgeBase>>,
    pub sub_converter: Arc<SubConverterManager>,
}

// ── App Logger ────────────────────────────────────────────────────────────────

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

#[tauri::command]
async fn save_ai_api_key(api_key: String) -> Result<(), String> {
    secure_store::save_ai_api_key(api_key).await
}

#[tauri::command]
async fn load_ai_api_key() -> Result<Option<String>, String> {
    secure_store::load_ai_api_key().await
}

#[tauri::command]
async fn clear_ai_api_key() -> Result<(), String> {
    secure_store::clear_ai_api_key().await
}

#[tauri::command]
async fn clear_core_logs(state: State<'_, AppState>) -> Result<(), String> {
    state.core_manager.clear_logs().await
}

// ── Chat History ──────────────────────────────────────────────────────────────

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
    let hist_msgs: Vec<chat_history::ChatMessage> = messages
        .iter()
        .map(|m| chat_history::ChatMessage {
            role: m.role.clone(),
            content: m.content.clone(),
            timestamp: 0,
        })
        .collect();
    auto_title(&hist_msgs)
}

// ── AI Chat (harness-agent) ───────────────────────────────────────────────────

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
    pub http_port: Option<u16>,
    pub socks_port: Option<u16>,
    pub allow_lan: Option<bool>,
    pub server_count: Option<usize>,
    pub subscription_count: Option<usize>,
    pub servers: Option<Vec<ServerContext>>,
    pub subscriptions: Option<Vec<SubscriptionContext>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerContext {
    pub name: Option<String>,
    pub protocol: Option<String>,
    pub address: Option<String>,
    pub port: Option<u16>,
    pub source: Option<String>,
    pub sub_name: Option<String>,
    pub latency: Option<String>,
    pub tcp_latency: Option<String>,
    pub proxy_latency: Option<String>,
    pub allow_insecure: Option<bool>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubscriptionContext {
    pub name: Option<String>,
    pub node_count: Option<usize>,
    pub updated_at: Option<i64>,
}

/// Build the system message: static prompt + RAG context + local env snapshot.
fn build_system_message(
    doc_rag: &str,
    hist_rag: &str,
    context: &Option<ConnectionContext>,
) -> String {
    let mut sys = agent::system_prompt().to_string();

    // doc_rag / hist_rag already include section headers produced by
    // get_rag_context() / format_history_rag_context() — append directly.
    if !doc_rag.is_empty() {
        sys.push_str(doc_rag);
    }
    if !hist_rag.is_empty() {
        sys.push_str(hist_rag);
    }

    if let Some(ctx) = context {
        sys.push_str(&build_env_snapshot(ctx));
    }

    sys
}

fn build_env_snapshot(ctx: &ConnectionContext) -> String {
    let server_lines: Vec<String> = ctx
        .servers
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .take(80)
        .enumerate()
        .map(|(idx, s)| {
            format!(
                "{}. {}{} [{}] {}:{} | 来源:{}{}{}{}",
                idx + 1,
                if s.is_active.unwrap_or(false) {
                    "* "
                } else {
                    ""
                },
                s.name.as_deref().unwrap_or("未命名"),
                s.protocol.as_deref().unwrap_or("?").to_uppercase(),
                s.address.as_deref().unwrap_or("?"),
                s.port.unwrap_or(0),
                s.source.as_deref().unwrap_or("?"),
                s.sub_name
                    .as_ref()
                    .map(|v| format!(" | 订阅:{v}"))
                    .unwrap_or_default(),
                s.latency
                    .as_ref()
                    .map(|v| format!(" | 延迟:{v}"))
                    .unwrap_or_default(),
                if s.allow_insecure.unwrap_or(false) {
                    " | allowInsecure:true"
                } else {
                    ""
                },
            )
        })
        .collect();

    let sub_lines: Vec<String> = ctx
        .subscriptions
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .enumerate()
        .map(|(idx, s)| {
            format!(
                "{}. {} | 节点:{}",
                idx + 1,
                s.name.as_deref().unwrap_or("未命名"),
                s.node_count.unwrap_or(0),
            )
        })
        .collect();

    format!(
        "\n\n---\n## [本地环境快照]\n\
        连接: {} | 当前节点: {} | 协议: {} | 延迟: {}ms\n\
        代理端口: HTTP {} / SOCKS {} | LAN: {} | 路由: {}\n\
        节点总数: {} | 订阅总数: {}\n\n\
        **节点列表**\n{}\n\n\
        **订阅列表**\n{}\n---",
        ctx.is_connected,
        ctx.server_name.as_deref().unwrap_or("无"),
        ctx.protocol.as_deref().unwrap_or("无"),
        ctx.latency_ms.unwrap_or(0),
        ctx.http_port.unwrap_or(0),
        ctx.socks_port.unwrap_or(0),
        ctx.allow_lan.unwrap_or(false),
        ctx.routing_mode.as_deref().unwrap_or("smart"),
        ctx.server_count.unwrap_or(0),
        ctx.subscription_count.unwrap_or(0),
        if server_lines.is_empty() {
            "无节点".to_string()
        } else {
            server_lines.join("\n")
        },
        if sub_lines.is_empty() {
            "无订阅".to_string()
        } else {
            sub_lines.join("\n")
        },
    )
}

#[tauri::command]
async fn chat_with_ai(
    message: String,
    history: Vec<ChatMessage>,
    settings: AiSettings,
    context: Option<ConnectionContext>,
    state: State<'_, AppState>,
) -> Result<AgentResult, String> {
    // ── Dual RAG: document search + history search (concurrent) ──────────────
    let (doc_rag, hist_chunks) = tokio::join!(
        async {
            let kb = state.knowledge_base.lock().await;
            kb.get_rag_context(&message, 4)
        },
        search_history_rag(&message, 3, 600),
    );
    let hist_rag = format_history_rag_context(&hist_chunks);

    // ── Build system message with RAG + local env context ────────────────────
    let system_content = build_system_message(&doc_rag, &hist_rag, &context);

    // ── Assemble the full message thread ─────────────────────────────────────
    // [system] → [history turns] → [current user message]
    let mut thread = Vec::with_capacity(history.len() + 2);
    thread.push(ChatMessage {
        role: "system".to_string(),
        content: system_content,
    });
    thread.extend(history);
    thread.push(ChatMessage {
        role: "user".to_string(),
        content: message,
    });

    // ── Run agentic loop ──────────────────────────────────────────────────────
    agent::run(
        &state.ai_service,
        &settings.base_url,
        &settings.api_key,
        &settings.model,
        thread,
        &state.sub_converter,
    )
    .await
}

// ── History RAG Debug Command ─────────────────────────────────────────────────

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

// ── Knowledge Base ────────────────────────────────────────────────────────────

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
    Ok(chunks
        .iter()
        .map(|c| {
            serde_json::json!({
                "id": c.id, "title": c.title, "source": c.source, "content": c.content,
            })
        })
        .collect())
}

// ── Config Generation & Management ───────────────────────────────────────────

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
    allow_lan: Option<bool>,
) -> serde_json::Value {
    let listen = if allow_lan.unwrap_or(false) {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    };
    server.to_v2ray_config_with_listen(http_port, socks_port, &routing_mode, listen)
}

#[tauri::command]
async fn apply_config(
    server: ServerConfig,
    http_port: u16,
    socks_port: u16,
    routing_mode: String,
    allow_lan: Option<bool>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let listen = if allow_lan.unwrap_or(false) {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    };
    let config = server.to_v2ray_config_with_listen(http_port, socks_port, &routing_mode, listen);
    state.config_manager.write_active_config(&config).await
}

#[tauri::command]
async fn apply_raw_config(
    config: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<String, String> {
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

// ── Core Management ───────────────────────────────────────────────────────────

#[tauri::command]
async fn start_core(
    core_path: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let config_path = state.config_manager.active_config_path();
    if !std::path::Path::new(&config_path).exists() {
        return Err("请先连接一个节点以生成配置文件".to_string());
    }
    state
        .core_manager
        .start(&core_path, &config_path, Some(app))
        .await
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
    state
        .core_manager
        .test_config(&core_path, &config_path)
        .await
}

#[tauri::command]
async fn get_core_version(core_path: String, state: State<'_, AppState>) -> Result<String, String> {
    state.core_manager.get_version(&core_path).await
}

#[tauri::command]
async fn fetch_latest_xray() -> Result<core_manager::XrayRelease, String> {
    fetch_latest_xray_release().await
}

#[tauri::command]
async fn download_xray_core(download_url: String, install_dir: String) -> Result<String, String> {
    download_xray(&download_url, &install_dir).await
}

#[tauri::command]
async fn auto_detect_core() -> Result<String, String> {
    find_xray_core().await
}

#[tauri::command]
async fn resolve_core() -> Result<CoreResolveResult, String> {
    resolve_or_download_core().await
}

// ── System Proxy ──────────────────────────────────────────────────────────────

#[tauri::command]
async fn enable_proxy(http_port: u16, socks_port: u16) -> Result<String, String> {
    enable_system_proxy(&ProxySettings::local(http_port, socks_port)).await
}

#[tauri::command]
async fn disable_proxy() -> Result<String, String> {
    disable_system_proxy().await
}

// ── Latency & Health ──────────────────────────────────────────────────────────

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

// ── Subscription ──────────────────────────────────────────────────────────────

#[tauri::command]
async fn fetch_subscription(
    url: String,
    state: State<'_, AppState>,
) -> Result<Vec<ServerConfig>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("v2rayN/6.31")
        .no_proxy()
        .build()
        .map_err(|e| format!("构建请求客户端失败: {e}"))?;

    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("获取订阅失败：{e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "订阅服务器返回 HTTP {}，请检查链接是否正确",
            response.status()
        ));
    }

    let content = response
        .text()
        .await
        .map_err(|e| format!("读取订阅内容失败：{e}"))?;

    let servers = parse_subscription(&content);
    if !servers.is_empty() {
        return Ok(servers);
    }

    log::info!("内置解析器未能解析订阅，尝试 subconverter 转换...");
    match state.sub_converter.convert_subscription(&url).await {
        Ok(converted_text) => {
            let servers = parse_subscription(&converted_text);
            if !servers.is_empty() {
                log::info!("subconverter 成功转换 {} 个节点", servers.len());
                return Ok(servers);
            }
            let preview = converted_text
                .get(..100)
                .unwrap_or(&converted_text)
                .replace('\n', " ");
            Err(format!(
                "subconverter 已转换但仍无法解析节点。\n转换结果前缀: {preview}"
            ))
        }
        Err(converter_err) => {
            let preview = content.get(..100).unwrap_or(&content).replace('\n', " ");
            let is_clash = content.trim_start().starts_with("proxies:")
                || content.trim_start().starts_with("proxy-groups:")
                || (content.contains("proxies:") && content.contains("server:"));

            if is_clash {
                Err(format!(
                    "检测到 Clash 格式订阅，内置解析器不支持此格式。\n\
                    请在「工具箱」中安装并启动 subconverter 后重试。\n\nsubconverter 状态：{converter_err}"
                ))
            } else {
                Err(format!(
                    "订阅解析结果为空，未发现支持的节点格式。\n\
                    内容前缀: {preview}\n\nsubconverter 状态：{converter_err}"
                ))
            }
        }
    }
}

// ── SubConverter ──────────────────────────────────────────────────────────────

#[tauri::command]
async fn install_subconverter(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    state.sub_converter.install(&app).await
}

#[tauri::command]
async fn start_subconverter(state: State<'_, AppState>) -> Result<String, String> {
    state.sub_converter.start().await
}

#[tauri::command]
async fn stop_subconverter(state: State<'_, AppState>) -> Result<String, String> {
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

// ── App Entry Point ───────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let logger = app_logger::AppLogger::init();
    APP_LOGGER.set(logger).ok();

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
            get_app_logs,
            clear_app_logs,
            clear_core_logs,
            save_ai_api_key,
            load_ai_api_key,
            clear_ai_api_key,
            save_chat,
            load_chat,
            list_chats,
            delete_chat,
            search_chats,
            generate_conv_title,
            chat_with_ai,
            search_history_knowledge,
            refresh_knowledge_base,
            search_knowledge,
            parse_proxy_link,
            parse_subscription_content,
            generate_config,
            apply_config,
            apply_raw_config,
            get_active_config_path,
            list_saved_configs,
            delete_saved_config,
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
            enable_proxy,
            disable_proxy,
            test_latency,
            start_health_monitor,
            stop_health_monitor,
            fetch_subscription,
            install_subconverter,
            start_subconverter,
            stop_subconverter,
            get_subconverter_status,
            convert_subscription_via_tool,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
