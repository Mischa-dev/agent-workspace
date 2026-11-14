//! One list of errors for every edge. `message` is written for whoever
//! reads it, person or agent: what happened and what to do next. It never
//! carries anything the caller sent.

use std::borrow::Cow;

use axum::http::StatusCode;
use serde::Serialize;

use super::Perm;
use crate::error::AppError;

#[derive(Clone, Debug)]
pub enum OpError {
    Unauthenticated,
    NotFound {
        what: &'static str,
    },
    Invalid {
        field: Cow<'static, str>,
        message: Cow<'static, str>,
    },
    Denied {
        reason: DenyReason,
        message: Cow<'static, str>,
    },
    Conflict {
        reason: ConflictReason,
        message: Cow<'static, str>,
        current: Option<serde_json::Value>,
    },
    RateLimited {
        retry_after_seconds: u64,
    },
    /// A plan or spend limit (H1).
    LimitReached {
        limit: Cow<'static, str>,
    },
    /// A suspended workspace refuses writes (H1).
    WorkspaceSuspended,
    TooLarge {
        limit_bytes: usize,
    },
    /// Logged with the operation's name; never leaks detail.
    Unavailable,
}

#[derive(Clone, Debug)]
pub enum DenyReason {
    Role { permission: Perm },
    ReadOnlyCredential,
    ModuleOff { module: String },
    Delegation,
    AccountPaused,
    HostRevoked,
}

#[derive(Clone, Debug)]
pub enum ConflictReason {
    Version { current_version: i64 },
    State { code: &'static str },
    Transition,
    RequestIdReused,
    Archived,
}

/// What a caller should do about an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryCategory {
    Retry,
    Reread,
    FixInput,
    Reauthenticate,
    AskForAccess,
    Module,
    Wait,
    DoNotRetry,
}

/// Every error outcome, in the order the envelope schema lists them.
pub const ERROR_OUTCOMES: &[&str] = &[
    "unauthenticated",
    "not_found",
    "invalid_request",
    "permission_denied",
    "conflict",
    "rate_limited",
    "limit_reached",
    "workspace_suspended",
    "too_large",
    "temporarily_unavailable",
];

impl OpError {
    pub fn invalid(field: &'static str, message: &'static str) -> Self {
        Self::Invalid {
            field: Cow::Borrowed(field),
            message: Cow::Borrowed(message),
        }
    }

    pub fn not_found(what: &'static str) -> Self {
        Self::NotFound { what }
    }

