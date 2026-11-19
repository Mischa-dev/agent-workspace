//! Starting shapes (K4.20, K4 §3.11): what a space holds when it starts,
//! as data. The built-in shapes are `modules/spaces/<kind>.json`; a shape
//! is the space part of a package (M1), so packages and workspace templates
//! describe a space the same way.
//!
//! Applying a shape is idempotent and never deletes anything: a channel or
//! page already there by its name is left as it is. The shape a space was
//! made from is kept in `projects.shape`, so "made from" and later updates
//! can read it.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::SpaceKind;
use crate::{
    conversations::NewChannel,
    error::AppError,
    memory::{self, PageInput, Writer},
};

/// A space's starting shape.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpaceShape {
    /// The kind of space it is for: `project`, `team`, `personal` or
    /// `workspace`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<ShapeChannel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<ShapePage>,
}

/// A channel a shape makes.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShapeChannel {
    pub name: String,
    /// Its members only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub private: bool,
}

/// A page a shape makes.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShapePage {
    pub title: String,
    /// The space's guide, which agents read before they work there.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub guide: bool,
    #[serde(default)]
    pub markdown: String,
}

/// The built-in shape of a kind of space.
pub fn builtin(kind: SpaceKind) -> SpaceShape {
    let text = match kind {
        SpaceKind::Project => include_str!("../../../../modules/spaces/project.json"),
        SpaceKind::Team => include_str!("../../../../modules/spaces/team.json"),
        SpaceKind::Personal => include_str!("../../../../modules/spaces/personal.json"),
        SpaceKind::Workspace => include_str!("../../../../modules/spaces/workspace.json"),
    };
    // The files are checked by a test; one that doesn't read makes nothing.
    serde_json::from_str(text).unwrap_or_default()
}

/// What applying a shape made.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Made {
    pub channels: Vec<Uuid>,
    pub pages: Vec<Uuid>,
}

/// Make what `shape` names in `space` that isn't there yet, as `writer`,
/// and record the shape on the space.
pub async fn apply(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    space: Uuid,
    shape: &SpaceShape,
) -> Result<Made, AppError> {
    let mut made = Made::default();
    // One shape at a time per space, so two at once don't both make a page.
    sqlx::query("SELECT id FROM projects WHERE id = $1 FOR UPDATE")
        .bind(space)
        .execute(&mut **transaction)
        .await?;
    for channel in &shape.channels {
        let name = crate::conversations::normalize_conversation_name(&channel.name)?;
        let exists = sqlx::query_scalar::<_, bool>(
            r#"
            -- visibility: only whether a channel by the name exists is read,
            -- since names are unique in a space, never what is in it.
            SELECT EXISTS (
                SELECT 1 FROM conversations
                WHERE project_id = $1 AND kind = 'channel' AND archived_at IS NULL
                  AND lower(name) = lower($2)
            )
            "#,
        )
        .bind(space)
        .bind(&name)
        .fetch_one(&mut **transaction)
        .await?;
        if !exists {
            let id = crate::conversations::insert_channel(
                transaction,
                space,
                writer.actor_id,
                &name,
                if channel.private {
                    NewChannel {
                        private: true,
                        auto_join: false,
                    }
                } else {
                    NewChannel::EVERYONE
                },
            )
            .await?;
            made.channels.push(id);
        }
    }
    for page in &shape.pages {
        let title = page.title.trim();
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM project_pages \
             WHERE project_id = $1 AND archived_at IS NULL AND lower(title) = lower($2))",
        )
        .bind(space)
        .bind(title)
        .fetch_one(&mut **transaction)
        .await?;
        if !exists {
            let id = memory::create_page(
                transaction,
                writer,
                space,
                &PageInput {
                    title: Some(title.to_owned()),
                    body: Some(page.markdown.clone()),
                    is_guide: page.guide.then_some(true),
                    is_template: None,
                    from_template_id: None,
                    skill_description: None,
                    expected_version: None,
                    restore_version: None,
                    archived: None,
                    note: None,
                    parent_page_id: None,
                },
            )
            .await?;
            made.pages.push(id);
        }
    }
    sqlx::query("UPDATE projects SET shape = $2 WHERE id = $1")
        .bind(space)
        .bind(sqlx::types::Json(shape))
        .execute(&mut **transaction)
        .await?;
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_shapes_read() {
        for kind in [
            SpaceKind::Project,
            SpaceKind::Team,
            SpaceKind::Personal,
            SpaceKind::Workspace,
        ] {
            let text = match kind {
                SpaceKind::Project => include_str!("../../../../modules/spaces/project.json"),
                SpaceKind::Team => include_str!("../../../../modules/spaces/team.json"),
                SpaceKind::Personal => include_str!("../../../../modules/spaces/personal.json"),
                SpaceKind::Workspace => include_str!("../../../../modules/spaces/workspace.json"),
            };
            let shape: SpaceShape = serde_json::from_str(text).expect("a shape");
            assert_eq!(shape, builtin(kind));
            assert_eq!(
                shape.kind.as_deref(),
                Some(match kind {
                    SpaceKind::Project => "project",
                    other => other.stored(),
                })
            );
        }
        assert!(!builtin(SpaceKind::Team).channels.is_empty());
        assert!(builtin(SpaceKind::Team).pages.iter().any(|page| page.guide));
    }
}
