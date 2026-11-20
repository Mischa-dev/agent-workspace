//! K3.20: activity and the audit trail read from the log. The silent,
//! costly failures: an audit trail that loses who did it, through which
//! app or for whom, and a feed that shows a private conversation's events
//! to someone outside it.

use serde_json::{Value, json};

use super::support::{
    app::{send, tool},
    world::WorldBuilder,
};

fn events_where(body: &Value, test: impl Fn(&Value) -> bool) -> Vec<Value> {
    body["events"]
        .as_array()
        .unwrap_or_else(|| panic!("events in {body}"))
        .iter()
        .filter(|event| test(event))
        .cloned()
        .collect()
}

#[tokio::test]
async fn the_audit_trail_names_who_how_and_for_whom_and_keeps_private_conversations() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("ana", "member")
        .person("vera", "member")
        .agent("rook", "member")
        .connected_app("helper")
        .private_group("secret", &["ana"])
        .build("k3_20_activity")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let helper = &world.apps["helper"];
    let rook = &world.agents["rook"];
    let ana = world.people["ana"].actor;
    let vera = &world.people["vera"];
    // Vera administers the workspace but isn't in the private group.
    sqlx::query(
        "UPDATE organization_memberships SET role = 'admin', role_explicit = true \
         WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(world.organization)
    .bind(vera.actor)
    .execute(pool)
    .await
    .expect("vera is an admin");

    // Rook makes a task, asked by Ana.
    let created = tool(
        app,
        &rook.token,
        &rook.session,
        "task_create",
        json!({"projectId": world.project("WORK"), "title": "Paint", "clientRequestId": "k3-20-task"}),
    )
    .await;
    let task = created["workItemId"]
        .as_str()
        .or_else(|| created["value"]["workItemId"].as_str())
        .unwrap_or_else(|| panic!("task in {created}"))
        .to_owned();
    let staged = sqlx::query(
        "UPDATE event_staging SET requested_by_account_id = $1 \
         WHERE type = 'task.created' AND thing_id = $2::uuid",
    )
    .bind(ana)
    .bind(&task)
    .execute(pool)
    .await
    .expect("asked by Ana");
    assert_eq!(staged.rows_affected(), 1, "one task.created staged");

    // The owner renames it through their connected app.
    let version =
        sqlx::query_scalar::<_, i64>("SELECT version FROM work_items WHERE id = $1::uuid")
            .bind(&task)
            .fetch_one(pool)
            .await
            .expect("task version");
    let updated = tool(
        app,
        &helper.token,
        &helper.session,
        "task_update",
        json!({"taskId": task, "title": "Paint the hall", "expectedVersion": version}),
    )
    .await;
    assert!(
        matches!(updated["outcome"].as_str(), Some("ok" | "applied")),
        "{updated}"
    );

    // And says something in the private group.
    let said = tool(
        app,
        &helper.token,
        &helper.session,
        "message_send",
        json!({"conversationId": world.conversations["secret"], "text": "between us", "clientRequestId": "k3-20-secret"}),
    )
    .await;
    let message = said["value"]["messageId"]
        .as_str()
        .unwrap_or_else(|| panic!("message in {said}"))
        .to_owned();
    agent_workspace_server::events::catch_up(pool)
        .await
        .expect("catch up");

    let path = format!("/api/v1/organizations/{}/audit-events", world.organization);
    let (status, owner_log) = send(
        app,
        world.owner.browser.request("GET", &path, None, Value::Null),
    )
    .await;
    assert!(status.is_success(), "{status} {owner_log}");
    let rook_id = rook.actor.to_string();
    let made = events_where(&owner_log, |event| {
        event["action"] == "task.created" && event["target"]["id"] == task.as_str()
    });
    assert_eq!(made.len(), 1, "one entry for the new task: {owner_log}");
    assert_eq!(made[0]["actor"]["actorId"], rook_id.as_str(), "{made:?}");
    assert_eq!(
        made[0]["metadata"]["requestedBy"],
        ana.to_string(),
        "{made:?}"
    );
    let renamed = events_where(&owner_log, |event| {
        event["action"] == "task.updated" && event["target"]["id"] == task.as_str()
    });
    assert_eq!(renamed.len(), 1, "one entry for the rename: {owner_log}");
    assert_eq!(renamed[0]["app"]["clientName"], "helper", "{renamed:?}");
    assert_eq!(renamed[0]["app"]["channel"], "mcp", "{renamed:?}");
    assert!(
        renamed[0]["metadata"]["changes"]
            .as_array()
            .is_some_and(|changes| changes.iter().any(|key| key == "title")),
        "{renamed:?}"
    );
    // Content stays in its record.
    assert!(
        !owner_log.to_string().contains("Paint the hall"),
        "{owner_log}"
    );
    assert!(!owner_log.to_string().contains("between us"), "{owner_log}");
    // A group lives in no workspace (K4.15): its message is in no
    // workspace's trail, even for the owner, who is in the group.
    assert!(
        events_where(&owner_log, |event| event["target"]["id"]
            == message.as_str())
        .is_empty(),
        "the group's message stays out of the workspace's trail: {owner_log}"
    );

    // Vera, an admin outside the group, sees the task but none of the group.
    let (status, vera_log) = send(app, vera.browser.request("GET", &path, None, Value::Null)).await;
    assert!(status.is_success(), "{status} {vera_log}");
    assert!(
        !events_where(&vera_log, |event| event["target"]["id"] == task.as_str()).is_empty(),
        "{vera_log}"
    );
    let secret = world.conversations["secret"].as_str();
    assert!(
        events_where(&vera_log, |event| {
            event["target"]["id"] == message.as_str()
                || event["target"]["id"] == secret
                || event["metadata"]["conversation_id"] == secret
                || event["metadata"]["conversationId"] == secret
        })
        .is_empty(),
        "nothing of the private group: {vera_log}"
    );

    // The task's activity, newest first, for anyone who sees the task.
    let activity = tool(
        app,
        &rook.token,
        &rook.session,
        "activity_list",
        json!({"thing": task}),
    )
    .await;
    assert_eq!(activity["outcome"], "ok", "{activity}");
    let entries = activity["value"]["entries"].as_array().expect("entries");
    let types = entries
        .iter()
        .map(|entry| entry["type"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(types.first(), Some(&"task.updated"), "{activity}");
    assert!(types.contains(&"task.created"), "{activity}");
    let first = &entries[0];
    assert_eq!(first["via"], "helper", "{first}");
    assert_eq!(first["actorId"], world.owner.actor.to_string(), "{first}");
    let made = entries
        .iter()
        .find(|entry| entry["type"] == "task.created")
        .expect("created");
    assert_eq!(made["actor"], "rook", "{made}");
    assert_eq!(made["requestedById"], ana.to_string(), "{made}");
    // Paging: older than the newest leaves the newest out.
    let older = tool(
        app,
        &rook.token,
        &rook.session,
        "activity_list",
        json!({"thing": task, "before": first["position"]}),
    )
    .await;
    assert!(
        older["value"]["entries"]
            .as_array()
            .is_some_and(|older| older
                .iter()
                .all(|entry| entry["position"] != first["position"])),
        "{older}"
    );

    // Rook isn't in the group: its message and the group are not found.
    for thing in [message.as_str(), secret] {
        let hidden = tool(
            app,
            &rook.token,
            &rook.session,
            "activity_list",
            json!({"thing": thing}),
        )
        .await;
        assert_eq!(hidden["outcome"], "not_found", "{hidden}");
        assert!(!hidden.to_string().contains("between us"), "{hidden}");
    }
    let mine = tool(
        app,
        &helper.token,
        &helper.session,
        "activity_list",
        json!({"thing": message}),
    )
    .await;
    assert_eq!(mine["outcome"], "ok", "a member reads it: {mine}");
    world.finish().await;
}

/// K3 guard 3: the log carries ids, kinds and field names, never anyone's
/// words. Workspace B's world puts its marker into every text it writes
/// (messages, comments, pages, cells) and every name; after it is built and
/// the log has caught up, no event's payload holds the marker except under
/// a key that names something (a title or a name, which rule 10 allows).
#[tokio::test]
async fn the_log_carries_no_text() {
    let Some(fixture) = super::support::two_workspaces::two_workspaces("k3_21_no_text").await
    else {
        return;
    };
    agent_workspace_server::events::catch_up(&fixture.pool)
        .await
        .expect("catch up");
    let marker = fixture.b.marker.to_ascii_lowercase();
    let rows = sqlx::query_as::<_, (String, Value)>(
        "SELECT type, payload FROM events \
         UNION ALL SELECT type, payload FROM event_staging",
    )
    .fetch_all(&fixture.pool)
    .await
    .expect("events");
    assert!(!rows.is_empty(), "the world logged nothing");
    // The marker is really in B's words, and B's writes are in the log.
    let (said, logged) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT (SELECT count(*) FROM messages WHERE lower(plain_text) LIKE '%' || $1 || '%'), \
                (SELECT count(*) FROM events WHERE workspace_id = $2)",
    )
    .bind(&marker)
    .bind(fixture.b.organization)
    .fetch_one(&fixture.pool)
    .await
    .expect("sanity");
    assert!(said > 0 && logged > 0, "messages {said}, events {logged}");
    let mut found = Vec::new();
    for (ty, payload) in &rows {
        text_under(payload, &marker, "", &mut |path| {
            found.push(format!("{ty}: {path}"));
        });
    }
    found.sort();
    found.dedup();
    assert!(found.is_empty(), "text in the log: {found:#?}");
    fixture.finish().await;
}

/// Keys whose value names a thing rather than holding someone's words.
const NAME_KEYS: &[&str] = &[
    "title",
    "name",
    "displayName",
    "display_name",
    "key",
    "handle",
    "label",
    "slug",
];

fn text_under(value: &Value, marker: &str, path: &str, found: &mut impl FnMut(String)) {
    match value {
        Value::String(text) => {
            let key = path.rsplit('.').next().unwrap_or_default();
            if text.to_ascii_lowercase().contains(marker) && !NAME_KEYS.contains(&key) {
                found(path.to_owned());
            }
        }
        Value::Array(items) => {
            for item in items {
                text_under(item, marker, path, found);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                text_under(item, marker, &format!("{path}.{key}"), found);
            }
        }
        _ => {}
    }
}
