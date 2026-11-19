//! Memory pages (A2.16): one page per space and per channel or thread
//! that holds what anyone working there should know first. It is an
//! ordinary page in the space, with every version kept, made on its first
//! write. A conversation's memory is as private as the conversation: only
//! its readers find, read or change it (`page_visible_to`), and it is
//! never listed with the space's pages.
//!
//! Agents meet it where they start: `conversation_open`, `project_open` and
//! `reply_claim` carry a pointer with its first lines.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use super::{PageInput, Writer};
use crate::{
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

/// How much of the page a pointer carries.
const HEAD_CHARS: usize = 1_500;

pub fn register_ops(registry: &mut Registry) {
    registry.add::<MemoryPageOpen>().add::<MemoryPageWrite>();
}

/// What a memory page is for.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryFor {
    Space,
    Conversation,
}

impl MemoryFor {
    const fn key(self) -> &'static str {
        match self {
            Self::Space => "space",
            Self::Conversation => "conversation",
        }
    }
}

/// A pointer to a memory page, as `conversation_open`, `project_open` and
/// `reply_claim` include it: read it before acting there.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPointer {
    #[schemars(with = "String")]
    pub page_id: Uuid,
    pub title: String,
    pub version: i64,
    pub updated_at: String,
    /// The page's first 1,500 characters.
    pub head: String,
    /// Whether there is more than the head.
    pub truncated: bool,
}

/// The memory page of a space or conversation, if it has one and `viewer`
/// may see it.
pub async fn pointer(
    connection: &mut PgConnection,
    memory_for: MemoryFor,
    id: Uuid,
    viewer: Uuid,
) -> Result<Option<MemoryPointer>, AppError> {
    let row = sqlx::query_as::<_, (Uuid, String, i64, String, String)>(
        r#"
        SELECT page.id, page.title, page.version, page.updated_at::text, page.body
        FROM project_pages page
        WHERE page.memory_for_kind = $1 AND page.memory_for_id = $2
          AND page.archived_at IS NULL
          AND page_visible_to(page.id, $3)
        "#,
    )
    .bind(memory_for.key())
    .bind(id)
    .bind(viewer)
    .fetch_optional(&mut *connection)
    .await?;
    Ok(row.map(|(page_id, title, version, updated_at, body)| {
        let truncated = body.chars().count() > HEAD_CHARS;
        MemoryPointer {
            page_id,
            title,
            version,
            updated_at,
            head: body.chars().take(HEAD_CHARS).collect(),
            truncated,
        }
    }))
}

/// [`pointer`] on its own connection from the pool, for readers that hold
/// none; a failure leaves the pointer out rather than failing the read.
pub async fn pointer_from_pool(
    pool: &sqlx::PgPool,
    memory_for: MemoryFor,
    id: Uuid,
    viewer: Uuid,
) -> Option<MemoryPointer> {
    let mut transaction = crate::db::scoped_tx(pool).await.ok()?;
    let found = pointer(&mut transaction, memory_for, id, viewer).await;
    let _ = transaction.commit().await;
    match found {
        Ok(found) => found,
        Err(error) => {
            tracing::warn!(?error, "memory pointer");
            None
        }
    }
}

/// A conversation that can have memory: a channel or a thread the caller
/// can read. Its name, for the page's title.
async fn conversation_name(
    connection: &mut PgConnection,
    conversation_id: Uuid,
    viewer: Uuid,
) -> Result<String, OpError> {
    let row = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT kind::text, name FROM conversations \
         WHERE id = $1 AND conversation_visible_to(id, $2)",
    )
    .bind(conversation_id)
    .bind(viewer)
    .fetch_optional(&mut *connection)
    .await
    .map_err(AppError::from)?;
    match row {
        Some((kind, name)) if kind == "channel" || kind == "thread" => {
            let name = name.unwrap_or_else(|| "this thread".to_owned());
            Ok(if kind == "channel" {
                format!("#{name}")
            } else {
                name
            })
        }
        Some(_) => Err(OpError::invalid(
            "conversation",
            "Memory pages are for spaces, channels and threads.",
        )),
        None => Err(OpError::not_found("That conversation")),
    }
}

/// Where the memory is: a space or a conversation.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryPlace {
    /// A channel or thread, or a link to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<IdRef>,
    /// A space, or a link to it. Give this or conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<IdRef>,
}

impl MemoryPlace {
    fn target(&self) -> Target {
        match (self.conversation, self.space) {
            (Some(conversation), _) => Target::thing("conversation", conversation.id()),
            (None, Some(space)) => Target::Space(space.id()),
            (None, None) => Target::Workspace(None),
        }
    }

    fn of(&self) -> Result<(MemoryFor, Uuid), OpError> {
        match (self.conversation, self.space) {
            (Some(_), Some(_)) => Err(OpError::invalid(
                "space",
                "Give a conversation or a space, not both.",
            )),
            (Some(conversation), None) => Ok((MemoryFor::Conversation, conversation.id())),
            (None, Some(space)) => Ok((MemoryFor::Space, space.id())),
            (None, None) => Err(OpError::invalid(
                "conversation",
                "Give the channel, thread or space whose memory this is.",
            )),
        }
    }
}

/// A memory page in full.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPageView {
    #[schemars(with = "String")]
    pub page_id: Uuid,
    #[schemars(with = "String")]
    pub space_id: Uuid,
    pub title: String,
    pub body: String,
    pub version: i64,
    pub updated_at: String,
}

