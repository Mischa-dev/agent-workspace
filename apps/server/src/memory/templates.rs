//! Page templates (C1.24). A template is a page marked `is_template`, or one
//! of the built-in ones that ship as Markdown under
//! `src/modules/templates/pages/`. "New page from template" copies the
//! template's title and text with `{{date}}`, `{{author}}` and `{{project}}`
//! filled in for the person (or agent) making the page.
//!
//! A page template from any project the writer can see in the same workspace
//! can be used; anything else is "not found", as if it weren't there.

use rmcp::schemars::{self, JsonSchema};
use serde::Serialize;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use super::{Writer, project_access};
use crate::{error::AppError, policy::Action};

/// A built-in template: its id (what `fromTemplateId` takes) and Markdown,
/// whose top heading is the new page's title.
struct BuiltIn {
    id: &'static str,
    markdown: &'static str,
}

const BUILT_IN: &[BuiltIn] = &[
    BuiltIn {
        id: "meeting-notes",
        markdown: include_str!("../modules/templates/pages/meeting-notes.md"),
    },
    BuiltIn {
        id: "decision",
        markdown: include_str!("../modules/templates/pages/decision.md"),
    },
    BuiltIn {
        id: "retrospective",
        markdown: include_str!("../modules/templates/pages/retrospective.md"),
    },
    BuiltIn {
        id: "project-brief",
        markdown: include_str!("../modules/templates/pages/project-brief.md"),
    },
];

/// The built-in templates' ids, for tool descriptions and errors.
pub const BUILT_IN_IDS: &str = "meeting-notes, decision, retrospective, project-brief";

/// A template as the gallery lists it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "PageTemplateView")]
pub struct TemplateSummary {
    /// A page's ID, or a built-in template's name.
    pub id: String,
    pub title: String,
    pub excerpt: String,
    pub built_in: bool,
    /// The project a page template lives in; none for built-in ones.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
}

/// A template's title and text, with its variables still in.
struct Source {
    title: String,
    body: String,
}

/// A template's title and text filled in for one new page.
pub struct Filled {
    pub title: String,
    pub body: String,
}

fn split(markdown: &str) -> Source {
    let text = markdown.trim_start();
    match text.strip_prefix("# ") {
        Some(rest) => {
            let (title, body) = rest.split_once('\n').unwrap_or((rest, ""));
            Source {
                title: title.trim().to_owned(),
                body: body.trim_start_matches('\n').to_owned(),
            }
        }
        None => Source {
            title: "Untitled".to_owned(),
            body: text.to_owned(),
        },
    }
}

/// The templates a writer can start a page from in a project: the
/// built-in ones, then page templates in the projects they can see in that
/// project's workspace.
pub async fn list(
    transaction: &mut Transaction<'_, Postgres>,
    viewer: Uuid,
    reach: crate::drafts::Reach,
    project_id: Uuid,
) -> Result<Vec<TemplateSummary>, AppError> {
    let mut templates = BUILT_IN
        .iter()
        .map(|built_in| {
            let source = split(built_in.markdown);
            TemplateSummary {
                id: built_in.id.to_owned(),
                title: source.title,
                excerpt: excerpt(&source.body),
                built_in: true,
                project_name: None,
            }
        })
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, (Uuid, String, String, String)>(
        r#"
        SELECT page.id, page.title, left(regexp_replace(page.body, '\s+', ' ', 'g'), 240),
               project.name
        FROM project_pages page
        JOIN projects project ON project.id = page.project_id
        JOIN projects here ON here.id = $2
        WHERE page.is_template AND page.archived_at IS NULL
          AND project.organization_id = here.organization_id
          AND project.archived_at IS NULL
          AND actor_can_access_project($1, $3, project.id)
          AND ($4::uuid IS NULL OR project.id = $4)
          AND (page.memory_for_kind IS DISTINCT FROM 'conversation'
               OR page_visible_to(page.id, $1))
        ORDER BY page.project_id <> $2, lower(page.title), page.id
        LIMIT 200
        "#,
    )
    .bind(viewer)
    .bind(project_id)
    .bind(reach.connection_id)
    .bind(reach.confined_space)
    .fetch_all(&mut **transaction)
    .await?;
    templates.extend(
        rows.into_iter()
            .map(|(id, title, excerpt, project_name)| TemplateSummary {
                id: id.to_string(),
                title,
                excerpt,
                built_in: false,
                project_name: Some(project_name),
            }),
    );
    Ok(templates)
}

