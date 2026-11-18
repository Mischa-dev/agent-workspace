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
    agent_interface::{Browser, connect_app, create_agent, initialize, send, tool},
    mcp_admin::{create_project, setup},
    message_edits::add_person,
    support::app::last_tool,
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

/// A host operation from the web app, in `workspace`: the value on
/// success, the refusal otherwise.
pub(super) async fn host_op(
    app: &axum::Router,
    browser: &Browser,
    workspace: Uuid,
    name: &str,
    input: Value,
) -> (StatusCode, Value) {
    let key = Uuid::now_v7().to_string();
    let (status, body) = send(
        app,
        browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/{name}"),
            Some(&key),
            input,
        ),
    )
    .await;
    if body["outcome"] == "ok" {
        (status, body["value"].clone())
    } else {
        (status, body)
    }
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
    let (status, refused) = host_op(
        app,
        &setup.browser,
        setup.organization_id,
        "host_unlink",
        json!({"hostId": builtin}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");

    // A member without hosts.manage can't link one for the workspace, but
    // may link one of their own.
    let (status, refused) = host_op(
        app,
        &mara_browser,
        setup.organization_id,
        "host_enrolment_create",
        json!({"owner": "workspace"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    let (status, personal) = host_op(
        app,
        &mara_browser,
        setup.organization_id,
        "host_enrolment_create",
        json!({"owner": "person", "label": "Mara's laptop"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{personal}");
    assert_eq!(personal["owner"]["kind"], "person");
    let personal_code = personal["code"].as_str().expect("code").to_owned();
    assert!(personal_code.starts_with("AWH-"), "{personal_code}");

    let (status, made) = host_op(
        app,
        &setup.browser,
        setup.organization_id,
        "host_enrolment_create",
        json!({"owner": "workspace", "label": "Team host"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made}");
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
    let replayable = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM idempotency_keys WHERE response_body::text LIKE '%' || $1 || '%'",
    )
    .bind(&team_code)
    .fetch_one(&setup.pool)
    .await
    .expect("replays");
    assert_eq!(replayable, 0, "no stored answer holds the code");

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
    let (status, personal) = host_op(
        app,
        &mara_browser,
        setup.organization_id,
        "host_enrolment_create",
        json!({"owner": "person"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{personal}");
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
    let (status, bound) = host_op(
        app,
        &setup.browser,
        setup.organization_id,
        "host_agent_bind",
        json!({"hostId": team_host, "accountId": fixer, "hostAgentRef": "fixer"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bound}");
    assert_eq!(bound["agents"][0]["hostAgentRef"], "fixer");
    assert_eq!(bound["agents"][0]["actions"][0]["id"], "pause");
    // Mara sees the team host but can't manage it.
    let (status, refused) = host_op(
        app,
        &mara_browser,
        setup.organization_id,
        "host_agent_bind",
        json!({"hostId": team_host, "accountId": scout, "hostAgentRef": "scout"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    // Mara owns her host, but agents.manage is what lets her hand it an agent.
    let (status, refused) = host_op(
        app,
        &mara_browser,
        setup.organization_id,
        "host_agent_bind",
        json!({"hostId": mara_host_id, "accountId": scout, "hostAgentRef": "scout"}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    // The owner can't reach Mara's own host at all.
    let (status, hidden) = host_op(
        app,
        &setup.browser,
        setup.organization_id,
        "host_agent_bind",
        json!({"hostId": mara_host_id, "accountId": scout, "hostAgentRef": "scout"}),
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
    let (status, unlinked) = host_op(
        app,
        &setup.browser,
        setup.organization_id,
        "host_unlink",
        json!({"hostId": team_host}),
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
    let (status, listed) = host_op(
        app,
        &mara_browser,
        setup.organization_id,
        "host_list",
        json!({}),
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

/// Every text a caller or host types into runs, requests and hosts is kept
/// and returned with its secrets removed: one secret, put into each field,
/// is found in no stored row and no answer.
#[tokio::test]
async fn secrets_typed_into_runs_requests_and_hosts_are_never_kept() {
    const SECRET: &str = "s3cr3tVALUE9876543210";
    let Some(setup) = setup("a1_text_redaction").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let (fixer, fixer_token) = create_agent(app, &setup.browser, &work, "Fixer", "fixer").await;
    let fixer_mcp = initialize(app, &fixer_token).await.expect("Fixer MCP");
    let person = connect_app(app, &setup.browser, "Desk", "full").await;
    let desk = initialize(app, &person).await.expect("Desk MCP");
    let typed = |what: &str| format!("{what} token={SECRET}");
    let mut answers = Vec::new();

    // Runs: every text a run's reporter types.
    let opened = tool(
        app,
        &fixer_token,
        &fixer_mcp,
        "run_create",
        json!({"spaceId": "WORK", "title": typed("Deploy"), "clientRunId": typed("r"),
               "cause": {"kind": "manual", "ref": typed("ref")},
               "settings": {"note": typed("note")}}),
    )
    .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    let run_id = opened["value"]["run"]["runId"]
        .as_str()
        .expect("run")
        .to_owned();
    for (name, arguments) in [
        (
            "run_update",
            json!({"runId": run_id, "title": typed("Retitled"), "statusLine": typed("Status"),
                   "backend": typed("b"), "externalUrl": format!("https://example.com/r?token={SECRET}"),
                   "failureReason": typed("Why"), "outcomeSummary": typed("Summary")}),
        ),
        (
            "run_usage_report",
            json!({"runId": run_id, "reportKey": typed("k"), "model": typed("m"), "inputTokens": 1}),
        ),
        (
            "run_result_report",
            json!({"runId": run_id, "results": [{"kind": "link", "title": typed("Report"),
                   "url": format!("https://example.com/x?token={SECRET}")}]}),
        ),
        (
            "run_activity_report",
            json!({"runId": run_id, "activities": [{"id": typed("q"), "kind": "interrupt",
                   "stepRef": typed("step"), "parentRef": typed("parent"), "summary": typed("Asking"),
                   "payload": {"request": {"kind": "approval", "clientRequestId": typed("c"),
                       "title": typed("Ship"), "detail": typed("Detail"),
                       "options": [{"id": typed("y"), "label": typed("Yes"), "style": "approve"},
                                   {"id": "no", "label": "No", "style": "deny"}]}}}]}),
        ),
    ] {
        let answer = tool(app, &fixer_token, &fixer_mcp, name, arguments).await;
        assert_eq!(answer["outcome"], "ok", "{name}: {answer}");
        answers.push(answer);
    }
    assert_eq!(answers[3]["value"]["state"], "waiting", "{}", answers[3]);
    let cancelled = tool(
        app,
        &person,
        &desk,
        "run_cancel",
        json!({"runId": run_id, "reason": format!("Leaked API_KEY={SECRET}")}),
    )
    .await;
    assert_eq!(cancelled["outcome"], "ok", "{cancelled}");
    answers.push(opened);
    answers.push(cancelled);

    // Hosts: what a person labels and what a host says of itself.
    let (status, made) = host_op(
        app,
        &setup.browser,
        setup.organization_id,
        "host_enrolment_create",
        json!({"owner": "workspace", "label": typed("Lab")}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let code = made["code"].as_str().expect("code").to_owned();
    let (status, enrolled) = send(
        app,
        host_call(
            "POST",
            "/api/v1/hosts/enrol",
            None,
            json!({"code": code, "host": {"name": typed("Lab"), "software": typed("sw"),
                   "version": typed("v")},
                   "protocolVersions": ["2026-11"], "connection": "dial_out"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{enrolled}");
    let host_id = enrolled["hostId"]
        .as_str()
        .expect("host")
        .parse::<Uuid>()
        .expect("uuid");
    let token = enrolled["credential"]["token"]
        .as_str()
        .expect("token")
        .to_owned();
    let agent_ref = typed("fixer");
    let mut host_answers = vec![made];
    for (uri, body) in [
        (
            "/api/v1/host/describe",
            json!({
                "software": {"name": typed("sw"), "version": typed("v")},
                "issuer": format!("https://host.example/?token={SECRET}"),
                "module": {"manifestUrl": format!("https://host.example/m?token={SECRET}")},
                "capabilities": {"note": typed("cap")},
                "offers": [{"id": "new", "title": typed("New"), "summary": typed("Sum"),
                            "start": {"panel": typed("p")}}],
                "actions": [{"id": "act", "label": typed("Act"), "needs": "panel"}],
                "agents": [{"hostAgentRef": agent_ref, "name": typed("Fixer"),
                            "ref": {"issuer": format!("https://host.example/?token={SECRET}"),
                                    "id": typed("id"), "lineage": "l-01"},
                            "backends": [{"id": "b1", "name": typed("B")}],
                            "actions": [{"id": "pause", "label": typed("Pause"), "needs": "confirm"}]}]
            }),
        ),
        ("bind", Value::Null),
        (
            "/api/v1/host/heartbeat",
            json!({"capacity": {"note": typed("cap")},
                   "agents": [{"hostAgentRef": agent_ref, "status": "paused"}]}),
        ),
        (
            "/api/v1/host/notices",
            json!({"kind": "agent_profile_changed", "hostAgentRef": agent_ref,
                   "profile": {"name": typed("Fixer"), "bio": typed("Bio")}}),
        ),
        (
            "/api/v1/host/notices",
            json!({"kind": "agent_ref_changed", "hostAgentRef": agent_ref,
                   "newHostAgentRef": typed("fixer-2"),
                   "ref": {"id": typed("id"), "lineage": typed("l-02")}}),
        ),
        (
            "/api/v1/host/notices",
            json!({"kind": "agent_retired", "hostAgentRef": agent_ref, "reason": typed("Gone")}),
        ),
    ] {
        let (status, answer) = if uri == "bind" {
            // The person binds the agent by the name the host gave it.
            host_op(
                app,
                &setup.browser,
                setup.organization_id,
                "host_agent_bind",
                json!({"hostId": host_id, "accountId": fixer, "hostAgentRef": agent_ref}),
            )
            .await
        } else {
            send(app, host_call("POST", uri, Some(&token), body)).await
        };
        assert_eq!(status, StatusCode::OK, "{uri}: {answer}");
        host_answers.push(answer);
    }
    // The host's name for the agent still matches, secrets removed alike.
    assert_eq!(
        host_answers[1]["unboundAgents"][0],
        "fixer token=[redacted]"
    );
    assert_eq!(
        host_answers[2]["agents"][0]["hostAgentRef"],
        "fixer token=[redacted]"
    );
    assert_eq!(
        host_answers[5]["outcome"], "request_opened",
        "{}",
        host_answers[5]
    );
    let notices =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM host_notices WHERE host_id = $1")
            .bind(host_id)
            .fetch_one(&setup.pool)
            .await
            .expect("notices");
    assert_eq!(notices, 3, "each notice found the agent by its kept name");
    let (status, listed) = host_op(
        app,
        &setup.browser,
        setup.organization_id,
        "host_list",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    answers.extend(host_answers);
    answers.push(listed);
    answers.push(enrolled);

    for answer in &answers {
        assert!(!answer.to_string().contains(SECRET), "returned: {answer}");
    }
    for table in [
        "runs",
        "run_activities",
        "run_usage",
        "usage_reports",
        "requests",
        "hosts",
        "host_agents",
        "host_enrolments",
        "host_notices",
        "project_memberships",
        "events",
        "event_staging",
        "deliveries",
        "audit_events",
        "idempotency_keys",
    ] {
        let kept = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT count(*) FROM {table} row WHERE to_jsonb(row)::text LIKE '%' || $1 || '%'"
        ))
        .bind(SECRET)
        .fetch_one(&setup.pool)
        .await
        .unwrap_or_else(|error| panic!("{table}: {error}"));
        assert_eq!(kept, 0, "{table} keeps a secret");
    }
}

// ------------------------------------------------------------------ A1.12

use agent_workspace_server::runs::{
    ops::{Cause, RunOpen, open_queued},
    state::RunState,
};

use super::support::test_host::TestHost;

/// A run for `agent` handed to its host, as A1.13's launch path will.
pub(super) async fn dispatch(pool: &sqlx::PgPool, space: Uuid, agent: Uuid, title: &str) -> Uuid {
    let mut transaction = pool.begin().await.expect("transaction");
    let opened = open_queued(
        &mut transaction,
        agent,
        RunOpen {
            space_id: space,
            client_run_id: None,
            title: title.to_owned(),
            cause: Cause {
                kind: "manual".to_owned(),
                conversation_id: None,
                work_item_id: None,
                message_id: None,
                reference: None,
            },
            parent_run_id: None,
            report_interval_seconds: None,
            settings: None,
            initial: RunState::Queued,
            requested_by: None,
            via_app_id: None,
            launch_id: None,
        },
    )
    .await
    .expect("queued");
    assert_eq!(opened.run.state, "queued");
    let (run, _) = hosts::dispatch::start_run(
        &mut transaction,
        opened.run.id,
        hosts::dispatch::StartInput::default(),
    )
    .await
    .expect("dispatched");
    transaction.commit().await.expect("commit");
    assert_eq!(run.state, "starting");
    run.id
}

pub(super) async fn run_state(pool: &sqlx::PgPool, run: Uuid) -> String {
    sqlx::query_scalar::<_, String>("SELECT state FROM runs WHERE id = $1")
        .bind(run)
        .fetch_one(pool)
        .await
        .expect("run state")
}

/// The acknowledgement was lost: its time runs out.
async fn lose_ack(pool: &sqlx::PgPool, run: Uuid) {
    sqlx::query(
        "UPDATE host_commands SET ack_due_at = now() - interval '1 second' WHERE run_id = $1 AND kind = 'start' AND state IN ('pending', 'fetched')",
    )
    .bind(run)
    .execute(pool)
    .await
    .expect("overdue");
    hosts::commands::sweep(pool).await.expect("sweep");
}

/// The re-offer schedule comes due now.
async fn reoffer_due(pool: &sqlx::PgPool, run: Uuid) {
    sqlx::query(
        "UPDATE host_commands SET next_offer_at = now() - interval '1 second' WHERE run_id = $1 AND kind = 'start'",
    )
    .bind(run)
    .execute(pool)
    .await
    .expect("due");
}

async fn bound_host(
    setup: &super::support::world::Setup,
    agent: Uuid,
    capabilities: Value,
) -> TestHost {
    let host = TestHost::enrol(
        &setup.app,
        &setup.browser,
        setup.organization_id,
        "Team host",
        json!({"connection": "dial_out"}),
    )
    .await;
    host.describe(
        capabilities,
        json!([{"hostAgentRef": "fixer", "name": "Fixer"}]),
    )
    .await;
    let (status, bound) = host_op(
        &setup.app,
        &setup.browser,
        setup.organization_id,
        "host_agent_bind",
        json!({"hostId": host.id, "accountId": agent, "hostAgentRef": "fixer"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bound}");
    host
}

/// Nothing a host was handed (a run credential) is kept in a command, an
/// acknowledgement or any event.
async fn kept_nowhere(pool: &sqlx::PgPool, secret: &str) {
    for table in [
        "host_commands",
        "run_credentials",
        "run_activities",
        "runs",
        "events",
        "event_staging",
        "deliveries",
        "audit_events",
        "background_jobs",
    ] {
        let found = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT count(*) FROM {table} row WHERE strpos(row::text, $1) > 0"
        ))
        .bind(secret)
        .fetch_one(pool)
        .await
        .expect("search");
        assert_eq!(found, 0, "a run credential is kept in {table}");
    }
}

#[tokio::test]
async fn a_host_runs_each_start_once_however_often_it_is_offered() {
    let Some(setup) = setup("a1_host_commands").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let space = work.parse::<Uuid>().expect("space");
    let (fixer, _) = create_agent(app, &setup.browser, &work, "Fixer", "fixer").await;
    let mut host = bound_host(
        &setup,
        fixer,
        json!({"start": true, "cancel": true, "idempotentStart": true}),
    )
    .await;
    assert_eq!(host.enrolled["pushSigningSecret"], Value::Null);

    let run = dispatch(&setup.pool, space, fixer, "Fix the build").await;
    let offered = host.poll().await;
    assert_eq!(offered.len(), 1, "{offered:?}");
    assert_eq!(offered[0]["kind"], "start");
    assert_eq!(offered[0]["runId"], run.to_string());
    assert_eq!(offered[0]["offer"], 1);
    let command_id = offered[0]["commandId"].clone();
    let mut credentials = Vec::new();

    // Offered three times (the acknowledgement lost twice): the same
    // command, run once on the host.
    for offer in 1..=3 {
        if offer > 1 {
            reoffer_due(&setup.pool, run).await;
            let again = host.poll().await;
            assert_eq!(again.len(), 1, "offer {offer}: {again:?}");
            assert_eq!(again[0]["commandId"], command_id);
            assert_eq!(again[0]["offer"], offer);
        }
        let (status, command) = host.fetch(&command_id).await;
        assert_eq!(status, StatusCode::OK, "{command}");
        assert_eq!(command["startKey"], format!("run-{run}"));
        assert_eq!(command["agent"]["hostAgentRef"], "fixer");
        assert_eq!(command["input"]["prompt"], "Fix the build");
        let token = command["workspaceCredential"]["token"]
            .as_str()
            .expect("credential")
            .to_owned();
        assert!(token.starts_with("awr_"), "{token}");
        assert!(
            command["workspaceCredential"]["scope"]
                .as_str()
                .expect("scope")
                .contains(&format!("run:{run}"))
        );
        credentials.push(token);
        let (reference, first) = host.run_start(&command);
        assert_eq!(first, offer == 1, "only the first offer starts a run");
        if offer < 3 {
            lose_ack(&setup.pool, run).await;
            assert_eq!(run_state(&setup.pool, run).await, "start_uncertain");
        } else {
            let (status, acked) = host
                .ack(
                    &command_id,
                    json!({"outcome": "accepted", "hostRunRef": reference}),
                )
                .await;
            assert_eq!(status, StatusCode::OK, "{acked}");
            assert_eq!(acked["state"], "acked");
            assert_eq!(acked["runState"], "accepted");
        }
    }
    assert_eq!(host.started.len(), 1);
    // A run on another host can't spend the workspace's model (A1.25): only
    // the built-in assistant's runs may ask it for a turn.
    let turn = json!({"system": "", "messages": [{"role": "user", "parts": [{"type": "text", "text": "hi"}]}]});
    let (status, refused) = send(
        app,
        host_call(
            "POST",
            &format!("/api/v1/host/runs/{run}/model"),
            credentials.last().map(String::as_str),
            turn.clone(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    let (status, refused) = send(
        app,
        host_call(
            "POST",
            &format!("/api/v1/host/runs/{}/model", Uuid::now_v7()),
            credentials.last().map(String::as_str),
            turn,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{refused}");
    let starts = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM host_commands WHERE run_id = $1 AND kind = 'start'",
    )
    .bind(run)
    .fetch_one(&setup.pool)
    .await
    .expect("starts");
    assert_eq!(starts, 1);
    // Acknowledging again changes nothing.
    let (status, again) = host
        .ack(
            &command_id,
            json!({"outcome": "accepted", "hostRunRef": "hr-1"}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["state"], "acked");
    let reference =
        sqlx::query_scalar::<_, Option<String>>("SELECT host_run_ref FROM runs WHERE id = $1")
            .bind(run)
            .fetch_one(&setup.pool)
            .await
            .expect("reference");
    assert_eq!(reference.as_deref(), Some("hr-1"));

    // The host reports as the run's agent with the credential it was
    // handed: working, then done.
    let token = credentials.last().expect("credential").clone();
    let session = initialize(app, &token).await.expect("run credential MCP");
    let started = tool(
        app,
        &token,
        &session,
        "run_activity",
        json!({"runId": run, "activities": [
            {"id": "s1", "kind": "run_started"},
            {"id": "m1", "kind": "text_message", "payload": {"text": "On it."}}]}),
    )
    .await;
    assert_eq!(started["outcome"], "ok", "{started}");
    assert_eq!(run_state(&setup.pool, run).await, "working");
    // A run that outlasts its start command's lifetime still gets a fresh
    // credential from the start it accepted.
    sqlx::query("UPDATE host_commands SET expires_at = now() - interval '1 second' WHERE run_id = $1 AND kind = 'start'")
        .bind(run)
        .execute(&setup.pool)
        .await
        .expect("age the start");
    let (status, refreshed) = host.fetch(&command_id).await;
    assert_eq!(status, StatusCode::OK, "{refreshed}");
    assert!(
        refreshed["workspaceCredential"]["token"]
            .as_str()
            .is_some_and(|token| token.starts_with("awr_")),
        "{refreshed}"
    );
    credentials.push(
        refreshed["workspaceCredential"]["token"]
            .as_str()
            .expect("credential")
            .to_owned(),
    );
    // It reaches the run's space only, even where the agent itself is a
    // member too.
    let elsewhere = create_project(&setup, "ELSE", "Elsewhere").await;
    let (status, added) = send(
        app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{elsewhere}/members"),
            None,
            json!({"actorId": fixer, "role": "member"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    let listed = tool(app, &token, &session, "projects_list", json!({})).await;
    assert!(!listed.to_string().contains(&elsewhere), "{listed}");
    assert!(listed.to_string().contains(&work), "{listed}");
    let opened = tool(
        app,
        &token,
        &session,
        "project_open",
        json!({"projectId": elsewhere}),
    )
    .await;
    assert_ne!(opened["outcome"], "ok", "{opened}");
    let done = last_tool(
        app,
        &token,
        &session,
        "run_update",
        json!({"runId": run, "state": "done"}),
    )
    .await;
    assert_eq!(done["outcome"], "ok", "{done}");
    let live = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM run_credentials WHERE run_id = $1 AND revoked_at IS NULL",
    )
    .bind(run)
    .fetch_one(&setup.pool)
    .await
    .expect("credentials");
    assert_eq!(live, 0, "the run's credentials end with it");
    assert!(
        initialize(app, &token).await.is_none(),
        "a run's credential stops working when the run ends"
    );
    // A finished run's start is no longer handed a credential.
    let (status, late) = host.fetch(&command_id).await;
    assert_eq!(status, StatusCode::OK, "{late}");
    assert_eq!(late["workspaceCredential"], Value::Null);

    // A run the host stops listing for three heartbeats is interrupted.
    let second = dispatch(&setup.pool, space, fixer, "Watch the deploy").await;
    let offered = host.poll().await;
    assert_eq!(offered.len(), 1, "{offered:?}");
    let (_, command) = host.fetch(&offered[0]["commandId"]).await;
    credentials.push(
        command["workspaceCredential"]["token"]
            .as_str()
            .expect("credential")
            .to_owned(),
    );
    let (reference, _) = host.run_start(&command);
    let (status, acked) = host
        .ack(
            &offered[0]["commandId"],
            json!({"outcome": "accepted", "hostRunRef": reference}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{acked}");
    host.heartbeat(json!([{"runId": second, "hostRunRef": reference, "state": "working"}]))
        .await;
    for beat in 1..=3 {
        assert_eq!(
            run_state(&setup.pool, second).await,
            "accepted",
            "beat {beat}"
        );
        host.heartbeat(json!([])).await;
    }
    assert_eq!(run_state(&setup.pool, second).await, "interrupted");
    let live = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM run_credentials WHERE run_id = $1 AND revoked_at IS NULL",
    )
    .bind(second)
    .fetch_one(&setup.pool)
    .await
    .expect("credentials");
    assert_eq!(live, 0);

    for token in &credentials {
        kept_nowhere(&setup.pool, token).await;
    }
    let digest_only = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM run_credentials WHERE token_digest = sha256(convert_to($1, 'UTF8'))",
    )
    .bind(&credentials[0])
    .fetch_one(&setup.pool)
    .await
    .expect("digest");
    assert_eq!(digest_only, 1);
}

#[tokio::test]
async fn a_start_nobody_confirmed_is_never_run_twice_and_a_person_decides() {
    let Some(setup) = setup("a1_host_uncertain").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let space = work.parse::<Uuid>().expect("space");
    let (fixer, _) = create_agent(app, &setup.browser, &work, "Fixer", "fixer").await;
    let person = connect_app(app, &setup.browser, "Desk", "full").await;
    let desk = initialize(app, &person).await.expect("Desk MCP");
    // No idempotentStart: an unconfirmed start is never offered again.
    let mut host = bound_host(
        &setup,
        fixer,
        json!({"start": true, "cancel": true, "inspect": true}),
    )
    .await;

    let run = dispatch(&setup.pool, space, fixer, "Migrate the data").await;
    let offered = host.poll().await;
    let start_id = offered[0]["commandId"].clone();
    let (status, command) = host.fetch(&start_id).await;
    assert_eq!(status, StatusCode::OK, "{command}");
    let (reference, _) = host.run_start(&command);
    lose_ack(&setup.pool, run).await;
    assert_eq!(run_state(&setup.pool, run).await, "start_uncertain");
    let never = sqlx::query_scalar::<_, bool>(
        "SELECT next_offer_at = 'infinity'::timestamptz FROM host_commands WHERE id = $1::uuid",
    )
    .bind(start_id.as_str().expect("id"))
    .fetch_one(&setup.pool)
    .await
    .expect("schedule");
    assert!(never, "a non-idempotent host's start is not offered again");

    // It is asked whether it knows the run instead; it doesn't.
    let asked = host.poll().await;
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0]["kind"], "inspect");
    let (status, answered) = host
        .ack(
            &asked[0]["commandId"],
            json!({"outcome": "done", "result": {"known": false}}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    assert_eq!(run_state(&setup.pool, run).await, "start_uncertain");
    assert!(host.poll().await.is_empty());

    // A person marks it as not started.
    let marked = tool(
        app,
        &person,
        &desk,
        "run_mark_not_started",
        json!({"runId": run}),
    )
    .await;
    assert_eq!(marked["outcome"], "ok", "{marked}");
    assert_eq!(run_state(&setup.pool, run).await, "failed");
    let closed =
        sqlx::query_scalar::<_, String>("SELECT state FROM host_commands WHERE id = $1::uuid")
            .bind(start_id.as_str().expect("id"))
            .fetch_one(&setup.pool)
            .await
            .expect("start state");
    assert_eq!(closed, "expired");

    // Then the host reports it after all: the run comes back.
    let (status, late) = host
        .ack(
            &start_id,
            json!({"outcome": "accepted", "hostRunRef": reference}),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{late}");
    assert_eq!(late["runState"], "accepted");
    let (reason, revived_ref) = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT failure_reason, host_run_ref FROM runs WHERE id = $1",
    )
    .bind(run)
    .fetch_one(&setup.pool)
    .await
    .expect("revived");
    assert_eq!(reason, None);
    assert_eq!(revived_ref, Some(reference));

    // A stop is asked of the host, and the run ends only when it confirms.
    let stop = tool(
        app,
        &person,
        &desk,
        "run_cancel",
        json!({"runId": run, "reason": "Not now", "clientRequestId": "stop-1"}),
    )
    .await;
    assert_eq!(stop["outcome"], "ok", "{stop}");
    assert_eq!(run_state(&setup.pool, run).await, "cancel_requested");
    agent_workspace_server::runs::liveness::settle(&setup.pool)
        .await
        .expect("settle");
    assert_eq!(run_state(&setup.pool, run).await, "cancel_requested");
    let cancel = host.poll().await;
    assert_eq!(cancel.len(), 1, "{cancel:?}");
    assert_eq!(cancel[0]["kind"], "cancel");
    let (status, command) = host.fetch(&cancel[0]["commandId"]).await;
    assert_eq!(status, StatusCode::OK, "{command}");
    assert_eq!(command["reason"], "Not now");
    assert_eq!(run_state(&setup.pool, run).await, "cancel_requested");
    let (status, stopped) = host
        .ack(&cancel[0]["commandId"], json!({"outcome": "done"}))
        .await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    assert_eq!(stopped["runState"], "cancelled");
    // Unknown and other hosts' commands are refused.
    let (status, unknown) = host.fetch(&json!(Uuid::now_v7().to_string())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{unknown}");
    assert_eq!(unknown["error"], "command_unknown");
}

#[tokio::test]
async fn a_push_host_is_sent_signed_commands_and_its_secrets_are_sealed() {
    let Some(setup) = setup("a1_host_push").await else {
        return;
    };
    let app = &setup.app;
    let work = create_project(&setup, "WORK", "Work").await;
    let space = work.parse::<Uuid>().expect("space");
    let (fixer, _) = create_agent(app, &setup.browser, &work, "Fixer", "fixer").await;
    let host = TestHost::enrol(
        app,
        &setup.browser,
        setup.organization_id,
        "Push host",
        json!({"connection": "push", "push": {"url": "https://host.example/aw"},
               "hostToken": "host-side-token-0123456789"}),
    )
    .await;
    let signing = host.enrolled["pushSigningSecret"]
        .as_str()
        .expect("signing secret")
        .to_owned();
    assert!(signing.starts_with("whsec_"), "{signing}");
    let (connection, url, sealed) = sqlx::query_as::<_, (String, Option<String>, bool)>(
        "SELECT connection, push_url, push_secret_id IS NOT NULL AND host_token_secret_id IS NOT NULL FROM hosts WHERE id = $1",
    )
    .bind(host.id)
    .fetch_one(&setup.pool)
    .await
    .expect("host");
    assert_eq!(connection, "push");
    assert_eq!(url.as_deref(), Some("https://host.example/aw"));
    assert!(sealed);
    for secret in [signing.as_str(), "host-side-token-0123456789"] {
        let plain = sqlx::query_scalar::<_, i64>(
            "SELECT (SELECT count(*) FROM secrets row WHERE strpos(row::text, $1) > 0) + (SELECT count(*) FROM hosts row WHERE strpos(row::text, $1) > 0)",
        )
        .bind(secret)
        .fetch_one(&setup.pool)
        .await
        .expect("search");
        assert_eq!(plain, 0, "a push secret is kept only sealed");
    }

    // An address that isn't public https is refused.
    let key = Uuid::now_v7().to_string();
    let (_, made) = send(
        app,
        setup.browser.request(
            "POST",
            &format!(
                "/api/v2/w/{}/ops/host_enrolment_create",
                setup.organization_id
            ),
            Some(&key),
            json!({"owner": "workspace"}),
        ),
    )
    .await;
    let (status, refused) = send(
        app,
        host_call(
            "POST",
            "/api/v1/hosts/enrol",
            None,
            json!({"code": made["value"]["code"], "host": {"name": "Inside"},
                   "connection": "push", "push": {"url": "http://127.0.0.1:9/aw"}}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["field"], "push.url", "{refused}");

    // A command for it is pushed by a job.
    host.describe(
        json!({"start": true, "idempotentStart": true}),
        json!([{"hostAgentRef": "fixer", "name": "Fixer"}]),
    )
    .await;
    let (status, bound) = host_op(
        app,
        &setup.browser,
        setup.organization_id,
        "host_agent_bind",
        json!({"hostId": host.id, "accountId": fixer, "hostAgentRef": "fixer"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{bound}");
    let run = dispatch(&setup.pool, space, fixer, "Ship it").await;
    let pushes = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM background_jobs WHERE kind = 'hosts.push_command' AND payload->>'commandId' = (SELECT id::text FROM host_commands WHERE run_id = $1)",
    )
    .bind(run)
    .fetch_one(&setup.pool)
    .await
    .expect("push job");
    assert_eq!(pushes, 1);
}
