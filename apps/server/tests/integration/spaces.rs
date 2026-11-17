//! K4: spaces. The new names are views over today's tables (K4.1), and
//! every statement form the code uses works through them; a personal space
//! needs an owner and there is one per account per workspace. Workspace
//! slugs keep old links working for members only (K4.5).

use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;

use super::support::{
    app::send,
    world::{WorldBuilder, setup},
};

#[tokio::test]
async fn views_write_through() {
    let Some(setup) = setup("k41_views").await else {
        return;
    };
    let pool = &setup.pool;
    let workspace = setup.organization_id;
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1",
    )
    .bind(workspace)
    .fetch_one(pool)
    .await
    .expect("owner");

    // INSERT, with renamed columns.
    let space = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO spaces (id, workspace_id, key, name, kind, visibility, created_by_actor_id) \
         VALUES ($1, $2, 'VIEW', 'Through the view', 'team', 'workspace', $3)",
    )
    .bind(space)
    .bind(workspace)
    .bind(owner)
    .execute(pool)
    .await
    .expect("insert through spaces");
    let (key, kind, visibility) = sqlx::query_as::<_, (String, String, String)>(
        "SELECT project_key, kind, visibility FROM projects WHERE id = $1",
    )
    .bind(space)
    .fetch_one(pool)
    .await
    .expect("base row");
    assert_eq!(
        (key.as_str(), kind.as_str(), visibility.as_str()),
        ("VIEW", "team", "workspace")
    );

    // INSERT ... ON CONFLICT DO NOTHING, and ON CONFLICT on renamed columns.
    sqlx::query(
        "INSERT INTO spaces (id, workspace_id, key, name) VALUES ($1, $2, 'VIEW', 'Again') \
         ON CONFLICT DO NOTHING",
    )
    .bind(space)
    .bind(workspace)
    .execute(pool)
    .await
    .expect("on conflict do nothing");
    let member = Uuid::now_v7();
    for name in ["Owner", "Owner again"] {
        sqlx::query(
            "INSERT INTO space_members (id, space_id, actor_id, role, display_name, handle) \
             VALUES ($1, $2, $3, 'owner', $4, 'owner-handle') \
             ON CONFLICT (space_id, actor_id) DO UPDATE SET display_name = EXCLUDED.display_name",
        )
        .bind(member)
        .bind(space)
        .bind(owner)
        .bind(name)
        .execute(pool)
        .await
        .expect("upsert through space_members");
    }
    let membership_name = sqlx::query_scalar::<_, String>(
        "SELECT display_name FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(space)
    .bind(owner)
    .fetch_one(pool)
    .await
    .expect("membership");
    assert!(!membership_name.is_empty());

    // UPDATE ... FROM, and row locks of an alias of the view.
    let renamed = sqlx::query(
        "UPDATE spaces s SET description = 'From the workspace' FROM workspaces w \
         WHERE s.id = $1 AND w.id = s.workspace_id",
    )
    .bind(space)
    .execute(pool)
    .await
    .expect("update from")
    .rows_affected();
    assert_eq!(renamed, 1);
    let mut tx = pool.begin().await.expect("begin");
    let locked = sqlx::query_scalar::<_, Uuid>(
        "SELECT s.id FROM spaces s JOIN workspace_members m ON m.workspace_id = s.workspace_id \
         WHERE s.id = $1 AND m.actor_id = $2 FOR UPDATE OF s",
    )
    .bind(space)
    .bind(owner)
    .fetch_one(&mut *tx)
    .await
    .expect("for update of a view alias");
    assert_eq!(locked, space);
    sqlx::query_scalar::<_, Uuid>(
        "SELECT m.actor_id FROM workspace_members m WHERE m.workspace_id = $1 FOR SHARE OF m",
    )
    .bind(workspace)
    .fetch_all(&mut *tx)
    .await
    .expect("for share of a view alias");
    tx.rollback().await.expect("rollback");

    // DELETE through the view (a membership: a project gets side rows from
    // triggers, so a project is never deleted this way).
    let deleted = sqlx::query("DELETE FROM space_members WHERE space_id = $1 AND actor_id = $2")
        .bind(space)
        .bind(owner)
        .execute(pool)
        .await
        .expect("delete through space_members")
        .rows_affected();
    assert_eq!(deleted, 1);

    // The workspace view reads the base table.
    let slug = sqlx::query_scalar::<_, String>("SELECT slug FROM workspaces WHERE id = $1")
        .bind(workspace)
        .fetch_one(pool)
        .await
        .expect("workspace");
    assert!(!slug.is_empty());
    // The workspace's own space is open to the workspace.
    let closed = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM spaces WHERE kind = 'workspace' AND visibility <> 'workspace'",
    )
    .fetch_one(pool)
    .await
    .expect("count");
    assert_eq!(closed, 0);
}

