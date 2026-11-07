# OmaMail — design notes

v1 is a local Gmail reader that should feel like OmaCal: small, keyboard-first, themed by Omarchy, no account of ours in the path.

## Stack

| Layer | Choice | Why |
| --- | --- | --- |
| Shell | Tauri v2 | One Linux/macOS window, small enough for Omarchy. |
| UI | Svelte 5 | Paints a list and a reader. Theme is CSS variables. |
| Mail | Rust crates | OAuth, Gmail, SQLite, keyring, CLI. The window does not contain mailbox logic. |
| Secrets | `keyring` | Secret Service on Linux. Refresh tokens only. |
| Store | `sqlx` + SQLite | Headers and plain bodies. No tokens. |

GTK was not used, for the same reason OmaCal skipped it: a later macOS build should not be a rewrite.

## Auth

Desktop PKCE. A gitignored `oauth.local.toml` can be compiled into a local binary. It is not committed. Env and `~/.config/omamail/oauth.toml` override it. Secrets are sent to Google and never logged.

Scopes: `openid`, `email`, `gmail.modify`. Not send.

`prompt=consent select_account` and `access_type=offline` so a repeat sign-in still returns a refresh token and the account chooser still appears.

Access tokens stay in process memory. `Debug` on token types redacts them.

## Sync

One shot, not a daemon. List INBOX, cap 50, fetch metadata concurrently, replace the cached inbox so the list matches that pull rather than growing forever. Body is fetched on open and cached. A vanished message drops its cached body.

## Theme

Read the current Omarchy theme:

1. `~/.local/state/omarchy/current/theme` or `~/.config/omarchy/current/theme` if present
2. else `~/.local/state/omarchy/current/theme.name`, then `~/.config/omarchy/themes/<name>/colors.toml`, then `$OMARCHY_PATH/themes/<name>/colors.toml`

`background` and `foreground` are required. `light_foreground` is preferred over `muted` for secondary text, because Omarchy's `muted` is often a swatch, not readable copy. Missing theme → Tokyo Night-like fallback, so the window still opens.

## CLI

The binary with no arguments opens the window. Subcommands never do. JSON envelope is `{ "ok": true, "data": ... }` or `{ "ok": false, "error": "..." }`. Exit 1 on a failed command, 2 on bad usage. Progress goes to stderr, never mixed into stdout.
