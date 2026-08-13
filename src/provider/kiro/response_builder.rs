use axum::{Json, response::{Sse, sse::Event}};
use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::collections::HashMap;
use uuid::Uuid;

/// Transforms Kiro's binary-framed stream into OpenAI-compatible responses.
///
/// A. Response from Kiro (AWS binary event stream, each frame contains one JSON object):
///
/// Content events:
///   {"content": "Hello", "modelId": "glm-5"}
///   {"content": " world", "modelId": "glm-5"}
///
/// Streaming tool call events (3 phases per tool call):
///   {"name": "read", "toolUseId": "tooluse_xxx"}                          — start
///   {"input": "{\"filePath\": \"src/", "name": "read", "toolUseId": "tooluse_xxx"}  — arg chunks
///   {"input": "main.rs\"}", "name": "read", "toolUseId": "tooluse_xxx"}   — more arg chunks
///   {"name": "read", "stop": true, "toolUseId": "tooluse_xxx"}            — complete
///
/// Metadata events (ignored):
///   {"stopReason": "END_TURN"}
///   {"stopReason": "TOOL_USE"}
///   {"contextUsagePercentage": 6.3}
///   {"unit": "credit", "unitPlural": "credits", "usage": 0.15}
///
///
/// B. Transformed OpenAI-compatible SSE format (stream=true):
///
/// data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":...,"model":"kiro/glm-5","choices":[{"index":0,"delta":{"role":"assistant","content":"Hello"},"finish_reason":null}]}
/// data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":...,"model":"kiro/glm-5","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":null}]}
/// data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":...,"model":"kiro/glm-5","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"tooluse_xxx","type":"function","function":{"name":"read","arguments":"{\"filePath\":\"src/main.rs\"}"}}]},"finish_reason":null}]}
/// data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":...,"model":"kiro/glm-5","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}
/// data: [DONE]
///
///
/// C. Transformed OpenAI-compatible JSON format (stream=false):
///
/// {
///   "id": "chatcmpl-xxx",
///   "object": "chat.completion",
///   "created": 1723190400,
///   "model": "kiro/glm-5",
///   "choices": [{
///     "index": 0,
///     "message": {
///       "role": "assistant",
///       "content": "Hello world",
///       "tool_calls": [
///         {"index": 0, "id": "tooluse_xxx", "type": "function", "function": {"name": "read", "arguments": "{\"filePath\":\"src/main.rs\"}"}}
///       ]
///     },
///     "finish_reason": "tool_calls"
///   }]
/// }




/// Accumulates streaming tool call fragments from Kiro.
/// Kiro sends tool calls in 3 phases:(added space just for human readable)
/// {"name":"skill",                                     "toolUseId":"tooluse_K3Yc"}
/// {"name":"skill", "input":"{\"name\": \"grilling\"}", "toolUseId":"tooluse_K3Yc"}
/// {"name":"skill", "stop":true,                        "toolUseId":"tooluse_K3Yc"}
/// 
/// -------------------- openai format
/// {
///   "choices": [{
///     "delta": {
///       "tool_calls": [{
///         "id": "tooluse_K3Yc",
///         "type": "function",
///         "function": {
///           "name": "skill",
///           "arguments": "{\"name\": \"grilling\"}"
///         }
///       }]
///     }
///   }]
/// }
struct ToolCallAccumulator {
    calls: HashMap<String, (String, String)>, // toolUseId -> (name, accumulated_input)
}

impl ToolCallAccumulator {
    fn new() -> Self {
        Self { calls: HashMap::new() }
    }

    /// Process a streaming event. Returns Some(completed tool call) when stop:true is received.
    fn process(&mut self, parsed: &Value) -> Option<Value> {
        let tool_use_id = parsed.get("toolUseId")?.as_str()?;
        let name = parsed.get("name")?.as_str()?;

        // Skip if this is a content event (has "content" key and "modelId" key)
        if parsed.get("modelId").is_some() {
            return None;
        }

        // Check if this is the stop signal
        if parsed.get("stop").and_then(|v| v.as_bool()).unwrap_or(false) {
            // Tool call complete — remove from map and return finished tool call
            let (tool_name, input_str) = self.calls.remove(tool_use_id)?;
            let input: Value = serde_json::from_str(&input_str).unwrap_or(json!({}));

            return Some(json!({
                "index": 0, // placeholder, will be updated by caller
                "id": tool_use_id,
                "type": "function",
                "function": {
                    "name": tool_name,
                    "arguments": serde_json::to_string(&input).unwrap_or_default()
                }
            }));
        }

        // Accumulate input fragments
        if let Some(input_fragment) = parsed.get("input").and_then(|v| v.as_str()) {
            self.calls
                .entry(tool_use_id.to_string())
                .and_modify(|(_, acc)| acc.push_str(input_fragment))
                .or_insert_with(|| (name.to_string(), input_fragment.to_string()));
        } else {
            // First event (no input yet) — register the tool call
            self.calls
                .entry(tool_use_id.to_string())
                .or_insert_with(|| (name.to_string(), String::new()));
        }

        None
    }
}



