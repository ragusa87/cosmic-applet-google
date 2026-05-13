use chrono::{Local, NaiveDate};
use cosmic::Element;
use cosmic::app::Task;
use cosmic::iced::{self, Alignment, Length, Size};
use cosmic::widget::{
    Column, Row, button, container, scrollable, text, text_editor, text_input,
};
use cosmic_config::CosmicConfigEntry;

use crate::app::build_block_lines;
use crate::config::{APP_ID, Config};
use crate::sessions;
use crate::state::{self, AppState};
use crate::taxi::{self, TaxiRunner, Taxirc};

pub fn run() -> iced::Result {
    // SAFETY: signal(2) with SIG_IGN is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGUSR2, libc::SIG_IGN);
    }
    let settings = cosmic::app::Settings::default().size(Size::new(720.0, 640.0));
    cosmic::app::run::<ExportApp>(settings, ())
}

pub struct ExportApp {
    core: cosmic::Core,
    config: Config,
    state: AppState,
    taxirc: Option<Taxirc>,
    taxi_runner: TaxiRunner,
    date_input: String,
    preview_content: text_editor::Content,
    /// Read-only `text_editor::Content` for the "current file" panel. We
    /// use `text_editor` rather than `text::monotext` so the user can
    /// click, drag-select, Ctrl+C, etc. Edit actions are filtered out at
    /// the action handler.
    current_content: text_editor::Content,
    /// Same shape, for the "raw content" collapsible.
    raw_content: text_editor::Content,
    show_current: bool,
    show_raw: bool,
    /// `to_remove[date] = Vec<(timer_id, session_idx)>` — populated each
    /// time the preview is recomputed. On successful Export, sessions for
    /// the chosen date get dropped from local state.
    pending_to_remove: Vec<(uuid::Uuid, usize)>,
    busy: bool,
    status: Option<String>,
}

