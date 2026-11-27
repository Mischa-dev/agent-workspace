//! AWD-221: a project inside a project. A sub-project is a space with a
//! parent; its people are its own or its parent's, kept in step by the
//! database, so nobody outside it reads it whoever is in the parent, and a
//! person and an agent holding the same role get the same answers.

use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::{send, tool},
    world::{Workspace, WorldBuilder},
};

/// Calls `name` as `who`: a person through the v2 route, an agent through
/// its MCP tool. Both answer `{outcome, value}`.
async fn op(world: &Workspace, who: &str, name: &str, input: Value) -> Value {
    if let Some(agent) = world.agents.get(who) {
        let mut input = input;
        if name.ends_with("_create") {
            input["clientRequestId"] = json!(Uuid::now_v7().to_string());
        }
        return tool(&world.app, &agent.token, &agent.session, name, input).await;
    }
    let browser = if who == "owner" {
        &world.owner.browser
    } else {
        &world.people[who].browser
    };
    let route = format!("/api/v2/w/{}/ops/{name}", world.organization);
    let key = Uuid::now_v7().to_string();
    send(
        &world.app,
        browser.request("POST", &route, Some(&key), input),
    )
    .await
    .1
}

fn id(answer: &Value) -> String {
    assert_eq!(answer["outcome"], "ok", "{answer}");
    answer["value"]["id"].as_str().expect("id").to_owned()
}

async fn inside(
    world: &Workspace,
    who: &str,
    parent: &str,
    key: &str,
    people: Option<&str>,
) -> Value {
    let mut input = json!({"key": key, "name": format!("Project {key}"), "parentId": parent});
    if let Some(people) = people {
        input["people"] = json!(people);
    }
    op(world, who, "space_create", input).await
}

/// `(enabled, role, via_group, via_parent)` of an account's seat in a space.
async fn seat(world: &Workspace, space: &str, account: Uuid) -> Option<(bool, String, bool, bool)> {
    sqlx::query_as::<_, (bool, String, bool, bool)>(
        "SELECT enabled, role, via_group, via_parent FROM project_memberships \
         WHERE project_id = $1::uuid AND actor_id = $2",
    )
    .bind(space)
    .bind(account)
    .fetch_optional(world.pool())
    .await
    .expect("seat")
}

fn holds(seat: &Option<(bool, String, bool, bool)>) -> bool {
    seat.as_ref().is_some_and(|seat| seat.0)
}

