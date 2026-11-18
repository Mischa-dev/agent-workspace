//! T1.26: initiatives. The silent, costly failure: an initiative's progress
//! counting milestones (and their tasks) in a space the reader can't see.

use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    callers::{Call, Caller},
    world::WorldBuilder,
};

#[tokio::test]
async fn an_initiative_counts_only_the_spaces_its_reader_can_see() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .project("SIDE")
        .agent("rook", "member")
        .connected_app("laptop")
        .build("t1_initiatives")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let laptop = &world.apps["laptop"];
    let rook = &world.agents["rook"];
    let call = |name: &'static str, arguments: Value| async move {
        laptop.call(app, Call::Tool { name, arguments }).await.1
    };

    // Nothing yet: no collection, no initiatives.
    let empty = call("initiative_list", json!({})).await;
    assert_eq!(empty["outcome"], "ok", "{empty}");
    assert!(empty["value"]["collectionId"].is_null(), "{empty}");

    // The first one makes the workspace's Initiatives collection.
    let made = call(
        "initiative_create",
        json!({"title": "Launch", "fields": {"Health": "At risk", "Status": "Active"},
               "clientRequestId": "launch"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let launch = made["value"]["id"].as_str().expect("id").to_owned();
    let again = call(
        "initiative_create",
        json!({"title": "Launch", "fields": {"Health": "At risk", "Status": "Active"},
               "clientRequestId": "launch"}),
    )
    .await;
    assert_eq!(again["value"]["id"], launch.as_str(), "{again}");

    // A milestone in each space, part of it, with tasks: WORK 1 of 2 done
    // (and a canceled one left out), SIDE 2 of 3.
    for (space, milestone, done, open) in [("WORK", "Beta", 1, 1), ("SIDE", "Docs", 2, 1)] {
        let written = call(
            "milestone_write",
            json!({"projectId": space, "name": milestone, "initiative": "Launch",
                   "clientRequestId": format!("{space}-milestone")}),
        )
        .await;
        assert_eq!(written["outcome"], "ok", "{written}");
        assert_eq!(written["value"]["initiative"], "Launch", "{written}");
        let statuses = std::iter::repeat_n("Done", done)
            .chain(std::iter::repeat_n("Ready", open))
            .chain(std::iter::once("Canceled"));
        for (index, status) in statuses.enumerate() {
            let created = call(
                "task_create",
                json!({"projectId": space, "title": format!("{milestone} {index}"),
                       "status": status, "milestone": milestone,
                       "clientRequestId": format!("{space}-{index}")}),
            )
            .await;
            assert_eq!(created["outcome"], "ok", "{created}");
        }
    }
    // The milestone's parent is the initiative record.
    let parents = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT thing.parent_id FROM things thing \
         JOIN project_milestones milestone ON milestone.id = thing.id",
    )
    .fetch_all(pool)
    .await
    .expect("parents");
    let launch_id = Uuid::parse_str(&launch).expect("uuid");
    assert_eq!(parents, vec![Some(launch_id), Some(launch_id)]);

    let progress = |listed: &Value| {
        let initiative = listed["value"]["initiatives"]
            .as_array()
            .and_then(|all| all.iter().find(|one| one["title"] == "Launch"))
            .cloned()
            .unwrap_or_else(|| panic!("{listed}"));
        (
            initiative["scope"].as_i64().expect("scope"),
            initiative["completed"].as_i64().expect("completed"),
            initiative["milestones"]
                .as_array()
                .expect("milestones")
                .len(),
            initiative,
        )
    };
    let owner_view = call("initiative_list", json!({})).await;
    let (scope, completed, milestones, initiative) = progress(&owner_view);
    assert_eq!((scope, completed, milestones), (5, 3, 2), "{owner_view}");
    let health = initiative["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .find(|field| field["name"] == "Health")
        .cloned()
        .expect("health");
    assert_eq!(health["value"], "At risk");

    // Rook leaves SIDE: the initiative is still there, SIDE's half isn't.
    sqlx::query(
        "UPDATE project_memberships SET enabled = false \
         WHERE actor_id = $1 AND project_id = $2::uuid",
    )
    .bind(rook.actor)
    .bind(world.project("SIDE"))
    .execute(pool)
    .await
    .expect("leave SIDE");
    let rook_view = rook
        .call(
            app,
            Call::Tool {
                name: "initiative_list",
                arguments: json!({}),
            },
        )
        .await
        .1;
    let (scope, completed, milestones, initiative) = progress(&rook_view);
    assert_eq!((scope, completed, milestones), (2, 1, 1), "{rook_view}");
    assert_eq!(initiative["milestones"][0]["spaceKey"], "WORK");
    assert!(!rook_view.to_string().contains("Docs"), "{rook_view}");

    // A milestone can't join an initiative of another workspace, or one
    // that isn't there.
    let nowhere = call(
        "milestone_write",
        json!({"projectId": "WORK", "name": "Stray", "initiative": "Nope",
               "clientRequestId": "stray"}),
    )
    .await;
    assert_eq!(nowhere["outcome"], "invalid_request", "{nowhere}");

    world.finish().await;
}
