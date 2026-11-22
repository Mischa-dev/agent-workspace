//! A1.25: the built-in host's assistant. With no model set, the offer says
//! what to set and adds nothing; once the workspace has a provider and key,
//! adding it is one step, and a DM to it is answered in the DM, by a run
//! that used a tool as the assistant and whose tokens are on the run and in
//! the workspace's AI usage. The silent failures: the key showing up
//! anywhere outside the secrets store (a command, a run's log, a message,
//! an event), and another host's run using the workspace's model.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::send,
    callers::Person,
    run_due_jobs,
    world::{Workspace, WorldBuilder},
};

const KEY: &str = "sk-assistant-test-SECRET-4242";

async fn op(
    app: &axum::Router,
    world: &Workspace,
    who: &Person,
    name: &str,
    input: Value,
) -> (StatusCode, Value) {
    send(
        app,
        who.browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/{name}", world.organization),
            Some(&Uuid::now_v7().to_string()),
            input,
        ),
    )
    .await
}

fn assistant_offer(listed: &Value) -> (String, Value) {
    let found = listed["value"]["offers"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|offer| offer["offer"]["id"] == "assistant")
        .unwrap_or_else(|| panic!("no assistant offer in {listed}"));
    (
        found["hostId"].as_str().expect("host").to_owned(),
        found["offer"].clone(),
    )
}

/// Run the jobs and the built-in host's commands until nothing changes.
async fn settle(state: &agent_workspace_server::AppState) {
    for _ in 0..4 {
        run_due_jobs(state).await;
        agent_workspace_server::scheduler::drain_now(state)
            .await
            .expect("drain");
    }
    run_due_jobs(state).await;
}