impl Default for ExportApp {
    fn default() -> Self {
        Self {
            core: cosmic::Core::default(),
            config: Config::default(),
            state: AppState::default(),
            taxirc: None,
            taxi_runner: TaxiRunner {
                argv: Vec::new(),
                available: false,
            },
            date_input: Local::now().format("%d/%m/%Y").to_string(),
            preview_content: text_editor::Content::new(),
            current_content: text_editor::Content::new(),
            raw_content: text_editor::Content::new(),
            show_current: false,
            show_raw: false,
            pending_to_remove: Vec::new(),
            busy: false,
            status: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    DateInput(String),
    Today,
    PreviewAction(text_editor::Action),
    CurrentAction(text_editor::Action),
    RawAction(text_editor::Action),
    ResetPreview,
    ToggleCurrent,
    ToggleRaw,
    Export,
    Push,
    Copy,
    TaxiDetected(TaxiRunner),
    PushDone(Result<(), String>),
    Close,
}

impl cosmic::Application for ExportApp {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Msg;

    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &cosmic::Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(core: cosmic::Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let config = cosmic_config::Config::new(APP_ID, Config::VERSION)
            .map(|ctx| match Config::get_entry(&ctx) {
                Ok(c) => c,
                Err((_e, c)) => c,
            })
            .unwrap_or_default();

        let state = AppState::load().unwrap_or_default();
        let taxirc = Taxirc::resolve_path(&config).and_then(|p| taxi::load_taxirc(&p).ok());
        let format = taxirc
            .as_ref()
            .map_or("%d/%m/%Y", |t| t.date_format.as_str());
        let date_input = Local::now().format(format).to_string();

        let mut app = Self {
            core,
            config: config.clone(),
            state,
            taxirc,
            date_input,
            ..Self::default()
        };
        app.regenerate_preview();

        let detect = cosmic::task::future(async move {
            Msg::TaxiDetected(TaxiRunner::detect(&config).await)
        });
        (app, detect)
    }

    #[allow(clippy::too_many_lines)]
    fn view(&self) -> Element<'_, Self::Message> {
        let header = text::title4("Export to taxi");

        let date_input = text_input("dd/mm/yyyy", &self.date_input)
            .label("Date")
            .on_input(Msg::DateInput);
        let today_btn = button::standard("Today").on_press(Msg::Today);
        let date_row = Row::new()
            .spacing(8)
            .align_y(Alignment::End)
            .push(date_input)
            .push(today_btn);

        let date_format = self
            .taxirc
            .as_ref()
            .map_or("%d/%m/%Y", |t| t.date_format.as_str());
        let parsed_date = NaiveDate::parse_from_str(&self.date_input, date_format).ok();
        let path_text = match (&self.taxirc, parsed_date) {
            (Some(rc), Some(d)) => taxi::resolve_tks_path(&rc.file_template, d)
                .display()
                .to_string(),
            _ => "(invalid date or missing taxirc)".to_owned(),
        };
        let path_row = text::body(format!("Will write to: {path_text}"));

        let current_toggle = button::text(if self.show_current {
            "▼ Hide current file content"
        } else {
            "▶ Show current file content"
        })
        .on_press(Msg::ToggleCurrent)
        .class(cosmic::theme::Button::Text);

        let current_panel: Element<'_, Msg> = if self.show_current {
            // Selectable read-only editor: edit actions are filtered out
            // by `Msg::CurrentAction`, but click/drag/select/Ctrl+C work.
            container(
                text_editor(&self.current_content)
                    .placeholder("(no existing section for this date)")
                    .on_action(Msg::CurrentAction)
                    .height(Length::Fixed(160.0)),
            )
            .padding(4)
            .width(Length::Fill)
            .style(boxed_panel)
            .into()
        } else {
            cosmic::widget::Space::new()
                .width(Length::Fill)
                .height(Length::Fixed(0.0))
                .into()
        };

        let raw_toggle = button::text(if self.show_raw {
            "▼ Hide raw content"
        } else {
            "▶ Show raw content (verbatim sessions, no rounding)"
        })
        .on_press(Msg::ToggleRaw)
        .class(cosmic::theme::Button::Text);

        let raw_panel: Element<'_, Msg> = if self.show_raw {
            container(
                text_editor(&self.raw_content)
                    .placeholder("(no closed sessions for this date)")
                    .on_action(Msg::RawAction)
                    .height(Length::Fixed(160.0)),
            )
            .padding(4)
            .width(Length::Fill)
            .style(boxed_panel)
            .into()
        } else {
            cosmic::widget::Space::new()
                .width(Length::Fill)
                .height(Length::Fixed(0.0))
                .into()
        };

        let preview_editor = text_editor(&self.preview_content)
            .placeholder("(no closed sessions for this date)")
            .on_action(Msg::PreviewAction)
            .height(Length::Fixed(260.0));

        let reset_preview_btn = button::text("Reset preview")
            .on_press(Msg::ResetPreview)
            .class(cosmic::theme::Button::Text);

        let preview_area = Column::new()
            .spacing(4)
            .push(text::caption("Preview (editable)"))
            .push(preview_editor)
            .push(reset_preview_btn);

        // Action buttons
        let can_write =
            self.taxirc.is_some() && parsed_date.is_some() && !self.preview_content.text().trim().is_empty();
        let mut export_btn = button::suggested("Export");
        if can_write && !self.busy {
            export_btn = export_btn.on_press(Msg::Export);
        }

        let mut push_btn = button::standard("Push (taxi commit)");
        if can_write && !self.busy && self.taxi_runner.available {
            push_btn = push_btn.on_press(Msg::Push);
        }

        let mut copy_btn = button::standard("Copy to clipboard");
        if !self.preview_content.text().is_empty() && !self.busy {
            copy_btn = copy_btn.on_press(Msg::Copy);
        }

        let actions = Row::new()
            .spacing(8)
            .push(button::standard("Close").on_press(Msg::Close))
            .push(copy_btn)
            .push(push_btn)
            .push(export_btn);

