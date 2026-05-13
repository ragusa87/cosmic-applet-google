# AGENTS.md

Notes for AI coding agents (and humans new to the codebase). The README is the
user-facing doc; this file is the *contributor*-facing one.

## What this is

A Cargo workspace bundling three COSMIC desktop panel applets — two share
OAuth + Secret Service plumbing for Google APIs, the third is a standalone
time tracker:

- **`cosmic-applet-gmail`** — Gmail unread count, polls every N seconds.
- **`cosmic-applet-google-agenda`** — Next Google Calendar event with a live
  countdown + desktop notification.
- **`cosmic-applet-taxi`** — Multi-timer time tracking with daily auto-export
  to a [taxi](https://github.com/sephii/taxi) timesheet (`~/zebra/%Y/%m.tks`).
  No OAuth, no Google. Reads `~/.config/taxi/taxirc` directly, shells out
  to `taxi` via `uv run` for alias listing and updates.
- **`cosmic-google-common`** — shared library crate (gmail + agenda only)
  exporting the OAuth2 PKCE flow (`auth`) and the keyring-backed token
  store (`secrets`). The taxi applet does not depend on this crate.

All three applets are written in Rust on libcosmic / iced and follow the
"one binary, multiple modes" shape; see
[Two modes, not two binaries](#two-modes-not-two-binaries) below.

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
├── cosmic-applet-google-agenda/       # Agenda applet (see below)
│   ├── Cargo.toml
│   ├── data/
│   └── src/                           # main / app / settings / ui / config /
│                                      # calendar / debug
│
└── cosmic-applet-taxi/                # Taxi tracker applet (see below)
    ├── Cargo.toml                     # NO dep on cosmic-google-common
    ├── data/                          # .desktop + icon
    └── src/                           # main / app / settings / export / ui /
                                       # config / state / sessions / taxi / lock
```

## Two modes, not two binaries

A `cosmic::applet::run` process is constrained: every surface it creates
(including `surface::action::app_window`) is rendered as a transparent
sub-surface embedded in the panel. Real toplevels with WM chrome require
`cosmic::app::run`. The two entry points are incompatible in the same
process, but a single binary can dispatch to either based on `argv` — which
saves maintaining two installs and two `.desktop` files per applet.

All three applets do this:

| Mode | Entry | Surface | Trigger |
|---|---|---|---|
| Panel applet | `cosmic::applet::run::<AppModel>(())` | transparent sub-surface | default — no flag |
| Settings window | `cosmic::app::run::<SettingsApp>(...)` | regular xdg_toplevel | `--show-settings` |

The agenda binary adds two extra `argv`-selected modes (no iced involved):

| Mode | Entry | Surface | Trigger |
|---|---|---|---|
| CLI debug dump | `debug::run()` (tokio current-thread runtime) | stdout only | `--debug` |
| Test notification | one-shot `notify_rust::Notification::show()` in `main.rs` | desktop notification | `--notify` (stacks with `--debug`) |

The taxi binary adds one extra mode:

| Mode | Entry | Surface | Trigger |
|---|---|---|---|
| Export dialog | `export::run()` (`cosmic::app::run`) | regular xdg_toplevel | `--show-export` |

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

### cosmic-applet-taxi

- **APP_ID**: `com.github.ragusa87.CosmicAppletTaxi`
- **No keyring entry** — no OAuth involved. No `cosmic-google-common`
  dependency.
- **Config schema** (`src/config.rs`): `cutover_hour` (u8, default 4),
  `merge_gap_minutes` (u32, default 5), `round_min_minutes` (u32, default
  15), `taxi_command` (String, default
  `"uv run --with taxi,taxi-zebra taxi"`), `taxirc_path` (String, blank
  → resolve `~/.config/taxi/taxirc`).
- **Persistent state**: `~/.local/state/cosmic-applet-taxi/state.json`
  (`AppState { timers, suppressed_aliases, total_selected,
  schema_version }`) — atomic write via `state.json.tmp` + rename.
  cosmic-config is *not* used for the timers/sessions vec because it
  grows dynamically and isn't best expressed as a RON schema; small
  scalar settings still go through cosmic-config and `watch_config`.
- **uv gating**: taxi-related features (export, alias-list refresh,
  daily auto-export) only activate when `uv --version` succeeds.
  `TaxiRunner::detect` runs once at startup and on SIGUSR2. When
  unavailable, the popup shows an "Install `uv` to enable" caption and
  the Export button is disabled.
- **Hard invariant**: at most one timer has an open session at a time.
  `state::start_timer` calls `pause_all_running` first. Means panel can
  always show "the running timer" unambiguously.
- **Hard invariant**: `Timer.alias` is unique across `state.timers`.
  `state::add_timer` returns `None` if the alias is taken.
- **Files**:

```
src/
├── main.rs        argv → applet::run / settings::run (--show-settings) /
│                  export::run (--show-export)
├── app.rs         panel applet — Application impl, popup wiring, 1s + 60s
│                  ticks, sigusr2 listener, dbus lock listener, message
│                  dispatch, persist-on-mutation. Auto-export runs on
│                  the 60s tick and skips while the edit form is open.
├── settings.rs    standalone settings window (cut-over hour, merge gap,
│                  round min, taxi command, taxirc path, "Refresh aliases"
│                  button, uv diagnostic)
├── export.rs      standalone export dialog — date input, editable
│                  `text_editor` preview, collapsible "show current file
│                  content", Export / Push / Copy buttons. Reads state.json,
│                  writes via taxi::replace_day, removes exported sessions,
│                  signals the applet with pkill -USR2.
├── ui.rs          popup_view, timer_row, edit_row (per-session table,
│                  description is a multi-line `text_editor`),
│                  total_row, footer_row (icon buttons + tooltips),
│                  add_row (alias autocomplete with dismiss-on-pick)
├── config.rs      cosmic-config schema + APP_ID. `round_min_minutes` is
│                  reused as the quantize grid.
├── state.rs       Timer / Session / AppState structs + JSON persistence +
│                  cutover_date helper + mutation helpers
├── sessions.rs    pure: group_by_date, merge (description dedup + " / "
│                  concat), split_zero_duration (< 1 min threshold),
│                  quantize_grid (asymmetric: nearest-with-threshold start,
│                  ceil end), aggregate_zero / aggregate_lines (consolidate
│                  > 3 sub-minute sessions), export_lines (incl. comment
│                  lines and duration format for cross-midnight) + tests
├── taxi.rs        Taxirc parser (configparser, walks every
│                  [<backend>_aliases] section), parse_tks line-iterator,
│                  replace_day (overwrites the target date's section,
│                  preserves other days — manual export path),
│                  append_day (non-destructive sibling: inserts a
│                  `# --- appended <ts> ---` marker and appends new
│                  body lines under the existing date section — auto
│                  export path), TaxiRunner (uv subprocess wrapper),
│                  parse_alias_list (tolerant: =, ->, :, whitespace forms)
│                  + tests
└── lock.rs        zbus 5: subscribes to org.freedesktop.ScreenSaver
                   ActiveChanged (session bus) AND
                   org.freedesktop.login1.Manager PrepareForSleep (system
                   bus); coalesces into LockEvent::{Locked, Unlocked}.
                   Failures are logged and ignored — applet still works
                   manually.
```

#### Business logic: time, duration, and taxi export

This is the heart of the applet. Read it before changing anything in
`sessions.rs`, `state.rs::sum_for_date`/`cutover_date`, `taxi.rs::
replace_day`, or the export pipeline in `app.rs` /  `export.rs`. A
walkthrough of how a click on the popup's ▶ becomes a line in
`~/zebra/2026/05.tks`.

##### 1. Session capture

`Session { start, end, description }`. `end: None` means the session
is currently running (only one such session can exist across all
timers — invariant from `start_timer`). Timestamps are full
`DateTime<Local>` with sub-second precision (they come from
`Local::now()` at the moment of click).

Description is **per-session**: snapshots from
`Timer.default_description` on start, freely editable. When the
running session's description is edited, the timer's default is also
updated (**sticky default**) so the next start pre-fills the same
text. When the timer is paused, edits target only the default.

Pausing closes the session with `end = Some(now)`. Resuming on the
same timer pushes a fresh `Session`.

##### 2. The cut-over hour

`config.cutover_hour` (default `4`) is the boundary between
"yesterday's" and "today's" work. `sessions::cutover_date(t,
cutover_hour) = (t - cutover_hour hours).date_naive()`. A session
that started at `02:30` with cutover `4` belongs to the **previous
calendar day**'s timesheet — useful when you sometimes work past
midnight.

`group_by_date(sessions, cutover_hour)` partitions all closed
sessions by their cut-over-shifted date. This is what determines
which `.tks` section each session ends up in.

##### 3. Merge: collapse pause/resume hiccups

`sessions::merge(sessions, gap)`:
- sort by `start`,
- collapse adjacent sessions whose gap < `config.merge_gap_minutes`
  (default 5) into one `Span`,
- when collapsing, the merged span's `description` is the deduped
  " / " join of the inputs' descriptions (empties dropped, order
  preserved).

So `start → 09:00 / pause @ 09:30 / resume 09:32 / pause @ 10:00`
becomes **one** entry `09:00-10:00`, not two. But a 10-min coffee
break opens a real boundary.

Single-input spans keep their description unchanged.

##### 4. Sub-minute drop / aggregate

`sessions::split_zero_duration(spans)` partitions by
**`duration < 1 minute`** — not exact `start == end`. This catches
clicks where the user hit start and pause within the same minute
(real timestamps have sub-second precision, so even a `~30s` session
satisfies `start != end` but isn't real work). Without this, a
30-second span would get quantized to `09:15-09:15` then
belt-and-braces-bumped to a `09:15-09:30` 15-min entry.

For each (timer, date) bucket of sub-minute spans:
- **count ≤ 3** → dropped silently.
- **count > 3** → aggregated via `sessions::aggregate_zero` +
  `sessions::aggregate_lines` into one duration-format line:
  ```
  # 5 zero-duration sessions consolidated into 15 min
  _alias 0.25 deduped / descriptions
  ```
  Duration is one grid unit (`round_min_minutes` = 15 → `0.25h`).

##### 5. Quantize: snap to 15-min grid (asymmetric)

`sessions::quantize_grid(spans, grid_minutes)` applies an
**asymmetric** rounding:

- **Start: nearest with DOWN-biased threshold.** `threshold = ceil(
  grid_minutes / 2)`. For `grid=15` → threshold `8`. Compute `offset =
  trunc_minute % grid`; if `offset ≤ 8` → DOWN (`trunc - offset min`),
  else UP (`trunc + (grid - offset) min`). Sub-minute precision is
  truncated to whole minutes first.

  Examples: `09:03→09:00`, `09:07→09:00`, `09:08→09:00`, `09:09→09:15`,
  `09:14→09:15`, `09:38→09:30`, `09:39→09:45`.

- **End: ceil up.** Truncate sub-minute precision first. If the
  truncated minute is on-grid (`offset == 0`) return that minute (so
  `22:30:05 → 22:30`, not `22:45`). Otherwise push to the next grid
  step.

  Examples: `22:21→22:30`, `09:15:00→09:15`, `22:30:05→22:30`,
  `09:01→09:15`.

The asymmetric rule means **activity is never shorter than recorded**
— end always moves forward (or stays), start may move either way
but the rounded duration is always ≥ truncated raw duration.

**Belt-and-braces**: if `new_end ≤ new_start` after rounding, bump
`new_end = new_start + grid_minutes`. Can't happen for spans that
made it past the sub-minute filter, but defends against future edits.

**Comment emission**: `Span.original` is set to the pre-rounding
`(start, end)` iff the rounded values differ from the
**truncated-to-minute** input. Sub-minute-only differences don't
trigger a comment line.

##### 6. Cross-midnight → duration format

`Span::crosses_midnight()` is true when `end.date_naive() !=
start.date_naive()`. For those spans `export_lines` uses **decimal
hours** (taxi-compatible duration format) instead of `HH:MM-HH:MM`:

```
# original 23:50-00:10
_alias 0.5 late work
```

`format_duration_hours` trims trailing zeros: `0.25`, `0.5`, `1`,
`1.25`, `1.5`. The wall-clock `00:15` on a `23:45-00:15` line would
otherwise be read as the *source* day's midnight, which is wrong.

##### 7. Description discipline

The description column in the `.tks` is always the **session's** own
description (carried through `merge`'s " / " concatenation when
relevant). It is **never** the alias's `(Project, Subtask)` metadata
from `taxi alias list`.

The alias-metadata is still cached (`AppModel.alias_cache:
BTreeMap<String, AliasInfo>`) and used for:
- ranking autocomplete suggestions in the alias dropdown (`alias_index`
  + `score_match`),
- showing the project/subtask under the alias in the dropdown row.

But `Message::ConfirmAdd` does **not** pre-fill the new timer's
`default_description` from that cache. New timers start with empty
`default_description`; the user types real session descriptions in
edit mode (or directly on the running session, via sticky default).

##### 8. Pipeline order (per-timer, per-date)

```text
merge(sessions, merge_gap)
   ↓
split_zero_duration  →  (zeros, nonzero)
   ↓                       ↓
quantize_grid(nonzero)   aggregate_zero(&zeros)   // None if ≤3
   ↓                       ↓
export_lines(spans)      aggregate_lines(agg)
   ↘                     ↙
    body_lines for the day (via app::build_block_lines)
```

`app::build_block_lines` is the single shared helper between the
panel's auto-export and the export dialog's preview, so both paths
emit identical bytes.

##### 9. Writing the file

There are **two** writers, and which one is used depends on the
trigger:

- **`taxi::append_day(path, date, body_lines, date_format)` — used by
  the panel applet's 60 s auto-export.** Non-destructive: pre-existing
  entries under the target date header are kept; a
  `# --- appended <YYYY-MM-DD HH:MM> ---` marker is inserted before
  the new body lines so the user can see at a glance which entries
  came from a given auto-sweep. If the date isn't in the file, a
  fresh section is appended at the end (with a blank-line separator
  when the file is non-empty, no marker). Empty `body_lines` → no-op
  (file untouched, doesn't create the file either). Atomic write via
  `<file>.tks.tmp` + `rename(2)`.

- **`taxi::replace_day(path, date, body_lines, date_format)` — used
  by the manual export dialog (`export::do_export`).** Replaces the
  target date's section entirely with `date_header + body_lines`. The
  user has just edited the preview and clicked Export; what they see
  is what gets written. Other dates' content (including markers
  written by earlier auto-sweeps) is preserved bit-for-bit. Same
  atomic write protocol.

`body_lines` are **pre-rendered strings** — neither function knows
about taxi syntax, only about file-section slicing. The caller chose
the format (`HH:MM-HH:MM` or decimal-hours; comment lines or not).
This is what lets one date's section contain a mix of regular
entries, `# original …` comments, append markers, and aggregated
zero-duration duration-format lines.

The split (non-destructive auto, authoritative manual) means a noisy
auto-sweep can never silently overwrite hand edits the user made
between two ticks, while the manual flow still lets the user clean
up the preview and commit it as the day's truth.

##### 10. Per-date collation (don't clobber other timers)

Auto-export collates per-date across all timers via
`BTreeMap<NaiveDate, Vec<String>>` *before* calling `append_day`.
A single `append_day` call per date produces one marker followed by
all timers' lines for that date, instead of one marker per timer.

##### 11. Auto-export trigger

Every 60 s (`AutoExportTick`) the applet:
1. Skips entirely if the edit form is open (`self.editing.is_some()`).
   The user's in-flight edits include `original_start` / `original_end`
   snapshots; removing the underlying sessions from state while the
   form is open would resurrect them on save.
2. Walks each timer's closed sessions through the pipeline.
3. Filters out today's bucket (still in progress).
4. Collates per-date, writes each via `append_day`.
5. On success, removes the exported sessions from `state.timers` so
   they don't get re-exported later. Persists `state.json`.
6. Only sessions whose date's write **succeeded** are dropped — a
   transient I/O failure leaves the state intact for the next tick.

`!taxi.available` (uv missing) short-circuits the whole thing — the
pipeline ran but `append_day` would fail at the file resolve step
without `taxirc`. The popup keeps tracking; sessions accumulate
until uv is installed.

#### Timer-list auto-derivation

`AppModel::seed_timers_from_tks` parses the current + previous month's
`.tks`, then for each alias not in `state.suppressed_aliases` and not
already present, creates a `Timer` pre-filled with the most recent
description seen in those files. Runs on startup (after `Taxirc` loads)
and after every auto-export. Deleting a timer adds its alias to
`suppressed_aliases` so seeding doesn't bring it back.

## Build / run / test commands

```sh
just check                   # cargo clippy --workspace --all-features
just build-release           # cargo build --release (all three binaries)
just install-user            # ~/.local/{bin,share/applications,share/icons/...}
just run-gmail               # cargo run -p cosmic-applet-gmail (panel, headless)
just run-agenda              # cargo run -p cosmic-applet-google-agenda
just run-taxi                # cargo run -p cosmic-applet-taxi
cargo test --workspace       # state/sessions/taxi/gmail/agenda unit tests
```

There is **no automated UI test** — a real COSMIC session is required. After
changes to `view()`, panel layout, or popup logic, install + `pkill
cosmic-applet-…` and the panel respawns it. Then:

- Right-click → menu shows "Credentials…" (gmail/agenda) or opens the
  popup (taxi — there's no menu entry; right-click is wired to the same
  popup-toggle as left-click).
- Left-click — gmail opens mail.google.com; agenda opens Meet link of next
  event (fallback `calendar.google.com`); taxi toggles the popup.
- `pkill -USR2 cosmic-applet-…` → immediate refresh (gmail/agenda reload
  tokens + fetch; taxi reloads `state.json` + re-detects `uv`).
- `cosmic-applet-… --show-settings` from a terminal → settings window
  (useful for UI iteration without rebuilding the panel)
- agenda only: `cosmic-applet-google-agenda --debug` → dumps the raw event
  classification, no GUI
- taxi only: `cosmic-applet-taxi --show-export` → opens the per-date
  export dialog as a toplevel window

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
- Don't extract a third shared crate "just in case." `cosmic-google-common`
  exists because two applets word-for-word duplicated 250+ LOC of OAuth/
  keyring code; do the same only when a third applet starts duplicating
  something else. The taxi applet shares nothing with the Google pair, so
  it depends on neither.
- Don't add OAuth or Google API code to `cosmic-applet-taxi`; it's a
  deliberately offline-only tracker that talks to `taxi` via uv.
