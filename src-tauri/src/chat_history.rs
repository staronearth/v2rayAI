/// Persistent chat history storage
/// Stores conversations as JSON files in ~/.v2rayai/history/

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub messages: Vec<ChatMessage>,
    pub created_at: i64,
    pub updated_at: i64,
    pub summary: Option<String>, // first user message (used as preview)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMeta {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: usize,
    pub preview: String,
}

fn history_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".v2rayai").join("history")
}

fn conv_path(id: &str) -> PathBuf {
    history_dir().join(format!("{}.json", id))
}

/// Save or update a conversation
pub async fn save_conversation(conv: &Conversation) -> Result<(), String> {
    let dir = history_dir();
    fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("创建历史目录失败：{}", e))?;

    let json = serde_json::to_string(conv)
        .map_err(|e| format!("序列化对话失败：{}", e))?;

    fs::write(conv_path(&conv.id), json.as_bytes())
        .await
        .map_err(|e| format!("保存对话失败：{}", e))
}

/// Load a single conversation by ID
pub async fn load_conversation(id: &str) -> Result<Conversation, String> {
    let bytes = fs::read(conv_path(id))
        .await
        .map_err(|e| format!("读取对话失败：{}", e))?;

    serde_json::from_slice(&bytes)
        .map_err(|e| format!("解析对话失败：{}", e))
}

/// List all conversations (metadata only, sorted by updated_at desc)
pub async fn list_conversations() -> Result<Vec<ConversationMeta>, String> {
    let dir = history_dir();
    fs::create_dir_all(&dir).await.ok();

    let mut entries = match fs::read_dir(&dir).await {
        Ok(e) => e,
        Err(_) => return Ok(vec![]),
    };

    let mut metas: Vec<ConversationMeta> = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.ends_with(".json") {
            continue;
        }

        if let Ok(bytes) = fs::read(entry.path()).await {
            if let Ok(conv) = serde_json::from_slice::<Conversation>(&bytes) {
                let preview = conv.messages.iter()
                    .find(|m| m.role == "user")
                    .map(|m| {
                        let p = m.content.chars().take(80).collect::<String>();
                        if m.content.len() > 80 { format!("{}…", p) } else { p }
                    })
                    .unwrap_or_else(|| "（空对话）".to_string());

                metas.push(ConversationMeta {
                    id: conv.id.clone(),
                    title: conv.title.clone(),
                    created_at: conv.created_at,
                    updated_at: conv.updated_at,
                    message_count: conv.messages.len(),
                    preview,
                });
            }
        }
    }

    metas.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(metas)
}

/// Delete a conversation
pub async fn delete_conversation(id: &str) -> Result<(), String> {
    fs::remove_file(conv_path(id))
        .await
        .map_err(|e| format!("删除对话失败：{}", e))
}

/// Search conversations by keyword (searches title + messages content)
pub async fn search_conversations(query: &str) -> Result<Vec<ConversationMeta>, String> {
    let dir = history_dir();
    fs::create_dir_all(&dir).await.ok();

    let query_lower = query.to_lowercase();
    let mut results: Vec<(i64, ConversationMeta)> = Vec::new();

    let mut entries = match fs::read_dir(&dir).await {
        Ok(e) => e,
        Err(_) => return Ok(vec![]),
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        if !name.to_string_lossy().ends_with(".json") {
            continue;
        }

        if let Ok(bytes) = fs::read(entry.path()).await {
            if let Ok(conv) = serde_json::from_slice::<Conversation>(&bytes) {
                let in_title = conv.title.to_lowercase().contains(&query_lower);
                let in_content = conv.messages.iter()
                    .any(|m| m.content.to_lowercase().contains(&query_lower));

                if in_title || in_content {
                    let preview = conv.messages.iter()
                        .find(|m| m.role == "user")
                        .map(|m| {
                            let p = m.content.chars().take(80).collect::<String>();
                            if m.content.len() > 80 { format!("{}…", p) } else { p }
                        })
                        .unwrap_or_else(|| "（空对话）".to_string());

                    results.push((conv.updated_at, ConversationMeta {
                        id: conv.id.clone(),
                        title: conv.title.clone(),
                        created_at: conv.created_at,
                        updated_at: conv.updated_at,
                        message_count: conv.messages.len(),
                        preview,
                    }));
                }
            }
        }
    }

    results.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(results.into_iter().map(|(_, m)| m).collect())
}

/// Auto-generate a title from the first user message
pub fn auto_title(messages: &[ChatMessage]) -> String {
    messages.iter()
        .find(|m| m.role == "user")
        .map(|m| {
            let title: String = m.content.chars().take(30).collect();
            if m.content.len() > 30 { format!("{}…", title) } else { title }
        })
        .unwrap_or_else(|| "新对话".to_string())
}

