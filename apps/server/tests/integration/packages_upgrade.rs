//! M1.10: upgrading a package. The silent failures: an upgrade that
//! overwrites what someone changed here, one that deletes what the new
//! version dropped, one that changes a field's type under its values, and
//! one that runs a plan the person never saw.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use sqlx::PgPool;

use super::support::{
    callers::{Call, Caller},
    world::{Workspace, WorldBuilder},
};

async fn tool(world: &Workspace, name: &'static str, arguments: Value) -> Value {
    let call = Call::Tool { name, arguments };
    world.apps["desk"].call(&world.app, call).await.1
}

fn studio(version: &str, page: &str, skill: &str, with_agent: bool, score: &str) -> Value {
    let mut contents = vec![
        json!({"kind": "space", "key": "studio", "body": {"name": "Studio", "key": "STU"}}),
        json!({"kind": "channels", "key": "channels", "body": {"channels":
            if with_agent { json!([{"name": "playtests"}, {"name": "lobby"}]) }
            else { json!([{"name": "playtests"}, {"name": "builds"}]) }}}),
        json!({"kind": "collection", "key": "feedback", "body": {
            "name": "Feedback", "titleName": "Name",
            "fields": if with_agent {
                json!([{"name": "Notes", "type": "text"}, {"name": "Score", "type": score}])
            } else {
                json!([{"name": "Notes", "type": "text"}, {"name": "Score", "type": score},
                       {"name": "Platform", "type": "text"}])
            },
            "views": []}}),
        json!({"kind": "page", "key": "guide", "body": {"title": "Studio guide", "markdown": page}}),
        json!({"kind": "skill", "key": "triage", "body": {
            "name": "Triage", "description": "When sorting new feedback", "instructions": skill}}),
    ];
    if with_agent {
        contents.push(json!({"kind": "agent", "key": "triage_agent", "body": {
            "name": "Triage agent", "purpose": "Sorts new feedback",
            "instructions": "Sort feedback as it arrives."}}));
    }
    json!({
        "format": "agent-workspace-package/1",
        "id": "studio.game_studio",
        "version": version,
        "name": "Game studio",
        "target": "space",
        "contents": contents,
    })
}

