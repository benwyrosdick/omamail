# Roadmap

v1 is Google OAuth and a read-only Gmail inbox. The order below is the intended one; nothing under it is started.

## Providers

- **Microsoft 365 / Outlook** via OAuth, behind the same `MailProvider` trait.
- **Fastmail** and other OAuth mailbox hosts.
- **IMAP**, then **POP3**, for hosts that have no API. Read first, write later. A provider row already exists on `accounts` (`google` today, `imap` / `pop` when those backends land).

POP and IMAP should not special-case the UI. They implement `MailProvider` and the store keeps provider ids as opaque strings.

## Mail the window still cannot do

- Send, reply, forward, drafts.
- Mark read, archive, trash, stars, labels.
- Threads (the thread id is already stored).
- Search.
- Attachments.
- A sandboxed HTML view. Plain text is the contract until that exists.
- More than one signed-in account at a time.
- Background sync, notifications, a tray icon.
- An Omarchy bar widget for the unread count, in the style of `omacal.upcoming`.

## Around the app

- Agent skill install, stable enough that a terminal agent can read the same cache the window does. The CLI JSON envelope is the start of that.
- Packaging: desktop entry, AppImage, signature check on update.
- macOS keychain (the `keyring` crate already abstracts it; it is untested here).
