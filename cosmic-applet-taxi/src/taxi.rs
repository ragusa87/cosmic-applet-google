use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate, NaiveTime};
use configparser::ini::Ini;
use regex::Regex;
use tokio::process::Command;

use crate::config::Config;

const TAXIRC_RELATIVE: &str = ".config/taxi/taxirc";

#[derive(Debug, Clone)]
pub struct Taxirc {
    pub file_template: String,
    pub date_format: String,
    /// All aliases sourced from any `[<backend>_aliases]` section of the
    /// taxirc — keyed by alias name, value is the raw mapping string
    /// (e.g. `"7/16"`).
    pub aliases: BTreeMap<String, String>,
}

impl Taxirc {
    pub fn default_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(TAXIRC_RELATIVE))
    }

    pub fn resolve_path(config: &Config) -> Option<PathBuf> {
        let raw = config.taxirc_path.trim();
        if raw.is_empty() {
            Self::default_path()
        } else {
            Some(expand_home(raw))
        }
    }
}

pub fn load_taxirc(path: &Path) -> Result<Taxirc> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("read {}", path.display()))?;
    parse_taxirc(&content)
}

pub fn parse_taxirc(content: &str) -> Result<Taxirc> {
    let mut ini = Ini::new();
    ini.read(content.to_owned())
        .map_err(|e| anyhow::anyhow!("invalid INI: {e}"))?;

    let file_template = ini
        .get("taxi", "file")
        .unwrap_or_else(|| "~/zebra/%Y/%m.tks".to_owned());
    let date_format = ini
        .get("taxi", "date_format")
        .unwrap_or_else(|| "%d/%m/%Y".to_owned());

    let mut aliases = BTreeMap::new();
    for section in ini.sections() {
        if !section.ends_with("_aliases") {
            continue;
        }
        if let Some(map) = ini.get_map_ref().get(&section) {
            for (k, v) in map {
                aliases.insert(k.clone(), v.clone().unwrap_or_default());
            }
        }
    }

    Ok(Taxirc {
        file_template,
        date_format,
        aliases,
    })
}

fn expand_home(s: &str) -> PathBuf {
    if let Some(stripped) = s.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(stripped);
    }
    PathBuf::from(s)
}

