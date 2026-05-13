use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, Local, NaiveDate, Timelike};

use crate::state::{Session, cutover_date};

/// A merged + rounded export-ready segment. Differs from `Session` in:
/// - `end` is always present (open sessions are filtered out by `merge`)
/// - `description` may be a `" / "`-joined concatenation
/// - `original` is `Some((pre-rounded-start, pre-rounded-end))` when
///   `quantize_grid` moved either bound; otherwise `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub description: String,
    pub original: Option<(DateTime<Local>, DateTime<Local>)>,
}

impl Span {
    pub fn duration(&self) -> Duration {
        self.end - self.start
    }

    /// True iff the rounded span ends on a different calendar day than it
    /// starts (e.g. 23:50 → 00:15 next day). The line-emitter switches to
    /// a duration format for these so the wall-clock `00:15` isn't
    /// misleading.
    pub fn crosses_midnight(&self) -> bool {
        self.start.date_naive() != self.end.date_naive()
    }
}

/// Group **closed** sessions by their cut-over-shifted date.
pub fn group_by_date(
    sessions: &[Session],
    cutover_hour: u8,
) -> BTreeMap<NaiveDate, Vec<Session>> {
    let mut out: BTreeMap<NaiveDate, Vec<Session>> = BTreeMap::new();
    for s in sessions {
        if s.end.is_none() {
            continue;
        }
        let d = cutover_date(s.start, cutover_hour);
        out.entry(d).or_default().push(s.clone());
    }
    out
}

/// Sort by start; merge adjacent sessions whose gap < `gap`. Descriptions
/// are joined with " / " (deduped, in chronological order, empties dropped).
pub fn merge(mut sessions: Vec<Session>, gap: Duration) -> Vec<Span> {
    sessions.retain(|s| s.end.is_some());
    sessions.sort_by_key(|s| s.start);

    let mut out: Vec<Span> = Vec::new();
    for s in sessions {
        let end = s.end.expect("filtered above");
        let desc = s.description.clone();
        if let Some(last) = out.last_mut() {
            let cur_gap = s.start - last.end;
            if cur_gap < gap {
                last.end = end;
                merge_description(&mut last.description, &desc);
                continue;
            }
        }
        out.push(Span {
            start: s.start,
            end,
            description: desc,
            original: None,
        });
    }
    out
}

fn merge_description(dest: &mut String, addition: &str) {
    let addition = addition.trim();
    if addition.is_empty() {
        return;
    }
    if dest.split(" / ").any(|piece| piece == addition) {
        return;
    }
    if dest.is_empty() {
        dest.push_str(addition);
    } else {
        dest.push_str(" / ");
        dest.push_str(addition);
    }
}

/// Snap each span to a `grid_minutes` boundary using the asymmetric rule:
///
/// - **Start** rounds to the **nearest** grid step with a midpoint
///   threshold biased toward DOWN (offset ≤ ceil(grid/2) → DOWN, else UP).
///   For grid=15 that's threshold 8: 09:08→09:00, 09:09→09:15.
/// - **End** always rounds UP (ceil) to the next grid step, so the
///   exported activity is never shorter than recorded.
///
/// Zero-duration spans (start==end) are dropped at the top — those are
/// either misclicks (handled by the aggregate-or-drop rule above this
/// function) or were filtered before calling `quantize_grid`.
///
/// Belt-and-braces: if rounding collapses the span (`new_end ≤ new_start`),
/// `new_end` is bumped to `new_start + grid_minutes` so every emitted span
/// has at least one grid unit of duration.
pub fn quantize_grid(spans: Vec<Span>, grid_minutes: u32) -> Vec<Span> {
    spans
        .into_iter()
        .filter_map(|sp| {
            if sp.start == sp.end {
                return None;
            }
            let new_start = round_start(sp.start, grid_minutes);
            let mut new_end = round_end(sp.end, grid_minutes);
            if new_end <= new_start {
                new_end = new_start + Duration::minutes(i64::from(grid_minutes));
            }
            // Compare to the truncated-to-minute originals: rounding
            // operates at minute precision, so a sub-minute-only change
            // (e.g. 22:30:05 → 22:30:00) is not a real round and
            // shouldn't trigger a `# original …` comment line.
            let original = if new_start != truncate_to_minute(sp.start)
                || new_end != truncate_to_minute(sp.end)
            {
                Some((sp.start, sp.end))
            } else {
                None
            };
            Some(Span {
                start: new_start,
                end: new_end,
                description: sp.description,
                original,
            })
        })
        .collect()
}

