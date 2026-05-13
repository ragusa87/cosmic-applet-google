use cosmic::Element;
use cosmic::app::Task;
use cosmic::iced::widget::mouse_area;
use cosmic::iced::{Limits, Subscription, window::Id};
use cosmic::surface::{self, action::destroy_popup};
use cosmic::widget::{button, text};
use cosmic_config::CosmicConfigEntry;
use futures_util::SinkExt;
use tokio::signal::unix::{SignalKind, signal};

use cosmic_google_common::auth;
use cosmic_google_common::secrets::{self, Tokens};

use crate::config::{APP_ID, Config, KEYRING_SERVICE};
use crate::gmail;
use crate::ui;

const GMAIL_URL: &str = "https://mail.google.com";
const GMAIL_ICON_SVG: &[u8] =
    include_bytes!("../data/icons/com.github.ragusa87.CosmicAppletGmail.svg");

#[derive(Default)]
pub struct AppModel {
    pub core: cosmic::Core,
    pub config: Config,
    pub menu_popup: Option<Id>,
    pub unread: Option<u64>,
    pub stale: bool,
    pub tokens: Option<Tokens>,
}

#[derive(Debug, Clone)]
pub enum Message {
    LeftClick,
    OpenMenu,
    PopupClosed(Id),
    OpenCredentials,

    Tick,
    Fetched(Result<(Tokens, u64), String>),
    ForceRefresh,

    UpdateConfig(Config),
    TokensLoaded(Option<Tokens>),

    NoOp,
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

        let app = AppModel {
            core,
            config: config.clone(),
            ..Default::default()
        };

        let task = if config.is_configured() {
            let email = config.email.clone();
            cosmic::task::future(async move {
                let tokens = secrets::load(KEYRING_SERVICE, &email).await.ok();
                Message::TokensLoaded(tokens)
            })
        } else {
            Task::none()
        };

        (app, task)
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn view(&self) -> Element<'_, Self::Message> {
        use cosmic::applet::cosmic_panel_config::PanelAnchor;
        use cosmic::iced::{Color, Length};

        let is_horizontal = matches!(
            self.core.applet.anchor,
            PanelAnchor::Top | PanelAnchor::Bottom
        );

        let (icon_size, _) = self.core.applet.suggested_size(true);
        let (pad_major, pad_minor) = self.core.applet.suggested_padding(true);
        let icon_px = f32::from(icon_size);

        let icon = cosmic::widget::icon(cosmic::widget::icon::from_svg_bytes(
            GMAIL_ICON_SVG.to_vec(),
        ))
        .size(icon_size);

        let badge_label = match (self.unread, self.config.is_configured()) {
            (None, false) => None,
            (Some(n), _) => Some(n.to_string()),
            (None, true) => Some("…".to_owned()),
        };

        let badge_height = (icon_px * 0.7).round();
        let badge_text_size = (icon_px * 0.46).round();
        let badge_pad_h = (icon_px * 0.22).round();
        let badge_pad_v = (icon_px * 0.06).round();
        let badge_radius = badge_height / 2.0;
        let badge_color = Color::from_rgb(0.13, 0.50, 0.96);

        let extra = badge_radius.round();
        let stack_px = icon_px + extra;

        let icon_area = cosmic::widget::container(icon)
            .width(Length::Fixed(stack_px))
            .height(Length::Fixed(stack_px))
            .align_x(cosmic::iced::alignment::Horizontal::Left)
            .align_y(cosmic::iced::alignment::Vertical::Top);

        let stacked: Element<'_, Self::Message> = if let Some(label) = badge_label {
            let badge_text = text(label)
                .size(badge_text_size)
                .class(Color::WHITE)
                .font(cosmic::font::bold());

            let badge_pill = cosmic::widget::container(badge_text)
                .padding([badge_pad_v, badge_pad_h])
                .height(Length::Fixed(badge_height))
                .align_x(cosmic::iced::alignment::Horizontal::Center)
                .align_y(cosmic::iced::alignment::Vertical::Center)
                .style(move |_theme: &cosmic::Theme| cosmic::iced::widget::container::Style {
                    background: Some(cosmic::iced::Background::Color(badge_color)),
                    border: cosmic::iced::Border {
                        radius: cosmic::iced::border::Radius::from(badge_radius),
                        ..Default::default()
                    },
                    text_color: Some(Color::WHITE),
                    ..Default::default()
                });

            let badge_area = cosmic::widget::container(badge_pill)
                .width(Length::Fixed(stack_px))
                .height(Length::Fixed(stack_px))
                .align_x(cosmic::iced::alignment::Horizontal::Right)
                .align_y(cosmic::iced::alignment::Vertical::Bottom);

