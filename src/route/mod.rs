use axum::{Router, routing::{get, post}};
use crate::{provider::chat, state::AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/models", get(chat::list_models))
        .route("/v1/chat/completions", post(chat::chat_completions))
}