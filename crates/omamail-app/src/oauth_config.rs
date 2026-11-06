use crate::paths::AppPaths;
use anyhow::Context;
use serde::Deserialize;
use std::path::Path;

/// Google Desktop client baked into the binary, same model as OmaCal.
/// An installed-app secret identifies the app. It is not a user password and
/// cannot be kept confidential once the binary ships. Users still consent as
/// themselves. Override with `oauth.toml` or the environment for a different
/// Cloud project.
const BUNDLED_CLIENT_ID: &str =
    "";
const BUNDLED_CLIENT_SECRET: &str = "";

#[derive(Clone, PartialEq, Eq)]
pub struct OauthConfig {
    pub client_id: String,
    pub client_secret: Option<String>,
}

impl std::fmt::Debug for OauthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OauthConfig")
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[derive(Debug, Deserialize)]
struct FileConfig {
    client_id: Option<String>,
    client_secret: Option<String>,
}

impl OauthConfig {
    pub fn bundled() -> Self {
        Self {
            client_id: BUNDLED_CLIENT_ID.to_string(),
            client_secret: Some(BUNDLED_CLIENT_SECRET.to_string()),
        }
    }

    /// Environment wins over `oauth.toml`. An empty value counts as unset.
    /// Does not fall back to the baked-in client; [`resolve`](Self::resolve) does.
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

    /// Baked-in client, unless this machine has an override. A config file
    /// written before the secret was baked in (client id only) still uses the
    /// baked-in secret when the id matches. The secret is not copied into the
    /// file.
    pub fn resolve(paths: &AppPaths) -> Self {
        let Ok(mut config) = Self::load(paths) else {
            return Self::bundled();
        };
        if config.client_secret.is_none() && config.client_id == BUNDLED_CLIENT_ID {
            config.client_secret = Some(BUNDLED_CLIENT_SECRET.to_string());
        }
        config
    }

    /// Google's token endpoint rejects this Desktop client without a secret.
    /// Ask once on a terminal and store it next to the client id. The secret
    /// is not compiled in and is not printed.
    pub fn ensure_secret(&self, paths: &AppPaths) -> anyhow::Result<Self> {
        if self.client_secret.is_some() {
            return Ok(self.clone());
        }
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            anyhow::bail!(
                "Google requires a client secret for this desktop client. Add client_secret to {} — Cloud Console → Credentials → this Desktop client — then run auth login again.",
                paths.oauth_file().display()
            );
        }
        eprintln!(
            "Google requires a client secret for this desktop client.\nIt will be saved to {} and will not be printed.",
            paths.oauth_file().display()
        );
        let secret = prompt_secret()?;
        let config = Self {
            client_id: self.client_id.clone(),
            client_secret: Some(secret),
        };
        config.save(paths)?;
        eprintln!("saved client secret");
        Ok(config)
    }

    pub fn save(&self, paths: &AppPaths) -> anyhow::Result<()> {
        std::fs::create_dir_all(&paths.config_dir)
            .with_context(|| format!("create {}", paths.config_dir.display()))?;
        let mut body = format!(
            "# Written by omamail auth login. Edit to use a different client.\n# client_secret stays in this file (mode 600). It is not stored in the repo.\nclient_id = \"{}\"\n",
            escape_toml(&self.client_id)
        );
        if let Some(secret) = &self.client_secret {
            body.push_str(&format!("client_secret = \"{}\"\n", escape_toml(secret)));
        }
        let path = paths.oauth_file();
        std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
        restrict_private(&path);
        Ok(())
    }

    pub fn is_configured(paths: &AppPaths) -> bool {
        !BUNDLED_CLIENT_ID.is_empty() || Self::load(paths).is_ok()
    }
}

fn prompt_secret() -> anyhow::Result<String> {
    let secret = rpassword::prompt_password("Client secret: ")
        .context("could not read the client secret")?;
    let secret = secret.trim().to_string();
    if secret.is_empty() {
        anyhow::bail!("client secret was empty");
    }
    Ok(secret)
}

fn escape_toml(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn restrict_private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
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

    #[test]
    fn resolve_uses_the_baked_in_client_without_writing_the_secret() {
        let _guard = env_lock();
        std::env::remove_var("OMAMAIL_GOOGLE_CLIENT_ID");
        std::env::remove_var("OMAMAIL_GOOGLE_CLIENT_SECRET");
        let root = std::env::temp_dir().join(format!(
            "omamail-oauth-resolve-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = AppPaths::for_root(&root);
        let resolved = OauthConfig::resolve(&paths);
        assert_eq!(resolved.client_id, OauthConfig::bundled().client_id);
        assert!(resolved.client_secret.is_some());
        assert!(!paths.oauth_file().exists());
        let rendered = format!("{resolved:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("GOCSPX-"));

        fs::create_dir_all(&paths.config_dir).unwrap();
        fs::write(
            paths.oauth_file(),
            format!("client_id = \"{}\"\n", OauthConfig::bundled().client_id),
        )
        .unwrap();
        let from_file = OauthConfig::resolve(&paths);
        assert!(from_file.client_secret.is_some());
        assert_eq!(
            from_file.client_secret,
            OauthConfig::bundled().client_secret
        );

        fs::write(
            paths.oauth_file(),
            "client_id = \"kept.apps.googleusercontent.com\"\n",
        )
        .unwrap();
        let override_id = OauthConfig::resolve(&paths);
        assert_eq!(override_id.client_id, "kept.apps.googleusercontent.com");
        assert!(override_id.client_secret.is_none());
        let _ = fs::remove_dir_all(root);
    }
}
