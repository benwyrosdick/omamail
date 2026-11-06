use crate::auth::Tokens;
use anyhow::Context;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use omamail_core::{MailError, MailProvider, MessageSummary, ProviderKind};
use serde::Deserialize;

pub const GMAIL_API: &str = "https://gmail.googleapis.com/gmail/v1";
pub const USERINFO_ENDPOINT: &str = "https://openidconnect.googleapis.com/v1/userinfo";

pub struct Identity {
    pub email: String,
    pub sub: Option<String>,
    pub name: Option<String>,
}

pub struct GoogleClient {
    http: reqwest::Client,
    access_token: String,
}

impl GoogleClient {
    pub fn new(access_token: impl Into<String>) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("omamail/0.1")
            .build()
            .context("build http client")?;
        Ok(Self {
            http,
            access_token: access_token.into(),
        })
    }

    pub fn from_tokens(tokens: &Tokens) -> anyhow::Result<Self> {
        Self::new(tokens.access_token.clone())
    }
}

#[async_trait::async_trait]
impl MailProvider for GoogleClient {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Google
    }

    async fn list_inbox(&self, limit: u32) -> Result<Vec<MessageSummary>, MailError> {
        let list: ListResponse = self
            .get_json(&format!(
                "{GMAIL_API}/users/me/messages?labelIds=INBOX&maxResults={limit}"
            ))
            .await?;
        let ids: Vec<String> = list
            .messages
            .unwrap_or_default()
            .into_iter()
            .map(|item| item.id)
            .collect();
        let futs = ids.iter().map(|id| self.fetch_metadata(id));
        let fetched = futures_util::future::join_all(futs).await;
        let mut out = Vec::with_capacity(fetched.len());
        for item in fetched {
            out.push(item?);
        }
        Ok(out)
    }

    async fn fetch_plain(&self, id: &str) -> Result<String, MailError> {
        let url = format!(
            "{GMAIL_API}/users/me/messages/{}?format=full",
            urlencoding_path(id)
        );
        let raw: serde_json::Value = self.get_json(&url).await?;
        Ok(plain_from_message(&raw))
    }
}

impl GoogleClient {
    async fn fetch_metadata(&self, id: &str) -> Result<MessageSummary, MailError> {
        let url = format!(
            "{GMAIL_API}/users/me/messages/{}?format=metadata&metadataHeaders=From&metadataHeaders=Subject&metadataHeaders=Date",
            urlencoding_path(id)
        );
        let raw: serde_json::Value = self.get_json(&url).await?;
        parse_message(&raw).map_err(MailError::msg)
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, MailError> {
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.access_token)
            .send()
            .await
            .map_err(|err| MailError::msg(format!("gmail unreachable: {err}")))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(MailError::Remote {
                status: status.as_u16(),
                message: gmail_error_message(status.as_u16(), &body),
            });
        }
        response
            .json()
            .await
            .map_err(|err| MailError::msg(format!("gmail response was not JSON: {err}")))
    }
}

#[derive(Deserialize)]
struct ListResponse {
    messages: Option<Vec<ListItem>>,
}

#[derive(Deserialize)]
struct ListItem {
    id: String,
}

