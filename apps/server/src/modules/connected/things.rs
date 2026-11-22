//! A connected module's things (M1.13, ruling R54). Its manifest declares
//! qualified kinds (`acme.build`); installing it registers them, and only
//! that install writes things of those kinds in its workspace, through
//! `thing_upsert`. A thing is a `things` row like any other (title, status,
//! space), with its address in the app and its fields beside it in
//! `module_things`, so links, events and access treat it as any thing.
//! Workspace keeps the envelope rather than asking the app on every read,
//! so a thing still reads when the app is slow or gone.

use rmcp::schemars::{self, JsonSchema};
use serde_json::{Map, Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::manifest::Manifest;
use crate::{error::AppError, ops::OpError};

const MAX_FIELDS_BYTES: usize = 16 * 1024;

/// Registers the manifest's kinds for this install in its workspace. A
/// kind another live install there owns is refused; one a revoked install
/// left passes to this one, with its things.
pub(crate) async fn register_kinds(
    transaction: &mut Transaction<'static, Postgres>,
    workspace_id: Uuid,
    module_id: Uuid,
    manifest: &Manifest,
) -> Result<(), OpError> {
    for kind in &manifest.kinds {
        sqlx::query(
            "INSERT INTO thing_kinds (kind, module) VALUES ($1, 'connected') \
             ON CONFLICT (kind) DO NOTHING",
        )
        .bind(&kind.kind)
        .execute(&mut **transaction)
        .await
        .map_err(AppError::from)?;
        let owner = sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO module_kinds (workspace_id, kind, module_id, one, many)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (workspace_id, kind) DO UPDATE
               SET module_id = EXCLUDED.module_id, one = EXCLUDED.one, many = EXCLUDED.many
             WHERE module_kinds.module_id = EXCLUDED.module_id
                OR EXISTS (SELECT 1 FROM module_apps gone
                           WHERE gone.module_id = module_kinds.module_id
                             AND gone.state = 'revoked')
            RETURNING module_id
            "#,
        )
        .bind(workspace_id)
        .bind(&kind.kind)
        .bind(module_id)
        .bind(&kind.one)
        .bind(&kind.many)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(AppError::from)?;
        if owner != Some(module_id) {
            return Err(OpError::Conflict {
                reason: crate::ops::ConflictReason::State { code: "kind_taken" },
                message: format!(
                    "Another connected module here already owns the kind {}.",
                    kind.kind
                )
                .into(),
                current: None,
            });
        }
        // Things a revoked install left now belong to this one.
        sqlx::query(
            "UPDATE module_things mine SET module_id = $3, updated_at = now() \
             FROM things thing \
             WHERE thing.id = mine.thing_id AND mine.workspace_id = $1 AND thing.kind = $2 \
               AND mine.module_id <> $3",
        )
        .bind(workspace_id)
        .bind(&kind.kind)
        .bind(module_id)
        .execute(&mut **transaction)
        .await
        .map_err(AppError::from)?;
    }
    Ok(())
}

/// The install that owns `kind` in the workspace, if `account_id` is its
/// live account: only it writes or sends events about those things.
pub(crate) async fn owner_for(
    connection: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    kind: &str,
    account_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT owned.module_id FROM module_kinds owned \
         JOIN module_apps app ON app.module_id = owned.module_id \
         WHERE owned.workspace_id = $1 AND owned.kind = $2 \
           AND app.account_id = $3 AND app.state = 'active'",
    )
    .bind(workspace_id)
    .bind(kind)
    .bind(account_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// What the app says about one of its things.
pub(crate) struct Upsert<'a> {
    pub id: Option<Uuid>,
    pub kind: &'a str,
    pub title: &'a str,
    pub status: Option<&'a str>,
    pub space_id: Option<Uuid>,
    pub url: Option<&'a str>,
    pub fields: Option<&'a Map<String, Value>>,
    pub archived: Option<bool>,
}

/// A connected thing, as `thing_upsert` answers.
#[derive(Debug, serde::Deserialize, serde::Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedThingView {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub kind: String,
    pub title: String,
    pub status: Option<String>,
    #[schemars(with = "Option<String>")]
    pub space_id: Option<Uuid>,
    pub url: Option<String>,
    #[schemars(with = "serde_json::Map<String, Value>")]
    pub fields: Value,
    pub version: i64,
    pub archived: bool,
    /// False when it was made by this call.
    pub existed: bool,
}

