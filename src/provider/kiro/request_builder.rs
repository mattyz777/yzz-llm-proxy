use serde_json::{Value, json};
use uuid::Uuid;

use crate::types::openai_request::{ChatRequest, Message, Tool, ToolCall};



struct CurrentMessage {
    content: String,
    tool_results: Vec<Value>,
}


/// OpenAI → Kiro message conversion rules:
///
/// 1. System messages:
///    - Kiro has no dedicated system message field.
///    - All system messages are concatenated into a single prompt.
///    - The system prompt is prepended to the first user message:
///          "{system_prompt}\n\n{user_content}"
///
/// 2. User messages:
///    - First user message: may have system prompt prepended (rule 1).
///    - Subsequent user messages: forwarded as-is.
///
/// 3. Assistant messages:
///    - Converted to Kiro `assistantResponseMessage` history entries.
///    - Sources: previous LLM responses, or agent-generated states.
///
/// 4. Last message is assistant:
///    - Kiro requires `currentMessage` to be a user message.
///    - The assistant message is pushed into history.
///    - A synthetic "Continue" user message becomes `currentMessage`.
///
/// 5. Only system messages (no user/assistant):
///    - System prompt is prepended to a fallback "Hello" message.
///
/// 6. Empty messages array:
///    - Falls back to "Hello" as `currentMessage`, empty history.
///
/// 7. Tool messages (role: "tool"):
///    - Converted to Kiro `userInputMessage` with `toolResults` in `userInputMessageContext`.
///    - Tool result format: { content: [{text: "..."}], status: "success", toolUseId: "..." }
///
/// 8. Assistant messages with tool_calls:
///    - The `tool_calls` array is converted to Kiro `toolUses` on `assistantResponseMessage`.
///    - If content is null/empty, "(empty placeholder)" is used.
///
/// 9. Tool definitions (request.tools):
///    - Converted to Kiro `toolSpecification` format.
///    - Placed in `currentMessage.userInputMessage.userInputMessageContext.tools`.
///    - JSON schemas are sanitized (remove additionalProperties, empty required[]).
///
/// 10. Payload size (future):
///    - Kiro has request size limits.
///    - Long conversations may need history compaction (truncate old tool results).
///    - Not implemented in v1.
/// 
pub fn build_chat_payload(
    request: &ChatRequest,
    model: &str,
    profile_arn: &str,
) -> Value {
    let conversation_id = Uuid::new_v4().simple().to_string();
    let (system_prompt, non_system) = extract_system(&request.messages);
    let (current_msg, history) = build_conversation(&non_system, &system_prompt, model);

    let mut user_input_message = json!({
        "content": current_msg.content,
        "modelId": model,
        "origin": "AI_EDITOR",
    });

    let mut user_input_context = serde_json::Map::new();

    if let Some(tools) = &request.tools {
        let kiro_tools = convert_tools(tools);
        if !kiro_tools.is_empty() {
            user_input_context.insert("tools".to_string(), Value::Array(kiro_tools));
        }
    }

    if !current_msg.tool_results.is_empty() {
        user_input_context.insert("toolResults".to_string(), Value::Array(current_msg.tool_results));
    }

    if !user_input_context.is_empty() {
        user_input_message["userInputMessageContext"] = Value::Object(user_input_context);
    }

    let mut payload = json!({
        "conversationState": {
            "chatTriggerType": "MANUAL",
            "conversationId": conversation_id,
            "currentMessage": {
                "userInputMessage": user_input_message
            }
        }
    });

    if !history.is_empty() {
        payload["conversationState"]["history"] = Value::Array(history);
    }

    if !profile_arn.is_empty() {
        payload["profileArn"] = Value::String(profile_arn.to_string());
    }

    payload
}




