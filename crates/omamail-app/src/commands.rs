use crate::oauth_config::OauthConfig;
use crate::theme::{current_palette, Palette};
use crate::Services;
use omamail_core::{MessageSummary, INBOX_LIMIT};
use omamail_google::{needs_reauth, GoogleClient};
use serde::Serialize;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub signed_in: bool,
    pub email: Option<String>,
    pub client_configured: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Session {
    pub email: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InboxItem {
    pub id: String,
    pub thread_id: String,
    pub from: String,
    pub subject: String,
    pub date_ms: Option<i64>,
    pub snippet: String,
    pub unread: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageView {
    pub id: String,
    pub thread_id: String,
    pub from: String,
    pub subject: String,
    pub date_ms: Option<i64>,
    pub snippet: String,
    pub unread: bool,
    pub plain_text: String,
    pub html: Option<String>,
}

struct AccessCache {
    email: String,
    access_token: String,
    _refresh_token: String,
    expires_at_ms: i64,
}

pub async fn theme() -> Palette {
    current_palette()
}

pub async fn status(services: &Services) -> Status {
    let client_configured = OauthConfig::is_configured(&services.paths);
    match signed_in_account(services).await {
        Ok(Some(account)) => Status {
            signed_in: true,
            email: Some(account.email),
            client_configured,
            error: None,
        },
        Ok(None) => Status {
            signed_in: false,
            email: None,
            client_configured,
            error: None,
        },
        Err(err) => Status {
            signed_in: false,
            email: None,
            client_configured,
            error: Some(err.to_string()),
        },
    }
}

pub async fn login(services: &Services) -> anyhow::Result<Session> {
    let config = OauthConfig::resolve(&services.paths);
    let config = config.ensure_secret(&services.paths)?;
    let signed_in =
        omamail_google::sign_in_via_browser(&config.client_id, config.client_secret.as_deref())
            .await?;
    let refresh = signed_in
        .tokens
        .refresh_token
        .clone()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Google did not return a refresh token. Remove OmaMail's access at https://myaccount.google.com/permissions and sign in again."
            )
        })?;
    services
        .tokens
        .set(&signed_in.email, &refresh)
        .await
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    omamail_store::upsert_account(
        &services.db,
        &signed_in.email,
        signed_in.google_sub.as_deref(),
        signed_in.display_name.as_deref(),
        "google",
    )
    .await?;
    cache_access(
        services,
        &signed_in.email,
        &signed_in.tokens.access_token,
        &refresh,
        signed_in.tokens.expires_at_ms,
    );
    sync(services).await?;
    Ok(Session {
        email: signed_in.email,
    })
}

pub async fn logout(services: &Services) -> anyhow::Result<()> {
    let accounts = omamail_store::accounts(&services.db).await?;
    for account in accounts {
        services
            .tokens
            .delete(&account.email)
            .await
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    }
    clear_cache(services);
    Ok(())
}

pub async fn sync(services: &Services) -> anyhow::Result<usize> {
    let account = require_account(services).await?;
    let access = access_token(services, &account.email).await?;
    let client = GoogleClient::new(access)?;
    omamail_sync::sync_inbox(&services.db, account.id, &client, INBOX_LIMIT)
        .await
        .map_err(|err| anyhow::anyhow!(err.to_string()))
}

pub async fn inbox(services: &Services) -> anyhow::Result<Vec<InboxItem>> {
    let Some(account) = signed_in_account(services).await? else {
        return Ok(Vec::new());
    };
    let messages = omamail_store::list_inbox(&services.db, account.id).await?;
    Ok(messages.into_iter().map(item_from).collect())
}

