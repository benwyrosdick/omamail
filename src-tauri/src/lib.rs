mod display;
mod webkit;

use omamail_app::{colors_path, commands, current_palette, Services};
use std::path::PathBuf;
use tauri::{Emitter, Manager, RunEvent, WindowEvent};

#[tauri::command]
async fn theme() -> omamail_app::Palette {
    commands::theme().await
}

#[tauri::command]
async fn status(state: tauri::State<'_, Services>) -> Result<omamail_app::Status, String> {
    Ok(commands::status(&state).await)
}

#[tauri::command]
async fn login(state: tauri::State<'_, Services>) -> Result<omamail_app::Session, String> {
    commands::login(&state).await.map_err(|err| err.to_string())
}

#[tauri::command]
fn cancel_login() {
    omamail_google::cancel_sign_in();
}

#[tauri::command]
async fn logout(
    state: tauri::State<'_, Services>,
    email: Option<String>,
) -> Result<(), String> {
    commands::logout(&state, email.as_deref())
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn set_active_account(
    state: tauri::State<'_, Services>,
    email: String,
) -> Result<(), String> {
    commands::set_active_account(&state, &email)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn sync(state: tauri::State<'_, Services>) -> Result<usize, String> {
    commands::sync(&state).await.map_err(|err| err.to_string())
}

#[tauri::command]
async fn inbox(state: tauri::State<'_, Services>) -> Result<Vec<omamail_app::InboxItem>, String> {
    commands::inbox(&state).await.map_err(|err| err.to_string())
}

#[tauri::command]
async fn inbox_token(state: tauri::State<'_, Services>) -> Result<Option<String>, String> {
    commands::inbox_page_token(&state)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn load_more(
    state: tauri::State<'_, Services>,
    query: Option<String>,
    page_token: Option<String>,
) -> Result<omamail_app::commands::MailPage, String> {
    commands::load_more(&state, query, page_token)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn message(
    state: tauri::State<'_, Services>,
    id: String,
) -> Result<omamail_app::MessageView, String> {
    commands::message(&state, &id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn settings(state: tauri::State<'_, Services>) -> Result<omamail_app::MailSettings, String> {
    commands::mail_settings(&state)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn save_settings(
    state: tauri::State<'_, Services>,
    update: omamail_app::SettingsUpdate,
) -> Result<omamail_app::MailSettings, String> {
    commands::save_settings(&state, update)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn classify(state: tauri::State<'_, Services>, all: Option<bool>) -> Result<usize, String> {
    commands::classify(&state, all.unwrap_or(false))
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn folders(state: tauri::State<'_, Services>) -> Result<Vec<omamail_app::commands::Folder>, String> {
    commands::folders(&state)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn act(
    state: tauri::State<'_, Services>,
    kind: String,
    id: String,
    label_id: Option<String>,
) -> Result<(), String> {
    commands::act(&state, &kind, &id, label_id.as_deref())
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn unsubscribe(
    state: tauri::State<'_, Services>,
    id: String,
) -> Result<omamail_app::commands::UnsubscribeResult, String> {
    commands::unsubscribe(&state, &id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn open_link(url: String) -> Result<(), String> {
    let url = url.trim();
    if !is_openable_link(url) {
        return Err("only web and mail links can be opened".into());
    }
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
    cmd.spawn()
        .map(|_| ())
        .map_err(|err| format!("could not open the link: {err}"))
}

fn is_openable_link(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let allowed = lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:");
    allowed && !url.chars().any(|ch| ch.is_control() || ch == '"' || ch == '\'')
}

#[cfg(test)]
mod tests {
    use super::is_openable_link;

    #[test]
    fn only_web_and_mail_links_open() {
        assert!(is_openable_link("https://example.com/a"));
        assert!(is_openable_link("mailto:ada@example.com"));
        assert!(!is_openable_link("javascript:alert(1)"));
        assert!(!is_openable_link("file:///etc/passwd"));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if !display::ensure_display() {
        eprintln!("omamail: no Wayland or X11 display. Launch this from the desktop session.");
        std::process::exit(1);
    }
    webkit::apply_nvidia_workaround();
    let services = tauri::async_runtime::block_on(Services::open_default()).unwrap_or_else(|err| {
        eprintln!("omamail: could not open the local database: {err}");
        std::process::exit(1);
    });

    tauri::Builder::default()
        .manage(services)
        .invoke_handler(tauri::generate_handler![
            theme,
            status,
            login,
            cancel_login,
            logout,
            set_active_account,
            sync,
            inbox,
            inbox_token,
            load_more,
            message,
            open_link,
            settings,
            save_settings,
            classify,
            folders,
            act,
            unsubscribe
        ])
        .setup(|app| {
            if let Some(watcher) = watch_theme(app.handle().clone()) {
                Box::leak(Box::new(watcher));
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building OmaMail")
        .run(|app, event| {
            if let RunEvent::WindowEvent {
                label,
                event: WindowEvent::Focused(true),
                ..
            } = event
            {
                if let Some(window) = app.get_webview_window(&label) {
                    let _ = window.emit("theme-changed", current_palette());
                }
            }
        });
}

fn watch_theme(app: tauri::AppHandle) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    let last: &'static Mutex<Option<(String, Instant)>> = {
        static LAST: OnceLock<Mutex<Option<(String, Instant)>>> = OnceLock::new();
        LAST.get_or_init(|| Mutex::new(None))
    };
    let mut watcher = notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
        if let Ok(event) = event {
            if !event.kind.is_modify() && !event.kind.is_create() {
                return;
            }
        }
        let palette = current_palette();
        let key = format!("{}:{}:{}", palette.bg, palette.text, palette.accent);
        let now = Instant::now();
        if let Ok(mut last) = last.lock() {
            if let Some((prev, at)) = last.as_ref() {
                if prev == &key || now.duration_since(*at) < Duration::from_millis(400) {
                    return;
                }
            }
            *last = Some((key, now));
        }
        let _ = app.emit("theme-changed", palette);
    })
    .ok()?;
    if let Some(parent) = colors_path().and_then(|path| path.parent().map(PathBuf::from)) {
        let _ = watcher.watch(&parent, RecursiveMode::NonRecursive);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let state = PathBuf::from(home).join(".local/state/omarchy/current");
        if state.is_dir() {
            let _ = watcher.watch(&state, RecursiveMode::NonRecursive);
        }
    }
    Some(watcher)
}
