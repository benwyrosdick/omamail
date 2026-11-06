use crate::paths::AppPaths;
use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OauthConfig {
    pub client_id: String,
    pub client_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FileConfig {
    client_id: Option<String>,
    client_secret: Option<String>,
}

impl OauthConfig {
    /// Environment wins over `oauth.toml`. An empty value counts as unset.
    /// The secret is optional and is never required to be present in the repo.
    pub fn load(paths: &AppPaths) -> anyhow::Result<Self> {
        let mut client_id = nonempty_env("OMAMAIL_GOOGLE_CLIENT_ID");
        let mut client_secret = nonempty_env("OMAMAIL_GOOGLE_CLIENT_SECRET");

        if paths.oauth_file().is_file() {
            let raw = std::fs::read_to_string(paths.oauth_file())
                .with_context(|| format!("read {}", paths.oauth_file().display()))?;
            let file: FileConfig = toml::from_str(&raw)
                .with_context(|| format!("parse {}", paths.oauth_file().display()))?;
            if client_id.is_none() {
                client_id = file.client_id.filter(|s| !s.trim().is_empty());
            }
            if client_secret.is_none() {
                client_secret = file.client_secret.filter(|s| !s.trim().is_empty());
            }
        }

        let client_id = client_id.ok_or_else(|| {
            anyhow::anyhow!(
                "no Google OAuth client id. Set OMAMAIL_GOOGLE_CLIENT_ID or write {} — see docs/oauth.md",
                paths.oauth_file().display()
            )
        })?;
        Ok(Self {
            client_id,
            client_secret,
        })
    }

    pub fn is_configured(paths: &AppPaths) -> bool {
        Self::load(paths).is_ok()
    }
}

fn nonempty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap()
    }

    #[test]
    fn file_is_read_and_env_wins() {
        let _guard = env_lock();
        let root = std::env::temp_dir().join(format!("omamail-oauth-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let paths = AppPaths::for_root(&root);
        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(
            paths.oauth_file(),
            "client_id = \"from-file.apps.googleusercontent.com\"\nclient_secret = \"not-a-real-secret\"\n",
        )
        .unwrap();
        let loaded = OauthConfig::load(&paths).unwrap();
        assert_eq!(loaded.client_id, "from-file.apps.googleusercontent.com");
        assert!(loaded.client_secret.is_some());

        std::env::set_var(
            "OMAMAIL_GOOGLE_CLIENT_ID",
            "from-env.apps.googleusercontent.com",
        );
        let loaded = OauthConfig::load(&paths).unwrap();
        assert_eq!(loaded.client_id, "from-env.apps.googleusercontent.com");
        std::env::remove_var("OMAMAIL_GOOGLE_CLIENT_ID");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_config_is_an_error() {
        let _guard = env_lock();
        let root =
            std::env::temp_dir().join(format!("omamail-oauth-missing-{}", std::process::id()));
        let paths = AppPaths::for_root(&root);
        std::env::remove_var("OMAMAIL_GOOGLE_CLIENT_ID");
        std::env::remove_var("OMAMAIL_GOOGLE_CLIENT_SECRET");
        let err = OauthConfig::load(&paths).unwrap_err().to_string();
        assert!(err.contains("docs/oauth.md"));
    }
}