pub fn parse_profile_email(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    value
        .get("emailAddress")
        .or_else(|| value.get("email"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

pub fn parse_message(value: &serde_json::Value) -> Result<MessageSummary, String> {
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("gmail message had no id")?
        .to_string();
    let thread_id = value
        .get("threadId")
        .and_then(|v| v.as_str())
        .unwrap_or(&id)
        .to_string();
    let label_ids: Vec<String> = value
        .get("labelIds")
        .and_then(|v| v.as_array())
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let headers = header_map(value);
    let date_ms = value
        .get("internalDate")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok());
    Ok(MessageSummary {
        id,
        thread_id,
        from: headers.get("from").cloned().unwrap_or_default(),
        subject: headers
            .get("subject")
            .cloned()
            .unwrap_or_else(|| "(no subject)".into()),
        date_ms,
        snippet: value
            .get("snippet")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        unread: label_ids.iter().any(|label| label == "UNREAD"),
        label_ids,
    })
}

pub(crate) async fn fetch_identity(
    http: &reqwest::Client,
    access_token: &str,
) -> anyhow::Result<Identity> {
    let userinfo = http
        .get(USERINFO_ENDPOINT)
        .bearer_auth(access_token)
        .send()
        .await;
    if let Ok(response) = userinfo {
        if response.status().is_success() {
            if let Ok(body) = response.json::<serde_json::Value>().await {
                if let Some(email) = body.get("email").and_then(|v| v.as_str()) {
                    return Ok(Identity {
                        email: email.to_string(),
                        sub: body.get("sub").and_then(|v| v.as_str()).map(str::to_string),
                        name: body
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(str::to_string),
                    });
                }
            }
        }
    }

    let profile = http
        .get(format!("{GMAIL_API}/users/me/profile"))
        .bearer_auth(access_token)
        .send()
        .await
        .context("gmail profile unreachable")?;
    let status = profile.status();
    let body = profile.text().await.context("read gmail profile")?;
    if !status.is_success() {
        anyhow::bail!(
            "could not read the Gmail profile ({}): {}",
            status.as_u16(),
            gmail_error_message(status.as_u16(), &body)
        );
    }
    let email = parse_profile_email(&body).context("gmail profile had no emailAddress")?;
    Ok(Identity {
        email,
        sub: None,
        name: None,
    })
}

fn header_map(value: &serde_json::Value) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let Some(headers) = value.pointer("/payload/headers").and_then(|v| v.as_array()) else {
        return map;
    };
    for header in headers {
        let Some(name) = header.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(val) = header.get("value").and_then(|v| v.as_str()) else {
            continue;
        };
        map.insert(name.to_ascii_lowercase(), val.to_string());
    }
    map
}

fn plain_from_message(value: &serde_json::Value) -> String {
    let payload = value
        .get("payload")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if let Some(text) = find_part(&payload, "text/plain") {
        return text;
    }
    if let Some(html) = find_part(&payload, "text/html") {
        return html_to_text(&html);
    }
    "This message has no plain-text body.".into()
}

fn find_part(part: &serde_json::Value, mime: &str) -> Option<String> {
    let part_mime = part.get("mimeType").and_then(|v| v.as_str()).unwrap_or("");
    if part_mime.eq_ignore_ascii_case(mime) {
        if let Some(text) = part
            .pointer("/body/data")
            .and_then(|v| v.as_str())
            .and_then(decode_body)
        {
            if !text.trim().is_empty() {
                return Some(text);
            }
        }
    }
    for child in part
        .get("parts")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        if let Some(text) = find_part(child, mime) {
            return Some(text);
        }
    }
    None
}

fn decode_body(data: &str) -> Option<String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(data)
        .or_else(|_| URL_SAFE.decode(data))
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn gmail_error_message(status: u16, body: &str) -> String {
    let parsed: Option<String> = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    let message = parsed.unwrap_or_else(|| truncate(body, 280));
    if body.contains("accessNotConfigured") || message.contains("Gmail API has not been used") {
        format!("{message} Enable the Gmail API on the Google Cloud project that owns this OAuth client.")
    } else if status == 401 || status == 403 {
        message
    } else {
        message
    }
}

fn truncate(s: &str, max: usize) -> String {
    let mut out = s.chars().take(max).collect::<String>();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

fn urlencoding_path(id: &str) -> String {
    url::form_urlencoded::byte_serialize(id.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_message_parses_headers_and_unread() {
        let raw = serde_json::json!({
            "id": "abc",
            "threadId": "thr",
            "labelIds": ["INBOX", "UNREAD"],
            "snippet": "See you at 9",
            "internalDate": "1730894400000",
            "payload": {
                "headers": [
                    {"name": "From", "value": "Ada <ada@example.com>"},
                    {"name": "Subject", "value": "Standup"}
                ]
            }
        });
        let message = parse_message(&raw).unwrap();
        assert_eq!(message.id, "abc");
        assert_eq!(message.thread_id, "thr");
        assert_eq!(message.from, "Ada <ada@example.com>");
        assert_eq!(message.subject, "Standup");
        assert_eq!(message.date_ms, Some(1_730_894_400_000));
        assert!(message.unread);
    }

    #[test]
    fn full_message_prefers_plain_and_decodes_base64url() {
        let plain = URL_SAFE_NO_PAD.encode(b"hello from gmail");
        let raw = serde_json::json!({
            "payload": {
                "mimeType": "multipart/alternative",
                "parts": [
                    {"mimeType": "text/plain", "body": {"data": plain}},
                    {"mimeType": "text/html", "body": {"data": URL_SAFE_NO_PAD.encode(b"<p>html</p>")}}
                ]
            }
        });
        assert_eq!(plain_from_message(&raw), "hello from gmail");
    }

    #[test]
    fn html_only_message_is_flattened_to_text() {
        let raw = serde_json::json!({
            "payload": {
                "mimeType": "text/html",
                "body": {"data": URL_SAFE_NO_PAD.encode(b"<p>Hello &amp; welcome</p>")}
            }
        });
        let text = plain_from_message(&raw);
        assert!(text.contains("Hello & welcome"));
        assert!(!text.contains("<p>"));
    }

    #[test]
    fn profile_email_accepts_gmail_or_userinfo() {
        assert_eq!(
            parse_profile_email(r#"{"emailAddress":"ada@example.com"}"#).as_deref(),
            Some("ada@example.com")
        );
        assert_eq!(
            parse_profile_email(r#"{"email":"ada@example.com"}"#).as_deref(),
            Some("ada@example.com")
        );
    }
}