#[tokio::test]
async fn a_sub_project_has_its_parents_people_kept_in_step() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .agent("rook", "member")
        .build("awd221_people")
        .await
    else {
        return;
    };
    let work = world.project("WORK").to_owned();
    let lena = world.people["lena"].actor;
    let mara = world.people["mara"].actor;
    let rook = world.agents["rook"].actor;

    let made = inside(&world, "owner", &work, "SITE", None).await;
    let site = id(&made);
    assert_eq!(made["value"]["parentId"], work, "{made}");
    assert_eq!(made["value"]["peopleFromParent"], true, "{made}");
    assert_eq!(made["value"]["role"], "owner", "{made}");

    // Everyone in the parent is in it with their role there, people and
    // agents alike; the maker keeps a seat of their own.
    for account in [lena, mara, rook] {
        assert_eq!(
            seat(&world, &site, account).await,
            Some((true, "member".to_owned(), true, true)),
        );
    }
    assert_eq!(
        seat(&world, &site, world.owner.actor).await,
        Some((true, "owner".to_owned(), false, false)),
    );
    for who in ["lena", "rook"] {
        let seen = op(&world, who, "space_open", json!({"spaceId": site})).await;
        assert_eq!(seen["outcome"], "ok", "{seen}");
        assert_eq!(seen["value"]["role"], "member", "{seen}");
        assert_eq!(seen["value"]["parentId"], work, "{seen}");
    }

    // A role changed in the parent is the role here.
    let changed = op(
        &world,
        "owner",
        "space_member_update",
        json!({"spaceId": work, "account": "@lena", "role": "viewer"}),
    )
    .await;
    assert_eq!(changed["outcome"], "ok", "{changed}");
    assert_eq!(
        seat(&world, &site, lena).await,
        Some((true, "viewer".to_owned(), true, true)),
    );

    // Someone here through the parent is taken out there, not here.
    let refused = op(
        &world,
        "owner",
        "space_member_remove",
        json!({"spaceId": site, "account": "@mara"}),
    )
    .await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    assert!(holds(&seat(&world, &site, mara).await));

    // Out of the parent, out of the sub-project, and its conversations
    // close to them with it.
    let made = op(
        &world,
        "owner",
        "channel_create",
        json!({"projectId": site, "name": "site-talk"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let channel: Uuid = made["value"]["id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .expect("channel id");
    let pool = world.pool();
    let visible = |account: Uuid| async move {
        sqlx::query_scalar::<_, bool>("SELECT conversation_visible_to($1, $2)")
            .bind(channel)
            .bind(account)
            .fetch_one(pool)
            .await
            .expect("visible")
    };
    assert!(visible(mara).await);
    for (who, account) in [("@mara", mara), ("@rook", rook)] {
        let removed = op(
            &world,
            "owner",
            "space_member_remove",
            json!({"spaceId": work, "account": who}),
        )
        .await;
        assert_eq!(removed["outcome"], "ok", "{removed}");
        assert!(!holds(&seat(&world, &site, account).await), "{who}");
    }
    assert!(!visible(mara).await);
    for who in ["mara", "rook"] {
        let gone = op(&world, who, "space_open", json!({"spaceId": site})).await;
        assert_eq!(gone["outcome"], "not_found", "{who}: {gone}");
    }

    // Back in the parent, back here.
    let added = op(
        &world,
        "owner",
        "space_member_add",
        json!({"spaceId": work, "account": "@mara", "role": "member"}),
    )
    .await;
    assert_eq!(added["outcome"], "ok", "{added}");
    assert_eq!(
        seat(&world, &site, mara).await,
        Some((true, "member".to_owned(), true, true)),
    );
    assert!(visible(mara).await);

    // Its own people from here on: everyone keeps their seat as their own,
    // and now they are changed here.
    let own = op(
        &world,
        "owner",
        "space_update",
        json!({"spaceId": site, "people": "own"}),
    )
    .await;
    assert_eq!(own["outcome"], "ok", "{own}");
    assert_eq!(own["value"]["peopleFromParent"], false, "{own}");
    assert_eq!(
        seat(&world, &site, mara).await,
        Some((true, "member".to_owned(), false, false)),
    );
    let removed = op(
        &world,
        "owner",
        "space_member_remove",
        json!({"spaceId": site, "account": "@mara"}),
    )
    .await;
    assert_eq!(removed["outcome"], "ok", "{removed}");
    assert!(!holds(&seat(&world, &site, mara).await));
    assert!(!visible(mara).await);
    // The parent's people again: mara is back, through the parent.
    let again = op(
        &world,
        "owner",
        "space_update",
        json!({"spaceId": site, "people": "parent"}),
    )
    .await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(
        seat(&world, &site, mara).await,
        Some((true, "member".to_owned(), true, true)),
    );
    world.finish().await;
}

#[tokio::test]
async fn a_sub_project_with_its_own_people_is_closed_to_the_parents() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("rook", "member")
        .build("awd221_own")
        .await
    else {
        return;
    };
    let work = world.project("WORK").to_owned();
    let made = inside(&world, "owner", &work, "QUIET", Some("own")).await;
    let quiet = id(&made);
    assert_eq!(made["value"]["peopleFromParent"], false, "{made}");

    // In the parent, not in it: it isn't found, listed or in the tree.
    for who in ["lena", "rook"] {
        let hidden = op(&world, who, "space_open", json!({"spaceId": quiet})).await;
        assert_eq!(hidden["outcome"], "not_found", "{who}: {hidden}");
        let listed = op(&world, who, "space_list", json!({})).await;
        assert!(
            !listed["value"]["spaces"]
                .as_array()
                .expect("spaces")
                .iter()
                .any(|space| space["id"] == quiet.as_str()),
            "{who}: {listed}"
        );
        let tree = op(&world, who, "space_tree_get", json!({"spaceId": work})).await;
        assert_eq!(tree["outcome"], "ok", "{tree}");
        assert_eq!(tree["value"]["inside"], json!([]), "{who}: {tree}");
    }
    // Someone added by hand is in it and sees where it sits.
    let added = op(
        &world,
        "owner",
        "space_member_add",
        json!({"spaceId": quiet, "account": "@lena", "role": "member"}),
    )
    .await;
    assert_eq!(added["outcome"], "ok", "{added}");
    assert_eq!(added["value"]["viaParent"], false, "{added}");
    let tree = op(&world, "lena", "space_tree_get", json!({"spaceId": quiet})).await;
    assert_eq!(tree["outcome"], "ok", "{tree}");
    assert_eq!(tree["value"]["path"][0]["id"], work, "{tree}");

    // Without "Manage projects" in the parent nothing goes inside it, made
    // or moved, by a person or an agent.
    for who in ["lena", "rook"] {
        let refused = inside(&world, who, &work, "NOPE", None).await;
        assert_ne!(refused["outcome"], "ok", "{who}: {refused}");
    }
    let unknown = inside(&world, "owner", &Uuid::now_v7().to_string(), "LOST", None).await;
    assert_eq!(unknown["outcome"], "not_found", "{unknown}");
    world.finish().await;
}

#[tokio::test]
async fn archiving_a_parent_needs_manage_in_what_is_inside_it() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("mara", "admin")
        .build("awd221_archive_closed")
        .await
    else {
        return;
    };
    let work = world.project("WORK").to_owned();
    let quiet = id(&inside(&world, "owner", &work, "QUIET", Some("own")).await);
    let open = id(&inside(&world, "owner", &work, "OPEN", None).await);
    // Mara manages the parent and isn't in the closed project inside it, so
    // archiving the parent, which would take that one too, isn't hers.
    let refused = op(&world, "mara", "space_archive", json!({"spaceId": work})).await;
    assert_eq!(refused["outcome"], "conflict", "{refused}");
    let archived = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM spaces WHERE id = ANY($1::uuid[]) AND archived_at IS NOT NULL",
    )
    .bind(vec![work.clone(), quiet.clone()])
    .fetch_one(world.pool())
    .await
    .expect("archived");
    assert_eq!(archived, 0);
    // Whoever manages both can.
    let done = op(&world, "owner", "space_archive", json!({"spaceId": work})).await;
    assert_eq!(done["outcome"], "ok", "{done}");
    // An archived tree keeps its people: a later sync of the seats that come
    // from the archived parent leaves them in place.
    let seats = "SELECT count(*) FROM project_memberships \
        WHERE project_id = $1::uuid AND via_parent AND enabled";
    let before = sqlx::query_scalar::<_, i64>(seats)
        .bind(&open)
        .fetch_one(world.pool())
        .await
        .expect("seats");
    assert!(before > 0);
    sqlx::query(
        "SELECT space_group_seat_sync(project_id, actor_id) FROM project_memberships \
         WHERE project_id = $1::uuid AND via_parent",
    )
    .bind(&open)
    .execute(world.pool())
    .await
    .expect("sync");
    let after = sqlx::query_scalar::<_, i64>(seats)
        .bind(&open)
        .fetch_one(world.pool())
        .await
        .expect("seats");
    assert_eq!(after, before);
    // Mara brings back what is hers: the parent and the open project. The
    // closed one stays archived until someone who manages it restores it.
    let back = op(&world, "mara", "space_restore", json!({"spaceId": work})).await;
    assert_eq!(back["outcome"], "ok", "{back}");
    let archived = "SELECT archived_at IS NOT NULL FROM spaces WHERE id = $1::uuid";
    for (space, expected) in [(&work, false), (&open, false), (&quiet, true)] {
        let is = sqlx::query_scalar::<_, bool>(archived)
            .bind(space)
            .fetch_one(world.pool())
            .await
            .expect("archived");
        assert_eq!(is, expected);
    }
    let mine = op(&world, "mara", "space_restore", json!({"spaceId": quiet})).await;
    assert_ne!(mine["outcome"], "ok", "{mine}");
    let back = op(&world, "owner", "space_restore", json!({"spaceId": quiet})).await;
    assert_eq!(back["outcome"], "ok", "{back}");
    world.finish().await;
}

