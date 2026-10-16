//! O2: metrics, error tracking and alerts.
//!
//! `GET /metrics` serves Prometheus text: request counts and timings, the
//! job queue, run states, outbox lag, failing webhooks and errors reported
//! by browsers. Counters live in this process (each server is scraped on its
//! own); gauges are read from the database when scraped, so they agree
//! across servers.
//!
//! The endpoint needs `Authorization: Bearer <token>` where the token is the
//! content of the file named by `AGENT_WORKSPACE_METRICS_TOKEN_PATH`. Without
//! that file it answers only in development.
//!
//! [`alert`] is how the server tells an operator something needs them. It
//! goes to the log (target `agent_workspace::alert`, level error) and to any
//! sink installed with [`install_alert_sink`], which is where notifications
//! (N1) plug in once they're on main.

use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use subtle::ConstantTimeEq;

use crate::{AppState, error::AppError};

#[derive(Default)]
struct Registry {
    http: Mutex<BTreeMap<(String, &'static str), Timing>>,
    jobs: Mutex<BTreeMap<(String, &'static str), Timing>>,
    jobs_dead: Mutex<BTreeMap<String, u64>>,
    alerts: Mutex<BTreeMap<&'static str, u64>>,
    browser_errors: Mutex<u64>,
    browser_errors_dropped: Mutex<u64>,
    browser_window: Mutex<Option<(Instant, u32)>>,
    server_errors: Mutex<BTreeMap<&'static str, u64>>,
}

#[derive(Clone, Copy, Default)]
struct Timing {
    count: u64,
    seconds: f64,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(Registry::default)
}

fn bump<K: Ord>(map: &Mutex<BTreeMap<K, u64>>, key: K) {
    if let Ok(mut map) = map.lock() {
        *map.entry(key).or_default() += 1;
    }
}

fn time<K: Ord>(map: &Mutex<BTreeMap<K, Timing>>, key: K, elapsed: Duration) {
    if let Ok(mut map) = map.lock() {
        let timing = map.entry(key).or_default();
        timing.count += 1;
        timing.seconds += elapsed.as_secs_f64();
    }
}

/// Record a finished job attempt.
pub fn observe_job(kind: &str, elapsed: Duration, succeeded: bool) {
    time(
        &registry().jobs,
        (
            kind.to_owned(),
            if succeeded { "succeeded" } else { "failed" },
        ),
        elapsed,
    );
}

/// Record a job that ran out of attempts.
pub fn job_dead(kind: &str) {
    bump(&registry().jobs_dead, kind.to_owned());
}

/// Record a server error by category (`AppError::log_category`).
pub fn server_error(category: &'static str) {
    bump(&registry().server_errors, category);
}

/// Middleware: count and time every request by route class and status.
pub async fn track_requests(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let class = route_class(request.uri().path());
    let response = next.run(request).await;
    let status = match response.status().as_u16() {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        _ => "5xx",
    };
    time(
        &registry().http,
        (class.to_owned(), status),
        started.elapsed(),
    );
    response
}

/// A low-cardinality name for a path: the first two segments after
/// `/api/v1`, so ids never become label values.
fn route_class(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("/api/v1/") {
        let mut segments = rest.split('/').filter(|segment| !segment.is_empty());
        let first = segments.next().unwrap_or("root");
        let second = segments
            .nth(1)
            .filter(|segment| segment.chars().all(|c| c.is_ascii_lowercase() || c == '-'));
        let first = if first.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
            first
        } else {
            "other"
        };
        return match second {
            Some(second) => format!("api/{first}/{second}"),
            None => format!("api/{first}"),
        };
    }
    match path.split('/').nth(1) {
        Some("health") => "health".to_owned(),
        Some("metrics") => "metrics".to_owned(),
        Some("mcp" | "oauth" | ".well-known") => "mcp".to_owned(),
        _ => "web".to_owned(),
    }
}

/// Something an operator should know about.
#[derive(Clone, Debug)]
pub struct Alert {
    /// What kind of problem, such as `job_dead` or `outbox_lag`.
    pub key: &'static str,
    /// What it's about, such as the job kind. With `key`, it decides when a
    /// repeat is the same alert.
    pub subject: String,
    /// One or two plain sentences for a person.
    pub detail: String,
}

/// Where alerts go besides the log. Notifications (N1) install one.
pub trait AlertSink: Send + Sync {
    fn send(&self, alert: &Alert);
}

fn sinks() -> &'static Mutex<Vec<Arc<dyn AlertSink>>> {
    static SINKS: OnceLock<Mutex<Vec<Arc<dyn AlertSink>>>> = OnceLock::new();
    SINKS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn install_alert_sink(sink: Arc<dyn AlertSink>) {
    if let Ok(mut sinks) = sinks().lock() {
        sinks.push(sink);
    }
}

/// Raise an alert. The same key and subject alerts at most once an hour.
pub fn alert(alert: Alert) {
    static RECENT: OnceLock<Mutex<HashMap<(&'static str, String), Instant>>> = OnceLock::new();
    let recent = RECENT.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut recent) = recent.lock() {
        let key = (alert.key, alert.subject.clone());
        if recent
            .get(&key)
            .is_some_and(|at| at.elapsed() < Duration::from_secs(3600))
        {
            return;
        }
        recent.retain(|_, at| at.elapsed() < Duration::from_secs(3600));
        recent.insert(key, Instant::now());
    }
    bump(&registry().alerts, alert.key);
    tracing::error!(
        target: "agent_workspace::alert",
        alert = alert.key,
        subject = alert.subject,
        detail = alert.detail,
        "operator alert"
    );
    let sinks = sinks()
        .lock()
        .map(|sinks| sinks.clone())
        .unwrap_or_default();
    for sink in sinks {
        sink.send(&alert);
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/metrics", get(serve_metrics))
        .route("/api/v1/client-errors", post(report_client_error))
}

async fn serve_metrics(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !metrics_authorized(&state, &headers) {
        return AppError::NotFound("metrics").into_response();
    }
    match render(&state).await {
        Ok(body) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
            body,
        )
            .into_response(),
        Err(error) => error.into_response(),
    }
}

fn metrics_authorized(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(path) = &state.config.metrics_token_path else {
        return state.config.environment.is_development();
    };
    let Ok(expected) = std::fs::read_to_string(path) else {
        return false;
    };
    let expected = expected.trim();
    let Some(given) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    !expected.is_empty() && bool::from(given.as_bytes().ct_eq(expected.as_bytes()))
}

#[derive(sqlx::FromRow)]
struct DatabaseGauges {
    outbox_pending: i64,
    outbox_lag_seconds: f64,
    outbox_failed: i64,
    webhooks_failing: i64,
    github_jobs_failing: i64,
}

async fn render(state: &AppState) -> Result<String, AppError> {
    let mut out = String::new();
    let registry = registry();

    out.push_str("# HELP aw_http_requests_total HTTP requests by route class and status.\n");
    out.push_str("# TYPE aw_http_requests_total counter\n");
    let http = registry
        .http
        .lock()
        .map(|map| map.clone())
        .unwrap_or_default();
    for ((class, status), timing) in &http {
        let _ = writeln!(
            out,
            "aw_http_requests_total{{route=\"{class}\",status=\"{status}\"}} {}",
            timing.count
        );
    }
    out.push_str("# TYPE aw_http_request_seconds_sum counter\n");
    for ((class, status), timing) in &http {
        let _ = writeln!(
            out,
            "aw_http_request_seconds_sum{{route=\"{class}\",status=\"{status}\"}} {:.6}",
            timing.seconds
        );
    }

    out.push_str("# HELP aw_job_attempts_total Finished job attempts by kind and outcome.\n");
    out.push_str("# TYPE aw_job_attempts_total counter\n");
    let jobs = registry
        .jobs
        .lock()
        .map(|map| map.clone())
        .unwrap_or_default();
    for ((kind, outcome), timing) in &jobs {
        let _ = writeln!(
            out,
            "aw_job_attempts_total{{kind=\"{kind}\",outcome=\"{outcome}\"}} {}",
            timing.count
        );
        let _ = writeln!(
            out,
            "aw_job_seconds_sum{{kind=\"{kind}\",outcome=\"{outcome}\"}} {:.6}",
            timing.seconds
        );
    }
    out.push_str("# TYPE aw_jobs_dead_total counter\n");
    for (kind, count) in registry
        .jobs_dead
        .lock()
        .map(|map| map.clone())
        .unwrap_or_default()
    {
        let _ = writeln!(out, "aw_jobs_dead_total{{kind=\"{kind}\"}} {count}");
    }
    out.push_str("# TYPE aw_alerts_total counter\n");
    for (key, count) in registry
        .alerts
        .lock()
        .map(|map| map.clone())
        .unwrap_or_default()
    {
        let _ = writeln!(out, "aw_alerts_total{{alert=\"{key}\"}} {count}");
    }
    out.push_str("# TYPE aw_server_errors_total counter\n");
    for (category, count) in registry
        .server_errors
        .lock()
        .map(|map| map.clone())
        .unwrap_or_default()
    {
        let _ = writeln!(
            out,
            "aw_server_errors_total{{category=\"{category}\"}} {count}"
        );
    }
    out.push_str("# TYPE aw_browser_errors_dropped_total counter\n");
    let _ = writeln!(
        out,
        "aw_browser_errors_dropped_total {}",
        registry
            .browser_errors_dropped
            .lock()
            .map(|count| *count)
            .unwrap_or(0)
    );
    out.push_str("# TYPE aw_browser_errors_total counter\n");
    let _ = writeln!(
        out,
        "aw_browser_errors_total {}",
        registry
            .browser_errors
            .lock()
            .map(|count| *count)
            .unwrap_or(0)
    );

    let queue = crate::jobs::queue_stats(&state.pool).await?;
    out.push_str("# HELP aw_job_queue Jobs on the queue by state.\n");
    out.push_str("# TYPE aw_job_queue gauge\n");
    let _ = writeln!(out, "aw_job_queue{{state=\"due\"}} {}", queue.queued_due);
    let _ = writeln!(
        out,
        "aw_job_queue{{state=\"later\"}} {}",
        queue.queued_later
    );
    let _ = writeln!(out, "aw_job_queue{{state=\"running\"}} {}", queue.running);
    let _ = writeln!(
        out,
        "aw_job_queue{{state=\"dead_last_day\"}} {}",
        queue.dead_last_day
    );
    out.push_str("# TYPE aw_job_oldest_due_seconds gauge\n");
    let _ = writeln!(
        out,
        "aw_job_oldest_due_seconds {:.3}",
        queue.oldest_due_seconds
    );

    let runs = sqlx::query_as::<_, (String, i64)>(
        r#"
        SELECT state, count(*) FROM agent_runs
        WHERE state IN ('created','queued','starting','working','blocked','recovering')
           OR updated_at > now() - interval '1 day'
        GROUP BY state ORDER BY state
        "#,
    )
    .fetch_all(&state.pool)
    .await?;
    out.push_str("# HELP aw_agent_runs Runs that are live or changed in the last day, by state.\n");
    out.push_str("# TYPE aw_agent_runs gauge\n");
    for (run_state, count) in runs {
        let _ = writeln!(out, "aw_agent_runs{{state=\"{run_state}\"}} {count}");
    }

    let gauges = sqlx::query_as::<_, DatabaseGauges>(
        r#"
        SELECT
            (SELECT count(*) FROM outbox_events
             WHERE published_at IS NULL AND failed_at IS NULL) AS outbox_pending,
            COALESCE((SELECT EXTRACT(EPOCH FROM now() - min(created_at)) FROM outbox_events
             WHERE published_at IS NULL AND failed_at IS NULL), 0)::float8 AS outbox_lag_seconds,
            (SELECT count(*) FROM outbox_events
             WHERE failed_at > now() - interval '1 day') AS outbox_failed,
            (SELECT count(*) FROM agent_webhook_deliveries
             WHERE delivered_at IS NULL AND attempts > 0) AS webhooks_failing,
            (SELECT count(*) FROM github_jobs
             WHERE state IN ('failed', 'exhausted')
               AND updated_at > now() - interval '1 day') AS github_jobs_failing
        "#,
    )
    .fetch_one(&state.pool)
    .await?;
    out.push_str("# TYPE aw_outbox_pending gauge\n");
    let _ = writeln!(out, "aw_outbox_pending {}", gauges.outbox_pending);
    out.push_str("# TYPE aw_outbox_lag_seconds gauge\n");
    let _ = writeln!(
        out,
        "aw_outbox_lag_seconds {:.3}",
        gauges.outbox_lag_seconds
    );
    out.push_str("# TYPE aw_outbox_failed_last_day gauge\n");
    let _ = writeln!(out, "aw_outbox_failed_last_day {}", gauges.outbox_failed);
    out.push_str("# TYPE aw_agent_webhooks_failing gauge\n");
    let _ = writeln!(out, "aw_agent_webhooks_failing {}", gauges.webhooks_failing);
    out.push_str("# TYPE aw_github_jobs_failing_last_day gauge\n");
    let _ = writeln!(
        out,
        "aw_github_jobs_failing_last_day {}",
        gauges.github_jobs_failing
    );
    // AI spend joins here once the model gateway (AI1) records usage.
    Ok(out)
}

/// What a browser sends when the app hits an error it didn't handle.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientErrorReport {
    message: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    route: Option<String>,
    #[serde(default)]
    release: Option<String>,
}

