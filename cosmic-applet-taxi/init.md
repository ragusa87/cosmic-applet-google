# cosmic-applet-taxi — initial implementation

This file records the full set of decisions and architecture for the
first cut of `cosmic-applet-taxi`, built end-to-end in one session. It
is intentionally written so a future reader (or AI agent) can pick up
the project without re-running the Q&A rounds that produced the design.

## Context and goal

Laurent runs COSMIC, fills Liip's Zebra timesheets via
[taxi](https://github.com/sephii/taxi), and previously used the GNOME
`tracker` extension (https://github.com/aliakseiz/tracker, source kept
at `/home/laurent/compile/tracker`) to track time. Migrating to COSMIC
broke that workflow. The applet ports tracker's panel-button UX and
adds first-class taxi integration so that **daily timesheets are
written automatically** into `~/zebra/%Y/%m.tks` instead of being
copy-pasted from durations.

The crate lives next to two pre-existing Google-backed applets
(`cosmic-applet-gmail`, `cosmic-applet-google-agenda`) inside the
`cosmic-applet-google` workspace. It does **not** depend on
`cosmic-google-common` (no OAuth involved).

## Feature scope (agreed)

- **Multiple named timers**, displayed in a popup.
- **One running timer at a time** — invariant enforced in
  `state::start_timer`, which pauses any other open session first. No
  overlapping ranges; taxi would reject them anyway.
- **Per-timer `alias`** + **per-session `description`**. A timer also
  carries `default_description` — the *sticky default* that new
  sessions snapshot on start. When the user edits a running session's
  description, the timer's default is updated too, so the next start
  pre-fills the same text. When paused, editing only changes the
  default.
- **Real start/end timestamps per session** (not just an accumulating
  duration). Pausing closes the session; resuming opens a new one. So
  the user can do the `_hello → TICKET-1 → TICKET-2 → TICKET-1` flow
  on a single timer and get three distinct taxi entries.
- **Edit form**: a per-session table where each row is fully editable
  (description / start `HH:MM` / end `HH:MM`) plus a `+ Add session`
  button. Save validates (`start < end`, only the last row may have
  no end). No "edit total duration" shortcut — sum is recomputed.
- **Tracker-style eye picker** for the panel label (per-timer or
  total).
- **Panel button content**:
  - nothing running → `[icon HH:MM]` (today's picked-subset total)
  - something running → `[icon alias: description elapsed · total]`
  - description truncated past ~40 chars with `…`
  - vertical panel anchor degrades to just `[icon HH:MM]`
- **Auto-pause on screen lock / suspend** via DBus
  (`org.freedesktop.ScreenSaver.ActiveChanged` on the session bus +
  `org.freedesktop.login1.Manager.PrepareForSleep` on the system bus).
  Auto-resume on unlock. Coalesced inside `lock::stream`.
- **Daily auto-export** at a configurable cut-over hour (default
  `04:00`): closed sessions whose work-date is in the past get merged
  (5-min gap threshold), rounded (15-min minimum), and appended to the
  resolved `.tks`. On success they're removed from local state.
- **Manual `--show-export` window**: same pipeline but for any picked
  date, with a live preview pane.
- **`--show-settings` window**: cut-over hour, merge gap, round
  minimum, `taxi_command`, `taxirc_path`, "Refresh aliases", uv
  diagnostic.
- **Alias autocomplete**, ranked: alias-prefix > alias-substring >
  description-substring. Dropdown rows show the alias bold over the
  description in caption-grey. Sources: taxirc `[<backend>_aliases]`
  sections ∪ cached `taxi alias list` ∪ aliases currently in
  `state.timers`. Top 12 returned.
- **uv gating**. Taxi is never invoked directly: every taxi call goes
  through `uv run --with taxi,taxi-zebra taxi <args>` (the command is
  configurable). If `uv` isn't on `$PATH`, all taxi features are
  disabled at runtime and the popup shows a caption explaining how to
  enable them; pure timer tracking still works.

### Explicitly out of v1 scope

- Workspace-bound / window-title-regex auto-control (needs Wayland
  protocols COSMIC doesn't reliably expose).
- CSV import/export (taxi is the only sink the user wants).
- Multiple-taxi-backend picker UI (we honour whatever taxi resolves).
- Idle-detection notifications.

## Hard invariants

1. **At most one timer has an open session at a time**
   (`state::start_timer` calls `pause_all_running` first).
2. **`Timer.alias` is unique across `state.timers`**. `add_timer`
   returns `None` if the alias is taken; `+ Add timer` selects the
   existing row instead of duplicating.

## APP_ID, paths, binary

- Binary name: `cosmic-applet-taxi`.
- `APP_ID`: `com.github.ragusa87.CosmicAppletTaxi`.
- cosmic-config (scalar settings, watched live): RON in
  `~/.config/com.github.ragusa87.CosmicAppletTaxi/v1/`.
- Persistent state (timer list + sessions): JSON at
  `~/.local/state/cosmic-applet-taxi/state.json`. Atomic write via
  `state.json.tmp` + `rename(2)`.
- Alias cache (output of `taxi alias list`): same dir,
  `aliases.json` (not yet persisted in v1 — held in memory, refreshed
  on demand and at startup if available).

## Data model

```rust
// state.rs
struct Session {
    start: DateTime<Local>,
    end:   Option<DateTime<Local>>,  // None = currently running
    description: String,             // snapshot at start; freely editable
}

struct Timer {
    id: Uuid,
    alias: String,                   // unique within state.timers
    default_description: String,     // sticky default
    selected: bool,                  // panel-label eye picker
    auto_resume: bool,               // set on lock, cleared on resume
    sessions: Vec<Session>,
}

struct AppState {
    timers: Vec<Timer>,
    suppressed_aliases: Vec<String>, // aliases the user deleted
    total_selected: bool,            // panel-label "Total" toggle
    schema_version: u32,
}
```

```rust
// config.rs (cosmic-config schema, version = 1)
struct Config {
    cutover_hour: u8,            // default 4, clamped to 0..=23
    merge_gap_minutes: u32,      // default 5
    round_min_minutes: u32,      // default 15
    taxi_command: String,        // default "uv run --with taxi,taxi-zebra taxi"
    taxirc_path: String,         // "" → resolve ~/.config/taxi/taxirc
}
```

```rust
// taxi.rs
struct AliasInfo {
    mapping: String,         // e.g. "8288/34666"
    description: String,     // taxi's "(Project, Subtask)" — empty if absent
}
```

`AppModel.alias_cache: BTreeMap<String, AliasInfo>` powers the
dropdown.

## File layout

```
cosmic-applet-taxi/
├── Cargo.toml                # no cosmic-google-common dep
├── data/
│   ├── com.github.ragusa87.CosmicAppletTaxi.desktop
│   └── icons/com.github.ragusa87.CosmicAppletTaxi.svg
├── init.md                   # this document
└── src/
    ├── main.rs               # argv dispatch: applet | --show-settings | --show-export
    ├── app.rs                # panel applet — Application impl, message dispatch,
    │                         # 1s/60s ticks, sigusr2, DBus lock subscription,
    │                         # auto-export pipeline, .tks-derived seeding
    ├── settings.rs           # standalone settings toplevel (cosmic::app::run)
    ├── export.rs             # standalone export dialog toplevel
    ├── ui.rs                 # popup_view / timer_row / edit_row / footer_row /
    │                         # suggestion_row / total_row / add_row
    ├── config.rs             # cosmic-config schema + APP_ID + Config helpers
    ├── state.rs              # Timer/Session/AppState + JSON persistence +
    │                         # mutation helpers + cutover_date / sum_panel etc.
    ├── sessions.rs           # pure: group_by_date / merge (+ description dedup) /
    │                         # round_up / export_lines (+ unit tests)
    ├── taxi.rs               # taxirc INI parser (configparser) walking every
    │                         # [<backend>_aliases] section; .tks line-iterator
    │                         # parser + append_day writer; TaxiRunner (uv subprocess
    │                         # wrapper); parse_alias_list with description capture
    └── lock.rs               # zbus 5 — session bus ScreenSaver.ActiveChanged
                              # + system bus login1.Manager.PrepareForSleep,
                              # coalesced into LockEvent::{Locked, Unlocked}
```

## Pipelines

### Daily auto-export (and `--show-export`)

Triggered by `AutoExportTick` (60 s subscription) and on demand in the
export dialog. Steps:

1. `cutover_date(now, cutover_hour)` — anything before the cut-over
   hour is counted as the previous date.
2. For each `Timer`, partition closed sessions by their cut-over-
   shifted date (`sessions::group_by_date`).
3. Drop today's bucket (auto path) or pick a specific date (manual
   path).
4. `sessions::merge(_, merge_gap)` — sort by start, collapse adjacent
   sessions whose gap is below the threshold. Descriptions are joined
   with `" / "` when they differ (deduped, chronological).
5. `sessions::round_up(_, round_min)` — extend short spans to the
   minimum duration.
6. Write each date to `taxi::resolve_tks_path(template, date)` via
   `taxi::append_day` (inserts under existing date header, else
   appends a fresh section).
7. On success, drop the exported sessions from `state.timers`.

### Timer-list auto-derivation

`AppModel::seed_timers_from_tks` runs on startup (after the taxirc
loads) and on every `AutoExportTick`. It parses the current + previous
month's `.tks`, then for each distinct alias that:

- is **not** in `state.suppressed_aliases`, and
- is **not** already a `Timer`,

it appends a fresh `Timer` with `default_description` set to the most
recent description seen for that alias.

Deleting a timer in the popup adds its alias to
`suppressed_aliases` so the next seeding pass doesn't revive it.

### Screen lock / suspend

`lock::stream` connects to two DBus signals; failures are logged and
ignored individually. On the merged stream:

- `Locked` → `state.auto_pause_all(now)` — close any open session,
  set `auto_resume = true`.
- `Unlocked` → `state.auto_resume_one(now)` — restart the one timer
  flagged `auto_resume`, push a new session.

### Alias autocomplete + sticky default

`app::alias_index` builds a `BTreeMap<alias, description>` by merging
taxirc keys (no description), `alias_cache` entries (with
description), and `state.timers` aliases (no description). When the
same alias appears in several sources, the longest non-empty
description wins.

`app::alias_suggestions(query)` ranks each entry via `score_match`:

| match                        | score |
|------------------------------|-------|
| alias == query               | 100   |
| alias starts with query      |  80   |
| alias contains query         |  60   |
| description contains query   |  30   |

Sorted by score desc, then alphabetically; top 12 returned.

When the user picks an alias from the `+ Add timer` dropdown,
`app::description_for(alias)` is consulted and used as the new
timer's `default_description`, so the first session inherits the
project/subtask text without manual typing.

## Modes (argv dispatch)

```
cosmic-applet-taxi                  # panel applet (cosmic::applet::run)
cosmic-applet-taxi --show-settings  # settings window (cosmic::app::run)
cosmic-applet-taxi --show-export    # export dialog window (cosmic::app::run)
```

Settings and export windows install `SIG_IGN` on SIGUSR2 at startup so
that `pkill -USR2 -f cosmic-applet-taxi` only refreshes the panel
applet (the settings/export windows share the binary name).

After settings / export windows write changes, they send
`pkill -USR2 -f cosmic-applet-taxi` to make the panel applet reload
`state.json` and re-detect uv.

## Workspace integration

- Added to `[workspace] members` and the package list of the root
  `Cargo.toml`.
- New workspace dependencies: `configparser`, `dirs`, `regex`,
  `shell-words`, `uuid`, `zbus`.
- `justfile` gets `taxi-name` / `taxi-appid` variables; the
  `install` / `install-user` / `uninstall` / `uninstall-user`
  composite recipes append a third sub-call; `run-taxi *args` recipe
  added; `refresh` extends to `pkill -USR2 -f cosmic-applet-taxi`.
- `AGENTS.md` (workspace root) appended a per-applet section.
- `README.md` (workspace root) appended a "Taxi tracker applet"
  section + updated the applet table and install paths.

## Build, install, run

```sh
just build-release            # builds all three applets
just install-user             # installs into ~/.local/{bin,share/...}
pkill cosmic-applet-taxi      # panel respawns the binary

# directly, without panel:
cosmic-applet-taxi --show-settings
cosmic-applet-taxi --show-export
```

To refresh the running applet (reload state.json, re-detect uv):

```sh
just refresh                  # signals all three applets at once
# or
pkill -USR2 -f cosmic-applet-taxi
```

## Tests

`cargo test -p cosmic-applet-taxi` runs 28 unit tests:

- `state` — invariants (single-running, alias uniqueness, sticky
  default mirror), suppressed-aliases lifecycle, screen-lock cycle,
  cut-over date shifts.
- `sessions` — merge (empty / short gap / long gap / open sessions
  dropped / descriptions joined), round-up (short → extended, long →
  unchanged), `export_lines` (with and without description),
  `group_by_date` across the cut-over boundary.
- `taxi` — taxirc parser (all `[<backend>_aliases]` sections),
  `resolve_tks_path` (chrono format + `~` expansion), `parse_tks`
  (user's `0800-0900` format, `09:00-10:00` format, `?` placeholders,
  comments, multi-day), `append_day` (new file vs. inserting into an
  existing date section), `parse_alias_list` (both the
  `[backend] alias -> mapping (description)` form and the
  `alias = mapping` / `->` / `:` fallbacks). Anonymised fixtures.

`just check` is clippy-pedantic-clean for this crate. (Five pre-
existing warnings in `cosmic-google-common` are unrelated and predate
this work.)

## Things to verify manually (no automated UI tests)

1. Add two timers, set their aliases, start one, then start the other
   → only one runs at a time; the other has been auto-paused with a
   closed session.
2. Click `+ Add timer`, type a few characters → dropdown shows
   matches ranked by alias-prefix > alias-substring >
   description-substring. Picking an alias with a cached description
   pre-fills the new timer's display text.
3. Lock screen (`loginctl lock-session`) → running timer pauses;
   unlock → it resumes automatically.
4. `pkill -USR2 -f cosmic-applet-taxi` → state reload from disk +
   re-detect uv (the open settings/export windows are unaffected).
5. With `uv` uninstalled or `taxi_command` set to a bogus value → the
   popup shows the "Install `uv` …" caption and the Export button is
   inert; timer functions still work.
6. With `uv` installed → Settings → "Refresh aliases" populates the
   cache (count shown beside the button). Typing in the alias field
   surfaces matches by description too (e.g. typing "office" finds
   aliases whose `(Project, Subtask)` mentions Office).
7. Set `cutover_hour` to one minute from now. Wait for the boundary
   to pass and the next 60 s tick to fire → yesterday's sessions
   disappear from the popup and a new entry shows up at
   `~/zebra/%Y/%m.tks` for the correct day.
8. Open `--show-export`, type an arbitrary date, confirm the preview
   matches what you expect, click "Append to .tks" → the file gets a
   new section (or new lines under an existing one); the popup's
   timers lose those exported sessions.

## Notes for future iterations

- The five clippy warnings in `cosmic-google-common` are pre-existing
  (missing `# Errors` doc-sections, one `#[must_use]` candidate). Out
  of scope here.
- The `taxi.rs` module deliberately doesn't auto-import sessions back
  *from* `.tks` into the running state — `seed_timers_from_tks` only
  uses `.tks` to seed alias rows + default descriptions, not to
  re-create sessions. Bidirectional sync would invite duplicate
  exports.
- The export window's date picker is three numeric `text_input`s; if
  cosmic-iced ships a real date-picker widget later, swap it in.
- `AliasInfo.description` is captured but the dropdown only displays
  it. Storing it in `state.json` (as a frozen snapshot of the
  description at timer-creation time) could survive a missing taxi /
  uv, but isn't necessary today since `alias_cache` is rebuilt at
  startup whenever uv is available.
- `lock.rs` uses freedesktop interfaces. COSMIC's own session manager
  may expose richer signals in the future; if so, prefer those
  (`org.cosmic.*`) for tighter integration.

---

# Changelog

## v1.1 — export pipeline refinements

Iteration after first install surfaced gaps in the export side. v1.1
reshapes the rounding, file-write semantics, and the export dialog
UX. Old call sites (`round_up`, `append_day`) are removed.

### Rounding: asymmetric grid

`sessions::round_up` (extend short spans to `min` minutes) is replaced
by `sessions::quantize_grid(spans, grid_minutes)`:

- **Start** rounds to the **nearest grid step** with a DOWN-biased
  threshold `ceil(grid/2)`. For `grid=15` → threshold 8:
  `offset ≤ 8 → DOWN`, `offset > 8 → UP`.
  Examples: `09:03→09:00`, `09:07→09:00`, `09:08→09:00`, `09:09→09:15`,
  `09:14→09:15`, `09:31→09:30`, `09:38→09:30`, `09:39→09:45`.
- **End** rounds **UP** (ceil) to the next grid step. Sub-minute
  precision is dropped: `22:30:05 → 22:30` (already on grid), not
  `22:45`. Only when the truncated minute is off-grid do we push to
  the next step. Preserves "activity is never shorter than recorded".

`Span` gains an `original: Option<(DateTime<Local>, DateTime<Local>)>`
field. Set iff the rounded values differ from the truncated-to-minute
inputs (so a sub-minute-only change doesn't spuriously emit a comment
line).

**Belt-and-braces**: when `new_end ≤ new_start` (a span collapsed by
rounding), bump `new_end = new_start + grid_minutes` so every emitted
span has at least one grid unit of duration.

### Sub-minute / zero-duration filter

`sessions::split_zero_duration` partitions spans by **`duration < 1
minute`** (was `start == end` in v1.0). Caught case from the field:
two sessions like `23:10:05–23:10:42` and `23:25:11–23:25:55` had
non-zero seconds but were essentially "clicked start then pause
without working" — they now fall in the zero bucket. Without this,
the belt-and-braces inflated each into a `23:15-23:30` line.

Per-(timer, date):
- **≤ 3** sub-minute spans → **dropped silently**.
- **> 3** → **aggregated** into one duration-format line via
  `sessions::aggregate_zero` + `sessions::aggregate_lines`. Output:
  ```
  # 5 zero-duration sessions consolidated into 15 min
  _alias 0.25 deduped / descriptions
  ```

### Export line format

`sessions::export_lines` now emits two-line blocks when the span was
rounded:

```
# original 22:18-22:30
_hello 22:15-22:30 actual description
```

When the span was on-grid both ways, only the entry line is emitted.

**Cross-midnight detection** (`Span::crosses_midnight`): when the
rounded `end` falls on a different calendar day than `start`, the
entry-line switches to **duration format** (decimal hours) instead of
`HH:MM-HH:MM`. The wall-clock `00:15` for an end that's "next day"
would otherwise read as midnight in the source day, which is wrong.
`format_duration_hours` trims trailing zeros: `1`, `1.5`, `0.25`,
`0.75`.

The session **description** is always what the user typed (the
`Session.description` field). It is never the alias's
`(Project, Subtask)` metadata from `taxi alias list` — that data
isn't pre-filled into `Timer.default_description` any more (see
"`ConfirmAdd` no longer pre-fills" below).

### File-write semantics: `replace_day`, not `append_day`

`taxi::append_day` is gone. `taxi::replace_day(path, date, body_lines,
date_format)` replaces the target date's section entirely. Body lines
are pre-rendered by the caller (so the function doesn't have to know
about duration format, aggregate blocks, comment lines, etc.). Other
dates' content — including blank lines, comments, and hand-edited
entries — is preserved verbatim.

The export is now the **source of truth** for the day. A re-export
overwrites whatever was there; no silent stale-line accumulation.

### Per-date collation in auto-export

`auto_export_past_days` no longer calls `replace_day` once per
(timer, date). Two timers exporting to the same day would have
clobbered each other. Lines are now collated per-date across all
timers via a `BTreeMap<NaiveDate, Vec<String>>`, then a single
`replace_day` per date.

### Export dialog (`export.rs`)

Major rebuild:

- **Editable preview**. Replaced the read-only `text::monotext` with
  a multi-line `cosmic::widget::text_editor` whose state lives in
  `ExportApp.preview_content: text_editor::Content`. The user can fix
  typos / drop lines / add ad-hoc comments before clicking any of the
  three primary actions. The editor regenerates on `DateInput` /
  `Today` / `ResetPreview`; preview edits are discarded when the date
  changes.
- **Three actions**:
  - **Export** (primary, suggested style): writes preview text via
    `replace_day` and drops the matching sessions from local state.
  - **Push** (next to Export): Export + `taxi ci` via the uv-gated
    `TaxiRunner`. Disabled if uv isn't available.
  - **Copy to clipboard**: fire-and-forget
    `cosmic::iced::clipboard::write` of preview text. Doesn't touch
    `.tks`.
- **Collapsible "▶ Show current file content for this day"** toggle
  above the preview editor. Reads the existing date-section from
  disk synchronously when expanded; "(no existing section)" if the
  date isn't present.
- **Date-row alignment** fixed via `Row::align_y(Alignment::End)` so
  the Today button bottom-aligns with the date input (instead of
  floating above it because the input is taller).
- Path text "Append to: …" → "Will write to: …".
- Window size 720×640 (was 640×520) to host the new widgets.
- Whole form `scrollable` so small displays don't clip.

### App-side changes

- `Message::ConfirmAdd` no longer copies the alias's project/subtask
  metadata into `Timer.default_description`. New timers start with
  empty `default_description` so exported lines always show the user's
  typed text, never alias metadata. The autocomplete dropdown still
  surfaces the alias's metadata in caption-grey for picking.
- `Message::StartEdit(id)` pauses the timer first (with `persist`)
  before entering edit mode, so the edit table's session end-times
  are stable rather than growing as the user types.

### New `app::build_block_lines` helper

Shared between the panel's auto-export pipeline and the export
dialog's preview. Takes quantized spans + optional `ZeroAggregate` +
alias + `grid_minutes`, returns the body lines for one timer on one
date.

### Test coverage

47 unit tests in the taxi crate after v1.1. New tests cover:
- `round_start` examples on the threshold boundary (offset 7/8/9)
- `round_end` ceiling + sub-minute drop (`22:30:05 → 22:30`)
- `quantize_grid` belt-and-braces, records-original-when-moved,
  no-comment-when-only-sub-minute-changed, the user's reported
  `22:18-22:30` bug
- `split_zero_duration` drops sub-minute sessions; keeps full-minute
- `aggregate_zero` threshold + dedup
- `format_duration_hours` examples (`0.25`/`0.5`/`1`/`1.5`/etc.)
- `export_lines` comment + duration format for cross-midnight
- `aggregate_lines` format incl. singular/plural
- `replace_day` overwrites existing, appends when absent, preserves
  other days' middle-of-file content, preserves comment lines

## v1.2 — non-destructive auto-export + edit-form lockout

Iteration after v1.1 surfaced two symmetric problems on the auto-side:

1. The 60 s auto-sweep was happy to rewrite a `.tks` section the user
   had hand-tweaked between sweeps, because `replace_day` is
   authoritative by design.
2. Opening the edit form while a tick was pending could drop sessions
   from `state.timers` under the user's feet; saving the form then
   resurrected them (the `EditSession` rows carry `original_start` /
   `original_end` snapshots, so `commit_edit` happily rebuilds the
   pre-export vec).

### `taxi::append_day` revived (and `replace_day` kept)

`taxi::append_day(path, date, body_lines, date_format)` is back as a
non-destructive sibling of `replace_day`:

- Empty `body_lines` → no-op (file untouched, file not created).
- Date section absent → identical to the `replace_day` "append fresh
  section at EOF" branch (no marker).
- Date section present → insert a `# --- appended <YYYY-MM-DD HH:MM> ---`
  marker at the end of the existing section, then the new body lines,
  preserving everything that was there before.

`auto_export_past_days` calls `append_day` (no more silent overwrites).
`export::do_export` still calls `replace_day` (the manual flow is
authoritative — what the user typed is what gets written).

The split is intentional: the two writers exist at the same time to
keep the noisy auto-path conservative while keeping the curated manual
path a single-step source-of-truth commit.

### Edit-form lockout for auto-export

`AppModel::auto_export_past_days` short-circuits when `self.editing.is_some()`:

```rust
if self.editing.is_some() {
    return;
}
```

A `Message::AutoExportTick` fired while the edit form is open turns
into a no-op for the export step (the timer-seed step still runs).
Closing the form (Save / Cancel / popup-closed) clears `self.editing`
and the next tick proceeds normally. Covers the v1.1 race where a
mid-edit tick could remove the very sessions the user was editing.

### Test coverage

54 unit tests after v1.2. New tests cover:
- `append_day` four shapes: new file (no marker), date absent
  (no marker, fresh section), date present (marker + body added under
  existing entries), empty body (no-op, doesn't create file).
- `append_day` preserves other days' content when the middle section
  is appended to (marker sits between prior and new entries within
  the date's section).
