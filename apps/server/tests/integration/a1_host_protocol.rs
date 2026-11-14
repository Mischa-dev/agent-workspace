//! A1.11: hosts and enrolment. The silent failures: a code that works twice
//! or after it expired, a host acting for an agent nobody bound to it or
//! reaching a workspace through no agent of its own, a binding made by
//! someone who doesn't manage the host or the agent, a re-keyed agent
//! silently becoming another account, and an unlinked host still able to
//! call.

use agent_workspace_server::hosts;
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::{create_agent, send},
    mcp_admin::{create_project, setup},
    message_edits::add_person,
};

fn host_call(method: &str, uri: &str, token: Option<&str>, body: Value) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("aw-host-protocol", "2026-11")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    builder
        .body(Body::from(body.to_string()))
        .expect("host request")
}

async fn enrol(app: &axum::Router, code: &str, name: &str) -> (StatusCode, Value) {
    send(
        app,
        host_call(
            "POST",
            "/api/v1/hosts/enrol",
            None,
            json!({"code": code, "host": {"name": name, "software": "test-host", "version": "1.0"},
                   "protocolVersions": ["2026-11"], "connection": "dial_out"}),
        ),
    )
    .await
}

#[tokio::test]
async fn hosts_enrol_once_act_only_for_their_agents_and_stop_when_unlinked() {
    let Some(setup) = setup("a1_host_protocol").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (fixer, _) = create_agent(app, &setup.browser, &work, "Fixer", "fixer").await;
    let (scout, _) = create_agent(app, &setup.browser, &work, "Scout", "scout").await;
    let (mara, mara_browser) = add_person(&setup, &[&work], "mara").await;

    // Every workspace has its built-in host, which nobody unlinks here.
    let builtin = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM hosts WHERE owner_workspace_id = $1 AND builtin",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("built-in host");
    let (status, refused) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/hosts/{builtin}/unlink"),
            None,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");

    // A member without hosts.manage can't link one for the workspace, but
    // may link one of their own.
    let (status, refused) = send(
        app,
        mara_browser.request(
            "POST",
            "/api/v1/hosts/enrolments",
            None,
            json!({"workspaceId": setup.organization_id}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    let (status, personal) = send(
        app,
        mara_browser.request(
            "POST",
            "/api/v1/hosts/enrolments",
            None,
            json!({"label": "Mara's laptop"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{personal}");
    assert_eq!(personal["owner"]["kind"], "person");
    let personal_code = personal["code"].as_str().expect("code").to_owned();
    assert!(personal_code.starts_with("AWH-"), "{personal_code}");

    let (status, made) = send(
        app,
        setup.browser.request(
            "POST",
            "/api/v1/hosts/enrolments",
            None,
            json!({"workspaceId": setup.organization_id, "label": "Team host"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    assert_eq!(made["owner"]["kind"], "workspace");
    let team_code = made["code"].as_str().expect("code").to_owned();
    let stored = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM host_enrolments WHERE code_hash = sha256(convert_to($1, 'UTF8'))",
    )
    .bind(&team_code)
    .fetch_one(&setup.pool)
    .await
    .expect("code row");
    assert_eq!(stored, 1, "only the code's digest is kept");

    // A code works once, typed however.
    let typed = team_code.to_lowercase().replace('-', " ");
    let (status, team) = enrol(app, &typed, "Team host").await;
    assert_eq!(status, StatusCode::CREATED, "{team}");
    assert_eq!(team["owner"]["kind"], "workspace");
    assert_eq!(team["owner"]["id"], setup.organization_id.to_string());
    let team_host = team["hostId"]
        .as_str()
        .expect("host")
        .parse::<Uuid>()
        .expect("uuid");
    let team_token = team["credential"]["token"]
        .as_str()
        .expect("token")
        .to_owned();
    let (status, again) = enrol(app, &team_code, "Team host").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{again}");
    assert_eq!(again["error"], "enrolment_refused");
    let digest_only = sqlx::query_scalar::<_, bool>(
        "SELECT interim_token_digest = sha256(convert_to($2, 'UTF8')) FROM hosts WHERE id = $1",
    )
    .bind(team_host)
    .bind(&team_token)
    .fetch_one(&setup.pool)
    .await
    .expect("token digest");
    assert!(digest_only);

    // An expired code doesn't work.
    sqlx::query("UPDATE host_enrolments SET expires_at = now() - interval '1 second' WHERE owner_account_id = $1")
        .bind(mara)
        .execute(&setup.pool)
        .await
        .expect("expire");
    let (status, expired) = enrol(app, &personal_code, "Mara's laptop").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{expired}");
    let (status, personal) = send(
        app,
        mara_browser.request("POST", "/api/v1/hosts/enrolments", None, json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{personal}");
    let (status, mara_host) = enrol(
        app,
        personal["code"].as_str().expect("code"),
        "Mara's laptop",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{mara_host}");
    assert_eq!(mara_host["owner"]["kind"], "person");
    assert_eq!(mara_host["owner"]["id"], mara.to_string());
    let mara_host_id = mara_host["hostId"]
        .as_str()
        .expect("host")
        .parse::<Uuid>()
        .expect("uuid");
    let mara_token = mara_host["credential"]["token"]
        .as_str()
        .expect("token")
        .to_owned();

    // Describe stores what the host says, kept as it said it.
    let (status, described) = send(
        app,
        host_call(
            "POST",
            "/api/v1/host/describe",
            Some(&team_token),
            json!({
                "software": {"name": "test-host", "version": "1.1"},
                "issuer": "https://host.example",
                "capabilities": {"start": true, "cancel": true, "idempotentStart": true, "future": 1},
                "reportIntervalSeconds": 60,
                "offers": [{"id": "new", "title": "New agent", "summary": "Set one up",
                            "start": {"panel": "new-agent"}}],
                "actions": [{"id": "sign-ins", "label": "Model sign-ins", "needs": "panel"}],
                "agents": [
                    {"hostAgentRef": "fixer", "name": "Fixer",
                     "ref": {"issuer": "https://host.example", "id": "fixer", "lineage": "l-01"},
                     "backends": [{"id": "b1", "name": "Secret backend", "hidden": true}],
                     "actions": [{"id": "pause", "label": "Pause", "needs": "confirm"}]},
                    {"hostAgentRef": "scout", "name": "Scout"}
                ]
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{described}");
    assert_eq!(described["unboundAgents"], json!(["fixer", "scout"]));
    let (capabilities, offers, actions, described_agents) =
        sqlx::query_as::<_, (Value, Value, Value, Value)>(
            "SELECT capabilities, offers, actions, described_agents FROM hosts WHERE id = $1",
        )
        .bind(team_host)
        .fetch_one(&setup.pool)
        .await
        .expect("described");
    assert_eq!(capabilities["future"], 1, "unknown capabilities are kept");
    assert_eq!(offers[0]["start"]["panel"], "new-agent");
    assert_eq!(actions[0]["id"], "sign-ins");
    assert_eq!(
        described_agents[0]["backends"][0]["name"],
        Value::Null,
        "a hidden backend's name is never kept"
    );

    // Binding: the owner manages the team host and the agents.
    let (status, bound) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/hosts/{team_host}/agents"),
            None,
            json!({"accountId": fixer, "hostAgentRef": "fixer"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bound}");
    assert_eq!(bound["agents"][0]["hostAgentRef"], "fixer");
    assert_eq!(bound["agents"][0]["actions"][0]["id"], "pause");
    // Mara sees the team host but can't manage it.
    let (status, refused) = send(
        app,
        mara_browser.request(
            "POST",
            &format!("/api/v1/hosts/{team_host}/agents"),
            None,
            json!({"accountId": scout, "hostAgentRef": "scout"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    // Mara owns her host, but agents.manage is what lets her hand it an agent.
    let (status, refused) = send(
        app,
        mara_browser.request(
            "POST",
            &format!("/api/v1/hosts/{mara_host_id}/agents"),
            None,
            json!({"accountId": scout, "hostAgentRef": "scout"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    // The owner can't reach Mara's own host at all.
    let (status, hidden) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/hosts/{mara_host_id}/agents"),
            None,
            json!({"accountId": scout, "hostAgentRef": "scout"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{hidden}");

    // Acting for: only an agent bound to the host, only where it is a member.
    let mut connection = setup.pool.acquire().await.expect("connection");
    let acting = hosts::acting_for(&mut connection, team_host, fixer, setup.organization_id)
        .await
        .expect("acting for");
    assert!(acting.is_some());
    assert!(
        hosts::acting_for(&mut connection, team_host, scout, setup.organization_id)
            .await
            .expect("acting for")
            .is_none(),
        "scout isn't bound to the team host"
    );
    assert!(
        hosts::acting_for(&mut connection, mara_host_id, fixer, setup.organization_id)
            .await
            .expect("acting for")
            .is_none(),
        "Mara's host has no agents, so it reaches no workspace"
    );
    let elsewhere = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Elsewhere', $2)")
        .bind(elsewhere)
        .bind(format!("elsewhere-{}", elsewhere.simple()))
        .execute(&setup.pool)
        .await
        .expect("other workspace");
    assert!(
        hosts::acting_for(&mut connection, team_host, fixer, elsewhere)
            .await
            .expect("acting for")
            .is_none(),
        "not where the agent isn't a member"
    );
    drop(connection);

    // A notice about an agent that isn't bound is refused.
    let (status, unknown) = send(
        app,
        host_call(
            "POST",
            "/api/v1/host/notices",
            Some(&team_token),
            json!({"kind": "agent_retired", "hostAgentRef": "scout"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{unknown}");
    // Same lineage: the same account, under its new reference.
    let (status, moved) = send(
        app,
        host_call(
            "POST",
            "/api/v1/host/notices",
            Some(&team_token),
            json!({"kind": "agent_ref_changed", "hostAgentRef": "fixer", "newHostAgentRef": "fixer-2",
                   "ref": {"issuer": "https://host.example", "id": "fixer-2", "lineage": "l-01"}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(moved["outcome"], "waiting");
    let reference = sqlx::query_scalar::<_, String>(
        "SELECT host_agent_ref FROM host_agents WHERE host_id = $1 AND account_id = $2",
    )
    .bind(team_host)
    .bind(fixer)
    .fetch_one(&setup.pool)
    .await
    .expect("binding");
    assert_eq!(reference, "fixer-2");
    // A different lineage: the managers decide; the binding stays.
    let (status, asked) = send(
        app,
        host_call(
            "POST",
            "/api/v1/host/notices",
            Some(&team_token),
            json!({"kind": "agent_ref_changed", "hostAgentRef": "fixer-2", "newHostAgentRef": "fixer-3",
                   "ref": {"issuer": "https://host.example", "id": "fixer-3", "lineage": "l-02"}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{asked}");
    assert_eq!(asked["outcome"], "request_opened");
    let request = asked["requestIds"][0]
        .as_str()
        .expect("request")
        .parse::<Uuid>()
        .expect("uuid");
    let (raised_via, subject, status_now) =
        sqlx::query_as::<_, (Option<Uuid>, Option<Uuid>, String)>(
            "SELECT raised_via_host_id, subject_id, status FROM requests WHERE id = $1",
        )
        .bind(request)
        .fetch_one(&setup.pool)
        .await
        .expect("request");
    assert_eq!(
        (raised_via, subject, status_now.as_str()),
        (Some(team_host), Some(fixer), "open")
    );
    let reference = sqlx::query_scalar::<_, String>(
        "SELECT host_agent_ref FROM host_agents WHERE host_id = $1 AND account_id = $2",
    )
    .bind(team_host)
    .bind(fixer)
    .fetch_one(&setup.pool)
    .await
    .expect("binding");
    assert_eq!(reference, "fixer-2");

    // A run the host reports for Fixer; the heartbeat keeps it alive.
    let project = work.parse::<Uuid>().expect("project");
    let run = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO runs (id, workspace_id, space_id, account_id, host_id, title, cause_kind,
                          state, liveness, last_report_at, started_at)
        VALUES ($1, $2, $3, $4, $5, 'Fix the build', 'manual', 'working', 'quiet',
                now() - interval '1 hour', now() - interval '1 hour')
        "#,
    )
    .bind(run)
    .bind(setup.organization_id)
    .bind(project)
    .bind(fixer)
    .bind(team_host)
    .execute(&setup.pool)
    .await
    .expect("run");
    let (status, beat) = send(
        app,
        host_call(
            "POST",
            "/api/v1/host/heartbeat",
            Some(&team_token),
            json!({"capacity": {"slots": 4, "busy": 1},
                   "runs": [{"runId": run, "state": "working"}, {"runId": Uuid::now_v7()}]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{beat}");
    assert_eq!(beat["runsRenewed"], 1);
    let liveness = sqlx::query_scalar::<_, String>("SELECT liveness FROM runs WHERE id = $1")
        .bind(run)
        .fetch_one(&setup.pool)
        .await
        .expect("liveness");
    assert_eq!(liveness, "ok");
    // Mara's host listing that run renews nothing.
    let (status, beat) = send(
        app,
        host_call(
            "POST",
            "/api/v1/host/heartbeat",
            Some(&mara_token),
            json!({"runs": [{"runId": run}]}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{beat}");
    assert_eq!(beat["runsRenewed"], 0);

    // Retiring the agent interrupts its active runs on this host.
    let (status, retired) = send(
        app,
        host_call(
            "POST",
            "/api/v1/host/notices",
            Some(&team_token),
            json!({"kind": "agent_retired", "hostAgentRef": "fixer-2", "reason": "retired by its owner"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{retired}");
    assert_eq!(retired["interruptedRunIds"], json!([run]));
    let (state, reason) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT state, failure_reason FROM runs WHERE id = $1",
    )
    .bind(run)
    .fetch_one(&setup.pool)
    .await
    .expect("run state");
    assert_eq!(state, "interrupted");
    assert_eq!(reason.as_deref(), Some("The host retired this agent"));

    // Unlinking: the next call is 401 host_revoked, and what it asked for
    // is cancelled.
    let (status, unlinked) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/hosts/{team_host}/unlink"),
            None,
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{unlinked}");
    assert_eq!(unlinked["status"], "revoked");
    let (status, revoked) = send(
        app,
        host_call(
            "POST",
            "/api/v1/host/heartbeat",
            Some(&team_token),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{revoked}");
    assert_eq!(revoked["error"], "host_revoked");
    let status_now = sqlx::query_scalar::<_, String>("SELECT status FROM requests WHERE id = $1")
        .bind(request)
        .fetch_one(&setup.pool)
        .await
        .expect("request");
    assert_eq!(status_now, "cancelled");
    let (status, nobody) = send(
        app,
        host_call(
            "POST",
            "/api/v1/host/heartbeat",
            Some("awh_not-a-real-token-at-all"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{nobody}");
    assert_eq!(nobody["error"], "unauthorized");

    // Listing: Mara sees her own host and the workspace's, not the unlinked one.
    let (status, listed) = send(
        app,
        mara_browser.request(
            "GET",
            &format!("/api/v1/hosts?workspaceId={}", setup.organization_id),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let ids = listed["hosts"]
        .as_array()
        .expect("hosts")
        .iter()
        .map(|host| host["id"].as_str().expect("id").to_owned())
        .collect::<Vec<_>>();
    assert!(ids.contains(&mara_host_id.to_string()));
    assert!(ids.contains(&builtin.to_string()));
    assert!(!ids.contains(&team_host.to_string()));
}
