# cosmic-applet-google-agenda

A small COSMIC desktop panel applet that shows the **next event** on your
Google Calendar with a live countdown, and fires a desktop notification a
few minutes before it starts.

- **Left-click** the icon → opens the Google Meet link of the next event in
  your default browser, or falls back to `https://calendar.google.com`.
- **Right-click** → menu with **Credentials**. Selecting it spawns the same
  binary with `--show-settings`, which runs as a regular Wayland toplevel
  window (not a panel popup) so it survives focus changes — including
  switching to a password manager to paste the secret.
- The countdown (`12m`, `1h`, `now`) updates **every 30s locally** from a
  cached event list — the Calendar API is only hit every 5 minutes.
- Settings (email, OAuth client ID, intervals, lead time, title toggle)
  live in cosmic-config.
- Secrets (OAuth client secret, refresh token, access token) live in the
  freedesktop Secret Service (e.g. gnome-keyring under COSMIC).

## What gets filtered out

The applet ignores:

- **Cancelled** events.
- **All-day** events (no precise start time).
- Events you marked as **Free** (Calendar's `transparency=transparent`).
- Events where **you** declined the invite.

## Build & install

Requires Rust 1.85+ (for `edition = "2024"`), `just`, and a working Wayland
session. On Pop!_OS / COSMIC the Secret Service backend is gnome-keyring;
it must be running for the applet to remember credentials.

```sh
just build-release
just install-user        # installs into ~/.local; use `sudo just install` for /usr
```

`just install-user` lays the binary, desktop entry, and icon into:

- `~/.local/bin/cosmic-applet-google-agenda`
- `~/.local/share/applications/com.github.ragusa87.CosmicAppletGoogleAgenda.desktop`
- `~/.local/share/icons/hicolor/scalable/apps/com.github.ragusa87.CosmicAppletGoogleAgenda.svg`

> ⚠️ `~/.local/bin` must be on your `$PATH` — the panel runs
> `Exec=cosmic-applet-google-agenda` and resolves it via `PATH`. Most distros
> add it automatically; check with
> `echo $PATH | tr ':' '\n' | grep .local/bin`.

### Add it to the panel

A COSMIC panel applet is **not** a standalone program — `just run` (or
running the binary directly) will not produce a panel icon, because applets
are spawned by the COSMIC panel as Wayland sub-surfaces. Once installed:

1. **Settings → Desktop → Panel** (or right-click the panel → *Configure*).
2. Scroll to **Applets** → **Add Applet**.
3. Pick **Next meeting** from the list and drag it into Left, Center, or Right.

If **Next meeting** does not appear in the Add-Applet list, the panel has
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
2. **APIs & Services → Library** → search for **Google Calendar API** →
   click **Enable**.
3. **APIs & Services → OAuth consent screen**:
   - User type: **External**.
   - App name: anything (e.g. "My COSMIC Agenda Panel"), support email: your own.
   - **Scopes** → Add: `https://www.googleapis.com/auth/calendar.events.readonly`.
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
cosmic-applet-google-agenda --show-settings
```

## Debugging what the panel sees

If the panel isn't showing the event you expect (or *is* showing one you
don't), run the binary with `--debug`. It uses the stored credentials,
hits the Calendar API once, and prints every fetched event to stdout
together with the verdict (`KEEP` or `SKIP — <reason>`):

```sh
cosmic-applet-google-agenda --debug
```

The bottom of the report shows the configured intervals, which event would
be displayed next, and when a notification would fire. No GUI, no panel
required.

## Forcing a refresh

The applet hits the Calendar API every `fetch_interval_secs` (default 300s).
To trigger an immediate fetch (e.g. from a script, key binding, or post-commit
hook):

```sh
pkill -USR2 cosmic-applet-google-agenda
```

On receiving SIGUSR2, the applet reloads the OAuth tokens from Secret Service
and refetches events right away. The settings window (also running as
`cosmic-applet-google-agenda`) ignores SIGUSR2, so sending the signal to all
processes with that name is safe — only the panel applet acts on it.

### Pre-filling credentials from the environment

For local development, the client ID and secret are read from environment
variables when the form field is empty:

```sh
export AGENDA_PANEL_CLIENT_ID=...apps.googleusercontent.com
export AGENDA_PANEL_CLIENT_SECRET=GOCSPX-...
```

A persisted value (from a previous **Authorize** click) always wins over the
environment.

## Configuration

Non-secret settings live in `~/.config/com.github.ragusa87.CosmicAppletGoogleAgenda/v1/`:

| Key                       | Default | Notes                                              |
|---------------------------|---------|----------------------------------------------------|
| `email`                   | `""`    | Filled when you click **Authorize**.               |
| `client_id`               | `""`    | Same — written from the settings form.             |
| `fetch_interval_secs`     | `300`   | Calendar API poll cadence. Clamped to min 60s.     |
| `display_tick_secs`       | `30`    | Local countdown refresh. Clamped to min 5s.        |
| `notification_lead_secs`  | `300`   | Notify this many seconds before start. `0` disables. |
| `show_title`              | `true`  | Show event title next to the countdown.            |

You can edit these by hand; the applet picks up changes live.

Secrets are stored under Secret Service entry
`com.github.ragusa87.CosmicAppletGoogleAgenda / {email}` as a JSON blob containing
`client_secret`, `refresh_token`, `access_token`, and `expires_at_unix`.

## Troubleshooting

- **Panel shows the icon with no countdown** → either no credentials yet
  (right-click → Credentials) or no upcoming event in the next 24h.
- **Countdown never appears with credentials configured** → every fetch is
  failing. Run `RUST_LOG=info cosmic-applet-google-agenda` from a terminal and
  watch the logs.
- **`Secret Service unavailable`** → no keyring daemon is running.
  Install / start `gnome-keyring-daemon` (it ships with COSMIC by default).
- **Refresh token expired after a week** → on Google's OAuth consent screen
  in "Testing" mode, refresh tokens expire after 7 days. Either re-authorize
  once a week, or move the app to "In production" (still no review needed
  for a single-user desktop client).
- **Re-authorize from scratch / revoke access** → visit
  <https://myaccount.google.com/connections>, pick the app, and remove its
  access. The next **Authorize with Google** click will run the full consent
  flow again.

## Scope rationale

`calendar.events.readonly` is the minimum scope that exposes event titles,
times, and conference data. The applet calls
`calendars/primary/events` once per fetch interval — it never modifies
events, never sees attendee details beyond your own RSVP status, and never
touches other calendars.

## License

GPL-3.0-or-later.