        let status_text: String = match (&self.status, parsed_date) {
            (Some(s), _) => s.clone(),
            (None, Some(d)) => format!("Date interpreted as: {}", d.format("%Y-%m-%d")),
            (None, None) => "Date doesn't parse".to_owned(),
        };
        let status: Element<'_, Msg> = text::caption(status_text).into();

        let col = Column::new()
            .padding(16)
            .spacing(10)
            .push(header)
            .push(date_row)
            .push(path_row)
            .push(current_toggle)
            .push(current_panel)
            .push(raw_toggle)
            .push(raw_panel)
            .push(preview_area)
            .push(status)
            .push(actions);

        container(scrollable(col).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    #[allow(clippy::too_many_lines)]
    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Msg::DateInput(s) => {
                self.date_input = s;
                self.status = None;
                self.regenerate_preview();
                if self.show_current {
                    self.refresh_current_content();
                }
                if self.show_raw {
                    self.refresh_raw_content();
                }
            }
            Msg::Today => {
                let fmt = self
                    .taxirc
                    .as_ref()
                    .map_or("%d/%m/%Y", |t| t.date_format.as_str());
                self.date_input = Local::now().format(fmt).to_string();
                self.status = None;
                self.regenerate_preview();
                if self.show_current {
                    self.refresh_current_content();
                }
                if self.show_raw {
                    self.refresh_raw_content();
                }
            }
            Msg::PreviewAction(action) => {
                self.preview_content.perform(action);
            }
            Msg::CurrentAction(action) => {
                // Read-only: drop modifying actions so the text stays
                // verbatim from disk, but pass through everything else
                // (click / drag / Ctrl+A / Ctrl+C / scroll / arrow keys).
                if !action.is_edit() {
                    self.current_content.perform(action);
                }
            }
            Msg::RawAction(action) => {
                if !action.is_edit() {
                    self.raw_content.perform(action);
                }
            }
            Msg::ResetPreview => {
                self.regenerate_preview();
                self.status = Some("Preview reset from session state.".into());
            }
            Msg::ToggleCurrent => {
                self.show_current = !self.show_current;
                if self.show_current {
                    self.refresh_current_content();
                }
            }
            Msg::ToggleRaw => {
                self.show_raw = !self.show_raw;
                if self.show_raw {
                    self.refresh_raw_content();
                }
            }
            Msg::Export => {
                self.busy = true;
                let result = self.do_export();
                self.busy = false;
                match result {
                    Ok(()) => {
                        self.status = Some("✓ Exported.".into());
                        signal_applet_refresh();
                    }
                    Err(e) => self.status = Some(format!("✗ {e}")),
                }
            }
            Msg::Push => {
                self.busy = true;
                if let Err(e) = self.do_export() {
                    self.busy = false;
                    self.status = Some(format!("✗ export: {e}"));
                    return Task::none();
                }
                self.status = Some("Pushing…".into());
                signal_applet_refresh();
                let runner = self.taxi_runner.clone();
                return cosmic::task::future(async move {
                    let r = runner.run(&["ci"]).await.map_err(|e| e.to_string()).map(|_| ());
                    Msg::PushDone(r)
                });
            }
            Msg::PushDone(r) => {
                self.busy = false;
                self.status = Some(match r {
                    Ok(()) => "✓ Pushed.".into(),
                    Err(e) => format!("✗ taxi: {e}"),
                });
            }
            Msg::Copy => {
                // Clipboard writes are synchronous side effects in iced —
                // no async completion to await. Set the status and
                // dispatch the write task fire-and-forget.
                let text = self.preview_content.text();
                self.status = Some("✓ Copied to clipboard.".into());
                return cosmic::iced::clipboard::write::<cosmic::Action<Msg>>(text);
            }
            Msg::TaxiDetected(runner) => {
                self.taxi_runner = runner;
            }
            Msg::Close => return cosmic::iced::exit(),
        }
        Task::none()
    }
}