fn build_conversation(
    messages: &[&Message],
    system_prompt: &str,
    model_id: &str,
) -> (CurrentMessage, Vec<Value>) {
    if messages.is_empty() {
        let content = if system_prompt.is_empty() {
            "Hello".to_string()
        } else {
            format!("{system_prompt}\n\nHello")
        };
        return (CurrentMessage { content, tool_results: vec![] }, vec![]);
    }

    let (history_messages, last) = messages.split_at(messages.len() - 1);
    let last_message = last[0];

    let mut history = Vec::new();
    for message in history_messages {
        if let Some(entry) = convert_message_to_kiro_history(message, system_prompt, history.is_empty(), model_id) {
            history.push(entry);
        }
    }

    let current = build_current(last_message, system_prompt, &mut history);
    (current, history)
}


/// Converts a single OpenAI message to a Kiro history entry. Skip unknown field(None).
///
/// Case 1: User message → Kiro userInputMessage
///   Input:  { "role": "user", "content": "Write Rust code." }
///   Output: { "userInputMessage": { "content": "Write Rust code.", "modelId": "...", "origin": "AI_EDITOR" } }
///   Note:   First user message gets system prompt prepended: "{system}\n\n{content}"
///
/// Case 2: Assistant message → Kiro assistantResponseMessage
///   Input:  { "role": "assistant", "content": "fn main() {}", "tool_calls": [...] }
///   Output: { "assistantResponseMessage": { "content": "fn main() {}", "toolUses": [...] } }
///   Note:   If content is null/empty, "(empty placeholder)" is used.
///
/// Case 3: Tool result → Kiro userInputMessage with toolResults
///   Input:  { "role": "tool", "content": "1000", "tool_call_id": "call_001" }
///   Output: { "userInputMessage": { "content": "", "modelId": "...", "origin": "AI_EDITOR",
///             "userInputMessageContext": { "toolResults": [{ "content": [{"text": "1000"}], "status": "success", "toolUseId": "call_001" }] } } }
fn convert_message_to_kiro_history(
    message: &Message,
    system_prompt: &str,
    is_first: bool,
    model: &str,
) -> Option<Value> {
    match message.role.as_str() {
         "user" => {
            let content = prepend_system_if_first(
                is_first,
                system_prompt,
                &extract_text_content(&message.content),
            );

            Some(json!({
                "userInputMessage": {
                    "content": content,
                    "modelId": model,
                    "origin": "AI_EDITOR"
                }
            }))
         },

         "assistant" => {
            let content = extract_text_content(&message.content);
            let content = normalize_content(&content, "(empty placeholder)");

            let mut assistant_resp = json!({ "content": content });

            if let Some(tool_calls) = &message.tool_calls {
                let tool_uses = convert_tool_calls(tool_calls);
                if !tool_uses.is_empty() {
                    assistant_resp["toolUses"] = Value::Array(tool_uses);
                }
            }

            Some(json!({ "assistantResponseMessage": assistant_resp }))
         },

         "tool" => {
            let content = extract_text_content(&message.content);
            let content = normalize_content(&content, "(empty result)");

            let tool_use_id = message.tool_call_id.as_deref().unwrap_or("");

            let tool_result = json!({
                "content": [{ "text": content }],
                "status": "success",
                "toolUseId": tool_use_id,
            });

            Some(json!({
                "userInputMessage": {
                    "content": "",
                    "modelId": model,
                    "origin": "AI_EDITOR",
                    "userInputMessageContext": {
                        "toolResults": [tool_result]
                    }
                }
            }))
         },

         _ => None
    }
}



