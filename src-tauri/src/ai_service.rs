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

// ── Embeddings ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    #[serde(default)]
    index: usize,
}

/// Order embeddings by their `index` (the API may return them out of order) and
/// drop the indices.
fn sort_embeddings(mut resp: EmbeddingResponse) -> Vec<Vec<f32>> {
    resp.data.sort_by_key(|d| d.index);
    resp.data.into_iter().map(|d| d.embedding).collect()
}

// ── Streaming (SSE) ─────────────────────────────────────────────────────────

/// Parsed payload of a single streaming delta: a text fragment and/or fragments
/// of one or more tool calls (which arrive split across many deltas).
#[derive(Debug, Default, PartialEq)]
struct ParsedDelta {
    content: Option<String>,
    tool_calls: Vec<ToolCallDelta>,
}

/// A fragment of a tool call within a streaming delta. `id`/`name` arrive once;
/// `arguments` arrives as a string that must be concatenated across deltas.
#[derive(Debug, PartialEq)]
struct ToolCallDelta {
    index: usize,
    id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
}

/// One meaningful event parsed from a single SSE line.
#[derive(Debug, PartialEq)]
enum StreamEvent {
    Delta(ParsedDelta),
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
    delta: RawDelta,
}

#[derive(Debug, Default, Deserialize)]
struct RawDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<RawToolCallDelta>,
}

#[derive(Debug, Deserialize)]
struct RawToolCallDelta {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<RawFnDelta>,
}

#[derive(Debug, Deserialize)]
struct RawFnDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// Parse one raw SSE line into a `StreamEvent`. Pure and total — unknown or
/// malformed lines map to `Skip` rather than erroring, so a stray keep-alive
/// never aborts the stream.
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
    let raw = match chunk.choices.into_iter().next() {
        Some(c) => c.delta,
        None => return StreamEvent::Skip,
    };
    let tool_calls: Vec<ToolCallDelta> = raw
        .tool_calls
        .into_iter()
        .map(|t| ToolCallDelta {
            index: t.index,
            id: t.id,
            name: t.function.as_ref().and_then(|f| f.name.clone()),
            arguments: t.function.and_then(|f| f.arguments),
        })
        .collect();
    let content = raw.content.filter(|c| !c.is_empty());
    if content.is_none() && tool_calls.is_empty() {
        return StreamEvent::Skip;
    }
    StreamEvent::Delta(ParsedDelta {
        content,
        tool_calls,
    })
}

/// Accumulates streaming deltas into a final `Completion`, reassembling tool
/// calls from their fragments (keyed by `index`).
#[derive(Default)]
struct StreamAccumulator {
    content: String,
    tool_calls: Vec<ToolCall>,
}

impl StreamAccumulator {
    /// Apply a delta; returns the content fragment to emit downstream, if any.
    fn apply(&mut self, delta: ParsedDelta) -> Option<String> {
        for tcd in delta.tool_calls {
            while self.tool_calls.len() <= tcd.index {
                self.tool_calls.push(ToolCall::function("", "", ""));
            }
            let tc = &mut self.tool_calls[tcd.index];
            if let Some(id) = tcd.id {
                tc.id = id;
            }
            if let Some(name) = tcd.name {
                tc.function.name = name;
            }
            if let Some(args) = tcd.arguments {
                tc.function.arguments.push_str(&args);
            }
        }
        match delta.content {
            Some(c) if !c.is_empty() => {
                self.content.push_str(&c);
                Some(c)
            }
            _ => None,
        }
    }

    fn into_completion(self) -> Completion {
        let has_tools = self.tool_calls.iter().any(|t| !t.function.name.is_empty());
        Completion {
            content: self.content,
            tool_calls: if has_tools { Some(self.tool_calls) } else { None },
        }
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

    /// Stream a completion, optionally advertising `tools`, invoking `on_delta`
    /// for each text fragment as it arrives, and returning the fully assembled
    /// `Completion` (text + reassembled tool calls) at the end.
    ///
    /// `on_delta` keeps this free of any Tauri dependency; the command layer
    /// adapts a Channel to it. Tool-call turns emit little/no text, so `on_delta`
    /// fires mainly during the final text answer.
    pub async fn complete_stream_with_tools<F>(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        messages: &[ChatMessage],
        tools: Option<&[ToolSpec]>,
        mut on_delta: F,
    ) -> Result<Completion, String>
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
            tools,
            tool_choice: tools.map(|_| "auto"),
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
        let mut acc = StreamAccumulator::default();

        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| format!("Stream error: {}", e))?;
            buf.extend_from_slice(&bytes);

