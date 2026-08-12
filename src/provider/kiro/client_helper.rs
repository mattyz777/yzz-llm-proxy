use std::sync::{Arc, LazyLock};

use axum::http::{HeaderMap, HeaderValue};
use reqwest::{StatusCode, header::USER_AGENT};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::provider::kiro::credential::KiroToken;


static MACHINE_ID: LazyLock<String> = LazyLock::new(|| {
    let a = Uuid::new_v4().simple().to_string();
    let b = Uuid::new_v4().simple().to_string();
    format!("{a}{b}")
});



pub fn get_kiro_headers() -> HeaderMap {
    /***
    | UA(user-agent) part        | Source                     |
    | -------------------------- | -------------------------- |
    | aws-sdk-js/1.0.27          | AWS SDK                    |
    | os/win32                   | Node os module             |
    | md/nodejs#22.21.1          | Node runtime               |
    | api/codewhispererstreaming | AWS client package         |
    | KiroIDE-0.7.45             | Kiro application custom UA |
     */
    let ua = format!(
        "aws-sdk-js/1.0.27 ua/2.1 os/win32#10.0.19044 lang/js md/nodejs#22.21.1 api/codewhispererstreaming#1.0.27 m/E KiroIDE-0.7.45-{}",
        *MACHINE_ID
    );
    let x_amz_ua = format!(
        "aws-sdk-js/1.0.27 KiroIDE-0.7.45-{}",
        *MACHINE_ID
    );

    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_str(&ua).unwrap());
    headers.insert("x-amz-user-agent", HeaderValue::from_str(&x_amz_ua).unwrap());
    headers.insert("content-type", HeaderValue::from_static("application/x-amz-json-1.0"));
    headers
}



pub async fn get_valid_token(
    token: &Arc<RwLock<KiroToken>>,
    http: &reqwest::Client,
) -> Result<String, StatusCode> {
    {
        let t = token.read().await;
        if !t.is_expired() {
            return Ok(t.access_token.clone());
        }
    }

    let mut t = token.write().await;

    // Double-check after acquiring write lock (another request may have refreshed)
    if !t.is_expired() {
        return Ok(t.access_token.clone());
    }

    t.refresh(http).await.map_err(|e| {
        tracing::error!("Token refresh failed: {e}");
        StatusCode::UNAUTHORIZED
    })?;

    Ok(t.access_token.clone())
}

