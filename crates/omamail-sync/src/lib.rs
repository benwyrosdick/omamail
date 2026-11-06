//! Pull a bounded inbox into SQLite. No background loop in v1.

use omamail_core::{MailError, MailProvider};
use omamail_store::SqlitePool;

pub async fn sync_inbox(
    pool: &SqlitePool,
    account_id: i64,
    provider: &dyn MailProvider,
    limit: u32,
) -> Result<usize, SyncError> {
    let messages = provider
        .list_inbox(limit)
        .await
        .map_err(SyncError::Provider)?;
    let count = omamail_store::replace_inbox(pool, account_id, &messages)
        .await
        .map_err(|err| SyncError::Store(err.to_string()))?;
    Ok(count)
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("{0}")]
    Provider(MailError),
    #[error("{0}")]
    Store(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use omamail_core::{MessageSummary, ProviderKind};

    struct Fake {
        messages: Vec<MessageSummary>,
    }

    #[async_trait::async_trait]
    impl MailProvider for Fake {
        fn kind(&self) -> ProviderKind {
            ProviderKind::Google
        }

        async fn list_inbox(&self, limit: u32) -> Result<Vec<MessageSummary>, MailError> {
            Ok(self.messages.iter().take(limit as usize).cloned().collect())
        }

        async fn fetch_plain(&self, _id: &str) -> Result<String, MailError> {
            Ok("plain".into())
        }
    }

    #[tokio::test]
    async fn sync_replaces_the_cached_inbox() {
        let dir = std::env::temp_dir().join(format!("omamail-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let pool = omamail_store::open(&dir.join("omamail.db")).await.unwrap();
        let account = omamail_store::upsert_account(&pool, "ada@example.com", None, None, "google")
            .await
            .unwrap();
        let first = Fake {
            messages: vec![msg("a"), msg("b")],
        };
        assert_eq!(sync_inbox(&pool, account, &first, 50).await.unwrap(), 2);
        let second = Fake {
            messages: vec![msg("b")],
        };
        assert_eq!(sync_inbox(&pool, account, &second, 50).await.unwrap(), 1);
        let listed = omamail_store::list_inbox(&pool, account).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "b");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn msg(id: &str) -> MessageSummary {
        MessageSummary {
            id: id.into(),
            thread_id: id.into(),
            from: "Ada <ada@example.com>".into(),
            subject: id.into(),
            date_ms: Some(1),
            snippet: String::new(),
            unread: false,
            label_ids: vec!["INBOX".into()],
            category: None,
        }
    }
}