pub async fn message(services: &Services, id: &str) -> anyhow::Result<MessageView> {
    let account = require_account(services).await?;
    let summary = omamail_store::get_message(&services.db, account.id, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("message is not in the local inbox; run sync"))?;
    let cached = omamail_store::get_body(&services.db, account.id, id).await?;
    let body = if let Some(cached) = cached.filter(|body| body.html.is_some()) {
        cached
    } else {
        let access = access_token(services, &account.email).await?;
        let client = GoogleClient::new(access)?;
        let fetched = client
            .fetch_body(id)
            .await
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        omamail_store::put_body(
            &services.db,
            account.id,
            id,
            &fetched.plain_text,
            Some(fetched.html.as_deref().unwrap_or("")),
        )
        .await?;
        omamail_store::CachedBody {
            plain_text: fetched.plain_text,
            html: fetched.html,
        }
    };
    Ok(MessageView {
        id: summary.id,
        thread_id: summary.thread_id,
        from: summary.from,
        subject: summary.subject,
        date_ms: summary.date_ms,
        snippet: summary.snippet,
        unread: summary.unread,
        plain_text: body.plain_text,
        html: body.html.filter(|html| !html.is_empty()),
    })
}

struct SignedAccount {
    id: i64,
    email: String,
}

async fn require_account(services: &Services) -> anyhow::Result<SignedAccount> {
    signed_in_account(services)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not signed in"))
}

async fn signed_in_account(services: &Services) -> anyhow::Result<Option<SignedAccount>> {
    let accounts = omamail_store::accounts(&services.db).await?;
    for account in accounts {
        match services.tokens.get(&account.email).await {
            Ok(Some(_)) => {
                return Ok(Some(SignedAccount {
                    id: account.id,
                    email: account.email,
                }));
            }
            Ok(None) => continue,
            Err(err) => return Err(anyhow::anyhow!(err.to_string())),
        }
    }
    Ok(None)
}

async fn access_token(services: &Services, email: &str) -> anyhow::Result<String> {
    if let Some(token) = cached_access(services, email) {
        return Ok(token);
    }
    let refresh = services
        .tokens
        .get(email)
        .await
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .ok_or_else(|| anyhow::anyhow!("not signed in"))?;
    let config = OauthConfig::resolve(&services.paths);
    let tokens =
        match omamail_google::refresh(&config.client_id, config.client_secret.as_deref(), &refresh)
            .await
        {
            Ok(tokens) => tokens,
            Err(err) if needs_reauth(&err) => {
                let _ = services.tokens.delete(email).await;
                clear_cache(services);
                anyhow::bail!("Google rejected the saved sign-in. Run auth login again.");
            }
            Err(err) => return Err(err),
        };
    let next_refresh = tokens
        .refresh_token
        .clone()
        .unwrap_or_else(|| refresh.clone());
    if next_refresh != refresh {
        services
            .tokens
            .set(email, &next_refresh)
            .await
            .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    }
    cache_access(
        services,
        email,
        &tokens.access_token,
        &next_refresh,
        tokens.expires_at_ms,
    );
    Ok(tokens.access_token)
}

fn cache_slot() -> &'static Mutex<Option<AccessCache>> {
    static CACHE: std::sync::OnceLock<Mutex<Option<AccessCache>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn cache_access(services: &Services, email: &str, access: &str, refresh: &str, expires_at_ms: i64) {
    let _ = services;
    if let Ok(mut slot) = cache_slot().lock() {
        *slot = Some(AccessCache {
            email: email.to_string(),
            access_token: access.to_string(),
            _refresh_token: refresh.to_string(),
            expires_at_ms,
        });
    }
}

fn cached_access(services: &Services, email: &str) -> Option<String> {
    let _ = services;
    let slot = cache_slot().lock().ok()?;
    let cached = slot.as_ref()?;
    if cached.email != email {
        return None;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    if cached.expires_at_ms <= now {
        return None;
    }
    Some(cached.access_token.clone())
}

fn clear_cache(services: &Services) {
    let _ = services;
    if let Ok(mut slot) = cache_slot().lock() {
        *slot = None;
    }
}

fn item_from(message: MessageSummary) -> InboxItem {
    InboxItem {
        id: message.id,
        thread_id: message.thread_id,
        from: message.from,
        subject: message.subject,
        date_ms: message.date_ms,
        snippet: message.snippet,
        unread: message.unread,
    }
}