fn actions(plan: &Value) -> BTreeMap<String, String> {
    plan["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| {
            (
                format!(
                    "{}:{}",
                    item["kind"].as_str().unwrap_or(""),
                    item["key"].as_str().unwrap_or("")
                ),
                item["action"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect()
}

async fn page_body(pool: &PgPool, space: &str, title: &str) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT body FROM project_pages WHERE project_id = $1::uuid AND title = $2",
    )
    .bind(space)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("page")
}

#[tokio::test]
async fn an_upgrade_keeps_local_changes_detaches_and_only_grows() {
    let Some(world) = WorldBuilder::default()
        .connected_app("desk")
        .build("m110_upgrade")
        .await
    else {
        return;
    };
    let pool = world.pool().clone();

    // Version 1, installed.
    let v1 = studio("1.0.0", "Welcome.", "Set a severity.", true, "number");
    let preview = tool(&world, "package_preview", json!({"package": v1})).await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    let installed = tool(
        &world,
        "package_install",
        json!({"package": v1, "planHash": preview["value"]["planHash"],
               "clientRequestId": "studio-v1"}),
    )
    .await;
    assert_eq!(installed["outcome"], "ok", "{installed}");
    let install = installed["value"]["installId"]
        .as_str()
        .expect("install")
        .to_owned();
    let space = installed["value"]["spaceId"]
        .as_str()
        .expect("space")
        .to_owned();
    let fingerprints = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM package_objects WHERE install_id = $1::uuid AND local_hash IS NULL",
    )
    .bind(&install)
    .fetch_one(&pool)
    .await
    .expect("fingerprints");
    assert_eq!(fingerprints, 0, "every object is fingerprinted at install");

    // Someone edits the guide here.
    sqlx::query(
        "UPDATE project_pages SET body = 'Our own welcome.' \
         WHERE project_id = $1::uuid AND title = 'Studio guide'",
    )
    .bind(&space)
    .execute(&pool)
    .await
    .expect("edit");

    // Version 2: the guide and the skill change upstream, the agent and
    // #lobby are dropped, #builds and a Platform field are new, and Score
    // becomes text.
    let v2 = studio(
        "2.0.0",
        "Welcome, v2.",
        "Set a severity and a platform.",
        false,
        "text",
    );
    let preview = tool(
        &world,
        "package_upgrade_preview",
        json!({"installId": install, "package": v2}),
    )
    .await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    let plan = &preview["value"];
    assert_eq!(plan["fromVersion"], "1.0.0");
    assert_eq!(plan["major"], true);
    let lines = actions(plan);
    assert_eq!(lines["page:guide"], "conflict", "{plan}");
    assert_eq!(lines["skill:triage"], "update", "{plan}");
    assert_eq!(lines["agent:triage_agent"], "detach", "{plan}");
    assert_eq!(lines["channel:lobby"], "detach", "{plan}");
    assert_eq!(lines["channel:builds"], "create", "{plan}");
    assert_eq!(lines["channel:playtests"], "keep", "{plan}");
    assert_eq!(lines["collection:feedback"], "update", "{plan}");
    // The preview changed nothing.
    assert_eq!(page_body(&pool, &space, "Triage").await, "Set a severity.");

    // A stale plan is refused; so is a choice that names no conflict.
    let stale = tool(
        &world,
        "package_upgrade",
        json!({"installId": install, "package": v2, "planHash": "sha256:nope",
               "clientRequestId": "studio-v2-stale"}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    let wrong = tool(
        &world,
        "package_upgrade",
        json!({"installId": install, "package": v2, "planHash": plan["planHash"],
               "choices": {"skill:triage": "take_theirs"},
               "clientRequestId": "studio-v2-wrong"}),
    )
    .await;
    assert_eq!(wrong["outcome"], "invalid_request", "{wrong}");

    // Upgrading keeps the edited guide, takes the untouched skill, and
    // deletes nothing.
    let upgraded = tool(
        &world,
        "package_upgrade",
        json!({"installId": install, "package": v2, "planHash": plan["planHash"],
               "clientRequestId": "studio-v2"}),
    )
    .await;
    assert_eq!(upgraded["outcome"], "ok", "{upgraded}");
    let value = &upgraded["value"];
    let second = value["installId"].as_str().expect("install").to_owned();
    assert_eq!(value["upgradedFrom"], install.as_str());
    assert_eq!(
        page_body(&pool, &space, "Studio guide").await,
        "Our own welcome."
    );
    assert_eq!(
        page_body(&pool, &space, "Triage").await,
        "Set a severity and a platform."
    );
    let agent = sqlx::query_scalar::<_, bool>(
        "SELECT archived_at IS NULL FROM agent_templates WHERE name = 'Triage agent'",
    )
    .fetch_one(&pool)
    .await
    .expect("agent template stays");
    assert!(agent);
    let lobby = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM conversations WHERE project_id = $1::uuid AND name = 'lobby' \
         AND archived_at IS NULL",
    )
    .bind(&space)
    .fetch_one(&pool)
    .await
    .expect("lobby");
    assert_eq!(lobby, 1, "a channel the package dropped stays");
    let builds = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM conversations WHERE project_id = $1::uuid AND name = 'builds'",
    )
    .bind(&space)
    .fetch_one(&pool)
    .await
    .expect("builds");
    assert_eq!(builds, 1);
    // Score keeps its type and values; the new type comes in beside it.
    let fields = sqlx::query_as::<_, (String, String)>(
        "SELECT field.name, field.field_type FROM field_definitions field \
         JOIN collections collection ON collection.id = field.collection_id \
         WHERE collection.project_id = $1::uuid AND collection.name = 'Feedback' \
           AND field.archived_at IS NULL ORDER BY field.position",
    )
    .bind(&space)
    .fetch_all(&pool)
    .await
    .expect("fields");
    assert!(
        fields.contains(&("Score".to_owned(), "number".to_owned())),
        "{fields:?}"
    );
    assert!(
        fields.contains(&("Score (text)".to_owned(), "text".to_owned())),
        "{fields:?}"
    );
    assert!(
        fields.iter().any(|(name, _)| name == "Platform"),
        "{fields:?}"
    );
    let states = sqlx::query_as::<_, (String, String)>(
        "SELECT id::text, state FROM package_installs WHERE id IN ($1::uuid, $2::uuid)",
    )
    .bind(&install)
    .bind(&second)
    .fetch_all(&pool)
    .await
    .expect("states");
    assert!(states.contains(&(install.clone(), "superseded".to_owned())));
    assert!(states.contains(&(second.clone(), "installed".to_owned())));
    let detached = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM package_objects WHERE install_id = $1::uuid AND action = 'detached'",
    )
    .bind(&second)
    .fetch_one(&pool)
    .await
    .expect("detached");
    assert_eq!(detached, 2, "the agent template and #lobby");

    // Nor removed: its parts are the new install's now, or left as they are.
    let removed = tool(
        &world,
        "package_remove",
        json!({"installId": install, "parts": "archive", "clientRequestId": "remove-old"}),
    )
    .await;
    assert_eq!(removed["outcome"], "conflict", "{removed}");

    // The old install can't be upgraded again.
    let old = tool(
        &world,
        "package_upgrade_preview",
        json!({"installId": install, "package": v2}),
    )
    .await;
    assert_eq!(old["outcome"], "invalid_request", "{old}");

    // Version 3 changes the guide again: still a conflict, and taking theirs
    // applies it. The skill, untouched since, has nothing new.
    let v3 = studio(
        "2.1.0",
        "Welcome, v3.",
        "Set a severity and a platform.",
        false,
        "text",
    );
    let preview = tool(
        &world,
        "package_upgrade_preview",
        json!({"installId": second, "package": v3}),
    )
    .await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    let lines = actions(&preview["value"]);
    assert_eq!(lines["page:guide"], "conflict", "{preview}");
    assert_eq!(lines["skill:triage"], "keep", "{preview}");
    assert_eq!(lines["collection:feedback"], "keep", "{preview}");
    assert!(!lines.contains_key("agent:triage_agent"), "{preview}");
    // Someone edits the guide again after that preview: its plan is stale,
    // so taking theirs can't overwrite an edit nobody saw.
    sqlx::query(
        "UPDATE project_pages SET body = 'Welcome, edited again.' \
         WHERE project_id = $1::uuid AND title = 'Studio guide'",
    )
    .bind(&space)
    .execute(&pool)
    .await
    .expect("edit the guide");
    let stale = tool(
        &world,
        "package_upgrade",
        json!({"installId": second, "package": v3,
               "planHash": preview["value"]["planHash"],
               "choices": {"page:guide": "take_theirs"},
               "clientRequestId": "studio-v3-stale"}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    assert_eq!(
        page_body(&pool, &space, "Studio guide").await,
        "Welcome, edited again."
    );
    let preview = tool(
        &world,
        "package_upgrade_preview",
        json!({"installId": second, "package": v3}),
    )
    .await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    let upgraded = tool(
        &world,
        "package_upgrade",
        json!({"installId": second, "package": v3,
               "planHash": preview["value"]["planHash"],
               "choices": {"page:guide": "take_theirs"},
               "clientRequestId": "studio-v3"}),
    )
    .await;
    assert_eq!(upgraded["outcome"], "ok", "{upgraded}");
    assert_eq!(
        page_body(&pool, &space, "Studio guide").await,
        "Welcome, v3."
    );
}

fn handbook(version: &str, role: &str, with_module: bool) -> Value {
    let mut contents = vec![
        json!({"kind": "role", "key": "editor", "body": {
            "name": role, "baseRole": "member", "permissions": ["tasks.view"]}}),
        json!({"kind": "space", "key": "handbook", "body": {"name": "Handbook", "key": "HBK"}}),
        json!({"kind": "page", "key": "guide", "body": {"title": "Handbook guide", "markdown": "Read me."}}),
    ];
    if with_module {
        contents.push(json!({"kind": "module", "key": "handbook", "body": {
            "name": "Handbook", "parts": [{"kind": "page", "key": "guide"}]}}));
    }
    json!({
        "format": "agent-workspace-package/1",
        "id": "handbook",
        "version": version,
        "name": "Handbook",
        "target": "space",
        "contents": contents,
    })
}

/// A new version can name what an earlier one made and the upgrade leaves
/// be (a new module holding the old page), and a role renamed upstream is
/// the same role, renamed, not a second one beside it.
#[tokio::test]
async fn an_upgrade_builds_on_what_it_keeps_and_renames_its_role() {
    let Some(world) = WorldBuilder::default()
        .connected_app("desk")
        .build("m110_upgrade_kept")
        .await
    else {
        return;
    };
    let pool = world.pool().clone();
    let v1 = handbook("1.0.0", "Editor", false);
    let preview = tool(&world, "package_preview", json!({"package": v1})).await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    let installed = tool(
        &world,
        "package_install",
        json!({"package": v1, "planHash": preview["value"]["planHash"],
               "clientRequestId": "handbook-v1"}),
    )
    .await;
    assert_eq!(installed["outcome"], "ok", "{installed}");
    let install = installed["value"]["installId"]
        .as_str()
        .expect("install")
        .to_owned();
    let role = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT object_id FROM package_objects WHERE install_id = $1::uuid AND object_kind = 'role'",
    )
    .bind(&install)
    .fetch_one(&pool)
    .await
    .expect("role");

    let v2 = handbook("1.1.0", "Writer", true);
    let preview = tool(
        &world,
        "package_upgrade_preview",
        json!({"installId": install, "package": v2}),
    )
    .await;
    assert_eq!(preview["outcome"], "ok", "{preview}");
    let lines = actions(&preview["value"]);
    assert_eq!(lines["role:editor"], "update", "{preview}");
    assert_eq!(lines["module:handbook"], "create", "{preview}");
    let upgraded = tool(
        &world,
        "package_upgrade",
        json!({"installId": install, "package": v2, "planHash": preview["value"]["planHash"],
               "clientRequestId": "handbook-v2"}),
    )
    .await;
    assert_eq!(upgraded["outcome"], "ok", "{upgraded}");
    let second = upgraded["value"]["installId"]
        .as_str()
        .expect("install")
        .to_owned();

    let roles = sqlx::query_as::<_, (uuid::Uuid, String)>(
        "SELECT id, name FROM workspace_roles WHERE lower(name) IN ('editor', 'writer')",
    )
    .fetch_all(&pool)
    .await
    .expect("roles");
    assert_eq!(roles, vec![(role, "Writer".to_owned())]);
    let tracked = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT object_id FROM package_objects WHERE install_id = $1::uuid AND object_kind = 'role'",
    )
    .bind(&second)
    .fetch_one(&pool)
    .await
    .expect("tracked role");
    assert_eq!(tracked, role);
    let held = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM module_parts p JOIN modules m ON m.id = p.module_id \
         JOIN project_pages g ON g.id = p.object_id \
         WHERE m.package_install_id = $1::uuid AND g.title = 'Handbook guide'",
    )
    .bind(&second)
    .fetch_one(&pool)
    .await
    .expect("module parts");
    assert_eq!(
        held, 1,
        "the new module holds the page the first version made"
    );
}