#[tokio::test]
async fn projects_nest_three_deep_and_never_in_a_loop() {
    let Some(world) = WorldBuilder::default()
        .project("TOP")
        .project("SIDE")
        .build("awd221_tree")
        .await
    else {
        return;
    };
    let top = world.project("TOP").to_owned();
    let side = world.project("SIDE").to_owned();
    let mid = id(&inside(&world, "owner", &top, "MID", None).await);
    let leaf = id(&inside(&world, "owner", &mid, "LEAF", None).await);

    // A fourth level is refused, made or moved, in words.
    let deep = inside(&world, "owner", &leaf, "DEEP", None).await;
    assert_eq!(deep["outcome"], "invalid_request", "{deep}");
    let moved = op(
        &world,
        "owner",
        "space_update",
        json!({"spaceId": top, "parentId": side}),
    )
    .await;
    assert_eq!(moved["outcome"], "invalid_request", "{moved}");
    // Nothing goes inside itself or what is under it.
    for target in [&top, &mid, &leaf] {
        let looped = op(
            &world,
            "owner",
            "space_update",
            json!({"spaceId": top, "parentId": target}),
        )
        .await;
        assert_eq!(looped["outcome"], "invalid_request", "{looped}");
    }
    // Nor inside the workspace's own space, and not both ways at once.
    let workspace_space = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM spaces WHERE workspace_id = $1 AND kind = 'workspace'",
    )
    .bind(world.organization)
    .fetch_one(world.pool())
    .await
    .expect("workspace space");
    let refused = inside(&world, "owner", &workspace_space.to_string(), "HOME", None).await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    let both = op(
        &world,
        "owner",
        "space_update",
        json!({"spaceId": leaf, "parentId": side, "clearParent": true}),
    )
    .await;
    assert_eq!(both["outcome"], "invalid_request", "{both}");
    // The database refuses the same, whoever writes.
    let direct = sqlx::query("UPDATE projects SET parent_space_id = $2::uuid WHERE id = $1::uuid")
        .bind(&top)
        .bind(&leaf)
        .execute(world.pool())
        .await;
    assert!(direct.is_err(), "a loop written past the operations");

    // What is under a project moves with it; at the top it keeps its people.
    let moved = op(
        &world,
        "owner",
        "space_update",
        json!({"spaceId": mid, "parentId": side}),
    )
    .await;
    assert_eq!(moved["outcome"], "ok", "{moved}");
    assert_eq!(moved["value"]["parentId"], side, "{moved}");
    let tree = op(&world, "owner", "space_tree_get", json!({"spaceId": side})).await;
    let rows = tree["value"]["inside"].as_array().expect("inside").clone();
    assert_eq!(rows.len(), 2, "{tree}");
    assert_eq!(
        (&rows[0]["id"], &rows[0]["depth"]),
        (&json!(mid), &json!(1)),
        "{tree}"
    );
    assert_eq!(
        (&rows[1]["id"], &rows[1]["depth"]),
        (&json!(leaf), &json!(2)),
        "{tree}"
    );
    assert_eq!(rows[1]["parentId"], mid, "{tree}");
    let leaf_tree = op(&world, "owner", "space_tree_get", json!({"spaceId": leaf})).await;
    let path: Vec<&Value> = leaf_tree["value"]["path"]
        .as_array()
        .expect("path")
        .iter()
        .map(|row| &row["id"])
        .collect();
    assert_eq!(path, vec![&json!(side), &json!(mid)], "{leaf_tree}");
    let top_level = op(
        &world,
        "owner",
        "space_update",
        json!({"spaceId": mid, "clearParent": true}),
    )
    .await;
    assert_eq!(top_level["outcome"], "ok", "{top_level}");
    assert!(top_level["value"]["parentId"].is_null(), "{top_level}");
    assert_eq!(top_level["value"]["peopleFromParent"], false, "{top_level}");
    world.finish().await;
}

