# OmaMail

Gmail, the Omarchy way. A local mail client in the same spirit as [OmaCal](https://github.com/x3me/omacal): one binary is the window and the CLI, mail lives in SQLite, tokens live in the keyring, and colours follow the Omarchy theme. Nothing of yours passes through a server we run.

v1 is **Google sign-in and a read-only Gmail inbox**. Other providers, and direct POP/IMAP, are on the [roadmap](ROADMAP.md) — the mailbox trait is already the seam they will implement.

The mailbox, OAuth, store, and CLI are Rust. The window is a thin Svelte shell (Tauri v2), same split OmaCal uses, so the theme is just CSS variables and the app can move to macOS later without a GTK rewrite.

## What v1 does

- Connect one Google account with loopback PKCE. A local build can compile in a Desktop client from the gitignored `crates/omamail-app/oauth.local.toml`. That file is not committed. See [docs/oauth.md](docs/oauth.md).
- Sync the latest **50** inbox messages (headers and snippets).
- Open a message and render its HTML, with scripts removed. Plain text is the fallback. Remote images load.
- Follow `colors.toml` from the current Omarchy theme, including a live reload when the theme file changes.
- CLI, JSON on stdout, non-zero exit on failure, no prompts:

```bash
omamail auth login
omamail auth status
omamail auth logout
omamail sync
omamail inbox list
omamail message show <id>
```

No arguments opens the window. `j` / `k` move, `Enter` opens, `r` syncs.

## Layout

```
crates/omamail-core     accounts, messages, MailProvider
crates/omamail-google   PKCE + Gmail REST (readonly)
crates/omamail-store    SQLite. Tokens never land here.
crates/omamail-sync     one-shot inbox replace
crates/omamail-app      paths, keyring, theme, CLI
src-tauri               window commands
ui                      list + reader
```

Config: `~/.config/omamail/oauth.toml`
Database: `~/.local/share/omamail/omamail.db`
Keyring service: `omamail`

`OMAMAIL_ROOT` points both directories at `<root>/config` and `<root>/data` for a scratch run.

## Develop

```bash
cargo test
npm install
npm --prefix ui install
npm run dev
```

`npm run dev` is `tauri dev`. The first Rust build pulls WebKitGTK and takes a while.

## Not in v1

Send, reply, mark-read, labels, attachments, HTML rendering, threads, multiple accounts, background sync, the bar widget, POP/IMAP, and other providers. See [ROADMAP.md](ROADMAP.md).