impl ExportApp {
    /// Compute the preview text from current `state` + config + `date_input`,
    /// replace the editor's content, and refresh `pending_to_remove` so a
    /// subsequent Export drops exactly the sessions that produced this
    /// preview.
    fn regenerate_preview(&mut self) {
        let (text, to_remove) = self.compute_preview_and_remove_list();
        self.preview_content = text_editor::Content::with_text(&text);
        self.pending_to_remove = to_remove;
    }

    fn compute_preview_and_remove_list(&self) -> (String, Vec<(uuid::Uuid, usize)>) {
        let date_format = self
            .taxirc
            .as_ref()
            .map_or("%d/%m/%Y", |t| t.date_format.as_str());
        let Some(date) = NaiveDate::parse_from_str(&self.date_input, date_format).ok() else {
            return (String::new(), Vec::new());
        };
        let cutover = self.config.cutover_hour();
        let gap = self.config.merge_gap();
        let grid = self.config.round_min_minutes;

        let mut lines: Vec<String> = vec![date.format(date_format).to_string()];
        let mut to_remove: Vec<(uuid::Uuid, usize)> = Vec::new();
        let mut any = false;

        for timer in &self.state.timers {
            let bucket: Vec<state::Session> = timer
                .sessions
                .iter()
                .filter(|s| s.end.is_some() && state::cutover_date(s.start, cutover) == date)
                .cloned()
                .collect();
            if bucket.is_empty() {
                continue;
            }
            let merged = sessions::merge(bucket, gap);
            let (zeros, nonzero) = sessions::split_zero_duration(merged);
            let quantized = sessions::quantize_grid(nonzero, grid);
            let agg = sessions::aggregate_zero(&zeros);
            if quantized.is_empty() && agg.is_none() {
                continue;
            }
            any = true;
            let block = build_block_lines(&quantized, agg.as_ref(), &timer.alias, grid);
            lines.extend(block);

            for (idx, s) in timer.sessions.iter().enumerate() {
                if s.end.is_some() && state::cutover_date(s.start, cutover) == date {
                    to_remove.push((timer.id, idx));
                }
            }
        }
        if !any {
            // Leave the preview empty so the editor's placeholder shows
            // `(no closed sessions for this date)` and the Export button
            // stays disabled. Writing a placeholder line into the file
            // would pile up under `append_day`'s marker on later sweeps.
            return (String::new(), Vec::new());
        }
        (lines.join("\n"), to_remove)
    }

    fn refresh_current_content(&mut self) {
        let body = self
            .taxirc
            .as_ref()
            .zip(parse_date(&self.date_input, self.taxirc.as_ref()))
            .and_then(|(rc, d)| read_existing_section(rc, d))
            .unwrap_or_else(|| "(no existing section for this date)".to_owned());
        self.current_content = text_editor::Content::with_text(&body);
    }

    fn refresh_raw_content(&mut self) {
        let body = parse_date(&self.date_input, self.taxirc.as_ref())
            .map_or_else(|| "(invalid date)".to_owned(), |d| self.compute_raw_text(d));
        self.raw_content = text_editor::Content::with_text(&body);
    }

