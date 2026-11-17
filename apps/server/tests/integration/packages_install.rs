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
    // Open pages hear that the skill page went.
    let page_heard = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM (SELECT thing_id, thing_version, type FROM event_staging \
                 UNION ALL SELECT thing_id, thing_version, type FROM events) event \
         JOIN project_pages page ON page.id = event.thing_id \
         WHERE event.type = 'page.content_saved' AND page.title = 'Triage' \
           AND page.archived_at IS NOT NULL AND event.thing_version = page.version",
    )
    .fetch_one(&pool)
    .await
    .expect("page events");
    assert_eq!(page_heard, 1);
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
        .custom_role_on(
            "Steward",
            "admin",
            &[
                "roles.manage",
                "settings.manage",
                "modules.manage",
                "collections.build",
                "tasks.view",
            ],
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
    // The space may be named by its key, as anywhere else.
    let by_key = tool(
        &world,
        "rook",
        "package_preview",
        json!({"package": bugs(true), "target": {"spaceId": "WORK"}}),
    )
    .await;
    assert_eq!(
        by_key["value"]["planHash"], preview["value"]["planHash"],
        "{by_key}"
    );
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

    // A package whose module holds the collection it reused: removing it
    // archives the module, and the collection stays as it was, in sight.
    let tracker = json!({
        "format": "agent-workspace-package/1",
        "id": "tracker",
        "version": "1.0.0",
        "name": "Tracker",
        "target": "space",
        "contents": [
            {"kind": "collection", "key": "bugs", "body": {
                "name": "Bugs", "titleName": "Name", "fields": [], "views": []}},
            {"kind": "module", "key": "tracker", "body": {
                "name": "Tracker", "parts": [{"kind": "collection", "key": "bugs"}]}}
        ],
    });
    let preview = tool(
        &world,
        "rook",
        "package_preview",
        json!({"package": tracker, "target": target}),
    )
    .await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    let installed = tool(
        &world,
        "rook",
        "package_install",
        json!({"package": tracker, "target": target,
               "planHash": preview["value"]["planHash"], "clientRequestId": "tracker"}),
    )
    .await;
    assert_eq!(installed["outcome"], "ok", "{installed}");
    let removed = tool(
        &world,
        "rook",
        "package_remove",
        json!({"installId": installed["value"]["installId"], "parts": "archive",
               "clientRequestId": "tracker-remove"}),
    )
    .await;
    assert_eq!(removed["outcome"], "ok", "{removed}");
    let (archived, shown) = sqlx::query_as::<_, (bool, bool)>(
        "SELECT archived_at IS NOT NULL, module_part_on('collection', id) \
         FROM collections WHERE name = 'Bugs'",
    )
    .fetch_one(&pool)
    .await
    .expect("bugs");
    assert!(!archived && shown, "the reused collection stays in sight");
    let module_archived = sqlx::query_scalar::<_, bool>(
        "SELECT archived_at IS NOT NULL FROM modules WHERE name = 'Tracker'",
    )
    .fetch_one(&pool)
    .await
    .expect("module");
    assert!(module_archived);

    // Holding roles.manage in the space isn't holding it in the workspace,
    // whose roles a package changes.
    let steward = Uuid::parse_str(&world.roles["Steward"]).expect("role");
    sqlx::query("UPDATE project_memberships SET role_id = $1 WHERE actor_id = $2")
        .bind(steward)
        .bind(rook)
        .execute(&pool)
        .await
        .expect("give rook Steward in the space");
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
               "planHash": preview["value"]["planHash"], "clientRequestId": "bugs-steward"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    world.finish().await;
}

/// A members-only channel the caller isn't on is never named to them by a
/// preview: a package naming it shows it reused, without its id.
#[tokio::test]
async fn a_preview_never_reveals_a_channel_the_caller_cannot_see() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("desk")
        .build("m19_hidden_channel")
        .await
    else {
        return;
    };
    let pool = world.pool().clone();
    let space = world.project("WORK").to_owned();
    let hidden = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO conversations (id, project_id, kind, name, visibility) \
         VALUES ($1, $2::uuid, 'channel', 'war-room', 'members')",
    )
    .bind(hidden)
    .bind(&space)
    .execute(&pool)
    .await
    .expect("members-only channel");
    let package = json!({
        "format": "agent-workspace-package/1",
        "id": "studio.rooms",
        "version": "1.0.0",
        "name": "Rooms",
        "target": "space",
        "contents": [
            {"kind": "channels", "key": "channels",
             "body": {"channels": [{"name": "war-room"}]}}
        ]
    });
    // The owner's app: an owner too reads a members-only channel only from
    // its member list.
    let preview = tool(
        &world,
        "desk",
        "package_preview",
        json!({"package": package, "target": {"spaceId": space}}),
    )
    .await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    assert!(
        !preview.to_string().contains(&hidden.to_string()),
        "{preview}"
    );
    world.finish().await;
}