/// Resolve the timesheet path for a given date, expanding `~`, chrono
/// strftime specifiers (`%Y`, `%m`, `%d`), and environment-style placeholders.
pub fn resolve_tks_path(template: &str, date: NaiveDate) -> PathBuf {
    let formatted = date.format(template).to_string();
    expand_home(&formatted)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TksEntry {
    pub alias: String,
    pub start: Option<NaiveTime>,
    pub end: Option<NaiveTime>,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TksDay {
    pub date: NaiveDate,
    pub entries: Vec<TksEntry>,
}

/// Parse a taxi .tks file. Tolerant: lines that look like neither a date
/// header nor an entry are dropped.
pub fn parse_tks(content: &str, date_format: &str) -> Vec<TksDay> {
    let entry_re = Regex::new(r"^(?P<alias>\S+)\s+(?P<times>\S+)\s*(?P<desc>.*)$")
        .expect("regex compiles");

    let mut days: Vec<TksDay> = Vec::new();
    let mut current: Option<TksDay> = None;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Ok(date) = NaiveDate::parse_from_str(line, date_format) {
            if let Some(d) = current.take() {
                days.push(d);
            }
            current = Some(TksDay {
                date,
                entries: Vec::new(),
            });
            continue;
        }

        let Some(caps) = entry_re.captures(line) else {
            continue;
        };
        let alias = caps["alias"].to_string();
        let times = &caps["times"];
        let desc = caps["desc"].trim().to_string();
        let (start, end) = parse_times(times);

        if let Some(day) = current.as_mut() {
            day.entries.push(TksEntry {
                alias,
                start,
                end,
                description: desc,
            });
        }
    }

    if let Some(d) = current.take() {
        days.push(d);
    }
    days
}

fn parse_times(s: &str) -> (Option<NaiveTime>, Option<NaiveTime>) {
    let Some(idx) = s.find('-') else {
        return (parse_time(s), None);
    };
    let (a, b) = s.split_at(idx);
    let b = &b[1..];
    (parse_time(a), parse_time(b))
}

fn parse_time(s: &str) -> Option<NaiveTime> {
    let s = s.trim();
    if s.is_empty() || s == "?" {
        return None;
    }
    if let Ok(t) = NaiveTime::parse_from_str(s, "%H:%M") {
        return Some(t);
    }
    if s.len() == 4 && s.chars().all(|c| c.is_ascii_digit()) {
        return NaiveTime::parse_from_str(s, "%H%M").ok();
    }
    None
}

/// Replace the section of a `.tks` file corresponding to `date` with
/// fresh body lines. Other dates' content is preserved bit-for-bit.
///
/// Semantics:
/// - `body_lines` are the lines that go under the date header. The caller
///   pre-renders them (entry lines, `# original …` comments, aggregated
///   zero-duration lines, etc.) — `replace_day` itself doesn't know how
///   to format taxi entries, only how to slice/replace file sections.
/// - The "section" of a date is the header line plus every following
///   line until the next date header (or EOF).
/// - If the target date isn't present, a fresh section is appended at
///   the end (with a blank-line separator when the file is non-empty).
/// - Pre-existing entries for the target date are **discarded** — the
///   applet's export is the source of truth for that day. Other days'
///   lines, comments, and blank lines are kept verbatim in order.
/// - Creates parent dirs + file if missing.
/// - Atomic write via tmp + rename.
pub fn replace_day(
    path: &Path,
    date: NaiveDate,
    body_lines: &[String],
    date_format: &str,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }

    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let header = date.format(date_format).to_string();
    let mut new_section: Vec<String> = Vec::with_capacity(body_lines.len() + 1);
    new_section.push(header.clone());
    new_section.extend(body_lines.iter().cloned());

    let lines: Vec<&str> = existing.lines().collect();
    let mut h0: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if NaiveDate::parse_from_str(line.trim(), date_format).ok() == Some(date) {
            h0 = Some(i);
            break;
        }
    }

    let assembled: Vec<String> = if let Some(start) = h0 {
        // Find h1 = index of the next date header after `start`, or
        // lines.len() if none. Strip trailing blank lines from the
        // dropped section so the file stays tidy.
        let mut end = lines.len();
        for (i, line) in lines.iter().enumerate().skip(start + 1) {
            if NaiveDate::parse_from_str(line.trim(), date_format).is_ok() {
                end = i;
                break;
            }
        }
        let mut out: Vec<String> = Vec::with_capacity(lines.len());
        out.extend(lines[..start].iter().map(|s| (*s).to_owned()));
        out.extend(new_section);
        // Ensure a blank line separating from the next section, if any.
        if end < lines.len() {
            out.push(String::new());
            out.extend(lines[end..].iter().map(|s| (*s).to_owned()));
        }
        out
    } else {
        let mut out: Vec<String> = lines.iter().map(|s| (*s).to_owned()).collect();
        if !out.is_empty() {
            // Trailing blank line if the file didn't already have one.
            if !out.last().is_some_and(String::is_empty) {
                out.push(String::new());
            }
        }
        out.extend(new_section);
        out
    };

    let mut payload = assembled.join("\n");
    if !payload.ends_with('\n') {
        payload.push('\n');
    }

    let tmp = path.with_extension("tks.tmp");
    std::fs::write(&tmp, payload.as_bytes())
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// Non-destructive sibling of [`replace_day`]: keep the existing section
/// untouched and append `body_lines` to it, separated by a
/// `# --- appended <ts> ---` marker so re-runs are visually traceable.
///
/// - Empty `body_lines` → no-op (file untouched, `Ok(())`).
/// - Date section absent → identical to `replace_day`'s
///   "append fresh section at EOF" branch (no marker).
/// - Date section present → insert `# --- appended <ts> ---` then the
///   body lines at the end of that date's section, preserving every
///   pre-existing line under the same header.
pub fn append_day(
    path: &Path,
    date: NaiveDate,
    body_lines: &[String],
    date_format: &str,
) -> Result<()> {
    append_day_at(path, date, body_lines, date_format, Local::now())
}

