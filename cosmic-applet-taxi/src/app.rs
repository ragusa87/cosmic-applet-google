use std::collections::BTreeMap;

use chrono::{Datelike, Duration, Local, NaiveTime, Timelike};
use cosmic::Element;
use cosmic::app::Task;
use cosmic::iced::widget::mouse_area;
use cosmic::iced::{Limits, Subscription, window::Id};
use cosmic::surface::{self, action::destroy_popup};
use cosmic::widget::button;
use cosmic_config::CosmicConfigEntry;
use futures_util::SinkExt;
use tokio::signal::unix::{SignalKind, signal};
use uuid::Uuid;

use crate::config::{APP_ID, Config};
use crate::lock::{self, LockEvent};
use crate::state::{self, AppState, Timer};
use crate::taxi::{self, AliasInfo, TaxiRunner, Taxirc};
use crate::ui;

const TAXI_ICON_SVG: &[u8] =
    include_bytes!("../data/icons/com.github.ragusa87.CosmicAppletTaxi.svg");

#[derive(Default)]
pub struct EditBuf {
    pub alias: String,
    pub sessions: Vec<EditSession>,
    pub error: Option<String>,
    /// Index of a session that has been clicked once on its delete button.
    /// A second click on the same index actually removes it. Any other edit
    /// interaction disarms this.
    pub pending_delete: Option<usize>,
    /// `true` after the user clicked a suggestion in the alias dropdown.
    /// Causes the dropdown to stay hidden until the user types in the alias
    /// field again.
    pub alias_picked: bool,
    /// First click on "Delete timer" arms this; second click commits the
    /// deletion. Any other interaction disarms.
    pub pending_delete_timer: bool,
}

/// Per-session row in the edit form. `description` is held as a
/// `text_editor::Content` (not a plain `String`) so the field gets the
/// multi-line widget's native keyboard handling (selection, copy/paste,
/// Home/End, etc.). `Content` is not `Clone`, so neither is `EditSession`
/// — it isn't cloned anywhere in the pipeline today.
#[derive(Debug)]
pub struct EditSession {
    pub description: cosmic::widget::text_editor::Content,
    pub start: String,
    pub end: String,
    /// Calendar date the session started on. Preserved across edit so that
    /// saving a session that originally happened on a previous day doesn't
    /// silently move it onto today.
    pub date: chrono::NaiveDate,
    /// Original timestamps the row was loaded with — used on save to keep
    /// sub-minute precision when the user did not change the `HH:MM` field.
    pub original_start: chrono::DateTime<Local>,
    pub original_end: Option<chrono::DateTime<Local>>,
}

#[derive(Default)]
pub struct AddBuf {
    pub alias: String,
    pub active: bool,
    /// Same role as `EditBuf::alias_picked` — dismisses the suggestions
    /// dropdown once the user has clicked a row.
    pub alias_picked: bool,
}

#[derive(Debug, Clone)]
pub struct AliasSuggestion {
    pub alias: String,
    pub description: String,
}

/// Rank an alias / description against a lowercase query string.
/// Returns 0 when nothing matches.
fn score_match(alias: &str, description: &str, q_lower: &str) -> i32 {
    if q_lower.is_empty() {
        return 1;
    }
    let alias_l = alias.to_lowercase();
    if alias_l == q_lower {
        return 100;
    }
    if alias_l.starts_with(q_lower) {
        return 80;
    }
    if alias_l.contains(q_lower) {
        return 60;
    }
    if description.to_lowercase().contains(q_lower) {
        return 30;
    }
    0
}

pub struct AppModel {
    pub core: cosmic::Core,
    pub config: Config,
    pub state: AppState,
    pub taxi: TaxiRunner,
    pub taxirc: Option<Taxirc>,
    pub alias_cache: BTreeMap<String, AliasInfo>,
    pub menu_popup: Option<Id>,
    pub editing: Option<Uuid>,
    pub edit_buf: EditBuf,
    pub add_buf: AddBuf,
    pub status: Option<String>,
}