#[tokio::test]
async fn a_space_install_and_template_show_only_to_its_members() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("desk")
        .build("m19_template_space")
        .await
    else {
        return;
    };
    let pool = world.pool().clone();
    let space = world.project("WORK").to_owned();
    sqlx::query(
        "INSERT INTO agent_templates (id, workspace_id, space_id, template_key, name, definition) \
         SELECT $1, organization_id, id, 'closed_helper', 'Closed helper', \
                '{\"instructions\": \"private\"}'::jsonb \
         FROM projects WHERE id = $2::uuid",
    )
    .bind(Uuid::now_v7())
    .bind(&space)
    .execute(&pool)
    .await
    .expect("space template");
    let package_row = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO packages (id, workspace_id, package_key, version, name, document, digest, source) \
         SELECT $1, organization_id, 'closed_kit', '1.0.0', 'Closed kit', '{}'::jsonb, 'd', 'upload' \
         FROM projects WHERE id = $2::uuid",
    )
    .bind(package_row)
    .bind(&space)
    .execute(&pool)
    .await
    .expect("package");
    sqlx::query(
        "INSERT INTO package_installs (id, workspace_id, space_id, package_id, state, plan, \
             plan_hash, summary, installed_by_actor_id) \
         SELECT $1, organization_id, id, $3, 'installed', '{}'::jsonb, 'h', \
                ARRAY['Made #private-room'], (SELECT actor_id FROM users LIMIT 1) \
         FROM projects WHERE id = $2::uuid",
    )
    .bind(Uuid::now_v7())
    .bind(&space)
    .bind(package_row)
    .execute(&pool)
    .await
    .expect("space install");
    let set_membership = |enabled: bool| {
        let pool = pool.clone();
        let space = space.clone();
        async move {
            sqlx::query("UPDATE project_memberships SET enabled = $1 WHERE project_id = $2::uuid")
                .bind(enabled)
                .bind(&space)
                .execute(&pool)
                .await
                .expect("membership");
        }
    };
    // The owner's app, out of the space, lists the workspace's templates.
    set_membership(false).await;
    let hidden = tool(&world, "desk", "agent_template_list", json!({})).await;
    assert_eq!(hidden["outcome"], "ok", "{hidden}");
    assert_eq!(hidden["value"]["templates"], json!([]), "{hidden}");
    let installs = tool(&world, "desk", "package_list", json!({})).await;
    assert_eq!(installs["value"]["installs"], json!([]), "{installs}");
    // A preview naming its key doesn't learn its id or reuse it.
    let package = json!({
        "format": "agent-workspace-package/1",
        "id": "helpers",
        "version": "1.0.0",
        "name": "Helpers",
        "target": "workspace",
        "contents": [{"kind": "agent", "key": "closed_helper", "body": {
            "name": "Closed helper", "purpose": "Helps", "instructions": "Help."}}],
    });
    let preview = tool(
        &world,
        "desk",
        "package_preview",
        json!({"package": package}),
    )
    .await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    assert_eq!(preview["value"]["items"][0]["action"], "skip", "{preview}");
    assert!(
        preview["value"]["items"][0]["existingId"].is_null(),
        "{preview}"
    );
    set_membership(true).await;
    let installs = tool(&world, "desk", "package_list", json!({})).await;
    assert_eq!(
        installs["value"]["installs"][0]["packageId"], "closed_kit",
        "{installs}"
    );
    let shown = tool(&world, "desk", "agent_template_list", json!({})).await;
    assert_eq!(
        shown["value"]["templates"][0]["key"], "closed_helper",
        "{shown}"
    );
    world.finish().await;
}

/// A package that makes its own space plans against the space the install
/// will really use: one the installer made before, never another person's
/// space that happens to have the same key and name.
#[tokio::test]
async fn a_new_space_is_planned_as_the_install_makes_it() {
    let Some(world) = WorldBuilder::default()
        .project("STU")
        .connected_app("desk")
        .build("m19_new_space_landing")
        .await
    else {
        return;
    };
    let pool = world.pool().clone();
    let theirs = world.project("STU").to_owned();
    // STU (name STU) is a space the installer is in but doesn't own.
    sqlx::query(
        "UPDATE project_memberships SET role = 'member' \
         WHERE project_id = $1::uuid AND actor_id = $2",
    )
    .bind(&theirs)
    .bind(world.owner.actor)
    .execute(&pool)
    .await
    .expect("someone else's space");
    // It already has a Feedback collection.
    let made = tool(
        &world,
        "desk",
        "collection_write",
        json!({"projectId": theirs, "name": "Feedback"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let package = json!({
        "format": "agent-workspace-package/1",
        "id": "studio",
        "version": "1.0.0",
        "name": "Studio",
        "target": "space",
        "contents": [
            {"kind": "space", "key": "studio", "body": {"name": "STU", "key": "STU"}},
            {"kind": "collection", "key": "feedback", "body": {
                "name": "Feedback", "titleName": "Name", "fields": [], "views": []}}
        ],
    });
    let preview = tool(
        &world,
        "desk",
        "package_preview",
        json!({"package": package}),
    )
    .await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    assert!(
        preview["value"]["items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|line| line["action"] == "create"),
        "another person's space is never planned against: {preview}"
    );
    let installed = tool(
        &world,
        "desk",
        "package_install",
        json!({"package": package, "planHash": preview["value"]["planHash"],
               "clientRequestId": "studio-new"}),
    )
    .await;
    assert_eq!(installed["outcome"], "ok", "{installed}");
    let landed = installed["value"]["spaceId"].as_str().expect("space");
    assert_ne!(landed, theirs);
    let feedback = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM collections WHERE name = 'Feedback' AND project_id = $1::uuid",
    )
    .bind(landed)
    .fetch_one(&pool)
    .await
    .expect("feedback");
    assert_eq!(feedback, 1);
    world.finish().await;
}
