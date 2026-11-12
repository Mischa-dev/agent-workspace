//! A page out to a file and a file in as a page (AWD-53 criterion 9).
//!
//! - `GET /api/v1/pages/{id}/export?format=markdown|docx`: the page as a
//!   download. Markdown is the page's own text under its title; .docx is
//!   [`super::docx::export`].
//! - `POST /api/v1/projects/{id}/pages/import?name=…`: a .docx or Markdown
//!   file (the raw bytes as the body) as a new page. The answer names what
//!   the file held that a page can't, which was left out.
//!
//! `docs/frontend/page-import-export.md` lists how each block maps.

use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{docx, tree, write};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::parse_actor_kind,
    error::{ApiPath, ApiQuery, AppError},
    files::http::{content_disposition, read_body},
    memory::{
        self, MAX_PAGE_BODY, PageInput, PageView, audit, browser_writer, create_page, get_page,
        page_project, project_access,
    },
    policy::Action,
};

/// The largest file a page import takes. A .docx carries its images, which
/// are left out, so this is well above what its text could be.
pub const MAX_IMPORT_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportQuery {
    pub format: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportQuery {
    /// The file's name; its extension says what it is when `format` doesn't.
    pub name: Option<String>,
    /// `docx` or `markdown`.
    pub format: Option<String>,
    /// The new page's title; otherwise the document's own, or the file's
    /// name.
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportView {
    pub page: PageView,
    /// What the file held that a page can't, left out ("images and
    /// drawings", "footnotes").
    pub set_aside: Vec<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Markdown,
    Docx,
}

fn format_of(format: Option<&str>, name: Option<&str>) -> Option<Format> {
    let word = match format {
        Some(format) => format.to_ascii_lowercase(),
        None => name?.rsplit_once('.')?.1.to_ascii_lowercase(),
    };
    match word.as_str() {
        "md" | "markdown" | "txt" => Some(Format::Markdown),
        "docx" => Some(Format::Docx),
        _ => None,
    }
}

pub async fn export_page(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(page_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<ExportQuery>,
) -> Result<Response, AppError> {
    let format = format_of(Some(query.format.as_deref().unwrap_or("markdown")), None)
        .ok_or(AppError::Validation("format"))?;
    let project_id = page_project(&state.pool, page_id).await?;
    project_access(
        &state.pool,
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        project_id,
        Action::ProjectView,
    )
    .await
    .map_err(|_| AppError::NotFound("page"))?;
    let page = get_page(&state.pool, page_id, principal.actor_id)
        .await?
        .ok_or(AppError::NotFound("page"))?;
    let (bytes, content_type, extension) = match format {
        Format::Markdown => (
            markdown_file(&page.title, &page.body).into_bytes(),
            "text/markdown; charset=utf-8",
            "md",
        ),
        Format::Docx => (
            docx::export(
                &page.title,
                &tree::from_markdown(&page.body),
                Some(&crate::invitations::public_origin(&state)),
            ),
            docx::CONTENT_TYPE,
            "docx",
        ),
    };
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&content_disposition(
            &format!("{}.{extension}", file_stem(&page.title)),
            true,
        ))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub async fn import_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<ImportQuery>,
    body: Body,
) -> Result<Json<ImportView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let writer = browser_writer(&principal)?;
    let format =
        format_of(query.format.as_deref(), query.name.as_deref()).ok_or(AppError::Refused {
            field: "file",
            message: "Import a Word document (.docx) or a Markdown file (.md).",
        })?;
    // Check access before reading the file.
    let direct = project_access(
        &state.pool,
        writer.actor_id,
        writer.actor_kind,
        project_id,
        Action::PageEdit,
    )
    .await?;
    if direct {
        return Err(AppError::Validation("project"));
    }
    let bytes = read_body(body, MAX_IMPORT_BYTES).await?;
    let (title, body, set_aside) = match format {
        Format::Markdown => {
            let text = String::from_utf8(bytes).map_err(|_| AppError::Refused {
                field: "file",
                message: "This file isn't text. Import a Markdown file saved as UTF-8.",
            })?;
            let (title, body) = split_title(text.trim_start_matches('\u{feff}'));
            (title, body, Vec::new())
        }
        Format::Docx => {
            let origin = crate::invitations::public_origin(&state);
            let imported = docx::import(&bytes, Some(&origin)).map_err(|error| match error {
                docx::ImportError::NotDocx => AppError::Refused {
                    field: "file",
                    message: "This file can't be read as a Word document. Save it as .docx and try again.",
                },
                docx::ImportError::TooBig => AppError::PayloadTooLarge,
            })?;
            (
                imported.title,
                write::write_document(&imported.blocks).markdown,
                imported.set_aside,
            )
        }
    };
    if body.chars().count() > MAX_PAGE_BODY {
        return Err(AppError::Refused {
            field: "body",
            message: "This document is too long for one page. Split it, and import each part.",
        });
    }
    let title = query
        .title
        .filter(|title| !title.trim().is_empty())
        .or(title.filter(|title| !title.trim().is_empty()))
        .or_else(|| {
            query
                .name
                .as_deref()
                .map(|name| name.rsplit_once('.').map_or(name, |(stem, _)| stem))
                .map(str::to_owned)
                .filter(|stem| !stem.trim().is_empty())
        })
        .unwrap_or_else(|| "Imported page".to_owned());
    // A title too long for a page keeps its start.
    let title: String = title
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(memory::MAX_PAGE_TITLE)
        .collect();
    let mut transaction = state.pool.begin().await?;
    let title = free_title(&mut transaction, project_id, &title).await?;
    let page_id = create_page(
        &mut transaction,
        writer,
        project_id,
        &PageInput {
            title: Some(title),
            body: Some(body),
            is_guide: None,
            skill_description: None,
            expected_version: None,
            restore_version: None,
            archived: None,
            note: Some(match format {
                Format::Markdown => "Imported from Markdown".to_owned(),
                Format::Docx => "Imported from a Word document".to_owned(),
            }),
        },
    )
    .await?;
    audit(
        &mut transaction,
        &principal,
        project_id,
        "page.created",
        "page",
        page_id,
    )
    .await?;
    transaction.commit().await?;
    let page = get_page(&state.pool, page_id, principal.actor_id)
        .await?
        .ok_or(AppError::NotFound("page"))?;
    Ok(Json(ImportView { page, set_aside }))
}

/// `title`, or `title (2)` and on when another page has it: an import
/// never fails over a name.
async fn free_title(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project_id: Uuid,
    title: &str,
) -> Result<String, AppError> {
    let base: String = title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(memory::MAX_PAGE_TITLE - 6)
        .collect();
    let taken: std::collections::HashSet<String> = sqlx::query_scalar::<_, String>(
        r#"
        SELECT lower(title) FROM project_pages
        WHERE project_id = $1 AND archived_at IS NULL
          AND left(lower(title), char_length($2)) = lower($2)
        "#,
    )
    .bind(project_id)
    .bind(&base)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .collect();
    Ok((1..=1000)
        .map(|number| match number {
            1 => base.clone(),
            number => format!("{base} ({number})"),
        })
        .find(|candidate| !taken.contains(&candidate.to_lowercase()))
        .unwrap_or(base))
}

/// A page as a Markdown file: its title as the top heading, then its text.
fn markdown_file(title: &str, body: &str) -> String {
    let mut file = format!("# {}\n", title.trim());
    if !body.trim().is_empty() {
        file.push('\n');
        file.push_str(body.trim_end());
        file.push('\n');
    }
    file
}

/// A Markdown file's top heading as the page title, and the rest as its
/// text; the file as it is when it doesn't start with one.
fn split_title(text: &str) -> (Option<String>, String) {
    let start = text.trim_start_matches(['\n', '\r']);
    let (line, rest) = start.split_once('\n').unwrap_or((start, ""));
    match line.trim_end().strip_prefix("# ") {
        Some(title) if !title.trim().is_empty() => (
            Some(title.trim().trim_end_matches('#').trim_end().to_owned()),
            rest.trim_start_matches(['\n', '\r']).trim_end().to_owned(),
        ),
        _ => (None, text.trim_end().to_owned()),
    }
}

/// A file name from a page's title.
fn file_stem(title: &str) -> String {
    let stem: String = title
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
            {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let stem: String = stem.chars().take(120).collect();
    if stem.is_empty() {
        "page".to_owned()
    } else {
        stem
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_markdown_file_keeps_its_title_both_ways() {
        let file = markdown_file("Launch plan", "## Goals\n\n- one");
        assert_eq!(file, "# Launch plan\n\n## Goals\n\n- one\n");
        assert_eq!(
            split_title(&file),
            (
                Some("Launch plan".to_owned()),
                "## Goals\n\n- one".to_owned()
            )
        );
        assert_eq!(
            split_title("## Only\n\ntext"),
            (None, "## Only\n\ntext".to_owned())
        );
        assert_eq!(split_title("# \n\ntext").0, None);
    }

    #[test]
    fn formats_come_from_the_name_or_the_query() {
        assert_eq!(format_of(None, Some("Notes.DOCX")), Some(Format::Docx));
        assert_eq!(format_of(None, Some("notes.md")), Some(Format::Markdown));
        assert_eq!(
            format_of(Some("markdown"), Some("x.docx")),
            Some(Format::Markdown)
        );
        assert_eq!(format_of(None, Some("notes.pdf")), None);
        assert_eq!(format_of(None, None), None);
    }

    #[test]
    fn file_names_drop_what_file_systems_refuse() {
        assert_eq!(file_stem("Plan: Q3 / Q4?"), "Plan Q3 Q4");
        assert_eq!(file_stem("  "), "page");
    }
}
