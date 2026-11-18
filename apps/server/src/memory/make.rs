//! A page made from other things (C1.25): from messages (quoted in the order
//! they were sent, with a link back to their conversation), from a whole
//! thread, or from blocks of another page (copied, or moved: the blocks
//! leave their page and a link to the new page takes their place, as one
//! change that can be undone). The page remembers what it was made from
//! (`project_pages.made_from_id`, mirrored as a `made_from` link).
//!
//! From a DM or group the person names the space the page goes in; the
//! messages must be ones they can read.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::FromRow;
use uuid::Uuid;

use super::{PageInput, Writer};
use crate::{
    addresses::{Addresses, Target as Address},
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Target, Targeted,
        refs::{IdRef, MessageRef},
    },
    page_docs::{
        edits::{self, Edit, EditOp, Target as BlockTarget},
        store, ydoc,
    },
};

/// Messages one page quotes, picked one by one.
pub const MAX_MESSAGES: usize = 50;
/// Messages a page made from a thread quotes, oldest first.
pub const MAX_THREAD_MESSAGES: i64 = 200;
/// Blocks one page takes from another.
pub const MAX_BLOCKS: usize = 200;

pub struct PageMake;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageMakeInput {
    /// The space the page goes in: its id, link or key. Needed for messages
    /// in a DM or group; otherwise it's where they are.
    #[serde(default, alias = "projectId", skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
    /// The page's title; left out, the thread's name or the first words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Messages to quote, all from one conversation (up to 50). They're
    /// quoted in the order they were sent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<MessageRef>,
    /// A thread: the page quotes its messages, oldest first (up to 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<IdRef>,
    /// A page to take blocks from, with blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_id: Option<IdRef>,
    /// The blocks to take, by id (page_blocks lists them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<String>,
    /// With pageId: move the blocks rather than copy them. A link to the
    /// new page takes their place; change_undo puts them back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub r#move: bool,
}

