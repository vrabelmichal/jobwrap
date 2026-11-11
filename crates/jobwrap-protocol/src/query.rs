//! Filtering and paging for job lists.
//!
//! The daemon keeps every visible job summary in memory, so filters are
//! applied to summaries after authorization. The same query drives the HTTP
//! list endpoints and the CLI `list` command.

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use jobwrap_core::JobState;

use super::types::JobSummary;

/// The largest page a client may request from the paged endpoint.
pub const MAX_PAGE_SIZE: usize = 1000;

/// A coarse grouping of job states used by list filters and summary counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateGroup {
    #[default]
    All,
    /// Registering, running, or stopped.
    Active,
    /// Disconnected or lost: the outcome needs a human to look at it.
    Attention,
    /// Exited or signaled.
    Finished,
}

impl StateGroup {
    /// The group a concrete state belongs to (never `All`).
    pub fn of(state: JobState) -> Self {
        match state {
            JobState::Registering | JobState::Running | JobState::Stopped => Self::Active,
            JobState::Disconnected | JobState::Lost => Self::Attention,
            JobState::Exited { .. } | JobState::Signaled { .. } => Self::Finished,
        }
    }

    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "" | "all" => Ok(Self::All),
            "active" => Ok(Self::Active),
            "attention" => Ok(Self::Attention),
            "finished" => Ok(Self::Finished),
            other => Err(format!(
                "unknown state group `{other}`; expected all, active, attention, or finished"
            )),
        }
    }

    fn admits(self, state: JobState) -> bool {
        self == Self::All || self == Self::of(state)
    }
}

/// Filters and a page window over job summaries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobQuery {
    #[serde(default)]
    pub state: StateGroup,
    /// Case-insensitive substring over name, profile, ID, PIDs, and state.
    #[serde(default)]
    pub text: Option<String>,
    /// Substring over the visible Python launch environment kind, name or path.
    #[serde(default)]
    pub environment: Option<String>,
    /// Inclusive lower bound on the start time.
    #[serde(default)]
    pub started_after: Option<DateTime<Utc>>,
    /// Exclusive upper bound on the start time.
    #[serde(default)]
    pub started_before: Option<DateTime<Utc>>,
    /// Inclusive lower bound on the finish time. Excludes unfinished jobs.
    #[serde(default)]
    pub finished_after: Option<DateTime<Utc>>,
    /// Exclusive upper bound on the finish time. Excludes unfinished jobs.
    #[serde(default)]
    pub finished_before: Option<DateTime<Utc>>,
    /// Hide finished jobs that ended before this time. Unlike
    /// `finished_after`, unfinished jobs are always kept.
    #[serde(default)]
    pub hide_finished_before: Option<DateTime<Utc>>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Number of jobs per state group.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateCounts {
    pub all: usize,
    pub active: usize,
    pub attention: usize,
    pub finished: usize,
}

/// One page of a filtered job list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobPage {
    pub jobs: Vec<JobSummary>,
    /// Jobs matching every filter, before paging.
    pub total: usize,
    pub offset: usize,
    pub limit: Option<usize>,
    /// Per-group counts after the time and text filters but before the state
    /// filter, so a dashboard can show every group at once.
    pub counts: StateCounts,
    /// Finished jobs excluded only by `hide_finished_before`.
    pub hidden_finished: usize,
}

/// When a job reached a terminal state, if it has.
///
/// `lost` records may lack a finish time; their start time is the only
/// available bound.
fn finished_time(job: &JobSummary) -> Option<DateTime<Utc>> {
    if !job.state.is_finished() {
        return None;
    }
    Some(job.finished_at.unwrap_or(job.started_at))
}

fn in_range(
    time: DateTime<Utc>,
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
) -> bool {
    after.map_or(true, |after| time >= after) && before.map_or(true, |before| time < before)
}

fn matches_text(job: &JobSummary, needle: &str) -> bool {
    let mut haystack = format!(
        "{} {} {} {}",
        job.display_name,
        job.profile_name,
        job.id,
        job.state.describe()
    );
    for pid in [job.child_pid, job.wrapper_pid].into_iter().flatten() {
        haystack.push(' ');
        haystack.push_str(&pid.to_string());
    }
    if let Some(unit) = &job.service_unit {
        haystack.push(' ');
        haystack.push_str(unit);
    }
    haystack.push(' ');
    haystack.push_str(&job.visibility);
    if let Some(env) = &job.execution.python_environment {
        haystack.push_str(&format!(" {} {} {}", env.kind, env.name, env.path));
    }
    haystack.to_lowercase().contains(needle)
}

impl JobQuery {
    /// Whether any filter that narrows the list is set.
    pub fn is_filtered(&self) -> bool {
        self.environment
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
            || self.state != StateGroup::All
            || self
                .text
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
            || self.started_after.is_some()
            || self.started_before.is_some()
            || self.finished_after.is_some()
            || self.finished_before.is_some()
            || self.hide_finished_before.is_some()
    }

