//! Embed a local Desktop OAuth client when `oauth.local.toml` exists.
//! That file is gitignored. A clone without it still builds; sign-in then
//! needs `OMAMAIL_GOOGLE_CLIENT_ID` or `~/.config/omamail/oauth.toml`.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=oauth.local.toml");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let local = manifest.join("oauth.local.toml");
    let (client_id, client_secret) = std::fs::read_to_string(&local)
        .ok()
        .map(|text| (field(&text, "client_id"), field(&text, "client_secret")))
        .unwrap_or_default();
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("bundled_oauth.rs");
    std::fs::write(
        out,
        format!(
            "pub const BUNDLED_CLIENT_ID: &str = \"{}\";\npub const BUNDLED_CLIENT_SECRET: &str = \"{}\";\n",
            escape(&client_id),
            escape(&client_secret)
        ),
    )
    .unwrap();
}

fn field(text: &str, key: &str) -> String {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() != key {
            continue;
        }
        return value.trim().trim_matches('"').trim_matches('\'').to_string();
    }
    String::new()
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