/// Browser error reports the whole server logs per minute. The endpoint is
/// open to signed-out pages and an `Origin` header can be forged, so this
/// caps what anyone can write to the logs through it.
const BROWSER_REPORTS_PER_MINUTE: u32 = 60;

/// Whether one more browser report fits this minute's budget, else how long
/// until the next minute starts.
fn take_browser_report_slot(now: Instant) -> Result<(), u64> {
    let Ok(mut window) = registry().browser_window.lock() else {
        return Err(60);
    };
    match window.as_mut() {
        Some((started, used)) if now.duration_since(*started) < Duration::from_secs(60) => {
            if *used >= BROWSER_REPORTS_PER_MINUTE {
                let left = Duration::from_secs(60).saturating_sub(now.duration_since(*started));
                return Err(left.as_secs().max(1));
            }
            *used += 1;
        }
        _ => *window = Some((now, 1)),
    }
    Ok(())
}

/// Browser error reports: counted and logged, bounded and stripped of
/// query strings, never stored. Open to signed-out pages too, so it is
/// limited to allowed origins, small bodies and a server-wide budget.
async fn report_client_error(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<StatusCode, AppError> {
    crate::auth::require_allowed_origin(&state.config, &headers)?;
    if body.len() > 8 * 1024 {
        return Err(AppError::PayloadTooLarge);
    }
    if let Err(retry_after_seconds) = take_browser_report_slot(Instant::now()) {
        if let Ok(mut dropped) = registry().browser_errors_dropped.lock() {
            *dropped += 1;
        }
        return Err(AppError::RateLimited {
            retry_after_seconds,
        });
    }
    let report = serde_json::from_slice::<ClientErrorReport>(&body)
        .map_err(|_| AppError::Invalid("JSON request body"))?;
    if let Ok(mut count) = registry().browser_errors.lock() {
        *count += 1;
    }
    let clip = |value: &str, limit: usize| -> String {
        let value = value.split(['?', '#']).next().unwrap_or_default();
        value
            .chars()
            .filter(|character| !character.is_control())
            .take(limit)
            .collect()
    };
    tracing::warn!(
        target: "agent_workspace::browser",
        message = clip(&report.message, 300),
        source = report.source.as_deref().map(|value| clip(value, 200)),
        route = report.route.as_deref().map(|value| clip(value, 200)),
        release = report.release.as_deref().map(|value| clip(value, 64)),
        "browser error"
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Recurring check: alert when realtime delivery or agent wakeups have
/// fallen behind.
pub async fn health_check(state: &AppState) -> Result<(), AppError> {
    let gauges = sqlx::query_as::<_, (f64, i64)>(
        r#"
        SELECT
            COALESCE((SELECT EXTRACT(EPOCH FROM now() - min(created_at)) FROM outbox_events
             WHERE published_at IS NULL AND failed_at IS NULL), 0)::float8,
            (SELECT count(*) FROM agent_webhook_deliveries
             WHERE delivered_at IS NULL AND attempts >= 3)
        "#,
    )
    .fetch_one(&state.pool)
    .await?;
    if gauges.0 > 300.0 {
        alert(Alert {
            key: "outbox_lag",
            subject: "outbox".to_owned(),
            detail: format!(
                "Live updates are {} minutes behind: the outbox publisher isn't keeping up.",
                (gauges.0 / 60.0).round()
            ),
        });
    }
    if gauges.1 > 0 {
        alert(Alert {
            key: "agent_webhooks_failing",
            subject: "agent_webhooks".to_owned(),
            detail: format!(
                "{} agent wakeup webhooks have failed three times or more.",
                gauges.1
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_reports_have_a_server_wide_budget_per_minute() {
        let start = Instant::now();
        *registry().browser_window.lock().expect("window") = None;
        for _ in 0..BROWSER_REPORTS_PER_MINUTE {
            assert_eq!(take_browser_report_slot(start), Ok(()));
        }
        let refused = take_browser_report_slot(start + Duration::from_secs(10));
        assert_eq!(refused, Err(50));
        assert_eq!(
            take_browser_report_slot(start + Duration::from_secs(61)),
            Ok(())
        );
    }

    #[test]
    fn route_classes_never_carry_ids() {
        assert_eq!(route_class("/api/v1/projects"), "api/projects");
        assert_eq!(
            route_class("/api/v1/projects/0199a4c2-0000-7000-8000-000000000000/work-items"),
            "api/projects/work-items"
        );
        assert_eq!(
            route_class("/api/v1/conversations/abc123/messages/def"),
            "api/conversations/messages"
        );
        assert_eq!(route_class("/health/ready"), "health");
        assert_eq!(route_class("/projects/WORK"), "web");
    }

    #[test]
    fn repeated_alerts_are_held_back() {
        struct Count(Mutex<u32>);
        impl AlertSink for Count {
            fn send(&self, _: &Alert) {
                *self.0.lock().expect("count") += 1;
            }
        }
        let count = Arc::new(Count(Mutex::new(0)));
        install_alert_sink(count.clone());
        for _ in 0..3 {
            alert(Alert {
                key: "test_repeat",
                subject: "one".to_owned(),
                detail: "Something broke.".to_owned(),
            });
        }
        alert(Alert {
            key: "test_repeat",
            subject: "two".to_owned(),
            detail: "Something else broke.".to_owned(),
        });
        assert_eq!(*count.0.lock().expect("count"), 2);
    }
}
