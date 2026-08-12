
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
    let (name, model) = request.model.split_once('/')
        .ok_or_else(|| {
            tracing::warn!("Model {} is not allowed.", request.model);
            StatusCode::BAD_REQUEST
        })?;
    
    let provider = state.providers.get(name)
        .ok_or_else(|| {
            tracing::warn!("Model {} is not allowed.", request.model);
            StatusCode::NOT_FOUND
        })?;

    match provider {
        ProviderContext::Kiro { token, profile_arn, region } => {
            kiro::client::chat_kiro(&state, token, profile_arn, region, &request, model).await
        },
        ProviderContext::DashScope { .. } => {
            // TODO: proxy to DashScope
            Err(StatusCode::NOT_IMPLEMENTED)
        },
        ProviderContext::Agnes { .. } => {
            // TODO: proxy to Agnes
            Err(StatusCode::NOT_IMPLEMENTED)
        },
    }
}



// Output
// {
//   "object": "list",
//   "data": [
//     {
//       "id": "claude-sonnet-4",
//       "object": "model",
//       "owned_by": "yzz-llm-proxy"
//     }
//   ]
// }
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