/// Round `t` to the nearest grid step, biased toward DOWN. Truncates
/// sub-minute precision before applying the rule, so `09:08:30` is treated
/// like `09:08`.
fn round_start(t: DateTime<Local>, grid: u32) -> DateTime<Local> {
    let trunc = truncate_to_minute(t);
    let minute = trunc.minute();
    let offset = minute % grid;
    let threshold = grid.div_ceil(2);
    if offset <= threshold {
        trunc - Duration::minutes(i64::from(offset))
    } else {
        trunc + Duration::minutes(i64::from(grid - offset))
    }
}

/// Round `t` UP to the next grid step at minute precision. Sub-minute
/// precision is dropped: `22:30:05` rounds to `22:30`, not `22:45`. Only
/// when the truncated minute itself is off-grid do we push to the next
/// grid step.
fn round_end(t: DateTime<Local>, grid: u32) -> DateTime<Local> {
    let trunc = truncate_to_minute(t);
    let offset = trunc.minute() % grid;
    if offset == 0 {
        return trunc;
    }
    trunc + Duration::minutes(i64::from(grid - offset))
}

fn truncate_to_minute(t: DateTime<Local>) -> DateTime<Local> {
    t.with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .expect("zero is always a valid second/nanosecond")
}

/// Split closed spans into "essentially zero" and "real duration" buckets.
/// A span is considered zero when its duration is **less than one minute** —
/// covers both exact `start == end` (rare in practice) and the common
/// "clicked start then pause within the same minute" case where the
/// underlying timestamps differ only by seconds.
///
/// Spans below the threshold get aggregated (count > 3) or dropped
/// (count ≤ 3); without this, sub-minute sessions would survive
/// `quantize_grid` because the belt-and-braces bumps any collapsed-by-
/// rounding span up to one full grid unit, producing a 15-min entry for
/// what was 30 seconds of work.
pub fn split_zero_duration(spans: Vec<Span>) -> (Vec<Span>, Vec<Span>) {
    spans
        .into_iter()
        .partition(|s| s.duration() < Duration::minutes(1))
}

/// One aggregated entry replacing many zero-duration "misclicks" for a
/// single timer on a single day. Emitted as a duration-format line (no
/// wall-clock anchor) because the source sessions have no meaningful
/// times.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZeroAggregate {
    pub count: usize,
    /// Deduped " / "-joined descriptions of the source zero-duration spans.
    /// Empty if all source descriptions were empty.
    pub description: String,
}

