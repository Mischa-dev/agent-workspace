//! M1.6: built modules. The silent failures: a module switched off that
//! still shows its records (in queries, search or a thing's page), still
//! takes form answers or still runs its automations; switching it back on
//! losing data; a retried create making two of everything; a part from
//! another workspace joining a module; and a role deciding by account kind.

use agent_workspace_server::jobs::{self, WorkerOptions};
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::send,
    callers::{Call, Caller, drain_outbox},
    world::{Workspace, WorldBuilder},
};

/// A tool call by an agent, or by the owner's connected app ("desk"), who
/// builds the modules here: agents join spaces as members or viewers.
async fn tool(world: &Workspace, who: &str, name: &'static str, arguments: Value) -> Value {
    let call = Call::Tool { name, arguments };
    match world.agents.get(who) {
        Some(agent) => agent.call(&world.app, call).await.1,
        None => world.apps[who].call(&world.app, call).await.1,
    }
}

/// Publishes the outbox and runs due jobs until both are quiet.
async fn pump(world: &Workspace) {
    let registry = jobs::default_registry();
    let options = WorkerOptions {
        concurrency: 1,
        poll_interval: std::time::Duration::from_millis(20),
        lease: std::time::Duration::from_secs(30),
        worker_id: "m16-test".to_owned(),
    };
    for _ in 0..10 {
        drain_outbox(&world.state).await;
        let ran = jobs::run_due(&world.state, &registry, &options)
            .await
            .expect("run jobs");
        if ran == 0 {
            return;
        }
    }
}

fn module_input() -> Value {
    json!({
        "spaceId": "WORK",
        "key": "playtest",
        "name": "Playtest tracker",
        "collections": [{"name": "Playtests", "fields": [{"name": "Notes", "type": "text"}]}],
        "forms": [{"name": "Report a playtest", "collection": "Playtests",
                   "fields": [{"field": "Notes"}]}],
        "automations": [{"collection": "Playtests", "rule": {
            "name": "Tell me about playtests",
            "trigger": {"kind": "record_created"},
            "actions": [{"kind": "notify", "recipients": "owner",
                         "message": "New playtest: {{record.title}}"}]
        }}],
        "nav": [{"label": "Playtests", "target": {"kind": "collection", "collection": "Playtests"}}],
        "clientRequestId": "playtest-module",
    })
}