    /// The envelope's `outcome`.
    pub fn outcome(&self) -> &'static str {
        match self {
            Self::Unauthenticated => "unauthenticated",
            Self::NotFound { .. } => "not_found",
            Self::Invalid { .. } => "invalid_request",
            Self::Denied { .. } => "permission_denied",
            Self::Conflict { .. } => "conflict",
            Self::RateLimited { .. } => "rate_limited",
            Self::LimitReached { .. } => "limit_reached",
            Self::WorkspaceSuspended => "workspace_suspended",
            Self::TooLarge { .. } => "too_large",
            Self::Unavailable => "temporarily_unavailable",
        }
    }

    pub fn retry_category(&self) -> RetryCategory {
        match self {
            Self::Unauthenticated => RetryCategory::Reauthenticate,
            Self::NotFound { .. } => RetryCategory::FixInput,
            Self::Invalid { .. } => RetryCategory::FixInput,
            Self::Denied { reason, .. } => match reason {
                DenyReason::ModuleOff { .. } => RetryCategory::Module,
                DenyReason::ReadOnlyCredential => RetryCategory::Reauthenticate,
                DenyReason::AccountPaused | DenyReason::HostRevoked => RetryCategory::DoNotRetry,
                DenyReason::Role { .. } | DenyReason::Delegation => RetryCategory::AskForAccess,
            },
            Self::Conflict { reason, .. } => match reason {
                ConflictReason::Version { .. } | ConflictReason::State { .. } => {
                    RetryCategory::Reread
                }
                ConflictReason::RequestIdReused => RetryCategory::FixInput,
                ConflictReason::Transition | ConflictReason::Archived => RetryCategory::DoNotRetry,
            },
            Self::RateLimited { .. } => RetryCategory::Wait,
            Self::LimitReached { .. } | Self::WorkspaceSuspended => RetryCategory::DoNotRetry,
            Self::TooLarge { .. } => RetryCategory::FixInput,
            Self::Unavailable => RetryCategory::Retry,
        }
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::NotFound { .. } => StatusCode::NOT_FOUND,
            Self::Invalid { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Denied { .. } => StatusCode::FORBIDDEN,
            Self::Conflict { .. } => StatusCode::CONFLICT,
            Self::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::LimitReached { .. } => StatusCode::PAYMENT_REQUIRED,
            Self::WorkspaceSuspended => StatusCode::LOCKED,
            Self::TooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    pub fn message(&self) -> Cow<'static, str> {
        match self {
            Self::Unauthenticated => "Sign in again: this connection isn't authorized.".into(),
            Self::NotFound { what } => format!("{what} wasn't found, or you can't see it.").into(),
            Self::Invalid { message, .. }
            | Self::Denied { message, .. }
            | Self::Conflict { message, .. } => message.clone(),
            Self::RateLimited {
                retry_after_seconds,
            } => format!("Too many requests. Try again in {retry_after_seconds} seconds.").into(),
            Self::LimitReached { limit } => {
                format!("This workspace has reached its {limit} limit.").into()
            }
            Self::WorkspaceSuspended => {
                "This workspace is suspended, so it can't be changed.".into()
            }
            Self::TooLarge { limit_bytes } => {
                format!("That's too large: the limit is {limit_bytes} bytes.").into()
            }
            Self::Unavailable => "Agent Workspace couldn't finish that. Try again shortly.".into(),
        }
    }

    /// The error half of the envelope.
    pub fn to_json(&self) -> serde_json::Value {
        let mut body = serde_json::json!({
            "outcome": self.outcome(),
            "message": self.message(),
            "retryCategory": self.retry_category(),
        });
        let mut details = serde_json::Map::new();
        match self {
            Self::Invalid { field, .. } => body["field"] = field.as_ref().into(),
            Self::Denied { reason, .. } => {
                let code = match reason {
                    DenyReason::Role { permission } => {
                        details.insert("permission".into(), permission.key().into());
                        "role"
                    }
                    DenyReason::ReadOnlyCredential => "read_only_credential",
                    DenyReason::ModuleOff { module } => {
                        details.insert("module".into(), module.as_str().into());
                        body["module"] = module.as_str().into();
                        "module_off"
                    }
                    DenyReason::Delegation => "delegation",
                    DenyReason::AccountPaused => "account_paused",
                    DenyReason::HostRevoked => "host_revoked",
                };
                body["code"] = code.into();
            }
            Self::Conflict {
                reason, current, ..
            } => {
                let code = match reason {
                    ConflictReason::Version { current_version } => {
                        details.insert("currentVersion".into(), (*current_version).into());
                        "version"
                    }
                    ConflictReason::State { code } => code,
                    ConflictReason::Transition => "transition",
                    ConflictReason::RequestIdReused => "request_id_reused",
                    ConflictReason::Archived => "archived",
                };
                body["code"] = code.into();
                if let Some(current) = current {
                    details.insert("current".into(), current.clone());
                }
            }
            Self::RateLimited {
                retry_after_seconds,
            } => {
                details.insert("retryAfterSeconds".into(), (*retry_after_seconds).into());
            }
            Self::TooLarge { limit_bytes } => {
                details.insert("limitBytes".into(), (*limit_bytes).into());
            }
            _ => {}
        }
        if !details.is_empty() {
            body["details"] = serde_json::Value::Object(details);
        }
        body
    }

    /// For a failure the caller can do nothing about: logged, then answered
    /// as `temporarily_unavailable`.
    pub fn logged(op: &'static str, error: &AppError) -> Self {
        let (code, constraint) = error.database_log_fields();
        tracing::error!(
            operation = op,
            category = error.log_category(),
            sqlstate = code.as_deref(),
            constraint = constraint.as_deref(),
            "operation failed"
        );
        Self::Unavailable
    }
}

