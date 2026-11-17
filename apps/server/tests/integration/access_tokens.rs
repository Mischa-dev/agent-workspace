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
    assert_ne!(made["outcome"], "applied", "{made}");

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
    assert_eq!(made["outcome"], "applied", "{made}");
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
