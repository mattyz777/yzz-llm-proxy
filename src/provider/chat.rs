
use axum::{Json, extract::State, http::StatusCode, response::Response};
use serde_json::{Value, json};

use crate::{
    provider::kiro,
    state::{AppState, ProviderContext},
    types::openai_request::ChatRequest,
};

/// POST /v1/chat/completions
pub async fn chat_completions(
    State(state): State<AppState>,
    Json(request): Json<ChatRequest>,
) -> Result<Response, StatusCode> {

}

pub async fn list_models(
    State(state): State<AppState>,
) -> Json<Value> {
    let models:Vec<Value>  = state.models.iter().map(|model| {
        json!({
            "id": model,
            "object": "model",
            "owned_by": "yzz-llm-proxy",
        })
    }).collect();

    Json(json!({
        "object": "list",
        "data": models,
    }))
}