impl Default for AppModel {
    fn default() -> Self {
        Self {
            core: cosmic::Core::default(),
            config: Config::default(),
            state: AppState::default(),
            taxi: TaxiRunner {
                argv: Vec::new(),
                available: false,
            },
            taxirc: None,
            alias_cache: BTreeMap::new(),
            menu_popup: None,
            editing: None,
            edit_buf: EditBuf::default(),
            add_buf: AddBuf::default(),
            status: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    NoOp,
    LeftClick,
    OpenMenu,
    PopupClosed(Id),

    Tick,
    AutoExportTick,
    LockEvt(LockEventDup),
    ForceRefresh,

    StartPause(Uuid),
    Pause,
    Reset(Uuid),

    BeginAdd,
    CancelAdd,
    AddBufAlias(String),
    AddBufAliasPick(String),
    ConfirmAdd,

    StartEdit(Uuid),
    EditAlias(String),
    EditAliasPick(String),
    EditDeleteTimer,
    EditSessionDesc(usize, cosmic::widget::text_editor::Action),
    EditSessionStart(usize, String),
    EditSessionEnd(usize, String),
    EditAddSession,
    EditDeleteSession(usize),
    SaveEdit,
    CancelEdit,

    OpenSettings,
    OpenExport,
    RefreshAliases,
    AliasesLoaded(BTreeMap<String, AliasInfo>),

    Taxirc(Option<Taxirc>),
    TaxiReady(TaxiRunner),
    UpdateConfig(Config),
}

/// Newtype wrapper because `LockEvent` is not `Hash` and Message must be `Clone`.
#[derive(Debug, Clone, Copy)]
pub enum LockEventDup {
    Locked,
    Unlocked,
}

impl From<LockEvent> for LockEventDup {
    fn from(value: LockEvent) -> Self {
        match value {
            LockEvent::Locked => Self::Locked,
            LockEvent::Unlocked => Self::Unlocked,
        }
    }
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(core: cosmic::Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let config = cosmic_config::Config::new(Self::APP_ID, Config::VERSION)
            .map(|ctx| match Config::get_entry(&ctx) {
                Ok(c) => c,
                Err((_errors, c)) => c,
            })
            .unwrap_or_default();

        let state = AppState::load().unwrap_or_else(|e| {
            tracing::warn!(error = %e, "loading state failed; starting fresh");
            AppState {
                total_selected: true,
                ..AppState::default()
            }
        });

        let app = AppModel {
            core,
            config: config.clone(),
            state,
            ..Default::default()
        };

        let detect_task = cosmic::task::future(async move {
            let runner = TaxiRunner::detect(&config).await;
            Message::TaxiReady(runner)
        });

        (app, detect_task)
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn view(&self) -> Element<'_, Self::Message> {
        use cosmic::applet::cosmic_panel_config::PanelAnchor;
        use cosmic::iced::{Alignment, Length};
        use cosmic::widget::{Row, text};

        let is_horizontal = matches!(
            self.core.applet.anchor,
            PanelAnchor::Top | PanelAnchor::Bottom
        );

        let (icon_size, _) = self.core.applet.suggested_size(true);
        let (pad_major, pad_minor) = self.core.applet.suggested_padding(true);

        let icon = cosmic::widget::icon(cosmic::widget::icon::from_svg_bytes(
            TAXI_ICON_SVG.to_vec(),
        ))
        .size(icon_size);

        let label = self.panel_label_text(is_horizontal);

        let content: Element<'_, Self::Message> = if let Some(text_s) = label {
            Row::new()
                .align_y(Alignment::Center)
                .spacing(4)
                .push(icon)
                .push(text::body(text_s))
                .into()
        } else {
            Element::from(icon)
        };

        let (horizontal_padding, vertical_padding) = if is_horizontal {
            (pad_major, pad_minor)
        } else {
            (pad_minor, pad_major)
        };

        let btn = button::custom(content)
            .padding([vertical_padding, horizontal_padding])
            .on_press(Message::LeftClick)
            .class(cosmic::theme::Button::AppletIcon)
            .height(Length::Shrink);

        let interactive = mouse_area(btn).on_right_press(Message::OpenMenu);
        self.core.applet.autosize_window(interactive).into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        cosmic::widget::container(cosmic::widget::text("")).into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let one_sec = cosmic::iced::time::every(std::time::Duration::from_secs(1))
            .map(|_| Message::Tick);
        let minute = cosmic::iced::time::every(std::time::Duration::from_mins(1))
            .map(|_| Message::AutoExportTick);
        let watch = self
            .core()
            .watch_config::<Config>(Self::APP_ID)
            .map(|update| Message::UpdateConfig(update.config));
        Subscription::batch([
            one_sec,
            minute,
            watch,
            sigusr2_subscription(),
            lock_subscription(),
        ])
    }

    #[allow(clippy::too_many_lines)]
    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::NoOp | Message::Tick => {}

            Message::LeftClick => {
                if let Some(id) = self.menu_popup.take() {
                    return dispatch_surface(destroy_popup(id));
                }
                let new_id = Id::unique();
                self.menu_popup = Some(new_id);
                return open_popup(new_id);
            }

            Message::OpenMenu => {
                return self.update(Message::LeftClick);
            }

            Message::PopupClosed(id) => {
                if self.menu_popup.as_ref() == Some(&id) {
                    self.menu_popup = None;
                    self.editing = None;
                    self.add_buf = AddBuf::default();
                }
            }

            Message::AutoExportTick => {
                self.auto_export_past_days();
                self.seed_timers_from_tks();
                self.persist();
            }

            Message::LockEvt(LockEventDup::Locked) => {
                self.state.auto_pause_all(Local::now());
                self.persist();
            }
            Message::LockEvt(LockEventDup::Unlocked) => {
                self.state.auto_resume_one(Local::now());
                self.persist();
            }

            Message::ForceRefresh => {
                if let Ok(s) = AppState::load() {
                    self.state = s;
                }
                let config = self.config.clone();
                return cosmic::task::future(async move {
                    Message::TaxiReady(TaxiRunner::detect(&config).await)
                });
            }

            Message::StartPause(id) => {
                let now = Local::now();
                let running = self
                    .state
                    .find_timer(id)
                    .is_some_and(Timer::is_running);
                if running {
                    self.state.pause_timer(id, now);
                } else {
                    self.state.start_timer(id, now);
                }
                self.persist();
            }

            Message::Pause => {
                self.state.pause_all_running(Local::now());
                self.persist();
            }

            Message::Reset(id) => {
                self.state.reset_timer(id);
                self.persist();
            }

            Message::BeginAdd => {
                self.add_buf.active = true;
                self.add_buf.alias.clear();
            }

            Message::CancelAdd => {
                self.add_buf = AddBuf::default();
            }

            Message::AddBufAlias(s) => {
                self.add_buf.alias = s;
                self.add_buf.alias_picked = false;
            }
            Message::AddBufAliasPick(s) => {
                self.add_buf.alias = s;
                self.add_buf.alias_picked = true;
            }

            Message::ConfirmAdd => {
                let alias = self.add_buf.alias.trim().to_owned();
                if alias.is_empty() {
                    return Task::none();
                }
                // Don't pre-fill the timer's default description from the
                // alias's project/subtask metadata. Sessions inherit the
                // (empty) default; the user fills in their actual work
                // description per-session, so the exported description
                // column is what they typed — not taxi alias metadata.
                self.state.add_timer(alias, String::new());
                self.add_buf = AddBuf::default();
                self.persist();
            }

            Message::StartEdit(id) => {
                // Pause the timer first if it's running, so the edit
                // table shows stable times instead of a row whose end
                // keeps growing while the user types.
                if self
                    .state
                    .find_timer(id)
                    .is_some_and(crate::state::Timer::is_running)
                {
                    self.state.pause_timer(id, Local::now());
                    self.persist();
                }
                self.editing = Some(id);
                self.edit_buf = build_edit_buf(self.state.find_timer(id));
            }

            Message::EditAlias(s) => {
                self.edit_buf.alias = s;
                self.edit_buf.alias_picked = false;
                self.edit_buf.pending_delete = None;
                self.edit_buf.pending_delete_timer = false;
            }
            Message::EditAliasPick(s) => {
                self.edit_buf.alias = s;
                self.edit_buf.alias_picked = true;
                self.edit_buf.pending_delete = None;
                self.edit_buf.pending_delete_timer = false;
            }
            Message::EditSessionDesc(i, action) => {
                if let Some(row) = self.edit_buf.sessions.get_mut(i) {
                    row.description.perform(action);
                }
                self.edit_buf.pending_delete = None;
                self.edit_buf.pending_delete_timer = false;
            }
            Message::EditSessionStart(i, s) => {
                if let Some(row) = self.edit_buf.sessions.get_mut(i) {
                    row.start = s;
                }
                self.edit_buf.pending_delete = None;
                self.edit_buf.pending_delete_timer = false;
            }
            Message::EditSessionEnd(i, s) => {
                if let Some(row) = self.edit_buf.sessions.get_mut(i) {
                    row.end = s;
                }
                self.edit_buf.pending_delete = None;
                self.edit_buf.pending_delete_timer = false;
            }
            Message::EditAddSession => {
                let now = Local::now();
                let stamp = now.format("%H:%M").to_string();
                self.edit_buf.sessions.push(EditSession {
                    description: cosmic::widget::text_editor::Content::new(),
                    start: stamp.clone(),
                    end: stamp,
                    date: now.date_naive(),
                    original_start: now,
                    original_end: Some(now),
                });
                self.edit_buf.pending_delete = None;
                self.edit_buf.pending_delete_timer = false;
            }
            Message::EditDeleteSession(i) => {
                self.edit_buf.pending_delete_timer = false;
                if i >= self.edit_buf.sessions.len() {
                    self.edit_buf.pending_delete = None;
                } else if self.edit_buf.pending_delete == Some(i) {
                    self.edit_buf.sessions.remove(i);
                    self.edit_buf.pending_delete = None;
                } else {
                    self.edit_buf.pending_delete = Some(i);
                }
            }

            Message::EditDeleteTimer => {
                self.edit_buf.pending_delete = None;
                if self.edit_buf.pending_delete_timer {
                    if let Some(id) = self.editing.take() {
                        self.state.remove_timer(id);
                        self.edit_buf = EditBuf::default();
                        self.persist();
                    }
                } else {
                    self.edit_buf.pending_delete_timer = true;
                }
            }

            Message::CancelEdit => {
                self.editing = None;
                self.edit_buf = EditBuf::default();
            }

            Message::SaveEdit => {
                let Some(id) = self.editing else {
                    return Task::none();
                };
                if let Err(e) = self.commit_edit(id) {
                    self.edit_buf.error = Some(e.to_string());
                    return Task::none();
                }
                self.editing = None;
                self.edit_buf = EditBuf::default();
                self.persist();
            }

            Message::OpenSettings => {
                let destroy_popup = self
                    .menu_popup
                    .take()
                    .map_or_else(Task::none, |id| dispatch_surface(destroy_popup(id)));
                let launch = spawn_subwindow("--show-settings");
                return Task::batch([destroy_popup, launch]);
            }
            Message::OpenExport => {
                let destroy_popup = self
                    .menu_popup
                    .take()
                    .map_or_else(Task::none, |id| dispatch_surface(destroy_popup(id)));
                let launch = spawn_subwindow("--show-export");
                return Task::batch([destroy_popup, launch]);
            }

            Message::RefreshAliases => {
                if !self.taxi.available {
                    return Task::none();
                }
                let runner = self.taxi.clone();
                return cosmic::task::future(async move {
                    if let Err(e) = runner.update().await {
                        tracing::warn!(error = %e, "taxi update failed");
                    }
                    let aliases = runner.alias_list().await.unwrap_or_default();
                    Message::AliasesLoaded(aliases)
                });
            }

            Message::AliasesLoaded(map) => {
                self.alias_cache = map;
            }

            Message::Taxirc(rc) => {
                self.taxirc = rc;
                self.seed_timers_from_tks();
                self.persist();
            }

            Message::TaxiReady(runner) => {
                self.taxi = runner;
                let path = Taxirc::resolve_path(&self.config);
                let fetch_taxirc = path.map_or_else(Task::none, |p| {
                    cosmic::task::future(async move {
                        Message::Taxirc(taxi::load_taxirc(&p).ok())
                    })
                });
                let fetch_aliases = if self.taxi.available {
                    let runner = self.taxi.clone();
                    cosmic::task::future(async move {
                        let aliases = runner.alias_list().await.unwrap_or_default();
                        Message::AliasesLoaded(aliases)
                    })
                } else {
                    Task::none()
                };
                return Task::batch([fetch_taxirc, fetch_aliases]);
            }

            Message::UpdateConfig(c) => {
                self.config = c;
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}

impl AppModel {
    /// Text shown next to the panel icon. Minimised to keep the panel from
    /// reflowing every tick on big screens — only the running timer's alias
    /// and today's accumulated duration (`HH:MM`, so the width changes at
    /// most once a minute). Idle / vertical panels show the icon alone.
    fn panel_label_text(&self, is_horizontal: bool) -> Option<String> {
        if !is_horizontal {
            return None;
        }
        let t = self.state.running_timer()?;
        let now = Local::now();
        let day = state::cutover_date(now, self.config.cutover_hour());
        let elapsed = state::sum_for_date(t, day, self.config.cutover_hour(), now);
        Some(format!(
            "{} {}",
            t.alias,
            fmt_duration_hms_short(elapsed)
        ))
    }

    fn persist(&self) {
        if let Err(e) = self.state.save() {
            tracing::warn!(error = %e, "saving state failed");
        }
    }

    /// Aggregate all known aliases (taxirc + CLI cache + current timers) into a
    /// single map `alias -> description` (description may be empty). When the
    /// same alias appears in multiple sources, the longest non-empty
    /// description wins.
    pub fn alias_index(&self) -> BTreeMap<String, String> {
        let mut out: BTreeMap<String, String> = BTreeMap::new();
        if let Some(rc) = &self.taxirc {
            for k in rc.aliases.keys() {
                out.entry(k.clone()).or_default();
            }
        }
        for (alias, info) in &self.alias_cache {
            out.entry(alias.clone())
                .and_modify(|d| {
                    if d.len() < info.description.len() {
                        info.description.clone_into(d);
                    }
                })
                .or_insert_with(|| info.description.clone());
        }
        for t in &self.state.timers {
            out.entry(t.alias.clone()).or_default();
        }
        out
    }

    /// Autocomplete suggestions ranked by best match against alias name first,
    /// then against the description. Caller can render `description` as a
    /// secondary line in the dropdown.
    pub fn alias_suggestions(&self, query: &str) -> Vec<AliasSuggestion> {
        let q = query.trim().to_lowercase();
        let entries = self.alias_index();
        let mut scored: Vec<(i32, AliasSuggestion)> = entries
            .into_iter()
            .filter_map(|(alias, description)| {
                let score = score_match(&alias, &description, &q);
                if score > 0 || q.is_empty() {
                    Some((
                        score,
                        AliasSuggestion {
                            alias,
                            description,
                        },
                    ))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0).then_with(|| a.1.alias.cmp(&b.1.alias))
        });
        scored.into_iter().take(12).map(|(_, s)| s).collect()
    }

    /// For each closed session whose effective date is before today's cut-over
    /// date: merge → round → append to the corresponding .tks file, then
    /// remove the exported sessions from local state.
    ///
    /// **Skipped while the edit form is open.** `EditSession` rows snapshot
    /// `original_start` / `original_end`, so a tick that dropped sessions
    /// from `state.timers` mid-edit would see them resurrected by the next
    /// `SaveEdit` (which rebuilds the timer's session vec from the buffer).
    /// Closing the form clears `self.editing` and lets the next tick proceed.
    fn auto_export_past_days(&mut self) {
        let Some(rc) = self.taxirc.clone() else {
            return;
        };
        let today = state::cutover_date(Local::now(), self.config.cutover_hour());
        run_auto_export(
            &mut self.state,
            self.editing,
            &rc,
            &self.config,
            today,
        );
    }

    /// Seed `state.timers` with one row per distinct alias found in the
    /// current and previous month's .tks files, unless suppressed.
    fn seed_timers_from_tks(&mut self) {
        let Some(rc) = self.taxirc.clone() else {
            return;
        };
        let cutover = self.config.cutover_hour();
        let today = state::cutover_date(Local::now(), cutover);
        let first_of_month = chrono::NaiveDate::from_ymd_opt(today.year(), today.month(), 1)
            .unwrap_or(today);
        let prev_month = first_of_month
            .pred_opt()
            .unwrap_or(first_of_month);

        let mut latest_desc: BTreeMap<String, (chrono::NaiveDate, String)> = BTreeMap::new();
        for date in [today, prev_month] {
            let path = taxi::resolve_tks_path(&rc.file_template, date);
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            for day in taxi::parse_tks(&content, &rc.date_format) {
                for entry in &day.entries {
                    let desc = entry.description.clone();
                    if desc.is_empty() {
                        latest_desc
                            .entry(entry.alias.clone())
                            .or_insert((day.date, String::new()));
                    } else {
                        latest_desc
                            .entry(entry.alias.clone())
                            .and_modify(|(d, s)| {
                                if day.date >= *d {
                                    *d = day.date;
                                    s.clone_from(&desc);
                                }
                            })
                            .or_insert((day.date, desc));
                    }
                }
            }
        }

        for (alias, (_, desc)) in latest_desc {
            if self.state.suppressed_aliases.contains(&alias) {
                continue;
            }
            if self.state.find_by_alias(&alias).is_some() {
                continue;
            }
            self.state.add_timer(alias, desc);
        }
    }

    fn commit_edit(&mut self, id: Uuid) -> anyhow::Result<()> {
        // Phase 1: validate **without** mutating self.edit_buf. If anything
        // fails, the user's typed input survives so they can fix it.
        let alias = self.edit_buf.alias.trim().to_owned();
        if alias.is_empty() {
            anyhow::bail!("alias cannot be empty");
        }

        let prev_alias = self
            .state
            .find_timer(id)
            .map(|t| t.alias.clone())
            .unwrap_or_default();
        if alias != prev_alias
            && self.state.timers.iter().any(|t| t.id != id && t.alias == alias)
        {
            anyhow::bail!("another timer already uses alias '{alias}'");
        }

        let last_idx = self.edit_buf.sessions.len().saturating_sub(1);
        let mut new_sessions = Vec::with_capacity(self.edit_buf.sessions.len());
        for (i, row) in self.edit_buf.sessions.iter().enumerate() {
            let start_time = parse_clock(&row.start)
                .ok_or_else(|| anyhow::anyhow!("invalid start time '{}'", row.start))?;
            let end_text = row.end.trim();
            let parsed_end = if end_text.is_empty() {
                if i != last_idx {
                    anyhow::bail!("only the last row may have an open end");
                }
                None
            } else {
                Some(
                    parse_clock(end_text).ok_or_else(|| {
                        anyhow::anyhow!("invalid end time '{}'", row.end)
                    })?,
                )
            };

            let start_dt = resolve_dt(
                row.original_start,
                row.date,
                row.start.trim(),
                start_time,
            )
            .ok_or_else(|| anyhow::anyhow!("could not place start on row's date"))?;

            let end_dt = if let Some(t) = parsed_end {
                let dt = match row.original_end {
                    Some(orig) => resolve_dt(orig, row.date, end_text, t),
                    None => state::datetime_on(row.date, t.hour(), t.minute()),
                }
                .ok_or_else(|| anyhow::anyhow!("could not place end on row's date"))?;
                if dt < start_dt {
                    anyhow::bail!("end cannot be before start");
                }
                Some(dt)
            } else {
                None
            };

            new_sessions.push(state::Session {
                start: start_dt,
                end: end_dt,
                description: row.description.text().trim().to_owned(),
            });
        }

        let last_desc = new_sessions
            .last()
            .map(|s| s.description.clone())
            .unwrap_or_default();

        // Phase 2: commit. Past this point the operation succeeds.
        let Some(timer) = self.state.find_timer_mut(id) else {
            anyhow::bail!("timer not found");
        };
        timer.alias = alias;
        timer.sessions = new_sessions;
        if !last_desc.is_empty() {
            timer.default_description = last_desc;
        }

        Ok(())
    }
}

/// Build the body lines for one timer on one date: quantized entry lines
/// (with `# original …` comments) followed by an optional aggregated
/// zero-duration block. Shared between the panel's auto-export pipeline
/// and the export dialog's preview.
pub fn build_block_lines(
    quantized: &[crate::sessions::Span],
    aggregate: Option<&crate::sessions::ZeroAggregate>,
    alias: &str,
    grid_minutes: u32,
) -> Vec<String> {
    let mut out = crate::sessions::export_lines(quantized, alias);
    if let Some(agg) = aggregate {
        out.extend(crate::sessions::aggregate_lines(agg, alias, grid_minutes));
    }
    out
}

/// Pure-ish core of `auto_export_past_days`, extracted so it can be unit-
/// tested without constructing a full `AppModel` (which carries `cosmic::Core`).
///
/// Returns early if `editing.is_some()` — the edit form is open and any
/// session removal would race the user's in-flight buffer (see
/// `auto_export_past_days` doc-comment).
fn run_auto_export(
    state: &mut AppState,
    editing: Option<Uuid>,
    rc: &Taxirc,
    config: &Config,
    today: chrono::NaiveDate,
) {
    if editing.is_some() {
        return;
    }

    let cutover = config.cutover_hour();
    let gap = config.merge_gap();
    let grid = config.round_min_minutes;

    // Collate all timers' lines per date so a single `append_day` call
    // emits one marker for everyone's contributions on that date.
    let mut by_date: std::collections::BTreeMap<chrono::NaiveDate, Vec<String>> =
        std::collections::BTreeMap::new();
    let mut to_export: Vec<(Uuid, usize)> = Vec::new();

    for timer in &state.timers {
        let groups = crate::sessions::group_by_date(&timer.sessions, cutover);
        for (date, day_sessions) in groups {
            if date >= today {
                continue;
            }
            let merged = crate::sessions::merge(day_sessions.clone(), gap);
            let (zeros, nonzero) = crate::sessions::split_zero_duration(merged);
            let quantized = crate::sessions::quantize_grid(nonzero, grid);
            let agg = crate::sessions::aggregate_zero(&zeros);

            if quantized.is_empty() && agg.is_none() {
                continue;
            }

            let block_lines = build_block_lines(&quantized, agg.as_ref(), &timer.alias, grid);
            by_date.entry(date).or_default().extend(block_lines);

            for (idx, s) in timer.sessions.iter().enumerate() {
                if s.end.is_some()
                    && state::cutover_date(s.start, cutover) == date
                {
                    to_export.push((timer.id, idx));
                }
            }
        }
    }

    let mut all_written_dates: Vec<chrono::NaiveDate> = Vec::new();
    for (date, body) in by_date {
        let path = taxi::resolve_tks_path(&rc.file_template, date);
        match taxi::append_day(&path, date, &body, &rc.date_format) {
            Ok(()) => all_written_dates.push(date),
            Err(e) => {
                tracing::warn!(error = %e, "auto-export append_day failed");
            }
        }
    }

    let written: std::collections::BTreeSet<_> = all_written_dates.into_iter().collect();
    to_export.retain(|(id, idx)| {
        state
            .find_timer(*id)
            .and_then(|t| t.sessions.get(*idx))
            .is_some_and(|s| written.contains(&state::cutover_date(s.start, cutover)))
    });
    for (timer_id, idx) in to_export.iter().rev() {
        if let Some(t) = state.find_timer_mut(*timer_id)
            && *idx < t.sessions.len()
        {
            t.sessions.remove(*idx);
        }
    }
}

fn build_edit_buf(timer: Option<&Timer>) -> EditBuf {
    let Some(t) = timer else {
        return EditBuf::default();
    };
    let sessions = t
        .sessions
        .iter()
        .map(|s| EditSession {
            description: cosmic::widget::text_editor::Content::with_text(&s.description),
            start: s.start.format("%H:%M").to_string(),
            end: s
                .end
                .map(|e| e.format("%H:%M").to_string())
                .unwrap_or_default(),
            date: s.start.date_naive(),
            original_start: s.start,
            original_end: s.end,
        })
        .collect();
    EditBuf {
        alias: t.alias.clone(),
        sessions,
        error: None,
        pending_delete: None,
        alias_picked: false,
        pending_delete_timer: false,
    }
}

fn parse_clock(s: &str) -> Option<NaiveTime> {
    let s = s.trim();
    if let Ok(t) = NaiveTime::parse_from_str(s, "%H:%M") {
        return Some(t);
    }
    if s.len() == 4 && s.chars().all(|c| c.is_ascii_digit()) {
        return NaiveTime::parse_from_str(s, "%H%M").ok();
    }
    None
}

/// Resolve the final `DateTime` for an edit-form row. If the displayed
/// `HH:MM` text matches the original timestamp's `HH:MM` and the row's date
/// is unchanged, the original (with sub-minute precision) is returned —
/// useful for sessions shorter than a minute that would otherwise collapse
/// to zero duration on save.
fn resolve_dt(
    original: chrono::DateTime<Local>,
    date: chrono::NaiveDate,
    displayed: &str,
    parsed: NaiveTime,
) -> Option<chrono::DateTime<Local>> {
    let unchanged = original.date_naive() == date
        && original.format("%H:%M").to_string() == displayed;
    if unchanged {
        Some(original)
    } else {
        state::datetime_on(date, parsed.hour(), parsed.minute())
    }
}

pub fn fmt_duration_hms(d: Duration) -> String {
    let total = d.num_seconds().max(0);
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

pub fn fmt_duration_hms_short(d: Duration) -> String {
    let total = d.num_seconds().max(0);
    let h = total / 3600;
    let m = (total % 3600) / 60;
    format!("{h:02}:{m:02}")
}

fn dispatch_surface(a: surface::Action) -> Task<Message> {
    cosmic::task::message(cosmic::Action::Cosmic(cosmic::app::Action::Surface(a)))
}

fn sigusr2_stream() -> impl cosmic::iced::futures::Stream<Item = Message> {
    cosmic::iced::stream::channel(4, |mut sender: cosmic::iced::futures::channel::mpsc::Sender<Message>| async move {
        let mut sig = match signal(SignalKind::user_defined2()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "failed to install SIGUSR2 handler");
                return;
            }
        };
        while sig.recv().await.is_some() {
            if sender.send(Message::ForceRefresh).await.is_err() {
                break;
            }
        }
    })
}

fn sigusr2_subscription() -> Subscription<Message> {
    Subscription::run(sigusr2_stream)
}

fn lock_stream() -> impl cosmic::iced::futures::Stream<Item = Message> {
    use futures_util::StreamExt;
    lock::stream().map(|ev| Message::LockEvt(LockEventDup::from(ev)))
}

fn lock_subscription() -> Subscription<Message> {
    Subscription::run(lock_stream)
}

fn spawn_subwindow(flag: &'static str) -> Task<Message> {
    cosmic::task::future(async move {
        match std::env::current_exe() {
            Ok(path) => {
                if let Err(e) = tokio::process::Command::new(path).arg(flag).spawn() {
                    tracing::warn!(error = %e, flag, "failed to spawn settings/export binary");
                }
            }
            Err(e) => tracing::warn!(error = %e, "current_exe() failed"),
        }
        Message::NoOp
    })
}

fn open_popup(new_id: Id) -> Task<Message> {
    let action = surface::action::app_popup::<AppModel>(
        move |state: &mut AppModel| {
            let parent = state.core.main_window_id().unwrap_or(Id::NONE);
            let mut settings =
                state.core.applet.get_popup_settings(parent, new_id, None, None, None);
            settings.grab = true;
            settings.positioner.size_limits = Limits::NONE
                .max_width(1640.0)
                .min_width(1120.0)
                .min_height(240.0)
                .max_height(1440.0);
            settings
        },
        Some(Box::new(|state: &AppModel| {
            let body = ui::popup_view(state);
            Element::from(state.core.applet.popup_container(body)).map(cosmic::Action::App)
        })),
    );
    dispatch_surface(action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeZone};
    use std::path::PathBuf;

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "cosmic-applet-taxi-app-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    fn yesterday_state() -> (AppState, Uuid) {
        let mut s = AppState::default();
        let id = s.add_timer("_x".into(), String::new()).unwrap();
        let t = s.find_timer_mut(id).unwrap();
        t.sessions.push(crate::state::Session {
            start: Local.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap(),
            end: Some(Local.with_ymd_and_hms(2026, 5, 12, 10, 0, 0).unwrap()),
            description: "y".into(),
        });
        (s, id)
    }