pub fn build_sse_stream(
    resp: reqwest::Response,
    model: String,
) -> Sse<impl futures_util::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let completion_id = format!("chatcmpl-{}", Uuid::new_v4());
    let created = chrono::Utc::now().timestamp();

    let stream = async_stream::stream! {
        let mut buffer = String::new();
        let mut byte_stream = resp.bytes_stream();
        let mut first_chunk = true;
        let mut tool_calls: Vec<Value> = Vec::new();
        let mut tool_accumulator = ToolCallAccumulator::new();

        // Raw stream arrives in binary-framed chunks. Each frame has a header
        // ending with the literal "event" followed by the JSON payload.
        //
        // Example chunk: b"\x00\x00\x00\x89...:message-type\x07\x00\x05event{\"content\":\"Hello\"}\x27\x76..."
        //
        // The parser searches for "event{" to locate the start of each JSON object,
        // then uses find_matching_brace to extract the complete object.
        while let Some(chunk_result) = byte_stream.next().await {
            let chunk:Bytes = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Stream read error: {e}");
                    break;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));
            tracing::debug!("Raw chunk ({} bytes): {:?}", chunk.len(), String::from_utf8_lossy(&chunk[..chunk.len().min(200)]));

            while let Some(pos) = buffer.find("event{").map(|p| p + 5) {
                let end = match find_matching_brace(&buffer, pos) {
                    Some(e) => e,
                    None => break,
                };

                let json_str = buffer[pos..=end].to_string();
                tracing::debug!("Extracted JSON: {}", &json_str[..json_str.len().min(200)]);

                buffer = buffer[end + 1..].to_string();

                let parsed: Value = match serde_json::from_str(&json_str) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                if let Some(event) = handle_content_event(&parsed, &mut first_chunk, &completion_id, created, &model) {
                    yield Ok::<_, std::convert::Infallible>(event);
                }

                if let Some(mut tool_call) = tool_accumulator.process(&parsed) {
                    tool_call["index"] = json!(tool_calls.len());
                    tool_calls.push(tool_call);
                }
            }
        }

        // Trade off, it collects all tool calls during the stream, emits them as one batch at the end.
        if let Some(event) = build_tool_calls_event(&tool_calls, &completion_id, created, &model) {
            yield Ok::<_, std::convert::Infallible>(event);
        }

        let finish_reason = if tool_calls.is_empty() { "stop" } else { "tool_calls" };
        yield Ok::<_, std::convert::Infallible>(build_finish_event(&completion_id, created, &model, finish_reason));

        yield Ok::<_, std::convert::Infallible>(Event::default().data("[DONE]"));
    };

    Sse::new(stream)
}