#[tokio::test]
async fn archiving_a_project_takes_what_is_inside_it_and_restoring_brings_it_back() {
    let Some(world) = WorldBuilder::default()
        .project("TOP")
        .build("awd221_archive")
        .await
    else {
        return;
    };
    let top = world.project("TOP").to_owned();
    let mid = id(&inside(&world, "owner", &top, "MID", None).await);
    let leaf = id(&inside(&world, "owner", &mid, "LEAF", None).await);
    let old = id(&inside(&world, "owner", &top, "OLD", None).await);
    let pool = world.pool();
    let archived = |space: String| async move {
        sqlx::query_scalar::<_, bool>(
            "SELECT archived_at IS NOT NULL FROM spaces WHERE id = $1::uuid",
        )
        .bind(space)
        .fetch_one(pool)
        .await
        .expect("archived")
    };

    // One archived by itself earlier stays archived when the rest returns.
    let first = op(&world, "owner", "space_archive", json!({"spaceId": old})).await;
    assert_eq!(first["outcome"], "ok", "{first}");
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let all = op(&world, "owner", "space_archive", json!({"spaceId": top})).await;
    assert_eq!(all["outcome"], "ok", "{all}");
    for space in [&top, &mid, &leaf, &old] {
        assert!(archived(space.clone()).await, "{space}");
    }
    // One inside an archived project waits for that one.
    let early = op(&world, "owner", "space_restore", json!({"spaceId": mid})).await;
    assert_eq!(early["outcome"], "invalid_request", "{early}");
    let back = op(&world, "owner", "space_restore", json!({"spaceId": top})).await;
    assert_eq!(back["outcome"], "ok", "{back}");
    for space in [&top, &mid, &leaf] {
        assert!(!archived(space.clone()).await, "{space}");
    }
    assert!(archived(old.clone()).await);
    let late = op(&world, "owner", "space_restore", json!({"spaceId": old})).await;
    assert_eq!(late["outcome"], "ok", "{late}");
    // Nothing goes inside an archived project.
    let again = op(&world, "owner", "space_archive", json!({"spaceId": old})).await;
    assert_eq!(again["outcome"], "ok", "{again}");
    let refused = inside(&world, "owner", &old, "LATE", None).await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");
    world.finish().await;
}

