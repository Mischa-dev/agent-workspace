use axum::{
    Json,
    extract::{FromRequest, FromRequestParts, Path, Query, Request},
    http::{HeaderValue, StatusCode, header::RETRY_AFTER},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tracing::error;
use uuid::Uuid;

/// JSON request extractor that keeps every API rejection inside the public
/// error-envelope contract. Axum's default rejection includes parser details
/// in a plain-text body, which is neither stable nor appropriate to expose.
pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = AppError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::<T>::from_request(request, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|rejection| {
                if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
                    AppError::PayloadTooLarge
                } else {
                    AppError::Invalid("JSON request body")
                }
            })
    }
}

/// Path extractor with a stable, structured rejection.
pub struct ApiPath<T>(pub T);

impl<S, T> FromRequestParts<S> for ApiPath<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(|_| AppError::Invalid("request path"))
    }
}

/// Query extractor with a stable, structured rejection.
pub struct ApiQuery<T>(pub T);

impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &S,
    ) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(|_| AppError::Invalid("request query"))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("authentication is required")]
    AuthenticationRequired,
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("the actor is not authorized")]
    Forbidden,
    #[error("request origin was rejected")]
    OriginRejected,
    #[error("CSRF token is invalid")]
    CsrfInvalid,
    #[error("{0} was not found")]
    NotFound(&'static str),
    #[error("{0} conflict: {1}")]
    Conflict(&'static str, &'static str),
    #[error("{resource} version conflict at version {current_version}")]
    VersionConflict {
        resource: &'static str,
        current_version: i64,
    },
    #[error("invalid request: {0}")]
    Invalid(&'static str),
    #[error("validation failed: {0}")]
    Validation(&'static str),
    /// Refused for a reason the person can act on: `message` is shown to them
    /// as it is, so it never carries anything they sent.
    #[error("refused: {field}")]
    Refused {
        field: &'static str,
        message: &'static str,
    },
    #[error("the project key already exists")]
    ProjectKeyConflict,
    #[error("the project has an active run")]
    ProjectHasActiveRuns,
    #[error("the repository has an active run")]
    RepositoryHasActiveRuns,
    /// The database's admission check refused to let a run go ahead, such
    /// as when an approval is answered. `reason` says why in words the
    /// person can act on; it never carries anything they sent.
    #[error("the run can't go ahead: {reason}")]
    RunNotAdmitted { reason: &'static str },
    #[error("the project is archived")]
    ProjectArchived,
    #[error("a repository selection is required")]
    RepositoryChoiceRequired { repository_ids: Vec<Uuid> },
    #[error("the selected repository conflicts with the Thread binding")]
    RepositoryBindingConflict,
    #[error("a personal execution profile must be connected before starting a run")]
    ExecutionProfileRequired,
    #[error("request rate limit exceeded; retry after {retry_after_seconds} seconds")]
    RateLimited { retry_after_seconds: u64 },
    #[error("request payload is too large")]
    PayloadTooLarge,
    #[error("development authentication is unavailable")]
    DevelopmentAuthUnavailable,
    #[error("encrypted secrets are not configured")]
    SecretsNotConfigured,
    /// The route belongs to a module that's off where the request acts
    /// (AWD-97); `module` is its id, such as `tasks`.
    #[error("the {module} module is turned off")]
    ModuleOff { module: &'static str },
    #[error("internal invariant failed: {0}")]
    Internal(&'static str),
}

impl AppError {
    pub fn rate_limited(retry_after_seconds: u64) -> Self {
        Self::RateLimited {
            retry_after_seconds: retry_after_seconds.max(1),
        }
    }

    pub fn log_category(&self) -> &'static str {
        match self {
            Self::Database(_) => "database",
            Self::AuthenticationRequired => "authentication_required",
            Self::InvalidCredentials => "invalid_credentials",
            Self::Forbidden => "forbidden",
            Self::OriginRejected => "origin_rejected",
            Self::CsrfInvalid => "csrf_invalid",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_, _) => "conflict",
            Self::VersionConflict { .. } => "version_conflict",
            Self::Invalid(_) => "invalid",
            Self::Validation(_) | Self::Refused { .. } => "validation",
            Self::ProjectKeyConflict => "project_key_conflict",
            Self::ProjectHasActiveRuns => "project_has_active_runs",
            Self::RepositoryHasActiveRuns => "repository_has_active_runs",
            Self::RunNotAdmitted { .. } => "run_not_admitted",
            Self::ProjectArchived => "project_archived",
            Self::RepositoryChoiceRequired { .. } => "repository_choice_required",
            Self::RepositoryBindingConflict => "repository_binding_conflict",
            Self::ExecutionProfileRequired => "execution_profile_required",
            Self::RateLimited { .. } => "rate_limited",
            Self::PayloadTooLarge => "payload_too_large",
            Self::DevelopmentAuthUnavailable => "development_auth_unavailable",
            Self::SecretsNotConfigured => "secrets_not_configured",
            Self::ModuleOff { .. } => "module_off",
            Self::Internal(_) => "internal",
        }
    }

    /// Return bounded database diagnostics suitable for structured logs.
    ///
    /// SQLSTATE and constraint names identify the failed invariant without
    /// exposing driver messages, SQL text, bound values, or database content.
    pub fn database_log_fields(&self) -> (Option<String>, Option<String>) {
        let Self::Database(sqlx::Error::Database(error)) = self else {
            return (None, None);
        };
        (
            error.code().map(|value| value.into_owned()),
            error.constraint().map(str::to_owned),
        )
    }
}

#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<ErrorDetails>,
}

#[derive(Serialize)]
#[serde(untagged, rename_all = "camelCase")]
enum ErrorDetails {
    Version {
        resource: &'static str,
        #[serde(rename = "currentVersion")]
        current_version: i64,
    },
    RepositoryChoice {
        #[serde(rename = "repositoryIds")]
        repository_ids: Vec<Uuid>,
    },
    Action {
        action: &'static str,
    },
    Module {
        module: &'static str,
    },
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let details = match &self {
            Self::VersionConflict {
                resource,
                current_version,
            } => Some(ErrorDetails::Version {
                resource,
                current_version: *current_version,
            }),
            Self::RepositoryChoiceRequired { repository_ids } => {
                Some(ErrorDetails::RepositoryChoice {
                    repository_ids: repository_ids.clone(),
                })
            }
            Self::RepositoryBindingConflict => Some(ErrorDetails::Action {
                action: "start_new_thread",
            }),
            Self::ExecutionProfileRequired => Some(ErrorDetails::Action {
                action: "connect_provider_account",
            }),
            Self::ModuleOff { module } => Some(ErrorDetails::Module { module }),
            _ => None,
        };
        let (status, code, message, retry_after_seconds) = match self {
            Self::AuthenticationRequired => (
                StatusCode::UNAUTHORIZED,
                "authentication_required",
                "Sign in to continue.",
                None,
            ),
            Self::InvalidCredentials => (
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                "The login name or password is invalid.",
                None,
            ),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "forbidden",
                "You do not have permission to perform this action.",
                None,
            ),
            Self::OriginRejected => (
                StatusCode::FORBIDDEN,
                "request_origin_rejected",
                "The request origin is not allowed.",
                None,
            ),
            Self::CsrfInvalid => (
                StatusCode::FORBIDDEN,
                "csrf_token_invalid",
                "The request security token is invalid.",
                None,
            ),
            Self::NotFound(_) => (
                StatusCode::NOT_FOUND,
                "not_found",
                "The requested resource was not found.",
                None,
            ),
            Self::Conflict(_, _) => (
                StatusCode::CONFLICT,
                "conflict",
                "That conflicts with a change made elsewhere. Refresh and try again.",
                None,
            ),
            Self::VersionConflict { .. } => (
                StatusCode::CONFLICT,
                "version_conflict",
                "Someone changed this after you opened it. Refresh and try again.",
                None,
            ),
            Self::Invalid(_) => (
                StatusCode::BAD_REQUEST,
                "invalid_request",
                "The request is invalid.",
                None,
            ),
            Self::Validation(_) => (
                StatusCode::BAD_REQUEST,
                "validation_failed",
                "One or more fields are invalid.",
                None,
            ),
            Self::Refused { message, .. } => {
                (StatusCode::BAD_REQUEST, "validation_failed", message, None)
            }
            Self::ProjectKeyConflict => (
                StatusCode::CONFLICT,
                "project_key_conflict",
                "That Project key is already in use.",
                None,
            ),
            Self::ProjectHasActiveRuns => (
                StatusCode::CONFLICT,
                "project_has_active_runs",
                "Interrupt or finish active runs before archiving this Project.",
                None,
            ),
            Self::RepositoryHasActiveRuns => (
                StatusCode::CONFLICT,
                "repository_has_active_runs",
                "Stop or finish this repository's runs before archiving it.",
                None,
            ),
            Self::RunNotAdmitted { reason } => {
                (StatusCode::CONFLICT, "run_not_admitted", reason, None)
            }
            Self::ProjectArchived => (
                StatusCode::CONFLICT,
                "project_archived",
                "The Project is archived.",
                None,
            ),
            Self::RepositoryChoiceRequired { .. } => (
                StatusCode::CONFLICT,
                "repository_choice_required",
                "Choose a trusted repository before starting this run.",
                None,
            ),
            Self::RepositoryBindingConflict => (
                StatusCode::CONFLICT,
                "repository_binding_conflict",
                "This Thread is already bound to another repository. Start a new Thread.",
                None,
            ),
            Self::ExecutionProfileRequired => (
                StatusCode::CONFLICT,
                "execution_profile_required",
                "Connect your personal Codex account before starting a run.",
                None,
            ),
            Self::RateLimited {
                retry_after_seconds,
            } => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "Too many attempts. Try again later.",
                Some(retry_after_seconds),
            ),
            Self::PayloadTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "The request payload is too large.",
                None,
            ),
            Self::DevelopmentAuthUnavailable => (
                StatusCode::FORBIDDEN,
                "development_auth_unavailable",
                "Development authentication is unavailable.",
                None,
            ),
            Self::SecretsNotConfigured => (
                StatusCode::SERVICE_UNAVAILABLE,
                "secrets_not_configured",
                "This server can't store keys yet: its administrator needs to set up the secrets keyring.",
                None,
            ),
            Self::ModuleOff { .. } => (
                StatusCode::FORBIDDEN,
                "module_off",
                "This part of the workspace is turned off.",
                None,
            ),
            Self::Database(error) => {
                let (database_code, database_constraint) = error
                    .as_database_error()
                    .map(|database_error| {
                        (
                            database_error.code().map(|value| value.into_owned()),
                            database_error.constraint().map(str::to_owned),
                        )
                    })
                    .unwrap_or((None, None));
                error!(
                    database_code = database_code.as_deref().unwrap_or("unavailable"),
                    database_constraint = database_constraint.as_deref().unwrap_or("unavailable"),
                    "request database operation failed"
                );
                crate::metrics::server_error("database");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "The request could not be completed.",
                    None,
                )
            }
            Self::Internal(invariant) => {
                error!(invariant, "request invariant failed");
                crate::metrics::server_error("internal");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "The request could not be completed.",
                    None,
                )
            }
        };

        let mut response = (
            status,
            Json(ErrorEnvelope {
                error: ErrorBody {
                    code,
                    message,
                    details,
                },
            }),
        )
            .into_response();
        if let Some(seconds) = retry_after_seconds
            && let Ok(value) = HeaderValue::from_str(&seconds.to_string())
        {
            response.headers_mut().insert(RETRY_AFTER, value);
        }
        response
    }
}

