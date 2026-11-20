//! A1.16: how the built-in host reaches the server it runs in. Its calls are
//! the public host protocol, the same HTTP requests a host elsewhere sends,
//! handed to the server's own router in process instead of over a socket.
//!
//! A command call (listing, fetching or answering commands) carries
//! [`BuiltinCaller`], a request extension, instead of a host token: it stands
//! for every workspace's built-in host row at once, so the install needs no
//! credential per workspace (ruling R24). An extension can't come over the
//! network, so no outside caller can claim it. Run reports carry the run's
//! own credential, minted with its start, like any host's.

use agent_workspace_host_protocol::{
    ClientError, HostClient, HttpRequest, HttpResponse, Transport, TransportFuture,
};
use std::sync::Arc;

use axum::body::Body;
use tower::ServiceExt;

use crate::AppState;

/// Marks a request as the built-in host's own (see the module's comment).
#[derive(Clone, Copy, Debug)]
pub struct BuiltinCaller;

/// The server's router, called in process.
#[derive(Clone)]
pub struct InProcess {
    router: axum::Router,
}

impl std::fmt::Debug for InProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InProcess")
    }
}

impl InProcess {
    pub fn new(state: &AppState) -> Self {
        Self {
            router: crate::router(state.clone()),
        }
    }
}

impl Transport for InProcess {
    fn send(&self, request: HttpRequest) -> TransportFuture<'_> {
        Box::pin(async move {
            // In process there is no socket: the loopback name stands for
            // the server itself (MCP checks the host it is called by).
            let mut builder = axum::http::Request::builder()
                .method(request.method)
                .uri(&request.path)
                .header("host", "localhost");
            for (name, value) in &request.headers {
                builder = builder.header(*name, value);
            }
            if request.body.is_some() {
                builder = builder.header("content-type", "application/json");
            }
            let mut built = builder
                .body(Body::from(request.body.unwrap_or_default()))
                .map_err(|error| ClientError::Http(error.to_string()))?;
            built.extensions_mut().insert(BuiltinCaller);
            let response = self
                .router
                .clone()
                .oneshot(built)
                .await
                .map_err(|error| ClientError::Http(error.to_string()))?;
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
                })
                .collect();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .map_err(|error| ClientError::Http(error.to_string()))?
                .to_vec();
            Ok(HttpResponse {
                status,
                body,
                headers,
            })
        })
    }
}

/// The built-in host's client: every builtin row's commands, in process,
/// or, in the host's own program (A1.17), over HTTP to the server with the
/// install's token.
pub fn client(state: &AppState) -> HostClient {
    if let Some(remote) = &state.config.builtin_host_remote {
        // The program made the same client once at its start, so this
        // can't fail here.
        return HostClient::new(&remote.server_url, remote.token.expose())
            .expect("the built-in host's client was made at its start");
    }
    let origin = state
        .config
        .public_origin
        .clone()
        .unwrap_or_else(|| "http://builtin.invalid".to_owned());
    HostClient::with_transport(origin, String::new(), Arc::new(InProcess::new(state)))
}
