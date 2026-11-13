//! The workspace space (AWD-96): a hidden project, one per workspace, that
//! holds the pages belonging to the workspace instead of to one project.
//!
//! It reuses a project row the way the Messages space does, so pages there
//! keep every page path (revisions, live editing, comments, search, links,
//! "Referenced in") and the same membership checks. Every enabled member of
//! the workspace, people and agents, is a member with their workspace role:
//! everyone when the space is made, anyone who joins or comes back as their
//! workspace membership commits, disabled with their other memberships when
//! they leave, and kept in step by the workspace membership triggers. It's never
//! listed as a project, and only Pages is needed there, not Projects.

use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::error::AppError;

/// The space's key: never shown, but projects need one, unique in the
/// workspace. A project that already took it moves the space to a
/// suffixed key.
const SPACE_KEY: &str = "WSPACE";
pub const SPACE_NAME: &str = "Workspace";

/// The workspace's space, if it has been made.
pub async fn find(
    connection: &mut PgConnection,
    organization_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM projects WHERE organization_id = $1 AND kind = 'workspace'",
    )
    .bind(organization_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// The workspace's space, made if it doesn't exist yet.
async fn ensure(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
) -> Result<Uuid, AppError> {
    if let Some(project_id) = find(transaction, organization_id).await? {
        return Ok(project_id);
    }
    let project_id = Uuid::now_v7();
    for key in [
        SPACE_KEY.to_owned(),
        format!("{SPACE_KEY}{}", &project_id.simple().to_string()[26..]).to_uppercase(),
    ] {
        sqlx::query(
            r#"
            INSERT INTO projects (id, organization_id, project_key, name, kind)
            SELECT $1, $2, $3, $4, 'workspace'
            WHERE NOT EXISTS (
                SELECT 1 FROM projects
                WHERE organization_id = $2 AND lower(project_key) = lower($3)
            )
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(project_id)
        .bind(organization_id)
        .bind(&key)
        .bind(SPACE_NAME)
        .execute(&mut **transaction)
        .await?;
        if let Some(found) = find(transaction, organization_id).await? {
            sqlx::query(
                "INSERT INTO project_counters (project_id, next_work_item_number) VALUES ($1, 1) ON CONFLICT DO NOTHING",
            )
            .bind(found)
            .execute(&mut **transaction)
            .await?;
            sqlx::query(
                "INSERT INTO project_instructions (project_id, content) VALUES ($1, '') ON CONFLICT DO NOTHING",
            )
            .bind(found)
            .execute(&mut **transaction)
            .await?;
            // Everyone already in the workspace is in its space from the start.
            sqlx::query(
                r#"
                SELECT workspace_space_add_member($1, membership.actor_id)
                FROM organization_memberships membership
                WHERE membership.organization_id = $2 AND membership.enabled
                ORDER BY membership.created_at, membership.actor_id
                "#,
            )
            .bind(found)
            .bind(organization_id)
            .execute(&mut **transaction)
            .await?;
            return Ok(found);
        }
    }
    Err(AppError::Internal("workspace space could not be created"))
}

/// The workspace space, with the actor a member of it, when they're an
/// enabled member of the workspace; `None` when they aren't. Making the
/// space adds everyone in the workspace; later joiners are added as their
/// workspace membership commits (`workspace_space_add_member`).
pub async fn join(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    actor_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    let member = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM organization_memberships membership
            JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
            WHERE membership.organization_id = $1 AND membership.actor_id = $2
              AND membership.enabled
        )
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .fetch_one(&mut **transaction)
    .await?;
    if !member {
        return Ok(None);
    }
    let project_id = ensure(transaction, organization_id).await?;
    sqlx::query("SELECT workspace_space_add_member($1, $2)")
        .bind(project_id)
        .bind(actor_id)
        .execute(&mut **transaction)
        .await?;
    Ok(Some(project_id))
}