    fn rc_at(template: &std::path::Path) -> Taxirc {
        Taxirc {
            file_template: template.to_string_lossy().into_owned(),
            date_format: "%d/%m/%Y".to_owned(),
            aliases: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn auto_export_writes_yesterday_when_idle() {
        let tmp = tempdir();
        let path = tmp.join("month.tks");
        let rc = rc_at(&path);
        let cfg = Config::default();
        let (mut state, id) = yesterday_state();
        let today = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();

        run_auto_export(&mut state, None, &rc, &cfg, today);

        assert!(path.exists(), "auto-export should have written the .tks");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("12/05/2026"));
        assert!(content.contains("_x 09:00-10:00 y"));
        // Session was dropped from state on successful write.
        let t = state.find_timer(id).unwrap();
        assert!(t.sessions.is_empty(), "exported session should be removed");
    }

    #[test]
    fn auto_export_skipped_when_edit_form_open() {
        let tmp = tempdir();
        let path = tmp.join("month.tks");
        let rc = rc_at(&path);
        let cfg = Config::default();
        let (mut state, id) = yesterday_state();
        let today = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();

        // editing = Some(...) should short-circuit the whole pipeline:
        // no file write, no session removal.
        run_auto_export(&mut state, Some(id), &rc, &cfg, today);

        assert!(
            !path.exists(),
            ".tks must not be created while edit form is open"
        );
        let t = state.find_timer(id).unwrap();
        assert_eq!(
            t.sessions.len(),
            1,
            "session must remain in state while edit form is open"
        );
    }

    #[test]
    fn auto_export_skip_does_not_care_which_timer_is_being_edited() {
        // The skip is global — even if the user is editing a *different*
        // timer, we still don't move sessions from anywhere else, because
        // the form may reference any timer by alias once typed.
        let tmp = tempdir();
        let path = tmp.join("month.tks");
        let rc = rc_at(&path);
        let cfg = Config::default();
        let (mut state, _id) = yesterday_state();
        let unrelated = Uuid::new_v4();
        let today = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();

        run_auto_export(&mut state, Some(unrelated), &rc, &cfg, today);

        assert!(!path.exists());
        assert_eq!(state.timers[0].sessions.len(), 1);
    }
}
