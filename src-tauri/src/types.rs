//! Shared types for the AI pipeline.
//!
//! Centralizes the conversation message model so that the HTTP client
//! (`ai_service`), the agent loop (`agent`), and the Tauri command layer
//! (`lib`) all speak the same vocabulary. The `Role::Tool` variant and the
//! tool-call types here back the native function-calling support; on endpoints
//! that don't support it the agent falls back to the text `[[TOOL:...]]`
//! protocol.

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

/// The `function` payload inside a model-emitted tool call. `arguments` is a raw
/// JSON string exactly as the model produced it (OpenAI delivers it unparsed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

/// A native tool call emitted by the model inside an assistant message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

impl ToolCall {
    /// Build a `"function"`-type tool call.
    pub fn function(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }
}

/// A tool advertised to the model in the request `tools` array (OpenAI shape:
/// `{ "type": "function", "function": { name, description, parameters } }`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub spec_type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the tool's parameters.
    pub parameters: Value,
}

impl ToolSpec {
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
    ) -> Self {
        Self {
            spec_type: "function".to_string(),
            function: ToolFunction {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

/// A single message in a model conversation. This is the transport shape sent
/// to and received from the OpenAI-compatible endpoint. The `tool_calls` and
/// `tool_call_id` fields are omitted from the wire format when absent, so plain
/// messages serialize exactly as before.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    fn bare(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self::bare(role, content)
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::bare(Role::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::bare(Role::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::bare(Role::Assistant, content)
    }

    /// Assistant message carrying native tool calls. `content` is usually empty
    /// since the model's "turn" is the tool invocation itself.
    pub fn assistant_tool_calls(content: impl Into<String>, calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Some(calls),
            tool_call_id: None,
        }
    }

    /// Tool-result message replying to a specific `tool_call_id`.
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────
// These lock the wire format to the OpenAI-compatible contract.

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn role_serializes_lowercase() {
        assert_eq!(serde_json::to_value(Role::Tool).unwrap(), json!("tool"));
        assert_eq!(
            serde_json::to_value(Role::Assistant).unwrap(),
            json!("assistant")
        );
    }

    #[test]
    fn plain_message_omits_tool_fields() {
        // A normal user message must serialize exactly as before the tool fields
        // were added — no `tool_calls`, no `tool_call_id` keys at all.
        let v = serde_json::to_value(ChatMessage::user("hi")).unwrap();
        assert_eq!(v, json!({ "role": "user", "content": "hi" }));
    }

    #[test]
    fn tool_result_message_carries_id() {
        let v = serde_json::to_value(ChatMessage::tool("call_1", "result text")).unwrap();
        assert_eq!(
            v,
            json!({
                "role": "tool",
                "content": "result text",
                "tool_call_id": "call_1"
            })
        );
    }

    #[test]
    fn assistant_tool_calls_serialize_openai_shape() {
        let msg = ChatMessage::assistant_tool_calls(
            "",
            vec![ToolCall::function(
                "call_abc",
                "fetch_subscription",
                r#"{"url":"https://x.com"}"#,
            )],
        );
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            v,
            json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call_abc",
                    "type": "function",
                    "function": {
                        "name": "fetch_subscription",
                        "arguments": "{\"url\":\"https://x.com\"}"
                    }
                }]
            })
        );
    }

    #[test]
    fn deserializes_response_with_tool_calls() {
        // Shape of a `choices[0].message` returned by the API when it decides to
        // call a tool.
        let raw = r#"{
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "call_9",
                "type": "function",
                "function": { "name": "subconverter_status", "arguments": "{}" }
            }]
        }"#;
        let msg: ChatMessage = serde_json::from_str(raw).unwrap();
        let calls = msg.tool_calls.expect("tool_calls present");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_9");
        assert_eq!(calls[0].call_type, "function");
        assert_eq!(calls[0].function.name, "subconverter_status");
    }

    #[test]
    fn deserializes_legacy_message_without_tool_fields() {
        // History messages coming from the frontend only have role/content; the
        // new optional fields must default to None.
        let msg: ChatMessage =
            serde_json::from_str(r#"{ "role": "user", "content": "hello" }"#).unwrap();
        assert!(msg.tool_calls.is_none());
        assert!(msg.tool_call_id.is_none());
    }

    #[test]
    fn tool_spec_serializes_function_shape() {
        let spec = ToolSpec::function(
            "parse_proxy_links",
            "解析代理分享链接",
            json!({
                "type": "object",
                "properties": { "links": { "type": "string" } },
                "required": ["links"]
            }),
        );
        let v = serde_json::to_value(&spec).unwrap();
        assert_eq!(v["type"], json!("function"));
        assert_eq!(v["function"]["name"], json!("parse_proxy_links"));
        assert_eq!(v["function"]["parameters"]["required"], json!(["links"]));
    }

    #[test]
    fn tool_call_helper_sets_function_type() {
        let tc = ToolCall::function("id1", "n", "{}");
        assert_eq!(tc.call_type, "function");
        assert_eq!(tc.function.name, "n");
    }
}
