//! Glue between the mailbox crates, the keyring, and both front doors
//! (the CLI and the Tauri window).

mod cli;
pub mod commands;
mod oauth_config;
mod paths;
mod theme;
mod tokens;

pub use cli::{dispatch, Launch};
pub use commands::{
    inbox, login, logout, message, status, sync, InboxItem, MessageView, Session, Status,
};
pub use oauth_config::OauthConfig;
pub use paths::AppPaths;
pub use theme::{colors_path, current_palette, parse_colors, Palette};
pub use tokens::{KeyringStore, MemoryTokenStore, TokenStore};

use omamail_store::SqlitePool;
use std::sync::Arc;

#[derive(Clone)]
pub struct Services {
    pub paths: AppPaths,
    pub db: SqlitePool,
    pub tokens: Arc<dyn TokenStore>,
}

impl Services {
    pub async fn open_default() -> anyhow::Result<Self> {
        Self::open(AppPaths::system(), Arc::new(KeyringStore)).await
    }

    pub async fn open(paths: AppPaths, tokens: Arc<dyn TokenStore>) -> anyhow::Result<Self> {
        let db = omamail_store::open(&paths.database()).await?;
        Ok(Self { paths, db, tokens })
    }
}
