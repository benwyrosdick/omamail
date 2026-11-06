//! Domain types shared by every mailbox backend.
//!
//! v1 speaks Gmail. POP, IMAP, and other providers implement [`MailProvider`]
//! later; the UI and the store should not grow Google-specific fields.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Which backend an account row belongs to. Only [`Google`](Self::Google) is
/// implemented. The other variants are named so a stored provider string has
/// somewhere honest to land once those backends exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Google,
    Imap,
    Pop,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Imap => "imap",
            Self::Pop => "pop",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "google" => Some(Self::Google),
            "imap" => Some(Self::Imap),
            "pop" => Some(Self::Pop),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: i64,
    pub email: String,
    pub google_sub: Option<String>,
    pub display_name: Option<String>,
    pub provider: String,
}

/// A message header as stored and as listed. `id` is the provider's id
/// (a Gmail message id, in v1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSummary {
    pub id: String,
    pub thread_id: String,
    pub from: String,
    pub subject: String,
    pub date_ms: Option<i64>,
    pub snippet: String,
    pub unread: bool,
    pub label_ids: Vec<String>,
    /// Local category assigned by Jev. Not a Gmail label.
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageBody {
    pub id: String,
    pub plain_text: String,
}

#[derive(Debug, Error)]
pub enum MailError {
    #[error("{0}")]
    Message(String),
    #[error("mailbox request failed ({status}): {message}")]
    Remote { status: u16, message: String },
}

impl MailError {
    pub fn msg(s: impl Into<String>) -> Self {
        Self::Message(s.into())
    }
}

/// Read-only mailbox. Send, labels, and flag changes are deliberately absent
/// until a later milestone.
#[async_trait::async_trait]
pub trait MailProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;

    async fn list_inbox(&self, limit: u32) -> Result<Vec<MessageSummary>, MailError>;

    async fn fetch_plain(&self, id: &str) -> Result<String, MailError>;
}

/// How many inbox rows v1 will pull in one sync. Enough to prove the pipe,
/// small enough that a metadata fetch stays interactive.
pub const INBOX_LIMIT: u32 = 50;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_round_trips() {
        for kind in [ProviderKind::Google, ProviderKind::Imap, ProviderKind::Pop] {
            assert_eq!(ProviderKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(ProviderKind::parse("graph"), None);
    }
}
