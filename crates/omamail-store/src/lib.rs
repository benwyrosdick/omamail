//! Local mailbox cache. Refresh tokens belong in the OS keyring, never in
//! these tables.

use anyhow::Context;
use omamail_core::{Account, MessageSummary};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::Row;
pub use sqlx::SqlitePool;
use std::path::Path;
use std::str::FromStr;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS accounts (
    id            INTEGER PRIMARY KEY,
    email         TEXT NOT NULL UNIQUE,
    google_sub    TEXT,
    display_name  TEXT,
    provider      TEXT NOT NULL,
    created_at    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    gmail_id    TEXT NOT NULL,
    thread_id   TEXT NOT NULL,
    from_addr   TEXT NOT NULL,
    subject     TEXT NOT NULL,
    date_ms     INTEGER,
    snippet     TEXT NOT NULL,
    unread      INTEGER NOT NULL,
    label_ids   TEXT NOT NULL,
    PRIMARY KEY (account_id, gmail_id)
);

CREATE TABLE IF NOT EXISTS bodies (
    account_id  INTEGER NOT NULL,
    gmail_id    TEXT NOT NULL,
    plain_text  TEXT NOT NULL,
    fetched_at  INTEGER NOT NULL,
    PRIMARY KEY (account_id, gmail_id)
);

INSERT OR IGNORE INTO meta (key, value) VALUES ('schema_version', '1');
"#;

pub async fn open(path: &Path) -> anyhow::Result<SqlitePool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let url = format!("sqlite://{}", path.display());
    let opts = SqliteConnectOptions::from_str(&url)
        .with_context(|| format!("sqlite url for {}", path.display()))?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await
        .with_context(|| format!("open {}", path.display()))?;
    migrate(&pool).await?;
    Ok(pool)
}

