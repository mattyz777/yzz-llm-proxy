use serde::{Deserialize, Serialize};
use serde_json::Value;

// OpenAI-compatible ChatRequest example:
//
// {
//   "model": "gpt-5",
//   "messages": [
//     {
//       "role": "system",
//       "content": "You are a helpful assistant."
//     },
//     {
//       "role": "user",
//       "content": "What is the weather in Singapore?"
//     },
//     {
//       "role": "assistant",
//       "content": null,
//       "tool_calls": [
//         {
//           "id": "call_123456",
//           "type": "function",
//           "function": {
//             "name": "get_weather",
//             "arguments": "{\"city\":\"Singapore\"}"
//           }
//         }
//       ]
//     },
//     {
//       "role": "tool",
//       "tool_call_id": "call_123456",
//       "content": "{\"temperature\":32,\"condition\":\"sunny\"}"
//     }
//   ],
//   "stream": true,
//   "temperature": 0.7,
//   "max_tokens": 1024,
//   "tools": [
//     {
//       "type": "function",
//       "function": {
//         "name": "get_weather",
//         "description": "Get current weather information",
//         "parameters": {
//           "type": "object",
//           "properties": {
//             "city": {
//               "type": "string",
//               "description": "City name"
//             }
//           },
//           "required": [
//             "city"
//           ]
//         }
//       }
//     }
//   ]
// }
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default="default_true")]
    pub stream: bool,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub tools: Option<Vec<Tool>>,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Message {
    pub role: String,
    #[serde(default)]
    pub content: Option<Value>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub tool_call_id: Option<String>,
    pub name: Option<String>,
}


#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: Option<String>,
    pub function: ToolCallFunction,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: Option<ToolFunction>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<serde_json::Value>,
}