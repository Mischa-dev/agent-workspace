//! Framed panels (M1.15): the escape hatch for what blocks can't draw, such
//! as a field for a secret. A panel's answer may be a frame: an address on
//! one of the origins the module's manifest declares (and an admin approved
//! with it). Workspace never frames that address itself. It records a
//! launch, and the viewer's page loads Workspace's host page for it,
//! `/panel-frame/{launch}`, once. That page runs no script, has a policy
//! that frames only the app's origin, and is the only place the launch
//! token appears: in the app's address it frames. The app redeems the token
//! server to server, once, within five minutes (`http::redeem_frame`), which
//! tells it who it is drawing for. When the frame says it's done, the web
//! calls `module_frame_finish`, and Workspace asks the app's panel for the
//! result itself; nothing the frame's page says is trusted on its own.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{Duration, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::AppRow;
use super::panels::{PanelActionInput, PanelThingInput, PanelView, Request, round_trip};
use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::AppError,
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Target, Targeted, refs::IdRef},
};

/// How long a launch waits to be opened and its token redeemed.
const LAUNCH_MINUTES: i64 = 5;
/// The action a panel's entry hears when the frame it opened says it's
/// done: `values` carries the launch and the frame's `ref`.
pub const FRAME_DONE_ACTION: &str = "aw.frame_done";
const THE_FRAME: &str = "That frame";

/// The host page's own stylesheet: the frame fills it.
const HOST_CSS: &str = "html,body{margin:0;padding:0;height:100%;background:transparent}\
iframe{display:block;border:0;width:100%;height:100%}\
p{font:14px/1.4 system-ui,sans-serif;margin:16px;color:#595959}\
[data-scheme=dark] p{color:#a3a3a3}";

/// What a panel's frame answer becomes.
pub(super) struct NewLaunch<'a> {
    pub account: Uuid,
    pub panel: &'a str,
    pub space: Option<Uuid>,
    pub thing: Option<(&'a str, Uuid)>,
    pub url: url::Url,
    pub title: String,
    pub height: u16,
}

pub(super) struct Launched {
    pub id: Uuid,
    pub address: String,
    pub origin: String,
    pub title: String,
    pub height: u16,
}

/// The host page's address for a launch.
pub fn host_address(launch: Uuid) -> String {
    format!("/panel-frame/{launch}")
}

/// Records a launch for a frame `install`'s panel answered with, after
/// checking its origin is one the install's manifest declares.
pub(super) async fn launch(
    state: &AppState,
    install: &AppRow,
    new: NewLaunch<'_>,
) -> Result<Launched, OpError> {
    let origin = new.url.origin().ascii_serialization();
    let declared = install
        .manifest()
        .map_err(|error| OpError::logged("module_panel_open", &error))?
        .frame_origins;
    if !declared
        .iter()
        .any(|known| known.trim_end_matches('/') == origin)
    {
        return Err(OpError::Invalid {
            field: "answer".into(),
            message: "The app asked to frame an address outside the frame origins its \
                      manifest declares."
                .into(),
        });
    }
    let id = Uuid::now_v7();
    let mut transaction = crate::db::scoped_tx(&state.pool)
        .await
        .map_err(|error| OpError::logged("module_panel_open", &error.into()))?;
    sqlx::query(
        "INSERT INTO module_frame_launches \
         (id, workspace_id, space_id, module_id, account_id, panel, thing_kind, thing_id, \
          url, origin, title, height, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(id)
    .bind(install.workspace_id)
    .bind(new.space)
    .bind(install.module_id)
    .bind(new.account)
    .bind(new.panel)
    .bind(new.thing.map(|(kind, _)| kind))
    .bind(new.thing.map(|(_, id)| id))
    .bind(new.url.as_str())
    .bind(&origin)
    .bind(&new.title)
    .bind(i32::from(new.height))
    .bind(Utc::now() + Duration::minutes(LAUNCH_MINUTES))
    .execute(&mut *transaction)
    .await
    .map_err(|error| OpError::logged("module_panel_open", &error.into()))?;
    transaction
        .commit()
        .await
        .map_err(|error| OpError::logged("module_panel_open", &error.into()))?;
    Ok(Launched {
        id,
        address: host_address(id),
        origin,
        title: new.title,
        height: new.height,
    })
}

/// Text for an HTML attribute or element.
fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// The host page's policy: nothing but the app's one origin in a frame, no
/// script, its own stylesheet, framed only by Workspace's own pages.
pub fn host_policy(origin: Option<&str>) -> String {
    let frames = origin.map_or_else(|| "'none'".to_owned(), str::to_owned);
    format!(
        "default-src 'none'; frame-src {frames}; script-src 'none'; style-src 'self'; \
         base-uri 'none'; form-action 'none'; frame-ancestors 'self'"
    )
}

fn host_response(status: StatusCode, origin: Option<&str>, body: String) -> Response {
    let mut response = (status, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // The origin is a serialized origin the manifest check accepted, so the
    // policy is plain ASCII.
    if let Ok(policy) = HeaderValue::from_str(&host_policy(origin)) {
        headers.insert(header::CONTENT_SECURITY_POLICY, policy);
    } else {
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; frame-ancestors 'self'"),
        );
    }
    headers.insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("SAMEORIGIN"),
    );
    response
}

