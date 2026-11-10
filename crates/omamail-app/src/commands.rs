use crate::jev::{self, Category};
use crate::oauth_config::OauthConfig;
use crate::theme::{current_palette, Palette};
use crate::Services;
use omamail_core::{MessageSummary, INBOX_LIMIT};
use omamail_google::{needs_reauth, GoogleClient};
use omamail_store::StoredLabel;
use serde::Serialize;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub signed_in: bool,
    pub email: Option<String>,
    pub accounts: Vec<String>,
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
    pub category: Option<String>,
    pub tags: Vec<Tag>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Tag {
    pub name: String,
    pub color: Option<String>,
    pub kind: String,
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
    pub tags: Vec<Tag>,
    pub unsubscribe_url: Option<String>,
    pub unsubscribe_one_click: bool,
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
    match signed_in_accounts(services).await {
        Ok(accounts) => {
            let active = signed_in_account(services).await.ok().flatten();
            Status {
                signed_in: active.is_some(),
                email: active.map(|account| account.email),
                accounts: accounts.into_iter().map(|account| account.email).collect(),
                client_configured,
                error: None,
            }
        }
        Err(err) => Status {
            signed_in: false,
            email: None,
            accounts: Vec::new(),
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
    omamail_store::set_setting(&services.db, "active_account", &signed_in.email).await?;
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

pub async fn logout(services: &Services, email: Option<&str>) -> anyhow::Result<()> {
    let email = match email {
        Some(email) => email.to_string(),
        None => {
            require_account(services).await?.email
        }
    };
    services
        .tokens
        .delete(&email)
        .await
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    clear_cache(services);
    let remaining = signed_in_accounts(services).await?;
    if let Some(next) = remaining.first() {
        omamail_store::set_setting(&services.db, "active_account", &next.email).await?;
    } else {
        omamail_store::set_setting(&services.db, "active_account", "").await?;
    }
    Ok(())
}

pub async fn set_active_account(services: &Services, email: &str) -> anyhow::Result<()> {
    let accounts = signed_in_accounts(services).await?;
    if !accounts.iter().any(|account| account.email == email) {
        anyhow::bail!("that account is not signed in");
    }
    omamail_store::set_setting(&services.db, "active_account", email).await?;
    clear_cache(services);
    Ok(())
}

pub async fn sync(services: &Services) -> anyhow::Result<usize> {
    let account = require_account(services).await?;
    let access = access_token(services, &account.email).await?;
    let client = GoogleClient::new(access)?;
    let key = history_key(account.id);
    let start = omamail_store::get_setting(&services.db, &key)
        .await?
        .filter(|start| !start.is_empty());
    let delta = match start {
        Some(start) => client
            .history_since(&start)
            .await
            .map_err(|err| anyhow::anyhow!(err.to_string()))?,
        None => None,
    };
    let changed = match delta {
        Some(delta) if delta.changes.len() <= HISTORY_REFETCH_LIMIT => {
            apply_history(services, &client, account.id, delta).await?
        }
        // No history yet, history expired, or so much changed that a fresh
        // first page is cheaper than refetching every message.
        _ => {
            let history_id = client
                .history_id()
                .await
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            let page = load_gmail_page(services, &client, account.id, None, None).await?;
            omamail_store::set_setting(&services.db, &key, &history_id).await?;
            page.added
        }
    };
    if let Err(err) = refresh_labels(services, &client, account.id).await {
        eprintln!("omamail: could not refresh Gmail labels: {err}");
    }
    Ok(changed)
}

/// Past this many changed messages, a full first-page reload beats refetching each one.
const HISTORY_REFETCH_LIMIT: usize = 200;

fn history_key(account_id: i64) -> String {
    format!("history_id:{account_id}")
}

/// Bring the cache up to date from a history delta, then advance the history id.
/// Only inbox mail and messages already cached are stored; the rest is skipped.
async fn apply_history(
    services: &Services,
    client: &GoogleClient,
    account_id: i64,
    delta: omamail_google::HistoryDelta,
) -> anyhow::Result<usize> {
    let mut refetch = Vec::new();
    let mut changed = 0;
    for (id, labels) in delta.changes {
        let cached = omamail_store::get_message(&services.db, account_id, &id)
            .await?
            .is_some();
        match labels {
            None => {
                if cached {
                    omamail_store::remove_message(&services.db, account_id, &id).await?;
                    changed += 1;
                }
            }
            Some(labels) => {
                let inbox = labels.is_empty() || labels.iter().any(|label| label == "INBOX");
                if cached || inbox {
                    refetch.push((id, cached));
                }
            }
        }
    }
    let ids: Vec<&str> = refetch.iter().map(|(id, _)| id.as_str()).collect();
    let fetched = client
        .message_summaries(&ids)
        .await
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let mut fresh = Vec::new();
    for ((id, cached), summary) in refetch.into_iter().zip(fetched) {
        match summary {
            Some(summary) if cached || summary.label_ids.iter().any(|label| label == "INBOX") => {
                fresh.push(summary);
            }
            Some(_) => {}
            None if cached => {
                omamail_store::remove_message(&services.db, account_id, &id).await?;
                changed += 1;
            }
            None => {}
        }
    }
    changed += omamail_store::upsert_messages(&services.db, account_id, &fresh).await?;
    omamail_store::set_setting(&services.db, &history_key(account_id), &delta.history_id).await?;
    Ok(changed)
}

#[derive(Debug, Clone, Serialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
}

pub async fn folders(services: &Services) -> anyhow::Result<Vec<Folder>> {
    let Some(account) = signed_in_account(services).await? else {
        return Ok(Vec::new());
    };
    let mut folders = omamail_store::gmail_labels(&services.db, account.id)
        .await?
        .into_iter()
        .filter(|label| label.id.starts_with("Label_"))
        .map(|label| Folder {
            id: label.id,
            name: label.name,
        })
        .collect::<Vec<_>>();
    folders.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(folders)
}

pub async fn act(
    services: &Services,
    kind: &str,
    id: &str,
    label_id: Option<&str>,
) -> anyhow::Result<()> {
    let account = require_account(services).await?;
    let access = access_token(services, &account.email).await?;
    let client = GoogleClient::new(access)?;
    match kind {
        "archive" => {
            client
                .modify(id, &[], &["INBOX"])
                .await
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            omamail_store::remove_message(&services.db, account.id, id).await?;
        }
        "trash" => {
            client
                .trash(id)
                .await
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            omamail_store::remove_message(&services.db, account.id, id).await?;
        }
        "spam" => {
            client
                .modify(id, &["SPAM"], &["INBOX"])
                .await
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            omamail_store::remove_message(&services.db, account.id, id).await?;
        }
        "read" => {
            client
                .modify(id, &[], &["UNREAD"])
                .await
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            omamail_store::mark_read(&services.db, account.id, id).await?;
        }
        "move" => {
            let label = label_id.ok_or_else(|| anyhow::anyhow!("choose a folder"))?;
            if !label.starts_with("Label_") {
                anyhow::bail!("that is not a mail folder");
            }
            client
                .modify(id, &[label], &["INBOX"])
                .await
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
            omamail_store::remove_message(&services.db, account.id, id).await?;
        }
        other => anyhow::bail!("unknown action {other}"),
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct MailPage {
    pub messages: Vec<InboxItem>,
    pub next_page_token: Option<String>,
}

pub async fn load_more(
    services: &Services,
    query: Option<String>,
    page_token: Option<String>,
) -> anyhow::Result<MailPage> {
    let account = require_account(services).await?;
    let access = access_token(services, &account.email).await?;
    let client = GoogleClient::new(access)?;
    let query = query.as_deref().map(str::trim).filter(|query| !query.is_empty());
    let token = page_token.as_deref().filter(|token| !token.is_empty());
    let page = load_gmail_page(services, &client, account.id, query, token).await?;
    let labels = omamail_store::gmail_labels(&services.db, account.id).await?;
    let categories = load_categories(services, account.id).await?;
    let messages = page
        .messages
        .into_iter()
        .map(|message| item_from(message, &labels, &categories))
        .collect();
    Ok(MailPage {
        messages,
        next_page_token: page.next_page_token,
    })
}

struct LoadedPage {
    added: usize,
    messages: Vec<MessageSummary>,
    next_page_token: Option<String>,
}

async fn load_gmail_page(
    services: &Services,
    client: &GoogleClient,
    account_id: i64,
    query: Option<&str>,
    page_token: Option<&str>,
) -> anyhow::Result<LoadedPage> {
    let page = client
        .list_page(query, page_token, INBOX_LIMIT)
        .await
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    // A fresh inbox fetch is the new top of the mailbox. Anything still
    // cached as inbox but absent from this page was deleted or archived in Gmail.
    if query.is_none() && page_token.is_none() {
        omamail_store::retain_inbox(&services.db, account_id, &page.messages).await?;
    }
    let added = omamail_store::upsert_messages(&services.db, account_id, &page.messages).await?;
    if query.is_none() {
        omamail_store::set_setting(
            &services.db,
            &format!("inbox_page_token:{account_id}"),
            page.next_page_token.as_deref().unwrap_or(""),
        )
        .await?;
    }
    Ok(LoadedPage {
        added,
        messages: page.messages,
        next_page_token: page.next_page_token,
    })
}

pub async fn inbox_page_token(services: &Services) -> anyhow::Result<Option<String>> {
    let Some(account) = signed_in_account(services).await? else {
        return Ok(None);
    };
    let token = omamail_store::get_setting(
        &services.db,
        &format!("inbox_page_token:{}", account.id),
    )
    .await?;
    Ok(token.filter(|token| !token.is_empty()))
}

pub async fn inbox(services: &Services) -> anyhow::Result<Vec<InboxItem>> {
    let Some(account) = signed_in_account(services).await? else {
        return Ok(Vec::new());
    };
    let labels = labels_for(services, account.id).await;
    let categories = load_categories(services, account.id).await?;
    let messages = omamail_store::list_inbox(&services.db, account.id).await?;
    Ok(messages
        .into_iter()
        .map(|message| item_from(message, &labels, &categories))
        .collect())
}

pub async fn message(services: &Services, id: &str) -> anyhow::Result<MessageView> {
    let account = require_account(services).await?;
    let summary = omamail_store::get_message(&services.db, account.id, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("message is not in the local inbox; run sync"))?;
    let tags = tags_for(
        &summary,
        &labels_for(services, account.id).await,
        &load_categories(services, account.id).await?,
    );
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
            fetched.unsubscribe_url.as_deref(),
            fetched.unsubscribe_one_click,
        )
        .await?;
        omamail_store::CachedBody {
            plain_text: fetched.plain_text,
            html: fetched.html,
            unsubscribe_url: fetched.unsubscribe_url,
            unsubscribe_one_click: fetched.unsubscribe_one_click,
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
        tags,
        unsubscribe_url: body
            .unsubscribe_url
            .filter(|url| url.starts_with("https://")),
        unsubscribe_one_click: body.unsubscribe_one_click,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct UnsubscribeResult {
    pub one_click: bool,
    pub url: Option<String>,
}

pub async fn unsubscribe(services: &Services, id: &str) -> anyhow::Result<UnsubscribeResult> {
    let account = require_account(services).await?;
    let body = omamail_store::get_body(&services.db, account.id, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("open the message before unsubscribing"))?;
    let url = body
        .unsubscribe_url
        .filter(|url| url.starts_with("https://") && !url.chars().any(char::is_whitespace))
        .ok_or_else(|| anyhow::anyhow!("this message has no unsubscribe link"))?;
    if !body.unsubscribe_one_click {
        return Ok(UnsubscribeResult {
            one_click: false,
            url: Some(url),
        });
    }
    let http = reqwest::Client::builder()
        .user_agent("omamail/0.1")
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let response = http
        .post(&url)
        .header("content-type", "application/x-www-form-urlencoded")
        .body("List-Unsubscribe=One-Click")
        .send()
        .await
        .map_err(|err| anyhow::anyhow!("unsubscribe request failed: {err}"))?;
    if !response.status().is_success() {
        anyhow::bail!("the sender refused the unsubscribe ({})", response.status().as_u16());
    }
    Ok(UnsubscribeResult {
        one_click: true,
        url: None,
    })
}

#[derive(Clone)]
struct SignedAccount {
    id: i64,
    email: String,
}

async fn require_account(services: &Services) -> anyhow::Result<SignedAccount> {
    signed_in_account(services)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not signed in"))
}

async fn signed_in_accounts(services: &Services) -> anyhow::Result<Vec<SignedAccount>> {
    let mut signed_in = Vec::new();
    for account in omamail_store::accounts(&services.db).await? {
        match services.tokens.get(&account.email).await {
            Ok(Some(_)) => signed_in.push(SignedAccount {
                id: account.id,
                email: account.email,
            }),
            Ok(None) => continue,
            Err(err) => return Err(anyhow::anyhow!(err.to_string())),
        }
    }
    Ok(signed_in)
}

async fn signed_in_account(services: &Services) -> anyhow::Result<Option<SignedAccount>> {
    let accounts = signed_in_accounts(services).await?;
    let active = omamail_store::get_setting(&services.db, "active_account").await?;
    if let Some(email) = active.filter(|email| !email.is_empty()) {
        if let Some(account) = accounts.iter().find(|account| account.email == email) {
            return Ok(Some(account.clone()));
        }
    }
    if let Some(account) = accounts.first() {
        omamail_store::set_setting(&services.db, "active_account", &account.email).await?;
        return Ok(Some(account.clone()));
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

const TYPESAFE_ACCOUNT: &str = "typesafe";
const SETTING_REFRESH: &str = "refresh_minutes";
const DEFAULT_REFRESH_MINUTES: u32 = 10;
const MAX_REFRESH_MINUTES: u32 = 24 * 60;
fn jev_key(account_id: i64) -> String {
    format!("jev_enabled:{account_id}")
}

fn categories_key(account_id: i64) -> String {
    format!("categories:{account_id}")
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MailSettings {
    pub key_configured: bool,
    pub jev_enabled: bool,
    pub categories: Vec<Category>,
    pub account: Option<String>,
    /// Minutes between background inbox refreshes; 0 turns polling off.
    pub refresh_minutes: u32,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SettingsUpdate {
    pub api_key: Option<String>,
    pub jev_enabled: bool,
    pub categories: Vec<Category>,
    #[serde(default)]
    pub refresh_minutes: Option<u32>,
}

pub async fn mail_settings(services: &Services) -> anyhow::Result<MailSettings> {
    let key_configured = services
        .tokens
        .get(TYPESAFE_ACCOUNT)
        .await
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .is_some();
    let account = signed_in_account(services).await?;
    let (jev_enabled, categories) = if let Some(account) = &account {
        (
            account_jev_enabled(services, account.id).await?,
            load_categories(services, account.id).await?,
        )
    } else {
        (false, jev::default_categories())
    };
    let refresh_minutes = omamail_store::get_setting(&services.db, SETTING_REFRESH)
        .await?
        .and_then(|raw| raw.parse::<u32>().ok())
        .unwrap_or(DEFAULT_REFRESH_MINUTES)
        .min(MAX_REFRESH_MINUTES);
    Ok(MailSettings {
        key_configured,
        jev_enabled,
        categories,
        account: account.map(|account| account.email),
        refresh_minutes,
    })
}

pub async fn save_settings(services: &Services, update: SettingsUpdate) -> anyhow::Result<MailSettings> {
    let categories = normalize_categories(update.categories)?;
    if let Some(key) = update.api_key {
        let key = key.trim().to_string();
        if key.is_empty() {
            services
                .tokens
                .delete(TYPESAFE_ACCOUNT)
                .await
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        } else {
            services
                .tokens
                .set(TYPESAFE_ACCOUNT, &key)
                .await
                .map_err(|err| anyhow::anyhow!(err.to_string()))?;
        }
    }
    if let Some(minutes) = update.refresh_minutes {
        let minutes = minutes.min(MAX_REFRESH_MINUTES).to_string();
        omamail_store::set_setting(&services.db, SETTING_REFRESH, &minutes).await?;
    }
    if let Some(account) = signed_in_account(services).await? {
        omamail_store::set_setting(
            &services.db,
            &jev_key(account.id),
            if update.jev_enabled { "1" } else { "0" },
        )
        .await?;
        let encoded = serde_json::to_string(&categories)?;
        omamail_store::set_setting(&services.db, &categories_key(account.id), &encoded).await?;
    }
    mail_settings(services).await
}

pub async fn classify(services: &Services, all: bool) -> anyhow::Result<usize> {
    let settings = mail_settings(services).await?;
    if settings.categories.is_empty() {
        anyhow::bail!("add at least one category before sorting");
    }
    let api_key = services
        .tokens
        .get(TYPESAFE_ACCOUNT)
        .await
        .map_err(|err| anyhow::anyhow!(err.to_string()))?
        .ok_or_else(|| anyhow::anyhow!("add a TypeSafe API key in settings"))?;
    let Some(account) = signed_in_account(services).await? else {
        anyhow::bail!("not signed in");
    };
    let pending: Vec<MessageSummary> = omamail_store::list_inbox(&services.db, account.id)
        .await?
        .into_iter()
        .filter(|message| all || message.category.is_none())
        .collect();
    if pending.is_empty() {
        return Ok(0);
    }
    let http = reqwest::Client::builder()
        .user_agent("omamail/0.1")
        .build()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let mut sorted = 0;
    for message in pending {
        let (choice, _) = jev::classify(
            &http,
            &api_key,
            &message.from,
            &message.subject,
            &message.snippet,
            &settings.categories,
        )
        .await?;
        omamail_store::set_category(&services.db, account.id, &message.id, &choice).await?;
        sorted += 1;
    }
    Ok(sorted)
}

async fn account_jev_enabled(services: &Services, account_id: i64) -> anyhow::Result<bool> {
    if let Some(value) = omamail_store::get_setting(&services.db, &jev_key(account_id)).await? {
        return Ok(value == "1");
    }
    if signed_in_accounts(services).await?.len() == 1 {
        return Ok(omamail_store::get_setting(&services.db, "jev_enabled")
            .await?
            .as_deref()
            == Some("1"));
    }
    Ok(false)
}

async fn load_categories(services: &Services, account_id: i64) -> anyhow::Result<Vec<Category>> {
    let raw = match omamail_store::get_setting(&services.db, &categories_key(account_id)).await? {
        Some(raw) => Some(raw),
        None if signed_in_accounts(services).await?.len() <= 1 => {
            omamail_store::get_setting(&services.db, "categories").await?
        }
        None => None,
    };
    match raw {
        Some(raw) => {
            let mut categories: Vec<Category> = serde_json::from_str(&raw)
                .map_err(|err| anyhow::anyhow!("saved categories are not readable: {err}"))?;
            for (index, category) in categories.iter_mut().enumerate() {
                category.color = jev::normalize_color(&category.color, &category.name, index);
            }
            Ok(categories)
        }
        None => Ok(jev::default_categories()),
    }
}

fn normalize_categories(categories: Vec<Category>) -> anyhow::Result<Vec<Category>> {
    let mut names = std::collections::HashSet::new();
    let mut out = Vec::new();
    for category in categories {
        let name = category.name.trim().to_string();
        let description = category.description.trim().to_string();
        if name.is_empty() {
            continue;
        }
        let key = name.to_ascii_lowercase();
        if !names.insert(key) {
            anyhow::bail!("two categories are named {name}");
        }
        out.push(Category {
            color: jev::normalize_color(&category.color, &name, out.len()),
            description: if description.is_empty() {
                name.clone()
            } else {
                description
            },
            name,
        });
    }
    if out.is_empty() {
        anyhow::bail!("keep at least one category");
    }
    Ok(out)
}

fn item_from(message: MessageSummary, labels: &[StoredLabel], categories: &[Category]) -> InboxItem {
    let tags = tags_for(&message, labels, categories);
    InboxItem {
        id: message.id,
        thread_id: message.thread_id,
        from: message.from,
        subject: message.subject,
        date_ms: message.date_ms,
        snippet: message.snippet,
        unread: message.unread,
        category: message.category,
        tags,
    }
}

fn tags_for(message: &MessageSummary, labels: &[StoredLabel], categories: &[Category]) -> Vec<Tag> {
    let mut tags = Vec::new();
    if let Some(name) = message.category.as_deref() {
        let color = categories
            .iter()
            .find(|category| category.name.eq_ignore_ascii_case(name))
            .map(|category| jev::normalize_color(&category.color, &category.name, 0));
        tags.push(Tag {
            name: name.to_string(),
            color,
            kind: "jev".into(),
        });
    }
    for id in &message.label_ids {
        if let Some(tag) = gmail_tag(id, labels) {
            tags.push(tag);
        }
    }
    tags
}

fn gmail_tag(id: &str, labels: &[StoredLabel]) -> Option<Tag> {
    if matches!(
        id,
        "INBOX" | "UNREAD" | "SENT" | "DRAFT" | "SPAM" | "TRASH" | "CHAT"
    ) {
        return None;
    }
    if let Some(stored) = labels.iter().find(|label| label.id == id) {
        return Some(Tag {
            name: friendly_label(&stored.name, id),
            color: stored.color.clone(),
            kind: "gmail".into(),
        });
    }
    if id.starts_with("Label_") {
        return None;
    }
    Some(Tag {
        name: friendly_label(id, id),
        color: None,
        kind: "gmail".into(),
    })
}

fn friendly_label(name: &str, id: &str) -> String {
    let source = if name.starts_with("CATEGORY_") || name == id {
        id
    } else {
        return name.to_string();
    };
    match source {
        "STARRED" => "Starred".into(),
        "IMPORTANT" => "Important".into(),
        "CATEGORY_PERSONAL" => "Primary".into(),
        "CATEGORY_SOCIAL" => "Social".into(),
        "CATEGORY_PROMOTIONS" => "Promotions".into(),
        "CATEGORY_UPDATES" => "Updates".into(),
        "CATEGORY_FORUMS" => "Forums".into(),
        other => other
            .trim_start_matches("CATEGORY_")
            .replace('_', " ")
            .to_lowercase(),
    }
}

async fn labels_for(services: &Services, account_id: i64) -> Vec<StoredLabel> {
    if let Ok(labels) = omamail_store::gmail_labels(&services.db, account_id).await {
        if !labels.is_empty() {
            return labels;
        }
    }
    let Ok(Some(account)) = signed_in_account(services).await else {
        return Vec::new();
    };
    let Ok(access) = access_token(services, &account.email).await else {
        return Vec::new();
    };
    let Ok(client) = GoogleClient::new(access) else {
        return Vec::new();
    };
    let _ = refresh_labels(services, &client, account_id).await;
    omamail_store::gmail_labels(&services.db, account_id)
        .await
        .unwrap_or_default()
}

async fn refresh_labels(
    services: &Services,
    client: &GoogleClient,
    account_id: i64,
) -> anyhow::Result<()> {
    let labels = client
        .list_labels()
        .await
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    let stored = labels
        .into_iter()
        .map(|label| StoredLabel {
            id: label.id,
            name: label.name,
            color: label.color,
        })
        .collect::<Vec<_>>();
    omamail_store::replace_labels(&services.db, account_id, &stored).await
}
