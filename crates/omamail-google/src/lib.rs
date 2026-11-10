//! Google sign-in and Gmail reads. No client secret is compiled into this
//! crate. The desktop client id comes from the user's config.

mod auth;
mod client;

pub use auth::{
    authorize_url, exchange_code, generate_pkce, needs_reauth, refresh_access, AuthParams, Pkce,
    SignedIn, TokenRejected, Tokens, AUTH_ENDPOINT, GMAIL_SCOPE, TOKEN_ENDPOINT,
};
pub use client::{
    parse_message, parse_profile_email, GoogleClient, HistoryDelta, USERINFO_ENDPOINT,
};

use anyhow::Context;
use auth::post_token;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Loopback PKCE sign-in. Opens the system browser and waits for Google to
/// redirect back to 127.0.0.1. `client_secret` is optional; send it only when
/// the user configured one. It is never logged.
pub async fn sign_in_via_browser(
    client_id: &str,
    client_secret: Option<&str>,
) -> anyhow::Result<SignedIn> {
    let pkce = generate_pkce();
    let state = generate_pkce().verifier;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("bind the OAuth loopback port")?;
    let port = listener
        .local_addr()
        .context("read the OAuth loopback port")?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}");
    let url = authorize_url(&AuthParams {
        client_id,
        redirect_uri: &redirect_uri,
        challenge: &pkce.challenge,
        state: &state,
    });

    eprintln!("opening a browser for Google sign-in");
    if let Err(err) = open_browser(&url) {
        eprintln!("could not open a browser ({err})");
        eprintln!("open this URL yourself:\n{url}");
    }

    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel();
    if let Ok(mut slot) = SIGN_IN_CANCEL.lock() {
        if let Some(previous) = slot.replace(cancel_tx) {
            let _ = previous.send(());
        }
    }
    let query = wait_for_callback(listener, cancel_rx).await;
    if let Ok(mut slot) = SIGN_IN_CANCEL.lock() {
        slot.take();
    }
    let query = query?;
    if query.get("state").map(String::as_str) != Some(state.as_str()) {
        anyhow::bail!("OAuth state did not match; sign-in was abandoned");
    }
    if let Some(error) = query.get("error") {
        let description = query
            .get("error_description")
            .map(String::as_str)
            .unwrap_or("");
        anyhow::bail!("Google did not grant access: {error} {description}");
    }
    let code = query
        .get("code")
        .context("the OAuth redirect had no code")?;

    let tokens = exchange_code(
        TOKEN_ENDPOINT,
        client_id,
        client_secret.unwrap_or(""),
        code,
        &pkce.verifier,
        &redirect_uri,
    )
    .await?;

    let http = reqwest::Client::builder()
        .user_agent("omamail/0.1")
        .build()
        .context("build http client")?;
    let profile = client::fetch_identity(&http, &tokens.access_token).await?;
    Ok(SignedIn {
        tokens,
        email: profile.email,
        google_sub: profile.sub,
        display_name: profile.name,
    })
}

fn open_browser(url: &str) -> std::io::Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "xdg-open"
    };
    let mut cmd = std::process::Command::new(program);
    if cfg!(target_os = "windows") {
        cmd.args(["/C", "start", "", url]);
    } else {
        cmd.arg(url);
    }
    cmd.spawn().map(|_| ())
}

static SIGN_IN_CANCEL: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>> =
    std::sync::Mutex::new(None);

/// Stop a sign-in that is waiting for the browser. Safe to call if none is running.
pub fn cancel_sign_in() {
    if let Ok(mut slot) = SIGN_IN_CANCEL.lock() {
        if let Some(cancel) = slot.take() {
            let _ = cancel.send(());
        }
    }
}

async fn wait_for_callback(
    listener: TcpListener,
    mut cancel: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<std::collections::HashMap<String, String>> {
    let accepted = tokio::select! {
        accepted = listener.accept() => accepted.context("accept the OAuth redirect")?,
        _ = &mut cancel => anyhow::bail!("sign-in cancelled"),
        _ = tokio::time::sleep(Duration::from_secs(180)) => {
            anyhow::bail!("timed out waiting for Google to redirect back — sign-in was not finished")
        }
    };
    let (mut socket, _) = accepted;
    let mut buf = vec![0u8; 8192];
    let n = tokio::time::timeout(Duration::from_secs(10), socket.read(&mut buf))
        .await
        .context("timed out reading the OAuth redirect")?
        .context("read the OAuth redirect")?;
    let request = String::from_utf8_lossy(&buf[..n]);
    let query = parse_redirect_query(&request);
    let page = if query.contains_key("error") {
        callback_page("Google cancelled sign-in. You can close this window.")
    } else {
        callback_page("OmaMail is signed in. You can close this window.")
    };
    let _ = socket.write_all(page.as_bytes()).await;
    let _ = socket.shutdown().await;
    Ok(query)
}

pub fn parse_redirect_query(request: &str) -> std::collections::HashMap<String, String> {
    let first = request.lines().next().unwrap_or("");
    let path = first.split_whitespace().nth(1).unwrap_or("");
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

fn callback_page(message: &str) -> String {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>OmaMail</title>\
         <body style=\"margin:0;font-family:system-ui,sans-serif;background:#1a1b26;color:#c0caf5;\
         display:grid;place-items:center;height:100vh\"><p>{message}</p>"
    );
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}

pub async fn refresh(
    client_id: &str,
    client_secret: Option<&str>,
    refresh_token: &str,
) -> anyhow::Result<Tokens> {
    refresh_access(
        TOKEN_ENDPOINT,
        client_id,
        client_secret.unwrap_or(""),
        refresh_token,
    )
    .await
}

/// Used by tests that want the token poster without a browser.
pub async fn exchange_for_test(
    endpoint: &str,
    client_id: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> anyhow::Result<Tokens> {
    post_token(
        endpoint,
        &[
            ("client_id", client_id),
            ("code", code),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", redirect_uri),
        ],
    )
    .await
}
