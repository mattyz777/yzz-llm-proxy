use anyhow::Result;
use std::{collections::HashMap, sync::Arc};

use reqwest::Client;
use tokio::sync::RwLock;

use crate::{
    config::{AccountConfig, AppConfig}, 
    provider::kiro::{client::list_models, credential::KiroToken}
};


#[derive(Clone, Debug)]
pub enum ProviderContext {
    Agnes {
        base_url: String, 
        api_key: String,
    },
    DashScope {
        base_url: String, 
        api_key: String,
    },
    Kiro {
        token: Arc<RwLock<KiroToken>>, // update token after refreshing
        profile_arn: String,
        region: String,
    }, 
}


#[derive(Clone, Debug)]
pub struct AppState {
    pub http: reqwest::Client,
    pub models: Vec<String>,  // All models: "kiro/glm-5", "dashscope/opus-5".
    pub providers: HashMap<String, ProviderContext>,
}

impl AppState {
    pub async fn init(config: &AppConfig) -> Result<Self> {
        let mut providers: HashMap<String, ProviderContext> = HashMap::new();
        let mut models = vec!();
        let http = Client::new();

        let Some(accounts) = &config.accounts else {
            anyhow::bail!("No LLM provider is set");
        };

        for account in accounts {
            match account {
                AccountConfig::Kiro {  } => {
                    let token = KiroToken::load_refresh(&http).await?;
                    let profile_arn = KiroToken::load_profile_arn()?;
                    let kiro_models = list_models(&http, &token, &profile_arn).await?;
                    let provider = "kiro".to_string();

                    models.extend(kiro_models.iter().map(|m| 
                        format!("{}/{}", provider, m))
                    );

                    providers.insert(provider, ProviderContext::Kiro { 
                        region: token.region.clone(),
                        token: Arc::new(RwLock::new(token)),
                        profile_arn
                    });
                },
                AccountConfig::DashScope { base_url, api_key } => {
                    anyhow::bail!("account is not supported: {:?}",  account);
                },
            }
        }

        Ok(Self { http, providers, models })
    }
}