/// The light or dark the viewer's page shows. The host page takes the same,
/// so the browser keeps the frame see-through instead of painting it white
/// on a dark page.
#[derive(Debug, Default, Deserialize)]
pub(super) struct HostQuery {
    scheme: Option<String>,
}

impl HostQuery {
    fn scheme(&self) -> &'static str {
        match self.scheme.as_deref() {
            Some("dark") => "dark",
            _ => "light",
        }
    }
}

fn head(scheme: &str, title: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\" data-scheme=\"{scheme}\"><head><meta charset=\"utf-8\">\
         <meta name=\"color-scheme\" content=\"{scheme}\">\
         <meta name=\"referrer\" content=\"no-referrer\">\
         <title>{title}</title><link rel=\"stylesheet\" href=\"/panel-frame.css\"></head>"
    )
}

/// The page for one launch, with the app's address and its token.
fn host_page(scheme: &str, address: &str, title: &str) -> String {
    let title = escaped(title);
    format!(
        "{head}\
         <body><iframe src=\"{src}\" title=\"{title}\" \
         sandbox=\"allow-scripts allow-forms allow-same-origin allow-popups \
         allow-popups-to-escape-sandbox\" referrerpolicy=\"no-referrer\" allow=\"\">\
         </iframe></body></html>",
        head = head(scheme, &title),
        src = escaped(address),
    )
}

fn gone(scheme: &str) -> Response {
    host_response(
        StatusCode::GONE,
        None,
        format!(
            "{head}<body><p>This frame was opened already, or it lapsed. Open the panel \
             again.</p></body></html>",
            head = head(scheme, "Frame closed"),
        ),
    )
}

/// `GET /panel-frame/{launch}`: the host page, for the viewer the launch is
/// for, once. Opening it mints the launch token; only its digest is kept.
pub(super) async fn open_host(
    State(state): State<AppState>,
    Path(launch_id): Path<Uuid>,
    Query(query): Query<HostQuery>,
    principal: AuthenticatedActor,
) -> Result<Response, AppError> {
    let scheme = query.scheme();
    let token = crate::auth::random_token();
    let digest = Sha256::digest(token.as_bytes()).to_vec();
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let opened = sqlx::query_as::<_, (String, String, String)>(
        "UPDATE module_frame_launches launch SET opened_at = now(), token_digest = $3 \
         WHERE launch.id = $1 AND launch.account_id = $2 AND launch.opened_at IS NULL \
           AND launch.expires_at > now() \
           AND EXISTS (SELECT 1 FROM module_apps app \
                       WHERE app.module_id = launch.module_id \
                         AND app.workspace_id = launch.workspace_id \
                         AND app.state = 'active' \
                         AND launch.origin = ANY(app.frame_origins)) \
         RETURNING launch.url, launch.origin, launch.title",
    )
    .bind(launch_id)
    .bind(principal.actor_id)
    .bind(&digest)
    .fetch_optional(&mut *transaction)
    .await?;
    transaction.commit().await?;
    let Some((url, origin, title)) = opened else {
        return Ok(gone(scheme));
    };
    let Ok(mut address) = url::Url::parse(&url) else {
        return Ok(gone(scheme));
    };
    address.query_pairs_mut().append_pair("aw_launch", &token);
    Ok(host_response(
        StatusCode::OK,
        Some(&origin),
        host_page(scheme, address.as_str(), &title),
    ))
}

