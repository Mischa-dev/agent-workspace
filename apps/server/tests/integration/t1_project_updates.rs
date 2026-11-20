//! T1.43: project status and update posts. An update sets the space's
//! health and shows on its home, in its members' My work and in the
//! initiatives its milestones are part of; an agent posts one through the
//! tool; someone who can't read the space never sees it.

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::support::{
    callers::{Call, Caller},
    world::WorldBuilder,
};

#[tokio::test]
async fn an_update_sets_health_and_reaches_members() {
    // Server errors show in the output.
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("agent_workspace=error")
        .try_init();
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .project("SIDE")
        .person("lena", "member")
        .person("vic", "viewer")
        .agent("rook", "member")
        .connected_app("laptop")
        .build("t1_project_updates")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let laptop = &world.apps["laptop"];
    let rook = &world.agents["rook"];
    let lena = &world.people["lena"];
    let vic = &world.people["vic"];
    let tool = |name: &'static str, arguments: Value| Call::Tool { name, arguments };
    // What a person's browser runs.
    let op = |name: &str, body: Value| Call::Http {
        method: "POST",
        path: format!("/api/v2/ops/{name}"),
        body,
    };

    // Nothing set yet.
    let (_, empty) = laptop
        .call(app, tool("project_status_get", json!({"spaceId": "WORK"})))
        .await;
    assert_eq!(empty["outcome"], "ok", "{empty}");
    assert_eq!(empty["value"]["version"], 0, "{empty}");
    assert!(empty["value"]["health"].is_null(), "{empty}");

    // The owner sets a target date and makes Lena the lead.
    let (_, set) = laptop
        .call(
            app,
            tool(
                "project_status_set",
                json!({"spaceId": "WORK", "targetDate": "2026-12-31", "lead": "lena",
                       "health": "On track", "clientRequestId": "status-1"}),
            ),
        )
        .await;
    assert_eq!(set["outcome"], "ok", "{set}");
    assert_eq!(set["value"]["health"], "on_track", "{set}");
    assert_eq!(set["value"]["targetDate"], "2026-12-31", "{set}");
    assert_eq!(set["value"]["lead"]["id"], lena.actor.to_string(), "{set}");
    let (_, stale) = laptop
        .call(
            app,
            tool(
                "project_status_set",
                json!({"spaceId": "WORK", "health": "off_track", "expectedVersion": 0,
                       "clientRequestId": "status-stale"}),
            ),
        )
        .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");

    // An update needs a channel to go in.
    let (_, channel) = laptop
        .call(
            app,
            tool(
                "channel_create",
                json!({"projectId": "WORK", "name": "general"}),
            ),
        )
        .await;
    assert_eq!(channel["outcome"], "ok", "{channel}");

    // An update too long to post changes nothing: no thread, no health.
    let (_, refused) = rook
        .call(
            app,
            tool(
                "project_update_create",
                json!({"spaceId": "WORK", "health": "off_track",
                       "text": "a".repeat(20_001), "clientRequestId": "too-long"}),
            ),
        )
        .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    let updates = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM conversations WHERE thread_outcome IS NOT NULL",
    )
    .fetch_one(pool)
    .await
    .expect("threads");
    assert_eq!(updates, 0, "{refused}");
    let health = sqlx::query_scalar::<_, Option<String>>("SELECT health FROM space_statuses")
        .fetch_one(pool)
        .await
        .expect("health");
    assert_eq!(health.as_deref(), Some("on_track"), "{refused}");

    // An agent posts the weekly update, at risk: it becomes the health.
    let post = json!({"spaceId": "WORK", "health": "at_risk",
                      "text": "Beta slipped a week: the importer needs another pass.",
                      "clientRequestId": "weekly-1"});
    let (_, posted) = rook
        .call(app, tool("project_update_create", post.clone()))
        .await;
    assert_eq!(posted["outcome"], "ok", "{posted}");
    let thread = posted["value"]["threadId"]
        .as_str()
        .expect("thread")
        .to_owned();
    assert_eq!(posted["value"]["status"]["health"], "at_risk", "{posted}");
    assert_eq!(
        posted["value"]["status"]["latestUpdate"]["threadId"],
        thread.as_str()
    );
    // A retry posts once.
    let (_, again) = rook.call(app, tool("project_update_create", post)).await;
    assert_eq!(again["value"]["threadId"], thread.as_str(), "{again}");
    let messages = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM messages WHERE conversation_id = $1::uuid",
    )
    .bind(&thread)
    .fetch_one(pool)
    .await
    .expect("messages");
    assert_eq!(messages, 1);
    // The thread's outcome is the health it set.
    let outcome = sqlx::query_scalar::<_, Option<String>>(
        "SELECT thread_outcome FROM conversations WHERE id = $1::uuid",
    )
    .bind(&thread)
    .fetch_one(pool)
    .await
    .expect("outcome");
    assert_eq!(outcome.as_deref(), Some("At risk"));

    // On the space's home (its status) and in a member's My work.
    let (_, home) = lena
        .call(
            app,
            op(
                "project_status_get",
                json!({"spaceId": world.project("WORK")}),
            ),
        )
        .await;
    assert_eq!(home["value"]["healthLabel"], "At risk", "{home}");
    assert_eq!(home["value"]["latestUpdate"]["health"], "At risk", "{home}");
    let (status, my_work) = lena
        .call(
            app,
            Call::Http {
                method: "GET",
                path: "/api/v1/my-work".to_owned(),
                body: Value::Null,
            },
        )
        .await;
    assert!(status.is_success(), "{my_work}");
    let updates = my_work["updates"].as_array().expect("updates");
    assert_eq!(updates.len(), 1, "{my_work}");
    assert_eq!(updates[0]["spaceKey"], "WORK");
    assert_eq!(updates[0]["latestUpdate"]["threadId"], thread.as_str());
    let (_, agent_work) = rook.call(app, tool("my_work", json!({}))).await;
    assert_eq!(
        agent_work["value"]["updates"][0]["health"], "at_risk",
        "{agent_work}"
    );

    // A viewer reads the status but can't post or change it.
    let (viewer_status, viewer_post) = vic
        .call(
            app,
            op(
                "project_update_create",
                json!({"spaceId": world.project("WORK"), "health": "on_track",
                       "text": "All good"}),
            ),
        )
        .await;
    assert_eq!(viewer_status, StatusCode::FORBIDDEN, "{viewer_post}");

    // In the initiative WORK's milestone is part of.
    let (_, made) = laptop
        .call(
            app,
            tool(
                "initiative_create",
                json!({"title": "Launch", "clientRequestId": "launch"}),
            ),
        )
        .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let (_, milestone) = laptop
        .call(
            app,
            tool(
                "milestone_write",
                json!({"projectId": "WORK", "name": "Beta", "initiative": "Launch",
                       "clientRequestId": "beta"}),
            ),
        )
        .await;
    assert_eq!(milestone["outcome"], "ok", "{milestone}");
    let (_, initiatives) = lena.call(app, op("initiative_list", json!({}))).await;
    let spaces = &initiatives["value"]["initiatives"][0]["spaces"];
    assert_eq!(spaces[0]["spaceKey"], "WORK", "{initiatives}");
    assert_eq!(spaces[0]["health"], "at_risk", "{initiatives}");

    // Lena leaves WORK: its update leaves her My work.
    sqlx::query(
        "UPDATE project_memberships SET enabled = false \
         WHERE actor_id = $1 AND project_id = $2::uuid",
    )
    .bind(lena.actor)
    .bind(world.project("WORK"))
    .execute(pool)
    .await
    .expect("leave WORK");
    let (_, after) = lena
        .call(
            app,
            Call::Http {
                method: "GET",
                path: "/api/v1/my-work".to_owned(),
                body: Value::Null,
            },
        )
        .await;
    assert_eq!(after["updates"], json!([]), "{after}");
    assert!(!after.to_string().contains(&thread), "{after}");

    world.finish().await;
}
