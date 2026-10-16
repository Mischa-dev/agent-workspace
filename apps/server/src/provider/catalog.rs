//! Each workspace's model catalog per harness (H3). Adapters that can list
//! their models refresh it; people edit the rest (context size, prices,
//! names, hiding a model) and those edits survive every refresh.

use serde::Serialize;
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use super::manifest::{AdapterManifest, ModelEntry, ModelOverrides};
use crate::error::AppError;

/// The most models one harness may list for one workspace.
pub const MAX_CATALOG_MODELS: usize = 200;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    #[serde(flatten)]
    pub model: ModelEntry,
    /// `provider` when the harness reported it, `manual` when a person added
    /// it, `adapter` when it comes from the adapter's built-in list.
    pub source: &'static str,
    pub edited: bool,
    pub overrides: ModelOverrides,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub models: Vec<CatalogModel>,
    pub refreshed_at: Option<String>,
}

#[derive(FromRow)]
struct ModelRow {
    model_id: String,
    provider_data: Option<Value>,
    overrides: Value,
}

pub async fn catalog(
    pool: &PgPool,
    organization_id: Uuid,
    manifest: &AdapterManifest,
) -> Result<Catalog, AppError> {
    let rows = sqlx::query_as::<_, ModelRow>(
        r#"
        SELECT model_id, provider_data, overrides
        FROM harness_models
        WHERE organization_id = $1 AND adapter_kind = $2
        ORDER BY position, model_id
        LIMIT $3
        "#,
    )
    .bind(organization_id)
    .bind(&manifest.kind)
    .bind(i64::try_from(MAX_CATALOG_MODELS).unwrap_or(200))
    .fetch_all(pool)
    .await?;
    let refreshed_at = sqlx::query_scalar::<_, String>(
        r#"
        SELECT refreshed_at::text FROM harness_catalog_refreshes
        WHERE organization_id = $1 AND adapter_kind = $2
        "#,
    )
    .bind(organization_id)
    .bind(&manifest.kind)
    .fetch_optional(pool)
    .await?;

    let mut models = Vec::with_capacity(rows.len().max(manifest.default_models.len()));
    for default in &manifest.default_models {
        if !rows.iter().any(|row| row.model_id == default.id) {
            models.push(CatalogModel {
                model: default.clone(),
                source: "adapter",
                edited: false,
                overrides: ModelOverrides::default(),
            });
        }
    }
    for row in rows {
        let overrides: ModelOverrides = serde_json::from_value(row.overrides).unwrap_or_default();
        let (base, source) = match row
            .provider_data
            .and_then(|data| serde_json::from_value::<ModelEntry>(data).ok())
        {
            Some(entry) => (entry, "provider"),
            None => match manifest
                .default_models
                .iter()
                .find(|model| model.id == row.model_id)
            {
                Some(default) => (default.clone(), "adapter"),
                None => (
                    ModelEntry {
                        display_name: row.model_id.clone(),
                        id: row.model_id.clone(),
                        ..ModelEntry::default()
                    },
                    "manual",
                ),
            },
        };
        let edited = overrides != ModelOverrides::default();
        models.push(CatalogModel {
            model: overrides.apply(base),
            source,
            edited,
            overrides,
        });
    }
    // Exactly one default when there is any model at all: one a person chose
    // wins over the harness's, and among several the first one counts.
    let chosen = models
        .iter()
        .position(|model| model.overrides.is_default == Some(true) && !model.model.hidden)
        .or_else(|| {
            models
                .iter()
                .position(|model| model.model.is_default && !model.model.hidden)
        });
    for (index, model) in models.iter_mut().enumerate() {
        model.model.is_default = Some(index) == chosen;
    }
    if chosen.is_none()
        && let Some(first) = models.iter_mut().find(|model| !model.model.hidden)
    {
        first.model.is_default = true;
    }
    Ok(Catalog {
        models,
        refreshed_at,
    })
}

/// The models a run may use: the catalog without hidden ones.
pub async fn pickable_models(
    pool: &PgPool,
    organization_id: Uuid,
    manifest: &AdapterManifest,
) -> Result<Vec<ModelEntry>, AppError> {
    Ok(catalog(pool, organization_id, manifest)
        .await?
        .models
        .into_iter()
        .map(|model| model.model)
        .filter(|model| !model.hidden)
        .collect())
}

