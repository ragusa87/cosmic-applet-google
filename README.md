# cosmic-applet-gmail

A small COSMIC desktop panel applet that shows the number of unread Gmail
messages and refreshes periodically.

- **Left-click** the count → opens `https://mail.google.com` in your default browser.
- **Right-click** the count → opens a menu whose only entry is **Credentials**.
  Selecting it spawns the same binary with `--show-settings`, which runs as a
  regular Wayland toplevel window (not a panel popup) so it survives focus
  changes — including switching to a password manager to paste the secret.
- Settings (email, OAuth client ID, poll interval) live in cosmic-config.
- Secrets (OAuth client secret, refresh token, access token) live in the
  freedesktop Secret Service (e.g. gnome-keyring under COSMIC).

## Build & install

Requires Rust 1.85+ (for `edition = "2024"`), `just`, and a working Wayland
session. On Pop!_OS / COSMIC the Secret Service backend is gnome-keyring;
it must be running for the applet to remember credentials.

```sh
just build-release
just install-user        # installs into ~/.local; use `sudo just install` for /usr
```

`just install-user` lays the binary, desktop entry, and icon into:

- `~/.local/bin/cosmic-applet-gmail`
- `~/.local/share/applications/com.github.ragusa87.CosmicAppletGmail.desktop`
- `~/.local/share/icons/hicolor/scalable/apps/com.github.ragusa87.CosmicAppletGmail.svg`

> ⚠️ `~/.local/bin` must be on your `$PATH` — the panel runs `Exec=cosmic-applet-gmail`
> and resolves it via `PATH`. Most distros add it automatically; check with
> `echo $PATH | tr ':' '\n' | grep .local/bin`.

### Add it to the panel

A COSMIC panel applet is **not** a standalone program — `just run` (or running
the binary directly) will not produce a panel icon, because applets are
spawned by the COSMIC panel as Wayland sub-surfaces. Once installed:

1. **Settings → Desktop → Panel** (or right-click the panel → *Configure*).
2. Scroll to **Applets** → **Add Applet**.
3. Pick **Gmail Unread** from the list and drag it into Left, Center, or Right.

If **Gmail Unread** does not appear in the Add-Applet list, the panel has
cached its applet index. Force a re-scan with one of:

```sh
pkill cosmic-panel        # the session manager respawns it within ~1s
# or: log out and back in
```

Then proceed to the [one-time Google Cloud setup](#one-time-google-cloud-setup)
below, and right-click the new panel icon → **Credentials** to authorize.

### Uninstall

```sh
just uninstall-user
```

## One-time Google Cloud setup

This applet uses a **bring-your-own-credentials** model: instead of shipping
a shared OAuth client (which would be capped at 100 unverified users), each
user creates their own Google Cloud OAuth client. It takes ~5 minutes once.

1. Open <https://console.cloud.google.com/> and create a new project (any name).
2. **APIs & Services → Library** → search for **Gmail API** → click **Enable**.
3. **APIs & Services → OAuth consent screen**:
   - User type: **External**.
   - App name: anything (e.g. "My COSMIC Gmail Applet"), support email: your own.
   - **Scopes** → Add: `https://www.googleapis.com/auth/gmail.metadata`.
   - **Test users** → Add your own Google account.
   - Leave the app in **Testing** mode (don't submit for verification — you're
     the only user).
4. **APIs & Services → Credentials → Create credentials → OAuth client ID**:
   - Application type: **Desktop app**.
   - Name: anything.
   - Click **Create**. Copy the **Client ID** and **Client secret**.
5. Right-click the applet in the panel → **Credentials**. The applet spawns
   itself with `--show-settings`, which opens a standalone window with the
   form. It's a real toplevel window so clicking other apps (e.g. a password
   manager) won't dismiss it. Close it with one of:
   - **Authorize with Google** — runs the OAuth flow (opens a browser tab to
     Google's consent screen; granting access redirects to a "you can close
     this tab" page) and exits the settings window once the refresh token is
     stored.
   - **Cancel** — exits without saving.
   - The window manager's ✕ button — same as Cancel.

   The panel applet picks up the new credentials automatically: when settings
   writes to cosmic-config, the applet's config watcher fires and triggers a
   reload of the tokens from Secret Service.

You can also launch the settings window directly without going through the
panel:

```sh
cosmic-applet-gmail --show-settings
```

## Forcing a refresh

The applet polls every `poll_interval_secs` (default 60s). To trigger an
immediate refresh (e.g. from a script, key binding, or post-commit hook):

```sh
pkill -USR2 cosmic-applet-gmail
```

On receiving SIGUSR2, the applet reloads the OAuth tokens from Secret Service
and fetches the unread count right away. The settings window (also running
as `cosmic-applet-gmail`) ignores SIGUSR2, so sending the signal to all
processes with that name is safe — only the panel applet acts on it.

Within one polling interval (default 60s) the unread count appears.

### Pre-filling credentials from the environment

For local development, the client ID and secret are read from environment
variables when the form field is empty:

```sh
export GMAIL_APPLET_CLIENT_ID=...apps.googleusercontent.com
export GMAIL_APPLET_CLIENT_SECRET=GOCSPX-...
```

A persisted value (from a previous **Authorize** click) always wins over the
environment.

## Configuration

Non-secret settings live in `~/.config/com.github.ragusa87.CosmicAppletGmail/v1/`:

| Key                 | Default | Notes                              |
|---------------------|---------|------------------------------------|
| `email`             | `""`    | Filled when you click **Authorize**. |
| `client_id`         | `""`    | Same — written from the popup form.  |
| `poll_interval_secs`| `60`    | Clamped to a minimum of 15s.         |

You can edit `poll_interval_secs` by hand; the applet picks up changes live.

Secrets are stored under Secret Service entry
`com.github.ragusa87.CosmicAppletGmail:tokens / {email}` as a JSON blob
containing `client_secret`, `refresh_token`, `access_token`, and
`expires_at_unix`.

## Troubleshooting

- **Panel shows `—` forever** → the applet has no credentials; right-click →
  Credentials to authorize.
- **Panel shows `…` forever** → credentials are present but every fetch is
  failing. Run `RUST_LOG=info cosmic-applet-gmail` from a terminal and watch
  the logs.
- **`Secret Service unavailable`** → no keyring daemon is running.
  Install / start `gnome-keyring-daemon` (it ships with COSMIC by default).
- **Refresh token expired after a week** → on Google's OAuth consent screen
  in "Testing" mode, refresh tokens expire after 7 days. Either re-authorize
  once a week, or move the app to "In production" (still no review needed
  for a single-user desktop client).
- **Authorization fails or returns stale state** → reset the OAuth connection
  by removing the app from <https://myaccount.google.com/connections>, then
  re-authorize from the Credentials window.

## Scope rationale

`gmail.metadata` is the minimum scope that exposes label counts. The applet
calls `users/me/labels/INBOX` once per poll and reads the `messagesUnread`
field — it never reads message bodies, subjects, or sender addresses.

## License

GPL-3.0-or-later.
