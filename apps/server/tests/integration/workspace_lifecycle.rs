//! H1.24: a workspace can be suspended, resumed, scheduled for deletion,
//! kept, and deleted. A suspended or deleting workspace refuses its agents
//! with `workspace_suspended`; deleting one removes every row naming it and
//! its file bytes, and leaves workspace B exactly as it was.

use std::collections::{BTreeMap, HashSet};

use agent_workspace_server::workspace::lifecycle::{self, OperatorAction, SuspendReason};
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    agent_interface::{Browser, initialize, send},
    support::two_workspaces::{TwoWorkspaces, sweep_ids, two_workspaces},
    workspace_isolation::{call_tool, structured},
};

/// One of the agent's tools, as its agent sees it: the raw body.
async fn agent_task_list(t: &TwoWorkspaces, token: &str) -> String {
    let session = initialize(&t.app, token).await.expect("agent MCP");
    let (status, body) = call_tool(&t.app, token, &session, "task_list", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

fn refused_as_suspended(body: &str) -> bool {
    structured(body)["code"] == "workspace_suspended"
        || structured(body)["outcome"] == "workspace_suspended"
        || (body.contains("suspended") && !structured(body)["outcome"].eq("ok"))
}

async fn op(
    t: &TwoWorkspaces,
    browser: &Browser,
    workspace: Uuid,
    name: &str,
    input: Value,
) -> (StatusCode, Value) {
    let key = Uuid::now_v7().to_string();
    send(
        &t.app,
        browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/{name}"),
            Some(&key),
            input,
        ),
    )
    .await
}

async fn slug_of(pool: &PgPool, workspace: Uuid) -> String {
    sqlx::query_scalar("SELECT slug FROM organizations WHERE id = $1")
        .bind(workspace)
        .fetch_one(pool)
        .await
        .expect("slug")
}

/// Every row whose `id` is one of `ids`, per table, as text: what "B is
/// untouched" compares.
async fn digest(pool: &PgPool, ids: &HashSet<String>) -> BTreeMap<String, String> {
    let ids = ids
        .iter()
        .filter_map(|id| Uuid::parse_str(id).ok())
        .collect::<Vec<_>>();
    let tables = sqlx::query_scalar::<_, String>(
        r#"
        SELECT columns.table_name::text
        FROM information_schema.columns columns
        JOIN information_schema.tables listed
          ON listed.table_schema = columns.table_schema
         AND listed.table_name = columns.table_name
         AND listed.table_type = 'BASE TABLE'
        WHERE columns.table_schema = current_schema()
          AND columns.column_name = 'id' AND columns.data_type = 'uuid'
        ORDER BY 1
        "#,
    )
    .fetch_all(pool)
    .await
    .expect("tables");
    let mut digest = BTreeMap::new();
    for table in tables {
        let text = sqlx::query_scalar::<_, Option<String>>(&format!(
            "SELECT md5(string_agg(row_data::text, '|' ORDER BY row_data.id)) \
             FROM {table} row_data WHERE row_data.id = ANY($1)"
        ))
        .bind(&ids)
        .fetch_one(pool)
        .await
        .expect("digest");
        if let Some(text) = text {
            digest.insert(table, text);
        }
    }
    digest
}

#[tokio::test]
async fn a_workspace_is_suspended_kept_and_deleted_without_touching_another() {
    let Some(t) = two_workspaces("workspace_lifecycle").await else {
        return;
    };
    let (a, b) = (&t.a, &t.b);

    // Open: the agent works.
    let body = agent_task_list(&t, &a.agent_token).await;
    assert!(!refused_as_suspended(&body), "{body}");

    // Suspended by the operator: A's agent is refused, B's isn't.
    let a_slug = slug_of(&t.pool, a.organization).await;
    lifecycle::operator_command(
        &t.pool,
        &a_slug,
        OperatorAction::Suspend(SuspendReason::Operator),
    )
    .await
    .expect("suspend");
    let body = agent_task_list(&t, &a.agent_token).await;
    assert!(refused_as_suspended(&body), "{body}");
    let body = agent_task_list(&t, &b.agent_token).await;
    assert!(!refused_as_suspended(&body), "{body}");
    // An operation in A is refused too, with the code and 423.
    let (status, body) = op(
        &t,
        &a.browser,
        a.organization,
        "workspace_delete_cancel",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::LOCKED, "{body}");
    assert_eq!(body["outcome"], "workspace_suspended", "{body}");
    // The browser's older routes refuse writes too, and still answer reads.
    let (status, body) = send(
        &t.app,
        a.browser.request(
            "PATCH",
            &format!("/api/v1/organizations/{}", a.organization),
            None,
            json!({"name": "Renamed while suspended"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::LOCKED, "{body}");
    let (status, body) = send(
        &t.app,
        a.browser.request(
            "GET",
            &format!("/api/v1/organizations/{}", a.organization),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Open screens hear of it.
    let announced = sqlx::query_scalar::<_, String>(
        "SELECT payload->>'lifecycle' FROM outbox_events \
         WHERE event_type = 'workspace.lifecycle.changed' AND payload->>'workspaceId' = $1 \
         ORDER BY event_cursor DESC LIMIT 1",
    )
    .bind(a.organization.to_string())
    .fetch_optional(&t.pool)
    .await
    .expect("lifecycle event");
    assert_eq!(announced.as_deref(), Some("suspended"));
    // An app connected for the owner's whole account gets past the MCP
    // gate (it may span workspaces), but its write lands in A and is
    // refused there.
    let session = initialize(&t.app, &a.app_token).await.expect("app MCP");
    let (_, body) = call_tool(
        &t.app,
        &a.app_token,
        &session,
        "project_create",
        json!({"key": "SUSP", "name": "Made while suspended"}),
    )
    .await;
    assert!(refused_as_suspended(&body), "{body}");
    let made = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM projects WHERE organization_id = $1 AND project_key = 'SUSP'",
    )
    .bind(a.organization)
    .fetch_one(&t.pool)
    .await
    .expect("projects");
    assert_eq!(made, 0);
    // A channel or thread named by id is A's too.
    let channel = sqlx::query_scalar::<_, Uuid>(
        "SELECT conversation.id FROM conversations conversation \
         JOIN projects space ON space.id = conversation.project_id \
         WHERE space.organization_id = $1 AND conversation.kind = 'channel' LIMIT 1",
    )
    .bind(a.organization)
    .fetch_one(&t.pool)
    .await
    .expect("a channel in A");
    let (status, body) = send(
        &t.app,
        a.browser.request(
            "POST",
            &format!("/api/v1/channels/{channel}/messages"),
            Some(&Uuid::now_v7().to_string()),
            json!({"content": {"version": 1, "nodes": [{"type": "text", "text": "hi"}]}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::LOCKED, "{body}");
    // The session says why.
    let (_, session) = send(
        &t.app,
        a.browser
            .request("GET", "/api/v1/auth/session", None, Value::Null),
    )
    .await;
    let mine = session["organizations"]
        .as_array()
        .expect("organizations")
        .iter()
        .find(|organization| organization["id"] == json!(a.organization))
        .cloned()
        .expect("A in the session");
    assert_eq!(mine["lifecycle"], "suspended");
    lifecycle::operator_command(&t.pool, &a_slug, OperatorAction::Resume)
        .await
        .expect("resume");
    let body = agent_task_list(&t, &a.agent_token).await;
    assert!(!refused_as_suspended(&body), "{body}");

    // Only someone holding "Delete the workspace" may ask, and only with the
    // address typed out.
    let (status, body) = op(
        &t,
        &a.browser,
        a.organization,
        "workspace_delete",
        json!({"confirm": "not-the-address"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = op(
        &t,
        &b.browser,
        a.organization,
        "workspace_delete",
        json!({"confirm": a_slug}),
    )
    .await;
    assert!(
        matches!(status, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN),
        "B's owner can't delete A: {status} {body}"
    );
    let (status, body) = op(
        &t,
        &a.browser,
        a.organization,
        "workspace_delete",
        json!({"confirm": a_slug}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["value"]["lifecycle"], "deleting", "{body}");
    assert!(body["value"]["deleteAfter"].is_string(), "{body}");
    // Deleting: everything is refused but keeping it.
    let body = agent_task_list(&t, &a.agent_token).await;
    assert!(refused_as_suspended(&body), "{body}");
    // Not yet due: the job leaves it alone.
    assert_eq!(
        lifecycle::delete_now(&t.state, a.organization, false)
            .await
            .expect("not due"),
        None
    );
    // Kept within the grace period.
    let (status, body) = op(
        &t,
        &a.browser,
        a.organization,
        "workspace_delete_cancel",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["value"]["lifecycle"], "active", "{body}");
    let queued = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM background_jobs WHERE kind = 'workspace.delete' AND state = 'queued'",
    )
    .fetch_one(&t.pool)
    .await
    .expect("jobs");
    assert_eq!(queued, 0, "cancelling cancels the job");
    let body = agent_task_list(&t, &a.agent_token).await;
    assert!(!refused_as_suspended(&body), "{body}");
    // A kept workspace isn't deleted even if the job runs.
    assert_eq!(
        lifecycle::delete_now(&t.state, a.organization, true)
            .await
            .expect("kept"),
        None
    );

    // Deleted for good.
    let b_ids = sweep_ids(&t.pool, b.organization).await;
    let b_before = digest(&t.pool, &b_ids).await;
    let a_owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 AND role = 'owner' LIMIT 1",
    )
    .bind(a.organization)
    .fetch_one(&t.pool)
    .await
    .expect("A's owner");
    // A host A owns raised a request in B: B keeps the request.
    let b_owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 AND role = 'owner' LIMIT 1",
    )
    .bind(b.organization)
    .fetch_one(&t.pool)
    .await
    .expect("B's owner");
    let host_account = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO actors (id, kind, status) VALUES (gen_random_uuid(), 'integration', 'active') RETURNING id",
    )
    .fetch_one(&t.pool)
    .await
    .expect("host account");
    let host = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO hosts (id, account_id, owner_workspace_id, name, protocol_version, \
         enrolled_by_account_id) VALUES (gen_random_uuid(), $1, $2, 'A host', '2026-11', $3) \
         RETURNING id",
    )
    .bind(host_account)
    .bind(a.organization)
    .bind(a_owner)
    .fetch_one(&t.pool)
    .await
    .expect("host");
    let b_request = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO requests (id, workspace_id, kind, raised_by_account_id, raised_via_host_id, \
         title, answer_permission) \
         VALUES (gen_random_uuid(), $1, 'question', $2, $3, 'Through A host', 'runs.answer') \
         RETURNING id",
    )
    .bind(b.organization)
    .bind(b_owner)
    .bind(host)
    .fetch_one(&t.pool)
    .await
    .expect("request in B");
    let (status, body) = op(
        &t,
        &a.browser,
        a.organization,
        "workspace_delete",
        json!({"confirm": a_slug}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let deleted = lifecycle::delete_now(&t.state, a.organization, true)
        .await
        .expect("delete")
        .expect("deleted");
    assert!(deleted.rows.contains_key("organizations"), "{deleted:?}");
    assert!(deleted.rows.contains_key("messages"), "{deleted:?}");

    // No row names A in any workspace-owned table.
    for (table, column) in agent_workspace_server::tenancy::tables::TENANT_TABLES {
        let left = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT count(*) FROM {table} WHERE {column} = $1"
        ))
        .bind(a.organization)
        .fetch_one(&t.pool)
        .await
        .expect("count");
        assert_eq!(left, 0, "{table} still names the deleted workspace");
    }
    for (table, column) in [
        ("organizations", "id"),
        ("hosts", "owner_workspace_id"),
        ("actors", "owner_workspace_id"),
        ("users", "default_organization_id"),
    ] {
        let left = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT count(*) FROM {table} WHERE {column} = $1"
        ))
        .bind(a.organization)
        .fetch_one(&t.pool)
        .await
        .expect("count");
        assert_eq!(
            left, 0,
            "{table}.{column} still names the deleted workspace"
        );
    }
    let projects_left =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM projects WHERE organization_id = $1")
            .bind(a.organization)
            .fetch_one(&t.pool)
            .await
            .expect("projects");
    assert_eq!(projects_left, 0);
    let recorded = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT requested_by_actor_id FROM deleted_workspaces WHERE id = $1",
    )
    .bind(a.organization)
    .fetch_one(&t.pool)
    .await
    .expect("the deletion is recorded");
    assert_eq!(recorded, Some(a_owner));
    // Accounts stay: the owner can still sign in elsewhere.
    let owner_left = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM actors WHERE id = $1")
        .bind(a_owner)
        .fetch_one(&t.pool)
        .await
        .expect("owner");
    assert_eq!(owner_left, 1);
    // No bytes: A's folder is gone, B's is there.
    let files = t.data_root.join("files");
    assert!(!files.join(a.organization.simple().to_string()).exists());
    assert!(files.join(b.organization.simple().to_string()).exists());

    // B's request is still there, no longer pointing at A's host.
    let via = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT raised_via_host_id FROM requests WHERE id = $1",
    )
    .bind(b_request)
    .fetch_one(&t.pool)
    .await
    .expect("B's request survives");
    assert_eq!(via, None);
    sqlx::query("DELETE FROM requests WHERE id = $1")
        .bind(b_request)
        .execute(&t.pool)
        .await
        .expect("set the request aside");

    // B: every row exactly as it was, and its agent still works.
    assert_eq!(sweep_ids(&t.pool, b.organization).await, b_ids);
    assert_eq!(digest(&t.pool, &b_ids).await, b_before);
    let body = agent_task_list(&t, &b.agent_token).await;
    assert!(!refused_as_suspended(&body), "{body}");
    let b_ok = structured(&body);
    assert!(b_ok.is_object(), "{body}");

    // Running the job again finds nothing to do.
    assert_eq!(
        lifecycle::delete_now(&t.state, a.organization, true)
            .await
            .expect("again"),
        None
    );
    t.finish().await;
}
