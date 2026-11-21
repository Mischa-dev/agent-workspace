//! A1.17: the built-in host's plan windows go to the server the way any
//! host's do, on `POST /api/v1/host/windows`. The built-in host's accounts
//! are its own account homes on this machine; it names each by its id.

use agent_workspace_host_protocol::{AccountWindows, HostClient, PlanWindow, WindowsReport};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::{
    RuntimeError,
    harness::adapter::{UsageReport, UsageWindow},
};

fn at(seconds: Option<i64>) -> Option<String> {
    seconds
        .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0))
        .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn window(window: &UsageWindow) -> PlanWindow {
    PlanWindow {
        key: window.key.clone(),
        label: window.limit_name.clone(),
        used_percent: window.used_percent.min(100),
        resets_at: at(window.resets_at),
        duration_minutes: window.duration_minutes,
        limit_reached: window.limit_reached,
    }
}

/// The protocol report of what a harness said about one account. A partial
/// report (heard during a run) changes only the windows it lists.
pub fn windows_report(account: Uuid, report: &UsageReport, partial: bool) -> WindowsReport {
    WindowsReport {
        accounts: vec![AccountWindows {
            reference: account.to_string(),
            plan: report.plan.clone(),
            windows: report.windows.iter().map(window).collect(),
            reset_credits: report.reset_credits.as_ref().map(|credits| {
                json!({
                    "available": credits.available,
                    "credits": credits.credits.iter().map(|credit| json!({
                        "id": credit.id,
                        "title": credit.title,
                        "expiresAt": at(credit.expires_at),
                        "grantedAt": at(credit.granted_at),
                    })).collect::<Vec<_>>(),
                })
            }),
        }],
        partial,
    }
}

/// Reports one account's windows. The caller decides what a failure means:
/// for a run, usage is a side note, so it is only logged.
pub async fn report(
    client: &HostClient,
    account: Uuid,
    report: &UsageReport,
    partial: bool,
) -> Result<(), RuntimeError> {
    client
        .report_windows(&windows_report(account, report, partial))
        .await
        .map(drop)
        .map_err(|error| {
            let status = match error {
                agent_workspace_host_protocol::ClientError::Refused(status, _) => status,
                _ => 0,
            };
            tracing::warn!(status, "the built-in host's plan windows were not taken");
            RuntimeError::Internal("report plan windows")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::adapter::{ResetCredit, ResetCredits};

    #[test]
    fn a_harness_report_becomes_the_protocol_report() {
        let account = Uuid::now_v7();
        let report = UsageReport {
            windows: vec![UsageWindow {
                key: "codex.secondary".to_owned(),
                limit_name: Some("Weekly".to_owned()),
                used_percent: 140,
                resets_at: Some(1_800_000_000),
                duration_minutes: Some(10_080),
                limit_reached: false,
            }],
            reset_credits: Some(ResetCredits {
                available: 1,
                credits: vec![ResetCredit {
                    id: "r1".to_owned(),
                    title: None,
                    expires_at: None,
                    granted_at: Some(1_700_000_000),
                }],
            }),
            plan: Some("pro".to_owned()),
        };
        let sent = serde_json::to_value(windows_report(account, &report, true)).expect("json");
        assert_eq!(sent["partial"], true);
        let sent_account = &sent["accounts"][0];
        assert_eq!(sent_account["ref"], account.to_string());
        assert_eq!(sent_account["windows"][0]["usedPercent"], 100);
        assert_eq!(
            sent_account["windows"][0]["resetsAt"],
            "2027-01-15T08:00:00Z"
        );
        assert_eq!(sent_account["resetCredits"]["available"], 1);
        assert_eq!(
            sent_account["resetCredits"]["credits"][0]["grantedAt"],
            "2023-11-14T22:13:20Z"
        );
        let full = serde_json::to_value(windows_report(account, &report, false)).expect("json");
        assert!(full.get("partial").is_none());
    }
}
