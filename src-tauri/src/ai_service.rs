use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};

// Re-exported so existing `ai_service::ChatMessage` paths keep working while the
// canonical definition lives in `types`.
pub use crate::types::ChatMessage;
use crate::types::{ToolCall, ToolSpec};

/// A parsed model completion: free-text content plus any native tool calls the
/// model decided to make. On endpoints/turns without tool calls, `tool_calls`
/// is `None` and this collapses to plain text.
#[derive(Debug, Clone)]
pub struct Completion {
    pub content: String,
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl Completion {
    pub fn has_tool_calls(&self) -> bool {
        self.tool_calls.as_ref().is_some_and(|t| !t.is_empty())
    }
}

#[derive(Debug, Serialize)]
struct ApiRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    temperature: f64,
    max_tokens: u32,
    /// Omitted entirely when no tools are advertised, so requests to endpoints
    /// that don't support function calling are byte-for-byte unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [ToolSpec]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Debug, Deserialize)]
struct ResponseMessage {
    // `content` is `null` when the model returns only tool calls.
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCall>>,
}

/// Pull the first choice out of an API response into a `Completion`.
fn extract_completion(resp: ApiResponse) -> Result<Completion, String> {
    let msg = resp
        .choices
        .into_iter()
        .next()
        .map(|c| c.message)
        .ok_or_else(|| "No response from AI".to_string())?;
    Ok(Completion {
        content: msg.content.unwrap_or_default(),
        tool_calls: msg.tool_calls,
    })
}

#[derive(Debug, Clone)]
pub struct AiService {
    client: reqwest::Client,
}

impl AiService {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// Send `messages` and return the model's text reply. Thin wrapper over
    /// `complete_with_tools` for callers that don't use native function calling.
    pub async fn complete(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        messages: &[ChatMessage],
    ) -> Result<String, String> {
        Ok(self
            .complete_with_tools(base_url, api_key, model, messages, None)
            .await?
            .content)
    }

    /// Send `messages`, optionally advertising `tools`, and return the full
    /// `Completion` (text + any tool calls). When `tools` is `None`, the request
    /// body omits the `tools`/`tool_choice` fields entirely.
    pub async fn complete_with_tools(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        messages: &[ChatMessage],
        tools: Option<&[ToolSpec]>,
    ) -> Result<Completion, String> {
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", api_key))
                .map_err(|e| format!("Invalid API key format: {}", e))?,
        );

        let body = ApiRequest {
            model,
            messages,
            temperature: 0.7,
            max_tokens: 8192,
            tools,
            tool_choice: tools.map(|_| "auto"),
        };

        let response = self
            .client
            .post(&url)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("API error ({}): {}", status, body));
        }

        let api_response: ApiResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        extract_completion(api_response)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one_user_msg() -> Vec<ChatMessage> {
        vec![ChatMessage::user("hi")]
    }

    #[test]
    fn request_without_tools_omits_tool_fields() {
        // Regression guard: requests to non-tool endpoints must be unchanged.
        let m = one_user_msg();
        let body = ApiRequest {
            model: "gpt",
            messages: &m,
            temperature: 0.7,
            max_tokens: 100,
            tools: None,
            tool_choice: None,
        };
        let v = serde_json::to_value(&body).unwrap();
        assert!(v.get("tools").is_none());
        assert!(v.get("tool_choice").is_none());
        assert_eq!(v["model"], json!("gpt"));
        assert_eq!(v["messages"][0], json!({ "role": "user", "content": "hi" }));
    }

    #[test]
    fn request_with_tools_includes_tools_and_choice() {
        let m = one_user_msg();
        let tools = vec![ToolSpec::function(
            "fetch_subscription",
            "拉取订阅",
            json!({ "type": "object" }),
        )];
        let body = ApiRequest {
            model: "gpt",
            messages: &m,
            temperature: 0.7,
            max_tokens: 100,
            tools: Some(&tools),
            tool_choice: Some("auto"),
        };
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(
            v["tools"][0]["function"]["name"],
            json!("fetch_subscription")
        );
        assert_eq!(v["tool_choice"], json!("auto"));
    }

    #[test]
    fn extract_plain_content() {
        let resp: ApiResponse =
            serde_json::from_str(r#"{ "choices": [{ "message": { "content": "hello" } }] }"#)
                .unwrap();
        let c = extract_completion(resp).unwrap();
        assert_eq!(c.content, "hello");
        assert!(!c.has_tool_calls());
    }

    #[test]
    fn extract_tool_calls_with_null_content() {
        let resp: ApiResponse = serde_json::from_str(
            r#"{ "choices": [{ "message": {
                "content": null,
                "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": { "name": "fetch_subscription", "arguments": "{}" }
                }]
            }}]}"#,
        )
        .unwrap();
        let c = extract_completion(resp).unwrap();
        assert_eq!(c.content, "");
        assert!(c.has_tool_calls());
        assert_eq!(
            c.tool_calls.unwrap()[0].function.name,
            "fetch_subscription"
        );
    }

    #[test]
    fn extract_empty_choices_errs() {
        let resp: ApiResponse = serde_json::from_str(r#"{ "choices": [] }"#).unwrap();
        assert!(extract_completion(resp).is_err());
    }
}
