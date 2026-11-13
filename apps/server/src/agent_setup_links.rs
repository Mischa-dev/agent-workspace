//! AWD-63: a one-time link that sets an agent up in the app it runs in.
//!
//! A person who manages agents asks for a link for one connection and the
//! app it runs in (`external_agents` makes it, under the same checks as a
//! token rotation, which it is). The fresh token waits in the secrets store.
//! The person pastes one setup message into the app; the agent fetches the
//! link, gets its token and the steps for that app, and the link is spent:
//! the secret is deleted and the link never answers again. Links last 30
//! minutes, and a rotated, disabled or removed agent's open links go at once.

use axum::{
    Router,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand_core::{OsRng, RngCore};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    secrets::{self, NewSecret, SecretOwner, SecretValue},
};

/// How long a link waits to be opened.
pub const LIFETIME_MINUTES: i64 = 30;
const CODE_BYTES: usize = 32;

/// The apps a link writes its steps for.
pub const APPS: [&str; 3] = ["claude_code", "codex", "other"];

/// What the person gets back: the link to paste, and when it lapses.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupLinkView {
    pub url: String,
    pub app: String,
    pub expires_at: String,
}

fn code_hash(code: &str) -> [u8; 32] {
    Sha256::digest(code.as_bytes()).into()
}

fn secret_name(link_id: Uuid) -> String {
    format!("agent_setup:{link_id}")
}

/// Store a link for `token`, which the caller has just issued to the
/// connection in this transaction.
#[allow(clippy::too_many_arguments)]
pub async fn store(
    transaction: &mut Transaction<'_, Postgres>,
    config: &crate::config::AppConfig,
    origin: &str,
    organization_id: Uuid,
    project_id: Uuid,
    connection_id: Uuid,
    actor_id: Uuid,
    app: &str,
    token: &str,
) -> Result<SetupLinkView, AppError> {
    if !APPS.contains(&app) {
        return Err(AppError::Validation("app"));
    }
    let keys = secrets::keyring(config)?;
    let mut bytes = [0_u8; CODE_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let code = URL_SAFE_NO_PAD.encode(bytes);
    let link_id = Uuid::now_v7();
    let stored = secrets::put(
        transaction,
        keys.as_ref(),
        NewSecret {
            owner: SecretOwner::Project {
                organization_id,
                project_id,
            },
            name: &secret_name(link_id),
            description: Some("An agent's token, waiting for its one-time setup link."),
            value: &SecretValue::new(token),
            actor_id: Some(actor_id),
            system_managed: true,
        },
    )
    .await?;
    let expires_at = sqlx::query_scalar::<_, String>(
        r#"
        INSERT INTO agent_setup_links (
            id, project_id, connection_id, code_hash, secret_id, app,
            created_by_actor_id, expires_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, now() + make_interval(mins => $8))
        RETURNING to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')
        "#,
    )
    .bind(link_id)
    .bind(project_id)
    .bind(connection_id)
    .bind(code_hash(&code).as_slice())
    .bind(stored.id)
    .bind(app)
    .bind(actor_id)
    .bind(i32::try_from(LIFETIME_MINUTES).unwrap_or(30))
    .fetch_one(&mut **transaction)
    .await?;
    Ok(SetupLinkView {
        url: format!("{origin}/connect/agent/{code}"),
        app: app.to_owned(),
        expires_at,
    })
}

/// Spend every open link of a connection whose token is changing or going
/// away, and delete the tokens they held.
pub async fn close_for_connection(
    connection: &mut PgConnection,
    connection_id: Uuid,
) -> Result<(), AppError> {
    let secrets = sqlx::query_scalar::<_, Uuid>(
        r#"
        WITH open AS (
            SELECT id, secret_id FROM agent_setup_links
            WHERE connection_id = $1 AND secret_id IS NOT NULL
            FOR UPDATE
        )
        UPDATE agent_setup_links link
        SET claimed_at = COALESCE(link.claimed_at, now()), secret_id = NULL
        FROM open WHERE link.id = open.id
        RETURNING open.secret_id
        "#,
    )
    .bind(connection_id)
    .fetch_all(&mut *connection)
    .await?;
    for secret_id in secrets {
        secrets::delete(&mut *connection, secret_id).await?;
    }
    Ok(())
}

/// Delete the tokens of links that lapsed unopened.
pub async fn purge_expired(connection: &mut PgConnection) -> Result<(), AppError> {
    let lapsed = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT id, secret_id FROM agent_setup_links
        WHERE secret_id IS NOT NULL AND expires_at <= now()
        ORDER BY expires_at
        LIMIT 200
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .fetch_all(&mut *connection)
    .await?;
    for (link_id, secret_id) in lapsed {
        sqlx::query("UPDATE agent_setup_links SET secret_id = NULL WHERE id = $1")
            .bind(link_id)
            .execute(&mut *connection)
            .await?;
        secrets::delete(&mut *connection, secret_id).await?;
    }
    Ok(())
}

pub fn router() -> Router<AppState> {
    // A HEAD (a link checker, a client peeking first) would otherwise run
    // the GET handler and spend the link without delivering the token.
    Router::new().route("/connect/agent/{code}", get(claim).head(peek))
}

/// Answers a HEAD without touching the link.
async fn peek() -> Response {
    markdown(StatusCode::OK, String::new())
}

#[derive(FromRow)]
struct OpenLink {
    id: Uuid,
    secret_id: Uuid,
    app: String,
    display_name: String,
    handle: String,
    project_name: String,
}

fn markdown(status: StatusCode, body: String) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
            (header::REFERRER_POLICY, "no-referrer"),
            (
                header::HeaderName::from_static("x-robots-tag"),
                "noindex, nofollow",
            ),
        ],
        body,
    )
        .into_response()
}

