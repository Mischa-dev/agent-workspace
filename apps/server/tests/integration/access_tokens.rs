//! K1.24: a narrow token holds no more than its account, and only what it
//! was narrowed to: its permissions, its spaces. Revoking it cuts it off.

use agent_workspace_server::{
    access::tokens::{self, ScopedTokenRequest},
    db::scoped_tx,
    error::AppError,
    policy::{Permission, PermissionSet},
};
use serde_json::json;
use uuid::Uuid;

use super::support::{
    app::{initialize, tool},
    world::WorldBuilder,
};

fn request(account: Uuid, workspace: Uuid, home: Uuid) -> ScopedTokenRequest {
    ScopedTokenRequest {
        account_id: account,
        workspace_id: workspace,
        home_space: home,
        permissions: None,
        spaces: Some(vec![home]),
        minutes: 30,
        requested_by: None,
        run: None,
        credential_id: None,
        scopes: None,
    }
}

async fn issue(
    world: &super::support::world::Workspace,
    request: ScopedTokenRequest,
) -> Result<String, AppError> {
    let mut transaction = scoped_tx(world.pool()).await.expect("begin");
    let issued = tokens::issue_scoped(&mut transaction, request).await?;
    transaction.commit().await.expect("commit");
    Ok(issued.token)
}

