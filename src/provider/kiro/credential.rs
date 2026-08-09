use anyhow::{anyhow, Result};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;
use std::path::PathBuf;


#[derive(Clone, Debug, Deserialize)]
pub struct KiroToken {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: String,
    pub region: String,
}

#[derive(Debug, Deserialize)]
struct DeviceRegistration {
    client_id: String,
    client_secret: String,
    region: String,
}


#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefreshTokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: i64,
}

#[derive(Debug, Deserialize)]
struct ProfileData {
    arn: String,
}


impl KiroToken {
    fn db_path() -> Result<PathBuf> {
        #[cfg(target_os = "windows")]
        let dir = dirs::data_local_dir(); // %LOCALAPPDATA%

        #[cfg(not(target_os = "windows"))]
        let dir = dirs::data_dir();       // ~/Library/Application Support

        Ok(
            dir.ok_or_else(|| anyhow!("Failed to get data directory"))?
                .join("kiro-cli")
                .join("data.sqlite3")
        )
    }

    fn get_connection() -> Result<Connection> {
        let db = Self::db_path()?;
        let conn = rusqlite::Connection::open_with_flags(
            &db, 
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
        )?;
        Ok(conn)
    }

    fn read_tb_auth_kv(key: &str) -> Result<Option<String>> {
        let conn = Self::get_connection()?;
        let data = conn.query_row(
            "SELECT value FROM auth_kv WHERE key = ?1",
            [key],
            |row| row.get(0),
        ).optional()?;
        Ok(data)
    }

    fn read_tb_state(key: &str) -> Result<Option<String>> {
        let conn = Self::get_connection()?;
        let data = conn.query_row(
            "SELECT value FROM state WHERE key = ?1",
            [key],
            |row| row.get(0),
        ).optional()?;
        Ok(data)
    }

    fn load_token() -> Result<Self> {
        let keys = [
            "kirocli:odic:token",
            "kirocli:social:token",
            "codewhisperer:odic:token",
        ];

        for key in keys {
            let Some(value) = Self::read_tb_auth_kv(key)? else {
                continue;
            };

            match serde_json::from_str::<Self>(&value) {
                Ok(token) => return Ok(token),
                Err(e) => {
                    tracing::warn!("Invalid token data in {key}: {e}");
                }
            }
        }

        Err(anyhow!("No valid token found"))
    }

    fn load_device_registration() -> Result<DeviceRegistration> {
        let keys = [
            "kirocli:odic:device-registration",
            "codewhisperer:odic:device-registration",
        ];
    
        for key in keys {
            let Some(value) = Self::read_tb_auth_kv(key)? else {
                continue;
            };
            match serde_json::from_str::<DeviceRegistration>(&value) {
                Ok(dr) => {
                    return Ok(dr);
                },
                Err(e) => {
                    tracing::warn!("Invalid token data in {key}: {e}");
                }
            }
        }

        Err(anyhow!("No valid device registration"))
    }

    pub fn is_expired(&self) -> bool {
        chrono::DateTime::parse_from_rfc3339(&self.expires_at)
            .map(|dt| Utc::now() >= dt)
            .unwrap_or(true)
    }

    fn compute_expiry(new_expired_at: i64) -> String {
        (Utc::now() + chrono::TimeDelta::seconds(new_expired_at)).to_rfc3339()
    }

    pub fn load_profile_arn() -> Result<String> {
        let value = Self::read_tb_state("api.codewhisperer.profile")?
            .ok_or_else(|| anyhow!("profile arn not found"))?;
        let profile: ProfileData = serde_json::from_str(&value)?;
        Ok(profile.arn)
    }

    pub async fn refresh(&mut self, http: &reqwest::Client) -> Result<()> {
        let reg = Self::load_device_registration()?;

        let body = serde_json::json!({
            "grantType": "refresh_token",
            "clientId": reg.client_id,
            "clientSecret": reg.client_secret,
            "refreshToken": self.refresh_token,
        });

        let url = format!("https://oidc.{}.amazonaws.com/token", &reg.region);

        let resp = http
            .post(url)
            .json(&body)
            .send()
            .await?;
        
        let status = resp.status();
        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("token refresh failed: {status} - {text}");
        }

        let res: RefreshTokenResponse = resp.json().await?;
        self.access_token = res.access_token;
        if let Some(rt) = res.refresh_token {
            self.refresh_token = rt;
        }
        self.expires_at = Self::compute_expiry(res.expires_in);
        tracing::info!("Kiro token refreshed, expires at {}", self.expires_at);

        Ok(())
    }

    pub async fn load_refresh(http: &reqwest::Client) -> Result<Self> {
        let mut token = Self::load_token()?;
        if token.is_expired() {
            token.refresh(http).await?;
        }
        Ok(token)
    }
}