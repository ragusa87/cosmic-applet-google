use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

const EVENTS_URL: &str =
    "https://www.googleapis.com/calendar/v3/calendars/primary/events";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: String,
    pub summary: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub meet_url: Option<String>,
    pub location: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    Cancelled,
    AllDay,
    Free,
    Declined,
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("status=cancelled"),
            Self::AllDay => f.write_str("all-day (no precise start time)"),
            Self::Free => f.write_str("transparency=transparent (Free-marked)"),
            Self::Declined => f.write_str("self responseStatus=declined"),
        }
    }
}

/// One row in the `--debug` report — every event the API returned, plus the
/// verdict the panel applet would apply.
pub struct DebugItem {
    pub id: String,
    pub summary: String,
    pub start_display: String,
    pub end_display: String,
    pub status: Option<String>,
    pub transparency: Option<String>,
    pub self_response: Option<String>,
    pub attendee_count: usize,
    pub meet_url: Option<String>,
    pub location: Option<String>,
    pub verdict: Result<Event, SkipReason>,
}

pub async fn upcoming_events(access_token: &str) -> Result<Vec<Event>> {
    let items = fetch_raw(access_token).await?;
    Ok(filter_and_map(items))
}

pub async fn debug_fetch(access_token: &str) -> Result<Vec<DebugItem>> {
    let items = fetch_raw(access_token).await?;
    Ok(items.into_iter().map(to_debug_item).collect())
}

async fn fetch_raw(access_token: &str) -> Result<Vec<RawEvent>> {
    let now = Utc::now();
    let window_end = now + Duration::hours(24);
    let client = reqwest::Client::new();
    let response = client
        .get(EVENTS_URL)
        .bearer_auth(access_token)
        .query(&[
            ("timeMin", now.to_rfc3339()),
            ("timeMax", window_end.to_rfc3339()),
            ("maxResults", "20".to_owned()),
            ("singleEvents", "true".to_owned()),
            ("orderBy", "startTime".to_owned()),
        ])
        .send()
        .await
        .context("call Calendar events.list")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("calendar events.list returned {status}: {body}");
    }

    let parsed: EventsResponse = response.json().await.context("parse Calendar JSON")?;
    Ok(parsed.items)
}

fn filter_and_map(items: Vec<RawEvent>) -> Vec<Event> {
    let mut events: Vec<Event> = items.into_iter().filter_map(map_event).collect();
    events.sort_by_key(|e| e.start);
    events
}

fn map_event(raw: RawEvent) -> Option<Event> {
    classify(&raw).ok()?;
    Some(build_event(raw))
}

fn classify(raw: &RawEvent) -> Result<DateTime<Utc>, SkipReason> {
    if raw.status.as_deref() == Some("cancelled") {
        return Err(SkipReason::Cancelled);
    }
    if raw.transparency.as_deref() == Some("transparent") {
        return Err(SkipReason::Free);
    }
    if raw
        .attendees
        .iter()
        .flatten()
        .any(|a| a.self_attendee && a.response_status.as_deref() == Some("declined"))
    {
        return Err(SkipReason::Declined);
    }
    // All-day events have `start.date` set, not `start.dateTime`.
    raw.start.date_time.ok_or(SkipReason::AllDay)
}

fn build_event(raw: RawEvent) -> Event {
    let start = raw.start.date_time.unwrap_or_else(Utc::now);
    let end = raw.end.as_ref().and_then(|e| e.date_time).unwrap_or(start);
    let meet_url = extract_meet_url(&raw);
    let location = raw
        .location
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Event {
        id: raw.id,
        summary: raw.summary.unwrap_or_else(|| "(no title)".to_owned()),
        start,
        end,
        meet_url,
        location,
    }
}

fn to_debug_item(raw: RawEvent) -> DebugItem {
    let verdict_result = classify(&raw);
    let self_response = raw
        .attendees
        .iter()
        .flatten()
        .find(|a| a.self_attendee)
        .and_then(|a| a.response_status.clone());
    let attendee_count = raw.attendees.as_ref().map_or(0, Vec::len);
    let id = raw.id.clone();
    let summary = raw
        .summary
        .clone()
        .unwrap_or_else(|| "(no title)".to_owned());
    let start_display = format_event_time(&raw.start);
    let end_display = raw
        .end
        .as_ref()
        .map_or_else(|| "(no end)".to_owned(), format_event_time);
    let status = raw.status.clone();
    let transparency = raw.transparency.clone();
    let meet_url = extract_meet_url(&raw);
    let location = raw
        .location
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let verdict = verdict_result.map(|_| build_event(raw));
    DebugItem {
        id,
        summary,
        start_display,
        end_display,
        status,
        transparency,
        self_response,
        attendee_count,
        meet_url,
        location,
        verdict,
    }
}

fn format_event_time(t: &RawEventTime) -> String {
    if let Some(dt) = t.date_time {
        dt.with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M %Z")
            .to_string()
    } else if let Some(d) = &t.date {
        format!("{d} (all-day)")
    } else {
        "(no time)".to_owned()
    }
}

