use chrono::Local;
use cosmic::Element;
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{Column, Row, button, container, scrollable, text, text_input};

use crate::app::{AliasSuggestion, AppModel, EditSession, Message, fmt_duration_hms};
use crate::state::{self, Timer};

const PLAY: &str = "\u{25b6}";
const PAUSE_ICON: &str = "\u{23f8}";
const DELETE: &str = "\u{2716}";
const ADD: &str = "+";
const EDIT_ICON: &str = "\u{270e}";
const RESET_ICON: &str = "\u{21bb}";

pub fn popup_view(app: &AppModel) -> Element<'_, Message> {
    let mut col = Column::new().padding(8).spacing(6).width(Length::Fill);

    if app.state.timers.is_empty() {
        col = col.push(
            text::body("No timers yet — use “+ Add timer” to start tracking.")
                .width(Length::Fill),
        );
    } else if let Some(editing_id) = app.editing {
        // Exclusive edit view: only the timer being edited is visible.
        // Save / Cancel inside the edit row are the only ways out.
        if let Some(timer) = app.state.find_timer(editing_id) {
            col = col.push(edit_row(app, timer));
        }
    } else {
        for timer in &app.state.timers {
            col = col.push(timer_row(app, timer));
        }
    }

    col = col.push(
        container(text::caption(" ").width(Length::Fill))
            .height(Length::Fixed(1.0))
            .width(Length::Fill)
            .style(|theme: &cosmic::Theme| container::Style {
                background: Some(theme.cosmic().background.divider.into()),
                ..Default::default()
            }),
    );

    col = col.push(total_row(app));

    // While editing a timer, hide the footer action row + add-bar so the
    // user can focus on the edit form. Save/Cancel inside the edit row
    // remain reachable.
    if app.editing.is_none() {
        col = col.push(footer_row(app));
        if app.add_buf.active {
            col = col.push(add_row(app));
        }
    }

    if let Some(s) = &app.status {
        col = col.push(text::caption(s.clone()));
    }

    if !app.taxi.available {
        col = col.push(text::caption(
            "Install `uv` to enable taxi export (uv run --with taxi,taxi-zebra taxi).",
        ));
    }

    col.into()
}

/// Truncate `alias: default-description` so a long description can't push
/// the action buttons off-screen. Ellipsis at codepoint boundary. Always
/// shows the **timer's** `default_description` rather than the running
/// session's snapshot — keeps the row stable while a session is in flight
/// and keeps semantics clear when the user opens edit.
fn timer_label(t: &Timer) -> String {
    const MAX: usize = 60;
    let desc = t.default_description.as_str();
    let s = if desc.is_empty() {
        t.alias.clone()
    } else {
        format!("{}: {desc}", t.alias)
    };
    if s.chars().count() > MAX {
        let head: String = s.chars().take(MAX - 1).collect();
        format!("{head}…")
    } else {
        s
    }
}

// Pixel widths used to give the timer row a stable column layout — without
// these the iced auto-sizer makes the duration / action buttons jitter
// between rows depending on each row's text content.
const COL_DURATION: f32 = 80.0;
const COL_BUTTON: f32 = 36.0;

/// Fixed-width button with a single glyph centered in the content area.
/// `button::text(...)` left-aligns its label inside a wider button; this
/// wraps a `Length::Fill` centered `text` inside `button::custom` so the
/// glyph sits in the middle.
///
/// `accessible_label` powers the hover-tooltip so the button's purpose is
/// discoverable without text being visible on its face. Pass `None` for
/// `on_press` to render the button disabled (no click handler) — used for
/// footer actions like Export when taxi isn't available.
fn icon_button(
    glyph: &'static str,
    accessible_label: &'static str,
    on_press: Option<Message>,
) -> Element<'static, Message> {
    use cosmic::iced::alignment::Horizontal;
    let mut btn = button::custom(
        text::body(glyph)
            .width(Length::Fill)
            .align_x(Horizontal::Center),
    )
    .class(cosmic::theme::Button::Text)
    .width(Length::Fixed(COL_BUTTON));
    if let Some(m) = on_press {
        btn = btn.on_press(m);
    }
    cosmic::widget::tooltip(
        btn,
        cosmic::widget::container(text::body(accessible_label)).padding(4),
        cosmic::widget::tooltip::Position::Top,
    )
    .into()
}

