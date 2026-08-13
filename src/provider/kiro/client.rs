use std::sync::Arc;

use axum::{
    http:: StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::{
    provider::kiro::{client_helper::{get_kiro_headers, get_valid_token}, credential::KiroToken, request_builder::build_chat_payload, response_builder::{build_json_response, build_sse_stream}}, state::AppState, types::{kiro_response::ModelsResponse, openai_request::ChatRequest}
};


pub async fn chat_kiro(
    state: &AppState,
    token: &Arc<RwLock<KiroToken>>,
    profile_arn: &str,
    region: &str,
    request: &ChatRequest,
    model: &str,
) -> Result<Response, StatusCode> {
    tracing::info!("chat_kiro called: model={}, stream={}", request.model, request.stream);
    let access_token = get_valid_token(token, &state.http).await?;
    let payload = build_chat_payload(request, model, profile_arn);
    
    tracing::debug!("Payload: {}", serde_json::to_string(&payload).unwrap());
    
    tracing::info!("Sending to Kiro...");
    let resp = send_to_kiro(&state.http, &access_token, region, &payload).await?;
    tracing::info!("Kiro responded: status={}", resp.status());

    if request.stream {
        Ok(build_sse_stream(resp, request.model.clone()).into_response())
    } else {
        Ok(build_json_response(resp, request.model.clone()).await?.into_response())
    }
}

pub async fn list_models(
    http: &reqwest::Client,
    token: &KiroToken,
    profile_arn: &str,
) -> anyhow::Result<Vec<String>> {
    let url = format!("https://q.{}.amazonaws.com/ListAvailableModels", token.region);
    
    let res = http
        .get(&url)
        .headers(get_kiro_headers())
        .bearer_auth(&token.access_token)
        .query(&[("origin", "AI_EDITOR"), ("profileArn", profile_arn)])
        .send()
        .await?;

    if !res.status().is_success() {
        let text = res.text().await.unwrap_or_default();
        anyhow::bail!("kiro ListAvailableModels failed: {text}");
    }

    let data: ModelsResponse = res.json().await?;
    Ok(data.models.into_iter().map(|m| m.model_id).collect())
}



async fn send_to_kiro(
    http: &reqwest::Client,
    access_token: &str,
    region: &str,
    payload: &Value,
) -> Result<reqwest::Response, StatusCode> {
    let url = format!(
        "https://runtime.{}.kiro.dev/generateAssistantResponse",
        region
    );

    let resp = http
        .post(&url)
        .headers(get_kiro_headers())
        .bearer_auth(access_token)
        .json(payload)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| {
            tracing::error!("Kiro request failed: {e}");
            StatusCode::BAD_GATEWAY
        })?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        tracing::error!("Kiro API error: {status} - {text}");
        return Err(StatusCode::BAD_GATEWAY);
    }

    Ok(resp)
}