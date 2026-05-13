use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::APP_ID;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Tokens {
    pub client_secret: String,
    pub refresh_token: String,
    pub access_token: String,
    pub expires_at_unix: u64,
}

impl Tokens {
    pub fn is_access_token_fresh(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        self.expires_at_unix > now + 30
    }
}

#[derive(Debug, Error)]
pub enum SecretsError {
    #[error("secret service unavailable: {0}")]
    Backend(String),
    #[error("no credentials stored for this account")]
    NotFound,
    #[error("malformed stored credentials: {0}")]
    Decode(String),
}

impl From<keyring::Error> for SecretsError {
    fn from(e: keyring::Error) -> Self {
        match e {
            keyring::Error::NoEntry => SecretsError::NotFound,
            other => SecretsError::Backend(other.to_string()),
        }
    }
}

fn entry(email: &str) -> Result<keyring::Entry, SecretsError> {
    keyring::Entry::new(APP_ID, email).map_err(SecretsError::from)
}

pub async fn load(email: &str) -> Result<Tokens, SecretsError> {
    let email = email.to_owned();
    tokio::task::spawn_blocking(move || {
        let blob = entry(&email)?.get_password()?;
        serde_json::from_str::<Tokens>(&blob).map_err(|e| SecretsError::Decode(e.to_string()))
    })
    .await
    .map_err(|e| SecretsError::Backend(e.to_string()))?
}

pub async fn save(email: &str, tokens: &Tokens) -> Result<(), SecretsError> {
    let email = email.to_owned();
    let blob = serde_json::to_string(tokens).map_err(|e| SecretsError::Decode(e.to_string()))?;
    tokio::task::spawn_blocking(move || -> Result<(), SecretsError> {
        entry(&email)?.set_password(&blob)?;
        Ok(())
    })
    .await
    .map_err(|e| SecretsError::Backend(e.to_string()))?
}