#[tokio::test]
async fn a_module_switched_off_hides_its_parts_and_loses_nothing() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("builder")
        .agent("watcher", "viewer")
        .build("m16_built_modules")
        .await
    else {
        return;
    };
    let pool = world.pool().clone();

    // A viewer can't build one; whoever's role builds modules can (here the
    // owner, through their app), and everything is made in one call.
    let refused = tool(&world, "watcher", "module_create", module_input()).await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let made = tool(&world, "builder", "module_create", module_input()).await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let module = made["value"].clone();
    assert_eq!(module["key"], "local.playtest");
    assert_eq!(module["on"], true);
    let kinds = module["parts"]
        .as_array()
        .expect("parts")
        .iter()
        .map(|part| part["kind"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    for kind in ["collection", "form", "automation", "nav"] {
        assert!(
            kinds.iter().any(|found| found == kind),
            "{kind} in {module}"
        );
    }
    let module_id = module["id"].as_str().expect("module").to_owned();
    // The same call again changes nothing.
    let again = tool(&world, "builder", "module_create", module_input()).await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(again["replayed"], true, "{again}");
    assert_eq!(again["value"]["id"], module_id.as_str());
    let collections =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM collections WHERE name = 'Playtests'")
            .fetch_one(&pool)
            .await
            .expect("collections");
    assert_eq!(collections, 1);
    let form_id = module["parts"]
        .as_array()
        .expect("parts")
        .iter()
        .find(|part| part["kind"] == "form")
        .and_then(|part| part["id"].as_str())
        .expect("form")
        .to_owned();

    // A record, a link to the form, and a pending automation run.
    let written = tool(
        &world,
        "builder",
        "record_write",
        json!({"projectId": "WORK", "collection": "Playtests",
               "records": [{"title": "Zeppelin level", "fields": {"Notes": "resets"}}],
               "clientRequestId": "zeppelin"}),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let record = written["value"]["records"][0]["id"]
        .as_str()
        .expect("record")
        .to_owned();
    let (status, link) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/bug-report-links/{form_id}/replace"),
            Some("form-link"),
            json!({}),
        ),
    )
    .await;
    assert!(status.is_success(), "{link}");
    let form_url = link["formUrl"].as_str().expect("form url").to_owned();
    let token = form_url.rsplit('/').next().expect("token").to_owned();
    let form = |token: String| {
        let world = &world;
        async move {
            send(
                &world.app,
                world.owner.browser.request(
                    "GET",
                    &format!("/api/v1/forms/{token}"),
                    None,
                    Value::Null,
                ),
            )
            .await
        }
    };
    assert_eq!(form(token.clone()).await.0, StatusCode::OK);

    // Off: the records vanish from queries, search and the thing's page,
    // the form is turned off and the automation holds its run.
    let off = tool(
        &world,
        "builder",
        "module_set",
        json!({"moduleId": module_id, "on": false, "clientRequestId": "off"}),
    )
    .await;
    assert_eq!(off["outcome"], "ok", "{off}");
    assert_eq!(off["value"]["on"], false);
    let query = json!({"projectId": "WORK", "collection": "Playtests"});
    let hidden = tool(&world, "builder", "record_query", query.clone()).await;
    assert_ne!(hidden["outcome"], "ok", "{hidden}");
    let searched = tool(
        &world,
        "builder",
        "search",
        json!({"query": "zeppelin", "kinds": ["records"]}),
    )
    .await;
    assert_eq!(
        searched["value"]["results"].as_array().map(Vec::len),
        Some(0),
        "{searched}"
    );
    let opened = tool(
        &world,
        "builder",
        "thing_open",
        json!({"kind": "record", "id": record}),
    )
    .await;
    assert_ne!(opened["outcome"], "ok", "{opened}");
    let (status, closed) = form(token.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{closed}");
    assert!(closed.to_string().contains("turned off"), "{closed}");
    pump(&world).await;
    let skipped = sqlx::query_scalar::<_, Option<String>>(
        "SELECT detail FROM automation_runs WHERE state = 'skipped' LIMIT 1",
    )
    .fetch_optional(&pool)
    .await
    .expect("runs");
    assert_eq!(
        skipped.flatten().as_deref(),
        Some("Its module is turned off."),
        "the rule's run is held with the reason"
    );

    // On again: everything is back, nothing lost.
    let on = tool(
        &world,
        "builder",
        "module_set",
        json!({"moduleId": module_id, "on": true, "clientRequestId": "on"}),
    )
    .await;
    assert_eq!(on["outcome"], "ok", "{on}");
    let back = tool(&world, "builder", "record_query", query.clone()).await;
    assert_eq!(back["value"]["total"], 1, "{back}");
    assert_eq!(form(token).await.0, StatusCode::OK);

    // Archived keeping its parts: the collection lives on, plain.
    let archived = tool(
        &world,
        "builder",
        "module_archive",
        json!({"moduleId": module_id, "parts": "keep", "clientRequestId": "archive"}),
    )
    .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    assert_eq!(archived["value"]["archived"], true);
    let plain = tool(&world, "builder", "record_query", query).await;
    assert_eq!(plain["value"]["total"], 1, "{plain}");
    let owned = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM module_parts WHERE module_id = $1::uuid AND part_kind = 'collection'",
    )
    .bind(&module_id)
    .fetch_one(&pool)
    .await
    .expect("parts");
    assert_eq!(owned, 0);
    world.finish().await;
}