fn append_day_at(
    path: &Path,
    date: NaiveDate,
    body_lines: &[String],
    date_format: &str,
    now: DateTime<Local>,
) -> Result<()> {
    if body_lines.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {}", parent.display()))?;
    }

    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let header = date.format(date_format).to_string();
    let lines: Vec<&str> = existing.lines().collect();
    let mut h0: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if NaiveDate::parse_from_str(line.trim(), date_format).ok() == Some(date) {
            h0 = Some(i);
            break;
        }
    }

    let assembled: Vec<String> = if let Some(start) = h0 {
        let mut end = lines.len();
        for (i, line) in lines.iter().enumerate().skip(start + 1) {
            if NaiveDate::parse_from_str(line.trim(), date_format).is_ok() {
                end = i;
                break;
            }
        }
        // Trim trailing blanks inside the section so the marker hugs the
        // last real entry.
        let mut section_end = end;
        while section_end > start + 1
            && lines[section_end - 1].trim().is_empty()
        {
            section_end -= 1;
        }

        let marker = format!("# --- appended {} ---", now.format("%Y-%m-%d %H:%M"));
        let mut out: Vec<String> = Vec::with_capacity(lines.len() + body_lines.len() + 2);
        out.extend(lines[..section_end].iter().map(|s| (*s).to_owned()));
        out.push(marker);
        out.extend(body_lines.iter().cloned());
        if end < lines.len() {
            out.push(String::new());
            out.extend(lines[end..].iter().map(|s| (*s).to_owned()));
        }
        out
    } else {
        let mut new_section: Vec<String> = Vec::with_capacity(body_lines.len() + 1);
        new_section.push(header);
        new_section.extend(body_lines.iter().cloned());
        let mut out: Vec<String> = lines.iter().map(|s| (*s).to_owned()).collect();
        if !out.is_empty() && !out.last().is_some_and(String::is_empty) {
            out.push(String::new());
        }
        out.extend(new_section);
        out
    };

    let mut payload = assembled.join("\n");
    if !payload.ends_with('\n') {
        payload.push('\n');
    }

    let tmp = path.with_extension("tks.tmp");
    std::fs::write(&tmp, payload.as_bytes())
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

/// uv-gated wrapper around the `taxi` CLI.
#[derive(Debug, Clone)]
pub struct TaxiRunner {
    pub argv: Vec<String>,
    pub available: bool,
}

impl TaxiRunner {
    pub async fn detect(config: &Config) -> Self {
        let argv = config.taxi_argv();
        let uv_ok = which_uv().await;
        Self {
            available: uv_ok && !argv.is_empty(),
            argv,
        }
    }