            cosmic::iced::widget::Stack::new()
                .width(Length::Fixed(stack_px))
                .height(Length::Fixed(stack_px))
                .push(icon_area)
                .push(badge_area)
                .into()
        } else {
            icon_area.into()
        };

        let (horizontal_padding, vertical_padding) = if is_horizontal {
            (pad_major, pad_minor)
        } else {
            (pad_minor, pad_major)
        };

        let btn = button::custom(stacked)
            .padding([vertical_padding, horizontal_padding])
            .on_press(Message::LeftClick)
            .class(cosmic::theme::Button::AppletIcon);

        let interactive = mouse_area(btn).on_right_press(Message::OpenMenu);

        self.core.applet.autosize_window(interactive).into()
    }

    fn view_window(&self, _id: Id) -> Element<'_, Self::Message> {
        // The right-click menu popup uses an inline view via surface::action,
        // so this fallback isn't reached.
        cosmic::widget::container(cosmic::widget::text("")).into()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let poll = cosmic::iced::time::every(self.config.poll_interval()).map(|_| Message::Tick);
        let watch = self
            .core()
            .watch_config::<Config>(Self::APP_ID)
            .map(|update| Message::UpdateConfig(update.config));
        Subscription::batch([poll, watch, sigusr2_subscription()])
    }

    #[allow(clippy::too_many_lines)]
    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::NoOp => {}

            Message::LeftClick => {
                // Drop the click when the menu popup is up. Wayland can deliver
                // the click event to the panel surface as the popup's grab is
                // being dismissed, which would otherwise open Gmail when the
                // user meant to interact with the menu.
                if self.menu_popup.is_some() {
                    return Task::none();
                }
                return cosmic::task::future(async {
                    let _ = tokio::process::Command::new("xdg-open")
                        .arg(GMAIL_URL)
                        .status()
                        .await;
                    Message::NoOp
                });
            }

            Message::OpenMenu => {
                if let Some(id) = self.menu_popup.take() {
                    return dispatch_surface(destroy_popup(id));
                }
                let new_id = Id::unique();
                self.menu_popup = Some(new_id);
                return open_menu_popup(new_id);
            }

            Message::PopupClosed(id) => {
                if self.menu_popup.as_ref() == Some(&id) {
                    self.menu_popup = None;
                }
            }

            Message::OpenCredentials => {
                let destroy_menu = self
                    .menu_popup
                    .take()
                    .map_or_else(Task::none, |id| dispatch_surface(destroy_popup(id)));

                let launch = cosmic::task::future(async {
                    match std::env::current_exe() {
                        Ok(path) => {
                            if let Err(e) = tokio::process::Command::new(path)
                                .arg("--show-settings")
                                .spawn()
                            {
                                tracing::warn!(error = %e, "failed to spawn settings binary");
                            }
                        }
                        Err(e) => tracing::warn!(error = %e, "current_exe() failed"),
                    }
                    Message::NoOp
                });

                return Task::batch([destroy_menu, launch]);
            }

            Message::Tick => {
                let Some(tokens) = self.tokens.clone() else {
                    return Task::none();
                };
                if !self.config.is_configured() {
                    return Task::none();
                }
                let client_id = self.config.client_id.clone();
                let email = self.config.email.clone();
                return cosmic::task::future(async move {
                    let result = refresh_and_fetch(&client_id, &email, tokens)
                        .await
                        .map_err(|e| e.to_string());
                    Message::Fetched(result)
                });
            }

            Message::Fetched(Ok((tokens, count))) => {
                self.tokens = Some(tokens);
                self.unread = Some(count);
                self.stale = false;
            }

            Message::Fetched(Err(e)) => {
                tracing::warn!(error = %e, "fetch failed");
                self.stale = true;
            }

            Message::ForceRefresh => {
                if !self.config.is_configured() {
                    return Task::none();
                }
                tracing::info!("SIGUSR2 received, reloading tokens from keyring");
                let email = self.config.email.clone();
                return cosmic::task::future(async move {
                    let tokens = secrets::load(KEYRING_SERVICE, &email).await.ok();
                    Message::TokensLoaded(tokens)
                });
            }

            Message::UpdateConfig(config) => {
                let email_changed = config.email != self.config.email;
                self.config = config;
                if email_changed && !self.config.email.is_empty() {
                    let email = self.config.email.clone();
                    return cosmic::task::future(async move {
                        let tokens = secrets::load(KEYRING_SERVICE, &email).await.ok();
                        Message::TokensLoaded(tokens)
                    });
                }
                if self.config.email.is_empty() {
                    self.tokens = None;
                    self.unread = None;
                }
            }

            Message::TokensLoaded(tokens) => {
                self.tokens = tokens;
                return cosmic::task::message(cosmic::Action::App(Message::Tick));
            }
        }
        Task::none()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
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

fn open_menu_popup(new_id: Id) -> Task<Message> {
    let action = surface::action::app_popup::<AppModel>(
        move |state: &mut AppModel| {
            let parent = state.core.main_window_id().unwrap_or(Id::NONE);
            let mut settings =
                state.core.applet.get_popup_settings(parent, new_id, None, None, None);
            settings.grab = true;
            settings.positioner.size_limits = Limits::NONE
                .max_width(280.0)
                .min_width(180.0)
                .min_height(40.0)
                .max_height(200.0);
            settings
        },
        Some(Box::new(|state: &AppModel| {
            let body = ui::menu_view();
            Element::from(state.core.applet.popup_container(body)).map(cosmic::Action::App)
        })),
    );
    dispatch_surface(action)
}

async fn refresh_and_fetch(
    client_id: &str,
    email: &str,
    tokens: Tokens,
) -> anyhow::Result<(Tokens, u64)> {
    let tokens = if tokens.is_access_token_fresh() {
        tokens
    } else {
        let new = auth::refresh(client_id, &tokens).await?;
        if let Err(e) = secrets::save(KEYRING_SERVICE, email, &new).await {
            tracing::warn!(error = %e, "failed to persist refreshed tokens");
        }
        new
    };
    let count = gmail::unread_count(&tokens.access_token).await?;
    Ok((tokens, count))
}