fn extract_meet_url(raw: &RawEvent) -> Option<String> {
    if let Some(conf) = raw.conference_data.as_ref() {
        for ep in conf.entry_points.iter().flatten() {
            if ep.entry_point_type.as_deref() == Some("video")
                && let Some(uri) = ep.uri.as_ref()
                && uri.starts_with("https://meet.google.com/")
            {
                return Some(uri.clone());
            }
        }
    }
    raw.hangout_link.clone()
}

#[derive(Debug, Deserialize)]
struct EventsResponse {
    #[serde(default)]
    items: Vec<RawEvent>,
}

#[derive(Debug, Deserialize)]
struct RawEvent {
    id: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    transparency: Option<String>,
    start: RawEventTime,
    #[serde(default)]
    end: Option<RawEventTime>,
    #[serde(default, rename = "hangoutLink")]
    hangout_link: Option<String>,
    #[serde(default, rename = "conferenceData")]
    conference_data: Option<RawConferenceData>,
    #[serde(default)]
    attendees: Option<Vec<RawAttendee>>,
    #[serde(default)]
    location: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawEventTime {
    #[serde(default, rename = "dateTime")]
    date_time: Option<DateTime<Utc>>,
    #[serde(default)]
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawConferenceData {
    #[serde(default, rename = "entryPoints")]
    entry_points: Option<Vec<RawEntryPoint>>,
}

#[derive(Debug, Deserialize)]
struct RawEntryPoint {
    #[serde(default, rename = "entryPointType")]
    entry_point_type: Option<String>,
    #[serde(default)]
    uri: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawAttendee {
    #[serde(default, rename = "self")]
    self_attendee: bool,
    #[serde(default, rename = "responseStatus")]
    response_status: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Vec<Event> {
        let resp: EventsResponse = serde_json::from_str(json).unwrap();
        filter_and_map(resp.items)
    }

    #[test]
    fn filters_cancelled_declined_allday_and_free() {
        let json = r#"{
            "items": [
                {
                    "id": "ok1",
                    "summary": "Standup",
                    "status": "confirmed",
                    "start": { "dateTime": "2026-05-12T09:00:00Z" },
                    "end":   { "dateTime": "2026-05-12T09:15:00Z" }
                },
                {
                    "id": "cancelled",
                    "summary": "Cancelled Meeting",
                    "status": "cancelled",
                    "start": { "dateTime": "2026-05-12T10:00:00Z" },
                    "end":   { "dateTime": "2026-05-12T10:30:00Z" }
                },
                {
                    "id": "allday",
                    "summary": "Holiday",
                    "status": "confirmed",
                    "start": { "date": "2026-05-12" },
                    "end":   { "date": "2026-05-13" }
                },
                {
                    "id": "free",
                    "summary": "Lunch",
                    "status": "confirmed",
                    "transparency": "transparent",
                    "start": { "dateTime": "2026-05-12T12:00:00Z" },
                    "end":   { "dateTime": "2026-05-12T13:00:00Z" }
                },
                {
                    "id": "declined",
                    "summary": "Declined",
                    "status": "confirmed",
                    "start": { "dateTime": "2026-05-12T11:00:00Z" },
                    "end":   { "dateTime": "2026-05-12T11:30:00Z" },
                    "attendees": [
                        { "self": true, "responseStatus": "declined" }
                    ]
                },
                {
                    "id": "ok2",
                    "summary": "Design review",
                    "status": "confirmed",
                    "start": { "dateTime": "2026-05-12T14:00:00Z" },
                    "end":   { "dateTime": "2026-05-12T15:00:00Z" }
                }
            ]
        }"#;
        let events = parse(json);
        let ids: Vec<&str> = events.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["ok1", "ok2"]);
    }

    #[test]
    fn extracts_meet_link_from_conference_data() {
        let json = r#"{
            "items": [{
                "id": "x",
                "summary": "X",
                "status": "confirmed",
                "start": { "dateTime": "2026-05-12T09:00:00Z" },
                "end":   { "dateTime": "2026-05-12T09:30:00Z" },
                "conferenceData": {
                    "entryPoints": [
                        { "entryPointType": "phone", "uri": "tel:+15555555" },
                        { "entryPointType": "video", "uri": "https://meet.google.com/abc-defg-hij" }
                    ]
                }
            }]
        }"#;
        let events = parse(json);
        assert_eq!(
            events[0].meet_url.as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
    }

    #[test]
    fn falls_back_to_hangout_link() {
        let json = r#"{
            "items": [{
                "id": "x",
                "summary": "X",
                "status": "confirmed",
                "start": { "dateTime": "2026-05-12T09:00:00Z" },
                "end":   { "dateTime": "2026-05-12T09:30:00Z" },
                "hangoutLink": "https://meet.google.com/legacy-link"
            }]
        }"#;
        let events = parse(json);
        assert_eq!(
            events[0].meet_url.as_deref(),
            Some("https://meet.google.com/legacy-link")
        );
    }

    #[test]
    fn no_meet_link_returns_none() {
        let json = r#"{
            "items": [{
                "id": "x",
                "summary": "Solo work",
                "status": "confirmed",
                "start": { "dateTime": "2026-05-12T09:00:00Z" },
                "end":   { "dateTime": "2026-05-12T09:30:00Z" }
            }]
        }"#;
        let events = parse(json);
        assert!(events[0].meet_url.is_none());
    }
}