    pub async fn run(&self, args: &[&str]) -> Result<String> {
        if !self.available {
            anyhow::bail!("taxi runner not available (uv missing or empty command)");
        }
        let (head, tail) = self
            .argv
            .split_first()
            .context("taxi command is empty")?;
        let mut cmd = Command::new(head);
        cmd.args(tail).args(args);
        cmd.stdin(Stdio::null());
        let output = cmd.output().await.context("spawn taxi subprocess")?;
        if !output.status.success() {
            anyhow::bail!(
                "taxi exited with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub async fn alias_list(&self) -> Result<BTreeMap<String, AliasInfo>> {
        let stdout = self.run(&["alias", "list"]).await?;
        Ok(parse_alias_list(&stdout))
    }

    pub async fn update(&self) -> Result<()> {
        self.run(&["update"]).await.map(|_| ())
    }
}

async fn which_uv() -> bool {
    let Ok(output) = Command::new("uv")
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .await
    else {
        return false;
    };
    output.status.success()
}

/// One row from `taxi alias list`. `description` is whatever's between the
/// final parens on the line (typically `Project, Subtask`); empty if the line
/// didn't carry one.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AliasInfo {
    pub mapping: String,
    #[serde(default)]
    pub description: String,
}

/// Parse the output of `taxi alias list`. Tolerant across formats:
///
/// - `[default] alias -> mapping (description)` — current taxi format
/// - `[default] alias -> not mapped` — internal-backend aliases
/// - `alias = mapping`, `alias -> mapping`, `alias : mapping`
/// - `alias mapping` (whitespace-separated, 2 tokens)
///
/// The leading `[backend]` tag is dropped. A trailing parenthesised group is
/// captured into `AliasInfo.description`.
pub fn parse_alias_list(stdout: &str) -> BTreeMap<String, AliasInfo> {
    let backend_re = Regex::new(
        r"^\s*\[(?P<backend>[^\]]+)\]\s+(?P<alias>\S+)\s+->\s+(?P<mapping>.+?)\s*$",
    )
    .expect("regex compiles");
    let sep_re =
        Regex::new(r"^\s*(?P<alias>\S+)\s*(?:=|->|:)\s*(?P<mapping>.+?)\s*$")
            .expect("regex compiles");
    let two_token_re =
        Regex::new(r"^\s*(?P<alias>\S+)\s+(?P<mapping>\S+)\s*$").expect("regex compiles");

    let mut out = BTreeMap::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (alias, raw_mapping) = if let Some(c) = backend_re.captures(line) {
            (c["alias"].to_string(), c["mapping"].to_string())
        } else if let Some(c) = sep_re.captures(line) {
            (c["alias"].to_string(), c["mapping"].to_string())
        } else if let Some(c) = two_token_re.captures(line) {
            (c["alias"].to_string(), c["mapping"].to_string())
        } else {
            continue;
        };
        let (mapping, description) = split_trailing_paren(raw_mapping.trim());
        out.insert(
            alias,
            AliasInfo {
                mapping,
                description,
            },
        );
    }
    out
}