/// Replaces what the harness reported, keeping every person's edits. Models
/// the harness no longer lists are dropped unless someone edited them.
pub async fn store_refresh(
    pool: &PgPool,
    organization_id: Uuid,
    adapter_kind: &str,
    models: &[ModelEntry],
) -> Result<usize, AppError> {
    let models = &models[..models.len().min(MAX_CATALOG_MODELS)];
    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("harness-catalog:{organization_id}:{adapter_kind}"))
        .execute(&mut *transaction)
        .await?;
    let ids = models
        .iter()
        .map(|model| model.id.clone())
        .collect::<Vec<_>>();
    sqlx::query(
        r#"
        DELETE FROM harness_models
        WHERE organization_id = $1 AND adapter_kind = $2
          AND provider_data IS NOT NULL AND overrides = '{}'::jsonb
          AND NOT (model_id = ANY($3))
        "#,
    )
    .bind(organization_id)
    .bind(adapter_kind)
    .bind(&ids)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE harness_models
        SET provider_data = NULL, updated_at = now(), version = version + 1
        WHERE organization_id = $1 AND adapter_kind = $2
          AND provider_data IS NOT NULL
          AND NOT (model_id = ANY($3))
        "#,
    )
    .bind(organization_id)
    .bind(adapter_kind)
    .bind(&ids)
    .execute(&mut *transaction)
    .await?;
    for (position, model) in models.iter().enumerate() {
        let data = serde_json::to_value(model)
            .map_err(|_| AppError::Internal("serialize catalog model"))?;
        sqlx::query(
            r#"
            INSERT INTO harness_models (
                id, organization_id, adapter_kind, model_id, provider_data,
                position, refreshed_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, now())
            ON CONFLICT (organization_id, adapter_kind, model_id)
            DO UPDATE SET provider_data = EXCLUDED.provider_data,
                          position = EXCLUDED.position,
                          refreshed_at = now(),
                          updated_at = now(),
                          version = harness_models.version + 1
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(organization_id)
        .bind(adapter_kind)
        .bind(&model.id)
        .bind(data)
        .bind(i32::try_from(position).unwrap_or(i32::MAX))
        .execute(&mut *transaction)
        .await?;
    }
    sqlx::query(
        r#"
        INSERT INTO harness_catalog_refreshes (
            organization_id, adapter_kind, refreshed_at, model_count
        )
        VALUES ($1, $2, now(), $3)
        ON CONFLICT (organization_id, adapter_kind)
        DO UPDATE SET refreshed_at = now(), model_count = EXCLUDED.model_count
        "#,
    )
    .bind(organization_id)
    .bind(adapter_kind)
    .bind(i32::try_from(models.len()).unwrap_or(i32::MAX))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(models.len())
}

/// Saves a person's edits to one model, adding it by hand when the catalog
/// doesn't have it.
pub async fn save_overrides(
    pool: &PgPool,
    organization_id: Uuid,
    adapter_kind: &str,
    model_id: &str,
    overrides: &ModelOverrides,
    actor_id: Uuid,
) -> Result<(), AppError> {
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM harness_models WHERE organization_id = $1 AND adapter_kind = $2",
    )
    .bind(organization_id)
    .bind(adapter_kind)
    .fetch_one(pool)
    .await?;
    let choose_default = overrides.is_default == Some(true);
    let overrides = serde_json::to_value(overrides)
        .map_err(|_| AppError::Internal("serialize catalog overrides"))?;
    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("harness_models:{organization_id}:{adapter_kind}"))
        .execute(&mut *transaction)
        .await?;
    if choose_default {
        // A new default replaces whichever model a person chose before.
        sqlx::query(
            r#"
            UPDATE harness_models
               SET overrides = overrides - 'isDefault', updated_at = now(),
                   version = version + 1
             WHERE organization_id = $1 AND adapter_kind = $2 AND model_id <> $3
               AND overrides ? 'isDefault'
            "#,
        )
        .bind(organization_id)
        .bind(adapter_kind)
        .bind(model_id)
        .execute(&mut *transaction)
        .await?;
    }
    let saved = sqlx::query(
        r#"
        INSERT INTO harness_models (
            id, organization_id, adapter_kind, model_id, overrides, position,
            edited_by_actor_id, edited_at
        )
        SELECT $1, $2, $3, $4, $5, $6, $7, now()
        WHERE EXISTS (
            SELECT 1 FROM harness_models
            WHERE organization_id = $2 AND adapter_kind = $3 AND model_id = $4
        ) OR $8
        ON CONFLICT (organization_id, adapter_kind, model_id)
        DO UPDATE SET overrides = EXCLUDED.overrides,
                      edited_by_actor_id = EXCLUDED.edited_by_actor_id,
                      edited_at = now(), updated_at = now(),
                      version = harness_models.version + 1
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(organization_id)
    .bind(adapter_kind)
    .bind(model_id)
    .bind(overrides)
    .bind(i32::try_from(count).unwrap_or(i32::MAX))
    .bind(actor_id)
    .bind(usize::try_from(count).unwrap_or(usize::MAX) < MAX_CATALOG_MODELS)
    .execute(&mut *transaction)
    .await?;
    if saved.rows_affected() == 0 {
        return Err(AppError::Conflict(
            "harness_model",
            "the catalog is full; hide or remove a model first",
        ));
    }
    transaction.commit().await?;
    Ok(())
}

/// Removes a model a person added by hand, or clears their edits to one the
/// harness reports.
pub async fn remove_model(
    pool: &PgPool,
    organization_id: Uuid,
    adapter_kind: &str,
    model_id: &str,
) -> Result<bool, AppError> {
    let deleted = sqlx::query(
        r#"
        DELETE FROM harness_models
        WHERE organization_id = $1 AND adapter_kind = $2 AND model_id = $3
          AND provider_data IS NULL
        "#,
    )
    .bind(organization_id)
    .bind(adapter_kind)
    .bind(model_id)
    .execute(pool)
    .await?
    .rows_affected();
    let cleared = sqlx::query(
        r#"
        UPDATE harness_models
        SET overrides = '{}'::jsonb, edited_by_actor_id = NULL, edited_at = NULL,
            updated_at = now(), version = version + 1
        WHERE organization_id = $1 AND adapter_kind = $2 AND model_id = $3
          AND provider_data IS NOT NULL
        "#,
    )
    .bind(organization_id)
    .bind(adapter_kind)
    .bind(model_id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(deleted + cleared > 0)
}
