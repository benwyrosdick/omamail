# Google OAuth client

A local build can compile in a Desktop client from the gitignored file `crates/omamail-app/oauth.local.toml` (copy `oauth.local.toml.example`). That file is never committed. People still sign in with their own Google account. The client secret identifies the app to Google. It is not their password, and it does not let the app read mail they have not consented to.

Until the Cloud app is verified, Google shows an "unverified app" warning, and sign-in only works for accounts listed as test users. Add each person under the consent screen, or publish and verify the app, or other people stop at `access_denied`.

`omamail auth login` uses that local client when it was compiled in. It does not write the secret into `oauth.toml` or into git.

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

Copy the client secret too. Google's token endpoint requires it for a Desktop client. Keep it in `oauth.local.toml` or `~/.config/omamail/oauth.toml`, never in git.

## 4. Sign in

```bash
omamail auth login
```

That opens the browser with the compiled-in client, if `oauth.local.toml` was present at build time. Otherwise set `OMAMAIL_GOOGLE_CLIENT_ID` and `OMAMAIL_GOOGLE_CLIENT_SECRET`, or write `~/.config/omamail/oauth.toml`. The environment wins over the file, and either wins over a compiled-in client.

Do not commit `oauth.toml`. It is gitignored.

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
| `invalid_request` / `client_secret is missing` | Run `auth login` in a terminal and paste the Desktop client secret, or add `client_secret` to `oauth.toml`. |
| `invalid_client` | The secret does not belong to this client id. |
| `accessNotConfigured` / Gmail API has not been used | Enable the Gmail API on that project. |
| No refresh token | Revoke OmaMail at [Google account permissions](https://myaccount.google.com/permissions) and sign in again. |
| Keyring hang or `keyring unavailable` | Start a secret service. A minimal Hyprland session does not always have one. |
