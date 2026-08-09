use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde::Deserialize;

use crate::constant::DEFAULT_CONFIG_CONTENT;


#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub accounts: Option<Vec<AccountConfig>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ServerConfig {
    pub listen: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "provider")]
pub enum AccountConfig {
    #[serde(rename = "kiro")]
    Kiro { },

    #[serde(rename = "dashscope")]
    DashScope {
        base_url: String,
        api_key: String,
    },
}

impl Config {
    pub fn init() -> Result<Self> {
        let path = Self::get_config_path()?;
        if !path.exists() {
            Self::create_default(&path)?;
            tracing::warn!(
                "Config file created at: {}  Please fill in real configuration and restart the program.",
                path.display()
            );
            std::process::exit(0);
        }
        
        let content = std::fs::read_to_string(&path)?;
        let config: Config = toml::from_str(&content)?;

        Self::print_load_providers(&config);

        Ok(config)
    }

    fn print_load_providers(config: &Config) -> String {
        let providers:String = match &config.accounts {
            Some(accounts) => {
                accounts.iter().map(|account| {
                    match account {
                        AccountConfig::Kiro {} => "kiro",
                        AccountConfig::DashScope { .. } => "dashscope",
                    }
                })
                .collect::<Vec<_>>()
                .join(",")
            }
            None => {
                "".into()
            }
        };

        let providers = if providers.is_empty() {
            "Not provider configed.".into()
        } else {
            providers
        };

        tracing::info!("loaded providers: {}",  providers);
        providers
    }

    fn get_config_path() -> Result<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            dirs::data_dir() // %APPDATA%\Roaming
                .map(|d| d.join(".yzz-llm-proxy").join("config.toml"))
                .ok_or_else(||{
                    tracing::error!("cannot determine windows data directory");
                    anyhow!("cannot determine windows data directory")
                })
        }

        #[cfg(not(target_os = "windows"))]
        {
            dirs::home_dir() // ~
                .map(|d| d.join(".yzz-llm-proxy").join("config.toml"))
                .ok_or_else(||{
                    tracing::error!("cannot determine home directory");
                    anyhow!("cannot determine home directory")
                })
        }
    }

    fn create_default(path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, DEFAULT_CONFIG_CONTENT)?;
        Ok(())
    }
}