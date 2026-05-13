# AGENTS.md

Notes for AI coding agents (and humans new to the codebase). The README is the
user-facing doc; this file is the *contributor*-facing one.

## What this is

A Cargo workspace bundling two COSMIC desktop panel applets that share their
OAuth + Secret Service plumbing:

- **`cosmic-applet-gmail`** — Gmail unread count, polls every N seconds.
- **`cosmic-applet-google-agenda`** — Next Google Calendar event with a live
  countdown + desktop notification.
- **`cosmic-google-common`** — shared library crate exporting the OAuth2 PKCE
  flow (`auth`) and the keyring-backed token store (`secrets`). Each applet
  passes its own `scope`, `success_html`, and Secret-Service `service` string
  in and gets the rest for free.

Both applets are written in Rust on libcosmic / iced and follow the same
"one binary, two modes" shape; see [Two modes, not two binaries](#two-modes-not-two-binaries)
below.

## Workspace layout

```
cosmic-applet-google/
├── Cargo.toml                         # workspace root + workspace.dependencies
├── justfile                           # build/install/uninstall both applets
├── rust-toolchain.toml                # channel = stable
├── LICENSE.md                         # GPL-3.0-or-later + icon attribution
│
├── cosmic-google-common/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                     # pub mod auth; pub mod secrets;
│       ├── auth.rs                    # PKCE + loopback redirect, parameterized
│       │                              # on `scope` and `success_html`. Exports
│       │                              # `OAuthParams`, `start_oauth_flow`, `refresh`.
│       └── secrets.rs                 # keyring v3 wrapper. `Tokens` struct +
│                                      # `load(service, email)` / `save(service, email, tokens)`.
│
├── cosmic-applet-gmail/               # Gmail applet (see below)
│   ├── Cargo.toml
│   ├── data/                          # .desktop + icon
│   └── src/                           # main / app / settings / ui / config / gmail
│
└── cosmic-applet-google-agenda/       # Agenda applet (see below)
    ├── Cargo.toml
    ├── data/
    └── src/                           # main / app / settings / ui / config /
                                       # calendar / debug
```

## Two modes, not two binaries

A `cosmic::applet::run` process is constrained: every surface it creates
(including `surface::action::app_window`) is rendered as a transparent
sub-surface embedded in the panel. Real toplevels with WM chrome require
`cosmic::app::run`. The two entry points are incompatible in the same
process, but a single binary can dispatch to either based on `argv` — which
saves maintaining two installs and two `.desktop` files per applet.

Both applets do this:

| Mode | Entry | Surface | Trigger |
|---|---|---|---|
| Panel applet | `cosmic::applet::run::<AppModel>(())` | transparent sub-surface | default — no flag |
| Settings window | `cosmic::app::run::<SettingsApp>(...)` | regular xdg_toplevel | `--show-settings` |

The agenda binary adds two extra `argv`-selected modes (no iced involved):

| Mode | Entry | Surface | Trigger |
|---|---|---|---|
| CLI debug dump | `debug::run()` (tokio current-thread runtime) | stdout only | `--debug` |
| Test notification | one-shot `notify_rust::Notification::show()` in `main.rs` | desktop notification | `--notify` (stacks with `--debug`) |

The applet's right-click menu → **Credentials…** spawns `current_exe()` with
`--show-settings`, which is how the user reaches the OAuth setup.

## Shared OAuth + Secrets crate

`cosmic-google-common` exposes the two parts that are otherwise word-for-word
identical between applets:

- `secrets::{Tokens, load(service, email), save(service, email, tokens),
  SecretsError}`. `service` is the Secret-Service service string the
  caller chooses (Gmail uses `format!("{APP_ID}:tokens")`, agenda uses
  `APP_ID` — both forms are preserved for backwards-compat with stored
  tokens).
- `auth::{OAuthParams { scope, success_html }, start_oauth_flow(params,
  client_id, client_secret), refresh(client_id, tokens)}`. `scope` and
  `success_html` are the only things that differ between applets.

Add a new Google-backed applet later: depend on `cosmic-google-common`,
declare a per-applet `const SCOPE` and `const SUCCESS_HTML`, and reuse the
same OAuth flow.

## Storage split (both applets)

| Item | Where |
|---|---|
| `email`, `client_id`, intervals/toggles | cosmic-config (RON in `~/.config/{APP_ID}/v1/`), watched live |
| `client_secret`, `refresh_token`, `access_token`, `expires_at_unix` | Secret Service via `keyring` v3, one JSON blob keyed by `email` |

Cross-binary propagation: the settings binary writes both. The applet's
`watch_config::<Config>` subscription delivers `Message::UpdateConfig` when
either field changes; the applet then reloads tokens from the keyring and
triggers an immediate refetch. No IPC.

## SIGUSR2 → force refresh

Both applets listen for SIGUSR2 (subscription in `src/app.rs::sigusr2_stream`,
built on `tokio::signal::unix`). On receipt → reload tokens → fetch.

The settings mode installs `SIG_IGN` for SIGUSR2 at startup so
`pkill -USR2 cosmic-applet-…` (which would match both modes' processes by
name) doesn't terminate an open settings window. See each crate's
`src/settings.rs::run`.

## Per-applet specifics

### cosmic-applet-gmail

- **APP_ID**: `com.github.ragusa87.CosmicAppletGmail`
- **Secret Service service**: `{APP_ID}:tokens`
- **Config schema**: `email`, `client_id`, `poll_interval_secs` (default 60,
  clamp ≥15)
- **OAuth scope**: `https://www.googleapis.com/auth/gmail.metadata`
- **API call**: single `GET users/me/labels/INBOX` per poll → `messagesUnread`
- **Files**:

```
src/
├── main.rs        argv → applet::run or app::run (settings)
├── app.rs         panel applet — Application impl, panel button view,
│                  right-click menu popup, polling subscription,
│                  SIGUSR2 listener, token refresh + fetch loop
├── settings.rs    standalone settings app — toplevel, OAuth flow,
│                  writes config + tokens via cosmic-google-common, exits
├── ui.rs          shared widgets — menu popup view, credentials form view
│                  (generic over Message via `CredentialsHandlers<M>`)
├── config.rs      cosmic-config schema + APP_ID
└── gmail.rs       single GET on users/me/labels/INBOX → messagesUnread
                   (+ JSON-parsing unit tests)
```

### cosmic-applet-google-agenda

- **APP_ID**: `com.github.ragusa87.CosmicAppletGoogleAgenda`
- **Secret Service service**: `{APP_ID}` (note: agenda historically did not
  append `:tokens`; preserved to avoid invalidating existing keyring entries)
- **Config schema**: `email`, `client_id`, `fetch_interval_secs` (default 300,
  clamp ≥60), `display_tick_secs` (default 30, clamp ≥5),
  `notification_lead_secs` (default 300, `0` disables), `notify`, `show_title`,
  `show_time`, `show_progress`
- **OAuth scope**: `https://www.googleapis.com/auth/calendar.events.readonly`
- **API call**: `GET /calendar/v3/calendars/primary/events?timeMin=...&timeMax=...&singleEvents=true&orderBy=startTime` once per fetch interval
- **Files**:

```
src/
├── main.rs        argv → applet::run / app::run / debug::run / fire test notification
├── app.rs         panel applet — Application impl, panel button view, right-click
│                  menu popup, two timer subscriptions (display 30s, fetch 5min),
│                  SIGUSR2 listener, token refresh + fetch loop, notification dispatch
├── settings.rs    standalone settings app — toplevel, OAuth flow,
│                  writes config + tokens via cosmic-google-common, exits
├── debug.rs       --debug CLI — prints config, loads tokens, refreshes if needed,
│                  calls calendar::debug_fetch, dumps every event with KEEP/SKIP.
│                  No GUI. Spins its own tokio current-thread runtime.
├── ui.rs          shared widgets — menu popup view (incl. event_info_view,
│                  settings_view), credentials form view
├── config.rs      cosmic-config schema + APP_ID
└── calendar.rs    GET /calendar/v3/calendars/primary/events → Vec<Event>
                   (id, summary, start, end, meet_url). `classify` filters
                   cancelled / all-day / transparent / declined; `debug_fetch`
                   returns Vec<DebugItem> for --debug. (+ JSON-parsing tests)
```

#### Two timers (display vs. fetch)

`AppModel` caches the event list in `self.events` and runs two independent
timer subscriptions, batched in `subscription()`:

- **display tick** (`display_tick_secs`, default 30s) → `Message::Tick`. Pure
  local recompute: drops events whose end is in the past from the cache,
  picks `self.next`, recomputes the relative-time string for `view()`, and
  fires `maybe_notify` (one-shot per event id, tracked in `self.notified`).
- **fetch tick** (`fetch_interval_secs`, default 5min) → `Message::Refetch`.
  Refreshes the access token if needed, then calls `calendar::upcoming_events`
  and replaces `self.events`. Chains an immediate `Tick` so the display
  updates.

Network blips therefore only delay the next *refetch* — the countdown
continues smoothly from cached events. `notified` is pruned on every Tick to
drop ids no longer in the upcoming window, so recurring meetings notify again
the next day.

#### Event filtering rules (`src/calendar.rs::classify`)

Applied to the raw API response, in order:

1. Drop `status == "cancelled"`.
2. Drop `transparency == "transparent"` ("Free"-marked).
3. Drop self-declined: an attendee with `self == true` and
   `responseStatus == "declined"`.
4. Drop all-day (`start.date` present, `start.dateTime` missing).

`classify` returns `Result<DateTime<Utc>, SkipReason>`. The applet uses
`map_event` (`classify(...).ok() → build_event`) to drop skipped events
silently; the `--debug` CLI uses `to_debug_item` to print every event with
its verdict so you can see *why* something was filtered.

Meet-link extraction prefers `conferenceData.entryPoints[]` with
`entryPointType == "video"` and `uri` starting `https://meet.google.com/`,
and falls back to the top-level legacy `hangoutLink`.

#### Notifications

`maybe_notify` (in `src/app.rs`) is a one-shot per event id: when the next
event's start is within `notification_lead_secs` of now, it inserts the id
into `self.notified` and spawns a `tokio::task::spawn_blocking` that calls
`notify_rust::Notification::show()`. Setting `notification_lead_secs = 0`
disables all notifications.

## Build / run / test commands

```sh
just check                   # cargo clippy --workspace --all-features
just build-release           # cargo build --release (both binaries)
just install-user            # ~/.local/{bin,share/applications,share/icons/...}
just run-gmail               # cargo run -p cosmic-applet-gmail (panel, headless)
just run-agenda              # cargo run -p cosmic-applet-google-agenda
cargo test --workspace       # JSON parsing tests + helper tests
```

There is **no automated UI test** — a real COSMIC session is required. After
changes to `view()`, panel layout, or popup logic, install + `pkill
cosmic-applet-…` and the panel respawns it. Then:

- Right-click → menu shows "Credentials…"
- Left-click — gmail opens mail.google.com; agenda opens Meet link of next
  event (fallback `calendar.google.com`)
- `pkill -USR2 cosmic-applet-…` → immediate refresh
- `cosmic-applet-… --show-settings` from a terminal → settings window
  (useful for UI iteration without rebuilding the panel)
- agenda only: `cosmic-applet-google-agenda --debug` → dumps the raw event
  classification, no GUI

## Conventions (applies to all crates)

- **clippy pedantic is mandatory.** `just check` must stay clean. The one
  `#[allow(clippy::too_many_lines)]` on each `App::update` is intentional —
  keep the message dispatch flat; don't split it just to shrink line count.
- **No `unwrap()` or `expect()`** in normal paths. Use `anyhow::Result` for
  fallible work, log with `tracing::warn!(error = %e, ...)` when an error is
  recovered from but worth noting.
- **No comments explaining *what* the code does** — only *why* when it's
  non-obvious (subtle invariant, Wayland quirk, libcosmic-API workaround).
  See e.g. the `LeftClick` guard comment, the `SIG_IGN` rationale, the
  all-day-event filter comment.
- **No docstrings on private items.** Public API of the modules (`pub fn`)
  gets a one-line summary at most.
- **Don't add `derive(Default)` to enums** unless `#[default]` makes sense
  semantically.
- **Shared dependencies belong in `[workspace.dependencies]`** at the root
  Cargo.toml. Member crates reference them with `{ workspace = true }`.

## libcosmic 1.0 gotchas (learned the hard way)

- `cosmic::Task<M>` from `cosmic::prelude::*` is `iced::Task<M>` — *not* the
  `iced::Task<Action<M>>` the trait wants. Import `cosmic::app::Task`
  explicitly. The prelude re-export is misleading.
- `cosmic::iced_winit::commands::popup` (referenced in the official template)
  doesn't exist; use `cosmic::surface::action::{app_popup, destroy_popup,
  app_window, destroy_window}` and dispatch them via
  `cosmic::task::message(cosmic::Action::Cosmic(cosmic::app::Action::Surface(a)))`.
  The `dispatch_surface` helper in each `app.rs` encapsulates this.
- `Application::title(&self, id)` (with the `multi-window` feature) is on
  `ApplicationExt`, which has a *blanket* impl — you cannot override it.
  `core.set_title(id, ...)` exists but returns `Task::none()` (no-op). There
  is currently no public way to set per-window titles; settings shows a
  `text::title4(...)` heading inside the window instead.
- `keyring` v4 is the deprecated CLI/sample crate. Use `keyring` **v3**
  (`sync-secret-service` + `crypto-rust` features) for the library API. The
  workspace pins v3.
- `Subscription::run_with_id` (in older templates) is gone; use
  `Subscription::run(fn_pointer)` where the fn pointer's address is the
  identity. For dynamic-stream subscriptions wrap a `cosmic::iced::stream::
  channel(buffer, async closure)` call inside a `fn() -> impl Stream`.
- `text(...).color(Color)` requires `Theme::Class: From<StyleFn>` which
  cosmic's text theme doesn't satisfy. Use `text(...).class(Color::WHITE)`
  instead — `cosmic::theme::Text: From<Color>` works.
- Panel popups with `grab: false` *still* get dismissed by COSMIC when focus
  changes (compositor-side decision, not our flag). The settings window had
  to be a real toplevel (`app_window` from `cosmic::app::run`, NOT from
  inside the applet) for this reason.
- `text` widgets center their glyph inside their line-height box by default.
  To put a glyph at a corner of a container you need `text.align_x(End)
  .align_y(End)` *and* the container's `align_x(Right).align_y(Bottom)` —
  one without the other looks centered. See each `view()` in `app.rs`.
- Always use `self.core.applet.suggested_padding(true)` (returns a
  `(major, minor)` tuple) and rotate horizontal vs vertical based on
  `self.core.applet.anchor`. Wrap final widget in
  `self.core.applet.autosize_window(...)` so the panel sizes the surface
  correctly. See each `view()`.

## Don't

- Don't write to `target/`, `Cargo.lock`, `data/icons/` from agents without
  asking; these are part of the working state the user iterates on.
- Don't commit. The user asks explicitly when commits are wanted.
- Don't add a second `[[bin]]` entry to either applet. The `--show-settings`
  split exists *specifically* to avoid the maintenance cost of two binaries;
  if you find yourself wanting two, ask first.
- Don't change `APP_ID` or the Secret-Service service string of either
  applet; existing users have stored tokens under those keys.
- Don't introduce a global async runtime — libcosmic / iced own the runtime.
  Async work goes through `cosmic::task::future` or
  `tokio::task::spawn_blocking` (for the sync keyring + notify-rust APIs).
- Don't extract a third workspace crate "just in case." `cosmic-google-common`
  exists because two applets word-for-word duplicated 250+ LOC; do the same
  only when a third applet starts duplicating something else.
