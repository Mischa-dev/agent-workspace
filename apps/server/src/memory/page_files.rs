//! A file kept on a page (C1.33): `page_file_upload` stores it with the page
//! and answers the block that shows it. The browser sends large files as
//! raw bytes to `/api/v1/things/page/{id}/files`, which stores them the
//! same way (`files::thing_destination`, `files::store_upload`).

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    error::AppError,
    files::{self, Destination, Purpose, ThingKind, Upload},
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Target, Targeted, refs::IdRef},
};

/// The most one call carries, decoded: the rest of the request is the
/// base64 around it.
pub const MAX_BYTES: usize = 4 * 1024 * 1024;

/// Keep a file on a page.
pub struct PageFileUpload;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageFileUploadInput {
    /// The page: its id or link.
    pub page_id: IdRef,
    /// The file name people see, such as plan.pdf.
    pub name: String,
    /// The MIME type; left out, the name's extension decides. Pictures are
    /// recognized from their bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// The content as text, for text files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Or the bytes, base64: up to 4 MiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
}

impl Targeted for PageFileUploadInput {
    fn target(&self) -> Target {
        Target::thing("page", self.page_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PageFileUploaded {
    #[schemars(with = "String")]
    pub file_id: Uuid,
    pub name: String,
    pub content_type: String,
    pub size: i64,
    /// The block that shows it, for page_update or page_update_blocks.
    pub embed: String,
}

impl Operation for PageFileUpload {
    const NAME: &'static str = "page_file_upload";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Perm> = Some(Perm::PagesEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const MAX_BODY_BYTES: usize = 6 * 1024 * 1024;
    const TITLE: &'static str = "Keep a file on a page";
    const DESCRIPTION: &'static str = "Keep a file on a page (text, or base64 up to 4 MiB). It \
        answers the block that shows it in the page; add that with page_update or page_update_blocks.";
    /// Staged by the files table's trigger as the row goes in.
    const EMITS: &'static [&'static str] = &["file.attached"];

    type Input = PageFileUploadInput;
    type Output = PageFileUploaded;

    async fn run(
        call: &mut OpCall<'_>,
        input: PageFileUploadInput,
    ) -> Result<PageFileUploaded, OpError> {
        let bytes = match (input.text, input.data_base64) {
            (Some(text), None) => text.into_bytes(),
            (None, Some(data)) => {
                if data.len() > MAX_BYTES * 4 / 3 + 8 {
                    return Err(OpError::TooLarge {
                        limit_bytes: MAX_BYTES,
                    });
                }
                STANDARD
                    .decode(data.trim())
                    .map_err(|_| OpError::invalid("dataBase64", "That isn't valid base64."))?
            }
            _ => return Err(OpError::invalid("text", "Pass either text or dataBase64.")),
        };
        if input.name.trim().is_empty() {
            return Err(OpError::invalid("name", "Give the file a name."));
        }
        let page_id = input.page_id.id();
        let uploader = call.account_id();
        let authorized = call
            .ctx
            .space_id
            .ok_or_else(|| OpError::not_found("That page"))?;
        if bytes.len() > MAX_BYTES {
            return Err(OpError::TooLarge {
                limit_bytes: MAX_BYTES,
            });
        }
        // The bytes go to the store before any transaction opens; a refusal
        // below takes them away again.
        let prepared = files::prepare_upload(Upload {
            name: input.name,
            declared_type: input.content_type,
            bytes,
        })
        .await?;
        let state = call.state;
        let destination = Destination::Thing {
            project_id: authorized,
            thing: ThingKind::Page,
            thing_id: page_id,
            purpose: Purpose::Inline,
        };
        let staged = files::stage_prepared(state, destination, prepared).await?;
        let size = staged.incoming();
        let checked = async {
            let tx = call.tx().await?;
            // The page's row holds it open while the file joins it.
            let found = sqlx::query_as::<_, (Uuid, bool)>(
                "SELECT project_id, archived_at IS NULL FROM project_pages WHERE id = $1 FOR SHARE",
            )
            .bind(page_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(AppError::from)?;
            let Some((space_id, open)) = found else {
                return Err(OpError::not_found("That page"));
            };
            // The call was allowed in the page's space as it was resolved; a
            // move that landed since then leaves it with no say over the page.
            if authorized != space_id {
                return Err(OpError::not_found("That page"));
            }
            if !open {
                return Err(OpError::invalid(
                    "pageId",
                    "That page is archived. Bring it back first.",
                ));
            }
            let limit = files::upload_limit(tx, space_id).await?.min(MAX_BYTES);
            if usize::try_from(size).unwrap_or(usize::MAX) > limit {
                return Err(OpError::TooLarge { limit_bytes: limit });
            }
            Ok(())
        }
        .await;
        if let Err(error) = checked {
            files::abandon_staged(state, &staged).await;
            return Err(error);
        }
        let tx = call.tx().await?;
        let row = files::store_prepared(state, tx, uploader, destination, staged).await?;
        call.audit_target("page", page_id, json!({"file": row.id}));
        let view = row.view();
        Ok(PageFileUploaded {
            file_id: view.id,
            embed: format!(
                "[{}](/files/{} \"embed\")",
                view.name.replace(['[', ']'], ""),
                view.id
            ),
            name: view.name,
            content_type: view.content_type,
            size: view.size,
        })
    }
}