/// Build an aggregate when there are *more than three* zero-duration spans
/// for one timer on one day. ≤3 → returns `None` (caller drops them).
pub fn aggregate_zero(zeros: &[Span]) -> Option<ZeroAggregate> {
    if zeros.len() <= 3 {
        return None;
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut joined = String::new();
    for z in zeros {
        let d = z.description.trim();
        if d.is_empty() {
            continue;
        }
        if !seen.insert(d.to_owned()) {
            continue;
        }
        if !joined.is_empty() {
            joined.push_str(" / ");
        }
        joined.push_str(d);
    }
    Some(ZeroAggregate {
        count: zeros.len(),
        description: joined,
    })
}

/// Format a `Duration` as decimal hours (taxi-compatible). `0.25`, `0.5`,
/// `1`, `1.25`, `1.5` — trailing zeros and stray decimal points trimmed.
pub fn format_duration_hours(d: Duration) -> String {
    let minutes = d.num_minutes().max(0);
    #[allow(clippy::cast_precision_loss)]
    let hours = minutes as f64 / 60.0;
    let formatted = format!("{hours:.2}");
    let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Emit taxi lines for one timer's quantized spans on one date.
///
/// For each `Span`:
/// - If `span.original` is `Some`, prepend `# original HH:MM-HH:MM` so the
///   user can see at a glance what was rounded.
/// - If the rounded span crosses midnight, the entry line uses
///   **duration format** (`alias 0.5 description`); otherwise the standard
///   time-range form (`alias HH:MM-HH:MM description`).
///
/// Description is always the span's own description (carried through from
/// the session); never the alias's project/subtask metadata.
pub fn export_lines(spans: &[Span], alias: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(spans.len() * 2);
    for sp in spans {
        if let Some((orig_start, orig_end)) = sp.original {
            out.push(format!(
                "# original {}-{}",
                orig_start.format("%H:%M"),
                orig_end.format("%H:%M"),
            ));
        }
        let entry = if sp.crosses_midnight() {
            let h = format_duration_hours(sp.duration());
            format_entry(alias, &h, &sp.description)
        } else {
            let s = sp.start.format("%H:%M").to_string();
            let e = sp.end.format("%H:%M").to_string();
            let range = format!("{s}-{e}");
            format_entry(alias, &range, &sp.description)
        };
        out.push(entry);
    }
    out
}

/// Emit the two lines for an aggregated zero-duration block:
///
/// ```text
/// # 5 zero-duration sessions consolidated into 15 min
/// _alias 0.25 desc1 / desc2 / desc3
/// ```
pub fn aggregate_lines(agg: &ZeroAggregate, alias: &str, grid_minutes: u32) -> Vec<String> {
    let comment = format!(
        "# {} zero-duration session{} consolidated into {} min",
        agg.count,
        if agg.count == 1 { "" } else { "s" },
        grid_minutes,
    );
    let h = format_duration_hours(Duration::minutes(i64::from(grid_minutes)));
    vec![comment, format_entry(alias, &h, &agg.description)]
}

fn format_entry(alias: &str, value: &str, description: &str) -> String {
    if description.is_empty() {
        format!("{alias} {value}")
    } else {
        format!("{alias} {value} {description}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn s(start: (u32, u32), end: Option<(u32, u32)>, desc: &str) -> Session {
        Session {
            start: Local.with_ymd_and_hms(2026, 5, 13, start.0, start.1, 0).unwrap(),
            end: end.map(|(h, m)| Local.with_ymd_and_hms(2026, 5, 13, h, m, 0).unwrap()),
            description: desc.into(),
        }
    }

    fn span(start: (u32, u32), end: (u32, u32), desc: &str) -> Span {
        Span {
            start: Local.with_ymd_and_hms(2026, 5, 13, start.0, start.1, 0).unwrap(),
            end: Local.with_ymd_and_hms(2026, 5, 13, end.0, end.1, 0).unwrap(),
            description: desc.into(),
            original: None,
        }
    }

    fn at(h: u32, m: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 5, 13, h, m, 0).unwrap()
    }

    #[test]
    fn merge_empty() {
        assert!(merge(vec![], Duration::minutes(5)).is_empty());
    }

    #[test]
    fn merge_drops_open_sessions() {
        let v = vec![s((9, 0), None, "open"), s((10, 0), Some((10, 30)), "a")];
        let out = merge(v, Duration::minutes(5));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].description, "a");
    }

    #[test]
    fn merge_collapses_short_gap() {
        let v = vec![
            s((9, 0), Some((9, 30)), "a"),
            s((9, 33), Some((10, 0)), "a"),
        ];
        let out = merge(v, Duration::minutes(5));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].start.format("%H:%M").to_string(), "09:00");
        assert_eq!(out[0].end.format("%H:%M").to_string(), "10:00");
        assert_eq!(out[0].description, "a");
    }

    #[test]
    fn merge_keeps_long_gap() {
        let v = vec![
            s((9, 0), Some((9, 30)), "a"),
            s((10, 0), Some((10, 30)), "a"),
        ];
        let out = merge(v, Duration::minutes(5));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn merge_joins_distinct_descriptions() {
        let v = vec![
            s((9, 0), Some((9, 30)), "TICKET-1 Setup"),
            s((9, 32), Some((10, 0)), "TICKET-2 Test"),
            s((10, 1), Some((10, 15)), "TICKET-2 Test"),
        ];
        let out = merge(v, Duration::minutes(5));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].description, "TICKET-1 Setup / TICKET-2 Test");
    }

    #[test]
    fn round_start_nearest_with_threshold_8() {
        // grid=15 → threshold = (15+1)/2 = 8.  offset ≤ 8 → DOWN; > 8 → UP.
        assert_eq!(round_start(at(9, 3), 15), at(9, 0));
        assert_eq!(round_start(at(9, 7), 15), at(9, 0));
        assert_eq!(round_start(at(9, 8), 15), at(9, 0));
        assert_eq!(round_start(at(9, 9), 15), at(9, 15));
        assert_eq!(round_start(at(9, 14), 15), at(9, 15));
        assert_eq!(round_start(at(9, 15), 15), at(9, 15));
        assert_eq!(round_start(at(9, 31), 15), at(9, 30));
        assert_eq!(round_start(at(9, 38), 15), at(9, 30));
        assert_eq!(round_start(at(9, 39), 15), at(9, 45));
    }

    #[test]
    fn round_end_ceils_up() {
        assert_eq!(round_end(at(9, 0), 15), at(9, 0));
        assert_eq!(round_end(at(9, 1), 15), at(9, 15));
        assert_eq!(round_end(at(9, 14), 15), at(9, 15));
        assert_eq!(round_end(at(9, 15), 15), at(9, 15));
        assert_eq!(round_end(at(9, 16), 15), at(9, 30));
        assert_eq!(round_end(at(22, 21), 15), at(22, 30));
    }

    #[test]
    fn round_end_drops_subminute_when_on_grid() {
        // A real session's `end` carries sub-second precision (it's
        // `Local::now()` at the moment of pause). For an end like
        // 22:30:05, the wall-clock minute is already on the grid, so
        // we drop the sub-minute precision and return 22:30 — NOT
        // 22:45.
        let with_seconds = Local
            .with_ymd_and_hms(2026, 5, 13, 22, 30, 5)
            .unwrap();
        assert_eq!(round_end(with_seconds, 15), at(22, 30));
    }

    #[test]
    fn quantize_grid_no_comment_when_only_subminute_changed() {
        // 22:18:30 → 22:30:05 should round to 22:15-22:30 with a comment,
        // but the comment's `# original 22:18-22:30` reflects minute
        // precision. Below we test the simpler case where rounding
        // doesn't change minute-precision values at all.
        let sp = Span {
            start: Local.with_ymd_and_hms(2026, 5, 13, 22, 15, 12).unwrap(),
            end: Local.with_ymd_and_hms(2026, 5, 13, 22, 30, 7).unwrap(),
            description: "x".into(),
            original: None,
        };
        let out = quantize_grid(vec![sp], 15);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].start.format("%H:%M").to_string(), "22:15");
        assert_eq!(out[0].end.format("%H:%M").to_string(), "22:30");
        assert!(
            out[0].original.is_none(),
            "no comment expected when only sub-minute precision differed"
        );
    }

    #[test]
    fn quantize_grid_users_reported_bug() {
        // 22:18-22:30 (the user's exact example): start should round
        // DOWN to 22:15; end is already on the grid → stays at 22:30
        // (NOT 22:45).
        let sp = span((22, 18), (22, 30), "aaa");
        let out = quantize_grid(vec![sp], 15);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].start.format("%H:%M").to_string(), "22:15");
        assert_eq!(out[0].end.format("%H:%M").to_string(), "22:30");
        let (os, oe) = out[0].original.expect("start moved → original recorded");
        assert_eq!(os.format("%H:%M").to_string(), "22:18");
        assert_eq!(oe.format("%H:%M").to_string(), "22:30");
    }

    #[test]
    fn quantize_grid_drops_zero_duration() {
        let v = vec![span((9, 0), (9, 0), "zero")];
        let out = quantize_grid(v, 15);
        assert!(out.is_empty());
    }

    #[test]
    fn split_zero_duration_drops_sub_minute_sessions() {
        // User's bug: two sessions like 23:10:05-23:10:42 (≈37s) and
        // 23:25:11-23:25:55 (≈44s) should NOT survive to the entry list.
        let same_minute_a = Span {
            start: Local.with_ymd_and_hms(2026, 5, 13, 23, 10, 5).unwrap(),
            end: Local.with_ymd_and_hms(2026, 5, 13, 23, 10, 42).unwrap(),
            description: "jguzt".into(),
            original: None,
        };
        let same_minute_b = Span {
            start: Local.with_ymd_and_hms(2026, 5, 13, 23, 25, 11).unwrap(),
            end: Local.with_ymd_and_hms(2026, 5, 13, 23, 25, 55).unwrap(),
            description: "jguzt".into(),
            original: None,
        };
        let real_work = span((10, 0), (11, 0), "real");

        let (zeros, nonzero) =
            split_zero_duration(vec![same_minute_a, same_minute_b, real_work]);
        assert_eq!(zeros.len(), 2);
        assert_eq!(nonzero.len(), 1);
        assert_eq!(nonzero[0].description, "real");
    }

    #[test]
    fn split_zero_duration_keeps_full_minute_sessions() {
        // Exactly 1 minute → kept (not zero).
        let one_min = Span {
            start: Local.with_ymd_and_hms(2026, 5, 13, 9, 0, 0).unwrap(),
            end: Local.with_ymd_and_hms(2026, 5, 13, 9, 1, 0).unwrap(),
            description: "x".into(),
            original: None,
        };
        let (zeros, nonzero) = split_zero_duration(vec![one_min]);
        assert!(zeros.is_empty());
        assert_eq!(nonzero.len(), 1);
    }

    #[test]
    fn quantize_grid_belt_and_braces() {
        // 09:11-09:14: start UP to 09:15, end UP to 09:15 → equal,
        // belt-and-braces bumps end to 09:30.
        let v = vec![span((9, 11), (9, 14), "tiny")];
        let out = quantize_grid(v, 15);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].start.format("%H:%M").to_string(), "09:15");
        assert_eq!(out[0].end.format("%H:%M").to_string(), "09:30");
        assert!(out[0].original.is_some());
    }

    #[test]
    fn quantize_grid_records_original_when_moved() {
        let out = quantize_grid(vec![span((22, 3), (22, 21), "x")], 15);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].start.format("%H:%M").to_string(), "22:00");
        assert_eq!(out[0].end.format("%H:%M").to_string(), "22:30");
        let (orig_start, orig_end) = out[0].original.expect("original recorded");
        assert_eq!(orig_start.format("%H:%M").to_string(), "22:03");
        assert_eq!(orig_end.format("%H:%M").to_string(), "22:21");
    }

    #[test]
    fn quantize_grid_no_original_when_aligned() {
        let out = quantize_grid(vec![span((9, 0), (9, 30), "x")], 15);
        assert_eq!(out.len(), 1);
        assert!(out[0].original.is_none());
    }

    #[test]
    fn aggregate_zero_below_threshold_returns_none() {
        let zeros: Vec<Span> = (0..3)
            .map(|_| span((9, 0), (9, 0), "x"))
            .collect();
        assert!(aggregate_zero(&zeros).is_none());
    }

    #[test]
    fn aggregate_zero_above_threshold_dedupes_descriptions() {
        let zeros = vec![
            span((9, 0), (9, 0), "a"),
            span((9, 30), (9, 30), "b"),
            span((10, 0), (10, 0), "a"),
            span((11, 0), (11, 0), "c"),
            span((12, 0), (12, 0), ""),
        ];
        let agg = aggregate_zero(&zeros).expect("> 3 → aggregated");
        assert_eq!(agg.count, 5);
        // BTreeSet iteration is alphabetical
        assert_eq!(agg.description, "a / b / c");
    }

    #[test]
    fn format_duration_hours_examples() {
        assert_eq!(format_duration_hours(Duration::minutes(15)), "0.25");
        assert_eq!(format_duration_hours(Duration::minutes(30)), "0.5");
        assert_eq!(format_duration_hours(Duration::minutes(45)), "0.75");
        assert_eq!(format_duration_hours(Duration::minutes(60)), "1");
        assert_eq!(format_duration_hours(Duration::minutes(75)), "1.25");
        assert_eq!(format_duration_hours(Duration::minutes(90)), "1.5");
    }

    #[test]
    fn export_lines_basic_aligned() {
        let sp = span((9, 0), (10, 0), "TICKET-1 Setup");
        assert_eq!(
            export_lines(&[sp], "_hello"),
            vec!["_hello 09:00-10:00 TICKET-1 Setup"]
        );
    }

    #[test]
    fn export_lines_emits_comment_when_rounded() {
        let mut sp = span((22, 0), (22, 30), "work");
        sp.original = Some((at(22, 3), at(22, 21)));
        let out = export_lines(&[sp], "_hello");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], "# original 22:03-22:21");
        assert_eq!(out[1], "_hello 22:00-22:30 work");
    }

    #[test]
    fn export_lines_no_description() {
        let sp = span((9, 0), (10, 0), "");
        assert_eq!(export_lines(&[sp], "_hello"), vec!["_hello 09:00-10:00"]);
    }

    #[test]
    fn export_lines_duration_format_for_cross_midnight() {
        // 23:45 → next-day 00:15 is built by rounding past midnight.
        let start = Local.with_ymd_and_hms(2026, 5, 13, 23, 45, 0).unwrap();
        let end = Local.with_ymd_and_hms(2026, 5, 14, 0, 15, 0).unwrap();
        let sp = Span {
            start,
            end,
            description: "late work".into(),
            original: Some((
                Local.with_ymd_and_hms(2026, 5, 13, 23, 50, 0).unwrap(),
                Local.with_ymd_and_hms(2026, 5, 14, 0, 10, 0).unwrap(),
            )),
        };
        let out = export_lines(&[sp], "_hello");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], "# original 23:50-00:10");
        assert_eq!(out[1], "_hello 0.5 late work");
    }

    #[test]
    fn aggregate_lines_format() {
        let agg = ZeroAggregate {
            count: 5,
            description: "a / b / c".into(),
        };
        let out = aggregate_lines(&agg, "_alias", 15);
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0],
            "# 5 zero-duration sessions consolidated into 15 min"
        );
        assert_eq!(out[1], "_alias 0.25 a / b / c");
    }

    #[test]
    fn aggregate_lines_singular_when_count_is_one() {
        // Won't occur in practice (threshold is > 3) but test the
        // pluralization helper anyway.
        let agg = ZeroAggregate {
            count: 1,
            description: String::new(),
        };
        let out = aggregate_lines(&agg, "_alias", 15);
        assert_eq!(
            out[0],
            "# 1 zero-duration session consolidated into 15 min"
        );
    }

    #[test]
    fn user_scenario_merge_with_subminute_tail() {
        // The user's reported scenario, end-to-end through the full
        // per-(timer, date) pipeline:
        //
        //   my 22:18-22:18 A      (sub-minute, but gap=0 → merged anyway)
        //   my 22:18-22:29 B
        //   my 22:29-22:29 C      (sub-minute)
        //   my 22:30-22:30 D      (sub-minute)
        //
        // Should collapse into a single entry:
        //
        //   # original 22:18-22:30
        //   my 22:15-22:30 A / B / C / D
        //
        // The merge step folds all four sessions into one span (gaps are
        // 0 min, 0 min, 1 min — all < merge_gap=5). The resulting span is
        // 22:18-22:30 (12 minutes, > 1 min) so it survives
        // split_zero_duration. quantize_grid rounds start 22:18 → 22:15
        // (offset 3 ≤ 8 → DOWN); end 22:30 is already on the grid.
        let bucket = vec![
            s((22, 18), Some((22, 18)), "A"),
            s((22, 18), Some((22, 29)), "B"),
            s((22, 29), Some((22, 29)), "C"),
            s((22, 30), Some((22, 30)), "D"),
        ];
        let merged = merge(bucket, Duration::minutes(5));
        let (zeros, nonzero) = split_zero_duration(merged);
        assert!(
            zeros.is_empty(),
            "merge folds the sub-minute tails before the < 1 min filter \
             runs, so the merged 22:18-22:30 span survives"
        );
        assert_eq!(nonzero.len(), 1);
        let quantized = quantize_grid(nonzero, 15);
        assert_eq!(quantized.len(), 1);
        let lines = export_lines(&quantized, "my");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "# original 22:18-22:30");
        assert_eq!(lines[1], "my 22:15-22:30 A / B / C / D");
    }

    #[test]
    fn group_by_date_assigns_pre_cutover_to_prev_day() {
        let v = vec![s((3, 0), Some((3, 30)), "early"), s((10, 0), Some((11, 0)), "late")];
        let g = group_by_date(&v, 4);
        let keys: Vec<&NaiveDate> = g.keys().collect();
        assert_eq!(keys.len(), 2);
    }
}
