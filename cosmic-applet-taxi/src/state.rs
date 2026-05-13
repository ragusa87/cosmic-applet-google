use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub start: DateTime<Local>,
    pub end: Option<DateTime<Local>>,
    #[serde(default)]
    pub description: String,
}

impl Session {
    pub fn is_running(&self) -> bool {
        self.end.is_none()
    }

    pub fn duration(&self, now: DateTime<Local>) -> Duration {
        let end = self.end.unwrap_or(now);
        end - self.start
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timer {
    pub id: Uuid,
    pub alias: String,
    #[serde(default)]
    pub default_description: String,
    #[serde(default)]
    pub selected: bool,
    #[serde(default)]
    pub auto_resume: bool,
    #[serde(default)]
    pub sessions: Vec<Session>,
}

impl Timer {
    pub fn new(alias: impl Into<String>, default_description: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            alias: alias.into(),
            default_description: default_description.into(),
            selected: false,
            auto_resume: false,
            sessions: Vec::new(),
        }
    }

    pub fn running_session(&self) -> Option<&Session> {
        self.sessions.last().filter(|s| s.is_running())
    }

    pub fn running_session_mut(&mut self) -> Option<&mut Session> {
        self.sessions.last_mut().filter(|s| s.is_running())
    }

    pub fn is_running(&self) -> bool {
        self.running_session().is_some()
    }

}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    #[serde(default)]
    pub timers: Vec<Timer>,
    #[serde(default)]
    pub suppressed_aliases: Vec<String>,
    #[serde(default)]
    pub total_selected: bool,
    #[serde(default)]
    pub schema_version: u32,
}

const SCHEMA_VERSION: u32 = 1;

impl AppState {
    pub fn state_path() -> Result<PathBuf> {
        let dir = dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .context("could not resolve XDG state dir")?;
        Ok(dir.join("cosmic-applet-taxi").join("state.json"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::state_path()?;
        if !path.exists() {
            return Ok(Self {
                schema_version: SCHEMA_VERSION,
                total_selected: true,
                ..Self::default()
            });
        }
        let bytes = std::fs::read(&path)
            .with_context(|| format!("read {}", path.display()))?;
        let mut state: AppState = serde_json::from_slice(&bytes)
            .with_context(|| format!("parse {}", path.display()))?;
        if state.schema_version == 0 {
            state.schema_version = SCHEMA_VERSION;
        }
        Ok(state)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::state_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        let json = serde_json::to_vec_pretty(self).context("serialize state")?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }

    pub fn find_timer(&self, id: Uuid) -> Option<&Timer> {
        self.timers.iter().find(|t| t.id == id)
    }

    pub fn find_timer_mut(&mut self, id: Uuid) -> Option<&mut Timer> {
        self.timers.iter_mut().find(|t| t.id == id)
    }

    pub fn find_by_alias(&self, alias: &str) -> Option<&Timer> {
        self.timers.iter().find(|t| t.alias == alias)
    }

    pub fn running_timer(&self) -> Option<&Timer> {
        self.timers.iter().find(|t| t.is_running())
    }

    pub fn add_timer(&mut self, alias: String, default_description: String) -> Option<Uuid> {
        if self.timers.iter().any(|t| t.alias == alias) {
            return None;
        }
        self.suppressed_aliases.retain(|a| a != &alias);
        let timer = Timer::new(alias, default_description);
        let id = timer.id;
        self.timers.push(timer);
        Some(id)
    }

    pub fn remove_timer(&mut self, id: Uuid) {
        let Some(idx) = self.timers.iter().position(|t| t.id == id) else {
            return;
        };
        let timer = self.timers.remove(idx);
        if !timer.alias.is_empty() && !self.suppressed_aliases.contains(&timer.alias) {
            self.suppressed_aliases.push(timer.alias);
        }
    }

    pub fn pause_all_running(&mut self, now: DateTime<Local>) {
        for t in &mut self.timers {
            if let Some(s) = t.running_session_mut() {
                s.end = Some(now);
            }
            // Clear `auto_resume` on every iteration, not just on the
            // timer we just closed. Pause is an explicit user action (or
            // a `start_timer` switching tracks) and must cancel any
            // pending "resume me on unlock" intent — otherwise unlocking
            // after a screen-lock-then-manual-pause would silently
            // restart the timer the user thought they had paused.
            t.auto_resume = false;
        }
    }

    pub fn start_timer(&mut self, id: Uuid, now: DateTime<Local>) {
        self.pause_all_running(now);
        let Some(t) = self.find_timer_mut(id) else {
            return;
        };
        let description = t.default_description.clone();
        t.sessions.push(Session {
            start: now,
            end: None,
            description,
        });
        t.auto_resume = false;
    }

    pub fn pause_timer(&mut self, id: Uuid, now: DateTime<Local>) {
        let Some(t) = self.find_timer_mut(id) else {
            return;
        };
        if let Some(s) = t.running_session_mut() {
            s.end = Some(now);
        }
        t.auto_resume = false;
    }

    /// Mark all currently-running timers as auto-resume and pause them.
    /// Used by screen-lock / suspend.
    pub fn auto_pause_all(&mut self, now: DateTime<Local>) {
        for t in &mut self.timers {
            if t.is_running() {
                t.auto_resume = true;
                if let Some(s) = t.running_session_mut() {
                    s.end = Some(now);
                }
            }
        }
    }

    /// Resume the one timer marked `auto_resume` (we only ever set the flag on
    /// the previously running timer, since the single-running invariant holds).
    pub fn auto_resume_one(&mut self, now: DateTime<Local>) {
        let target = self
            .timers
            .iter()
            .find(|t| t.auto_resume)
            .map(|t| t.id);
        if let Some(id) = target {
            self.start_timer(id, now);
        }
    }

    pub fn reset_timer(&mut self, id: Uuid) {
        if let Some(t) = self.find_timer_mut(id) {
            t.sessions.clear();
            t.auto_resume = false;
        }
    }
}

/// Return all sessions whose effective work-date (cut-over-shifted) equals `day`.
pub fn sessions_for_date(
    timer: &Timer,
    day: NaiveDate,
    cutover_hour: u8,
) -> impl Iterator<Item = &Session> {
    timer
        .sessions
        .iter()
        .filter(move |s| cutover_date(s.start, cutover_hour) == day)
}

pub fn sum_for_date(
    timer: &Timer,
    day: NaiveDate,
    cutover_hour: u8,
    now: DateTime<Local>,
) -> Duration {
    sessions_for_date(timer, day, cutover_hour).fold(Duration::zero(), |acc, s| {
        acc + s.duration(now)
    })
}

/// Effective "work date" of an instant, shifted by the cut-over hour.
/// Anything strictly before `cutover_hour:00:00` belongs to the previous date.
pub fn cutover_date(t: DateTime<Local>, cutover_hour: u8) -> NaiveDate {
    let shifted = t - Duration::hours(i64::from(cutover_hour));
    shifted.date_naive()
}

/// Build a Local datetime for `day` at `hour:minute`. Used to anchor parsed
/// time ranges from the .tks file to a specific date.
pub fn datetime_on(day: NaiveDate, hour: u32, minute: u32) -> Option<DateTime<Local>> {
    let naive = day.and_hms_opt(hour, minute, 0)?;
    Local.from_local_datetime(&naive).single()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 5, 13, h, m, 0).unwrap()
    }