/// Split `"mapping (description)"` into `("mapping", "description")`. If no
/// trailing parens, `description` is empty.
fn split_trailing_paren(s: &str) -> (String, String) {
    let s = s.trim_end();
    if !s.ends_with(')') {
        return (s.to_owned(), String::new());
    }
    let Some(open) = s.rfind('(') else {
        return (s.to_owned(), String::new());
    };
    let before = s[..open].trim_end().to_owned();
    let inside = s[open + 1..s.len() - 1].trim().to_owned();
    (before, inside)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAXIRC_SAMPLE: &str = r"
[taxi]
file = ~/zebra/%Y/%m.tks
date_format = %d/%m/%Y
auto_add = auto
editor = vim

[backends]
default = zebra://token@zebra.example.com
internal = dummy:///

[default_aliases]
_internal = 7/16
_hello = 1984/1328/420

[internal_aliases]
_meet = 999/999
";

    #[test]
    fn parse_taxirc_collects_all_alias_sections() {
        let t = parse_taxirc(TAXIRC_SAMPLE).unwrap();
        assert_eq!(t.file_template, "~/zebra/%Y/%m.tks");
        assert_eq!(t.date_format, "%d/%m/%Y");
        assert!(t.aliases.contains_key("_internal"));
        assert!(t.aliases.contains_key("_hello"));
        assert!(t.aliases.contains_key("_meet"));
    }

    #[test]
    fn resolve_path_expands_template_and_home() {
        let date = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();
        let p = resolve_tks_path("~/zebra/%Y/%m.tks", date);
        let s = p.to_string_lossy();
        assert!(s.ends_with("/zebra/2026/05.tks"), "got {s}");
        assert!(!s.starts_with('~'));
    }

    #[test]
    fn parse_tks_handles_users_format() {
        let raw = "13/05/2026\n\
                   _hello 0800-0900 TICKET-1 Setup\n\
                   _hello 0915-0945 TICKET-2 Test\n\
                   _hello 1000-1015 TICKET-1 Setup\n";
        let days = parse_tks(raw, "%d/%m/%Y");
        assert_eq!(days.len(), 1);
        let day = &days[0];
        assert_eq!(day.date, NaiveDate::from_ymd_opt(2026, 5, 13).unwrap());
        assert_eq!(day.entries.len(), 3);
        assert_eq!(day.entries[0].alias, "_hello");
        assert_eq!(day.entries[0].start, NaiveTime::from_hms_opt(8, 0, 0));
        assert_eq!(day.entries[0].end, NaiveTime::from_hms_opt(9, 0, 0));
        assert_eq!(day.entries[0].description, "TICKET-1 Setup");
        assert_eq!(day.entries[1].description, "TICKET-2 Test");
    }

    #[test]
    fn parse_tks_handles_colon_format() {
        let raw = "23/01/2014\npingpong 09:00-10:00 Play ping-pong\n";
        let days = parse_tks(raw, "%d/%m/%Y");
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].entries[0].alias, "pingpong");
        assert_eq!(
            days[0].entries[0].start,
            NaiveTime::from_hms_opt(9, 0, 0)
        );
    }

    #[test]
    fn parse_tks_handles_question_mark() {
        let raw = "13/05/2026\n_hello ?-? quick note\n";
        let days = parse_tks(raw, "%d/%m/%Y");
        assert_eq!(days[0].entries[0].start, None);
        assert_eq!(days[0].entries[0].end, None);
    }

    #[test]
    fn parse_tks_drops_comments_and_blanks() {
        let raw = "# header\n\n13/05/2026\n# inline\n_a 09:00-10:00 desc\n";
        let days = parse_tks(raw, "%d/%m/%Y");
        assert_eq!(days[0].entries.len(), 1);
    }

    #[test]
    fn parse_tks_separates_multiple_dates() {
        let raw = "12/05/2026\n_a 09:00-10:00 d1\n13/05/2026\n_b 11:00-12:00 d2\n";
        let days = parse_tks(raw, "%d/%m/%Y");
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].entries.len(), 1);
        assert_eq!(days[1].entries.len(), 1);
    }

    #[test]
    fn replace_day_creates_new_file() {
        let tmp = tempdir();
        let path = tmp.join("month.tks");
        let date = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();
        let body = vec!["_hello 08:00-09:00 TICKET-1".to_owned()];
        replace_day(&path, date, &body, "%d/%m/%Y").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("13/05/2026"));
        assert!(content.contains("_hello 08:00-09:00 TICKET-1"));
    }

    #[test]
    fn replace_day_overwrites_existing_section() {
        let tmp = tempdir();
        let path = tmp.join("month.tks");
        std::fs::write(
            &path,
            "13/05/2026\n_old 08:00-09:00 prior\n\n14/05/2026\n_other 10:00-11:00 next\n",
        )
        .unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();
        let body = vec!["_new 09:00-10:00 added".to_owned()];
        replace_day(&path, date, &body, "%d/%m/%Y").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains("_old"));
        assert!(content.contains("_new 09:00-10:00 added"));
        assert!(content.contains("_other 10:00-11:00 next"));
        let new_pos = content.find("_new").unwrap();
        let other_pos = content.find("_other").unwrap();
        assert!(new_pos < other_pos);
    }

    #[test]
    fn replace_day_appends_when_date_absent() {
        let tmp = tempdir();
        let path = tmp.join("month.tks");
        std::fs::write(&path, "14/05/2026\n_other 10:00-11:00 next\n").unwrap();
        let date = NaiveDate::from_ymd_opt(2015, 1, 1).unwrap();
        let body = vec!["_new 09:00-10:00 added".to_owned()];
        replace_day(&path, date, &body, "%d/%m/%Y").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("_other 10:00-11:00 next"));
        assert!(content.contains("01/01/2015"));
        let other_pos = content.find("_other").unwrap();
        let new_pos = content.find("_new").unwrap();
        assert!(other_pos < new_pos);
    }

    #[test]
    fn replace_day_preserves_other_days_when_middle_overwritten() {
        let tmp = tempdir();
        let path = tmp.join("month.tks");
        std::fs::write(
            &path,
            "10/05/2026\n_a 08:00-09:00 first\n\n13/05/2026\n_b 09:00-10:00 middle\n\n14/05/2026\n_c 10:00-11:00 last\n",
        )
        .unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();
        let body = vec!["_replaced 12:00-13:00 x".to_owned()];
        replace_day(&path, date, &body, "%d/%m/%Y").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("_a 08:00-09:00 first"));
        assert!(!content.contains("_b"));
        assert!(content.contains("_replaced 12:00-13:00 x"));
        assert!(content.contains("_c 10:00-11:00 last"));
        let a = content.find("_a").unwrap();
        let r = content.find("_replaced").unwrap();
        let c = content.find("_c").unwrap();
        assert!(a < r && r < c);
    }

    #[test]
    fn replace_day_preserves_body_lines_with_comments() {
        let tmp = tempdir();
        let path = tmp.join("month.tks");
        let date = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();
        let body = vec![
            "# original 09:14-09:21".to_owned(),
            "_hello 09:15-09:30 work".to_owned(),
            "# 5 zero-duration sessions consolidated into 15 min".to_owned(),
            "_hello 0.25 a / b".to_owned(),
        ];
        replace_day(&path, date, &body, "%d/%m/%Y").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        for line in &body {
            assert!(content.contains(line), "missing line: {line}");
        }
    }

    fn fixed_now() -> DateTime<Local> {
        use chrono::TimeZone;
        Local
            .with_ymd_and_hms(2026, 5, 14, 9, 30, 0)
            .single()
            .expect("fixed timestamp")
    }

    const APPEND_MARKER: &str = "# --- appended 2026-05-14 09:30 ---";

    #[test]
    fn append_day_creates_new_file() {
        let tmp = tempdir();
        let path = tmp.join("month.tks");
        let date = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();
        let body = vec!["_hello 08:00-09:00 TICKET-1".to_owned()];
        append_day_at(&path, date, &body, "%d/%m/%Y", fixed_now()).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("13/05/2026"));
        assert!(content.contains("_hello 08:00-09:00 TICKET-1"));
        assert!(
            !content.contains("--- appended"),
            "fresh-section creation must not emit a marker"
        );
    }

    #[test]
    fn append_day_appends_when_date_absent() {
        let tmp = tempdir();
        let path = tmp.join("month.tks");
        std::fs::write(&path, "14/05/2026\n_other 10:00-11:00 next\n").unwrap();
        let date = NaiveDate::from_ymd_opt(2015, 1, 1).unwrap();
        let body = vec!["_new 09:00-10:00 added".to_owned()];
        append_day_at(&path, date, &body, "%d/%m/%Y", fixed_now()).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("_other 10:00-11:00 next"));
        assert!(content.contains("01/01/2015"));
        assert!(
            !content.contains("--- appended"),
            "missing-date append must not emit a marker"
        );
        let other_pos = content.find("_other").unwrap();
        let new_pos = content.find("_new").unwrap();
        assert!(other_pos < new_pos);
    }

    #[test]
    fn append_day_appends_to_existing_section_with_marker() {
        let tmp = tempdir();
        let path = tmp.join("month.tks");
        std::fs::write(
            &path,
            "13/05/2026\n_old 08:00-09:00 prior\n_old 09:15-09:30 still here\n",
        )
        .unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();
        let body = vec!["_new 10:00-11:00 added".to_owned()];
        append_day_at(&path, date, &body, "%d/%m/%Y", fixed_now()).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("_old 08:00-09:00 prior"));
        assert!(content.contains("_old 09:15-09:30 still here"));
        assert!(content.contains(APPEND_MARKER), "marker missing:\n{content}");
        assert!(content.contains("_new 10:00-11:00 added"));

        let prior = content.find("_old 08:00-09:00 prior").unwrap();
        let still = content.find("_old 09:15-09:30 still here").unwrap();
        let marker = content.find(APPEND_MARKER).unwrap();
        let added = content.find("_new 10:00-11:00 added").unwrap();
        assert!(prior < still && still < marker && marker < added);
    }

    #[test]
    fn append_day_preserves_other_days_when_middle_appended() {
        let tmp = tempdir();
        let path = tmp.join("month.tks");
        std::fs::write(
            &path,
            "10/05/2026\n_a 08:00-09:00 first\n\n13/05/2026\n_b 09:00-10:00 middle\n\n14/05/2026\n_c 10:00-11:00 last\n",
        )
        .unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();
        let body = vec!["_added 12:00-13:00 x".to_owned()];
        append_day_at(&path, date, &body, "%d/%m/%Y", fixed_now()).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("_a 08:00-09:00 first"));
        assert!(content.contains("_b 09:00-10:00 middle"));
        assert!(content.contains(APPEND_MARKER));
        assert!(content.contains("_added 12:00-13:00 x"));
        assert!(content.contains("_c 10:00-11:00 last"));
        let a = content.find("_a").unwrap();
        let b = content.find("_b").unwrap();
        let marker = content.find(APPEND_MARKER).unwrap();
        let added = content.find("_added").unwrap();
        let c = content.find("_c").unwrap();
        assert!(a < b && b < marker && marker < added && added < c);
    }

    #[test]
    fn append_day_with_empty_body_is_noop() {
        let tmp = tempdir();
        let path = tmp.join("month.tks");
        let original = "13/05/2026\n_kept 08:00-09:00 ok\n";
        std::fs::write(&path, original).unwrap();
        let date = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();
        append_day_at(&path, date, &[], "%d/%m/%Y", fixed_now()).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, original);
    }

    #[test]
    fn append_day_with_empty_body_does_not_create_file() {
        let tmp = tempdir();
        let path = tmp.join("nonexistent.tks");
        let date = NaiveDate::from_ymd_opt(2026, 5, 13).unwrap();
        append_day_at(&path, date, &[], "%d/%m/%Y", fixed_now()).unwrap();
        assert!(!path.exists(), "no-op append must not create the file");
    }

    fn map<'a>(m: &'a BTreeMap<String, AliasInfo>, k: &str) -> (&'a str, &'a str) {
        let v = m.get(k).expect("alias missing");
        (v.mapping.as_str(), v.description.as_str())
    }

    #[test]
    fn parse_alias_list_handles_equals_and_arrow() {
        let s = "_internal = 7/16\n_hello -> 1/2/3\n_other : 4/5\n# comment\n";
        let m = parse_alias_list(s);
        assert_eq!(map(&m, "_internal"), ("7/16", ""));
        assert_eq!(map(&m, "_hello"), ("1/2/3", ""));
        assert_eq!(map(&m, "_other"), ("4/5", ""));
    }

    #[test]
    fn parse_alias_list_handles_taxi_cli_format() {
        let s = "\
[default] _foo_dev -> 1000/2000 (Some Project, Some Subtask)
[internal] __break -> not mapped
[default] _bar -> 1/2/3
[default] _baz -> 4/5/6 (Internal exploration)
[default] _qux -> 7/16
";
        let m = parse_alias_list(s);
        assert_eq!(map(&m, "_foo_dev"), ("1000/2000", "Some Project, Some Subtask"));
        assert_eq!(map(&m, "__break"), ("not mapped", ""));
        assert_eq!(map(&m, "_bar"), ("1/2/3", ""));
        assert_eq!(map(&m, "_baz"), ("4/5/6", "Internal exploration"));
        assert_eq!(map(&m, "_qux"), ("7/16", ""));
    }

    fn tempdir() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "cosmic-applet-taxi-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }
}
