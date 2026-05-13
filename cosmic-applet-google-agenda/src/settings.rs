use cosmic::Element;
use cosmic::app::Task;
use cosmic::iced::{self, Size};
use cosmic_config::CosmicConfigEntry;

use crate::auth;
use crate::config::{APP_ID, Config};
use crate::secrets::{self, Tokens};
use crate::ui::{self, CredentialsForm, SettingsHandlers, Status};

pub fn run() -> iced::Result {
    // Both modes ship in the same binary, so `pkill -USR2 cosmic-applet-google-agenda`
    // would also reach this process. SIGUSR2's default action is to terminate;
    // ignore it here so an external "refresh the applet" signal doesn't kill
    // an open settings window.
    // SAFETY: signal(2) with SIG_IGN is async-signal-safe and has no preconditions.
    unsafe {
        libc::signal(libc::SIGUSR2, libc::SIG_IGN);
    }

    let settings = cosmic::app::Settings::default().size(Size::new(500.0, 460.0));
    cosmic::app::run::<SettingsApp>(settings, ())
}

#[derive(Default)]
pub struct SettingsApp {
    core: cosmic::Core,
    config: Config,
    form: CredentialsForm,
    status: Status,
    authorizing: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    FormEmail(String),
    FormClientId(String),
    FormClientSecret(String),
    ToggleShowTitle(bool),
    ToggleShowTime(bool),
    ToggleShowProgress(bool),
    ToggleNotify(bool),
    Authorize,
    AuthorizeDone(Result<(String, String, Tokens), String>),
    Cancel,
    SavedAndExit,
    LoadTokens(Option<Tokens>),
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
                Err((_errors, c)) => c,
            })
            .unwrap_or_default();

        let mut form = CredentialsForm {
            email: config.email.clone(),
            client_id: config.client_id.clone(),
            ..CredentialsForm::default()
        };
        form.fill_ids_from_env();
        form.fill_secret_from_env();

        let task = if config.is_configured() {
            let email = config.email.clone();
            cosmic::task::future(async move {
                let tokens = secrets::load(&email).await.ok();
                Msg::LoadTokens(tokens)
            })
        } else {
            Task::none()
        };

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
        let handlers = SettingsHandlers {
            on_email: Msg::FormEmail,
            on_client_id: Msg::FormClientId,
            on_client_secret: Msg::FormClientSecret,
            on_toggle_show_title: Msg::ToggleShowTitle,
            on_toggle_show_time: Msg::ToggleShowTime,
            on_toggle_show_progress: Msg::ToggleShowProgress,
            on_toggle_notify: Msg::ToggleNotify,
            authorize: Msg::Authorize,
            cancel: Msg::Cancel,
        };
        ui::settings_view(
            &self.form,
            self.config.show_title,
            self.config.show_time,
            self.config.show_progress,
            self.config.notify,
            &self.status,
            self.authorizing,
            &handlers,
        )
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Msg::FormEmail(s) => self.form.email = s,
            Msg::FormClientId(s) => self.form.client_id = s,
            Msg::FormClientSecret(s) => self.form.client_secret = s,

            Msg::ToggleShowTitle(on) => {
                self.config.show_title = on;
                persist_config(&self.config);
            }

            Msg::ToggleShowTime(on) => {
                self.config.show_time = on;
                persist_config(&self.config);
            }

            Msg::ToggleShowProgress(on) => {
                self.config.show_progress = on;
                persist_config(&self.config);
            }

            Msg::ToggleNotify(on) => {
                self.config.notify = on;
                persist_config(&self.config);
            }

            Msg::LoadTokens(Some(tokens)) => {
                if self.form.client_secret.is_empty() {
                    self.form.client_secret = tokens.client_secret;
                }
            }
            Msg::LoadTokens(None) => {}

            Msg::Authorize => {
                if self.authorizing || !self.form.is_complete() {
                    return Task::none();
                }
                self.authorizing = true;
                self.status = Status::Authorizing;

                let email = self.form.email.clone();
                let client_id = self.form.client_id.clone();
                let client_secret = self.form.client_secret.clone();

                return cosmic::task::future(async move {
                    let result = auth::start_oauth_flow(client_id.clone(), client_secret).await;
                    let result = result
                        .map(|tokens| (email, client_id, tokens))
                        .map_err(|e| e.to_string());
                    Msg::AuthorizeDone(result)
                });
            }

            Msg::AuthorizeDone(Ok((email, client_id, tokens))) => {
                self.authorizing = false;
                self.status = Status::Saved;

                let mut new_cfg = self.config.clone();
                new_cfg.email.clone_from(&email);
                new_cfg.client_id = client_id;
                persist_config(&new_cfg);
                self.config = new_cfg;

                return cosmic::task::future(async move {
                    if let Err(e) = secrets::save(&email, &tokens).await {
                        tracing::warn!(error = %e, "failed to persist tokens");
                    }
                    Msg::SavedAndExit
                });
            }

            Msg::AuthorizeDone(Err(e)) => {
                self.authorizing = false;
                self.status = Status::Error(e);
            }

            Msg::Cancel | Msg::SavedAndExit => {
                return cosmic::iced::exit();
            }
        }
        Task::none()
    }
}

fn persist_config(config: &Config) {
    match cosmic_config::Config::new(APP_ID, Config::VERSION) {
        Ok(ctx) => {
            if let Err(why) = config.write_entry(&ctx) {
                tracing::warn!(?why, "failed writing config entry");
            }
        }
        Err(why) => tracing::warn!(?why, "failed opening cosmic-config"),
    }
}