impl Targeted for PageMakeInput {
    /// The space named, else the space of what the page is made from. A
    /// message, thread or page the caller can't see is `not_found`.
    fn target(&self) -> Target {
        match (
            self.space_id,
            self.messages.first(),
            self.thread_id,
            self.page_id,
        ) {
            (Some(space), ..) => Target::Space(space.id()),
            (None, Some(message), ..) => Target::thing("message", message.id()),
            (None, None, Some(thread), _) => Target::thing("conversation", thread.id()),
            (None, None, None, Some(page)) => Target::thing("page", page.id()),
            (None, None, None, None) => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PageMade {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub title: String,
    #[schemars(with = "String")]
    pub space_id: Uuid,
    pub url: String,
    /// With move: the version of the page the blocks left.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<i64>,
}

impl Operation for PageMake {
    const NAME: &'static str = "page_make";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Perm> = Some(Perm::PagesEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Make a page from messages or blocks";
    const DESCRIPTION: &'static str = "Make a page from messages (quoted in order, with a link \
        back), a whole thread, or blocks of another page (copied, or moved so a link to the new \
        page takes their place; change_undo puts them back). The page shows what it was made \
        from. From a DM or group, name the space.";
    const EMITS: &'static [&'static str] = &["page.updated"];

    type Input = PageMakeInput;
    type Output = PageMade;

    async fn run(call: &mut OpCall<'_>, input: PageMakeInput) -> Result<PageMade, OpError> {
        let sources = usize::from(!input.messages.is_empty())
            + usize::from(input.thread_id.is_some())
            + usize::from(input.page_id.is_some());
        if sources != 1 {
            return Err(OpError::invalid(
                "messages",
                "Make a page from one thing: messages, a threadId, or a pageId with blocks.",
            ));
        }
        let space_id = call.ctx.space_id.ok_or_else(|| {
            OpError::invalid(
                "spaceId",
                "Say which space the page goes in (spaceId): these messages aren't in one.",
            )
        })?;
        let workspace_id = call.ctx.workspace_id;
        let writer = Writer::from(call.ctx);
        let caller = call.ctx.caller.clone();
        let addresses = Addresses::of(call.state);
        let tx = call.tx().await?;
        reach_source(tx, &caller, workspace_id, &input).await?;
        let made = if let Some(page_id) = input.page_id {
            from_blocks(tx, writer, page_id.id(), &input.blocks).await?
        } else {
            let quoted = match input.thread_id {
                Some(thread) => thread_messages(tx, writer.actor_id, thread.id()).await?,
                None => picked_messages(tx, writer.actor_id, &input.messages).await?,
            };
            quote(addresses, &quoted)?
        };
        let title_seed = input
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map_or_else(|| made.title.clone(), str::to_owned);
        let title = super::free_page_title(tx, space_id, &title_seed).await?;
        let page = PageInput {
            title: Some(title.clone()),
            body: Some(made.body.clone()),
            ..PageInput::default()
        };
        let id = super::create_page(tx, writer, space_id, &page).await?;
        sqlx::query("UPDATE project_pages SET made_from_id = $2 WHERE id = $1")
            .bind(id)
            .bind(made.from)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        let path = addresses.in_workspace(workspace_id, &Address::page(space_id, id));
        let source_version = match (input.page_id, input.r#move) {
            (Some(source), true) => Some(
                move_out(
                    tx,
                    writer,
                    source.id(),
                    space_id,
                    &made.blocks,
                    &title,
                    &path,
                )
                .await?,
            ),
            _ => None,
        };
        call.audit_target(
            "page",
            id,
            json!({"madeFrom": made.from, "moved": source_version.is_some()}),
        );
        Ok(PageMade {
            id,
            title,
            space_id,
            url: path,
            source_version,
        })
    }
}

/// The call is admitted where the page goes; what it is made from is
/// checked here the same way, with the whole caller (a credential kept to
/// one space, a delegation's limits), not only whether the account sees it.
async fn reach_source(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    caller: &crate::ops::Caller,
    workspace_id: Uuid,
    input: &PageMakeInput,
) -> Result<(), OpError> {
    use crate::access::{self, Need};
    let (source, need, what) = match (input.messages.first(), input.thread_id, input.page_id) {
        (Some(message), ..) => (
            Target::thing("message", message.id()),
            Need::SEE,
            "That message",
        ),
        (None, Some(thread), _) => (
            Target::thing("conversation", thread.id()),
            Need::SEE,
            "That conversation",
        ),
        (None, None, Some(page)) => (
            Target::thing("page", page.id()),
            // Moving blocks out changes their page.
            if input.r#move {
                Need::permission(Perm::PagesEdit).writes()
            } else {
                Need::SEE
            },
            "That page",
        ),
        (None, None, None) => return Ok(()),
    };
    let located =
        crate::ops::target::locate(&mut **tx, &source, caller.account_id, Some(workspace_id))
            .await?;
    let scope = match located.space_id {
        Some(space) => access::Target::Space(space),
        None => access::Target::Workspace(located.workspace_id),
    };
    access::check_locked(tx, caller, need, scope)
        .await
        .map_err(AppError::from)?
        .map(|_| ())
        .map_err(|_| OpError::not_found(what))
}

/// What a page is made of: its first title, its Markdown, the thing it
/// was made from, and (from a page) the blocks it took.
struct Made {
    title: String,
    body: String,
    from: Uuid,
    blocks: Vec<String>,
}

#[derive(FromRow)]
struct Quoted {
    id: Uuid,
    author: String,
    text: String,
    sent: String,
    conversation_id: Uuid,
    conversation_name: Option<String>,
    conversation_kind: String,
    parent_id: Option<Uuid>,
    space_id: Option<Uuid>,
    workspace_id: Uuid,
}

const QUOTED: &str = "SELECT message.id, \
       COALESCE(member.display_name, profile.display_name, 'Someone') AS author, \
       message.plain_text AS text, \
       to_char(message.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI \"UTC\"') AS sent, \
       message.conversation_id, conversation.name AS conversation_name, \
       conversation.kind AS conversation_kind, conversation.parent_channel_id AS parent_id, \
       conversation.project_id AS space_id, conversation.workspace_id \
     FROM messages message \
     JOIN conversations conversation ON conversation.id = message.conversation_id \
     LEFT JOIN project_memberships member \
       ON member.project_id = message.project_id AND member.actor_id = message.author_actor_id \
     LEFT JOIN organization_actor_profiles profile \
       ON profile.organization_id = conversation.workspace_id \
      AND profile.actor_id = message.author_actor_id \
     WHERE conversation_visible_to(message.conversation_id, $2) AND message.removed_at IS NULL";

/// The messages picked, in the order they were sent. Each must be one the
/// writer can read, and all from one conversation.
async fn picked_messages(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    viewer: Uuid,
    picked: &[MessageRef],
) -> Result<Vec<Quoted>, OpError> {
    if picked.len() > MAX_MESSAGES {
        return Err(OpError::invalid(
            "messages",
            "A page quotes up to 50 picked messages; make it from the thread instead.",
        ));
    }
    let ids = picked
        .iter()
        .map(|message| message.id())
        .collect::<Vec<_>>();
    let found = sqlx::query_as::<_, Quoted>(&format!(
        "{QUOTED} AND message.id = ANY($1) ORDER BY message.created_at, message.id"
    ))
    .bind(&ids)
    .bind(viewer)
    .fetch_all(&mut **tx)
    .await
    .map_err(AppError::from)?;
    let distinct = ids.iter().collect::<std::collections::HashSet<_>>().len();
    if found.len() != distinct {
        return Err(OpError::not_found("That message"));
    }
    if found
        .iter()
        .any(|message| message.conversation_id != found[0].conversation_id)
    {
        return Err(OpError::invalid(
            "messages",
            "Pick messages from one conversation.",
        ));
    }
    Ok(found)
}

/// A thread's messages, oldest first.
async fn thread_messages(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    viewer: Uuid,
    thread: Uuid,
) -> Result<Vec<Quoted>, OpError> {
    let found = sqlx::query_as::<_, Quoted>(&format!(
        "{QUOTED} AND message.conversation_id = $1 AND conversation.kind = 'thread' \
         ORDER BY message.created_at, message.id LIMIT $3"
    ))
    .bind(thread)
    .bind(viewer)
    .bind(MAX_THREAD_MESSAGES)
    .fetch_all(&mut **tx)
    .await
    .map_err(AppError::from)?;
    if found.is_empty() {
        return Err(OpError::not_found("That thread"));
    }
    Ok(found)
}

/// The first line of `text`, for a title: up to 80 characters.
fn first_words(text: &str) -> String {
    let line = text
        .lines()
        .map(|line| line.trim().trim_start_matches(['#', '>', '-', '*', ' ']))
        .find(|line| !line.is_empty())
        .unwrap_or("Notes");
    let mut title = line.chars().take(80).collect::<String>();
    if line.chars().count() > 80 {
        title.push('…');
    }
    title
}

/// The messages as a page: a line naming where they're from (a link back),
/// then each one quoted under its author and time.
fn quote(addresses: Addresses, quoted: &[Quoted]) -> Result<Made, OpError> {
    let first = quoted.first().ok_or(OpError::not_found("That message"))?;
    let place = Address::conversation(
        first.space_id.unwrap_or_default(),
        &first.conversation_kind,
        first.conversation_id,
        first.parent_id,
        first.space_id.is_none(),
        true,
    );
    let link = addresses.in_workspace(first.workspace_id, &place);
    let name = first
        .conversation_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("the conversation");
    let thread = first.conversation_kind == "thread";
    let mut body = format!("From [{}]({link})\n", escape(name));
    for message in quoted {
        body.push_str(&format!(
            "\n> **{}** · {}\n>\n",
            escape(&message.author),
            message.sent
        ));
        for line in message.text.lines() {
            if line.trim().is_empty() {
                body.push_str(">\n");
            } else {
                body.push_str(&format!("> {line}\n"));
            }
        }
    }
    let title = match (thread, first.conversation_name.as_deref()) {
        (true, Some(name)) if !name.trim().is_empty() => name.trim().to_owned(),
        _ => first_words(&first.text),
    };
    let from = if thread && quoted.len() > 1 {
        first.conversation_id
    } else {
        first.id
    };
    Ok(Made {
        title,
        body,
        from,
        blocks: Vec::new(),
    })
}

/// Markdown's link text and emphasis characters, escaped.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(character, '[' | ']' | '*' | '_' | '`' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// Blocks of a page the writer can see, in the page's order, as Markdown.
async fn from_blocks(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    writer: Writer,
    page_id: Uuid,
    wanted: &[String],
) -> Result<Made, OpError> {
    if wanted.is_empty() || wanted.len() > MAX_BLOCKS {
        return Err(OpError::invalid(
            "blocks",
            "Name 1 to 200 blocks of the page, by id (page_blocks lists them).",
        ));
    }
    let visible = sqlx::query_scalar::<_, bool>("SELECT subject_visible_to('page', $1, $2)")
        .bind(page_id)
        .bind(writer.actor_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
    if !visible {
        return Err(OpError::not_found("That page"));
    }
    let page = store::lock_page(tx, page_id)
        .await?
        .ok_or(OpError::not_found("That page"))?;
    let stored = store::document(tx, page_id, &page).await?;
    let blocks = store::read_blocks(&stored.state)?;
    let derived = store::derive_markdown(&page.body, &blocks);
    let mut taken = Vec::new();
    let mut ids = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let Some(id) = block.id.as_deref() else {
            continue;
        };
        if !wanted.iter().any(|wanted| wanted == id) {
            continue;
        }
        if let Some(range) = derived.ranges.get(index).cloned().flatten() {
            taken.push(derived.markdown[range].trim_end().to_owned());
        }
        ids.push(id.to_owned());
    }
    if ids.len()
        != wanted
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
    {
        return Err(OpError::invalid(
            "blocks",
            "That page has no such block; read its blocks again.",
        ));
    }
    let body = taken.join("\n\n");
    Ok(Made {
        title: first_words(&body),
        body,
        from: page_id,
        blocks: ids,
    })
}

/// Takes the blocks out of their page and puts a link to the new page in
/// the first one's place, as a version of its own (a change set, so
/// change_undo puts them back). The page must be in the same space.
async fn move_out(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    writer: Writer,
    page_id: Uuid,
    space_id: Uuid,
    taken: &[String],
    title: &str,
    path: &str,
) -> Result<i64, OpError> {
    let page = store::lock_page(tx, page_id)
        .await?
        .ok_or(OpError::not_found("That page"))?;
    if page.project_id != space_id {
        return Err(OpError::invalid(
            "move",
            "Blocks move to a page in the same space; copy them to another space instead.",
        ));
    }
    if page.archived {
        return Err(OpError::invalid(
            "pageId",
            "That page is archived; bring it back before moving blocks out of it.",
        ));
    }
    let stored = store::document(tx, page_id, &page).await?;
    let current = store::read_blocks(&stored.state)?;
    let embed = format!("[{}]({path} \"embed\")", escape(title));
    let changes = taken
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let block = current
                .iter()
                .find(|block| block.id.as_deref() == Some(id.as_str()));
            Edit {
                op: if index == 0 {
                    EditOp::Replace
                } else {
                    EditOp::Delete
                },
                target: Some(BlockTarget::Id(id.clone())),
                hash: block.map(edits::content_hash),
                markdown: (index == 0).then(|| embed.clone()),
            }
        })
        .collect::<Vec<_>>();
    let blocks = edits::apply(&current, &changes).map_err(|error| OpError::Invalid {
        field: "blocks".into(),
        message: error.to_string().into(),
    })?;
    let change = store::change(&stored.state, &page.body, &[embed.as_str()], |doc, _| {
        ydoc::replace(doc, &blocks);
        Ok(())
    })?;
    let (version, _) = store::save(
        tx,
        writer,
        page_id,
        &page,
        &change,
        Some("Moved to a new page"),
        store::History::Version,
    )
    .await?;
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_comes_from_the_first_words() {
        assert_eq!(first_words("\n# Plan for Q1\nmore"), "Plan for Q1");
        assert_eq!(first_words("   "), "Notes");
        assert_eq!(first_words(&"a".repeat(90)).chars().count(), 81);
    }

    #[test]
    fn names_are_escaped_in_links() {
        assert_eq!(escape("a [b]*c*"), "a \\[b\\]\\*c\\*");
    }
}
