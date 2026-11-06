use anyhow::Context;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};

pub const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
pub const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// Readonly Gmail plus enough OpenID to learn the account email and subject.
/// Send, modify, and settings scopes are intentionally not requested.
pub const GMAIL_SCOPE: &str = "openid email https://www.googleapis.com/auth/gmail.readonly";

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub struct AuthParams<'a> {
    pub client_id: &'a str,
    pub redirect_uri: &'a str,
    pub challenge: &'a str,
    pub state: &'a str,
}

/// RFC 7636 S256. The verifier is 32 random bytes, base64url without padding.
pub fn generate_pkce() -> Pkce {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    Pkce {
        verifier,
        challenge,
    }
}

/// `prompt` is two values on purpose. `consent` plus `access_type=offline`
/// is what makes Google return a refresh token on a repeat sign-in.
/// `select_account` forces the account chooser, so "add account" does not
/// silently reuse the browser's only session.
pub fn authorize_url(params: &AuthParams<'_>) -> String {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", params.client_id)
        .append_pair("redirect_uri", params.redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", GMAIL_SCOPE)
        .append_pair("code_challenge", params.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", params.state)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent select_account")
        .finish();
    format!("{AUTH_ENDPOINT}?{query}")
}

#[derive(Clone)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at_ms: i64,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

pub struct SignedIn {
    pub tokens: Tokens,
    pub email: String,
    pub google_sub: Option<String>,
    pub display_name: Option<String>,
}

impl std::fmt::Debug for SignedIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedIn")
            .field("email", &self.email)
            .field("google_sub", &self.google_sub)
            .field("tokens", &self.tokens)
            .finish()
    }
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug)]
pub struct TokenRejected {
    pub code: String,
    description: String,
}

impl std::fmt::Display for TokenRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the token endpoint rejected the request: {}: {}",
            self.code, self.description
        )
    }
}

impl std::error::Error for TokenRejected {}

pub fn needs_reauth(err: &anyhow::Error) -> bool {
    err.downcast_ref::<TokenRejected>().is_some_and(|rejected| {
        matches!(
            rejected.code.as_str(),
            "invalid_grant" | "unauthorized_client" | "invalid_client"
        )
    })
}

pub(crate) async fn post_token(endpoint: &str, form: &[(&str, &str)]) -> anyhow::Result<Tokens> {
    let response = reqwest::Client::builder()
        .user_agent("omamail/0.1")
        .build()
        .context("build http client")?
        .post(endpoint)
        .form(form)
        .send()
        .await
        .context("token endpoint unreachable")?;

    let body: TokenResponse = response
        .json()
        .await
        .context("token response was not JSON")?;

    if let Some(code) = body.error {
        return Err(TokenRejected {
            code,
            description: body.error_description.unwrap_or_default(),
        }
        .into());
    }

    let access_token = body
        .access_token
        .context("token response had no access_token")?;
    let expires_in = body.expires_in.unwrap_or(3600);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock before unix epoch")?
        .as_millis() as i64;

    Ok(Tokens {
        access_token,
        refresh_token: body.refresh_token,
        expires_at_ms: now_ms + (expires_in - 60).max(0) * 1000,
    })
}

pub async fn exchange_code(
    token_endpoint: &str,
    client_id: &str,
    client_secret: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> anyhow::Result<Tokens> {
    let mut form = vec![
        ("client_id", client_id),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri),
    ];
    if !client_secret.is_empty() {
        form.push(("client_secret", client_secret));
    }
    post_token(token_endpoint, &form).await
}

pub async fn refresh_access(
    token_endpoint: &str,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> anyhow::Result<Tokens> {
    let mut form = vec![
        ("client_id", client_id),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    if !client_secret.is_empty() {
        form.push(("client_secret", client_secret));
    }
    let mut tokens = post_token(token_endpoint, &form).await?;
    if tokens.refresh_token.is_none() {
        tokens.refresh_token = Some(refresh_token.to_string());
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_asks_for_offline_gmail_and_hides_secrets() {
        let pkce = generate_pkce();
        let url = authorize_url(&AuthParams {
            client_id: "client.apps.googleusercontent.com",
            redirect_uri: "http://127.0.0.1:9",
            challenge: &pkce.challenge,
            state: "state-1",
        });
        assert!(url.starts_with(AUTH_ENDPOINT));
        assert!(url.contains("access_type=offline"));
        assert!(
            url.contains("prompt=consent+select_account")
                || url.contains("prompt=consent%20select_account")
        );
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("gmail.readonly"));
        assert!(!url.contains("client_secret"));
        assert!(!url.contains(&pkce.verifier));
    }

    #[test]
    fn token_debug_redacts_secrets() {
        let tokens = Tokens {
            access_token: "ya29.secret".into(),
            refresh_token: Some("1//refresh".into()),
            expires_at_ms: 10,
        };
        let rendered = format!("{tokens:?}");
        assert!(rendered.contains("<redacted>"));
        assert!(!rendered.contains("ya29.secret"));
        assert!(!rendered.contains("1//refresh"));
    }

    #[test]
    fn dead_grant_is_reauth() {
        let err = anyhow::Error::from(TokenRejected {
            code: "invalid_grant".into(),
            description: "expired".into(),
        });
        assert!(needs_reauth(&err));
        let other = anyhow::Error::msg("token endpoint unreachable");
        assert!(!needs_reauth(&other));
    }
}
