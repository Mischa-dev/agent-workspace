//! Workspace slugs (K4.5): the rules for a new slug, the reserved words,
//! readable slugs for new workspaces, renames that keep old links working,
//! and the lookup behind `/w/<slug>`.
//!
//! The history lives in `workspace_slugs`, written by a trigger on
//! `organizations` whenever a slug is set, so every path that makes or
//! renames a workspace keeps it. A retired slug stays with its workspace for
//! good: another workspace can't take it, and a member following an old link
//! is sent on to the current slug.

use axum::{Json, Router, extract::State, routing::get};
use serde::Serialize;
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    access::{self, Need, Target},
    auth::AuthenticatedActor,
    error::{ApiPath, AppError},
};

/// The shortest and longest slug a person may choose. Older slugs outside
/// these bounds (WorkOS's 42-character ones) stay valid.
pub const MIN_LENGTH: usize = 3;
pub const MAX_LENGTH: usize = 40;

/// Words no workspace may take: the web's own top-level paths and the
/// direct scope's reserved row (K4.23), which must never be addressable.
pub const RESERVED: &[&str] = &[
    "admin",
    "api",
    "app",
    "assets",
    "aw-direct-scope",
    "dm",
    "forms",
    "help",
    "join",
    "new",
    "settings",
    "sign-in",
    "static",
    "status",
    "sw",
    "w",
    "www",
];

/// Check a slug a person chose. Returns it lowercased.
pub fn check_new(value: &str) -> Result<String, AppError> {
    let slug = value.trim().to_ascii_lowercase();
    if slug.len() < MIN_LENGTH || slug.len() > MAX_LENGTH || !well_formed(&slug) {
        return Err(AppError::Validation("workspace slug"));
    }
    if RESERVED.contains(&slug.as_str()) {
        return Err(AppError::Refused {
            field: "slug",
            message: "That address is reserved. Choose another.",
        });
    }
    Ok(slug)
}

/// Lowercase letters and digits in groups joined by single hyphens.
pub fn well_formed(slug: &str) -> bool {
    !slug.is_empty()
        && slug.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

/// A slug made from a workspace's name: letters and digits kept, anything
/// else a hyphen, cut to leave room for a numeric suffix. Falls back to
/// `workspace` when the name gives too little, and never a reserved word.
pub fn from_name(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= MAX_LENGTH - 4 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-').to_owned();
    if slug.len() < MIN_LENGTH {
        return "workspace".to_owned();
    }
    if RESERVED.contains(&slug.as_str()) {
        return format!("{slug}-workspace");
    }
    slug
}

/// A free, readable slug for a new workspace named `name`: the name's slug,
/// else that with `-2`, `-3` and so on. Neither a current nor a retired slug
/// of any workspace is reused.
pub async fn readable_for_new(
    connection: &mut PgConnection,
    name: &str,
) -> Result<String, AppError> {
    let base = from_name(name);
    for suffix in 1..=50_u32 {
        let candidate = if suffix == 1 {
            base.clone()
        } else {
            format!("{base}-{suffix}")
        };
        if !taken(connection, &candidate).await? {
            return Ok(candidate);
        }
    }
    let random = Uuid::new_v4().simple().to_string();
    Ok(format!("{base}-{}", &random[..8]))
}

async fn taken(connection: &mut PgConnection, slug: &str) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM organizations WHERE slug = $1) \
             OR EXISTS (SELECT 1 FROM workspace_slugs WHERE slug = $1)",
    )
    .bind(slug)
    .fetch_one(&mut *connection)
    .await?)
}

/// Give a workspace a new slug, already checked with `check_new`. The old
/// one is kept as a retired slug by the trigger. A slug another workspace
/// holds now or held before is refused; the workspace's own retired slug
/// may be taken back. The caller has checked `settings.manage`, holds the
/// workspace row and bumps its version.
pub async fn rename(
    transaction: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    slug: &str,
) -> Result<(), AppError> {
    let holder = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM organizations WHERE slug = $1 \
         UNION SELECT workspace_id FROM workspace_slugs WHERE slug = $1",
    )
    .bind(slug)
    .fetch_all(&mut **transaction)
    .await?;
    if holder.iter().any(|holder| *holder != workspace_id) {
        return Err(AppError::Conflict(
            "workspace slug",
            "that address belongs to another workspace",
        ));
    }
    let result =
        sqlx::query("UPDATE organizations SET slug = $2, updated_at = now() WHERE id = $1")
            .bind(workspace_id)
            .bind(slug)
            .execute(&mut **transaction)
            .await;
    match result {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("23505") => {
            Err(AppError::Conflict(
                "workspace slug",
                "that address belongs to another workspace",
            ))
        }
        Err(error) => Err(error.into()),
    }
}

