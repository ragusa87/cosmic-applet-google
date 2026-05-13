use cosmic::Element;
use cosmic::app::Task;
use cosmic::iced::{self, Length, Size};
use cosmic::widget::{Column, Row, button, container, scrollable, text, text_input};
use cosmic_config::CosmicConfigEntry;

use crate::config::{APP_ID, Config};
use crate::taxi::TaxiRunner;

pub fn run() -> iced::Result {
    // The applet listens for SIGUSR2 to force-refresh. The settings binary
    // shares the same process name; install SIG_IGN so pkill -USR2 doesn't
    // kill an open settings window.
    // SAFETY: signal(2) with SIG_IGN is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGUSR2, libc::SIG_IGN);
    }

    let settings = cosmic::app::Settings::default().size(Size::new(520.0, 460.0));
    cosmic::app::run::<SettingsApp>(settings, ())
}

#[derive(Default)]
pub struct SettingsApp {
    core: cosmic::Core,
    config: Config,
    form: Form,
    taxi: Option<TaxiRunner>,
    aliases_count: Option<usize>,
    status: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct Form {
    cutover_hour: String,
    merge_gap: String,
    round_min: String,
    taxi_command: String,
    taxirc_path: String,
}

impl Form {
    fn from_config(c: &Config) -> Self {
        Self {
            cutover_hour: c.cutover_hour.to_string(),
            merge_gap: c.merge_gap_minutes.to_string(),
            round_min: c.round_min_minutes.to_string(),
            taxi_command: c.taxi_command.clone(),
            taxirc_path: c.taxirc_path.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    FormCutover(String),
    FormMergeGap(String),
    FormRoundMin(String),
    FormTaxiCommand(String),
    FormTaxircPath(String),
    Save,
    Saved,
    RefreshAliases,
    AliasesRefreshed(usize, Option<String>),
    TaxiReady(TaxiRunner),
    Close,
}

impl cosmic::Application for SettingsApp {
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

        let form = Form::from_config(&config);
        let cfg = config.clone();
        let task = cosmic::task::future(async move {
            Msg::TaxiReady(TaxiRunner::detect(&cfg).await)
        });

        (
            Self {
                core,
                config,
                form,
                ..Self::default()
            },
            task,
        )
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let header = text::title4("Cosmic Applet Taxi — Settings");

        let cutover_input = text_input("4", &self.form.cutover_hour)
            .label("Cut-over hour (0–23)")
            .on_input(Msg::FormCutover);
        let cutover_help = text::caption(
            "Boundary between yesterday's and today's work. Sessions whose \
             start time is before this hour count toward the previous day's \
             timesheet. Set to 4 if you sometimes work past midnight; set to \
             0 to use the calendar day boundary.",
        );
        let cutover = Column::new()
            .spacing(2)
            .push(cutover_input)
            .push(cutover_help);

        let gap = text_input("5", &self.form.merge_gap)
            .label("Merge gap (minutes)")
            .on_input(Msg::FormMergeGap);
        let round = text_input("15", &self.form.round_min)
            .label("Round minimum (minutes)")
            .on_input(Msg::FormRoundMin);
        let cmd = text_input("uv run --with taxi,taxi-zebra taxi", &self.form.taxi_command)
            .label("Taxi command")
            .on_input(Msg::FormTaxiCommand);
        let taxirc = text_input(
            "(default: ~/.config/taxi/taxirc)",
            &self.form.taxirc_path,
        )
        .label("taxirc path")
        .on_input(Msg::FormTaxircPath);

        let mut diag = Column::new().spacing(2);
        match self.taxi.as_ref() {
            Some(r) if r.available => {
                diag = diag.push(text::caption("uv: detected, taxi enabled"));
            }
            Some(_) => {
                diag = diag.push(text::caption(
                    "uv: not found — taxi export disabled. Install `uv`.",
                ));
            }
            None => {
                diag = diag.push(text::caption("Probing uv…"));
            }
        }
        if let Some(n) = self.aliases_count {
            diag = diag.push(text::caption(format!("{n} aliases cached")));
        }

        let mut refresh = button::standard("Refresh aliases");
        if self.taxi.as_ref().is_some_and(|r| r.available) {
            refresh = refresh.on_press(Msg::RefreshAliases);
        }

        // Transient status (e.g. "Refreshing…", "Refreshed.", "Save failed:
        // …"). The persistent count of cached aliases lives in `diag` so the
        // two never duplicate.
        let status: Element<'_, Msg> = match &self.status {
            Some(s) => text::caption(s.clone()).into(),
            None => text::caption("").into(),
        };

        let actions = Row::new()
            .spacing(8)
            .push(button::standard("Close").on_press(Msg::Close))
            .push(refresh)
            .push(button::suggested("Save").on_press(Msg::Save))
            .push(status);

        let col = Column::new()
            .padding(16)
            .spacing(10)
            .push(header)
            .push(cutover)
            .push(gap)
            .push(round)
            .push(cmd)
            .push(taxirc)
            .push(actions)
            .push(diag);

        container(scrollable(col).height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Msg::FormCutover(s) => self.form.cutover_hour = s,
            Msg::FormMergeGap(s) => self.form.merge_gap = s,
            Msg::FormRoundMin(s) => self.form.round_min = s,
            Msg::FormTaxiCommand(s) => self.form.taxi_command = s,
            Msg::FormTaxircPath(s) => self.form.taxirc_path = s,

            Msg::Save => {
                if let Err(e) = self.save() {
                    self.status = Some(format!("Save failed: {e}"));
                    return Task::none();
                }
                self.status = Some("Saved.".into());
                signal_applet_refresh();
                return cosmic::task::future(async { Msg::Saved });
            }
            Msg::Saved => {}

            Msg::TaxiReady(r) => {
                self.taxi = Some(r);
            }

            Msg::RefreshAliases => {
                let Some(runner) = self.taxi.clone() else {
                    return Task::none();
                };
                if !runner.available {
                    return Task::none();
                }
                self.status = Some("Refreshing…".into());
                return cosmic::task::future(async move {
                    if let Err(e) = runner.update().await {
                        return Msg::AliasesRefreshed(0, Some(format!("update: {e}")));
                    }
                    match runner.alias_list().await {
                        Ok(m) => Msg::AliasesRefreshed(m.len(), None),
                        Err(e) => Msg::AliasesRefreshed(0, Some(format!("list: {e}"))),
                    }
                });
            }
            Msg::AliasesRefreshed(n, err) => {
                self.aliases_count = Some(n);
                // The diag block already shows "{n} aliases cached"
                // permanently. Status carries only transient messages
                // (errors or a brief "Refreshed." acknowledgement) so the
                // count doesn't appear twice.
                self.status = match err {
                    Some(e) => Some(e),
                    None => Some("Refreshed.".to_owned()),
                };
            }

            Msg::Close => return cosmic::iced::exit(),
        }
        Task::none()
    }
}

impl SettingsApp {
    fn save(&mut self) -> anyhow::Result<()> {
        let cutover: u8 = self
            .form
            .cutover_hour
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("cut-over hour must be a number 0–23"))?;
        if cutover > 23 {
            anyhow::bail!("cut-over hour out of range");
        }
        let gap: u32 = self
            .form
            .merge_gap
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("merge gap must be a number"))?;
        let round: u32 = self
            .form
            .round_min
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("round min must be a number"))?;
        let cmd = self.form.taxi_command.trim().to_owned();
        if cmd.is_empty() {
            anyhow::bail!("taxi command cannot be empty");
        }
        let new_cfg = Config {
            cutover_hour: cutover,
            merge_gap_minutes: gap,
            round_min_minutes: round,
            taxi_command: cmd,
            taxirc_path: self.form.taxirc_path.trim().to_owned(),
        };
        let ctx = cosmic_config::Config::new(APP_ID, Config::VERSION)
            .map_err(|e| anyhow::anyhow!("cosmic-config: {e}"))?;
        new_cfg
            .write_entry(&ctx)
            .map_err(|e| anyhow::anyhow!("write_entry: {e}"))?;
        self.config = new_cfg;
        Ok(())
    }
}

fn signal_applet_refresh() {
    // Force the panel applet to reload state + re-detect taxi.
    // Send to the panel binary by pid; the settings window installs
    // SIG_IGN at startup so signalling self by name (pkill) is safe too.
    let _ = std::process::Command::new("pkill")
        .args(["-USR2", "-f", "cosmic-applet-taxi"])
        .status();
}