/// `GET /panel-frame.css`.
pub(super) async fn host_css() -> Response {
    let mut response = HOST_CSS.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    response
}

/// What a redeemed token tells the app.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Redeemed {
    pub launch: Uuid,
    pub workspace: Value,
    pub space: Option<Value>,
    pub viewer: Value,
    pub panel: String,
    pub thing: Option<Value>,
}

/// Redeems `token` for `module_id`'s launch, once, before it lapses.
pub(super) async fn redeem(
    connection: &mut sqlx::PgConnection,
    workspace: Uuid,
    module_id: Uuid,
    token: &str,
) -> Result<Option<Redeemed>, AppError> {
    let digest = Sha256::digest(token.trim().as_bytes()).to_vec();
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            Option<Uuid>,
            Uuid,
            String,
            Option<String>,
            Option<Uuid>,
        ),
    >(
        "UPDATE module_frame_launches SET redeemed_at = now() \
         WHERE token_digest = $1 AND workspace_id = $2 AND module_id = $3 \
           AND redeemed_at IS NULL AND opened_at IS NOT NULL AND expires_at > now() \
         RETURNING id, space_id, account_id, panel, thing_kind, thing_id",
    )
    .bind(&digest)
    .bind(workspace)
    .bind(module_id)
    .fetch_optional(&mut *connection)
    .await?;
    Ok(
        row.map(|(launch, space, account, panel, kind, thing)| Redeemed {
            launch,
            workspace: json!({ "id": workspace }),
            space: space.map(|id| json!({ "id": id })),
            viewer: json!({ "account": account }),
            panel,
            thing: kind
                .zip(thing)
                .map(|(kind, id)| json!({ "kind": kind, "id": id })),
        }),
    )
}

// ------------------------------------------------------------ finishing

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleFrameFinishInput {
    /// The launch the panel's frame answer gave.
    pub launch_id: IdRef,
    /// What the frame's `done` message carried, passed to the app's panel
    /// as it was.
    #[serde(rename = "ref")]
    pub reference: String,
}

impl Targeted for ModuleFrameFinishInput {
    fn target(&self) -> Target {
        Target::thing("frame_launch", self.launch_id.id())
    }
}

pub struct ModuleFrameFinish;