/// Builds the currentMessage(as userInputMessage) from the last message in the array.
///
/// Case 1: Last message is user → becomes currentMessage directly
///   Input:  { "role": "user", "content": "hello" }
///   Output: CurrentMessage { content: "hello", tool_results: [] }
///   Kiro:   { "currentMessage": { "userInputMessage": { "content": "hello", ... } } }
///
/// Case 2: Last message is assistant → pushed to history, "Continue" as currentMessage
///   Input:  { "role": "assistant", "content": "fn main() {}" }
///   Output: CurrentMessage { content: "Continue", tool_results: [] }
///   Kiro:   history gets { "assistantResponseMessage": { "content": "fn main() {}" } }
///           currentMessage gets { "userInputMessage": { "content": "Continue", ... } }
///
/// Case 3: Last message is tool → tool_results returned for userInputMessageContext
///   Input:  { "role": "tool", "content": "1000", "tool_call_id": "call_001" }
///   Output: CurrentMessage { content: "", tool_results: [{ "toolUseId": "call_001", ... }] }
///   Kiro:   { "currentMessage": { "userInputMessage": { "content": "", "userInputMessageContext": { "toolResults": [...] } } } }
fn build_current(
    message: &Message,
    system_prompt: &str,
    history: &mut Vec<Value>
) -> CurrentMessage {
    let content = extract_text_content(&message.content);

    match message.role.as_str() {    
        "assistant" => {
            let content = normalize_content(&content, "(empty placeholder)");
            let mut assistant_resp = json!({ "content": content });

            if let Some(tool_calls) = &message.tool_calls {
                let tool_uses = convert_tool_calls(tool_calls);
                if !tool_uses.is_empty() {
                    assistant_resp["toolUses"] = Value::Array(tool_uses);
                }
            }
            history.push(json!({ "assistantResponseMessage": assistant_resp }));


            CurrentMessage {
                content: "Continue".to_string(),
                tool_results: vec![],
            }
        },

        "tool" => {
            let content = normalize_content(&content, "(empty result)");
            let tool_use_id = message.tool_call_id.as_deref().unwrap_or("");

            let tool_result = json!({
                "content": [{ "text": content }],
                "status": "success",
                "toolUseId": tool_use_id,
            });

            CurrentMessage {
                content: String::new(),
                tool_results: vec![tool_result],
            }
        },

        _ => {
            let content = prepend_system_if_first(history.is_empty(), system_prompt, &content);

            CurrentMessage {
                content,
                tool_results: vec![],
            }
        },

    }
}



/// Prepends system prompt to content if this is the first user message in the conversation.
/// Kiro has no system role — this is the only way to inject system instructions.
fn prepend_system_if_first(is_first: bool, system_prompt: &str, content: &str) -> String {
    if is_first && !system_prompt.is_empty() {
        format!("{system_prompt}\n\n{content}")
    } else {
        content.to_string()
    }
}


fn extract_text_content(content: &Option<Value>) -> String {
    match content {
        // scenario: {"role": "assistant", "tool_calls": [...]}
        None => String::new(),
        // scenario: 	{"content": "Hello world"}
        Some(Value::String(s)) => s.clone(),
        // scenario: text with image 
        // {
        //  "content": 
        //     [
        //       {"type": "text", "text": "Hi"}, 
        //       {"type": "image_url", ...}
        //     ]
        // }       
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
        // scenario: {"content": null, "tool_calls": [...]}
        Some(Value::Null) => String::new(),
        // scenario: Unexpected type (number, bool, object)
        //           stringify to avoid silently dropping content
        Some(other) => other.to_string(),
    }
}


// Concatenates all system messages into a single string (newline-separated \n) and 
// collects the remaining messages into a vec.
fn extract_system(messages: &[Message]) -> (String, Vec<&Message>) {
    let mut system_prompt = String::new();
    let mut non_system = Vec::new();

    for message in messages {
        if message.role == "system" {
            if !system_prompt.is_empty() {
                system_prompt.push('\n');
            }
            system_prompt.push_str(&extract_text_content(&message.content));
        } else {
            non_system.push(message);
        }
    }

    (system_prompt, non_system)
}