async fn page_of(
    connection: &mut PgConnection,
    memory_for: MemoryFor,
    id: Uuid,
    viewer: Uuid,
) -> Result<Option<MemoryPageView>, AppError> {
    Ok(
        sqlx::query_as::<_, (Uuid, Uuid, String, String, i64, String)>(
            r#"
        SELECT page.id, page.project_id, page.title, page.body, page.version,
               page.updated_at::text
        FROM project_pages page
        WHERE page.memory_for_kind = $1 AND page.memory_for_id = $2
          AND page.archived_at IS NULL
          AND page_visible_to(page.id, $3)
        "#,
        )
        .bind(memory_for.key())
        .bind(id)
        .bind(viewer)
        .fetch_optional(&mut *connection)
        .await?
        .map(
            |(page_id, space_id, title, body, version, updated_at)| MemoryPageView {
                page_id,
                space_id,
                title,
                body,
                version,
                updated_at,
            },
        ),
    )
}

/// Read a space's or conversation's memory page.
pub struct MemoryPageOpen;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPageOpenOutput {
    /// The page; null until someone writes it.
    pub page: Option<MemoryPageView>,
}

impl Targeted for MemoryPlace {
    fn target(&self) -> Target {
        MemoryPlace::target(self)
    }
}

impl Operation for MemoryPageOpen {
    const NAME: &'static str = "memory_page_open";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Open memory";
    const DESCRIPTION: &'static str = "The memory page of a space, channel or thread: what \
        anyone working there should know first. Read it before acting there; page is null \
        until someone writes it with memory_page_set.";

    type Input = MemoryPlace;
    type Output = MemoryPageOpenOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: MemoryPlace,
    ) -> Result<MemoryPageOpenOutput, OpError> {
        let (memory_for, id) = input.of()?;
        let viewer = call.account_id();
        let tx = call.tx().await?;
        if memory_for == MemoryFor::Conversation {
            conversation_name(tx, id, viewer).await?;
        }
        Ok(MemoryPageOpenOutput {
            page: page_of(tx, memory_for, id, viewer).await?,
        })
    }
}

/// Write a space's or conversation's memory page, making it the first time.
pub struct MemoryPageWrite;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryPageWriteInput {
    /// A channel or thread, or a link to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<IdRef>,
    /// A space, or a link to it. Give this or conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<IdRef>,
    /// The whole page, in Markdown. Keep it short: durable facts only.
    pub body: String,
    /// The version you read; the write is refused if it changed since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    /// A short note on what changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Targeted for MemoryPageWriteInput {
    fn target(&self) -> Target {
        MemoryPlace {
            conversation: self.conversation,
            space: self.space,
        }
        .target()
    }
}

impl Operation for MemoryPageWrite {
    const NAME: &'static str = "memory_page_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Perm> = Some(Perm::PagesEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Write memory";
    const DESCRIPTION: &'static str = "Replace the memory page of a space, channel or thread \
        (made on the first write; every version is kept). Record durable facts others \
        working there need, briefly. Pass expectedVersion from memory_page_open.";
    const EMITS: &'static [&'static str] = &["page.updated"];

    type Input = MemoryPageWriteInput;
    type Output = MemoryPageView;

    async fn run(
        call: &mut OpCall<'_>,
        input: MemoryPageWriteInput,
    ) -> Result<MemoryPageView, OpError> {
        let (memory_for, id) = MemoryPlace {
            conversation: input.conversation,
            space: input.space,
        }
        .of()?;
        let writer = Writer::from(call.ctx);
        let viewer = writer.actor_id;
        let space_id = call
            .ctx
            .space_id
            .ok_or_else(|| OpError::not_found("That space"))?;
        let tx = call.tx().await?;
        let title = match memory_for {
            MemoryFor::Conversation => {
                format!("Memory of {}", conversation_name(tx, id, viewer).await?)
            }
            MemoryFor::Space => "Memory".to_owned(),
        };
        let existing = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM project_pages \
             WHERE memory_for_kind = $1 AND memory_for_id = $2 AND archived_at IS NULL \
             FOR UPDATE",
        )
        .bind(memory_for.key())
        .bind(id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let page_id = match existing {
            Some(page_id) => {
                super::update_page(
                    tx,
                    writer,
                    page_id,
                    &PageInput {
                        body: Some(input.body),
                        expected_version: input.expected_version,
                        note: input.note,
                        ..PageInput::default()
                    },
                )
                .await?;
                page_id
            }
            None => {
                if input.expected_version.is_some_and(|version| version != 0) {
                    return Err(AppError::VersionConflict {
                        resource: "page",
                        current_version: 0,
                    }
                    .into());
                }
                let title = super::free_page_title(tx, space_id, &title).await?;
                let page_id = super::create_page(
                    tx,
                    writer,
                    space_id,
                    &PageInput {
                        title: Some(title),
                        body: Some(input.body),
                        note: input.note,
                        ..PageInput::default()
                    },
                )
                .await?;
                // Listing `updated_at` brings the thing registry along, so
                // a conversation's memory takes the conversation as its
                // access parent there too.
                sqlx::query(
                    "UPDATE project_pages \
                     SET memory_for_kind = $2, memory_for_id = $3, updated_at = updated_at \
                     WHERE id = $1",
                )
                .bind(page_id)
                .bind(memory_for.key())
                .bind(id)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
                page_id
            }
        };
        let page = page_of(tx, memory_for, id, viewer)
            .await?
            .ok_or_else(|| OpError::not_found("That page"))?;
        call.audit_target(
            "page",
            page_id,
            json!({ "memoryFor": memory_for.key(), "version": page.version }),
        );
        Ok(page)
    }
}