#[tokio::test]
async fn a_parent_shows_the_tasks_of_what_is_inside_it() {
    let Some(world) = WorldBuilder::default()
        .project("TOP")
        .agent("rook", "member")
        .build("awd221_rollup")
        .await
    else {
        return;
    };
    let top = world.project("TOP").to_owned();
    let open = id(&inside(&world, "owner", &top, "OPEN", None).await);
    let quiet = id(&inside(&world, "owner", &top, "QUIET", Some("own")).await);
    // The agent is in OPEN through the parent, and not in QUIET.
    for (space, title) in [(&top, "Top task"), (&open, "Open task")] {
        let made = op(
            &world,
            "rook",
            "task_create",
            json!({"projectId": space, "title": title}),
        )
        .await;
        assert_eq!(made["outcome"], "ok", "{made}");
    }
    let hidden = op(
        &world,
        "owner",
        "task_create",
        json!({"projectId": quiet, "title": "Quiet task"}),
    )
    .await;
    assert_eq!(hidden["outcome"], "ok", "{hidden}");

    // The agent's roll-up counts and lists what it may see, and nothing of
    // the sub-project it isn't in.
    let tree = op(&world, "rook", "space_tree_get", json!({"spaceId": top})).await;
    assert_eq!(tree["outcome"], "ok", "{tree}");
    let rows = tree["value"]["inside"].as_array().expect("inside");
    assert_eq!(rows.len(), 1, "{tree}");
    assert_eq!(rows[0]["id"], open, "{tree}");
    assert_eq!(rows[0]["tasks"], json!({"open": 1, "done": 0}), "{tree}");
    assert_eq!(
        tree["value"]["tasks"],
        json!({"open": 1, "done": 0}),
        "{tree}"
    );
    assert_eq!(
        tree["value"]["tasksWithInside"],
        json!({"open": 2, "done": 0}),
        "{tree}"
    );
    let alone = op(&world, "rook", "task_list", json!({"projectId": top})).await;
    assert_eq!(alone["value"]["total"], 1, "{alone}");
    let with = op(
        &world,
        "rook",
        "task_list",
        json!({"projectId": top, "includeSubProjects": true}),
    )
    .await;
    assert_eq!(with["outcome"], "ok", "{with}");
    assert_eq!(with["value"]["total"], 2, "{with}");
    let titles = with.to_string();
    assert!(
        titles.contains("Open task") && !titles.contains("Quiet task"),
        "{with}"
    );

    // The owner, in all three, sees all three.
    let tree = op(&world, "owner", "space_tree_get", json!({"spaceId": top})).await;
    assert_eq!(
        tree["value"]["inside"].as_array().map(Vec::len),
        Some(2),
        "{tree}"
    );
    assert_eq!(
        tree["value"]["tasksWithInside"],
        json!({"open": 3, "done": 0}),
        "{tree}"
    );
    world.finish().await;
}
