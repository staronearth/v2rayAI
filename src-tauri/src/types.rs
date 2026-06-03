//! Shared types for the AI pipeline.
//!
//! Centralizes the conversation message model so that the HTTP client
//! (`ai_service`), the agent loop (`agent`), and the Tauri command layer
//! (`lib`) all speak the same vocabulary. The `Role` enum reserves a `Tool`
//! variant for the upcoming native function-calling support.

use serde::{Deserialize, Serialize};

/// Conversation role, serialized as the lowercase strings the OpenAI-compatible
/// API expects (`"system"`, `"user"`, `"assistant"`, `"tool"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    /// Wire representation, matching the serde serialization.
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// A single message in a model conversation. This is the transport shape sent
/// to and received from the OpenAI-compatible endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::new(Role::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new(Role::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(Role::Assistant, content)
    }
}
