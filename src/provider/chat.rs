
use axum::{Json, extract::State, http::StatusCode, response::Response};
use serde_json::Value;

use crate::{
    provider::kiro,
    state::{AppState, ProviderAuth},
    types::ChatRequest,
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
    
}