#[tokio::test]
async fn a_part_from_another_workspace_never_joins() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("vee", "member")
        .connected_app("builder")
        .build("m16_other_workspace")
        .await
    else {
        return;
    };
    let pool = world.pool().clone();
    // Another workspace with a collection of its own.
    let other = Uuid::now_v7();
    let other_space = Uuid::now_v7();
    let foreign = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Other', $2)")
        .bind(other)
        .bind(format!("other-{}", other.simple()))
        .execute(&pool)
        .await
        .expect("workspace");
    sqlx::query(
        "INSERT INTO projects (id, organization_id, project_key, name) VALUES ($1, $2, 'OTH', 'Other')",
    )
    .bind(other_space)
    .bind(other)
    .execute(&pool)
    .await
    .expect("space");
    // The owner belongs there too: even someone in both workspaces can't
    // move a part across.
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(other)
    .bind(world.owner.actor)
    .execute(&pool)
    .await
    .expect("membership");
    sqlx::query(
        "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) \
         VALUES ($1, $2, $3, 'owner', 'Owner', 'owner')",
    )
    .bind(Uuid::now_v7())
    .bind(other_space)
    .bind(world.owner.actor)
    .execute(&pool)
    .await
    .expect("space membership");
    sqlx::query(
        "INSERT INTO collections (id, project_id, name, description, title_name, position, \
         created_by_actor_id) VALUES ($1, $2, 'Secrets', '', 'Name', 0, $3)",
    )
    .bind(foreign)
    .bind(other_space)
    .bind(world.owner.actor)
    .execute(&pool)
    .await
    .expect("collection");

    let made = tool(
        &world,
        "builder",
        "module_create",
        json!({"spaceId": "WORK", "key": "grab", "name": "Grab",
               "parts": [{"kind": "collection", "id": foreign}],
               "clientRequestId": "grab"}),
    )
    .await;
    assert_ne!(made["outcome"], "ok", "{made}");
    assert!(!made.to_string().contains("Secrets"), "{made}");
    let empty = tool(
        &world,
        "builder",
        "module_create",
        json!({"spaceId": "WORK", "key": "grab", "name": "Grab", "clientRequestId": "grab-2"}),
    )
    .await;
    assert_eq!(empty["outcome"], "ok", "{empty}");
    let linked = tool(
        &world,
        "builder",
        "module_part_link",
        json!({"moduleId": empty["value"]["id"], "part": {"kind": "collection", "id": foreign},
               "clientRequestId": "grab-3"}),
    )
    .await;
    assert_ne!(linked["outcome"], "ok", "{linked}");
    let parts =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM module_parts WHERE object_id = $1")
            .bind(foreign)
            .fetch_one(&pool)
            .await
            .expect("parts");
    assert_eq!(parts, 0);

    // A space in this workspace the owner isn't in: a workspace-wide module
    // can't take a part from it either.
    let closed_space = Uuid::now_v7();
    let hidden = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO projects (id, organization_id, project_key, name) VALUES ($1, $2, 'SHUT', 'Shut')",
    )
    .bind(closed_space)
    .bind(world.organization)
    .execute(&pool)
    .await
    .expect("closed space");
    let vee = world.people["vee"].actor;
    sqlx::query(
        "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) \
         VALUES ($1, $2, $3, 'owner', 'Vee', 'vee')",
    )
    .bind(Uuid::now_v7())
    .bind(closed_space)
    .bind(vee)
    .execute(&pool)
    .await
    .expect("vee in the closed space");
    sqlx::query(
        "INSERT INTO collections (id, project_id, name, description, title_name, position, \
         created_by_actor_id) VALUES ($1, $2, 'Hidden', '', 'Name', 0, $3)",
    )
    .bind(hidden)
    .bind(closed_space)
    .bind(vee)
    .execute(&pool)
    .await
    .expect("hidden collection");
    let wide = tool(
        &world,
        "builder",
        "module_create",
        json!({"key": "wide", "name": "Wide", "workspace": world.organization,
               "clientRequestId": "wide"}),
    )
    .await;
    assert_eq!(wide["outcome"], "ok", "{wide}");
    let reached = tool(
        &world,
        "builder",
        "module_part_link",
        json!({"moduleId": wide["value"]["id"], "part": {"kind": "collection", "id": hidden},
               "clientRequestId": "wide-1"}),
    )
    .await;
    assert_ne!(reached["outcome"], "ok", "{reached}");
    assert!(!reached.to_string().contains("Hidden"), "{reached}");
    let parts =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM module_parts WHERE object_id = $1")
            .bind(hidden)
            .fetch_one(&pool)
            .await
            .expect("parts");
    assert_eq!(parts, 0);
    world.finish().await;
}