#[tokio::test]
async fn a_narrow_token_holds_only_what_it_was_given() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .project("OTHER")
        .agent("rook", "member")
        .build("k124_access_tokens")
        .await
    else {
        return;
    };
    let pool = world.pool();
    let rook = &world.agents["rook"];
    let work: Uuid = world.project("WORK").parse().expect("work");
    let other: Uuid = world.project("OTHER").parse().expect("other");
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(other)
    .bind(rook.actor)
    .execute(pool)
    .await
    .expect("rook leaves OTHER");

    // Never more than the account holds: a member doesn't manage members,
    // and rook isn't in OTHER.
    let wider = issue(
        &world,
        ScopedTokenRequest {
            permissions: Some(
                PermissionSet::EMPTY.with(Permission::from_key("members.manage").expect("key")),
            ),
            ..request(rook.actor, world.organization, work)
        },
    )
    .await;
    assert!(
        matches!(
            wider,
            Err(AppError::Refused {
                field: "permissions",
                ..
            })
        ),
        "{wider:?}"
    );
    let elsewhere = issue(
        &world,
        ScopedTokenRequest {
            spaces: Some(vec![work, other]),
            ..request(rook.actor, world.organization, work)
        },
    )
    .await;
    assert!(
        matches!(
            elsewhere,
            Err(AppError::Refused {
                field: "spaces",
                ..
            })
        ),
        "{elsewhere:?}"
    );
    let too_long = issue(
        &world,
        ScopedTokenRequest {
            minutes: 25 * 60,
            ..request(rook.actor, world.organization, work)
        },
    )
    .await;
    assert!(
        matches!(too_long, Err(AppError::Validation(_))),
        "{too_long:?}"
    );

    // A token narrowed to seeing tasks sees them and can't make one.
    let seeing = issue(
        &world,
        ScopedTokenRequest {
            permissions: Some(
                PermissionSet::EMPTY.with(Permission::from_key("tasks.view").expect("key")),
            ),
            ..request(rook.actor, world.organization, work)
        },
    )
    .await
    .expect("a narrower token");
    assert!(seeing.starts_with(tokens::TOKEN_PREFIX));
    let session = initialize(&world.app, &seeing).await.expect("session");
    let listed = tool(
        &world.app,
        &seeing,
        &session,
        "task_list",
        json!({"projectId": "WORK"}),
    )
    .await;
    assert_ne!(listed["outcome"], "forbidden", "{listed}");
    let made = tool(
        &world.app,
        &seeing,
        &session,
        "task_create",
        json!({"projectId": "WORK", "title": "Not from a seeing token", "clientRequestId": "k124-seeing"}),
    )
    .await;
    assert_ne!(made["outcome"], "ok", "{made}");

    // A whole token acts in its space, and stops when revoked.
    let whole = issue(&world, request(rook.actor, world.organization, work))
        .await
        .expect("a whole token");
    let session = initialize(&world.app, &whole).await.expect("session");
    let made = tool(
        &world.app,
        &whole,
        &session,
        "task_create",
        json!({"projectId": "WORK", "title": "From a narrow token", "clientRequestId": "k124-whole"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM access_tokens WHERE token_digest = sha256(convert_to($1, 'UTF8'))",
    )
    .bind(&whole)
    .fetch_one(pool)
    .await
    .expect("token row");
    let mut transaction = scoped_tx(pool).await.expect("begin");
    assert!(tokens::revoke(&mut transaction, id).await.expect("revoke"));
    transaction.commit().await.expect("commit");
    assert!(
        initialize(&world.app, &whole).await.is_none(),
        "a revoked token still works"
    );
    world.finish().await;
}

/// K1.25: what an agent writes with a token someone asked for says who
/// asked and which credential it came through, and a role held to its
/// requester does only what the requester could.
#[tokio::test]
async fn a_token_carries_who_asked_and_holds_to_them() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("vic", "viewer")
        .agent("rook", "member")
        .dm("lena", "rook")
        .build("k125_acting_context")
        .await
    else {
        return;
    };
    let pool = world.pool();
    let rook = &world.agents["rook"];
    let lena = world.people["lena"].actor;
    let vic = world.people["vic"].actor;
    let work: Uuid = world.project("WORK").parse().expect("work");

    // Asked for by Lena: the message rook sends records her and the token.
    // A DM is in no space, so the token isn't narrowed to listed spaces.
    let token = issue(
        &world,
        ScopedTokenRequest {
            requested_by: Some(lena),
            spaces: None,
            ..request(rook.actor, world.organization, work)
        },
    )
    .await
    .expect("token");
    let token_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM access_tokens WHERE token_digest = sha256(convert_to($1, 'UTF8'))",
    )
    .bind(&token)
    .fetch_one(pool)
    .await
    .expect("token row");
    let session = initialize(&world.app, &token).await.expect("session");
    let sent = tool(
        &world.app,
        &token,
        &session,
        "message_send",
        json!({"conversationId": world.conversations["lena+rook"], "text": "done", "clientRequestId": "k125-sent"}),
    )
    .await;
    assert_eq!(sent["outcome"], "ok", "{sent}");
    let recorded = sqlx::query_as::<_, (Option<Uuid>, Option<Uuid>)>(
        "SELECT requested_by_account_id, via_credential_id FROM events \
         WHERE actor_account_id = $1 AND requested_by_account_id IS NOT NULL \
         UNION ALL \
         SELECT requested_by_account_id, via_credential_id FROM event_staging \
         WHERE actor_account_id = $1 AND requested_by_account_id IS NOT NULL \
         LIMIT 1",
    )
    .bind(rook.actor)
    .fetch_optional(pool)
    .await
    .expect("events");
    assert_eq!(recorded, Some((Some(lena), Some(token_id))), "{sent}");

    // Held to its requester: asked for by a viewer, rook can't make tasks,
    // though its own role could.
    sqlx::query(
        "UPDATE workspace_roles SET act_within_requester = true WHERE id = \
         (SELECT role_id FROM project_memberships WHERE project_id = $1 AND actor_id = $2)",
    )
    .bind(work)
    .bind(rook.actor)
    .execute(pool)
    .await
    .expect("rook's role is held to its requester");
    let mut caller = agent_workspace_server::access::Caller::person(
        rook.actor,
        agent_workspace_server::policy::ActorKind::ExternalAgent,
    );
    caller.connection_id = Some(rook.connection);
    let mut connection = scoped_tx(pool).await.expect("begin");
    let create = Permission::from_key("tasks.create").expect("key");
    let own = agent_workspace_server::access::holding(
        &mut connection,
        &caller,
        agent_workspace_server::access::Scope::Space(work),
    )
    .await
    .expect("holding")
    .expect("in the space");
    assert!(own.permissions.contains(create));
    caller.requested_by = Some(vic);
    let for_vic = agent_workspace_server::access::holding(
        &mut connection,
        &caller,
        agent_workspace_server::access::Scope::Space(work),
    )
    .await
    .expect("holding")
    .expect("in the space");
    assert!(!for_vic.permissions.contains(create));
    caller.requested_by = Some(lena);
    let for_lena = agent_workspace_server::access::holding(
        &mut connection,
        &caller,
        agent_workspace_server::access::Scope::Space(work),
    )
    .await
    .expect("holding")
    .expect("in the space");
    assert!(for_lena.permissions.contains(create));
    drop(connection);
    world.finish().await;
}