/// The one mapping from the domain's errors (it was `ToolResult::from_error`,
/// `mcp/common.rs`); operations use `?` on domain calls.
impl From<AppError> for OpError {
    fn from(error: AppError) -> Self {
        match error {
            AppError::AuthenticationRequired | AppError::InvalidCredentials => {
                Self::Unauthenticated
            }
            AppError::NotFound(what) => Self::NotFound { what },
            AppError::Validation(field) | AppError::Invalid(field) => Self::Invalid {
                field: Cow::Borrowed(field),
                message: format!("{field} isn't valid.").into(),
            },
            AppError::Refused { field, message } => Self::Invalid {
                field: Cow::Borrowed(field),
                message: Cow::Borrowed(message),
            },
            AppError::Conflict("idempotency_key", "payload mismatch") => Self::Conflict {
                reason: ConflictReason::RequestIdReused,
                message: "That request id was already used for a different request. Use a new one."
                    .into(),
                current: None,
            },
            AppError::Conflict(code, message) => Self::Conflict {
                reason: ConflictReason::State { code },
                message: Cow::Borrowed(message),
                current: None,
            },
            AppError::VersionConflict {
                current_version, ..
            } => Self::Conflict {
                reason: ConflictReason::Version { current_version },
                message: format!(
                    "Someone changed this first (now version {current_version}). Read it again, then retry."
                )
                .into(),
                current: None,
            },
            AppError::ProjectArchived => Self::Conflict {
                reason: ConflictReason::Archived,
                message: "That space is archived. Restore it first.".into(),
                current: None,
            },
            AppError::Forbidden | AppError::OriginRejected | AppError::CsrfInvalid => {
                Self::Denied {
                    reason: DenyReason::Delegation,
                    message: "You don't have permission to do that here.".into(),
                }
            }
            AppError::ModuleOff { module } => Self::Denied {
                reason: DenyReason::ModuleOff {
                    module: module.to_owned(),
                },
                message: format!("{module} is turned off here (for this workspace, or for your role).")
                    .into(),
            },
            AppError::RateLimited {
                retry_after_seconds,
            } => Self::RateLimited {
                retry_after_seconds,
            },
            AppError::PayloadTooLarge => Self::TooLarge {
                limit_bytes: crate::MAX_APPLICATION_BODY_BYTES,
            },
            other => Self::logged("operation", &other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_errors_map_as_the_table_says() {
        let cases: Vec<(AppError, &str, StatusCode)> = vec![
            (
                AppError::NotFound("page"),
                "not_found",
                StatusCode::NOT_FOUND,
            ),
            (
                AppError::Validation("title"),
                "invalid_request",
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            (
                AppError::VersionConflict {
                    resource: "task",
                    current_version: 8,
                },
                "conflict",
                StatusCode::CONFLICT,
            ),
            (
                AppError::Conflict("idempotency_key", "payload mismatch"),
                "conflict",
                StatusCode::CONFLICT,
            ),
            (
                AppError::Forbidden,
                "permission_denied",
                StatusCode::FORBIDDEN,
            ),
            (
                AppError::ModuleOff { module: "pages" },
                "permission_denied",
                StatusCode::FORBIDDEN,
            ),
            (
                AppError::rate_limited(3),
                "rate_limited",
                StatusCode::TOO_MANY_REQUESTS,
            ),
            (
                AppError::PayloadTooLarge,
                "too_large",
                StatusCode::PAYLOAD_TOO_LARGE,
            ),
            (
                AppError::AuthenticationRequired,
                "unauthenticated",
                StatusCode::UNAUTHORIZED,
            ),
            (
                AppError::Internal("x"),
                "temporarily_unavailable",
                StatusCode::SERVICE_UNAVAILABLE,
            ),
        ];
        for (error, outcome, status) in cases {
            let mapped = OpError::from(error);
            assert_eq!(mapped.outcome(), outcome);
            assert_eq!(mapped.status(), status);
            assert!(ERROR_OUTCOMES.contains(&mapped.outcome()));
        }
        let version = OpError::from(AppError::VersionConflict {
            resource: "task",
            current_version: 8,
        })
        .to_json();
        assert_eq!(version["code"], "version");
        assert_eq!(version["details"]["currentVersion"], 8);
        assert_eq!(version["retryCategory"], "reread");
        let reused = OpError::from(AppError::Conflict("idempotency_key", "payload mismatch"));
        assert_eq!(reused.to_json()["code"], "request_id_reused");
        let module = OpError::from(AppError::ModuleOff { module: "pages" }).to_json();
        assert_eq!(module["retryCategory"], "module");
        assert_eq!(module["module"], "pages");
    }
}