    fn matches_time_and_text(&self, job: &JobSummary) -> bool {
        if let Some(filter) = self
            .environment
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let text = job
                .execution
                .python_environment
                .as_ref()
                .map(|env| format!("{} {} {}", env.kind, env.name, env.path).to_lowercase());
            if !text
                .as_ref()
                .is_some_and(|text| text.contains(&filter.to_lowercase()))
            {
                return false;
            }
        }
        if !in_range(job.started_at, self.started_after, self.started_before) {
            return false;
        }
        if self.finished_after.is_some() || self.finished_before.is_some() {
            match finished_time(job) {
                Some(time) if in_range(time, self.finished_after, self.finished_before) => {}
                _ => return false,
            }
        }
        match self.text.as_deref().map(str::trim) {
            Some(text) if !text.is_empty() => matches_text(job, &text.to_lowercase()),
            _ => true,
        }
    }

    fn hides_as_old(&self, job: &JobSummary) -> bool {
        match (self.hide_finished_before, finished_time(job)) {
            (Some(cutoff), Some(time)) => time < cutoff,
            _ => false,
        }
    }

    /// Filter, count, sort (newest first), and page the given jobs.
    pub fn apply(&self, jobs: Vec<JobSummary>) -> JobPage {
        let mut counts = StateCounts::default();
        let mut hidden_finished = 0;
        let mut matching: Vec<JobSummary> = Vec::new();
        for job in jobs {
            if !self.matches_time_and_text(&job) {
                continue;
            }
            if self.hides_as_old(&job) {
                if self.state.admits(job.state) {
                    hidden_finished += 1;
                }
                continue;
            }
            counts.all += 1;
            match StateGroup::of(job.state) {
                StateGroup::Active => counts.active += 1,
                StateGroup::Attention => counts.attention += 1,
                StateGroup::Finished => counts.finished += 1,
                StateGroup::All => {}
            }
            if self.state.admits(job.state) {
                matching.push(job);
            }
        }
        matching.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.id.cmp(&a.id)));
        let total = matching.len();
        let jobs = matching
            .into_iter()
            .skip(self.offset)
            .take(self.limit.unwrap_or(usize::MAX))
            .collect();
        JobPage {
            jobs,
            total,
            offset: self.offset,
            limit: self.limit,
            counts,
            hidden_finished,
        }
    }
}

/// Parse a time bound: RFC 3339, or a plain `YYYY-MM-DD` date in UTC.
///
/// A plain date names a whole day. As an upper bound (`end_of_day`) it
/// therefore resolves to the start of the following day, so
/// `started_before=2026-10-31` includes jobs started on October 31.
pub fn parse_time_bound(raw: &str, end_of_day: bool) -> Result<DateTime<Utc>, String> {
    let raw = raw.trim();
    if let Ok(time) = DateTime::parse_from_rfc3339(raw) {
        return Ok(time.with_timezone(&Utc));
    }
    let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| {
        format!("invalid time `{raw}`; use RFC 3339 (2026-10-31T12:00:00Z) or YYYY-MM-DD")
    })?;
    let date = if end_of_day {
        date.succ_opt()
            .ok_or_else(|| format!("date `{raw}` is out of range"))?
    } else {
        date
    };
    Ok(date
        .and_hms_opt(0, 0, 0)
        .expect("midnight is a valid time")
        .and_utc())
}