// ============================================================
// History RAG — TF-IDF semantic search over saved conversations
// ============================================================

/// A relevant snippet extracted from a past conversation for RAG injection.
#[derive(Debug, Clone)]
pub struct HistoryChunk {
    pub conv_title: String,
    pub conv_id: String,
    /// The matched message plus up to 1 adjacent message for context.
    pub snippet: String,
    pub score: f32,
}

/// Tokenise text into lowercase alphanumeric tokens (same strategy as knowledge_base).
fn rag_tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|w| w.len() > 1)
        .map(|w| w.to_string())
        .collect()
}

/// Technical keyword list shared with knowledge_base scoring logic.
const TECH_KEYWORDS: &[&str] = &[
    "vless", "vmess", "trojan", "shadowsocks", "reality", "xtls",
    "tls", "websocket", "ws", "grpc", "tcp", "http", "uuid", "flow",
    "routing", "outbound", "inbound", "dns", "sni", "fingerprint",
    "encryption", "alterid", "network", "security", "protocol",
    "publickey", "privatekey", "shortid", "servername", "vision",
    "subscription", "hysteria", "tuic", "cdn", "cloudflare",
    "geosite", "geoip", "direct", "proxy", "block", "sniffing",
];

/// Score a single text block against query terms.
/// Title-level terms get 3× weight, keyword exact matches 2×, body matches 0.5× per occurrence.
fn rag_score(text: &str, query_terms: &[String]) -> f32 {
    let lower = text.to_lowercase();
    let mut score = 0.0f32;
    for term in query_terms {
        let count = lower.matches(term.as_str()).count();
        score += count as f32 * 0.5;
        // Bonus for technical keyword exact match
        if TECH_KEYWORDS.contains(&term.as_str()) && count > 0 {
            score += 1.0;
        }
    }
    score
}

/// Search all saved conversation files and return the top-k most relevant snippets.
/// Each snippet contains the best-matching message plus the adjacent message for context.
/// Maximum snippet length is capped at `max_snippet_chars` characters.
pub async fn search_history_rag(
    query: &str,
    top_k: usize,
    max_snippet_chars: usize,
) -> Vec<HistoryChunk> {
    let dir = history_dir();
    fs::create_dir_all(&dir).await.ok();

    let query_terms = rag_tokenize(query);
    if query_terms.is_empty() {
        return vec![];
    }

    let mut entries = match fs::read_dir(&dir).await {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut results: Vec<HistoryChunk> = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        if !name.to_string_lossy().ends_with(".json") {
            continue;
        }

        let bytes = match fs::read(entry.path()).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let conv: Conversation = match serde_json::from_slice(&bytes) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Score every message, find the best one
        let mut best_score = 0.0f32;
        let mut best_idx = 0usize;

        for (i, msg) in conv.messages.iter().enumerate() {
            // Title gets 3× weight for query term match
            let title_bonus = if conv.title.to_lowercase()
                .split_whitespace()
                .any(|w| query_terms.iter().any(|qt| w.contains(qt.as_str())))
            { 2.0 } else { 0.0 };

            let s = rag_score(&msg.content, &query_terms) + title_bonus;
            if s > best_score {
                best_score = s;
                best_idx = i;
            }
        }

        if best_score <= 0.0 {
            continue;
        }

        // Build snippet: best message ± 1 adjacent message for context
        let start = best_idx.saturating_sub(1);
        let end = (best_idx + 2).min(conv.messages.len());
        let snippet_parts: Vec<String> = conv.messages[start..end]
            .iter()
            .map(|m| {
                let role_label = if m.role == "user" { "用户" } else { "助手" };
                let content: String = m.content.chars().take(max_snippet_chars / 2).collect();
                let ellipsis = if m.content.len() > max_snippet_chars / 2 { "…" } else { "" };
                format!("**[{}]**: {}{}", role_label, content, ellipsis)
            })
            .collect();

        results.push(HistoryChunk {
            conv_title: conv.title.clone(),
            conv_id: conv.id.clone(),
            snippet: snippet_parts.join("\n\n"),
            score: best_score,
        });
    }

    // Sort by score descending, return top-k
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    results.into_iter().take(top_k).collect()
}

/// Format history RAG chunks as a Markdown context block for AI prompt injection.
pub fn format_history_rag_context(chunks: &[HistoryChunk]) -> String {
    if chunks.is_empty() {
        return String::new();
    }
    let mut ctx = String::from("\n\n---\n## 💬 相关历史对话（对话 RAG）\n\n");
    for chunk in chunks {
        ctx.push_str(&format!(
            "### 来自对话：《{}》\n{}\n\n",
            chunk.conv_title, chunk.snippet
        ));
    }
    ctx.push_str("---\n");
    ctx
}
