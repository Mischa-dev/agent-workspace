//! A page out to a file and a file in as a page (AWD-53 criterion 9).
//!
//! - `GET /api/v1/pages/{id}/export?format=markdown|docx|pdf`: the page as a
//!   download. Markdown is the page's own text under its title; .docx is
//!   [`super::docx::export`]; PDF is [`super::pdf::export`], with the boards
//!   and sheet ranges it embeds drawn in when the reader may see them.
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

use std::collections::HashMap;

use super::{docx, pdf, tree, write};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
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
    /// Out only: a PDF can't come back in as a page.
    Pdf,
}

fn format_of(format: Option<&str>, name: Option<&str>) -> Option<Format> {
    let word = match format {
        Some(format) => format.to_ascii_lowercase(),
        None => name?.rsplit_once('.')?.1.to_ascii_lowercase(),
    };
    match word.as_str() {
        "md" | "markdown" | "txt" => Some(Format::Markdown),
        "docx" => Some(Format::Docx),
        "pdf" => Some(Format::Pdf),
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
    let project_id = page_project(&state.pool, page_id, principal.actor_id).await?;
    project_access(
        &state.pool,
        principal.actor_id,
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
        Format::Pdf => {
            // Drawing boards is the costly part: counted as an expensive read.
            state
                .rate_limits
                .check_expensive_read(principal.session_id)
                .await?;
            let blocks = tree::from_markdown(&page.body);
            let embeds = pdf_embeds(&state, &principal, &blocks).await?;
            (
                pdf::export(
                    &page.title,
                    &blocks,
                    Some(&crate::invitations::public_origin(&state)),
                    &embeds,
                ),
                pdf::CONTENT_TYPE,
                "pdf",
            )
        }
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
    require_authenticated_write(&state, &headers, &principal).await?;
    let writer = browser_writer(&principal)?;
    let format = format_of(query.format.as_deref(), query.name.as_deref())
        .filter(|format| *format != Format::Pdf)
        .ok_or(AppError::Refused {
            field: "file",
            message: "Import a Word document (.docx) or a Markdown file (.md).",
        })?;
    // Check access before reading the file.
    let direct = project_access(&state.pool, writer.actor_id, project_id, Action::PageEdit).await?;
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
        Format::Pdf => return Err(AppError::Validation("format")),
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
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let title = memory::free_page_title(&mut transaction, project_id, &title).await?;
    let page_id = create_page(
        &mut transaction,
        writer,
        project_id,
        &PageInput {
            title: Some(title),
            body: Some(body),
            note: Some(match format {
                Format::Markdown => "Imported from Markdown".to_owned(),
                Format::Docx | Format::Pdf => "Imported from a Word document".to_owned(),
            }),
            ..PageInput::default()
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
            Some(without_closing_hashes(title.trim()).to_owned()),
            rest.trim_start_matches(['\n', '\r']).trim_end().to_owned(),
        ),
        _ => (None, text.trim_end().to_owned()),
    }
}

/// A heading's text without its closing `#`s, which count only after a
/// space (`# C#` is "C#").
fn without_closing_hashes(text: &str) -> &str {
    let open = text.trim_end_matches('#');
    if open.len() < text.len() && (open.is_empty() || open.ends_with([' ', '\t'])) {
        open.trim_end()
    } else {
        text
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

/// The most embeds a PDF draws in; the rest show as their links.
const MAX_PDF_EMBEDS: usize = 24;
/// A sheet range in a PDF shows at most this much of the range.
const MAX_PDF_SHEET_ROWS: i32 = 60;
const MAX_PDF_SHEET_COLUMNS: i32 = 12;
/// A board in a PDF is drawn at most this many pixels on its longer side:
/// sharp at the page's width, and a bounded picture whatever the board.
const MAX_PDF_BOARD_SIDE: u32 = 1600;
/// The pixels all of a PDF's boards may hold together (about 32 MiB as
/// RGBA); boards past it show as their links.
const MAX_PDF_BOARD_PIXELS: u64 = 8_000_000;

/// The pictures for a PDF's embeds: a board's drawing, a sheet range's
/// values and a picture of the page (C1.33), each only when this reader may
/// open it there. Anything else, or
/// anything they can't see, stays its label and link.
async fn pdf_embeds(
    state: &AppState,
    principal: &AuthenticatedActor,
    blocks: &[tree::Block],
) -> Result<HashMap<String, pdf::EmbedPicture>, AppError> {
    let mut pictures = HashMap::new();
    let mut board_pixels = 0_u64;
    let hrefs = blocks
        .iter()
        .filter_map(|block| match &block.node {
            tree::Node::Embed { href, .. } => Some(href.clone()),
            _ => None,
        })
        .take(MAX_PDF_EMBEDS)
        .collect::<Vec<_>>();
    for href in hrefs {
        if pictures.contains_key(&href) {
            continue;
        }
        let picture = match embed_target(&href) {
            Some(EmbedTarget::Board(_) | EmbedTarget::File(_))
                if board_pixels >= MAX_PDF_BOARD_PIXELS =>
            {
                None
            }
            Some(EmbedTarget::File(id)) => {
                let picture = file_picture(state, principal, id).await?;
                if let Some(pdf::EmbedPicture::Image { width, height, .. }) = &picture {
                    board_pixels += u64::from(*width) * u64::from(*height);
                }
                picture.filter(|_| board_pixels <= MAX_PDF_BOARD_PIXELS)
            }
            Some(EmbedTarget::Board(id)) => {
                let picture = board_picture(state, principal, id).await?;
                if let Some(pdf::EmbedPicture::Image { width, height, .. }) = &picture {
                    board_pixels += u64::from(*width) * u64::from(*height);
                }
                picture.filter(|_| board_pixels <= MAX_PDF_BOARD_PIXELS)
            }
            Some(EmbedTarget::Sheet {
                sheet_id,
                chart: Some(chart),
                ..
            }) => {
                if board_pixels >= MAX_PDF_BOARD_PIXELS {
                    None
                } else {
                    let picture = chart_picture(state, principal, sheet_id, chart).await?;
                    if let Some(pdf::EmbedPicture::Image { width, height, .. }) = &picture {
                        board_pixels += u64::from(*width) * u64::from(*height);
                    }
                    picture.filter(|_| board_pixels <= MAX_PDF_BOARD_PIXELS)
                }
            }
            Some(EmbedTarget::Sheet {
                project_id,
                sheet_id,
                tab_id,
                range,
                name,
                chart: None,
            }) => {
                sheet_picture(state, principal, project_id, sheet_id, tab_id, range, name).await?
            }
            None => None,
        };
        if let Some(picture) = picture {
            pictures.insert(href, picture);
        }
    }
    Ok(pictures)
}

#[derive(Debug, PartialEq, Eq)]
enum EmbedTarget {
    Board(Uuid),
    /// A file of the page, such as a pasted picture: `/files/{id}`.
    File(Uuid),
    Sheet {
        project_id: Uuid,
        sheet_id: Uuid,
        tab_id: Option<Uuid>,
        range: Option<String>,
        /// A named range (C1.20), kept by its name so it follows the range.
        name: Option<String>,
        /// One of its charts (C1.18), drawn as the page shows it.
        chart: Option<Uuid>,
    },
}

/// What an embed's same-origin path names, as the web's `embedTarget` reads
/// it: a sheet at `/w/{workspace}/s/{space}/sheets/{s}?tab=&range=` (or the
/// older `/projects/{p}/sheets/{s}`), or a board at `…/whiteboards/{id}`.
fn embed_target(href: &str) -> Option<EmbedTarget> {
    let base = url::Url::parse("https://app.invalid/").ok()?;
    let url = base.join(href).ok()?;
    if url.origin() != base.origin() {
        return None;
    }
    let segments = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    match segments.as_slice() {
        ["projects", project, "sheets", sheet] | ["w", _, "s", project, "sheets", sheet] => {
            let query = |name: &str| {
                url.query_pairs()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| value.into_owned())
                    .filter(|value| !value.is_empty())
            };
            Some(EmbedTarget::Sheet {
                project_id: project.parse().ok()?,
                sheet_id: sheet.parse().ok()?,
                tab_id: query("tab").and_then(|tab| tab.parse().ok()),
                range: query("range"),
                name: query("name"),
                chart: query("chart").and_then(|chart| chart.parse().ok()),
            })
        }
        ["files", file] => file.parse().ok().map(EmbedTarget::File),
        [.., "whiteboards", board] => board.parse().ok().map(EmbedTarget::Board),
        _ => None,
    }
}

/// The most of a picture's bytes read for a PDF.
const MAX_PDF_PICTURE_BYTES: u64 = 30 * 1024 * 1024;

/// A picture of the page as pixels, when this reader may see the file and
/// it is one that shows inline. Anything else stays its label.
async fn file_picture(
    state: &AppState,
    principal: &AuthenticatedActor,
    file_id: Uuid,
) -> Result<Option<pdf::EmbedPicture>, AppError> {
    let row = match crate::files::readable_file(
        &state.pool,
        crate::files::Reader::Browser {
            actor_id: principal.actor_id,
        },
        file_id,
    )
    .await
    {
        Ok(row) => row,
        Err(AppError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    let (Some(width), Some(height)) = (row.image_width, row.image_height) else {
        return Ok(None);
    };
    let bytes = crate::files::FileStore::from_config(&state.config)
        .read_all(&row.storage_key, MAX_PDF_PICTURE_BYTES)
        .await?;
    use base64::Engine as _;
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"><image width="{width}" height="{height}" href="data:{};base64,{}"/></svg>"#,
        row.content_type,
        base64::engine::general_purpose::STANDARD.encode(&bytes),
    );
    Ok(
        crate::whiteboards::render::pixels_within(&svg, 1.0, MAX_PDF_BOARD_SIDE)
            .ok()
            .map(|(rgba, width, height)| pdf::EmbedPicture::Image {
                rgba,
                width,
                height,
            }),
    )
}

async fn board_picture(
    state: &AppState,
    principal: &AuthenticatedActor,
    board_id: Uuid,
) -> Result<Option<pdf::EmbedPicture>, AppError> {
    let caller = crate::whiteboards::Caller::browser(principal)?;
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    match crate::whiteboards::access(&mut connection, &caller, board_id, false).await {
        Ok(board) if !board.deleted => {}
        Ok(_) | Err(AppError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    }
    drop(connection);
    let elements = crate::whiteboards::store::live_elements(&state.pool, board_id).await?;
    if elements.is_empty() {
        return Ok(None);
    }
    let files = crate::whiteboards::store::files_for(state, board_id, &elements).await?;
    let rendered = crate::whiteboards::render::svg(
        &elements,
        &files,
        &crate::whiteboards::render::Options {
            only: None,
            background: true,
            dark: false,
        },
    );
    Ok(
        crate::whiteboards::render::pixels_within(&rendered.svg, 1.5, MAX_PDF_BOARD_SIDE)
            .ok()
            .map(|(rgba, width, height)| pdf::EmbedPicture::Image {
                rgba,
                width,
                height,
            }),
    )
}

/// A sheet's chart as a picture, drawn from its cells as they are now; one
/// the reader can't see, or that's gone, shows the embed as a link.
async fn chart_picture(
    state: &AppState,
    principal: &AuthenticatedActor,
    sheet_id: Uuid,
    chart_id: Uuid,
) -> Result<Option<pdf::EmbedPicture>, AppError> {
    use crate::sheets::charts;
    let figured = crate::db::scoped_one(&state.pool, async |connection| {
        let Some((chart, found_sheet, sheet_name)) =
            charts::find(connection, chart_id, principal.actor_id).await?
        else {
            return Ok::<_, AppError>(None);
        };
        if found_sheet != sheet_id {
            return Ok(None);
        }
        let figures = charts::figures(connection, &chart, found_sheet, sheet_name).await?;
        Ok(Some((chart, figures)))
    })
    .await?;
    let Some((chart, figures)) = figured else {
        return Ok(None);
    };
    let svg = charts::svg(&chart.title, &chart.style, figures.as_ref());
    Ok(
        crate::whiteboards::render::pixels_within(&svg, 1.5, MAX_PDF_BOARD_SIDE)
            .ok()
            .map(|(rgba, width, height)| pdf::EmbedPicture::Image {
                rgba,
                width,
                height,
            }),
    )
}

async fn sheet_picture(
    state: &AppState,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    sheet_id: Uuid,
    tab_id: Option<Uuid>,
    range: Option<String>,
    name: Option<String>,
) -> Result<Option<pdf::EmbedPicture>, AppError> {
    use crate::sheets;
    if project_access(
        &state.pool,
        principal.actor_id,
        project_id,
        Action::ProjectView,
    )
    .await
    .is_err()
    {
        return Ok(None);
    }
    let Some(sheet) = sheets::find(&state.pool, project_id, &sheet_id.to_string()).await? else {
        return Ok(None);
    };
    // A named range is read where it is now; a name that's gone shows the
    // embed as a link.
    let (tab_id, block) = match name.as_deref() {
        Some(name) => {
            let located = crate::db::scoped_one(&state.pool, async |connection| {
                sheets::ranges::resolve(connection, sheet.id, name).await
            })
            .await?;
            match located.filter(|located| located.name.is_some()) {
                Some(located) => (Some(located.tab_id), Some(located.range)),
                None => return Ok(None),
            }
        }
        None => (
            tab_id,
            range.as_deref().and_then(|range| {
                sheets::address::range(range, sheets::MAX_ROWS, sheets::MAX_COLUMNS)
            }),
        ),
    };
    let tabs = sheets::tabs(&state.pool, sheet.id).await?;
    let Some(tab) = tab_id
        .and_then(|id| tabs.iter().find(|tab| tab.id == id))
        .or_else(|| tabs.first())
    else {
        return Ok(None);
    };
    let (rows, columns) = match block {
        Some(range) => (range.rows, range.columns),
        None => {
            let (rows, columns) = sheets::used_extent(&state.pool, tab.id).await?;
            if rows == 0 || columns == 0 {
                return Ok(None);
            }
            ((1, rows), (1, columns))
        }
    };
    let rows = (rows.0, rows.1.min(rows.0 + MAX_PDF_SHEET_ROWS - 1));
    let columns = (
        columns.0,
        columns.1.min(columns.0 + MAX_PDF_SHEET_COLUMNS - 1),
    );
    let cells = sheets::cells_in(
        &state.pool,
        tab.id,
        rows,
        columns,
        i64::from(MAX_PDF_SHEET_ROWS * MAX_PDF_SHEET_COLUMNS),
        true,
    )
    .await?;
    let width = usize::try_from(columns.1 - columns.0 + 1).unwrap_or(0);
    let height = usize::try_from(rows.1 - rows.0 + 1).unwrap_or(0);
    let mut grid = vec![vec![String::new(); width]; height];
    for cell in &cells {
        let (Ok(row), Ok(column)) = (
            usize::try_from(cell.row_number - rows.0),
            usize::try_from(cell.column_number - columns.0),
        ) else {
            continue;
        };
        if let Some(slot) = grid.get_mut(row).and_then(|line| line.get_mut(column)) {
            *slot = cell
                .display
                .clone()
                .unwrap_or_else(|| sheets::text_of(&cell.value()));
        }
    }
    Ok(Some(pdf::EmbedPicture::Table(grid)))
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
        assert_eq!(
            split_title(&markdown_file("C#", "x")).0.as_deref(),
            Some("C#")
        );
        assert_eq!(split_title("# Plan ##\n").0.as_deref(), Some("Plan"));
    }

    #[test]
    fn formats_come_from_the_name_or_the_query() {
        assert_eq!(format_of(None, Some("Notes.DOCX")), Some(Format::Docx));
        assert_eq!(format_of(None, Some("notes.md")), Some(Format::Markdown));
        assert_eq!(
            format_of(Some("markdown"), Some("x.docx")),
            Some(Format::Markdown)
        );
        assert_eq!(format_of(Some("PDF"), None), Some(Format::Pdf));
        assert_eq!(format_of(None, Some("notes.odt")), None);
        assert_eq!(format_of(None, None), None);
    }

    #[test]
    fn embeds_name_boards_and_sheet_ranges() {
        let id = Uuid::nil();
        assert_eq!(
            embed_target(&format!("/whiteboards/{id}")),
            Some(EmbedTarget::Board(id))
        );
        assert_eq!(
            embed_target(&format!("/projects/{id}/sheets/{id}?range=A1:B2")),
            Some(EmbedTarget::Sheet {
                project_id: id,
                sheet_id: id,
                tab_id: None,
                range: Some("A1:B2".to_owned()),
                name: None,
                chart: None,
            })
        );
        assert_eq!(
            embed_target(&format!("/projects/{id}/sheets/{id}?name=Totals")),
            Some(EmbedTarget::Sheet {
                project_id: id,
                sheet_id: id,
                tab_id: None,
                range: None,
                name: Some("Totals".to_owned()),
                chart: None,
            })
        );
        assert_eq!(
            embed_target(&format!("/projects/{id}/sheets/{id}?chart={id}")),
            Some(EmbedTarget::Sheet {
                project_id: id,
                sheet_id: id,
                tab_id: None,
                range: None,
                name: None,
                chart: Some(id),
            })
        );
        assert_eq!(
            embed_target(&format!("https://elsewhere.test/whiteboards/{id}")),
            None
        );
        assert_eq!(
            embed_target(&format!("/w/acme/s/{id}/sheets/{id}?tab={id}")),
            Some(EmbedTarget::Sheet {
                project_id: id,
                sheet_id: id,
                tab_id: Some(id),
                range: None,
                name: None,
                chart: None,
            })
        );
        assert_eq!(
            embed_target(&format!("/w/acme/s/{id}/whiteboards/{id}")),
            Some(EmbedTarget::Board(id))
        );
        assert_eq!(embed_target("/projects/x/collections/y"), None);
    }

    #[test]
    fn file_names_drop_what_file_systems_refuse() {
        assert_eq!(file_stem("Plan: Q3 / Q4?"), "Plan Q3 Q4");
        assert_eq!(file_stem("  "), "page");
    }
}
