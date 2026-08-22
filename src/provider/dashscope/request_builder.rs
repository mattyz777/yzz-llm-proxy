use axum::http::{HeaderMap, HeaderValue};
use reqwest::header::USER_AGENT;
use serde_json::{Value, json};

use crate::types::openai_request::{ChatRequest, Message, Tool, ToolCall};

pub fn get_dashscope_headers(api_key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-api-key", HeaderValue::from_str(api_key).unwrap());
    headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    headers.insert("anthropic-beta", HeaderValue::from_static("claude-code-20250219"));
    headers.insert(USER_AGENT, HeaderValue::from_static("claude-cli/1.0.108 (external, cli)"));
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers
}


///
/// ## OpenAI Request (input to proxy):
/// {
///   "model": "qwen3.5-plus",
///   "stream": true,
///   "temperature": 0.7,
///   "max_tokens": 4096,
///   "messages": [
///     { "role": "system", "content": "You are a helpful assistant." },   <--------------------------------
///     { "role": "user", "content": "What's the weather in Singapore?" },
///     {
///       "role": "assistant",
///       "content": "Let me check that for you.",
///       "tool_calls": [{
///         "id": "call_001",
///         "type": "function",
///         "function": { "name": "get_weather", "arguments": "{\"city\":\"Singapore\"}" }
///       }]
///     },
///     { "role": "tool", "tool_call_id": "call_001", "content": "{\"temperature\":32}" },
///     { "role": "user", "content": "Thanks! How about Tokyo?" }
///   ],
///   "tools": [{
///     "type": "function",
///     "function": { "name": "get_weather", "description": "Get weather", "parameters": {"type":"object","properties":{"city":{"type":"string"}},"required":["city"]} }
///   }]
/// }
///
/// 
/// ## Anthropic Request (output to DashScope):
/// {
///   "model": "qwen3.5-plus",
///   "stream": true,
///   "temperature": 0.7,
///   "max_tokens": 4096,
///   "system": "You are a helpful assistant.",     <--------------------------------
///   "messages": [
///     { "role": "user", "content": "What's the weather in Singapore?" },
///     { 
///         "role": "assistant", 
///         "content": [
///             { "type": "text", "text": "Let me check that for you." },
///             { "type": "tool_use", "id": "call_001", "name": "get_weather", "input": {"city":"Singapore"} }
///         ]},
///     {
///         "role": "user", 
///         "content": [
///             { "type": "tool_result", "tool_use_id": "call_001", "content": "{\"temperature\":32}" }
///     ]},
///     { "role": "user", "content": "Thanks! How about Tokyo?" }
///   ],
///   "tools": [{
///     "name": "get_weather",
///     "description": "Get weather",
///     "input_schema": {"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}
///   }]
/// }
///
/// ## Key transformations:
///   - system messages → top-level "system" field
///   - model prefix "dashscope/" stripped
///   - assistant tool_calls → content blocks with type "tool_use", arguments parsed from string to object
///   - tool results (role:"tool") → user message with content type "tool_result"
///   - tool definitions
///
pub fn build_anthropic_payload(request: &ChatRequest, model: &str) -> Value {
    let (system_prompt, anthropic_messages) = convert_messages(&request.messages);

    let mut payload = json!({
        "model": model,
        "messages": anthropic_messages,
        "stream": request.stream,
        "max_tokens": request.max_tokens.unwrap_or(8192),
    });

    if !system_prompt.is_empty() {
        payload["system"] = json!(system_prompt);
    }

    if let Some(temperature) = request.temperature {
        payload["temperature"] = json!(temperature);
    }

    if let Some(tools) = &request.tools {
        let anthropic_tools = convert_tools(tools);
        if !anthropic_tools.is_empty() {
            payload["tools"] = Value::Array(anthropic_tools);
        }
    }

    payload
}




/// openai -> anthropic Rules:
///   - openai role "system" → concatenated into a single system string (returned separately)
///   - openai role "user" → Anthropic user message
///   - openai role "assistant" with tool_calls → Anthropic assistant message with tool_use content blocks
///   - openai role "assistant" without tool_calls → Anthropic assistant message with text content
///   - openai role "tool" → Anthropic user message with tool_result content block
fn convert_messages(messages: &[Message]) -> (String, Vec<Value>) {
    let mut system_prompt = String::new();
    let mut anthropic_messages: Vec<Value> = Vec::new();

    for message in messages {
        match message.role.as_str() {
            "system" => {
                if !system_prompt.is_empty() {
                    system_prompt.push('\n');
                }
                system_prompt.push_str(&extract_text_content(&message.content));
            }

            "user" => {
                // User might send images → preserve them
                anthropic_messages.push(json!({
                    "role": "user",
                    "content": convert_user_content(&message.content),
                }));
            }

            "assistant" => {
                let mut content_blocks: Vec<Value> = Vec::new();
                let text = extract_text_content(&message.content);
                if !text.is_empty() {
                    content_blocks.push(json!({
                        "type": "text",
                        "text": text,
                    }));
                }

                if let Some(tool_calls) = &message.tool_calls {
                    for tc in tool_calls {
                        let input: Value = serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
                        content_blocks.push(json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.function.name,
                            "input": input,
                        }));
                    }
                }

                if content_blocks.is_empty() {
                    content_blocks.push(json!({"type": "text", "text": ""}));
                }

                anthropic_messages.push(json!({
                    "role": "assistant",
                    "content": content_blocks,
                }));
            }

            "tool" => {
                let tool_call_id = message.tool_call_id.as_deref().unwrap_or("");
                let content = extract_text_content(&message.content);

                let tool_result = json!({
                    "type": "tool_result",
                    "tool_use_id": tool_call_id,
                    "content": content,
                });

                // Merge into previous user message if it's also tool_results
                let merged = anthropic_messages.last_mut().and_then(|prev| {
                    if prev.get("role")?.as_str()? == "user" {
                        prev.get_mut("content")?.as_array_mut()
                    } else {
                        None
                    }
                }).and_then(|arr| {
                    if arr.iter().all(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result")) {
                        Some(arr)
                    } else {
                        None
                    }
                });

                if let Some(arr) = merged {
                    arr.push(tool_result);
                } else {
                    anthropic_messages.push(json!({
                        "role": "user",
                        "content": [tool_result],
                    }));
                }
            }


            _ => {} // skip unknown roles
        }
    }

    (system_prompt, anthropic_messages)
}





