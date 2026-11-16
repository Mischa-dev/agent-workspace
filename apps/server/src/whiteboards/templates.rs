//! Board templates (C1.24): a board marked `is_template`, or a built-in one
//! that ships as scene operations (the `whiteboard_edit` language) under
//! `src/modules/templates/whiteboards/`. Starting a board from a template
//! copies its drawing into a new board wherever the caller puts it.
//!
//! A template board from the same workspace that the caller can see can be
//! used; anything else is "not found", as if it weren't there.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{
    Caller, Placement, access, copy_into, create,
    edit::{self, EditError},
    normalize_title,
    scene::Operation,
};
use crate::error::AppError;

struct BuiltIn {
    id: &'static str,
    json: &'static str,
}

const BUILT_IN: &[BuiltIn] = &[
    BuiltIn {
        id: "retrospective",
        json: include_str!("../modules/templates/whiteboards/retrospective.json"),
    },
    BuiltIn {
        id: "user-journey",
        json: include_str!("../modules/templates/whiteboards/user-journey.json"),
    },
    BuiltIn {
        id: "flowchart",
        json: include_str!("../modules/templates/whiteboards/flowchart.json"),
    },
    BuiltIn {
        id: "two-by-two",
        json: include_str!("../modules/templates/whiteboards/two-by-two.json"),
    },
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scene {
    title: String,
    operations: Vec<Operation>,
}

fn scene(built_in: &BuiltIn) -> Option<Scene> {
    serde_json::from_str(built_in.json).ok()
}

/// A template as the gallery lists it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "WhiteboardTemplateView")]
pub struct TemplateSummary {
    /// A board's ID, or a built-in template's name.
    pub id: String,
    pub title: String,
    pub built_in: bool,
    /// The project a template board lives in; none for built-in ones and
    /// boards for the whole workspace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
}

/// The templates the caller can start a board from in a workspace: the
/// built-in ones, then template boards there they can see.
pub async fn list(
    connection: &mut sqlx::PgConnection,
    caller: &Caller,
    organization_id: Uuid,
) -> Result<Vec<TemplateSummary>, AppError> {
    let mut templates = BUILT_IN
        .iter()
        .filter_map(|built_in| {
            scene(built_in).map(|scene| TemplateSummary {
                id: built_in.id.to_owned(),
                title: scene.title,
                built_in: true,
                project_name: None,
            })
        })
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, (Uuid, String, Option<String>)>(
        r#"
        SELECT board.id, board.title, project.name
        FROM whiteboards board
        LEFT JOIN projects project ON project.id = board.project_id
        WHERE board.organization_id = $1 AND board.is_template AND board.deleted_at IS NULL
          AND whiteboard_visible_to(board.id, $2)
          AND (board.project_id IS NULL OR actor_can_access_project($2, $3, board.project_id))
          AND ($4::uuid IS NULL OR board.project_id = $4)
        ORDER BY lower(board.title), board.id
        LIMIT 200
        "#,
    )
    .bind(organization_id)
    .bind(caller.actor_id)
    .bind(caller.connection_id)
    .bind(caller.confined_project)
    .fetch_all(&mut *connection)
    .await?;
    templates.extend(
        rows.into_iter()
            .map(|(id, title, project_name)| TemplateSummary {
                id: id.to_string(),
                title,
                built_in: false,
                project_name,
            }),
    );
    Ok(templates)
}

/// A new board at `placement` from the template `reference` names (a
/// built-in's name, or a template board's ID or link), titled `title` or
/// after the template.
pub async fn create_from(
    transaction: &mut Transaction<'_, Postgres>,
    caller: &Caller,
    reference: &str,
    placement: Placement,
    title: Option<&str>,
) -> Result<Uuid, EditError> {
    let reference = reference.trim();
    if let Some(built_in) = BUILT_IN
        .iter()
        .find(|built_in| built_in.id.eq_ignore_ascii_case(reference))
    {
        let scene = scene(built_in).ok_or(AppError::NotFound("template"))?;
        let title = normalize_title(title.unwrap_or(&scene.title))?;
        let id = create(transaction, caller, placement, &title).await?;
        edit::apply(transaction, caller, id, &scene.operations, None, false).await?;
        return Ok(id);
    }
    let template_id = board_id_in(reference).ok_or(AppError::NotFound("template"))?;
    let source = access(transaction, caller, template_id, false)
        .await
        .map_err(|_| AppError::NotFound("template"))?;
    let (is_template, original) = sqlx::query_as::<_, (bool, String)>(
        "SELECT is_template, title FROM whiteboards WHERE id = $1",
    )
    .bind(template_id)
    .fetch_one(&mut **transaction)
    .await?;
    let organization_id = match placement {
        Placement::Workspace(organization_id) => Some(organization_id),
        Placement::Project(project_id) => {
            sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
                .bind(project_id)
                .fetch_optional(&mut **transaction)
                .await?
        }
    };
    if !is_template || source.deleted || organization_id != Some(source.organization_id) {
        return Err(AppError::NotFound("template").into());
    }
    let title = normalize_title(title.unwrap_or(&original))?;
    Ok(copy_into(transaction, caller, template_id, placement, &title).await?)
}

/// A board's ID, or the ID at the end of a link to it.
fn board_id_in(value: &str) -> Option<Uuid> {
    if value.len() > 2048 {
        return None;
    }
    value
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .and_then(|segment| segment.split(['?', '#']).next())
        .and_then(|segment| segment.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::{BUILT_IN, scene};
    use crate::whiteboards::scene::Scene;

    #[test]
    fn built_in_templates_draw() {
        for built_in in BUILT_IN {
            let template = scene(built_in).unwrap_or_else(|| panic!("{} parses", built_in.id));
            let mut drawing = Scene::new(Vec::new(), 1);
            drawing
                .apply(&template.operations)
                .unwrap_or_else(|error| panic!("{}: {error:?}", built_in.id));
        }
    }
}
