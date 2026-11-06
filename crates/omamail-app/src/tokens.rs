use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

const SERVICE: &str = "omamail";

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("keyring unavailable: {0}")]
    Unavailable(String),
}

#[async_trait]
pub trait TokenStore: Send + Sync {
    async fn get(&self, account: &str) -> Result<Option<String>, TokenError>;
    async fn set(&self, account: &str, secret: &str) -> Result<(), TokenError>;
    async fn delete(&self, account: &str) -> Result<(), TokenError>;
}

/// OS keyring. The secret is the refresh token. Access tokens are not stored.
pub struct KeyringStore;

#[async_trait]
impl TokenStore for KeyringStore {
    async fn get(&self, account: &str) -> Result<Option<String>, TokenError> {
        let account = account.to_string();
        spawn(move || match entry(&account)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(err) if is_missing(&err) => Ok(None),
            Err(err) => Err(TokenError::Unavailable(err.to_string())),
        })
        .await
    }

    async fn set(&self, account: &str, secret: &str) -> Result<(), TokenError> {
        let account = account.to_string();
        let secret = secret.to_string();
        spawn(move || {
            entry(&account)?
                .set_password(&secret)
                .map_err(|err| TokenError::Unavailable(err.to_string()))
        })
        .await
    }

    async fn delete(&self, account: &str) -> Result<(), TokenError> {
        let account = account.to_string();
        spawn(move || match entry(&account)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(err) if is_missing(&err) => Ok(()),
            Err(err) => Err(TokenError::Unavailable(err.to_string())),
        })
        .await
    }
}

fn entry(account: &str) -> Result<keyring::Entry, TokenError> {
    keyring::Entry::new(SERVICE, account).map_err(|err| TokenError::Unavailable(err.to_string()))
}

fn is_missing(err: &keyring::Error) -> bool {
    matches!(err, keyring::Error::NoEntry)
}

async fn spawn<T, F>(work: F) -> Result<T, TokenError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, TokenError> + Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(err) => Err(TokenError::Unavailable(err.to_string())),
    }
}

#[derive(Default)]
pub struct MemoryTokenStore {
    inner: Mutex<HashMap<String, String>>,
}

#[async_trait]
impl TokenStore for MemoryTokenStore {
    async fn get(&self, account: &str) -> Result<Option<String>, TokenError> {
        Ok(self
            .inner
            .lock()
            .map_err(|err| TokenError::Unavailable(err.to_string()))?
            .get(account)
            .cloned())
    }

    async fn set(&self, account: &str, secret: &str) -> Result<(), TokenError> {
        self.inner
            .lock()
            .map_err(|err| TokenError::Unavailable(err.to_string()))?
            .insert(account.to_string(), secret.to_string());
        Ok(())
    }

    async fn delete(&self, account: &str) -> Result<(), TokenError> {
        self.inner
            .lock()
            .map_err(|err| TokenError::Unavailable(err.to_string()))?
            .remove(account);
        Ok(())
    }
}
