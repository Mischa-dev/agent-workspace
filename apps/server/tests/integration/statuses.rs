//! C2.15: account status. An app acting for a person sets that person's
//! status and an agent sets its own; nobody sets someone else's they don't
//! manage. A status for no workspace shows in every workspace the account
//! is in, a line about one workspace only there, and neither in a workspace
//! the account isn't in. A status past its `clearAt` is gone.

use serde_json::json;
use uuid::Uuid;

use agent_workspace_server::statuses;

use super::support::{app::tool, world::WorldBuilder};

#[tokio::test]
async fn statuses_show_where_they_belong_and_clear_themselves() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .connected_app("Desk")
        .build("c2_15_statuses")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let owner = world.owner.actor;
    let desk = &world.apps["Desk"];
    let rook = &world.agents["rook"];

    // The app sets the owner's status; the agent sets a line about its work
    // here.
    let set = tool(
        app,
        &desk.token,
        &desk.session,
        "status_set",
        json!({"text": "  In a   meeting ", "emoji": "📅"}),
    )
    .await;
    assert_eq!(set["outcome"], "ok", "{set}");
    assert_eq!(set["value"]["accountId"], owner.to_string(), "{set}");
    assert_eq!(set["value"]["text"], "In a meeting", "{set}");
    assert_eq!(set["value"]["workspaceOnly"], false, "{set}");
    let line = tool(
        app,
        &rook.token,
        &rook.session,
        "status_set",
        json!({"text": "Reviewing the release notes", "state": "working", "workspaceOnly": true}),
    )
    .await;
    assert_eq!(line["outcome"], "ok", "{line}");
    assert_eq!(line["value"]["accountId"], rook.actor.to_string(), "{line}");

    // The agent doesn't manage the owner: it can't set or clear theirs.
    for (name, arguments) in [
        (
            "status_set",
            json!({"text": "Gone fishing", "account": owner}),
        ),
        ("status_delete", json!({"account": owner})),
    ] {
        let refused = tool(app, &rook.token, &rook.session, name, arguments).await;
        assert_ne!(refused["outcome"], "ok", "{name}: {refused}");
    }
    let listed = tool(
        app,
        &desk.token,
        &desk.session,
        "status_list",
        json!({"accounts": [owner, rook.actor]}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let shown = listed["value"]["statuses"].as_array().expect("statuses");
    assert_eq!(shown.len(), 2, "{listed}");
    assert!(!listed.to_string().contains("Gone fishing"), "{listed}");

    // Another workspace both are in shows the owner's status, not the
    // agent's line about this one; a workspace the owner isn't in shows
    // nothing of theirs.
    let elsewhere = Uuid::now_v7();
    let stranger = Uuid::now_v7();
    for (id, slug) in [(elsewhere, "c2-15-elsewhere"), (stranger, "c2-15-stranger")] {
        sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Elsewhere', $2)")
            .bind(id)
            .bind(format!("{slug}-{}", &id.simple().to_string()[24..]))
            .execute(pool)
            .await
            .expect("workspace");
    }
    for actor in [owner, rook.actor] {
        sqlx::query(
            "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'member')",
        )
        .bind(elsewhere)
        .bind(actor)
        .execute(pool)
        .await
        .expect("membership");
    }
    let mut connection = pool.acquire().await.expect("connection");
    let there = statuses::shown_in(&mut connection, elsewhere, &[owner, rook.actor])
        .await
        .expect("shown elsewhere");
    assert_eq!(there.len(), 1, "{there:?}");
    assert_eq!(there[&owner][0].text.as_deref(), Some("In a meeting"));
    let strangers = statuses::shown_in(&mut connection, stranger, &[owner, rook.actor])
        .await
        .expect("shown to strangers");
    assert!(strangers.is_empty(), "{strangers:?}");

    // Past its time to clear, it's gone from reads and then from the table.
    sqlx::query(
        "UPDATE account_statuses SET clear_at = now() - interval '1 minute' WHERE account_id = $1",
    )
    .bind(owner)
    .execute(pool)
    .await
    .expect("due");
    let here = statuses::shown_in(&mut connection, world.organization, &[owner])
        .await
        .expect("shown here");
    assert!(here.is_empty(), "{here:?}");
    drop(connection);
    assert!(statuses::clear_due(pool, 500).await.expect("clear due") >= 1);
    let left =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM account_statuses WHERE account_id = $1")
            .bind(owner)
            .fetch_one(pool)
            .await
            .expect("left");
    assert_eq!(left, 0);

    // The agent clears its own line.
    let cleared = tool(
        app,
        &rook.token,
        &rook.session,
        "status_delete",
        json!({"workspaceOnly": true, "workspace": world.organization}),
    )
    .await;
    assert_eq!(cleared["value"]["cleared"], true, "{cleared}");

    world.finish().await;
}