    #[test]
    fn cutover_shifts_pre_cutover_into_previous_day() {
        let t1 = Local.with_ymd_and_hms(2026, 5, 13, 3, 0, 0).unwrap();
        let t2 = Local.with_ymd_and_hms(2026, 5, 13, 4, 0, 0).unwrap();
        let t3 = Local.with_ymd_and_hms(2026, 5, 13, 23, 30, 0).unwrap();
        assert_eq!(
            cutover_date(t1, 4),
            NaiveDate::from_ymd_opt(2026, 5, 12).unwrap()
        );
        assert_eq!(
            cutover_date(t2, 4),
            NaiveDate::from_ymd_opt(2026, 5, 13).unwrap()
        );
        assert_eq!(
            cutover_date(t3, 4),
            NaiveDate::from_ymd_opt(2026, 5, 13).unwrap()
        );
    }

    #[test]
    fn start_timer_pauses_others() {
        let mut s = AppState::default();
        let a = s.add_timer("_a".into(), String::new()).unwrap();
        let b = s.add_timer("_b".into(), String::new()).unwrap();
        s.start_timer(a, at(9, 0));
        s.start_timer(b, at(9, 30));
        assert_eq!(s.timers.iter().filter(|t| t.is_running()).count(), 1);
        assert_eq!(s.timers.iter().find(|t| t.id == b).unwrap().sessions.len(), 1);
        assert_eq!(
            s.timers.iter().find(|t| t.id == a).unwrap().sessions[0]
                .end
                .unwrap(),
            at(9, 30)
        );
    }

    #[test]
    fn delete_adds_to_suppressed_aliases() {
        let mut s = AppState::default();
        let a = s.add_timer("_a".into(), String::new()).unwrap();
        s.remove_timer(a);
        assert!(s.suppressed_aliases.contains(&"_a".to_string()));
    }

    #[test]
    fn add_timer_unsuppresses() {
        let mut s = AppState::default();
        s.suppressed_aliases.push("_a".to_owned());
        s.add_timer("_a".into(), String::new());
        assert!(!s.suppressed_aliases.contains(&"_a".to_string()));
    }

    #[test]
    fn add_timer_with_existing_alias_returns_none() {
        let mut s = AppState::default();
        s.add_timer("_a".into(), String::new()).unwrap();
        assert!(s.add_timer("_a".into(), String::new()).is_none());
    }

    #[test]
    fn auto_pause_then_resume_keeps_one_running() {
        let mut s = AppState::default();
        let a = s.add_timer("_a".into(), String::new()).unwrap();
        s.start_timer(a, at(9, 0));
        s.auto_pause_all(at(9, 30));
        assert!(!s.timers[0].is_running());
        assert!(s.timers[0].auto_resume);
        s.auto_resume_one(at(10, 0));
        assert!(s.timers[0].is_running());
        assert!(!s.timers[0].auto_resume);
    }
}
