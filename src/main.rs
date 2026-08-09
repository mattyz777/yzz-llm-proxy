use anyhow::Result;

use crate::{config::AppConfig, state::AppState, tracer::init_tracing};

mod config;
mod constant;
mod route;
mod provider;
mod state;
mod tracer;
mod types;


#[tokio::main]
async fn main() -> Result<()> {
    let guard = init_tracing();

    if let Err(e) = run().await {
        tracing::error!("Fatal: {e}");
        drop(guard);
    }

    Ok(())
}

async fn run() -> Result<()> {
    let config = AppConfig::init()?;

    let state = AppState::init(&config).await?;

    let app = route::routes().with_state(state);

    let listener = tokio::net::TcpListener::bind(&config.server.listen).await?;
    tracing::info!("Server will listen on {}", config.server.listen);
    axum::serve(listener, app).await?;

    Ok(())
}