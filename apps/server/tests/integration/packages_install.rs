//! M1.9: installing packages. The silent failures: a retried install that
//! makes everything twice, an install that runs a plan the person never
//! saw (the workspace changed since the preview), a member without
//! roles.manage adding a role through a package, and an install that
//! doesn't export back to the package it came from.

use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    callers::{Call, Caller},
    world::{Workspace, WorldBuilder},
};

async fn tool(world: &Workspace, who: &str, name: &'static str, arguments: Value) -> Value {
    let call = Call::Tool { name, arguments };
    match world.agents.get(who) {
        Some(agent) => agent.call(&world.app, call).await.1,
        None => world.apps[who].call(&world.app, call).await.1,
    }
}

fn studio() -> Value {
    json!({
        "format": "agent-workspace-package/1",
        "id": "studio.game_studio",
        "version": "1.0.0",
        "name": "Game studio",
        "target": "space",
        "contents": [
            {"kind": "role", "key": "tester", "body": {
                "name": "Tester", "baseRole": "member", "permissions": ["tasks.view"]}},
            {"kind": "space", "key": "studio", "body": {"name": "Studio", "key": "STU"}},
            {"kind": "channels", "key": "channels", "body": {"channels": [{"name": "playtests"}]}},
            {"kind": "collection", "key": "feedback", "body": {
                "name": "Feedback", "titleName": "Name",
                "fields": [{"name": "Notes", "type": "text"}], "views": []}},
            {"kind": "form", "key": "feedback_form", "body": {
                "name": "Feedback form", "collection": "@feedback",
                "fields": [{"field": "Notes"}]}},
            {"kind": "automation", "key": "tell_me", "body": {
                "name": "Tell me about feedback", "enabled": true,
                "trigger": {"kind": "record_created", "collectionId": "@feedback"},
                "actions": [{"kind": "notify", "recipients": "owner",
                             "message": "New feedback: {{record.title}}"}]}},
            {"kind": "skill", "key": "triage", "body": {
                "name": "Triage", "description": "When sorting new feedback",
                "instructions": "Read each report and set its severity."}},
            {"kind": "agent", "key": "triage_agent", "body": {
                "name": "Triage agent", "purpose": "Sorts new feedback",
                "instructions": "Sort feedback as it arrives.", "skills": ["triage"]}}
        ],
        "needsAdmin": ["role:tester", "form:feedback_form"]
    })
}

#[tokio::test]
async fn a_package_installs_once_and_exports_back() {
    let Some(world) = WorldBuilder::default()
        .connected_app("desk")
        .build("m19_install_round_trip")
        .await
    else {
        return;
    };
    let pool = world.pool().clone();
    let preview = tool(
        &world,
        "desk",
        "package_preview",
        json!({"package": studio()}),
    )
    .await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    let plan = &preview["value"];
    assert!(
        plan["items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|line| line["action"] == "create"),
        "{plan}"
    );
    let hash = plan["planHash"].as_str().expect("hash").to_owned();
    // The preview changed nothing.
    let none =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM collections WHERE name = 'Feedback'")
            .fetch_one(&pool)
            .await
            .expect("collections");
    assert_eq!(none, 0);

    let install = json!({"package": studio(), "planHash": hash, "clientRequestId": "studio-1"});
    let first = tool(&world, "desk", "package_install", install.clone()).await;
    assert_eq!(first["outcome"], "ok", "{first}");
    let space = first["value"]["spaceId"]
        .as_str()
        .expect("space")
        .to_owned();
    let again = tool(&world, "desk", "package_install", install).await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(again["replayed"], true, "{again}");
    assert_eq!(again["value"]["installId"], first["value"]["installId"]);
    for (table, column, value) in [
        ("collections", "name", "Feedback"),
        ("bug_report_links", "name", "Feedback form"),
        ("automation_rules", "name", "Tell me about feedback"),
        ("project_pages", "title", "Triage"),
        ("agent_templates", "name", "Triage agent"),
        ("workspace_roles", "name", "Tester"),
    ] {
        let count = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT count(*) FROM {table} WHERE {column} = $1"
        ))
        .bind(value)
        .fetch_one(&pool)
        .await
        .expect("count");
        assert_eq!(count, 1, "{table} {value}");
    }
    let listed = tool(&world, "desk", "package_list", json!({})).await;
    assert_eq!(
        listed["value"]["installs"][0]["packageId"], "studio.game_studio",
        "{listed}"
    );
    let templates = tool(&world, "desk", "agent_template_list", json!({})).await;
    assert_eq!(
        templates["value"]["templates"][0]["key"], "triage_agent",
        "{templates}"
    );

    // Exported again, the space gives back the parts it was given.
    let exported = tool(&world, "desk", "package_export", json!({"spaceId": space})).await;
    assert_eq!(exported["outcome"], "ok", "{exported}");
    let parts = exported["value"]["package"]["contents"]
        .as_array()
        .expect("contents")
        .clone();
    let body_of = |kind: &str, key: &str| {
        parts
            .iter()
            .find(|part| part["kind"] == kind && part["key"] == key)
            .map(|part| part["body"].clone())
            .unwrap_or_else(|| panic!("{kind}:{key} in {parts:?}"))
    };
    let given = studio();
    let given_of = |key: &str| {
        given["contents"]
            .as_array()
            .expect("contents")
            .iter()
            .find(|part| part["key"] == key)
            .map(|part| part["body"].clone())
            .expect("given")
    };
    assert_eq!(body_of("form", "feedback_form"), given_of("feedback_form"));
    assert_eq!(body_of("skill", "triage"), given_of("triage"));
    assert_eq!(body_of("collection", "feedback")["name"], "Feedback");
    assert_eq!(body_of("role", "tester")["name"], "Tester");
    let rule = body_of("automation", "tell_me_about_feedback");
    assert_eq!(rule["trigger"]["collectionId"], "@feedback", "{rule}");

    // Removing it archives what it made and deletes nothing.
    let install_id = first["value"]["installId"]
        .as_str()
        .expect("install")
        .to_owned();
    let removed = tool(
        &world,
        "desk",
        "package_remove",
        json!({"installId": install_id, "parts": "archive", "clientRequestId": "studio-remove"}),
    )
    .await;
    assert_eq!(removed["outcome"], "ok", "{removed}");
    assert_eq!(removed["value"]["state"], "removed");
    let archived = sqlx::query_scalar::<_, bool>(
        "SELECT archived_at IS NOT NULL FROM collections WHERE name = 'Feedback'",
    )
    .fetch_one(&pool)
    .await
    .expect("collection");
    assert!(archived);
    world.finish().await;
}