/// What a slug leads to, for someone who may see the workspace.
#[derive(Debug, Eq, PartialEq)]
pub enum Found {
    Current {
        workspace_id: Uuid,
        slug: String,
    },
    /// A retired slug: the address now lives at `slug`.
    Retired {
        workspace_id: Uuid,
        slug: String,
    },
}

/// Look a slug up without any access check (the route checks).
async fn find(connection: &mut PgConnection, slug: &str) -> Result<Option<Found>, AppError> {
    if let Some(workspace_id) =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM organizations WHERE slug = $1")
            .bind(slug)
            .fetch_optional(&mut *connection)
            .await?
    {
        return Ok(Some(Found::Current {
            workspace_id,
            slug: slug.to_owned(),
        }));
    }
    let retired = sqlx::query_as::<_, (Uuid, String)>(
        r#"
        SELECT history.workspace_id, workspace.slug
        FROM workspace_slugs history
        JOIN organizations workspace ON workspace.id = history.workspace_id
        WHERE history.slug = $1 AND NOT history.current
        "#,
    )
    .bind(slug)
    .fetch_optional(&mut *connection)
    .await?;
    Ok(retired.map(|(workspace_id, slug)| Found::Retired { workspace_id, slug }))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/workspaces/by-slug/{slug}", get(by_slug_http))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase", untagged)]
enum BySlugView {
    Current { id: Uuid, slug: String },
    Redirect { id: Uuid, redirect: String },
}

/// `GET /api/v1/workspaces/by-slug/{slug}`: the workspace for a member, a
/// redirect to the current slug for a member following a retired one, and
/// 404 for everyone else, so a stranger learns nothing, not even that the
/// slug exists.
async fn by_slug_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(slug): ApiPath<String>,
) -> Result<Json<BySlugView>, AppError> {
    let caller = principal.caller()?;
    let slug = slug.trim().to_ascii_lowercase();
    let mut connection = state.pool.acquire().await?;
    if RESERVED.contains(&slug.as_str()) || !well_formed(&slug) {
        return Err(AppError::NotFound("workspace"));
    }
    let Some(found) = find(&mut connection, &slug).await? else {
        return Err(AppError::NotFound("workspace"));
    };
    let workspace_id = match &found {
        Found::Current { workspace_id, .. } | Found::Retired { workspace_id, .. } => *workspace_id,
    };
    if access::check(
        &mut connection,
        &caller,
        Need::SEE,
        Target::Workspace(workspace_id),
    )
    .await?
    .is_err()
    {
        return Err(AppError::NotFound("workspace"));
    }
    Ok(Json(match found {
        Found::Current { workspace_id, slug } => BySlugView::Current {
            id: workspace_id,
            slug,
        },
        Found::Retired { workspace_id, slug } => BySlugView::Redirect {
            id: workspace_id,
            redirect: slug,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_slugs_follow_the_rules() {
        assert_eq!(check_new(" Acme-Labs ").expect("valid"), "acme-labs");
        for bad in ["ab", "a--b", "-ab", "ab-", "a_b", "ab c", &"a".repeat(41)] {
            assert!(check_new(bad).is_err(), "{bad}");
        }
        for reserved in ["api", "w", "settings", "sign-in", "aw-direct-scope"] {
            assert!(
                matches!(
                    check_new(reserved),
                    Err(AppError::Refused { .. }) | Err(AppError::Validation(_))
                ),
                "{reserved}"
            );
        }
        assert!(matches!(
            check_new("aw-direct-scope"),
            Err(AppError::Refused { .. })
        ));
    }

    #[test]
    fn names_make_readable_slugs() {
        assert_eq!(from_name("Acme Labs!"), "acme-labs");
        assert_eq!(from_name("  Ünïcode & Co  "), "n-code-co");
        assert_eq!(from_name("My workspace"), "my-workspace");
        assert_eq!(from_name("??"), "workspace");
        assert_eq!(from_name("API"), "api-workspace");
        let long = from_name(&"word ".repeat(30));
        assert!(long.len() <= MAX_LENGTH - 4 && well_formed(&long));
        for name in ["Acme Labs!", "x", "API", &"word ".repeat(30)] {
            assert!(check_new(&from_name(name)).is_ok(), "{name}");
        }
    }
}
