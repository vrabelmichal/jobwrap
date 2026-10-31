//! Filtered and paged job lists.
//!
//! `GET /api/v1/jobs` keeps its original array response; query parameters only
//! narrow it. `GET /api/v1/jobs/query` returns a page envelope with totals and
//! per-state counts for the dashboard.

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use jobwrap_protocol::query::{days_ago, parse_time_bound, MAX_PAGE_SIZE};
use jobwrap_protocol::{JobPage, JobQuery, JobSummary, StateGroup};

use crate::error::ApiError;
use crate::router::RouterState;

/// Page size of the dashboard and of `/api/v1/jobs/query` without `limit`.
pub const DEFAULT_PAGE_SIZE: usize = 50;
/// The dashboard hides jobs that finished longer ago than this by default.
pub const DEFAULT_RECENT_DAYS: u32 = 7;

/// Query-string parameters shared by both list endpoints.
#[derive(Debug, Default, Deserialize)]
pub struct ListParams {
    state: Option<String>,
    q: Option<String>,
    started_after: Option<String>,
    started_before: Option<String>,
    finished_after: Option<String>,
    finished_before: Option<String>,
    hide_finished_older_than_days: Option<String>,
    offset: Option<String>,
    limit: Option<String>,
}

// Numbers are parsed here rather than by the extractor so malformed values
// produce the API's JSON error shape.
fn number<T: std::str::FromStr>(raw: Option<&str>, name: &str) -> Result<Option<T>, ApiError> {
    match raw.map(str::trim) {
        None | Some("") => Ok(None),
        Some(raw) => raw
            .parse()
            .map(Some)
            .map_err(|_| ApiError::bad_request(format!("{name} must be a non-negative integer"))),
    }
}

fn bound(
    raw: Option<&str>,
    name: &str,
    end_of_day: bool,
) -> Result<Option<DateTime<Utc>>, ApiError> {
    match raw.map(str::trim) {
        None | Some("") => Ok(None),
        Some(raw) => parse_time_bound(raw, end_of_day)
            .map(Some)
            .map_err(|error| ApiError::bad_request(format!("{name}: {error}"))),
    }
}

impl ListParams {
    /// Build a query; `default_limit` applies when the client sent none.
    pub fn into_query(
        self,
        now: DateTime<Utc>,
        default_limit: Option<usize>,
    ) -> Result<JobQuery, ApiError> {
        let requested: Option<usize> = number(self.limit.as_deref(), "limit")?;
        let limit = match requested.or(default_limit) {
            Some(0) => return Err(ApiError::bad_request("limit must be at least 1")),
            Some(limit) if limit > MAX_PAGE_SIZE => {
                return Err(ApiError::bad_request(format!(
                    "limit must be at most {MAX_PAGE_SIZE}"
                )))
            }
            other => other,
        };
        let query = JobQuery {
            state: StateGroup::parse(self.state.as_deref().unwrap_or("").trim())
                .map_err(ApiError::bad_request)?,
            text: self.q.filter(|text| !text.trim().is_empty()),
            started_after: bound(self.started_after.as_deref(), "started_after", false)?,
            started_before: bound(self.started_before.as_deref(), "started_before", true)?,
            finished_after: bound(self.finished_after.as_deref(), "finished_after", false)?,
            finished_before: bound(self.finished_before.as_deref(), "finished_before", true)?,
            hide_finished_before: number::<u32>(
                self.hide_finished_older_than_days.as_deref(),
                "hide_finished_older_than_days",
            )?
            .map(|days| days_ago(now, days)),
            offset: number(self.offset.as_deref(), "offset")?.unwrap_or(0),
            limit,
        };
        if let (Some(after), Some(before)) = (query.started_after, query.started_before) {
            if after >= before {
                return Err(ApiError::bad_request(
                    "started_after must be earlier than started_before",
                ));
            }
        }
        if let (Some(after), Some(before)) = (query.finished_after, query.finished_before) {
            if after >= before {
                return Err(ApiError::bad_request(
                    "finished_after must be earlier than finished_before",
                ));
            }
        }
        Ok(query)
    }
}

