//! What the static web app needs to know at start (H1.41, R30), and CORS for
//! the origins it may be served from.
//!
//! The web app's build has no origin in it: it can be served by this server
//! (the self-host default), by any static host or by a CDN. At start it
//! fetches `GET /client-config` and talks to the `apiOrigin` and
//! `liveOrigin` it names. The answer is built from the server's
//! configuration and never holds a secret.
//!
//! A web app on another origin must be on the same site as the API (for
//! example `app.example.com` and `api.example.com`): the session cookies are
//! `SameSite=Strict`, so the browser only sends them within one site.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Request, State},
    http::{
        HeaderMap, HeaderValue, Method, StatusCode,
        header::{
            ACCESS_CONTROL_ALLOW_CREDENTIALS, ACCESS_CONTROL_ALLOW_HEADERS,
            ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
            ACCESS_CONTROL_EXPOSE_HEADERS, ACCESS_CONTROL_MAX_AGE, ACCESS_CONTROL_REQUEST_METHOD,
            CACHE_CONTROL, ORIGIN, VARY,
        },
    },
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Serialize;

use crate::{AppState, config::AppConfig};

pub fn router() -> Router<AppState> {
    Router::new().route("/client-config", get(read))
}

/// The web app's configuration.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientConfig {
    /// Where the API answers. `null` when the page asking was served by
    /// this server itself (no `Origin`, or `Sec-Fetch-Site: same-origin`):
    /// the app then keeps to the origin it was loaded from, whichever of the
    /// server's names that was.
    pub api_origin: Option<String>,
    /// Where the live connection opens (an https origin; the app switches it
    /// to wss); `null` means the API origin.
    pub live_origin: Option<String>,
    pub sign_in: SignIn,
    /// `invite` (accounts are made by invitation or by an operator) or
    /// `open` (anyone can sign up through the configured sign-in service).
    pub signup: &'static str,
    pub features: Features,
    /// The server's release.
    pub version: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignIn {
    /// `password`, and `workos` and `google` when configured.
    pub methods: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Features {
    pub link_previews: bool,
    pub github: bool,
}

impl ClientConfig {
    /// `from_other_origin`: the page asking was loaded from another origin
    /// (a static host), so it needs the API's origin spelled out.
    pub fn from_config(config: &AppConfig, from_other_origin: bool) -> Self {
        let mut methods = vec!["password"];
        if let Some(workos) = &config.workos {
            methods.push("workos");
            if workos.google_enabled {
                methods.push("google");
            }
        }
        Self {
            api_origin: config.public_origin.clone().filter(|_| from_other_origin),
            live_origin: config.live_origin.clone(),
            sign_in: SignIn { methods },
            signup: if config
                .workos
                .as_ref()
                .is_some_and(|workos| workos.allow_signup)
            {
                "open"
            } else {
                "invite"
            },
            features: Features {
                link_previews: config.link_previews,
                github: config
                    .github
                    .as_ref()
                    .is_some_and(|github| github.connect.is_some()),
            },
            version: env!("CARGO_PKG_VERSION"),
        }
    }
}

/// `GET /client-config`: public, and cacheable for a minute.
async fn read(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let same_origin = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("same-origin"));
    let from_other_origin = !same_origin && headers.contains_key(ORIGIN);
    let mut response =
        Json(ClientConfig::from_config(&state.config, from_other_origin)).into_response();
    let headers = response.headers_mut();
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=60"),
    );
    headers.append(VARY, HeaderValue::from_static("sec-fetch-site"));
    response
}

/// The origins the web app may be served from (`ALLOWED_ORIGINS`).
#[derive(Clone)]
pub struct WebOrigins(pub Arc<[String]>);