/// OpenAI input format
/// request.tools[]:
/// {
///   "type": "function",   <----------------------------------------------------- 
///   "function": {
///     "name": "query_db",
///     "description": "Run SQL", 
///     "parameters": {
///       "type": "object",
///       "properties": {
///         "sql": { "type": "string", "description": "The SQL query to execute" }
///       },
///       "required": ["sql"],
///       "additionalProperties": false
///     }
///   }
/// }
///
/// Kiro output format:
/// Each tool becomes a `toolSpecification` object placed in: 
///     `currentMessage.userInputMessage.userInputMessageContext.tools[]`
///
/// {
///   "toolSpecification": {
///     "name": "query_db",
///     "description": "Run SQL",
///     "inputSchema": {
///       "json": {
///         "type": "object",
///         "properties": {
///           "sql": { "type": "string", "description": "The SQL query to execute" }
///         },
///         "required": ["sql"]
///       }
///     }
///   }
/// }
fn convert_tools(tools: &[Tool]) -> Vec<Value> {
    tools.iter()
        .filter(|tc| tc.tool_type == "function")
        .filter_map(|tc| tc.function.as_ref())
        .map(|func| {
            let params = sanitize_json_schema(
               func.parameters.as_ref().unwrap_or(&json!({}))
            );

            let description = func.description.clone()
                .unwrap_or_else(|| format!("Tool: {}", func.name));
            
            json!({
                "toolSpecification": {
                    "name": func.name,
                    "description": description,
                    "inputSchema": { "json": params }
                }
            })
        }).collect()
}



///
/// OpenAI input format
/// {
///   "role": "assistant",
///   "content": null,
///   "tool_calls": [   <------------------------------------------
///     { 
///         "id": "call_001", 
///         "type": "function", 
///         "function": { "name": "query_db", "arguments": "{\"sql\":\"SELECT...\"}" }
///     }
///   ]
/// }
///
/// Kiro output format:
/// ```json
/// { 
///     "toolUseId": "call_001", 
///     "name": "query_db", 
///     "input": { ... }
/// }
/// ```
fn convert_tool_calls(tool_calls: &[ToolCall]) -> Vec<Value> {
    tool_calls.iter().map(|tc| {
        json!({
            "toolUseId": tc.id,
            "name": tc.function.name,
            "input": serde_json::from_str(&tc.function.arguments).unwrap_or(json!({})),
        })
    }).collect()
}


/// Recursively removes fields that Kiro API rejects.
/// - Remove all additionalProperties fields.
/// - Remove required fields only when they contain an empty array;
/// 
/// OpenAI input format
/// ```json
/// {
///   "tools": [
///     {
///       "type": "function",
///       "function": {
///         "name": "query_db",
///         "description": "Run SQL",
///         "parameters": {         <------------------------------------------
///           "type": "object",
///           "properties": {...},
///           "required": [],
///           "additionalProperties": false
///         }
///       }
///     }
///   ]
/// }
///
/// ```
///
/// Kiro output format:
/// {
///   "type": "object",
///   "properties": { ... }
/// }
/// 
///------------------ why it requires recursively clean up
// {
//   "type": "object",
//   "properties": {
//     "query": { "type": "string" },
//     "filters": {
//       "type": "object",
//       "properties": {
//         "date_range": {
//           "type": "object",
//           "properties": {
//             "start": { "type": "string" },
//             "end": { "type": "string" }
//           },
//           "additionalProperties": false,  // ← level 3
//           "required": []                   // ← level 3
//         }
//       },
//       "additionalProperties": false,  // ← level 2
//       "required": []                   // ← level 2
//     }
//   },
//   "additionalProperties": false,  // ← level 1
//   "required": ["query"]
// }
// 
fn sanitize_json_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => {
            let mut result = serde_json::Map::new();
            
            for (key, value) in map {
                if key == "additionalProperties" {
                    continue;
                }

                if key == "required" {
                    if let Value::Array(arr) = value {
                        if arr.is_empty() {
                            continue;
                        }
                    }
                }
                result.insert(key.clone(), sanitize_json_schema(value));
            }

            Value::Object(result)
        },
        Value::Array(arr) => {
            Value::Array(arr.iter().map(|v| sanitize_json_schema(v)).collect())
        },
        _ => schema.clone(),
    }
}


fn normalize_content(content: &str, default_value: &str) -> String {
    if content.is_empty() {
        default_value.to_string()
    } else {
        content.to_string()
    }
}