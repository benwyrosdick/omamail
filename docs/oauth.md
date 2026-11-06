# Google OAuth client

OmaMail does not ship a client id or secret. Create a Desktop client on a Cloud project you control, and add yourself as a test user. Until the app is verified, Google shows an "unverified app" warning — that is expected.

## 1. Cloud project

1. Open [Google Cloud Console](https://console.cloud.google.com/).
2. Create a project, or pick one you already use for local tools.
3. Enable the **Gmail API** (APIs & Services → Library → Gmail API).

## 2. Consent screen

1. APIs & Services → OAuth consent screen.
2. User type: External is fine. Publishing is not required.
3. Add your Google account under **Test users**. Without this, sign-in fails with `access_denied` even when the password is right.
4. Scopes the app will request: `openid`, `email` / `userinfo.email`, and `https://www.googleapis.com/auth/gmail.readonly`. You can add them on the consent screen or let the first sign-in prompt for them.

## 3. Client id

1. APIs & Services → Credentials → Create credentials → OAuth client ID.
2. Application type: **Desktop app**. A Web client will reject the loopback redirect.
3. Copy the client id. It looks like `something.apps.googleusercontent.com`.

Desktop clients may also show a client secret. Google does not treat that secret as confidential. Leave it unset unless the token endpoint answers `invalid_client`.

## 4. Configure OmaMail

```bash
mkdir -p ~/.config/omamail
cp oauth.toml.example ~/.config/omamail/oauth.toml
```

Edit the copy:

```toml
client_id = "something.apps.googleusercontent.com"
# client_secret = "only-if-token-exchange-rejects-the-public-client"
```

Or export `OMAMAIL_GOOGLE_CLIENT_ID` (and, only if needed, `OMAMAIL_GOOGLE_CLIENT_SECRET`). The environment wins over the file.

Do not commit `oauth.toml`. It is gitignored.

## 5. Sign in

```bash
omamail auth login
```

A browser opens on `127.0.0.1`. The refresh token goes to the OS keyring under the service name `omamail`. If the secret service is not running (`gnome-keyring`, KeePassXC, or kwallet), login fails with `keyring unavailable` rather than writing the token to SQLite.

Then:

```bash
omamail sync
omamail inbox list
```

Or open the window with `omamail` and use Connect Google.

## If it fails

| Symptom | Fix |
| --- | --- |
| `access_denied` | Add yourself as a test user. |
| `invalid_client` | Put the Desktop client secret in `oauth.toml`, or the client id is from a different project than you think. |
| `accessNotConfigured` / Gmail API has not been used | Enable the Gmail API on that project. |
| No refresh token | Revoke OmaMail at [Google account permissions](https://myaccount.google.com/permissions) and sign in again. |
| Keyring hang or `keyring unavailable` | Start a secret service. A minimal Hyprland session does not always have one. |
