use cosmic::Element;
use cosmic::applet::menu_button;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{Column, Row, button, scrollable, settings, text, text_input, toggler};

use crate::app::Message;
use crate::calendar::Event;

#[derive(Debug, Clone, Default)]
pub enum Status {
    #[default]
    Idle,
    Authorizing,
    Saved,
    Error(String),
}

#[derive(Debug, Clone, Default)]
pub struct CredentialsForm {
    pub email: String,
    pub client_id: String,
    pub client_secret: String,
}

impl CredentialsForm {
    pub fn fill_ids_from_env(&mut self) {
        if self.client_id.is_empty()
            && let Ok(v) = std::env::var("AGENDA_PANEL_CLIENT_ID")
        {
            self.client_id = v;
        }
    }

    pub fn fill_secret_from_env(&mut self) {
        if self.client_secret.is_empty()
            && let Ok(v) = std::env::var("AGENDA_PANEL_CLIENT_SECRET")
        {
            self.client_secret = v;
        }
    }

    pub fn is_complete(&self) -> bool {
        !self.email.is_empty() && !self.client_id.is_empty() && !self.client_secret.is_empty()
    }
}

pub fn menu_view<'a>() -> Element<'a, Message> {
    Column::new()
        .padding(4)
        .spacing(0)
        .push(menu_button(text::body("Settings\u{2026}")).on_press(Message::OpenCredentials))
        .into()
}

pub fn event_info_view<'a>(event: Option<&'a Event>, calendar_url: &str) -> Element<'a, Message> {
    let header: Element<'_, Message> = match event {
        Some(ev) => {
            let mut col = Column::new()
                .padding([8, 16])
                .spacing(4)
                .width(Length::Fill)
                .push(text::title4(ev.summary.clone()))
                .push(text::body(format_event_when(ev)));
            if let Some(loc) = ev.location.as_deref() {
                col = col.push(text::body(format!("\u{1f4cd} {loc}")));
            }
            col.into()
        }
        None => Column::new()
            .padding([8, 16])
            .width(Length::Fill)
            .push(text::body("No upcoming events"))
            .into(),
    };

    let (label, url) = match event.and_then(|e| e.meet_url.as_deref()) {
        Some(u) => ("Open in Google Meet\u{2026}", u.to_owned()),
        None => ("Open calendar\u{2026}", calendar_url.to_owned()),
    };

    Column::new()
        .padding([8, 0])
        .spacing(4)
        .width(Length::Fill)
        .push(header)
        .push(menu_button(text::body(label)).on_press(Message::OpenUrl(url)))
        .into()
}

fn format_event_when(ev: &Event) -> String {
    let start = ev.start.with_timezone(&chrono::Local);
    let end = ev.end.with_timezone(&chrono::Local);
    if start.date_naive() == end.date_naive() {
        format!(
            "{}\n{} \u{2013} {}",
            start.format("%A, %B %-d, %Y"),
            start.format("%H:%M"),
            end.format("%H:%M"),
        )
    } else {
        format!(
            "{}\n\u{2192} {}",
            start.format("%a, %b %-d, %Y %H:%M"),
            end.format("%a, %b %-d, %Y %H:%M"),
        )
    }
}

/// Builders for the messages emitted by the settings form. The form widget
/// is shared between the panel applet and the standalone settings binary;
/// they have different `Message` enums, so callers pass closures that build
/// their own variants from the form events.
pub struct SettingsHandlers<M: Clone> {
    pub on_email: fn(String) -> M,
    pub on_client_id: fn(String) -> M,
    pub on_client_secret: fn(String) -> M,
    pub on_toggle_show_title: fn(bool) -> M,
    pub on_toggle_show_time: fn(bool) -> M,
    pub on_toggle_show_progress: fn(bool) -> M,
    pub on_toggle_notify: fn(bool) -> M,
    pub authorize: M,
    pub cancel: M,
}

#[allow(clippy::fn_params_excessive_bools, clippy::too_many_arguments)]
pub fn settings_view<'a, M: Clone + 'static>(
    form: &'a CredentialsForm,
    show_title: bool,
    show_time: bool,
    show_progress: bool,
    notify: bool,
    status: &'a Status,
    authorizing: bool,
    handlers: &SettingsHandlers<M>,
) -> Element<'a, M> {
    let header = text::title4("Settings");

    let email_field = text_input("user@gmail.com", &form.email)
        .label("Email")
        .on_input(handlers.on_email);

    let id_field = text_input("…apps.googleusercontent.com", &form.client_id)
        .label("OAuth client ID")
        .on_input(handlers.on_client_id);

    let secret_field = text_input("GOCSPX-…", &form.client_secret)
        .label("OAuth client secret")
        .password()
        .on_input(handlers.on_client_secret);

    let mut authorize = button::suggested("Authorize with Google");
    if form.is_complete() && !authorizing {
        authorize = authorize.on_press(handlers.authorize.clone());
    }

    let mut cancel = button::standard("Close");
    if !authorizing {
        cancel = cancel.on_press(handlers.cancel.clone());
    }

    let status_line: Element<'a, M> = match status {
        Status::Idle => text::caption("").into(),
        Status::Authorizing => text::caption("Waiting for browser…").into(),
        Status::Saved => text::caption("✔ Saved").into(),
        Status::Error(e) => text::caption(format!("✗ {e}")).into(),
    };

    let actions = Row::new()
        .align_y(Alignment::Center)
        .spacing(8)
        .push(cancel)
        .push(authorize)
        .push(status_line);

    let hint = text::caption(
        "Create an OAuth desktop client in Google Cloud Console (see README). \
         Scope: calendar.events.readonly.",
    );

    let display_section = settings::section()
        .title("Display")
        .add(settings::item(
            "Show event time next to icon",
            toggler(show_time).on_toggle(handlers.on_toggle_show_time),
        ))
        .add(settings::item(
            "Show event title next to countdown",
            toggler(show_title).on_toggle(handlers.on_toggle_show_title),
        ))
        .add(settings::item(
            "Show meeting progress on icon",
            toggler(show_progress).on_toggle(handlers.on_toggle_show_progress),
        ));

    let notifications_section = settings::section()
        .title("Notifications")
        .add(settings::item(
            "Enable meeting notifications",
            toggler(notify).on_toggle(handlers.on_toggle_notify),
        ));

    let content = Column::new()
        .padding(12)
        .spacing(10)
        .width(Length::Fill)
        .push(header)
        .push(text::body("Google Calendar credentials"))
        .push(email_field)
        .push(id_field)
        .push(secret_field)
        .push(actions)
        .push(hint)
        .push(display_section)
        .push(notifications_section);

    scrollable(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}