const SPENT: &str = "# This setup link has been used or has expired\n\n\
A setup link works once, for 30 minutes. Ask your person to make a new one \
in Agents (Set up in an app) and paste you the new message.\n";

/// The agent opens its link: the token and its app's steps, once.
async fn claim(State(state): State<AppState>, Path(code): Path<String>) -> Response {
    match claim_inner(&state, &code).await {
        Ok(Some(body)) => markdown(StatusCode::OK, body),
        Ok(None) => markdown(StatusCode::GONE, SPENT.to_owned()),
        Err(error) => {
            tracing::warn!(category = error.log_category(), "agent setup link failed");
            markdown(
                StatusCode::SERVICE_UNAVAILABLE,
                "# Setup didn't finish\n\nTry the link again in a minute.\n".to_owned(),
            )
        }
    }
}

async fn claim_inner(state: &AppState, code: &str) -> Result<Option<String>, AppError> {
    if code.is_empty() || code.len() > 64 {
        return Ok(None);
    }
    let mut transaction = state.pool.begin().await?;
    purge_expired(&mut transaction).await?;
    let Some(link) = sqlx::query_as::<_, OpenLink>(
        r#"
        SELECT link.id, link.secret_id, link.app, member.display_name, member.handle,
               project.name AS project_name
        FROM agent_setup_links link
        JOIN external_agent_connections connection ON connection.id = link.connection_id
        JOIN project_memberships member
          ON member.project_id = connection.project_id AND member.actor_id = connection.actor_id
        JOIN projects project ON project.id = link.project_id
        WHERE link.code_hash = $1 AND link.claimed_at IS NULL
          AND link.secret_id IS NOT NULL AND link.expires_at > now()
          AND connection.status = 'active'
        FOR UPDATE OF link
        "#,
    )
    .bind(code_hash(code).as_slice())
    .fetch_optional(&mut *transaction)
    .await?
    else {
        transaction.commit().await?;
        return Ok(None);
    };
    let keys = secrets::keyring(&state.config)?;
    let token = secrets::reveal(
        &mut *transaction,
        keys.as_ref(),
        link.secret_id,
        "agent_setup_link.claim",
    )
    .await?;
    sqlx::query("UPDATE agent_setup_links SET claimed_at = now(), secret_id = NULL WHERE id = $1")
        .bind(link.id)
        .execute(&mut *transaction)
        .await?;
    secrets::delete(&mut *transaction, link.secret_id).await?;
    let origin = crate::events::public_origin(state);
    let body = setup_steps(&origin, &link, token.expose());
    transaction.commit().await?;
    Ok(Some(body))
}