/// The query the dashboard uses on first load.
pub fn dashboard_default_query(now: DateTime<Utc>) -> JobQuery {
    JobQuery {
        hide_finished_before: Some(days_ago(now, DEFAULT_RECENT_DAYS)),
        limit: Some(DEFAULT_PAGE_SIZE),
        ..JobQuery::default()
    }
}

fn visible_jobs(state: &RouterState, headers: &HeaderMap) -> Vec<JobSummary> {
    let principal = crate::handlers::principal_from_headers(&state.service, headers);
    state.service.list_jobs(&principal)
}

/// `GET /api/v1/jobs`: an array of visible jobs, optionally filtered.
pub async fn list_jobs(
    State(state): State<RouterState>,
    Query(params): Query<ListParams>,
    headers: HeaderMap,
) -> Result<Json<Vec<JobSummary>>, ApiError> {
    let query = params.into_query(Utc::now(), None)?;
    Ok(Json(query.apply(visible_jobs(&state, &headers)).jobs))
}

/// `GET /api/v1/jobs/query`: one page with totals and per-state counts.
pub async fn query_jobs(
    State(state): State<RouterState>,
    Query(params): Query<ListParams>,
    headers: HeaderMap,
) -> Result<Json<JobPage>, ApiError> {
    let query = params.into_query(Utc::now(), Some(DEFAULT_PAGE_SIZE))?;
    Ok(Json(query.apply(visible_jobs(&state, &headers))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        parse_time_bound("2026-10-31T12:00:00Z", false).expect("now")
    }

    #[test]
    fn empty_parameters_leave_the_list_unfiltered() {
        let query = ListParams::default()
            .into_query(now(), None)
            .expect("query");
        assert!(!query.is_filtered());
        assert_eq!(query.limit, None);
    }

    #[test]
    fn parameters_become_a_query() {
        let params = ListParams {
            state: Some("finished".into()),
            q: Some("  ".into()),
            started_after: Some("2026-10-01".into()),
            started_before: Some("2026-10-31".into()),
            hide_finished_older_than_days: Some("7".into()),
            offset: Some("50".into()),
            ..ListParams::default()
        };
        let query = params
            .into_query(now(), Some(DEFAULT_PAGE_SIZE))
            .expect("query");
        assert_eq!(query.state, StateGroup::Finished);
        assert_eq!(query.text, None);
        assert_eq!(
            query.started_before,
            Some(parse_time_bound("2026-11-01", false).expect("bound"))
        );
        assert_eq!(
            query.hide_finished_before,
            Some(parse_time_bound("2026-10-24T12:00:00Z", false).expect("cutoff"))
        );
        assert_eq!((query.offset, query.limit), (50, Some(DEFAULT_PAGE_SIZE)));
    }

    #[test]
    fn invalid_parameters_are_rejected() {
        for params in [
            ListParams {
                state: Some("done".into()),
                ..ListParams::default()
            },
            ListParams {
                finished_after: Some("last week".into()),
                ..ListParams::default()
            },
            ListParams {
                limit: Some("0".into()),
                ..ListParams::default()
            },
            ListParams {
                offset: Some("-1".into()),
                ..ListParams::default()
            },
            ListParams {
                limit: Some((MAX_PAGE_SIZE + 1).to_string()),
                ..ListParams::default()
            },
            ListParams {
                started_after: Some("2026-10-31".into()),
                started_before: Some("2026-10-30".into()),
                ..ListParams::default()
            },
        ] {
            let error = params.into_query(now(), None).expect_err("rejected");
            assert_eq!(error.code, jobwrap_protocol::ApiErrorCode::BadRequest);
        }
    }
}