#[tokio::test]
async fn kinds() {
    let Some(setup) = setup("k41_kinds").await else {
        return;
    };
    let pool = &setup.pool;
    let workspace = setup.organization_id;
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1",
    )
    .bind(workspace)
    .fetch_one(pool)
    .await
    .expect("owner");
    let without_owner = sqlx::query(
        "INSERT INTO spaces (id, workspace_id, key, name, kind) VALUES ($1, $2, 'MINE', 'Mine', 'personal')",
    )
    .bind(Uuid::now_v7())
    .bind(workspace)
    .execute(pool)
    .await;
    assert!(without_owner.is_err(), "a personal space needs its owner");
    sqlx::query(
        "INSERT INTO spaces (id, workspace_id, key, name, kind, owner_actor_id) \
         VALUES ($1, $2, 'MINE', 'Mine', 'personal', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(workspace)
    .bind(owner)
    .execute(pool)
    .await
    .expect("one personal space");
    let second = sqlx::query(
        "INSERT INTO spaces (id, workspace_id, key, name, kind, owner_actor_id) \
         VALUES ($1, $2, 'MINETWO', 'Mine too', 'personal', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(workspace)
    .bind(owner)
    .execute(pool)
    .await;
    assert!(
        second.is_err(),
        "one personal space per account per workspace"
    );
    let owned_project = sqlx::query(
        "INSERT INTO spaces (id, workspace_id, key, name, owner_actor_id) VALUES ($1, $2, 'OWNED', 'Owned', $3)",
    )
    .bind(Uuid::now_v7())
    .bind(workspace)
    .bind(owner)
    .execute(pool)
    .await;
    assert!(owned_project.is_err(), "only personal spaces have an owner");
}

#[tokio::test]
async fn slug_rename_keeps_old_links() {
    let Some(world) = WorldBuilder::default()
        .person("lena", "member")
        .build("k45_slugs")
        .await
    else {
        return;
    };
    let workspace = world.organization;
    let pool = world.pool();
    let get = |browser: &super::support::app::Browser, slug: &str| {
        browser.request(
            "GET",
            &format!("/api/v1/workspaces/by-slug/{slug}"),
            None,
            serde_json::Value::Null,
        )
    };
    let (old_slug, version) =
        sqlx::query_as::<_, (String, i64)>("SELECT slug, version FROM organizations WHERE id = $1")
            .bind(workspace)
            .fetch_one(pool)
            .await
            .expect("workspace");
    let (status, body) = send(&world.app, get(&world.people["lena"].browser, &old_slug)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], workspace.to_string());

    // Reserved words and malformed slugs are refused; a member without
    // settings.manage can't rename.
    for reserved in ["api", "settings", "aw-direct-scope", "x", "Not A Slug"] {
        let (status, body) = send(
            &world.app,
            world.owner.browser.request(
                "PATCH",
                &format!("/api/v1/organizations/{workspace}"),
                None,
                json!({ "slug": reserved, "expectedVersion": version }),
            ),
        )
        .await;
        assert!(status.is_client_error(), "{reserved}: {status} {body}");
    }
    let (status, _) = send(
        &world.app,
        world.people["lena"].browser.request(
            "PATCH",
            &format!("/api/v1/organizations/{workspace}"),
            None,
            json!({ "slug": "lenas-place", "expectedVersion": version }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The owner renames; the old slug now redirects members.
    let new_slug = format!("renamed-{}", &Uuid::now_v7().simple().to_string()[24..]);
    let (status, body) = send(
        &world.app,
        world.owner.browser.request(
            "PATCH",
            &format!("/api/v1/organizations/{workspace}"),
            None,
            json!({ "slug": new_slug, "expectedVersion": version }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["slug"], new_slug);
    assert_eq!(body["version"], version + 1);
    let (status, body) = send(&world.app, get(&world.people["lena"].browser, &old_slug)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["redirect"], new_slug, "{body}");
    let (status, body) = send(&world.app, get(&world.people["lena"].browser, &new_slug)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["slug"], new_slug);

    // Another workspace can take neither the retired slug nor the current
    // one, by any writer.
    let other = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Other', $2)")
        .bind(other)
        .bind(format!(
            "other-{}",
            &Uuid::now_v7().simple().to_string()[24..]
        ))
        .execute(pool)
        .await
        .expect("other workspace");
    for taken in [&old_slug, &new_slug] {
        let refused = sqlx::query("UPDATE organizations SET slug = $2 WHERE id = $1")
            .bind(other)
            .bind(taken)
            .execute(pool)
            .await;
        assert!(refused.is_err(), "{taken} was taken by another workspace");
    }

    // A non-member gets 404 for an existing slug, current or retired, the
    // same as for a slug nobody has.
    let other_slug =
        sqlx::query_scalar::<_, String>("SELECT slug FROM organizations WHERE id = $1")
            .bind(other)
            .fetch_one(pool)
            .await
            .expect("other slug");
    for slug in [other_slug.as_str(), "nobody-has-this-one"] {
        let (status, _) = send(&world.app, get(&world.people["lena"].browser, slug)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{slug}");
    }
    sqlx::query(
        "UPDATE organization_memberships SET enabled = false WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(workspace)
    .bind(world.people["lena"].actor)
    .execute(pool)
    .await
    .expect("disable lena");
    for slug in [&old_slug, &new_slug] {
        let (status, _) = send(&world.app, get(&world.people["lena"].browser, slug)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{slug}");
    }

    // The workspace can take its own retired slug back.
    let (status, body) = send(
        &world.app,
        world.owner.browser.request(
            "PATCH",
            &format!("/api/v1/organizations/{workspace}"),
            None,
            json!({ "slug": old_slug, "expectedVersion": version + 1 }),
        ),
    )
    .await;
    if old_slug.len() <= 40 {
        assert_eq!(status, StatusCode::OK, "{body}");
        let current = sqlx::query_scalar::<_, String>(
            "SELECT slug FROM workspace_slugs WHERE workspace_id = $1 AND current",
        )
        .bind(workspace)
        .fetch_one(pool)
        .await
        .expect("current");
        assert_eq!(current, old_slug);
    }
    world.finish().await;
}

/// K4.2: `space_membership` and `space_readable_by` for each way someone is
/// or isn't in a space, and `conversation_visible_to` agreeing with
/// `space_readable_by` for a channel open to the space in every case.
#[tokio::test]
async fn membership_matrix() {
    let Some(setup) = setup("k42_membership_matrix").await else {
        return;
    };
    let pool = &setup.pool;
    let workspace = setup.organization_id;
    let space = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO projects (id, organization_id, project_key, name) VALUES ($1, $2, 'MTX', 'Matrix')",
    )
    .bind(space)
    .bind(workspace)
    .execute(pool)
    .await
    .expect("a space");
    let person = |enabled_space: Option<bool>, enabled_workspace: bool, active: bool| {
        let pool = pool.clone();
        async move {
            let id = Uuid::now_v7();
            sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', $2)")
                .bind(id)
                .bind(if active { "active" } else { "disabled" })
                .execute(&pool)
                .await
                .expect("actor");
            sqlx::query(
                "INSERT INTO organization_memberships (organization_id, actor_id, role, enabled) \
                 VALUES ($1, $2, 'member', $3)",
            )
            .bind(workspace)
            .bind(id)
            .bind(enabled_workspace)
            .execute(&pool)
            .await
            .expect("workspace membership");
            if let Some(enabled) = enabled_space {
                sqlx::query(
                    "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle, enabled) \
                     VALUES ($1, $2, $3, 'member', 'P', $4, $5)",
                )
                .bind(Uuid::now_v7())
                .bind(space)
                .bind(id)
                .bind(format!("p{}", &id.simple().to_string()[24..]))
                .bind(enabled)
                .execute(&pool)
                .await
                .expect("space membership");
            }
            id
        }
    };
    let explicit = person(Some(true), true, true).await;
    let disabled_member = person(Some(false), true, true).await;
    let workspace_disabled = person(Some(true), false, true).await;
    let inactive = person(Some(true), true, false).await;
    let workspace_only = person(None, true, true).await;
    let ask = |space: Uuid, account: Uuid| {
        let pool = pool.clone();
        async move {
            let membership = sqlx::query_as::<_, (String, bool)>(
                "SELECT role, implicit FROM space_membership($1, $2)",
            )
            .bind(space)
            .bind(account)
            .fetch_optional(&pool)
            .await
            .expect("space_membership");
            let readable = sqlx::query_scalar::<_, bool>("SELECT space_readable_by($1, $2)")
                .bind(space)
                .bind(account)
                .fetch_one(&pool)
                .await
                .expect("space_readable_by");
            // conversation_visible_to writes the space check out in place
            // (for speed, K4.4); a channel open to its space must give the
            // same answer as space_readable_by, so the two never drift.
            let channel = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM conversations WHERE project_id = $1 AND name = 'agreement'",
            )
            .bind(space)
            .fetch_optional(&pool)
            .await
            .expect("the agreement channel");
            let channel = match channel {
                Some(channel) => channel,
                None => sqlx::query_scalar::<_, Uuid>(
                    "INSERT INTO conversations (id, project_id, kind, name, visibility) \
                     VALUES (gen_random_uuid(), $1, 'channel', 'agreement', 'project') RETURNING id",
                )
                .bind(space)
                .fetch_one(&pool)
                .await
                .expect("a channel open to the space"),
            };
            let visible = sqlx::query_scalar::<_, bool>("SELECT conversation_visible_to($1, $2)")
                .bind(channel)
                .bind(account)
                .fetch_one(&pool)
                .await
                .expect("conversation_visible_to");
            assert_eq!(visible, readable, "the channel rule and space_readable_by");
            (membership, readable)
        }
    };
    assert_eq!(
        ask(space, explicit).await,
        (Some(("member".to_owned(), false)), true)
    );
    assert_eq!(ask(space, disabled_member).await, (None, false));
    assert_eq!(ask(space, workspace_disabled).await, (None, false));
    assert_eq!(ask(space, inactive).await, (None, false));
    assert_eq!(ask(space, workspace_only).await, (None, false));

    // The workspace space: every enabled workspace member is in it.
    let mut connection = pool.acquire().await.expect("connection");
    let mut transaction = sqlx::Connection::begin(&mut *connection)
        .await
        .expect("transaction");
    let workspace_space = agent_workspace_server::spaces::workspace::join(
        &mut transaction,
        workspace,
        workspace_only,
    )
    .await
    .expect("the workspace space")
    .expect("a workspace member joins it");
    transaction.commit().await.expect("commit");
    let (membership, readable) = ask(workspace_space, workspace_only).await;
    assert!(readable);
    assert!(membership.is_some());
    assert_eq!(
        ask(workspace_space, workspace_disabled).await,
        (None, false)
    );

    // Another workspace's member reads nothing here.
    let other = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Elsewhere', $2)")
        .bind(other)
        .bind(format!("elsewhere-{}", &other.simple().to_string()[20..]))
        .execute(pool)
        .await
        .expect("another workspace");
    let stranger = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', 'active')")
        .bind(stranger)
        .execute(pool)
        .await
        .expect("someone elsewhere");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'owner')",
    )
    .bind(other)
    .bind(stranger)
    .execute(pool)
    .await
    .expect("their membership");
    assert_eq!(ask(space, stranger).await, (None, false));
    assert_eq!(ask(workspace_space, stranger).await, (None, false));

    // An archived space is read by nobody.
    sqlx::query("UPDATE projects SET archived_at = now(), archived_by_actor_id = $2 WHERE id = $1")
        .bind(space)
        .bind(explicit)
        .execute(pool)
        .await
        .expect("archive");
    assert_eq!(ask(space, explicit).await, (None, false));
    let listed = sqlx::query_scalar::<_, Uuid>("SELECT spaces_readable_by($1, $2)")
        .bind(explicit)
        .bind(workspace)
        .fetch_all(pool)
        .await
        .expect("spaces_readable_by");
    assert!(!listed.contains(&space));
}

/// K4.2: a team space open to the workspace is readable by every workspace
/// member, who isn't a member of it until they join.
#[tokio::test]
async fn open_team_space_is_readable_not_joined() {
    let Some(setup) = setup("k42_open_team_space").await else {
        return;
    };
    let pool = &setup.pool;
    let workspace = setup.organization_id;
    let owner = sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1",
    )
    .bind(workspace)
    .fetch_one(pool)
    .await
    .expect("owner");
    let team = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO spaces (id, workspace_id, key, name, kind, visibility, created_by_actor_id) \
         VALUES ($1, $2, 'OPEN', 'Open team', 'team', 'workspace', $3)",
    )
    .bind(team)
    .bind(workspace)
    .bind(owner)
    .execute(pool)
    .await
    .expect("an open team space");
    let closed = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO spaces (id, workspace_id, key, name, kind, visibility, created_by_actor_id) \
         VALUES ($1, $2, 'SHUT', 'Closed team', 'team', 'members', $3)",
    )
    .bind(closed)
    .bind(workspace)
    .bind(owner)
    .execute(pool)
    .await
    .expect("a closed team space");
    let mut connection = pool.acquire().await.expect("connection");
    let readable = agent_workspace_server::spaces::readable(
        &mut connection,
        owner,
        workspace,
        &[agent_workspace_server::spaces::SpaceKind::Team],
    )
    .await
    .expect("readable");
    assert!(readable.iter().any(|space| space.id == team));
    assert!(!readable.iter().any(|space| space.id == closed));
    assert!(
        agent_workspace_server::spaces::membership(&mut connection, owner, team)
            .await
            .expect("membership")
            .is_none(),
        "readable, not joined"
    );
    assert!(
        agent_workspace_server::spaces::get(&mut connection, owner, team)
            .await
            .expect("get")
            .is_some()
    );
    assert!(
        agent_workspace_server::spaces::get(&mut connection, owner, closed)
            .await
            .expect("get")
            .is_none()
    );
}

/// K4.18: an old address resolves to its workspace address only for someone
/// who may see what it leads to; anyone else gets the same 404 as for an
/// address that leads nowhere, and never learns the workspace's slug.
#[tokio::test]
async fn old_addresses_resolve_only_for_those_who_may_see_them() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .person("nia", "member")
        .dm("lena", "mara")
        .build("k418_resolve")
        .await
    else {
        return;
    };
    let slug = sqlx::query_scalar::<_, String>("SELECT slug FROM organizations WHERE id = $1")
        .bind(world.organization)
        .fetch_one(world.pool())
        .await
        .expect("slug");
    let resolve = |who: &str, path: &str| {
        world.people[who].browser.request(
            "GET",
            &format!("/api/v1/links/resolve?path={}", urlencode(path)),
            None,
            serde_json::Value::Null,
        )
    };
    let space = world.project("WORK");
    let dm = &world.conversations["lena+mara"];

    // A space's page, with its query kept.
    let (status, body) = send(
        &world.app,
        resolve("nia", &format!("/projects/{space}/pages?view=all")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["path"], format!("/w/{slug}/s/{space}/pages?view=all"));
    assert_eq!(body["workspaceId"], world.organization.to_string());

    // A DM: its two people only.
    for who in ["lena", "mara"] {
        let (status, body) = send(&world.app, resolve(who, &format!("/messages/{dm}"))).await;
        assert_eq!(status, StatusCode::OK, "{who}: {body}");
        assert_eq!(body["path"], format!("/w/{slug}/dm/{dm}"));
    }
    let (status, body) = send(&world.app, resolve("nia", &format!("/messages/{dm}"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert!(!body.to_string().contains(&slug), "{body}");

    // Nowhere, and things that aren't addresses, answer the same.
    for path in [
        format!("/projects/{}/pages", Uuid::now_v7()),
        format!("/messages/{}", Uuid::now_v7()),
        "//elsewhere.example/inbox".to_owned(),
        "/account".to_owned(),
    ] {
        let (status, body) = send(&world.app, resolve("nia", &path)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {body}");
    }
    world.finish().await;
}

fn urlencode(path: &str) -> String {
    path.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}