    /// Verbatim taxi-format rendering of the closed sessions for `date`,
    /// sorted by start time, across all timers — no merge, no quantize,
    /// no zero-duration filter, no aggregate. The on-grid version lives in
    /// the editable preview; this is the "what really got recorded"
    /// reference shown in the Raw collapsible.
    fn compute_raw_text(&self, date: chrono::NaiveDate) -> String {
        let date_format = self
            .taxirc
            .as_ref()
            .map_or("%d/%m/%Y", |t| t.date_format.as_str());
        let cutover = self.config.cutover_hour();

        let mut entries: Vec<(chrono::DateTime<Local>, String)> = Vec::new();
        for timer in &self.state.timers {
            for s in &timer.sessions {
                let Some(end) = s.end else { continue };
                if state::cutover_date(s.start, cutover) != date {
                    continue;
                }
                let start_s = s.start.format("%H:%M");
                let end_s = end.format("%H:%M");
                let line = if s.description.is_empty() {
                    format!("{} {start_s}-{end_s}", timer.alias)
                } else {
                    format!("{} {start_s}-{end_s} {}", timer.alias, s.description)
                };
                entries.push((s.start, line));
            }
        }

        if entries.is_empty() {
            return "(no closed sessions for this date)".to_owned();
        }

        entries.sort_by_key(|(start, _)| *start);
        let mut out = String::new();
        out.push_str(&date.format(date_format).to_string());
        out.push('\n');
        for (_, line) in entries {
            out.push_str(&line);
            out.push('\n');
        }
        out
    }

    /// Write the editor's contents to the resolved .tks (replacing that
    /// day's section). Drops the matching sessions from local state on
    /// success.
    fn do_export(&mut self) -> anyhow::Result<()> {
        let rc = self
            .taxirc
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("taxirc not loaded"))?;
        let date_format = &rc.date_format;
        let date = NaiveDate::parse_from_str(&self.date_input, date_format)
            .map_err(|_| anyhow::anyhow!("invalid date"))?;

        // The editor may contain a header line at the top. Strip it if
        // present so we don't write the date twice (replace_day adds the
        // header itself).
        let text = self.preview_content.text();
        let body_lines: Vec<String> = text
            .lines()
            .filter(|l| {
                NaiveDate::parse_from_str(l.trim(), date_format).ok() != Some(date)
            })
            .map(str::to_owned)
            .collect();

        let has_real_entry = body_lines
            .iter()
            .any(|l| {
                let t = l.trim();
                !t.is_empty() && !t.starts_with('#')
            });
        if !has_real_entry {
            tracing::info!(
                "manual export skipped: no entries to write for {date}"
            );
            return Ok(());
        }

        let path = taxi::resolve_tks_path(&rc.file_template, date);
        taxi::replace_day(&path, date, &body_lines, date_format)?;

        let to_remove = std::mem::take(&mut self.pending_to_remove);
        for (timer_id, idx) in to_remove.iter().rev() {
            if let Some(t) = self.state.find_timer_mut(*timer_id)
                && *idx < t.sessions.len()
            {
                t.sessions.remove(*idx);
            }
        }
        self.state.save()?;
        Ok(())
    }
}

fn parse_date(input: &str, taxirc: Option<&Taxirc>) -> Option<NaiveDate> {
    let date_format = taxirc.map_or("%d/%m/%Y", |t| t.date_format.as_str());
    NaiveDate::parse_from_str(input, date_format).ok()
}

fn read_existing_section(rc: &Taxirc, date: NaiveDate) -> Option<String> {
    let path = taxi::resolve_tks_path(&rc.file_template, date);
    let content = std::fs::read_to_string(&path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let mut start: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if NaiveDate::parse_from_str(line.trim(), &rc.date_format).ok() == Some(date) {
            start = Some(i);
            break;
        }
    }
    let s = start?;
    let mut end = lines.len();
    for (i, line) in lines.iter().enumerate().skip(s + 1) {
        if NaiveDate::parse_from_str(line.trim(), &rc.date_format).is_ok() {
            end = i;
            break;
        }
    }
    Some(lines[s..end].join("\n"))
}

fn boxed_panel(theme: &cosmic::Theme) -> container::Style {
    container::Style {
        background: Some(theme.cosmic().background.component.base.into()),
        border: cosmic::iced::Border {
            radius: cosmic::iced::border::Radius::from(4.0),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn signal_applet_refresh() {
    let _ = std::process::Command::new("pkill")
        .args(["-USR2", "-f", "cosmic-applet-taxi"])
        .status();
}