impl From<crate::policy::UnknownRole> for AppError {
    fn from(_: crate::policy::UnknownRole) -> Self {
        Self::Internal("stored membership role")
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::Request,
        response::IntoResponse,
        routing::get,
    };
    use serde::Deserialize;
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::{ApiPath, ApiQuery, AppError};

    async fn path_handler(ApiPath(_id): ApiPath<Uuid>) {}

    #[derive(Deserialize)]
    struct TestQuery {
        limit: u16,
    }

    async fn query_handler(ApiQuery(query): ApiQuery<TestQuery>) {
        let _ = query.limit;
    }

    #[test]
    fn rate_limit_errors_include_retry_after() {
        let response = AppError::rate_limited(17).into_response();

        assert_eq!(response.status(), 429);
        assert_eq!(
            response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok()),
            Some("17")
        );
    }

    #[tokio::test]
    async fn database_errors_do_not_expose_driver_details_in_the_response() {
        let response = AppError::Database(sqlx::Error::Protocol(
            "synthetic credential must remain private".to_owned(),
        ))
        .into_response();
        let body = to_bytes(response.into_body(), 16_384)
            .await
            .expect("error response body");
        let body = String::from_utf8(body.to_vec()).expect("UTF-8 error response");

        assert!(!body.contains("synthetic credential"));
        assert!(body.contains("\"code\":\"internal_error\""));
    }

    #[tokio::test]
    async fn version_conflicts_return_the_authoritative_version() {
        let response = AppError::VersionConflict {
            resource: "project_navigation",
            current_version: 7,
        }
        .into_response();
        assert_eq!(response.status(), 409);
        let body = to_bytes(response.into_body(), 16_384)
            .await
            .expect("version conflict body");
        let body = String::from_utf8(body.to_vec()).expect("UTF-8 conflict response");

        assert!(body.contains("\"code\":\"version_conflict\""));
        assert!(body.contains("\"resource\":\"project_navigation\""));
        assert!(body.contains("\"currentVersion\":7"));
    }

    #[tokio::test]
    async fn malformed_path_and_query_values_use_the_error_envelope() {
        let application = Router::new()
            .route("/items/{id}", get(path_handler))
            .route("/search", get(query_handler));

        for uri in ["/items/not-a-uuid", "/search?limit=not-a-number"] {
            let response = application
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), 400, "{uri}");
            let body = to_bytes(response.into_body(), 16_384)
                .await
                .expect("error response body");
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body).expect("JSON error envelope")["error"]
                    ["code"],
                "invalid_request",
                "{uri}"
            );
        }
    }
}