fn excerpt(body: &str) -> String {
    body.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(240)
        .collect()
}

/// The template `reference` names (a built-in's name, or a template page's
/// ID or link), filled in for `writer` making a page in `project_id`.
pub async fn fill(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    project_id: Uuid,
    reference: &str,
) -> Result<Filled, AppError> {
    let reference = reference.trim();
    let source = match BUILT_IN
        .iter()
        .find(|built_in| built_in.id.eq_ignore_ascii_case(reference))
    {
        Some(built_in) => split(built_in.markdown),
        None => {
            let id = page_id_in(reference).ok_or(AppError::NotFound("template"))?;
            template_page(transaction, writer, project_id, id).await?
        }
    };
    let (author, project) = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT COALESCE(membership.display_name, 'Someone'), project.name
        FROM projects project
        LEFT JOIN project_memberships membership
          ON membership.project_id = project.id AND membership.actor_id = $1
        WHERE project.id = $2
        "#,
    )
    .bind(writer.actor_id)
    .bind(project_id)
    .fetch_one(&mut **transaction)
    .await?;
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let variables = Variables {
        date: &date,
        author: &author,
        project: &project,
    };
    Ok(Filled {
        title: variables.apply(&source.title),
        body: variables.apply(&source.body),
    })
}

/// A template page the writer can see, in the workspace of the project the
/// new page goes in.
async fn template_page(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    project_id: Uuid,
    page_id: Uuid,
) -> Result<Source, AppError> {
    let row = sqlx::query_as::<_, (Uuid, String, String)>(
        r#"
        SELECT page.project_id, page.title, page.body
        FROM project_pages page
        JOIN projects project ON project.id = page.project_id
        JOIN projects here ON here.id = $2
        WHERE page.id = $1 AND page.is_template AND page.archived_at IS NULL
          AND project.organization_id = here.organization_id
          AND (page.memory_for_kind IS DISTINCT FROM 'conversation'
               OR page_visible_to(page.id, $3))
        "#,
    )
    .bind(page_id)
    .bind(project_id)
    .bind(writer.actor_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("template"))?;
    let (template_project, title, body) = row;
    if writer
        .confined_project
        .is_some_and(|confined| confined != template_project)
    {
        return Err(AppError::NotFound("template"));
    }
    project_access(
        &mut **transaction,
        writer.actor_id,
        template_project,
        Action::ProjectView,
    )
    .await
    .map_err(|_| AppError::NotFound("template"))?;
    Ok(Source { title, body })
}

/// A page's ID, or the ID at the end of a link to it.
fn page_id_in(value: &str) -> Option<Uuid> {
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

struct Variables<'a> {
    date: &'a str,
    author: &'a str,
    project: &'a str,
}

impl Variables<'_> {
    /// Fills `{{date}}`, `{{author}}` and `{{project}}` (spaces inside the
    /// braces allowed); anything else in braces is left as written.
    fn apply(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else {
                out.push_str(&rest[start..]);
                return out;
            };
            match after[..end].trim().to_ascii_lowercase().as_str() {
                "date" => out.push_str(self.date),
                "author" => out.push_str(self.author),
                "project" => out.push_str(self.project),
                _ => out.push_str(&rest[start..start + 2 + end + 2]),
            }
            rest = &after[end + 2..];
        }
        out.push_str(rest);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{BUILT_IN, Variables, split};

    #[test]
    fn variables_fill_and_strangers_stay() {
        let variables = Variables {
            date: "2026-11-15",
            author: "Lena",
            project: "Work",
        };
        assert_eq!(
            variables.apply("{{date}} by {{ Author }} in {{project}}: {{other}} {{"),
            "2026-11-15 by Lena in Work: {{other}} {{"
        );
    }

    #[test]
    fn built_in_templates_have_titles() {
        for built_in in BUILT_IN {
            let source = split(built_in.markdown);
            assert!(!source.title.is_empty(), "{}", built_in.id);
            assert!(!source.body.trim().is_empty(), "{}", built_in.id);
        }
    }
}