fn timer_row<'a>(app: &'a AppModel, t: &'a Timer) -> Element<'a, Message> {
    use cosmic::iced::alignment::Horizontal;

    let now = Local::now();
    let cutover = app.config.cutover_hour();
    let day = state::cutover_date(now, cutover);
    let elapsed = state::sum_for_date(t, day, cutover, now);

    let running = t.is_running();

    let duration_label = text::body(fmt_duration_hms(elapsed))
        .width(Length::Fixed(COL_DURATION))
        .align_x(Horizontal::Right);

    let play_pause = icon_button(
        if running { PAUSE_ICON } else { PLAY },
        if running { "Pause" } else { "Start" },
        Some(Message::StartPause(t.id)),
    );
    let edit_btn = icon_button(EDIT_ICON, "Edit timer", Some(Message::StartEdit(t.id)));
    let reset_btn = icon_button(RESET_ICON, "Reset timer", Some(Message::Reset(t.id)));

    let label = text::body(timer_label(t))
        .width(Length::Fill)
        .wrapping(cosmic::iced::widget::text::Wrapping::None);

    Row::new()
        .align_y(Alignment::Center)
        .spacing(6)
        .push(duration_label)
        .push(play_pause)
        .push(edit_btn)
        .push(reset_btn)
        .push(label)
        .into()
}

fn edit_row<'a>(app: &'a AppModel, t: &'a Timer) -> Element<'a, Message> {
    let buf = &app.edit_buf;

    let suggestions = app.alias_suggestions(&buf.alias);
    let show_suggestions =
        !suggestions.is_empty() && buf.alias != t.alias && !buf.alias_picked;
    let suggest_dropdown = if show_suggestions {
        let mut sc = Column::new().spacing(2);
        for s in suggestions {
            sc = sc.push(suggestion_row(s, Message::EditAliasPick));
        }
        Element::from(
            container(scrollable(sc).height(Length::Fixed(140.0)))
                .padding(2)
                .width(Length::Fill),
        )
    } else {
        Element::from(container(text::caption("")).height(Length::Fixed(0.0)))
    };

    let alias_input = text_input("alias", &buf.alias)
        .on_input(Message::EditAlias)
        .width(Length::Fill);

    let header = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(text::body("alias:"))
        .push(alias_input)
        .push(
            button::standard("Save")
                .on_press(Message::SaveEdit)
                .class(cosmic::theme::Button::Suggested),
        )
        .push(button::standard("Cancel").on_press(Message::CancelEdit));

    let mut session_col = Column::new().spacing(4).width(Length::Fill);
    session_col = session_col.push(
        Row::new()
            .spacing(8)
            .push(text::caption("description").width(Length::FillPortion(3)))
            .push(text::caption("start").width(Length::FillPortion(1)))
            .push(text::caption("end").width(Length::FillPortion(1)))
            .push(text::caption("").width(Length::Fixed(28.0))),
    );

    let today = Local::now().date_naive();
    for (i, row) in buf.sessions.iter().enumerate() {
        let pending = buf.pending_delete == Some(i);
        session_col = session_col.push(edit_session_row(i, row, pending, today));
    }

    let add_btn = button::text(format!("{ADD} Add session"))
        .on_press(Message::EditAddSession)
        .class(cosmic::theme::Button::Text);

    let delete_timer_label = if buf.pending_delete_timer {
        "Confirm delete?"
    } else {
        "Delete timer"
    };
    let delete_timer_btn = button::standard(delete_timer_label)
        .on_press(Message::EditDeleteTimer)
        .class(cosmic::theme::Button::Destructive);

    let edit_footer = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(add_btn)
        .push(
            cosmic::widget::Space::new()
                .width(Length::Fill)
                .height(Length::Fixed(0.0)),
        )
        .push(delete_timer_btn);

    let mut col = Column::new()
        .spacing(6)
        .padding(8)
        .width(Length::Fill)
        .push(header)
        .push(suggest_dropdown)
        .push(session_col)
        .push(edit_footer);

    if let Some(err) = &buf.error {
        col = col.push(text::caption(err.clone()));
    }

    container(col)
        .padding(4)
        .width(Length::Fill)
        .style(|theme: &cosmic::Theme| container::Style {
            background: Some(theme.cosmic().background.component.base.into()),
            border: cosmic::iced::Border {
                radius: cosmic::iced::border::Radius::from(4.0),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

fn suggestion_row(
    s: AliasSuggestion,
    on_pick: fn(String) -> Message,
) -> Element<'static, Message> {
    let AliasSuggestion { alias, description } = s;
    let alias_label = text::body(alias.clone()).font(cosmic::font::bold());
    let body: Element<'static, Message> = if description.is_empty() {
        Element::from(alias_label)
    } else {
        Element::from(
            Column::new()
                .spacing(1)
                .push(alias_label)
                .push(text::caption(description)),
        )
    };
    button::custom(body)
        .on_press(on_pick(alias))
        .width(Length::Fill)
        .class(cosmic::theme::Button::Text)
        .into()
}

fn edit_session_row(
    i: usize,
    row: &EditSession,
    pending_delete: bool,
    today: chrono::NaiveDate,
) -> Element<'_, Message> {
    let desc = cosmic::widget::text_editor(&row.description)
        .placeholder("description")
        .on_action(move |a| Message::EditSessionDesc(i, a))
        .height(Length::Fixed(64.0));
    let desc = cosmic::widget::container(desc).width(Length::FillPortion(3));
    let start = text_input("HH:MM", &row.start)
        .on_input(move |s| Message::EditSessionStart(i, s))
        .width(Length::FillPortion(1));
    let end = text_input("HH:MM (blank = running)", &row.end)
        .on_input(move |s| Message::EditSessionEnd(i, s))
        .width(Length::FillPortion(1));

    let del_label = if pending_delete { "delete?" } else { DELETE };
    let del = button::text(del_label)
        .on_press(Message::EditDeleteSession(i))
        .class(cosmic::theme::Button::Destructive);

    let date_caption = if row.date == today {
        text::caption("")
    } else {
        text::caption(row.date.format("%d/%m").to_string())
    };

    let row_widget = Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(desc)
        .push(start)
        .push(end)
        .push(date_caption)
        .push(del);
    row_widget.into()
}

fn total_row(app: &AppModel) -> Element<'_, Message> {
    let now = Local::now();
    let day = state::cutover_date(now, app.config.cutover_hour());
    let total = app
        .state
        .timers
        .iter()
        .fold(chrono::Duration::zero(), |a, t| {
            a + state::sum_for_date(t, day, app.config.cutover_hour(), now)
        });

    Row::new()
        .align_y(Alignment::Center)
        .spacing(6)
        .push(text::body("Total today").width(Length::Fill))
        .push(text::body(fmt_duration_hms(total)).font(cosmic::font::bold()))
        .into()
}

