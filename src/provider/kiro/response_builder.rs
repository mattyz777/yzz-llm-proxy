use axum::{Json, response::{Sse, sse::Event}};
use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

/// A. Response from Kiro (binary-framed stream, each line is an independent JSON object):
///
/// {"content": "Hello"}
/// {"content": " world"}
/// {"toolUse": {"name": "get_weather", "input": {"city": "Singapore"}, "toolUseId": "call_001"}}
/// {"toolUse": {"name": "get_weather", "input": {"city": "Tokyo"}, "toolUseId": "call_002"}}
///
///
/// B. Transformed OpenAI-compatible SSE format (stream=true):
///
/// data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1723190400,"model":"kiro/claude-sonnet-4","choices":[{"index":0,"delta":{"role":"assistant","content":"Hello"},"finish_reason":null}]}
/// data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1723190400,"model":"kiro/claude-sonnet-4","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":null}]}
/// data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1723190400,"model":"kiro/claude-sonnet-4","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_001","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Singapore\"}"}},{"index":1,"id":"call_002","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Tokyo\"}"}}]},"finish_reason":null}]}
/// data: {"id":"chatcmpl-xxx","object":"chat.completion.chunk","created":1723190400,"model":"kiro/claude-sonnet-4","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}
/// data: [DONE]
///
///
/// C. Transformed OpenAI-compatible JSON format (stream=false):
///
/// {
///   "id": "chatcmpl-xxx",
///   "object": "chat.completion",
///   "created": 1723190400,
///   "model": "kiro/claude-sonnet-4",
///   "choices": [{
///     "index": 0,
///     "message": {
///       "role": "assistant",
///       "content": "Hello world",
///       "tool_calls": [
///         {"index": 0, "id": "call_001", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"Singapore\"}"}},
///         {"index": 1, "id": "call_002", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"Tokyo\"}"}}
///       ]
///     },
///     "finish_reason": "tool_calls"
///   }]
/// }




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

        // Raw stream arrives in chunks:
        //
        // Chunk 1: b"\x00\x05\x0a{"content":"Hel"
        // Chunk 2: b"lo"}\x00\x03\x0b{"content":" wor"
        // Chunk 3: b"ld"}\x00\x02"
        //
        // ─── Chunk 1 received ───
        // buffer: "\x00\x05\x0a{"content":"Hel"
        //          ↑ find('{') at pos=3
        //          find_matching_brace → None (no closing })
        //          break, wait for next chunk
        //
        // ─── Chunk 2 received (appended) ───
        // buffer: "\x00\x05\x0a{"content":"Hello"}\x00\x03\x0b{"content":" wor"
        //          ↑ find('{') at pos=3
        //          find_matching_brace → Some(21)
        //          extract: {"content":"Hello"}  ✓ valid JSON → emit SSE
        //          buffer becomes: "\x00\x03\x0b{"content":" wor"
        //
        //          ↑ find('{') at pos=3
        //          find_matching_brace → None (incomplete)
        //          break, wait for next chunk
        //
        // ─── Chunk 3 received (appended) ───
        // buffer: "\x00\x03\x0b{"content":" world"}\x00\x02"
        //          ↑ find('{') at pos=3
        //          find_matching_brace → Some(22)
        //          extract: {"content":" world"}  ✓ valid JSON → emit SSE
        //          buffer becomes: "\x00\x02"
        //
        //          find('{') → None
        //          loop ends
        //
        while let Some(chunk_result) = byte_stream.next().await {
            let chunk:Bytes = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Stream read error: {e}");
                    break;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buffer.find('{') {
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

                if let Some(event) = handle_content_event(&parsed, &mut first_chunk, &completion_id, created, &model) {
                    yield Ok::<_, std::convert::Infallible>(event);
                }

                if let Some(tool_call) = extract_tool_use_event(&parsed, tool_calls.len()) {
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



/// Transforms Kiro's binary-framed stream into OpenAI-compatible responses.
///
/// Data flow through three layers:
///
/// 1. Network layer (raw TCP chunks):
///    Kiro sends raw bytes where a single JSON event may be split across
///    multiple chunks, or multiple events may arrive in one chunk.
///    e.g. b"\x00\x05\x0a{\"content\":\"Hel"  (incomplete)
///
/// 2. Logical event layer (complete JSON objects):
///    The sliding window buffer assembles raw bytes into complete, independent
///    JSON objects. Each object is one semantic event from Kiro.
///    e.g. {"content": "Hello"}  or  {"toolUse": {...}}
///
/// 3. OpenAI output layer (SSE "chunks"):
///    Each logical event is wrapped into a complete OpenAI `chat.completion.chunk`
///    JSON message and emitted as an SSE `data:` line. Despite the name "chunk",
///    each one is a fully valid JSON object — it's called a "chunk" because it
///    represents one piece of the overall assistant response, not because it's
///    incomplete data.
/// 
/// Input:  a parsed JSON value containing {"content": "Hello"}
/// Output: 
///         {"id":"...","object":"chat.completion.chunk","created":...,"model":"...","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}
///
/// On the first invocation, "role":"assistant" is included in the delta.
/// 
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



/// Extracts a Kiro tool use event and reshapes it into OpenAI tool_call format.
///
/// Input:  a parsed JSON value containing {"toolUse": {"name": "query_db", "input": {...}, "toolUseId": "call_001"}}
/// Output: Some({"index": 0, "id": "call_001", "type": "function", "function": {"name": "query_db", "arguments": "{...}"}})
///
fn extract_tool_use_event(parsed: &Value, index: usize) -> Option<Value> {
    //  Returns None if toolUse node doesn't exist or json missing critical fields.
    let tool_use = parsed.get("toolUse")?;
    let name = tool_use.get("name")?.as_str().unwrap_or("");
    let input = tool_use.get("input").cloned().unwrap_or(json!({}));

    let tool_use_id = tool_use.get("toolUseId")
        .and_then(|id| id.as_str())
        .unwrap_or("")
        .to_string();

    let id = if tool_use_id.is_empty() {
        format!("call_{}", Uuid::new_v4().simple())
    } else {
        tool_use_id
    };

    Some(json!({
        "index": index,
        "id": id,
        "type": "function",
        "function": {
            "name": name,
            "arguments": serde_json::to_string(&input).unwrap_or_default()
        }
    }))
}


/// Output 
/// {
///   "id":"...",
///   "object":"chat.completion.chunk",
///   "created":...,
///   "model":"...",
///   "choices":[
///     { "index":0,"delta":{"tool_calls":[...]},"finish_reason":null }
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

    while let Some(chunk_result) = byte_stream.next().await {
        let chunk: Bytes = match chunk_result {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Stream read error: {e}");
                break;
            }
        };

        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buffer.find('{') {
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

            if let Some(tool_call) = extract_tool_use_event(&parsed, tool_calls.len()) {
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