impl Operation for ModuleFrameFinish {
    const NAME: &'static str = "module_frame_finish";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    // The frame is the caller's own; the app decides what finishing means.
    const PERMISSION: Option<Perm> = None;
    // The app receives the request id and keeps it; a retry asks it again.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const AUTOMATION: bool = false;
    const EMITS: &'static [&'static str] = &["module.panel_pressed"];
    const TITLE: &'static str = "Finish a connected module's frame";
    const DESCRIPTION: &'static str = "When an outside app's framed page says it's done, ask \
        the app's panel for the result: it answers as a panel does (the panel to draw next, \
        done, or close). The frame must be yours, and the app must have redeemed its launch.";

    type Input = ModuleFrameFinishInput;
    type Output = PanelView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ModuleFrameFinishInput,
    ) -> Result<PanelView, OpError> {
        let reference = input.reference.trim();
        if reference.is_empty()
            || reference.chars().count() > 200
            || reference.chars().any(char::is_control)
        {
            return Err(OpError::invalid(
                "ref",
                "A short reference, 200 characters at most.",
            ));
        }
        let launch = input.launch_id.id();
        let account = call.ctx.caller.account_id;
        let workspace = call.ctx.workspace_id;
        let found = crate::db::scoped_one(&call.state.pool, async |connection| {
            Ok::<_, AppError>(
                sqlx::query_as::<_, (Uuid, String, Option<Uuid>, Option<String>, Option<Uuid>)>(
                    "SELECT module_id, panel, space_id, thing_kind, thing_id \
                     FROM module_frame_launches \
                     WHERE id = $1 AND account_id = $2 AND workspace_id = $3 \
                       AND redeemed_at IS NOT NULL AND finished_at IS NULL",
                )
                .bind(launch)
                .bind(account)
                .bind(workspace)
                .fetch_optional(&mut *connection)
                .await?,
            )
        })
        .await
        .map_err(|error| OpError::logged(Self::NAME, &error))?;
        let (module_id, panel, space, thing_kind, thing_id) =
            found.ok_or_else(|| OpError::not_found(THE_FRAME))?;
        let thing = thing_kind.zip(thing_id).map(|(kind, id)| PanelThingInput {
            kind,
            id: IdRef(id),
        });
        let mut values = Map::new();
        values.insert("launch".to_owned(), json!(launch));
        values.insert("ref".to_owned(), json!(reference));
        let action = PanelActionInput {
            id: FRAME_DONE_ACTION.to_owned(),
            values: Some(values),
        };
        let view = round_trip(
            call,
            Request {
                module_id,
                panel: &panel,
                space,
                thing: thing.as_ref(),
                state: None,
                action: Some(&action),
            },
        )
        .await?;
        let tx = call.tx().await?;
        let finished = sqlx::query(
            "UPDATE module_frame_launches SET finished_at = now() \
             WHERE id = $1 AND finished_at IS NULL",
        )
        .bind(launch)
        .execute(&mut **tx)
        .await
        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?
        .rows_affected();
        if finished == 0 {
            return Err(OpError::not_found(THE_FRAME));
        }
        let mut pressed = crate::events::NewEvent::thing(
            &crate::events::catalog::MODULE_PANEL_PRESSED,
            workspace,
            "",
            Uuid::nil(),
        )
        .audience(crate::events::Audience::Account(account))
        .workspace(workspace)
        .actor(account)
        .payload(json!({
            "moduleId": module_id,
            "panel": panel,
            "action": FRAME_DONE_ACTION,
        }));
        pressed.thing = None;
        pressed.space_id = None;
        crate::events::emit(tx, pressed).await?;
        call.audit_target(
            "module",
            module_id,
            json!({ "panel": panel, "action": FRAME_DONE_ACTION, "launch": launch }),
        );
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_page_frames_one_origin_and_runs_nothing() {
        let policy = host_policy(Some("https://keys.example"));
        assert!(policy.contains("default-src 'none'"));
        assert!(policy.contains("frame-src https://keys.example;"));
        assert!(policy.contains("script-src 'none'"));
        assert!(policy.contains("style-src 'self'"));
        assert!(policy.contains("frame-ancestors 'self'"));
        assert_eq!(
            policy.matches("https://").count(),
            1,
            "one origin: {policy}"
        );
        assert!(host_policy(None).contains("frame-src 'none'"));
        let page = host_page(
            "dark",
            "https://keys.example/aw?aw_launch=abc&x=\"><script>",
            "Keys <b>",
        );
        assert!(!page.contains("<script"), "{page}");
        assert!(page.contains("sandbox=\"allow-scripts allow-forms allow-same-origin"));
        assert!(!page.contains("allow-top-navigation"));
        assert!(page.contains("allow=\"\""));
        assert!(page.contains("Keys &lt;b&gt;"));
        assert!(page.contains("<meta name=\"color-scheme\" content=\"dark\">"));
        let query = |scheme: &str| HostQuery {
            scheme: Some(scheme.to_owned()),
        };
        assert_eq!(query("dark").scheme(), "dark");
        assert_eq!(query("\"><x").scheme(), "light");
        assert_eq!(HostQuery::default().scheme(), "light");
    }
}
