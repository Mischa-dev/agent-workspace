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
            "milestone_create",
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
        "milestone_create",
        json!({"projectId": "WORK", "name": "Stray", "initiative": "Nope",
               "clientRequestId": "stray"}),
    )
    .await;
    assert_eq!(nowhere["outcome"], "invalid_request", "{nowhere}");

    world.finish().await;
}

/// T1.42: a space's milestones keep the order set through the tool, and
/// each reads its roll-up.
#[tokio::test]
async fn milestones_keep_their_order_and_roll_up_their_tasks() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("laptop")
        .build("t1_milestone_order")
        .await
    else {
        return;
    };
    let app = &world.app;
    let laptop = &world.apps["laptop"];
    let call = |name: &'static str, arguments: Value| async move {
        laptop.call(app, Call::Tool { name, arguments }).await.1
    };
    let mut ids = Vec::new();
    for (name, date) in [
        ("Alpha", "2026-12-01"),
        ("Beta", "2026-12-15"),
        ("Gamma", ""),
    ] {
        let made = call(
            "milestone_create",
            json!({"projectId": "WORK", "name": name, "targetDate": date,
                   "clientRequestId": format!("order-{name}")}),
        )
        .await;
        assert_eq!(made["outcome"], "ok", "{made}");
        ids.push(made["value"]["id"].as_str().expect("id").to_owned());
    }
    for (index, (status, estimate, due)) in [
        ("Done", 3.0, "2026-12-10"),
        ("Ready", 2.0, "2026-12-20"),
        ("Canceled", 8.0, "2027-01-30"),
    ]
    .into_iter()
    .enumerate()
    {
        let created = call(
            "task_create",
            json!({"projectId": "WORK", "title": format!("Rolled {index}"), "status": status,
                   "milestone": "Beta", "estimate": estimate, "dueDate": due,
                   "clientRequestId": format!("rolled-{index}")}),
        )
        .await;
        assert_eq!(created["outcome"], "ok", "{created}");
    }
    let moved = call(
        "milestone_reorder",
        json!({"milestoneId": ids[2], "position": 1}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    let listed = call("milestone_list", json!({"projectId": "WORK"})).await;
    let milestones = listed["value"]["milestones"]
        .as_array()
        .expect("milestones");
    let names: Vec<&str> = milestones
        .iter()
        .map(|milestone| milestone["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, ["Gamma", "Alpha", "Beta"], "{listed}");
    let beta = &milestones[2];
    assert_eq!(beta["estimateTotal"], 5.0, "{beta}");
    assert_eq!(beta["estimateDone"], 3.0, "{beta}");
    assert_eq!(beta["latestDueDate"], "2026-12-20", "{beta}");
    assert_eq!(
        (beta["finished"].as_i64(), beta["total"].as_i64()),
        (Some(1), Some(2))
    );
    world.finish().await;
}
