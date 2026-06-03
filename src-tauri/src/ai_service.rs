use futures_util::StreamExt;
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

fn is_false(b: &bool) -> bool {
    !*b
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
    /// Omitted when false, keeping non-streaming requests unchanged.
    #[serde(skip_serializing_if = "is_false")]
    stream: bool,
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

// ── Streaming (SSE) ─────────────────────────────────────────────────────────

/// One meaningful event parsed from a single SSE line of the stream.
#[derive(Debug, PartialEq)]
enum StreamEvent {
    /// A text content delta to append/emit.
    Delta(String),
    /// The `[DONE]` sentinel: stream is finished.
    Done,
    /// A line with nothing to emit (blank, comment, role-only delta, parse miss).
    Skip,
}

#[derive(Debug, Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
}

#[derive(Debug, Default, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
}

/// Parse one raw SSE line into a `StreamEvent`. Pure and total — unknown or
/// malformed lines map to `Skip` rather than erroring, so a stray keep-alive
/// or a tool-call delta never aborts the stream.
fn parse_sse_line(line: &str) -> StreamEvent {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return StreamEvent::Skip;
    }
    let data = match line.strip_prefix("data:") {
        Some(d) => d.trim(),
        None => return StreamEvent::Skip,
    };
    if data == "[DONE]" {
        return StreamEvent::Done;
    }
    let chunk: StreamChunk = match serde_json::from_str(data) {
        Ok(c) => c,
        Err(_) => return StreamEvent::Skip,
    };
    match chunk.choices.into_iter().next().and_then(|c| c.delta.content) {
        Some(t) if !t.is_empty() => StreamEvent::Delta(t),
        _ => StreamEvent::Skip,
    }
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
        let headers = auth_headers(api_key)?;

        let body = ApiRequest {
            model,
            messages,
            temperature: 0.7,
            max_tokens: 8192,
            tools,
            tool_choice: tools.map(|_| "auto"),
            stream: false,
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

    /// Stream a plain-text completion, invoking `on_delta` for each text chunk
    /// as it arrives, and returning the fully assembled text at the end.
    ///
    /// This is for text answers only (no tool advertising) — the agent runs tool
    /// turns non-streamed and only streams the final reply. `on_delta` keeps this
    /// free of any Tauri dependency; the command layer adapts a Channel to it.
    pub async fn complete_stream<F>(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        messages: &[ChatMessage],
        mut on_delta: F,
    ) -> Result<String, String>
    where
        F: FnMut(&str),
    {
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        let headers = auth_headers(api_key)?;

        let body = ApiRequest {
            model,
            messages,
            temperature: 0.7,
            max_tokens: 8192,
            tools: None,
            tool_choice: None,
            stream: true,
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

        let mut stream = response.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        let mut full = String::new();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| format!("Stream error: {}", e))?;
            buf.extend_from_slice(&bytes);

            // Drain complete lines (SSE lines end in '\n'); the line boundary is a
            // byte, so buffered UTF-8 multibyte sequences are never split mid-line.
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line_bytes);
                match parse_sse_line(&line) {
                    StreamEvent::Delta(t) => {
                        on_delta(&t);
                        full.push_str(&t);
                    }
                    StreamEvent::Done => return Ok(full),
                    StreamEvent::Skip => {}
                }
            }
        }

        // Flush any trailing line without a final newline.
        if !buf.is_empty() {
            let line = String::from_utf8_lossy(&buf);
            if let StreamEvent::Delta(t) = parse_sse_line(&line) {
                on_delta(&t);
                full.push_str(&t);
            }
        }

        Ok(full)
    }
}

fn auth_headers(api_key: &str) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", api_key))
            .map_err(|e| format!("Invalid API key format: {}", e))?,
    );
    Ok(headers)
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
    fn request_without_tools_omits_tool_and_stream_fields() {
        // Regression guard: requests to non-tool, non-stream endpoints unchanged.
        let m = one_user_msg();
        let body = ApiRequest {
            model: "gpt",
            messages: &m,
            temperature: 0.7,
            max_tokens: 100,
            tools: None,
            tool_choice: None,
            stream: false,
        };
        let v = serde_json::to_value(&body).unwrap();
        assert!(v.get("tools").is_none());
        assert!(v.get("tool_choice").is_none());
        assert!(v.get("stream").is_none());
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
            stream: false,
        };
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(
            v["tools"][0]["function"]["name"],
            json!("fetch_subscription")
        );
        assert_eq!(v["tool_choice"], json!("auto"));
    }

    #[test]
    fn streaming_request_sets_stream_true() {
        let m = one_user_msg();
        let body = ApiRequest {
            model: "gpt",
            messages: &m,
            temperature: 0.7,
            max_tokens: 100,
            tools: None,
            tool_choice: None,
            stream: true,
        };
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["stream"], json!(true));
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

    #[test]
    fn sse_content_delta_is_parsed() {
        let line = r#"data: {"choices":[{"delta":{"content":"你好"}}]}"#;
        assert_eq!(parse_sse_line(line), StreamEvent::Delta("你好".to_string()));
    }

    #[test]
    fn sse_done_sentinel() {
        assert_eq!(parse_sse_line("data: [DONE]"), StreamEvent::Done);
    }

    #[test]
    fn sse_role_only_delta_is_skipped() {
        let line = r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#;
        assert_eq!(parse_sse_line(line), StreamEvent::Skip);
    }

    #[test]
    fn sse_blank_and_comment_lines_skipped() {
        assert_eq!(parse_sse_line(""), StreamEvent::Skip);
        assert_eq!(parse_sse_line(": keep-alive"), StreamEvent::Skip);
    }

    #[test]
    fn sse_empty_choices_chunk_skipped() {
        // Final usage-only chunks can carry an empty choices array.
        let line = r#"data: {"choices":[]}"#;
        assert_eq!(parse_sse_line(line), StreamEvent::Skip);
    }

    #[test]
    fn sse_malformed_json_skipped_not_errored() {
        assert_eq!(parse_sse_line("data: {not json"), StreamEvent::Skip);
    }

    #[test]
    fn sse_handles_crlf_line_ending() {
        let line = "data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\r\n";
        assert_eq!(parse_sse_line(line), StreamEvent::Delta("x".to_string()));
    }
}
