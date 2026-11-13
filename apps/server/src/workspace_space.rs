//! The workspace space (AWD-96): a hidden project, one per workspace, that
//! holds the pages belonging to the workspace instead of to one project.
//!
//! It reuses a project row the way the Messages space does, so pages there
//! keep every page path (revisions, live editing, comments, search, links,
//! "Referenced in") and the same membership checks. Every enabled member of
//! the workspace, people and agents, is a member with their workspace role:
//! added here the first time they reach it, disabled with their other
//! memberships when they leave the workspace, and kept in step by the
//! `organization_memberships_workspace_space_role` trigger. It's never
//! listed as a project, and only Pages is needed there, not Projects.

use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{error::AppError, projects::membership_handle};

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
            return Ok(found);
        }
    }
    Err(AppError::Internal("workspace space could not be created"))
}

/// The workspace space, with the actor a member of it, when they're an
/// enabled member of the workspace; `None` when they aren't.
pub async fn join(
    transaction: &mut Transaction<'_, Postgres>,
    organization_id: Uuid,
    actor_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    let Some((role, role_id)) = sqlx::query_as::<_, (String, Option<Uuid>)>(
        r#"
        SELECT membership.role, membership.role_id
        FROM organization_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
        WHERE membership.organization_id = $1 AND membership.actor_id = $2
          AND membership.enabled
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(None);
    };
    let project_id = ensure(transaction, organization_id).await?;
    let joined = sqlx::query_scalar::<_, bool>(
        "SELECT enabled FROM project_memberships WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let role = match role.as_str() {
        "owner" => "owner",
        "admin" => "admin",
        _ => "member",
    };
    match joined {
        Some(true) => return Ok(Some(project_id)),
        Some(false) => {
            // Back in the workspace: back in its space, with today's role.
            sqlx::query(
                r#"
                UPDATE project_memberships
                SET enabled = true, role = $3, role_id = $4, disabled_by_actor_id = NULL,
                    disabled_at = NULL, version = version + 1, updated_at = now()
                WHERE project_id = $1 AND actor_id = $2
                "#,
            )
            .bind(project_id)
            .bind(actor_id)
            .bind(role)
            .bind(role_id)
            .execute(&mut **transaction)
            .await?;
            return Ok(Some(project_id));
        }
        None => {}
    }
    // A person's name is their account's; an agent's is the one it has in
    // its projects.
    let (display_name, handle) = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT display_name, handle FROM (
            SELECT person.display_name, person.login_name AS handle, 0 AS rank, now() AS at
            FROM users person WHERE person.actor_id = $2
            UNION ALL
            SELECT membership.display_name, membership.handle, 1, membership.created_at
            FROM project_memberships membership
            JOIN projects project ON project.id = membership.project_id
            WHERE membership.actor_id = $2 AND project.organization_id = $1
        ) named
        ORDER BY rank, at
        LIMIT 1
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .unwrap_or_else(|| ("Agent".to_owned(), format!("agent-{}", short(actor_id))));
    let handle = membership_handle(&handle);
    // Handles are unique in a project: a taken one gets the actor's suffix.
    let fallback = format!(
        "{}-{}",
        handle.chars().take(24).collect::<String>(),
        short(actor_id)
    );
    for candidate in [handle, fallback] {
        let inserted = sqlx::query(
            r#"
            INSERT INTO project_memberships
                (id, project_id, actor_id, role, role_id, display_name, handle)
            SELECT $1, $2, $3, $4, $5, $6, $7
            WHERE NOT EXISTS (
                SELECT 1 FROM project_memberships
                WHERE project_id = $2 AND enabled AND lower(handle) = lower($7)
            )
            ON CONFLICT (project_id, actor_id) DO NOTHING
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(project_id)
        .bind(actor_id)
        .bind(role)
        .bind(role_id)
        .bind(&display_name)
        .bind(&candidate)
        .execute(&mut **transaction)
        .await?;
        if inserted.rows_affected() > 0 {
            return Ok(Some(project_id));
        }
        // Someone else's call may have just added them.
        let added = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM project_memberships WHERE project_id = $1 AND actor_id = $2)",
        )
        .bind(project_id)
        .bind(actor_id)
        .fetch_one(&mut **transaction)
        .await?;
        if added {
            return Ok(Some(project_id));
        }
    }
    Err(AppError::Internal(
        "workspace space membership could not be added",
    ))
}

fn short(actor_id: Uuid) -> String {
    actor_id.simple().to_string()[26..].to_owned()
}