#[tokio::test]
async fn the_assistant_is_offered_with_a_model_and_answers_a_dm_with_its_tools() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .build("a1_assistant")
        .await
    else {
        return;
    };
    let keys = tempfile::tempdir().expect("keyring root");
    let keyring = keys.path().join("secrets.keyring");
    agent_workspace_server::secrets::write_new_keyring(
        &keyring,
        &agent_workspace_server::secrets::FileKeyring::generate(),
    )
    .expect("keyring");
    let (app, state) = world.rebuilt_with(
        "AGENT_WORKSPACE_SECRETS_KEY_PATH",
        keyring.to_string_lossy().into_owned(),
    );
    let owner = &world.owner;
    let organization = world.organization;

    // No model yet: the offer says what to set, and choosing it adds nothing.
    let (status, listed) = op(&app, &world, owner, "agent_offer_list", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let (builtin, offer) = assistant_offer(&listed);
    let unavailable = offer["unavailable"].as_str().expect("says what to set");
    assert!(unavailable.contains("provider"), "{unavailable}");
    let (status, refused) = op(
        &app,
        &world,
        owner,
        "agent_offer_start",
        json!({"host": builtin, "offerId": "assistant"}),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{refused}");
    let accounts_before = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM actors")
        .fetch_one(world.pool())
        .await
        .expect("count");

    // A provider and its key: ready, and one step adds it.
    let (status, body) = op(
        &app,
        &world,
        owner,
        "setting_update",
        json!({"key": "ai.provider", "scope": "workspace", "value": "fake"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = send(
        &app,
        owner.browser.request(
            "PUT",
            &format!("/api/v1/organizations/{organization}/secrets/ai_key.fake"),
            None,
            json!({"value": KEY, "description": "The model key"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, listed) = op(&app, &world, owner, "agent_offer_list", json!({})).await;
    let (_, offer) = assistant_offer(&listed);
    assert!(offer.get("unavailable").is_none(), "{offer}");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM actors")
            .fetch_one(world.pool())
            .await
            .expect("count"),
        accounts_before,
        "the refused start added nobody"
    );
    let (status, added) = op(
        &app,
        &world,
        owner,
        "agent_offer_start",
        json!({"host": builtin, "offerId": "assistant", "name": "Helper"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{added}");
    let assistant: Uuid = added["value"]["accountId"]
        .as_str()
        .expect("account")
        .parse()
        .expect("uuid");
    let reference = sqlx::query_scalar::<_, String>(
        "SELECT binding.host_agent_ref FROM host_agents binding \
         JOIN hosts host ON host.id = binding.host_id AND host.builtin \
         WHERE binding.account_id = $1 AND binding.status = 'active'",
    )
    .bind(assistant)
    .fetch_one(world.pool())
    .await
    .expect("bound to the built-in host");
    assert!(reference.starts_with("assistant:"), "{reference}");

    // A DM to it is answered there, after it used a tool as itself.
    let (status, dm) = send(
        &app,
        owner.browser.request(
            "POST",
            "/api/v1/direct/dms",
            None,
            json!({"actorId": assistant}),
        ),
    )
    .await;
    assert!(status.is_success(), "{dm}");
    let conversation = dm["conversation"]["id"].as_str().expect("DM").to_owned();
    let (status, sent) = send(
        &app,
        owner.browser.request(
            "POST",
            &format!("/api/v1/conversations/{conversation}/messages"),
            Some(&Uuid::now_v7().to_string()),
            json!({"content": {"version": 1, "nodes": [
                {"type": "text", "text": "Which projects are there? [[call project_list {}]]"}]}}),
        ),
    )
    .await;
    assert!(status.is_success(), "{sent}");
    settle(&state).await;

    let run = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, state FROM runs WHERE account_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(assistant)
    .fetch_one(world.pool())
    .await
    .expect("a run for the DM");
    assert_eq!(run.1, "done", "the run finished");
    let tools = sqlx::query_scalar::<_, String>(
        "SELECT payload->>'tool' FROM run_activities WHERE run_id = $1 AND kind = 'tool_call'",
    )
    .bind(run.0)
    .fetch_all(world.pool())
    .await
    .expect("tool calls");
    let log = sqlx::query_scalar::<_, String>(
        "SELECT kind || ': ' || summary FROM run_activities WHERE run_id = $1 ORDER BY seq",
    )
    .bind(run.0)
    .fetch_all(world.pool())
    .await
    .expect("run log");
    assert_eq!(tools, vec!["project_list".to_owned()], "{log:#?}");
    let answer = sqlx::query_scalar::<_, String>(
        "SELECT plain_text FROM messages WHERE conversation_id = $1::uuid AND author_actor_id = $2",
    )
    .bind(&conversation)
    .bind(assistant)
    .fetch_all(world.pool())
    .await
    .expect("answers");
    assert_eq!(answer, vec!["Done: project_list worked.".to_owned()]);

    // Its tokens are on the run and in the workspace's AI usage.
    let (input, output) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT COALESCE(sum(input_tokens), 0)::bigint, COALESCE(sum(output_tokens), 0)::bigint \
         FROM run_usage WHERE run_id = $1",
    )
    .bind(run.0)
    .fetch_one(world.pool())
    .await
    .expect("run usage");
    assert!(input > 0 && output > 0, "{input} {output}");
    let calls = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM ai_usage WHERE organization_id = $1 AND prompt_id = 'assistant_turn' \
         AND actor_id = $2 AND outcome = 'succeeded'",
    )
    .bind(organization)
    .bind(assistant)
    .fetch_one(world.pool())
    .await
    .expect("ai usage");
    assert_eq!(calls, 2, "one call to choose the tool, one to answer");

    // Stopped before its turn, it ends cancelled and says nothing more.
    let (status, sent) = send(
        &app,
        owner.browser.request(
            "POST",
            &format!("/api/v1/conversations/{conversation}/messages"),
            Some(&Uuid::now_v7().to_string()),
            json!({"content": {"version": 1, "nodes": [
                {"type": "text", "text": "And the tasks? [[call project_list {}]]"}]}}),
        ),
    )
    .await;
    assert!(status.is_success(), "{sent}");
    run_due_jobs(&state).await;
    agent_workspace_server::scheduler::drain_now(&state)
        .await
        .expect("drain");
    let second = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM runs WHERE account_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(assistant)
    .fetch_one(world.pool())
    .await
    .expect("a second run");
    assert_ne!(second, run.0);
    let (status, stopped) = op(&app, &world, owner, "run_cancel", json!({"runId": second})).await;
    assert_eq!(status, StatusCode::OK, "{stopped}");
    // The host hears the stop before the turn it accepted gets to run.
    agent_workspace_server::scheduler::drain_now(&state)
        .await
        .expect("drain");
    settle(&state).await;
    let ended = sqlx::query_scalar::<_, String>("SELECT state FROM runs WHERE id = $1")
        .bind(second)
        .fetch_one(world.pool())
        .await
        .expect("run");
    assert_eq!(ended, "cancelled");
    let answers = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM messages WHERE conversation_id = $1::uuid AND author_actor_id = $2",
    )
    .bind(&conversation)
    .bind(assistant)
    .fetch_one(world.pool())
    .await
    .expect("answers");
    assert_eq!(answers, 1, "nothing said after the stop");

    // The key is nowhere but the secrets store.
    for (table, column) in [
        ("host_commands", "body::text"),
        ("host_commands", "ack::text"),
        ("run_activities", "payload::text"),
        ("messages", "plain_text"),
        ("events", "payload::text"),
        ("background_jobs", "payload::text"),
        ("audit_events", "metadata::text"),
    ] {
        let found = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT count(*) FROM {table} WHERE {column} LIKE '%' || $1 || '%'"
        ))
        .bind(KEY)
        .fetch_one(world.pool())
        .await
        .unwrap_or_else(|error| panic!("{table}.{column}: {error}"));
        assert_eq!(found, 0, "the key is in {table}");
    }
    world.finish().await;
}