pub async fn migrate(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::raw_sql(SCHEMA)
        .execute(pool)
        .await
        .context("apply schema")?;
    let columns = sqlx::query("PRAGMA table_info(bodies)")
        .fetch_all(pool)
        .await
        .context("inspect bodies")?;
    let has_html = columns
        .iter()
        .any(|row| row.get::<String, _>("name") == "html");
    if !has_html {
        sqlx::query("ALTER TABLE bodies ADD COLUMN html TEXT")
            .execute(pool)
            .await
            .context("add bodies.html")?;
    }
    let message_columns = sqlx::query("PRAGMA table_info(messages)")
        .fetch_all(pool)
        .await
        .context("inspect messages")?;
    let has_category = message_columns
        .iter()
        .any(|row| row.get::<String, _>("name") == "category");
    if !has_category {
        sqlx::query("ALTER TABLE messages ADD COLUMN category TEXT")
            .execute(pool)
            .await
            .context("add messages.category")?;
    }
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    )
    .execute(pool)
    .await
    .context("settings table")?;
    // Cached HTML was sanitized without the sender stylesheet, which left
    // inbox preview text visible. Drop it once so the next open refetches.
    let rev = get_setting(pool, "html_sanitizer").await?;
    for (column, ddl) in [
        (
            "unsubscribe_url",
            "ALTER TABLE bodies ADD COLUMN unsubscribe_url TEXT",
        ),
        (
            "unsubscribe_one_click",
            "ALTER TABLE bodies ADD COLUMN unsubscribe_one_click INTEGER NOT NULL DEFAULT 0",
        ),
    ] {
        if !columns.iter().any(|row| row.get::<String, _>("name") == column)
            && !sqlx::query("PRAGMA table_info(bodies)")
                .fetch_all(pool)
                .await
                .unwrap_or_default()
                .iter()
                .any(|row| row.get::<String, _>("name") == column)
        {
            sqlx::query(ddl)
                .execute(pool)
                .await
                .with_context(|| format!("add bodies.{column}"))?;
        }
    }
    if rev.as_deref() != Some("3") {
        sqlx::query("UPDATE bodies SET html = NULL")
            .execute(pool)
            .await
            .context("drop stale html cache")?;
        set_setting(pool, "html_sanitizer", "3").await?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedBody {
    pub plain_text: String,
    pub html: Option<String>,
    pub unsubscribe_url: Option<String>,
    pub unsubscribe_one_click: bool,
}

pub async fn upsert_account(
    pool: &SqlitePool,
    email: &str,
    google_sub: Option<&str>,
    display_name: Option<&str>,
    provider: &str,
) -> anyhow::Result<i64> {
    let now = now_ms();
    let row = sqlx::query(
        r#"
        INSERT INTO accounts (email, google_sub, display_name, provider, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(email) DO UPDATE SET
            google_sub = excluded.google_sub,
            display_name = COALESCE(excluded.display_name, accounts.display_name),
            provider = excluded.provider
        RETURNING id
        "#,
    )
    .bind(email)
    .bind(google_sub)
    .bind(display_name)
    .bind(provider)
    .bind(now)
    .fetch_one(pool)
    .await
    .context("upsert account")?;
    Ok(row.get("id"))
}

pub async fn accounts(pool: &SqlitePool) -> anyhow::Result<Vec<Account>> {
    let rows = sqlx::query(
        r#"
        SELECT id, email, google_sub, display_name, provider
        FROM accounts
        ORDER BY created_at DESC, id DESC
        "#,
    )
    .fetch_all(pool)
    .await
    .context("list accounts")?;
    Ok(rows
        .into_iter()
        .map(|row| Account {
            id: row.get("id"),
            email: row.get("email"),
            google_sub: row.get("google_sub"),
            display_name: row.get("display_name"),
            provider: row.get("provider"),
        })
        .collect())
}

/// Replace the cached inbox with `messages`. Rows not in the new set are
/// dropped so the list matches the latest sync, not a growing union.
/// Drop cached inbox rows that Gmail did not return on a fresh first page.
/// Bodies for those rows go with them. Categories on rows that remain are kept.
pub async fn retain_inbox(
    pool: &SqlitePool,
    account_id: i64,
    keep: &[MessageSummary],
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await.context("begin inbox retain")?;
    sqlx::query("CREATE TEMP TABLE IF NOT EXISTS keep_ids (id TEXT PRIMARY KEY)")
        .execute(&mut *tx)
        .await
        .context("create keep_ids")?;
    sqlx::query("DELETE FROM keep_ids")
        .execute(&mut *tx)
        .await
        .context("clear keep_ids")?;
    for message in keep {
        sqlx::query("INSERT OR IGNORE INTO keep_ids (id) VALUES (?1)")
            .bind(&message.id)
            .execute(&mut *tx)
            .await
            .context("remember kept id")?;
    }
    sqlx::query(
        "DELETE FROM messages WHERE account_id = ?1 AND label_ids LIKE '%\"INBOX\"%' AND gmail_id NOT IN (SELECT id FROM keep_ids)",
    )
    .bind(account_id)
    .execute(&mut *tx)
    .await
    .context("drop inbox messages Gmail no longer lists")?;
    sqlx::query(
        "DELETE FROM bodies WHERE account_id = ?1 AND gmail_id NOT IN (SELECT gmail_id FROM messages WHERE account_id = ?1)",
    )
    .bind(account_id)
    .execute(&mut *tx)
    .await
    .context("drop bodies for removed messages")?;
    tx.commit().await.context("commit inbox retain")?;
    Ok(())
}

pub async fn upsert_messages(
    pool: &SqlitePool,
    account_id: i64,
    messages: &[MessageSummary],
) -> anyhow::Result<usize> {
    for message in messages {
        let labels = serde_json::to_string(&message.label_ids).unwrap_or_else(|_| "[]".into());
        sqlx::query(
            r#"
            INSERT INTO messages (
                account_id, gmail_id, thread_id, from_addr, subject,
                date_ms, snippet, unread, label_ids
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(account_id, gmail_id) DO UPDATE SET
                thread_id = excluded.thread_id,
                from_addr = excluded.from_addr,
                subject = excluded.subject,
                date_ms = excluded.date_ms,
                snippet = excluded.snippet,
                unread = excluded.unread,
                label_ids = excluded.label_ids
            "#,
        )
        .bind(account_id)
        .bind(&message.id)
        .bind(&message.thread_id)
        .bind(&message.from)
        .bind(&message.subject)
        .bind(message.date_ms)
        .bind(&message.snippet)
        .bind(i64::from(message.unread))
        .bind(labels)
        .execute(pool)
        .await
        .context("upsert message page")?;
    }
    Ok(messages.len())
}

pub async fn replace_inbox(
    pool: &SqlitePool,
    account_id: i64,
    messages: &[MessageSummary],
) -> anyhow::Result<usize> {
    let mut tx = pool.begin().await.context("begin inbox replace")?;
    if messages.is_empty() {
        sqlx::query("DELETE FROM messages WHERE account_id = ?1")
            .bind(account_id)
            .execute(&mut *tx)
            .await
            .context("clear inbox")?;
        sqlx::query("DELETE FROM bodies WHERE account_id = ?1")
            .bind(account_id)
            .execute(&mut *tx)
            .await
            .context("clear bodies")?;
        tx.commit().await.context("commit empty inbox")?;
        return Ok(0);
    }

    sqlx::query("CREATE TEMP TABLE IF NOT EXISTS keep_ids (id TEXT PRIMARY KEY)")
        .execute(&mut *tx)
        .await
        .context("create keep_ids")?;
    sqlx::query("DELETE FROM keep_ids")
        .execute(&mut *tx)
        .await
        .context("clear keep_ids")?;

    for message in messages {
        let labels = serde_json::to_string(&message.label_ids).unwrap_or_else(|_| "[]".into());
        sqlx::query("INSERT OR IGNORE INTO keep_ids (id) VALUES (?1)")
            .bind(&message.id)
            .execute(&mut *tx)
            .await
            .context("remember kept id")?;
        sqlx::query(
            r#"
            INSERT INTO messages (
                account_id, gmail_id, thread_id, from_addr, subject,
                date_ms, snippet, unread, label_ids
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(account_id, gmail_id) DO UPDATE SET
                thread_id = excluded.thread_id,
                from_addr = excluded.from_addr,
                subject = excluded.subject,
                date_ms = excluded.date_ms,
                snippet = excluded.snippet,
                unread = excluded.unread,
                label_ids = excluded.label_ids
            "#,
        )
        .bind(account_id)
        .bind(&message.id)
        .bind(&message.thread_id)
        .bind(&message.from)
        .bind(&message.subject)
        .bind(message.date_ms)
        .bind(&message.snippet)
        .bind(i64::from(message.unread))
        .bind(labels)
        .execute(&mut *tx)
        .await
        .context("upsert message")?;
    }

    sqlx::query(
        "DELETE FROM messages WHERE account_id = ?1 AND gmail_id NOT IN (SELECT id FROM keep_ids)",
    )
    .bind(account_id)
    .execute(&mut *tx)
    .await
    .context("drop messages outside the latest inbox")?;

    sqlx::query(
        "DELETE FROM bodies WHERE account_id = ?1 AND gmail_id NOT IN (SELECT id FROM keep_ids)",
    )
    .bind(account_id)
    .execute(&mut *tx)
    .await
    .context("drop bodies outside the latest inbox")?;

    tx.commit().await.context("commit inbox replace")?;
    Ok(messages.len())
}

pub async fn list_inbox(pool: &SqlitePool, account_id: i64) -> anyhow::Result<Vec<MessageSummary>> {
    let rows = sqlx::query(
        r#"
        SELECT gmail_id, thread_id, from_addr, subject, date_ms, snippet, unread, label_ids, category
        FROM messages
        WHERE account_id = ?1 AND label_ids LIKE '%"INBOX"%'
        ORDER BY date_ms DESC, gmail_id DESC
        "#,
    )
    .bind(account_id)
    .fetch_all(pool)
    .await
    .context("list inbox")?;
    rows.into_iter().map(summary_from_row).collect()
}

pub async fn get_message(
    pool: &SqlitePool,
    account_id: i64,
    gmail_id: &str,
) -> anyhow::Result<Option<MessageSummary>> {
    let row = sqlx::query(
        r#"
        SELECT gmail_id, thread_id, from_addr, subject, date_ms, snippet, unread, label_ids, category
        FROM messages
        WHERE account_id = ?1 AND gmail_id = ?2
        "#,
    )
    .bind(account_id)
    .bind(gmail_id)
    .fetch_optional(pool)
    .await
    .context("get message")?;
    row.map(summary_from_row).transpose()
}

pub async fn get_body(
    pool: &SqlitePool,
    account_id: i64,
    gmail_id: &str,
) -> anyhow::Result<Option<CachedBody>> {
    let row = sqlx::query(
        "SELECT plain_text, html, unsubscribe_url, unsubscribe_one_click FROM bodies WHERE account_id = ?1 AND gmail_id = ?2",
    )
    .bind(account_id)
    .bind(gmail_id)
    .fetch_optional(pool)
    .await
    .context("get body")?;
    Ok(row.map(|row| {
        let one_click: i64 = row.get("unsubscribe_one_click");
        CachedBody {
            plain_text: row.get("plain_text"),
            html: row.get("html"),
            unsubscribe_url: row.get("unsubscribe_url"),
            unsubscribe_one_click: one_click != 0,
        }
    }))
}

pub async fn put_body(
    pool: &SqlitePool,
    account_id: i64,
    gmail_id: &str,
    plain_text: &str,
    html: Option<&str>,
    unsubscribe_url: Option<&str>,
    unsubscribe_one_click: bool,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO bodies (account_id, gmail_id, plain_text, html, unsubscribe_url, unsubscribe_one_click, fetched_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(account_id, gmail_id) DO UPDATE SET
            plain_text = excluded.plain_text,
            html = excluded.html,
            unsubscribe_url = excluded.unsubscribe_url,
            unsubscribe_one_click = excluded.unsubscribe_one_click,
            fetched_at = excluded.fetched_at
        "#,
    )
    .bind(account_id)
    .bind(gmail_id)
    .bind(plain_text)
    .bind(html)
    .bind(unsubscribe_url)
    .bind(i64::from(unsubscribe_one_click))
    .bind(now_ms())
    .execute(pool)
    .await
    .context("put body")?;
    Ok(())
}

fn summary_from_row(row: sqlx::sqlite::SqliteRow) -> anyhow::Result<MessageSummary> {
    let labels: String = row.get("label_ids");
    let label_ids = serde_json::from_str(&labels).unwrap_or_default();
    let unread: i64 = row.get("unread");
    Ok(MessageSummary {
        id: row.get("gmail_id"),
        thread_id: row.get("thread_id"),
        from: row.get("from_addr"),
        subject: row.get("subject"),
        date_ms: row.get("date_ms"),
        snippet: row.get("snippet"),
        unread: unread != 0,
        label_ids,
        category: row.get("category"),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredLabel {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
}

pub async fn replace_labels(
    pool: &SqlitePool,
    account_id: i64,
    labels: &[StoredLabel],
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await.context("begin label replace")?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS gmail_labels (
            account_id INTEGER NOT NULL,
            label_id TEXT NOT NULL,
            name TEXT NOT NULL,
            color TEXT,
            PRIMARY KEY (account_id, label_id)
        )",
    )
    .execute(&mut *tx)
    .await
    .context("gmail labels table")?;
    sqlx::query("DELETE FROM gmail_labels WHERE account_id = ?1")
        .bind(account_id)
        .execute(&mut *tx)
        .await
        .context("clear labels")?;
    for label in labels {
        sqlx::query(
            "INSERT INTO gmail_labels (account_id, label_id, name, color) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(account_id)
        .bind(&label.id)
        .bind(&label.name)
        .bind(&label.color)
        .execute(&mut *tx)
        .await
        .context("insert label")?;
    }
    tx.commit().await.context("commit labels")?;
    Ok(())
}

pub async fn gmail_labels(pool: &SqlitePool, account_id: i64) -> anyhow::Result<Vec<StoredLabel>> {
    let rows = sqlx::query(
        "SELECT label_id, name, color FROM gmail_labels WHERE account_id = ?1 ORDER BY name",
    )
    .bind(account_id)
    .fetch_all(pool)
    .await;
    match rows {
        Ok(rows) => Ok(rows
            .into_iter()
            .map(|row| StoredLabel {
                id: row.get("label_id"),
                name: row.get("name"),
                color: row.get("color"),
            })
            .collect()),
        Err(_) => Ok(Vec::new()),
    }
}

pub async fn remove_message(
    pool: &SqlitePool,
    account_id: i64,
    gmail_id: &str,
) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM messages WHERE account_id = ?1 AND gmail_id = ?2")
        .bind(account_id)
        .bind(gmail_id)
        .execute(pool)
        .await
        .context("remove message")?;
    sqlx::query("DELETE FROM bodies WHERE account_id = ?1 AND gmail_id = ?2")
        .bind(account_id)
        .bind(gmail_id)
        .execute(pool)
        .await
        .context("remove body")?;
    Ok(())
}

pub async fn mark_read(
    pool: &SqlitePool,
    account_id: i64,
    gmail_id: &str,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE messages SET unread = 0 WHERE account_id = ?1 AND gmail_id = ?2")
        .bind(account_id)
        .bind(gmail_id)
        .execute(pool)
        .await
        .context("mark read")?;
    Ok(())
}

pub async fn set_category(
    pool: &SqlitePool,
    account_id: i64,
    gmail_id: &str,
    category: &str,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE messages SET category = ?1 WHERE account_id = ?2 AND gmail_id = ?3")
        .bind(category)
        .bind(account_id)
        .bind(gmail_id)
        .execute(pool)
        .await
        .context("set category")?;
    Ok(())
}

pub async fn get_setting(pool: &SqlitePool, key: &str) -> anyhow::Result<Option<String>> {
    let row = sqlx::query("SELECT value FROM settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .context("get setting")?;
    Ok(row.map(|row| row.get("value")))
}

pub async fn set_setting(pool: &SqlitePool, key: &str, value: &str) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await
    .context("set setting")?;
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, subject: &str, date_ms: i64) -> MessageSummary {
        MessageSummary {
            id: id.into(),
            thread_id: format!("t-{id}"),
            from: "Ada <ada@example.com>".into(),
            subject: subject.into(),
            date_ms: Some(date_ms),
            snippet: "hello".into(),
            unread: true,
            label_ids: vec!["INBOX".into(), "UNREAD".into()],
            category: None,
        }
    }

    #[tokio::test]
    async fn account_and_inbox_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("omamail-store-{}-{}", std::process::id(), now_ms()));
        let pool = open(&dir.join("omamail.db")).await.unwrap();
        let id = upsert_account(
            &pool,
            "ada@example.com",
            Some("sub-1"),
            Some("Ada"),
            "google",
        )
        .await
        .unwrap();
        let again = upsert_account(&pool, "ada@example.com", Some("sub-1"), None, "google")
            .await
            .unwrap();
        assert_eq!(id, again);

        replace_inbox(
            &pool,
            id,
            &[sample("a", "Older", 10), sample("b", "Newer", 20)],
        )
        .await
        .unwrap();
        let listed = list_inbox(&pool, id).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, "b");
        assert_eq!(listed[0].subject, "Newer");
        assert!(listed[0].unread);

        put_body(&pool, id, "b", "body text", Some("<p>body</p>"), Some("https://example.com/unsub"), true)
            .await
            .unwrap();
        let cached = get_body(&pool, id, "b").await.unwrap().unwrap();
        assert_eq!(cached.plain_text, "body text");
        assert_eq!(cached.html.as_deref(), Some("<p>body</p>"));

        replace_inbox(&pool, id, &[sample("b", "Newer", 20)])
            .await
            .unwrap();
        let listed = list_inbox(&pool, id).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(get_body(&pool, id, "a").await.unwrap().is_none());
        assert_eq!(
            get_body(&pool, id, "b")
                .await
                .unwrap()
                .unwrap()
                .plain_text,
            "body text"
        );

        let _ = std::fs::remove_dir_all(dir);
    }
}
