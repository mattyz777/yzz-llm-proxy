use axum::{http::StatusCode, response::Response};

use crate::{
    state::AppState,
    types::openai_request::ChatRequest,
};

use super::request_builder::{build_anthropic_payload, get_dashscope_headers};



pub async fn chat_dashscope(
    state: &AppState,
    base_url: &str,
    api_key: &str,
    request: &ChatRequest,
    model: &str,
) -> Result<Response, StatusCode> {
    let payload = build_anthropic_payload(request, model);
    let headers = get_dashscope_headers(api_key);

    tracing::info!("chat_dashscope called: model={}, stream={}", request.model, request.stream);
    tracing::debug!("Payload: {}", serde_json::to_string(&payload).unwrap_or_default());

    let url = format!("{}/messages", base_url.trim_end_matches('/'));

    let resp = state.http
        .post(&url)
        .headers(headers)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(36000))
        .send()
        .await
        .map_err(|e| {
            tracing::error!("DashScope request failed: {e}");
            StatusCode::BAD_GATEWAY
        })?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        tracing::error!("DashScope API error: {status} - {text}");
        return Err(StatusCode::BAD_GATEWAY);
    }

    tracing::info!("DashScope responded: status=200 OK");

    // TODO: transform Anthropic SSE response → OpenAI SSE response
    // For now, return not implemented until response_builder is done
    Err(StatusCode::NOT_IMPLEMENTED)
}