/// Extracts text from system, assistant, and tool messages,these roles never contain images.
/// 
/// 
/// Case 1: content is null or absent (assistant message with only tool_calls)
///   Input:  {"role": "assistant", "content": null, "tool_calls": [...]}
///   Output: ""
///
/// Case 2: content is a plain string (most common)
///   Input:  {"role": "user", "content": "Hello world"}1
///   Output: "Hello world"
///
/// Case 3: content is an array (messages with images)
///   Input:  {"role": "user", "content": [
///             {"type": "text", "text": "What's in this image?"},
///             {"type": "image_url", "image_url": {"url": "data:image/png;base64,..."}}
///           ]}
///   Output: "What's in this image?"  (only text blocks extracted, images ignored)
///
/// Case 4: content is an unexpected type (number, bool, object — shouldn't happen)
///   Input:  {"role": "user", "content": 42}
///   Output: "42"  (stringified to avoid silently dropping content)
/// 
fn extract_text_content(content: &Option<Value>) -> String {
    match content {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => {
            arr.iter()
                .filter_map(|item| {
                    if item.get("type")?.as_str()? == "text" {
                        item.get("text")?.as_str().map(String::from)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("")
        }
        Some(other) => other.to_string(),
    }
}





/// Converts OpenAI content to Anthropic content format for user messages.
/// Preserves images (unlike extract_text_content which drops them).
///
/// Case 1: content is null or absent
///   Input:  null
///   Output: ""
///
/// Case 2: content is a plain string
///   Input:  "Hello world"
///   Output: "Hello world"
///
/// Case 3: content is an array with text + base64 image
///   Input:  [
///     {"type": "text", "text": "What's in this image?"},
///     {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBOR..."}}
///   ]
///   Output: [
///     {"type": "text", "text": "What's in this image?"},
///     {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBOR..."}}
///   ]
///
/// Case 4: content is an array with text + URL image
///   Input:  [
///     {"type": "text", "text": "Describe this"},
///     {"type": "image_url", "image_url": {"url": "https://example.com/photo.jpg"}}
///   ]
///   Output: [
///     {"type": "text", "text": "Describe this"},
///     {"type": "image", "source": {"type": "url", "url": "https://example.com/photo.jpg"}}
///   ]
///
/// Case 5: content is an unexpected type
///   Input:  42
///   Output: "42"
/// 
fn convert_user_content(content: &Option<Value>) -> Value {
    match content {
        None | Some(Value::Null) => json!(""),
        Some(Value::String(s)) => json!(s),
        Some(Value::Array(arr)) => {
            let blocks: Vec<Value> = arr.iter().filter_map(|item| {
                let item_type = item.get("type")?.as_str()?;
                match item_type {
                    "text" => {
                        let text = item.get("text")?.as_str()?;
                        Some(json!({"type": "text", "text": text}))
                    }
                    "image_url" => {
                        let url = item.get("image_url")?.get("url")?.as_str()?;
                        // Handle base64 data URLs
                        // "data:image/png;base64,iVBOR..."
                        // parts => ["data:image/png;base64", "iVBOR..."]
                        // metdia_type => "image/png"
                        if url.starts_with("data:") {
                            let parts: Vec<&str> = url.splitn(2, ',').collect();
                            if parts.len() == 2 {
                                let media_type = parts[0]
                                    .trim_start_matches("data:")
                                    .trim_end_matches(";base64");
                                Some(json!({
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": media_type,
                                        "data": parts[1],
                                    }
                                }))
                            } else {
                                None
                            }
                        } else {
                            Some(json!({
                                "type": "image",
                                "source": {
                                    "type": "url",
                                    "url": url,
                                }
                            }))
                        }
                    }
                    _ => None,
                }
            }).collect();
            Value::Array(blocks)
        }
        Some(other) => json!(other.to_string()),
    }
}



/// Converts OpenAI tool definitions to Anthropic format.
///
/// OpenAI:
///   { "type": "function", "function": { "name": "get_weather", "description": "Get weather", "parameters": {...} } }
///
/// Anthropic:
///   { "name": "get_weather", "description": "Get weather", "input_schema": {...} }
///
/// Key changes:
///   - Unwrap from "function" wrapper
///   - "parameters" → "input_schema"
///   - Filter out non-function tools
fn convert_tools(tools: &[Tool]) -> Vec<Value> {
    tools.iter()
        .filter(|t| t.tool_type == "function")
        .filter_map(|t| t.function.as_ref())
        .map(|func| {
            let mut tool = json!({ "name": func.name });

            if let Some(desc) = &func.description {
                tool["description"] = json!(desc);
            }

            if let Some(params) = &func.parameters {
                tool["input_schema"] = params.clone();
            } else {
                tool["input_schema"] = json!({"type": "object", "properties": {}});
            }

            tool
        })
        .collect()
}
