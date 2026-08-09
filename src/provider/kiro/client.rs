use std::sync::LazyLock;

use axum::http::{HeaderMap, HeaderValue};
use reqwest::header::USER_AGENT;
use serde::Deserialize;
use uuid::Uuid;

use crate::{
    provider::kiro::credential::KiroToken, 
    types::kiro_response::ModelsResponse
};



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