/// The steps for the link's app, with the token in them.
fn setup_steps(origin: &str, link: &OpenLink, token: &str) -> String {
    let mcp = format!("{origin}/api/v1/mcp");
    let app_steps = match link.app.as_str() {
        "claude_code" => format!(
            "Run this once. It stores the token in Claude Code's own config file:\n\n\
```bash\nclaude mcp add --scope user --transport http agent-workspace {mcp} --header \"Authorization: Bearer {token}\"\n```\n\n\
Tools load when a session starts, so start a new session afterwards (or run `/mcp` and reconnect **agent-workspace**).\n"
        ),
        "codex" => format!(
            "1. Save the token where only you can read it, for example as `AGENT_WORKSPACE_MCP_TOKEN` in the environment Codex starts in:\n\n\
```bash\nexport AGENT_WORKSPACE_MCP_TOKEN='{token}'\n```\n\n\
2. Add this to `~/.codex/config.toml`, then restart Codex:\n\n\
```toml\n[mcp_servers.agent_workspace]\nurl = \"{mcp}\"\nbearer_token_env_var = \"AGENT_WORKSPACE_MCP_TOKEN\"\n```\n"
        ),
        _ => format!(
            "Point your MCP client at the server (Streamable HTTP) with this header on every request:\n\n\
```\nAuthorization: Bearer {token}\n```\n\n\
- MCP server: `{mcp}`\n\
- Name it `agent-workspace`, or `agent_workspace` where hyphens aren't allowed.\n"
        ),
    };
    format!(
        "# You're {name} (@{handle}) in Agent Workspace\n\n\
Your person set you up to work in **{project}**. This page shows your token once; \
the link has now been used and won't work again.\n\n\
Keep the token secret: never put it in messages, logs, or files in a repository. \
It stops working if your person makes you a new one.\n\n\
## Connect\n\n{app_steps}\n\
## Then\n\n\
1. Call `briefing`. It lists what needs you.\n\
2. Read `{origin}/connect.md` (from \"Once you're connected\") for how work goes here.\n\
3. Tell your person you're connected.\n",
        name = link.display_name,
        handle = link.handle,
        project = link.project_name,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(app: &str) -> OpenLink {
        OpenLink {
            id: Uuid::nil(),
            secret_id: Uuid::nil(),
            app: app.to_owned(),
            display_name: "Scout".to_owned(),
            handle: "scout".to_owned(),
            project_name: "Work".to_owned(),
        }
    }

    #[test]
    fn each_app_gets_its_own_steps_with_the_token() {
        let claude = setup_steps("https://w.test", &link("claude_code"), "aw_ext_x");
        assert!(claude.contains("claude mcp add"));
        assert!(claude.contains("Bearer aw_ext_x"));
        assert!(claude.contains("@scout"));
        let codex = setup_steps("https://w.test", &link("codex"), "aw_ext_x");
        assert!(codex.contains("bearer_token_env_var"));
        assert!(codex.contains("aw_ext_x"));
        let other = setup_steps("https://w.test", &link("other"), "aw_ext_x");
        assert!(other.contains("https://w.test/api/v1/mcp"));
    }

    #[test]
    fn codes_are_stored_hashed() {
        assert_eq!(code_hash("abc").len(), 32);
        assert_ne!(code_hash("abc"), code_hash("abd"));
    }
}