// Footer-action glyphs. Distinct from RESET_ICON / EDIT_ICON / PLAY /
// PAUSE_ICON used in timer rows.
const FOOTER_EXPORT: &str = "\u{2B07}";  // ⬇
const FOOTER_ADD: &str = "+";
const FOOTER_SETTINGS: &str = "\u{2699}"; // ⚙
const FOOTER_REFRESH: &str = "\u{27F3}"; // ⟳ (distinct from ↻ used for Reset)

fn footer_row(app: &AppModel) -> Element<'_, Message> {
    let mut row = Row::new().spacing(6).align_y(Alignment::Center);

    let export_press = app.taxi.available.then_some(Message::OpenExport);
    row = row.push(icon_button(FOOTER_EXPORT, "Export…", export_press));

    let pause_press = app
        .state
        .running_timer()
        .is_some()
        .then_some(Message::Pause);
    row = row.push(icon_button(PAUSE_ICON, "Pause running timer", pause_press));

    row = row.push(icon_button(
        FOOTER_ADD,
        "Add timer",
        Some(Message::BeginAdd),
    ));

    row = row.push(icon_button(
        FOOTER_SETTINGS,
        "Settings",
        Some(Message::OpenSettings),
    ));

    if app.taxi.available {
        row = row.push(icon_button(
            FOOTER_REFRESH,
            "Refresh aliases",
            Some(Message::RefreshAliases),
        ));
    }

    row.into()
}

fn add_row(app: &AppModel) -> Element<'_, Message> {
    let input = text_input("alias", &app.add_buf.alias)
        .on_input(Message::AddBufAlias)
        .on_submit(|_| Message::ConfirmAdd)
        .width(Length::Fill);

    let suggestions = app.alias_suggestions(&app.add_buf.alias);

    let mut sc = Column::new().spacing(2);
    for s in suggestions {
        sc = sc.push(suggestion_row(s, Message::AddBufAliasPick));
    }
    let dropdown = if app.add_buf.alias.is_empty() || app.add_buf.alias_picked {
        Element::from(text::caption(""))
    } else {
        Element::from(container(scrollable(sc).height(Length::Fixed(180.0))))
    };

    let row = Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(input)
        .push(button::suggested("Add").on_press(Message::ConfirmAdd))
        .push(button::standard("Cancel").on_press(Message::CancelAdd));

    Column::new()
        .spacing(4)
        .padding(4)
        .push(row)
        .push(dropdown)
        .into()
}

