//! K4.7: spaces are made, changed, archived and brought back through the
//! space operations, and a person and an agent holding the same role get
//! the same answers.

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

fn listed(answer: &Value, space: &str) -> Option<Value> {
    answer["value"]["spaces"]
        .as_array()
        .expect("spaces")
        .iter()
        .find(|row| row["id"] == space)
        .cloned()
}

#[tokio::test]
async fn team_space_lifecycle() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .build("k47_team_space")
        .await
    else {
        return;
    };

    // A team space, open to the workspace, with an icon.
    let made = op(
        &world,
        "owner",
        "space_create",
        json!({"kind": "team", "key": "ops", "name": "Operations", "icon": "🛠️", "visibility": "workspace"}),
    )
    .await;
    let team = id(&made);
    assert_eq!(made["value"]["kind"], "team", "{made}");
    assert_eq!(made["value"]["key"], "OPS", "{made}");
    assert_eq!(made["value"]["icon"], "🛠️", "{made}");
    assert_eq!(made["value"]["role"], "owner", "{made}");
    let event = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM event_staging WHERE type = 'space.created' AND space_id = $1::uuid \
         UNION ALL SELECT count(*) FROM events WHERE type = 'space.created' AND space_id = $1::uuid",
    )
    .bind(&team)
    .fetch_all(world.pool())
    .await
    .map(|counts| counts.iter().sum::<i64>());
    assert_eq!(event.ok(), Some(1), "space.created is written once");

    // Open to the workspace: lena reads it without joining, and can't
    // change it.
    let seen = op(&world, "lena", "space_open", json!({"spaceId": team})).await;
    assert_eq!(seen["outcome"], "ok", "{seen}");
    assert!(seen["value"]["role"].is_null(), "{seen}");
    let refused = op(
        &world,
        "lena",
        "space_update",
        json!({"spaceId": team, "name": "Taken"}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");

    // Renamed, against the version read; a stale version is a conflict.
    let renamed = op(
        &world,
        "owner",
        "space_update",
        json!({"spaceId": team, "name": "Platform", "icon": "", "expectedVersion": 1}),
    )
    .await;
    assert_eq!(renamed["outcome"], "ok", "{renamed}");
    assert_eq!(renamed["value"]["name"], "Platform");
    assert!(renamed["value"]["icon"].is_null(), "{renamed}");
    let stale = op(
        &world,
        "owner",
        "space_update",
        json!({"spaceId": team, "name": "Again", "expectedVersion": 1}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");

    // Archived: out of everyone's list and reach; its members still find it
    // with includeArchived, and it comes back as it was.
    let archived = op(&world, "owner", "space_archive", json!({"spaceId": team})).await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    assert_eq!(archived["value"]["archived"], true);
    let lenas = op(&world, "lena", "space_list", json!({})).await;
    assert!(listed(&lenas, &team).is_none(), "{lenas}");
    let lenas = op(
        &world,
        "lena",
        "space_list",
        json!({"includeArchived": true}),
    )
    .await;
    assert!(listed(&lenas, &team).is_none(), "{lenas}");
    let refused = op(&world, "lena", "space_restore", json!({"spaceId": team})).await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");
    let owners = op(
        &world,
        "owner",
        "space_list",
        json!({"includeArchived": true}),
    )
    .await;
    assert_eq!(listed(&owners, &team).expect("listed")["archived"], true);
    let restored = op(&world, "owner", "space_restore", json!({"spaceId": team})).await;
    assert_eq!(restored["outcome"], "ok", "{restored}");
    assert_eq!(restored["value"]["archived"], false);
    assert_eq!(restored["value"]["name"], "Platform");
    let lenas = op(&world, "lena", "space_list", json!({"kinds": ["team"]})).await;
    assert!(listed(&lenas, &team).is_some(), "{lenas}");
    let events = sqlx::query_scalar::<_, String>(
        "SELECT type FROM event_staging WHERE space_id = $1::uuid AND type LIKE 'space.%' \
         UNION ALL SELECT type FROM events WHERE space_id = $1::uuid AND type LIKE 'space.%'",
    )
    .bind(&team)
    .fetch_all(world.pool())
    .await
    .unwrap_or_default();
    for kind in [
        "space.created",
        "space.updated",
        "space.archived",
        "space.restored",
    ] {
        assert!(
            events.iter().any(|event| event == kind),
            "{kind} in {events:?}"
        );
    }

    // The workspace's own space and personal spaces aren't made here.
    for kind in ["workspace", "personal"] {
        let refused = op(
            &world,
            "owner",
            "space_create",
            json!({"kind": kind, "key": "NOPE", "name": "Nope"}),
        )
        .await;
        assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    }
    world.finish().await;
}

#[tokio::test]
async fn a_person_and_an_agent_with_the_same_role_get_the_same_answers() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "admin")
        .agent("rook", "admin")
        .build("k47_same_role")
        .await
    else {
        return;
    };
    let pool = world.pool();
    let lena = world.people["lena"].actor;
    let rook = world.agents["rook"].actor;
    for actor in [lena, rook] {
        sqlx::query(
            "UPDATE organization_memberships SET role = 'admin', role_explicit = true \
             WHERE organization_id = $1 AND actor_id = $2",
        )
        .bind(world.organization)
        .bind(actor)
        .execute(pool)
        .await
        .expect("workspace admin");
    }
    let work = world.project("WORK").to_owned();
    let mut answers = Vec::new();
    for (who, key) in [("lena", "LENA"), ("rook", "ROOK")] {
        let made = op(
            &world,
            who,
            "space_create",
            json!({"kind": "team", "key": key, "name": "Team"}),
        )
        .await;
        let mut outcomes = vec![made["outcome"].clone(), made["value"]["role"].clone()];
        let mine = made["value"]["id"].as_str().unwrap_or_default().to_owned();
        for (name, input) in [
            ("space_open", json!({"spaceId": work})),
            ("space_update", json!({"spaceId": mine, "name": "Renamed"})),
            ("space_archive", json!({"spaceId": mine})),
            ("space_restore", json!({"spaceId": mine})),
            ("space_update", json!({"spaceId": work, "icon": "📦"})),
            (
                "space_create",
                json!({"kind": "workspace", "key": "NOPE", "name": "Nope"}),
            ),
        ] {
            let answer = op(&world, who, name, input).await;
            outcomes.push(answer["outcome"].clone());
        }
        let list = op(
            &world,
            who,
            "space_list",
            json!({"kinds": ["project", "team"]}),
        )
        .await;
        let mut names = list["value"]["spaces"]
            .as_array()
            .expect("spaces")
            .iter()
            .map(|space| space["key"].as_str().unwrap_or_default().to_owned())
            .collect::<Vec<_>>();
        names.sort();
        outcomes.push(json!(names.len()));
        answers.push(outcomes);
    }
    assert_eq!(answers[0], answers[1], "a person and an agent differ");
    assert_eq!(answers[0][0], "ok", "{:?}", answers[0]);
    world.finish().await;
}

#[tokio::test]
async fn join_open_space() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("rook", "member")
        .build("k48_join")
        .await
    else {
        return;
    };
    let open = id(&op(
        &world,
        "owner",
        "space_create",
        json!({"kind": "team", "key": "OPEN", "name": "Open", "visibility": "workspace"}),
    )
    .await);
    let closed = id(&op(
        &world,
        "owner",
        "space_create",
        json!({"kind": "team", "key": "SHUT", "name": "Closed"}),
    )
    .await);

    // A person and an agent join the open space alike; joining again is
    // harmless; the event names who joined.
    for who in ["lena", "rook"] {
        let joined = op(&world, who, "space_join", json!({"spaceId": open})).await;
        assert_eq!(joined["outcome"], "ok", "{joined}");
        assert_eq!(joined["value"]["role"], "member", "{joined}");
        assert_eq!(joined["value"]["active"], true, "{joined}");
        let again = op(&world, who, "space_join", json!({"spaceId": open})).await;
        assert_eq!(again["outcome"], "ok", "{again}");
    }
    let lena = world.people["lena"].actor;
    let events = sqlx::query_scalar::<_, i64>(
        "SELECT (SELECT count(*) FROM event_staging WHERE type = 'space.member.changed' \
                 AND space_id = $1::uuid AND payload->>'actorId' = $2::text) \
              + (SELECT count(*) FROM events WHERE type = 'space.member.changed' \
                 AND space_id = $1::uuid AND payload->>'actorId' = $2::text)",
    )
    .bind(&open)
    .bind(lena.to_string())
    .fetch_one(world.pool())
    .await
    .expect("events");
    assert_eq!(events, 1, "one event names lena");

    // A closed space can't be joined: it isn't even found.
    for who in ["lena", "rook"] {
        let refused = op(&world, who, "space_join", json!({"spaceId": closed})).await;
        assert_eq!(refused["outcome"], "not_found", "{refused}");
    }

    // Leaving: a member goes, and can't join again on their own once
    // removed by someone else.
    let left = op(&world, "lena", "space_leave", json!({"spaceId": open})).await;
    assert_eq!(left["outcome"], "ok", "{left}");
    assert_eq!(left["value"]["active"], false, "{left}");
    let removed = op(
        &world,
        "owner",
        "space_member_remove",
        json!({"spaceId": open, "account": "@rook"}),
    )
    .await;
    assert_eq!(removed["outcome"], "ok", "{removed}");
    let back = op(&world, "rook", "space_join", json!({"spaceId": open})).await;
    assert_ne!(back["outcome"], "ok", "{back}");
    // The last owner hands the space over before leaving.
    let last = op(&world, "owner", "space_leave", json!({"spaceId": open})).await;
    assert_eq!(last["outcome"], "conflict", "{last}");
    // The workspace's own space isn't joined, left or managed by hand.
    let workspace_space = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM spaces WHERE workspace_id = $1 AND kind = 'workspace'",
    )
    .bind(world.organization)
    .fetch_one(world.pool())
    .await
    .expect("workspace space");
    let refused = op(
        &world,
        "owner",
        "space_member_remove",
        json!({"spaceId": workspace_space, "account": "@lena"}),
    )
    .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    world.finish().await;
}