/// Finds the matching closing `}` for a JSON object starting at `start`.
/// Handles nested braces, quoted strings, and escape sequences.
///
/// Example 1: Simple object
///   Input:  `{"content": "hello"}` at start=0
///   Output: Some(19) — position of the closing `}`
///
/// Example 2: Nested braces
///   Input:  `{"input": {"sql": "SELECT"}}` at start=0
///   Output: Some(28) — outermost closing `}`
///
/// Example 3: Braces inside strings (ignored)
///   Input:  `{"msg": "hello { world }"}` at start=0
///   Output: Some(25) — only real braces count, not those in strings
///
/// Example 4: Escaped quotes in strings
///   Input:  `{"msg": "say \"hi\""}` at start=0
///   Output: Some(19) — escaped quotes don't end the string
///
/// Example 5: Incomplete object
///   Input:  `{"content": "hel` at start=0
///   Output: None — no matching `}` found, wait for more data
/// 
fn find_matching_brace(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for i in start..bytes.len() {
        // （1/2）If the previous character is a backslash (\), the current character should be 
        // treated as a normal character regardless of what it is
        if escape_next {
            escape_next = false;
            continue;
        }

        let ch = bytes[i];

        // （2/2） A backslash inside a string escapes the following character.
        if ch == b'\\' && in_string {
            escape_next = true;
            continue;
        }

        // A quote marks the beginning or end of a JSON string.
        if ch == b'"' {
            in_string = !in_string;
            continue;
        }

        if !in_string {
            if ch == b'{' {
                depth += 1;
            } else if ch == b'}' {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
    }

    None
}



/// Converts a parsed Kiro content event into an OpenAI SSE Event.
///
/// Input:  a parsed JSON value containing {"content": "Hello", "modelId": "glm-5"}
/// Output: an axum SSE Event with data:
///         {"id":"...","object":"chat.completion.chunk","created":...,"model":"...","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}
///
/// On the first invocation, "role":"assistant" is included in the delta.
/// Returns None if content is empty or "content" key is absent.
fn handle_content_event(
    parsed: &Value,
    first_chunk: &mut bool,
    completion_id: &str,
    created: i64,
    model: &str,
) -> Option<Event> {
    let content = parsed.get("content")?.as_str()?;
    if content.is_empty() {
        return None;
    }

    let mut delta = json!({"content": content});
    // The role is only sent once in the first chunk to signal "this is the assistant speaking."
    if *first_chunk {
        delta["role"] = json!("assistant");
        *first_chunk = false;
    }

    let chunk = json!({
        "id": completion_id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{"index": 0, "delta": delta, "finish_reason": null}]
    });

    let data = serde_json::to_string(&chunk).unwrap_or_default();
    Some(Event::default().data(data))
}



/// Output 
/// {
///   "id":"...",
///   "object":"chat.completion.chunk",
///   "created":...,
///   "model":"...",
///   "choices":[
///     { "index":0, "delta":{ "tool_calls" : [...] }, "finish_reason":null }
///   ]
/// }
///
fn build_tool_calls_event(
    tool_calls: &[Value],
    completion_id: &str,
    created: i64,
    model: &str,
) -> Option<Event> {
    if tool_calls.is_empty() {
        return None;
    }

    let chunk = json!({
        "id": completion_id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{"index": 0, "delta": {"tool_calls": tool_calls}, "finish_reason": null}]
    });

    let data = serde_json::to_string(&chunk).unwrap_or_default();
    Some(Event::default().data(data))
}


fn build_finish_event(completion_id: &str, created: i64, model: &str, finish_reason: &str) -> Event {
    let chunk = json!({
        "id": completion_id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{"index": 0, "delta": {}, "finish_reason": finish_reason}]
    });

    let data = serde_json::to_string(&chunk).unwrap_or_default();
    Event::default().data(data)
}



pub async fn build_json_response(
    resp: reqwest::Response,
    model: String,
) -> Result<Json<Value>, StatusCode> {
    let completion_id = format!("chatcmpl-{}", Uuid::new_v4());
    let created = chrono::Utc::now().timestamp();

    let mut buffer = String::new();
    let mut byte_stream = resp.bytes_stream();
    let mut full_content = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut tool_accumulator = ToolCallAccumulator::new();

    while let Some(chunk_result) = byte_stream.next().await {
        let chunk: Bytes = match chunk_result {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Stream read error: {e}");
                break;
            }
        };

        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buffer.find("event{").map(|p| p + 5) {
            let end = match find_matching_brace(&buffer, pos) {
                Some(e) => e,
                None => break,
            };

            let json_str = buffer[pos..=end].to_string();
            buffer = buffer[end + 1..].to_string();

            let parsed: Value = match serde_json::from_str(&json_str) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if let Some(content) = parsed.get("content").and_then(|c| c.as_str()) {
                full_content.push_str(content);
            }

            if let Some(mut tool_call) = tool_accumulator.process(&parsed) {
                tool_call["index"] = json!(tool_calls.len());
                tool_calls.push(tool_call);
            }
        }
    }

    let finish_reason = if tool_calls.is_empty() { "stop" } else { "tool_calls" };

    let mut message = json!({
        "role": "assistant",
        "content": if full_content.is_empty() { Value::Null } else { Value::String(full_content) },
    });

    if !tool_calls.is_empty() {
        message["tool_calls"] = Value::Array(tool_calls);
    }

    let response = json!({
        "id": completion_id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason
        }]
    });

    Ok(Json(response))
}