/// Middleware: CORS for the configured web origins only. A preflight from
/// one of them is answered here; a request from any other origin gets no
/// CORS headers, so the browser keeps its response from the page.
pub async fn cors(State(origins): State<WebOrigins>, request: Request, next: Next) -> Response {
    let allowed = allowed_origin(&origins, request.headers());
    if request.method() == Method::OPTIONS
        && request
            .headers()
            .contains_key(ACCESS_CONTROL_REQUEST_METHOD)
    {
        let mut response = StatusCode::NO_CONTENT.into_response();
        if let Some(origin) = allowed {
            let headers = response.headers_mut();
            allow(headers, origin);
            headers.insert(
                ACCESS_CONTROL_ALLOW_METHODS,
                HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE"),
            );
            headers.insert(
                ACCESS_CONTROL_ALLOW_HEADERS,
                HeaderValue::from_static("accept, content-type, idempotency-key, x-csrf-token"),
            );
            headers.insert(ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
        }
        response
            .headers_mut()
            .append(VARY, HeaderValue::from_static("origin"));
        return response;
    }
    let mut response = next.run(request).await;
    if let Some(origin) = allowed {
        allow(response.headers_mut(), origin);
        response.headers_mut().insert(
            ACCESS_CONTROL_EXPOSE_HEADERS,
            HeaderValue::from_static("x-request-id"),
        );
    }
    response
        .headers_mut()
        .append(VARY, HeaderValue::from_static("origin"));
    response
}

fn allowed_origin(origins: &WebOrigins, headers: &HeaderMap) -> Option<HeaderValue> {
    let origin = headers.get(ORIGIN)?;
    let text = origin.to_str().ok()?;
    origins
        .0
        .iter()
        .any(|allowed| allowed == text)
        .then(|| origin.clone())
}

fn allow(headers: &mut HeaderMap, origin: HeaderValue) {
    headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    headers.insert(
        ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
}

/// Whether the web app may be served from an origin other than the API's
/// own, so the API's files must be loadable by a page on another origin of
/// the same site.
pub fn split_origins(config: &AppConfig) -> bool {
    config
        .public_origin
        .as_ref()
        .is_some_and(|public| config.allowed_origins.iter().any(|origin| origin != public))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::{body::Body, http::Request as HttpRequest};
    use tower::ServiceExt as _;

    use super::*;

    fn config(values: &[(&str, &str)]) -> AppConfig {
        AppConfig::from_values(
            &values
                .iter()
                .map(|(key, value)| (format!("AGENT_WORKSPACE_{key}"), (*value).to_owned()))
                .collect::<BTreeMap<_, _>>(),
        )
        .expect("config")
    }

    #[test]
    fn the_config_names_origins_and_sign_in_methods() {
        let plain = ClientConfig::from_config(
            &config(&[("DATABASE_URL", "postgres://aw:pw@127.0.0.1/aw")]),
            true,
        );
        assert_eq!(plain.api_origin, None);
        assert_eq!(plain.sign_in.methods, ["password"]);
        assert_eq!(plain.signup, "invite");
        let split_config = config(&[
            ("DATABASE_URL", "postgres://aw:pw@127.0.0.1/aw"),
            ("PUBLIC_ORIGIN", "https://api.example.com"),
            ("LIVE_ORIGIN", "https://live.example.com/"),
        ]);
        // A page this server served keeps to its own origin.
        assert_eq!(
            ClientConfig::from_config(&split_config, false).api_origin,
            None
        );
        let split = ClientConfig::from_config(&split_config, true);
        assert_eq!(split.api_origin.as_deref(), Some("https://api.example.com"));
        assert_eq!(
            split.live_origin.as_deref(),
            Some("https://live.example.com")
        );
        assert!(
            AppConfig::from_values(&BTreeMap::from([
                (
                    "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
                    "postgres://aw:pw@127.0.0.1/aw".to_owned()
                ),
                (
                    "AGENT_WORKSPACE_LIVE_ORIGIN".to_owned(),
                    "http://live.example.com".to_owned()
                ),
            ]))
            .is_err(),
            "the live origin is https, like the public one"
        );
    }

    /// The route's answer holds no configured secret: every value of a
    /// variable that names a secret, and every secret file's contents, are
    /// looked for in it.
    #[tokio::test]
    async fn client_config_holds_no_secret() {
        use std::os::unix::fs::PermissionsExt as _;

        use crate::{rate_limit::RateLimiters, realtime::RealtimeHub};

        let secrets = tempfile::tempdir().expect("secrets folder");
        let data_root = tempfile::tempdir().expect("data root");
        let web_root = tempfile::tempdir().expect("web root");
        let file = |name: &str, contents: &str| {
            let path = secrets.path().join(name);
            std::fs::write(&path, contents).expect("secret file");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .expect("owner only");
            path.display().to_string()
        };
        let mut values = BTreeMap::from([
            (
                "DATABASE_URL".to_owned(),
                "postgres://aw:database-password-marker@127.0.0.1/aw".to_owned(),
            ),
            (
                "PUBLIC_ORIGIN".to_owned(),
                "https://api.example.com".to_owned(),
            ),
            (
                "ALLOWED_ORIGINS".to_owned(),
                "https://app.example.com".to_owned(),
            ),
            ("WORKOS_CLIENT_ID".to_owned(), "client_marker".to_owned()),
            ("WORKOS_GOOGLE_ENABLED".to_owned(), "true".to_owned()),
            (
                "WORKOS_API_KEY_PATH".to_owned(),
                file("workos", "sk_workos-key-marker"),
            ),
            ("GITHUB_APP_ID".to_owned(), "12345".to_owned()),
            (
                "GITHUB_PRIVATE_KEY_PATH".to_owned(),
                file("github-key", "github-private-key-marker"),
            ),
            (
                "GITHUB_WEBHOOK_SECRET_PATH".to_owned(),
                file("github-webhook", "github-webhook-secret-marker"),
            ),
            ("GITHUB_APP_SLUG".to_owned(), "aw-test".to_owned()),
            ("GITHUB_CLIENT_ID".to_owned(), "Iv1.marker".to_owned()),
            (
                "GITHUB_CLIENT_SECRET_PATH".to_owned(),
                file("github-client", "github-client-secret-marker"),
            ),
            (
                "METRICS_TOKEN_PATH".to_owned(),
                file("metrics", "metrics-token-marker-0123456789abcdef"),
            ),
            (
                "DATA_ROOT".to_owned(),
                data_root.path().display().to_string(),
            ),
            ("WEB_ROOT".to_owned(), web_root.path().display().to_string()),
        ])
        .into_iter()
        .map(|(key, value)| (format!("AGENT_WORKSPACE_{key}"), value))
        .collect::<BTreeMap<_, _>>();
        values.insert(
            "AGENT_WORKSPACE_SECRETS_KEY_PATH".to_owned(),
            file("keyring", "keyring-marker"),
        );
        let config = AppConfig::from_values(&values).expect("config with every secret");
        let mut secret_values = vec!["database-password-marker".to_owned()];
        for (key, value) in &values {
            let secret = ["SECRET", "KEY", "TOKEN", "PASSWORD", "DATABASE_URL"]
                .iter()
                .any(|word| key.contains(word));
            if !secret {
                continue;
            }
            secret_values.push(value.clone());
            if key.ends_with("_PATH") {
                secret_values.push(std::fs::read_to_string(value).expect("secret file"));
            }
        }
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy(config.database_url.expose())
            .expect("lazy pool");
        let supervisors =
            crate::provider::SupervisorRegistry::new(&config).expect("supervisor registry");
        let application = crate::router(AppState {
            config: Arc::new(config),
            pool,
            rate_limits: RateLimiters::default(),
            realtime: RealtimeHub::new(8),
            supervisors,
        });
        let response = application
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/client-config")
                    .header(ORIGIN, "https://app.example.com")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], "public, max-age=60");
        assert_eq!(
            response.headers()[ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://app.example.com"
        );
        assert_eq!(
            response.headers()["cross-origin-resource-policy"],
            "same-site",
            "a page on another origin of the site loads the API's files"
        );
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body");
        let body = String::from_utf8(body.to_vec()).expect("text");
        assert!(
            body.contains("\"apiOrigin\":\"https://api.example.com\""),
            "{body}"
        );
        assert!(body.contains("\"google\""), "{body}");
        for secret in secret_values {
            let secret = secret.trim();
            assert!(!body.contains(secret), "client-config holds {secret}");
        }

        // The request token goes back only to an allowed web origin.
        let csrf = |origin: &'static str| {
            HttpRequest::builder()
                .uri("/api/v1/auth/csrf")
                .header(ORIGIN, origin)
                .header("cookie", "agent_workspace_csrf=csrf-marker")
                .body(Body::empty())
                .expect("request")
        };
        let response = application
            .clone()
            .oneshot(csrf("https://app.example.com"))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        let body = axum::body::to_bytes(response.into_body(), 1 << 16)
            .await
            .expect("body");
        assert_eq!(&body[..], br#"{"csrfToken":"csrf-marker"}"#);
        let refused = application
            .oneshot(csrf("https://evil.example"))
            .await
            .expect("response");
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    }

    async fn through_cors(origin: Option<&str>, method: Method) -> Response {
        let origins = WebOrigins(Arc::from(vec!["https://app.example.com".to_owned()]));
        let app = Router::new()
            .route("/x", get(|| async { "ok" }).post(|| async { "posted" }))
            .layer(axum::middleware::from_fn_with_state(origins, cors));
        let mut request = HttpRequest::builder().method(method.clone()).uri("/x");
        if let Some(origin) = origin {
            request = request.header(ORIGIN, origin);
        }
        if method == Method::OPTIONS {
            request = request.header(ACCESS_CONTROL_REQUEST_METHOD, "POST");
        }
        app.oneshot(request.body(Body::empty()).expect("request"))
            .await
            .expect("response")
    }

    #[tokio::test]
    async fn only_configured_web_origins_get_cors_headers() {
        let allowed = through_cors(Some("https://app.example.com"), Method::GET).await;
        assert_eq!(
            allowed.headers()[ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://app.example.com"
        );
        assert_eq!(allowed.headers()[ACCESS_CONTROL_ALLOW_CREDENTIALS], "true");
        let preflight = through_cors(Some("https://app.example.com"), Method::OPTIONS).await;
        assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
        assert!(
            preflight.headers()[ACCESS_CONTROL_ALLOW_HEADERS]
                .to_str()
                .expect("text")
                .contains("x-csrf-token")
        );
        for origin in [Some("https://evil.example"), Some("null"), None] {
            for method in [Method::GET, Method::OPTIONS] {
                let response = through_cors(origin, method).await;
                assert!(
                    !response.headers().contains_key(ACCESS_CONTROL_ALLOW_ORIGIN),
                    "{origin:?}"
                );
                assert!(
                    !response
                        .headers()
                        .contains_key(ACCESS_CONTROL_ALLOW_CREDENTIALS)
                );
            }
        }
    }
}