fn invalid(field: &'static str, message: &'static str) -> OpError {
    OpError::invalid(field, message)
}

/// Checks what the app sent, before anything is written.
pub(crate) fn check(upsert: &Upsert<'_>) -> Result<(), OpError> {
    let title = upsert.title.trim();
    if title.is_empty() || title.chars().count() > 300 || title.contains(['\n', '\r']) {
        return Err(invalid(
            "title",
            "Give a title of 1 to 300 characters on one line.",
        ));
    }
    if let Some(status) = upsert.status {
        let mut chars = status.chars();
        let fits = status.len() <= 40
            && chars.next().is_some_and(|first| first.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !fits {
            return Err(invalid(
                "status",
                "A status is a key: lowercase letters, digits and _, up to 40.",
            ));
        }
    }
    if let Some(url) = upsert.url {
        let parsed = url::Url::parse(url).ok();
        if url.len() > 2000 || !parsed.is_some_and(|url| matches!(url.scheme(), "https" | "http")) {
            return Err(invalid("url", "Give the thing's https address in the app."));
        }
    }
    if let Some(fields) = upsert.fields
        && serde_json::to_vec(fields).map_or(true, |bytes| bytes.len() > MAX_FIELDS_BYTES)
    {
        return Err(invalid("fields", "Fields are at most 16 KB."));
    }
    Ok(())
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    kind: String,
    title: Option<String>,
    status: Option<String>,
    space_id: Option<Uuid>,
    version: i64,
    archived: bool,
    external_url: Option<String>,
    fields: Value,
}

/// Writes one thing for its owning install, in the call's transaction, and
/// stages the event saying so.
pub(crate) async fn upsert(
    transaction: &mut Transaction<'static, Postgres>,
    workspace_id: Uuid,
    module_id: Uuid,
    account_id: Uuid,
    upsert: Upsert<'_>,
) -> Result<ConnectedThingView, OpError> {
    let title = upsert.title.trim();
    let archived = upsert.archived;
    let existed = match upsert.id {
        Some(id) => {
            let found = sqlx::query_as::<_, (String, Option<Uuid>)>(
                "SELECT thing.kind, mine.module_id FROM things thing \
                 LEFT JOIN module_things mine ON mine.thing_id = thing.id \
                 WHERE thing.id = $1 AND thing.workspace_id = $2 AND thing.deleted_at IS NULL \
                 FOR UPDATE OF thing",
            )
            .bind(id)
            .bind(workspace_id)
            .fetch_optional(&mut **transaction)
            .await
            .map_err(AppError::from)?;
            match found {
                None => false,
                Some((kind, Some(owner))) if kind == upsert.kind && owner == module_id => true,
                // Another kind, or not this install's: as good as not there.
                Some(_) => return Err(OpError::not_found("That thing")),
            }
        }
        None => false,
    };
    let id = upsert.id.unwrap_or_else(Uuid::now_v7);
    if existed {
        sqlx::query(
            r#"
            UPDATE things
            SET title = $2, status = COALESCE($3, status), space_id = COALESCE($4, space_id),
                archived_at = CASE WHEN $5::bool IS NULL THEN archived_at
                                   WHEN $5 THEN COALESCE(archived_at, now()) ELSE NULL END,
                updated_by_account_id = $6, version = version + 1, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(title)
        .bind(upsert.status)
        .bind(upsert.space_id)
        .bind(archived)
        .bind(account_id)
        .execute(&mut **transaction)
        .await
        .map_err(AppError::from)?;
        sqlx::query(
            "UPDATE module_things SET external_url = COALESCE($2, external_url), \
             fields = COALESCE($3, fields), updated_at = now() WHERE thing_id = $1",
        )
        .bind(id)
        .bind(upsert.url)
        .bind(upsert.fields.map(|fields| Value::Object(fields.clone())))
        .execute(&mut **transaction)
        .await
        .map_err(AppError::from)?;
    } else {
        sqlx::query(
            r#"
            INSERT INTO things (id, kind, workspace_id, space_id, title, status,
                                created_by_account_id, updated_by_account_id, version,
                                created_at, updated_at, archived_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $7, 1, now(), now(),
                    CASE WHEN $8 THEN now() END)
            "#,
        )
        .bind(id)
        .bind(upsert.kind)
        .bind(workspace_id)
        .bind(upsert.space_id)
        .bind(title)
        .bind(upsert.status)
        .bind(account_id)
        .bind(archived.unwrap_or(false))
        .execute(&mut **transaction)
        .await
        .map_err(AppError::from)?;
        sqlx::query(
            "INSERT INTO module_things (thing_id, workspace_id, module_id, external_url, fields) \
             VALUES ($1, $2, $3, $4, COALESCE($5, '{}'::jsonb))",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(module_id)
        .bind(upsert.url)
        .bind(upsert.fields.map(|fields| Value::Object(fields.clone())))
        .execute(&mut **transaction)
        .await
        .map_err(AppError::from)?;
    }
    let row = sqlx::query_as::<_, Row>(
        "SELECT thing.id, thing.kind, thing.title, thing.status, thing.space_id, thing.version, \
                thing.archived_at IS NOT NULL AS archived, mine.external_url, mine.fields \
         FROM things thing JOIN module_things mine ON mine.thing_id = thing.id \
         WHERE thing.id = $1",
    )
    .bind(id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(AppError::from)?;
    let ty = &crate::events::catalog::CONNECTED_THING_CHANGED;
    let event = match row.space_id {
        Some(space) => crate::events::NewEvent::thing(ty, space, upsert.kind, id),
        None => {
            let mut event = crate::events::NewEvent::thing(ty, workspace_id, upsert.kind, id)
                .audience(crate::events::Audience::Workspace);
            event.space_id = None;
            event.workspace(workspace_id)
        }
    };
    crate::events::emit(
        transaction,
        event
            .version(row.version)
            .actor(account_id)
            .payload(json!({ "moduleId": module_id, "created": !existed })),
    )
    .await?;
    Ok(ConnectedThingView {
        id: row.id,
        kind: row.kind,
        title: row.title.unwrap_or_default(),
        status: row.status,
        space_id: row.space_id,
        url: row.external_url,
        fields: row.fields,
        version: row.version,
        archived: row.archived,
        existed,
    })
}

/// A thing of `kind` this install wrote, in the workspace: for an event
/// about it. Its space, if it has one.
pub(crate) async fn owned_thing(
    connection: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    module_id: Uuid,
    kind: &str,
    id: Uuid,
) -> Result<Option<Option<Uuid>>, AppError> {
    Ok(sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT thing.space_id FROM things thing \
         JOIN module_things mine ON mine.thing_id = thing.id \
         WHERE thing.id = $1 AND thing.workspace_id = $2 AND thing.kind = $3 \
           AND mine.module_id = $4 AND thing.deleted_at IS NULL",
    )
    .bind(id)
    .bind(workspace_id)
    .bind(kind)
    .bind(module_id)
    .fetch_optional(&mut *connection)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample<'a>() -> Upsert<'a> {
        Upsert {
            id: None,
            kind: "acme.build",
            title: "Build 214",
            status: Some("running"),
            space_id: None,
            url: Some("https://builds.example.com/b/214"),
            fields: None,
            archived: None,
        }
    }

    #[test]
    fn what_the_app_sends_is_checked() {
        assert!(check(&sample()).is_ok());
        assert!(
            check(&Upsert {
                title: " ",
                ..sample()
            })
            .is_err()
        );
        assert!(
            check(&Upsert {
                title: "a\nb",
                ..sample()
            })
            .is_err()
        );
        assert!(
            check(&Upsert {
                status: Some("Running"),
                ..sample()
            })
            .is_err()
        );
        assert!(
            check(&Upsert {
                url: Some("javascript:alert(1)"),
                ..sample()
            })
            .is_err()
        );
        let mut big = Map::new();
        big.insert("log".into(), Value::String("x".repeat(MAX_FIELDS_BYTES)));
        assert!(
            check(&Upsert {
                fields: Some(&big),
                ..sample()
            })
            .is_err()
        );
    }
}