async fn get(world: &Workspace, who: &str, uri: &str) -> (axum::http::StatusCode, Value) {
    let browser = if who == "owner" {
        &world.owner.browser
    } else {
        &world.people[who].browser
    };
    send(&world.app, browser.request("GET", uri, None, Value::Null)).await
}

/// K4.9: a personal space is its owner's alone. The owner reads and writes
/// there; another person, the workspace's owner and another agent get
/// not-found through the web, MCP and search.
#[tokio::test]
async fn personal_space_is_private() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "admin")
        .agent("rook", "member")
        .agent("scout", "admin")
        .build("k49_personal_space")
        .await
    else {
        return;
    };
    let rook = &world.agents["rook"];
    let scout = &world.agents["scout"];

    // An agent writes a page to "me": its personal space, made then.
    let written = tool(
        &world.app,
        &rook.token,
        &rook.session,
        "page_write",
        json!({"projectId": "me", "title": "Scratch", "body": "The wombatword plan."}),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let rook_page = written["value"]["id"].as_str().expect("page").to_owned();
    let mine = op(&world, "rook", "space_personal_open", json!({})).await;
    let rook_space = id(&mine);
    assert_eq!(mine["value"]["kind"], "personal", "{mine}");
    assert_eq!(mine["value"]["role"], "owner", "{mine}");
    assert_eq!(
        written["value"]["projectId"].as_str(),
        Some(rook_space.as_str()),
        "{written}"
    );
    // It starts with its Notes page.
    let notes = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM project_pages WHERE project_id = $1::uuid AND title = 'Notes'",
    )
    .bind(&rook_space)
    .fetch_one(world.pool())
    .await
    .expect("notes");
    assert_eq!(notes, 1);

    // A person's own: made on first open, the same space after.
    let first = op(&world, "owner", "space_personal_open", json!({})).await;
    let owner_space = id(&first);
    let again = op(&world, "owner", "space_personal_open", json!({})).await;
    assert_eq!(id(&again), owner_space);
    let (status, page) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{owner_space}/pages"),
            Some("k49-owner-page"),
            json!({"title": "Draft", "body": "A numbatword draft."}),
        ),
    )
    .await;
    assert!(status.is_success(), "{page}");
    let owner_page = page["id"].as_str().expect("page").to_owned();

    // The owners read their own.
    let read = tool(
        &world.app,
        &rook.token,
        &rook.session,
        "page_get",
        json!({"pageId": rook_page}),
    )
    .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    let (status, _) = get(&world, "owner", &format!("/api/v1/pages/{owner_page}")).await;
    assert!(status.is_success());
    let found = tool(
        &world.app,
        &rook.token,
        &rook.session,
        "search",
        json!({"query": "wombatword"}),
    )
    .await;
    assert!(found.to_string().contains(&rook_page), "{found}");
    let (_, found) = get(&world, "owner", "/api/v1/search?q=numbatword").await;
    assert!(found.to_string().contains(&owner_page), "{found}");

    // Nobody else does: not the workspace's owner, not an admin, not
    // another agent.
    for who in ["owner", "lena"] {
        let (status, body) = get(&world, who, &format!("/api/v1/pages/{rook_page}")).await;
        assert_eq!(status, axum::http::StatusCode::NOT_FOUND, "{who}: {body}");
        let (_, found) = get(&world, who, "/api/v1/search?q=wombatword").await;
        assert!(!found.to_string().contains(&rook_page), "{who}: {found}");
        let seen = op(&world, who, "space_open", json!({"spaceId": rook_space})).await;
        assert_eq!(seen["outcome"], "not_found", "{who}: {seen}");
    }
    let (status, body) = get(&world, "lena", &format!("/api/v1/pages/{owner_page}")).await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND, "{body}");
    let (_, found) = get(&world, "lena", "/api/v1/search?q=numbatword").await;
    assert!(!found.to_string().contains(&owner_page), "{found}");
    for page in [&rook_page, &owner_page] {
        let read = tool(
            &world.app,
            &scout.token,
            &scout.session,
            "page_get",
            json!({"pageId": page}),
        )
        .await;
        assert_ne!(read["outcome"], "ok", "{read}");
    }
    for word in ["wombatword", "numbatword"] {
        let found = tool(
            &world.app,
            &scout.token,
            &scout.session,
            "search",
            json!({"query": word}),
        )
        .await;
        assert!(
            !found.to_string().contains(&rook_page) && !found.to_string().contains(&owner_page),
            "{found}"
        );
    }
    let seen = op(
        &world,
        "scout",
        "space_open",
        json!({"spaceId": owner_space}),
    )
    .await;
    assert_eq!(seen["outcome"], "not_found", "{seen}");
    // Nobody is added to one.
    let added = op(
        &world,
        "owner",
        "space_member_add",
        json!({"spaceId": owner_space, "account": "@lena", "role": "viewer"}),
    )
    .await;
    assert_eq!(added["outcome"], "invalid_request", "{added}");
    world.finish().await;
}

/// K4.9: two first opens at once make one personal space.
#[tokio::test]
async fn concurrent_first_opens_make_one_personal_space() {
    let Some(world) = WorldBuilder::default()
        .person("lena", "member")
        .build("k49_personal_race")
        .await
    else {
        return;
    };
    let (a, b) = tokio::join!(
        op(&world, "lena", "space_personal_open", json!({})),
        op(&world, "lena", "space_personal_open", json!({})),
    );
    assert_eq!(id(&a), id(&b));
    let spaces = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM spaces WHERE owner_actor_id = $1 AND kind = 'personal'",
    )
    .bind(world.people["lena"].actor)
    .fetch_one(world.pool())
    .await
    .expect("count");
    assert_eq!(spaces, 1);
    world.finish().await;
}