            // Drain complete lines (SSE lines end in '\n'); the line boundary is a
            // byte, so buffered UTF-8 multibyte sequences are never split mid-line.
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line_bytes: Vec<u8> = buf.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line_bytes);
                match parse_sse_line(&line) {
                    StreamEvent::Delta(d) => {
                        if let Some(frag) = acc.apply(d) {
                            on_delta(&frag);
                        }
                    }
                    StreamEvent::Done => return Ok(acc.into_completion()),
                    StreamEvent::Skip => {}
                }
            }
        }

        // Flush any trailing line without a final newline.
        if !buf.is_empty() {
            let line = String::from_utf8_lossy(&buf);
            if let StreamEvent::Delta(d) = parse_sse_line(&line) {
                if let Some(frag) = acc.apply(d) {
                    on_delta(&frag);
                }
            }
        }

        Ok(acc.into_completion())
    }

    /// Compute embeddings for `inputs` via the OpenAI-compatible `/embeddings`
    /// endpoint. Returns one vector per input, in input order.
    pub async fn embeddings(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        inputs: &[String],
    ) -> Result<Vec<Vec<f32>>, String> {
        let url = format!("{}/embeddings", base_url.trim_end_matches('/'));
        let headers = auth_headers(api_key)?;
        let body = EmbeddingRequest {
            model,
            input: inputs,
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
            return Err(format!("Embedding API error ({}): {}", status, body));
        }

        let parsed: EmbeddingResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse embeddings: {}", e))?;
        Ok(sort_embeddings(parsed))
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

    fn content_delta(text: &str) -> StreamEvent {
        StreamEvent::Delta(ParsedDelta {
            content: Some(text.to_string()),
            tool_calls: vec![],
        })
    }

    #[test]
    fn sse_content_delta_is_parsed() {
        let line = r#"data: {"choices":[{"delta":{"content":"你好"}}]}"#;
        assert_eq!(parse_sse_line(line), content_delta("你好"));
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
        assert_eq!(parse_sse_line(line), content_delta("x"));
    }

    #[test]
    fn sse_tool_call_delta_is_parsed() {
        let line = r#"data: {"choices":[{"delta":{"tool_calls":[
            {"index":0,"id":"call_1","function":{"name":"fetch_subscription","arguments":"{\"u"}}
        ]}}]}"#;
        match parse_sse_line(line) {
            StreamEvent::Delta(d) => {
                assert!(d.content.is_none());
                assert_eq!(d.tool_calls.len(), 1);
                assert_eq!(d.tool_calls[0].index, 0);
                assert_eq!(d.tool_calls[0].name.as_deref(), Some("fetch_subscription"));
                assert_eq!(d.tool_calls[0].arguments.as_deref(), Some("{\"u"));
            }
            other => panic!("expected Delta, got {other:?}"),
        }
    }

    #[test]
    fn accumulator_assembles_text() {
        let mut acc = StreamAccumulator::default();
        assert_eq!(
            acc.apply(ParsedDelta {
                content: Some("Hel".into()),
                tool_calls: vec![]
            }),
            Some("Hel".to_string())
        );
        assert_eq!(
            acc.apply(ParsedDelta {
                content: Some("lo".into()),
                tool_calls: vec![]
            }),
            Some("lo".to_string())
        );
        let c = acc.into_completion();
        assert_eq!(c.content, "Hello");
        assert!(!c.has_tool_calls());
    }

    #[test]
    fn accumulator_reassembles_tool_call_from_fragments() {
        let mut acc = StreamAccumulator::default();
        // id + name arrive first, then arguments in pieces.
        acc.apply(ParsedDelta {
            content: None,
            tool_calls: vec![ToolCallDelta {
                index: 0,
                id: Some("call_9".into()),
                name: Some("fetch_subscription".into()),
                arguments: Some("{\"url\":".into()),
            }],
        });
        acc.apply(ParsedDelta {
            content: None,
            tool_calls: vec![ToolCallDelta {
                index: 0,
                id: None,
                name: None,
                arguments: Some("\"https://x.com\"}".into()),
            }],
        });
        let c = acc.into_completion();
        assert_eq!(c.content, "");
        let calls = c.tool_calls.expect("tool calls");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_9");
        assert_eq!(calls[0].function.name, "fetch_subscription");
        assert_eq!(calls[0].function.arguments, r#"{"url":"https://x.com"}"#);
    }

    #[test]
    fn accumulator_handles_multiple_tool_calls_by_index() {
        let mut acc = StreamAccumulator::default();
        acc.apply(ParsedDelta {
            content: None,
            tool_calls: vec![
                ToolCallDelta { index: 0, id: Some("a".into()), name: Some("subconverter_status".into()), arguments: Some("{}".into()) },
                ToolCallDelta { index: 1, id: Some("b".into()), name: Some("fetch_subscription".into()), arguments: Some("{\"url\":\"x\"}".into()) },
            ],
        });
        let calls = acc.into_completion().tool_calls.expect("tool calls");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "subconverter_status");
        assert_eq!(calls[1].function.name, "fetch_subscription");
    }

    #[test]
    fn embeddings_returned_in_index_order() {
        // The API may return data out of order; we sort by `index`.
        let resp: EmbeddingResponse = serde_json::from_str(
            r#"{ "data": [
                { "embedding": [0.0, 1.0], "index": 1 },
                { "embedding": [1.0, 0.0], "index": 0 }
            ]}"#,
        )
        .unwrap();
        assert_eq!(sort_embeddings(resp), vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
    }
}
