use crate::commands;
use crate::Services;
use clap::{Parser, Subcommand};
use serde::Serialize;
use serde_json::json;
#[cfg(test)]
use serde_json::Value;
use std::ffi::OsString;

pub enum Launch {
    Gui,
    Exit(i32),
}

#[derive(Parser)]
#[command(
    name = "omamail",
    version,
    about = "Gmail, the Omarchy way. No arguments opens the window."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Google sign-in.
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Pull the latest inbox headers (at most 50).
    Sync,
    /// Cached inbox.
    Inbox {
        #[command(subcommand)]
        action: InboxAction,
    },
    /// One cached message. The body is fetched on first read.
    Message {
        #[command(subcommand)]
        action: MessageAction,
    },
}

#[derive(Subcommand)]
enum AuthAction {
    /// Open a browser and store a refresh token in the keyring.
    Login,
    /// Whether a client id is configured and an account is signed in.
    Status,
    /// Forget saved refresh tokens. The local cache stays.
    Logout,
}

#[derive(Subcommand)]
enum InboxAction {
    /// List cached inbox headers.
    List,
}

#[derive(Subcommand)]
enum MessageAction {
    /// Print one message as JSON.
    Show { id: String },
}

/// `None`-equivalent: no CLI args means the caller should open the window.
/// `--help`, `-V`, and every subcommand stay in the terminal.
pub fn dispatch() -> Launch {
    let mut args = std::env::args_os();
    let argv0 = args.next().unwrap_or_else(|| OsString::from("omamail"));
    let rest: Vec<OsString> = args.collect();
    if rest.is_empty() {
        return Launch::Gui;
    }
    let mut full = vec![argv0];
    full.extend(rest);
    Launch::Exit(run_args(&full))
}

pub fn run_args(args: &[OsString]) -> i32 {
    match Cli::try_parse_from(args) {
        Ok(cli) => match runtime().block_on(run_command(cli.command)) {
            Ok(output) => {
                println!("{output}");
                0
            }
            Err(err) => {
                println!("{}", envelope_err(&err.to_string()));
                1
            }
        },
        Err(err) => {
            // clap already printed help or the parse error.
            let _ = err.print();
            if err.use_stderr() {
                2
            } else {
                0
            }
        }
    }
}

async fn run_command(command: Command) -> anyhow::Result<String> {
    let services = Services::open_default().await?;
    match command {
        Command::Auth {
            action: AuthAction::Login,
        } => {
            let session = commands::login(&services).await?;
            Ok(envelope(&session))
        }
        Command::Auth {
            action: AuthAction::Status,
        } => Ok(envelope(&commands::status(&services).await)),
        Command::Auth {
            action: AuthAction::Logout,
        } => {
            commands::logout(&services, None).await?;
            Ok(envelope(&json!({ "signed_out": true })))
        }
        Command::Sync => {
            let synced = commands::sync(&services).await?;
            let status = commands::status(&services).await;
            Ok(envelope(&json!({
                "synced": synced,
                "email": status.email,
            })))
        }
        Command::Inbox {
            action: InboxAction::List,
        } => {
            let status = commands::status(&services).await;
            if !status.signed_in {
                anyhow::bail!("not signed in");
            }
            let messages = commands::inbox(&services).await?;
            Ok(envelope(&json!({
                "account": status.email,
                "messages": messages,
            })))
        }
        Command::Message {
            action: MessageAction::Show { id },
        } => Ok(envelope(&commands::message(&services, &id).await?)),
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

fn envelope<T: Serialize>(data: &T) -> String {
    serde_json::to_string(&json!({ "ok": true, "data": data }))
        .unwrap_or_else(|_| r#"{"ok":false,"error":"could not encode the response"}"#.into())
}

pub fn envelope_err(message: &str) -> String {
    serde_json::to_string(&json!({ "ok": false, "error": message }))
        .unwrap_or_else(|_| r#"{"ok":false,"error":"unknown error"}"#.into())
}

/// Stable JSON for the inbox command, exercised without Google or the keyring.
#[cfg(test)]
async fn inbox_list_json(services: &Services) -> anyhow::Result<Value> {
    let status = commands::status(services).await;
    if let Some(error) = &status.error {
        anyhow::bail!(error.clone());
    }
    if !status.signed_in {
        anyhow::bail!("not signed in");
    }
    let messages = commands::inbox(services).await?;
    Ok(json!({
        "ok": true,
        "data": {
            "account": status.email,
            "messages": messages,
        }
    }))
}

#[cfg(test)]
async fn status_json(services: &Services) -> Value {
    let status = commands::status(services).await;
    json!({ "ok": true, "data": status })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::AppPaths;
    use crate::tokens::{MemoryTokenStore, TokenStore};
    use omamail_core::MessageSummary;
    use std::sync::Arc;

    #[tokio::test]
    async fn status_and_inbox_json_use_a_temp_db() {
        let root = std::env::temp_dir().join(format!(
            "omamail-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let paths = AppPaths::for_root(&root);
        let tokens = Arc::new(MemoryTokenStore::default());
        let services = Services::open(paths, tokens.clone() as Arc<dyn TokenStore>)
            .await
            .unwrap();

        let status = status_json(&services).await;
        assert_eq!(status["ok"], true);
        assert_eq!(status["data"]["signed_in"], false);
        assert!(status["data"]["email"].is_null());
        assert!(status["data"]["client_configured"].is_boolean());

        let id = omamail_store::upsert_account(
            &services.db,
            "ada@example.com",
            Some("sub"),
            None,
            "google",
        )
        .await
        .unwrap();
        tokens
            .set("ada@example.com", "refresh-token")
            .await
            .unwrap();
        omamail_store::replace_inbox(
            &services.db,
            id,
            &[MessageSummary {
                id: "m1".into(),
                thread_id: "t1".into(),
                from: "Grace <grace@example.com>".into(),
                subject: "Quarterly review".into(),
                date_ms: Some(1_700_000_000_000),
                snippet: "notes attached".into(),
                unread: true,
                label_ids: vec!["INBOX".into(), "UNREAD".into()],
                category: None,
            }],
        )
        .await
        .unwrap();

        let listed = inbox_list_json(&services).await.unwrap();
        assert_eq!(listed["ok"], true);
        assert_eq!(listed["data"]["account"], "ada@example.com");
        assert_eq!(listed["data"]["messages"][0]["id"], "m1");
        assert_eq!(listed["data"]["messages"][0]["subject"], "Quarterly review");
        assert_eq!(listed["data"]["messages"][0]["unread"], true);
        assert!(listed["data"]["messages"][0].get("plain_text").is_none());

        let err = envelope_err("not signed in");
        let err: Value = serde_json::from_str(&err).unwrap();
        assert_eq!(err["ok"], false);
        assert_eq!(err["error"], "not signed in");

        let _ = std::fs::remove_dir_all(root);
    }
}