fn bugs(with_role: bool) -> Value {
    let mut contents = vec![json!({"kind": "collection", "key": "bugs", "body": {
        "name": "Bugs", "titleName": "Name", "fields": [], "views": []}})];
    if with_role {
        contents.push(json!({"kind": "role", "key": "triager", "body": {
            "name": "Triager", "baseRole": "member", "permissions": ["tasks.view"]}}));
    }
    json!({
        "format": "agent-workspace-package/1",
        "id": "bugs",
        "version": "1.0.0",
        "name": "Bugs",
        "target": "space",
        "contents": contents,
    })
}

#[tokio::test]
async fn an_install_runs_only_the_plan_previewed_by_someone_allowed() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .connected_app("desk")
        .custom_role_on(
            "Builder",
            "admin",
            &["modules.manage", "collections.build", "tasks.view"],
        )
        .build("m19_install_rules")
        .await
    else {
        return;
    };
    let pool = world.pool().clone();
    let space = world.project("WORK").to_owned();
    // The agent holds Builder: it may manage modules, not roles.
    let builder = Uuid::parse_str(&world.roles["Builder"]).expect("role");
    let rook = world.agents["rook"].actor;
    for table in ["project_memberships", "organization_memberships"] {
        sqlx::query(&format!(
            "UPDATE {table} SET role_id = $1 WHERE actor_id = $2"
        ))
        .bind(builder)
        .bind(rook)
        .execute(&pool)
        .await
        .expect("give rook Builder");
    }
    let target = json!({"spaceId": space});

    // A role needs roles.manage.
    let preview = tool(
        &world,
        "rook",
        "package_preview",
        json!({"package": bugs(true), "target": target}),
    )
    .await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    let refused = tool(
        &world,
        "rook",
        "package_install",
        json!({"package": bugs(true), "target": target,
               "planHash": preview["value"]["planHash"], "clientRequestId": "bugs-role"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let triager =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workspace_roles WHERE name = 'Triager'")
            .fetch_one(&pool)
            .await
            .expect("roles");
    assert_eq!(triager, 0);

    // The same agent drafts one without the role, previews and installs it.
    let preview = tool(
        &world,
        "rook",
        "package_preview",
        json!({"package": bugs(false), "target": target}),
    )
    .await;
    let hash = preview["value"]["planHash"].clone();

    // Meanwhile someone makes a Bugs collection: the plan changed.
    let made = tool(
        &world,
        "desk",
        "collection_write",
        json!({"projectId": space, "name": "Bugs"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let stale = tool(
        &world,
        "rook",
        "package_install",
        json!({"package": bugs(false), "target": target, "planHash": hash,
               "clientRequestId": "bugs-stale"}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");

    // Previewed again, it reuses the collection and installs.
    let preview = tool(
        &world,
        "rook",
        "package_preview",
        json!({"package": bugs(false), "target": target}),
    )
    .await;
    assert_eq!(preview["value"]["items"][0]["action"], "reuse", "{preview}");
    let installed = tool(
        &world,
        "rook",
        "package_install",
        json!({"package": bugs(false), "target": target,
               "planHash": preview["value"]["planHash"], "clientRequestId": "bugs-ok"}),
    )
    .await;
    assert_eq!(installed["outcome"], "ok", "{installed}");
    assert_eq!(
        installed["value"]["objects"][0]["action"], "reused",
        "{installed}"
    );
    let collections =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM collections WHERE name = 'Bugs'")
            .fetch_one(&pool)
            .await
            .expect("collections");
    assert_eq!(collections, 1);
    world.finish().await;
}