/// The cutoff for hiding finished jobs older than `days`.
pub fn days_ago(now: DateTime<Utc>, days: u32) -> DateTime<Utc> {
    now - Duration::days(i64::from(days))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobwrap_core::{JobId, Signal, TerminalMetadata};

    fn at(raw: &str) -> DateTime<Utc> {
        parse_time_bound(raw, false).expect("time")
    }

    fn job(name: &str, state: JobState, started: &str, finished: Option<&str>) -> JobSummary {
        JobSummary {
            id: JobId::generate().expect("id"),
            display_name: name.to_string(),
            profile_name: "standard".to_string(),
            state,
            owner_uid: 1000,
            wrapper_pid: Some(10),
            child_pid: Some(4242),
            started_at: at(started),
            finished_at: finished.map(at),
            terminal: TerminalMetadata::default(),
            last_sequence: 0,
            output_bytes: 0,
            log_truncated: false,
            visibility: "public".to_string(),
            service_unit: None,
            execution: Default::default(),
        }
    }

    fn fixture() -> Vec<JobSummary> {
        vec![
            job(
                "old-done",
                JobState::Exited { code: 0 },
                "2026-10-01T10:00:00Z",
                Some("2026-10-01T11:00:00Z"),
            ),
            job(
                "old-running",
                JobState::Running,
                "2026-09-01T10:00:00Z",
                None,
            ),
            job(
                "recent-killed",
                JobState::Signaled {
                    signal: Signal::Kill,
                },
                "2026-10-29T10:00:00Z",
                Some("2026-10-30T09:00:00Z"),
            ),
            job(
                "lost-no-finish",
                JobState::Lost,
                "2026-10-02T00:00:00Z",
                None,
            ),
            job(
                "detached-server",
                JobState::Disconnected,
                "2026-10-30T12:00:00Z",
                None,
            ),
        ]
    }

    fn names(page: &JobPage) -> Vec<&str> {
        page.jobs.iter().map(|j| j.display_name.as_str()).collect()
    }

    #[test]
    fn default_query_returns_everything_newest_first() {
        let page = JobQuery::default().apply(fixture());
        assert_eq!(
            names(&page),
            [
                "detached-server",
                "recent-killed",
                "lost-no-finish",
                "old-done",
                "old-running"
            ]
        );
        assert_eq!(page.total, 5);
        assert_eq!(
            page.counts,
            StateCounts {
                all: 5,
                active: 1,
                attention: 2,
                finished: 2
            }
        );
    }

    #[test]
    fn hiding_old_finished_jobs_keeps_unfinished_ones() {
        let query = JobQuery {
            hide_finished_before: Some(days_ago(at("2026-10-31T00:00:00Z"), 7)),
            ..JobQuery::default()
        };
        let page = query.apply(fixture());
        assert_eq!(
            names(&page),
            ["detached-server", "recent-killed", "old-running"]
        );
        // The lost record without a finish time falls back to its start time.
        assert_eq!(page.hidden_finished, 2);
        assert_eq!(page.counts.all, 3);
    }

    #[test]
    fn finish_filters_exclude_unfinished_jobs() {
        let query = JobQuery {
            finished_after: Some(at("2026-10-01T00:00:00Z")),
            finished_before: Some(parse_time_bound("2026-10-01", true).expect("bound")),
            ..JobQuery::default()
        };
        assert_eq!(names(&query.apply(fixture())), ["old-done"]);
    }

    #[test]
    fn start_filters_are_inclusive_then_exclusive() {
        let query = JobQuery {
            started_after: Some(at("2026-10-29T10:00:00Z")),
            started_before: Some(at("2026-10-30T12:00:00Z")),
            ..JobQuery::default()
        };
        assert_eq!(names(&query.apply(fixture())), ["recent-killed"]);
    }

    #[test]
    fn state_filter_does_not_change_group_counts() {
        let query = JobQuery {
            state: StateGroup::Attention,
            ..JobQuery::default()
        };
        let page = query.apply(fixture());
        assert_eq!(names(&page), ["detached-server", "lost-no-finish"]);
        assert_eq!(page.total, 2);
        assert_eq!(page.counts.all, 5);
    }

    #[test]
    fn text_matches_names_pids_and_states() {
        for (text, expected) in [("SERVER", 1), ("terminated by sigkill", 1), ("4242", 5)] {
            let query = JobQuery {
                text: Some(text.to_string()),
                ..JobQuery::default()
            };
            assert_eq!(query.apply(fixture()).total, expected, "{text}");
        }
    }

    #[test]
    fn paging_reports_total_and_window() {
        let query = JobQuery {
            offset: 1,
            limit: Some(2),
            ..JobQuery::default()
        };
        let page = query.apply(fixture());
        assert_eq!(names(&page), ["recent-killed", "lost-no-finish"]);
        assert_eq!(page.total, 5);
        let past_end = JobQuery {
            offset: 10,
            limit: Some(2),
            ..JobQuery::default()
        };
        assert!(past_end.apply(fixture()).jobs.is_empty());
    }

    #[test]
    fn parses_dates_and_timestamps() {
        assert_eq!(
            parse_time_bound("2026-10-31", false).expect("date"),
            at("2026-10-31T00:00:00Z")
        );
        assert_eq!(
            parse_time_bound("2026-10-31", true).expect("date"),
            at("2026-11-01T00:00:00Z")
        );
        assert_eq!(
            parse_time_bound("2026-10-31T12:00:00+02:00", true).expect("rfc3339"),
            at("2026-10-31T10:00:00Z")
        );
        assert!(parse_time_bound("yesterday", false).is_err());
        assert!(StateGroup::parse("done").is_err());
    }
    #[test]
    fn environments_filter_and_search_before_paging() {
        let mut science = job("worker", JobState::Running, "2026-11-01", None);
        science.execution.python_environment = Some(jobwrap_core::execution::PythonEnvironment {
            kind: "uv".into(),
            name: "science".into(),
            path: "/project/.venv".into(),
            interpreter: None,
            evidence: "launch".into(),
        });
        let unknown = job("other", JobState::Running, "2026-11-01", None);
        for query in [
            JobQuery {
                environment: Some("SCIENCE".into()),
                ..Default::default()
            },
            JobQuery {
                text: Some(".venv".into()),
                ..Default::default()
            },
        ] {
            let page = query.apply(vec![science.clone(), unknown.clone()]);
            assert_eq!(page.total, 1);
            assert_eq!(page.jobs[0].id, science.id);
        }
    }
}
