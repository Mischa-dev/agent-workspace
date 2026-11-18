//! Sidebar sections (C2.9): the sections a person arranges their channels,
//! DMs and groups in, across a workspace's spaces. Each account keeps its
//! own: Starred (made the first time something is starred) and any it
//! names. A conversation sits in at most one; one in none stays where it
//! always was. Every read filters by `conversation_visible_to`, so losing
//! access hides an item and keeps its row. Agents arrange their own, as
//! people do.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

/// The most sections, and items, one account keeps in a workspace.
const MAX_SECTIONS: i64 = 50;
const MAX_ITEMS: i64 = 500;
const STARRED: &str = "Starred";

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<SidebarList>()
        .add::<SidebarSectionSet>()
        .add::<SidebarItemMove>();
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SectionSort {
    /// As placed.
    Manual,
    /// By name.
    Alpha,
    /// Newest activity first.
    Recent,
}

impl SectionSort {
    fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Alpha => "alpha",
            Self::Recent => "recent",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SectionShow {
    /// Everything in it.
    All,
    /// Only what's unread.
    Unread,
}

impl SectionShow {
    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Unread => "unread",
        }
    }
}

/// A conversation placed in a section.
#[derive(Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SidebarItem {
    #[schemars(with = "String")]
    pub conversation_id: Uuid,
    #[schemars(with = "String")]
    pub section_id: Uuid,
    /// channel, dm or group.
    pub kind: String,
    /// A channel's name, or a DM's or group's other people.
    pub name: String,
    /// The space it's in.
    #[schemars(with = "String")]
    pub space_id: Uuid,
    pub private: bool,
    pub unread: bool,
    pub position: i32,
    /// When its newest message came, if it has one.
    pub last_message_at: Option<String>,
}

/// A section and what's in it.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SidebarSection {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
    /// custom, or starred.
    pub kind: String,
    pub position: i32,
    pub sort: String,
    pub show: String,
    pub collapsed: bool,
    pub version: i64,
    pub items: Vec<SidebarItem>,
}

#[derive(sqlx::FromRow)]
struct SectionRow {
    id: Uuid,
    name: String,
    kind: String,
    position: i32,
    sort: String,
    show: String,
    collapsed: bool,
    version: i64,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Sidebar {
    pub sections: Vec<SidebarSection>,
    /// The workspace's own space, where its channels live when it has no
    /// projects (C2.40).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub workspace_space_id: Option<Uuid>,
}

/// The caller's sections in the call's workspace, with what they can still
/// see in each, sorted as each section says.
async fn sidebar(call: &mut OpCall<'_>) -> Result<Sidebar, OpError> {
    let caller = call.account_id();
    let workspace = call.ctx.workspace_id;
    let tx = call.tx().await?;
    let sections = sqlx::query_as::<_, SectionRow>(
        r#"
        SELECT id, name, kind, position, sort, show, collapsed, version
        FROM sidebar_sections
        WHERE account_id = $1 AND workspace_id = $2
        ORDER BY kind = 'starred' DESC, position, lower(name), id
        "#,
    )
    .bind(caller)
    .bind(workspace)
    .fetch_all(&mut **tx)
    .await
    .map_err(AppError::from)?;
    let items = sqlx::query_as::<_, SidebarItem>(
        r#"
        SELECT item.conversation_id, item.section_id, conversation.kind,
               CASE WHEN conversation.kind IN ('dm', 'group')
                         AND (conversation.kind = 'dm' OR btrim(conversation.name) = '')
                    THEN COALESCE((
                        SELECT string_agg(membership.display_name, ', '
                                          ORDER BY lower(membership.display_name))
                        FROM conversation_members member
                        JOIN project_memberships membership
                          ON membership.project_id = member.project_id
                         AND membership.actor_id = member.actor_id
                        WHERE member.conversation_id = conversation.id
                          AND member.actor_id <> $1
                    ), conversation.name)
                    ELSE conversation.name END AS name,
               conversation.project_id AS space_id,
               conversation.visibility = 'members' AND conversation.kind = 'channel' AS private,
               conversation.last_message_sequence
                   > COALESCE(read_state.highest_seen_sequence, 0) AS unread,
               item.position,
               (SELECT to_char(max(message.created_at) AT TIME ZONE 'UTC',
                               'YYYY-MM-DD"T"HH24:MI:SS"Z"')
                FROM messages message
                WHERE message.conversation_id = conversation.id
                  AND message.removed_at IS NULL) AS last_message_at
        FROM sidebar_items item
        JOIN conversations conversation ON conversation.id = item.conversation_id
        LEFT JOIN conversation_read_states read_state
          ON read_state.conversation_id = conversation.id AND read_state.actor_id = $1
        WHERE item.account_id = $1
          AND item.workspace_id = $2
          AND conversation.archived_at IS NULL
          AND conversation_visible_to(conversation.id, $1)
          AND EXISTS (SELECT 1 FROM project_memberships membership
                      WHERE membership.project_id = conversation.project_id
                        AND membership.actor_id = $1 AND membership.enabled)
        ORDER BY item.section_id, item.position, conversation.id
        "#,
    )
    .bind(caller)
    .bind(workspace)
    .fetch_all(&mut **tx)
    .await
    .map_err(AppError::from)?;
    let mut by_section: std::collections::HashMap<Uuid, Vec<SidebarItem>> =
        std::collections::HashMap::new();
    for item in items {
        by_section.entry(item.section_id).or_default().push(item);
    }
    let sections = sections
        .into_iter()
        .map(|section| {
            let mut mine = by_section.remove(&section.id).unwrap_or_default();
            match section.sort.as_str() {
                "alpha" => mine.sort_by_key(|item| item.name.to_lowercase()),
                "recent" => {
                    mine.sort_by(|left, right| right.last_message_at.cmp(&left.last_message_at))
                }
                _ => {}
            }
            // Every item comes back, read or not: "only unread" is how a
            // section shows, and where a conversation sits stays known.
            SidebarSection {
                id: section.id,
                name: section.name,
                kind: section.kind,
                position: section.position,
                sort: section.sort,
                show: section.show,
                collapsed: section.collapsed,
                version: section.version,
                items: mine,
            }
        })
        .collect();
    let workspace_space_id = crate::spaces::workspace::find(tx, workspace).await?;
    Ok(Sidebar {
        sections,
        workspace_space_id,
    })
}

/// Tells the caller's other tabs and devices (their id only), in the same
/// transaction. The envelope needs a space: the workspace's own.
async fn announce(call: &mut OpCall<'_>) -> Result<(), OpError> {
    let (workspace, account) = (call.ctx.workspace_id, call.account_id());
    let tx = call.tx().await?;
    let space = crate::spaces::workspace::ensure(tx, workspace).await?;
    crate::outbox::enqueue_typed(
        tx,
        space,
        crate::outbox::OutboxEventPayload::SidebarChanged(crate::outbox::SidebarChangedPayload {
            actor_id: account,
        }),
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// sidebar_list

pub struct SidebarList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SidebarListInput {}

impl Targeted for SidebarListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for SidebarList {
    const NAME: &'static str = "sidebar_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List sidebar sections";
    const DESCRIPTION: &'static str = "Your own sidebar sections in this workspace (Starred, and \
        any you named) with the channels, DMs and groups you placed in each, whether each is \
        unread, in each section's order. A section whose show is unread still lists \
        every item; display only the unread ones. Arrange them with sidebar_section_set and \
        sidebar_item_move.";

    type Input = SidebarListInput;
    type Output = Sidebar;

    async fn run(call: &mut OpCall<'_>, _input: SidebarListInput) -> Result<Sidebar, OpError> {
        sidebar(call).await
    }
}

// ---------------------------------------------------------------------------
// sidebar_section_set

pub struct SidebarSectionSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SidebarSectionSetInput {
    /// The section to change; left out, a new one is made (give its name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_id: Option<IdRef>,
    /// The version you last read; a change made since is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
    /// Its name, 1 to 80 characters on one line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its place among your sections, from 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<SectionSort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<SectionShow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapsed: Option<bool>,
    /// true removes the section; what was in it goes back where it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete: Option<bool>,
}

impl Targeted for SidebarSectionSetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

fn section_name(name: &str) -> Result<String, OpError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(OpError::invalid(
            "name",
            "A section's name is 1 to 80 characters, on one line.",
        ));
    }
    Ok(name.to_owned())
}

impl Operation for SidebarSectionSet {
    const NAME: &'static str = "sidebar_section_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Set sidebar section";
    const DESCRIPTION: &'static str = "Make one of your sidebar sections (leave sectionId out and \
        give a name), or change one: its name, place, sort (manual, alpha, recent), what it \
        shows (all, or unread only) and whether it's collapsed. delete: true removes it, and \
        what was in it goes back where it was. Returns your whole sidebar.";
    const EMITS: &'static [&'static str] = &["sidebar.changed"];

    type Input = SidebarSectionSetInput;
    type Output = Sidebar;

    async fn run(call: &mut OpCall<'_>, input: SidebarSectionSetInput) -> Result<Sidebar, OpError> {
        let caller = call.account_id();
        let workspace = call.ctx.workspace_id;
        let name = input.name.as_deref().map(section_name).transpose()?;
        if input.position.is_some_and(|position| position < 0) {
            return Err(OpError::invalid("position", "A position is 0 or more."));
        }
        let tx = call.tx().await?;
        let Some(section_id) = input.section_id.map(IdRef::id) else {
            let Some(name) = name else {
                return Err(OpError::invalid("name", "Name the new section."));
            };
            if input.delete == Some(true) {
                return Err(OpError::invalid(
                    "sectionId",
                    "Say which section to remove.",
                ));
            }
            let count = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM sidebar_sections WHERE account_id = $1 AND workspace_id = $2",
            )
            .bind(caller)
            .bind(workspace)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
            if count >= MAX_SECTIONS {
                return Err(OpError::invalid(
                    "name",
                    "You have 50 sections here already; remove one first.",
                ));
            }
            let id = Uuid::now_v7();
            sqlx::query(
                r#"
                INSERT INTO sidebar_sections
                    (id, workspace_id, account_id, name, position, sort, show, collapsed)
                VALUES ($1, $2, $3, $4,
                        COALESCE($5, (SELECT COALESCE(max(position) + 1, 0) FROM sidebar_sections
                                      WHERE account_id = $3 AND workspace_id = $2)),
                        $6, $7, $8)
                "#,
            )
            .bind(id)
            .bind(workspace)
            .bind(caller)
            .bind(&name)
            .bind(input.position)
            .bind(input.sort.unwrap_or(SectionSort::Manual).as_str())
            .bind(input.show.unwrap_or(SectionShow::All).as_str())
            .bind(input.collapsed.unwrap_or(false))
            .execute(&mut **tx)
            .await
            .map_err(|error| match AppError::from(error) {
                AppError::Conflict(..) | AppError::Database(_) => {
                    OpError::invalid("name", "You have a section with that name already.")
                }
                other => other.into(),
            })?;
            call.audit_target("sidebar_section", id, serde_json::json!({}));
            announce(call).await?;
            return sidebar(call).await;
        };
        let current = sqlx::query_scalar::<_, i64>(
            r#"
            SELECT version FROM sidebar_sections
            WHERE id = $1 AND account_id = $2 AND workspace_id = $3
            FOR UPDATE
            "#,
        )
        .bind(section_id)
        .bind(caller)
        .bind(workspace)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| OpError::not_found("That section"))?;
        if let Some(expected) = input.expected_version
            && expected != current
        {
            return Err(AppError::VersionConflict {
                resource: "sidebar_section",
                current_version: current,
            }
            .into());
        }
        if input.delete == Some(true) {
            sqlx::query("DELETE FROM sidebar_sections WHERE id = $1 AND account_id = $2")
                .bind(section_id)
                .bind(caller)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
        } else {
            sqlx::query(
                r#"
                UPDATE sidebar_sections
                SET name = COALESCE($3, name),
                    position = COALESCE($4, position),
                    sort = COALESCE($5, sort),
                    show = COALESCE($6, show),
                    collapsed = COALESCE($7, collapsed),
                    version = version + 1,
                    updated_at = now()
                WHERE id = $1 AND account_id = $2
                "#,
            )
            .bind(section_id)
            .bind(caller)
            .bind(&name)
            .bind(input.position)
            .bind(input.sort.map(SectionSort::as_str))
            .bind(input.show.map(SectionShow::as_str))
            .bind(input.collapsed)
            .execute(&mut **tx)
            .await
            .map_err(|_| OpError::invalid("name", "You have a section with that name already."))?;
        }
        call.audit_target("sidebar_section", section_id, serde_json::json!({}));
        announce(call).await?;
        sidebar(call).await
    }
}

// ---------------------------------------------------------------------------
// sidebar_item_move

pub struct SidebarItemMove;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SidebarItemMoveInput {
    /// The channel, DM or group: its id or a link to it.
    pub conversation_id: IdRef,
    /// The section to put it in. Leave it out (and starred) to take it out
    /// of your sections, back where it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_id: Option<IdRef>,
    /// true puts it in Starred (made if you have none); false takes it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starred: Option<bool>,
    /// Its place in the section, from 0; left out, the end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
}

impl Targeted for SidebarItemMoveInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.conversation_id.id())
    }
}

impl Operation for SidebarItemMove {
    const NAME: &'static str = "sidebar_item_move";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Move in sidebar";
    const DESCRIPTION: &'static str = "Put a channel, DM or group in one of your sidebar sections \
        (sectionId), star it (starred: true), or take it out of your sections (neither), at a \
        place in the section (position). Returns your whole sidebar.";
    const EMITS: &'static [&'static str] = &["sidebar.changed"];

    type Input = SidebarItemMoveInput;
    type Output = Sidebar;

    async fn run(call: &mut OpCall<'_>, input: SidebarItemMoveInput) -> Result<Sidebar, OpError> {
        let caller = call.account_id();
        let workspace = call.ctx.workspace_id;
        let conversation_id = input.conversation_id.id();
        if input.position.is_some_and(|position| position < 0) {
            return Err(OpError::invalid("position", "A position is 0 or more."));
        }
        if input.section_id.is_some() && input.starred.is_some() {
            return Err(OpError::invalid(
                "starred",
                "Give a section or starred, not both.",
            ));
        }
        let tx = call.tx().await?;
        let kind = sqlx::query_scalar::<_, String>(
            r#"
            SELECT kind FROM conversations
            WHERE id = $1 AND workspace_id = $3 AND archived_at IS NULL
              AND conversation_visible_to(id, $2)
            "#,
        )
        .bind(conversation_id)
        .bind(caller)
        .bind(workspace)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?
        .ok_or_else(|| OpError::not_found("That conversation"))?;
        if !matches!(kind.as_str(), "channel" | "dm" | "group") {
            return Err(OpError::invalid(
                "conversationId",
                "Sections hold channels, DMs and groups.",
            ));
        }
        let section = match (input.section_id.map(IdRef::id), input.starred) {
            (Some(section), _) => Some(
                sqlx::query_scalar::<_, Uuid>(
                    r#"
                    SELECT id FROM sidebar_sections
                    WHERE id = $1 AND account_id = $2 AND workspace_id = $3
                    FOR UPDATE
                    "#,
                )
                .bind(section)
                .bind(caller)
                .bind(workspace)
                .fetch_optional(&mut **tx)
                .await
                .map_err(AppError::from)?
                .ok_or_else(|| OpError::not_found("That section"))?,
            ),
            (None, Some(true)) => {
                sqlx::query(
                    r#"
                    INSERT INTO sidebar_sections (id, workspace_id, account_id, name, kind, position)
                    VALUES ($1, $2, $3, $4, 'starred', 0)
                    ON CONFLICT DO NOTHING
                    "#,
                )
                .bind(Uuid::now_v7())
                .bind(workspace)
                .bind(caller)
                .bind(STARRED)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
                Some(
                    sqlx::query_scalar::<_, Uuid>(
                        r#"
                        SELECT id FROM sidebar_sections
                        WHERE account_id = $1 AND workspace_id = $2 AND kind = 'starred'
                        "#,
                    )
                    .bind(caller)
                    .bind(workspace)
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(AppError::from)?,
                )
            }
            (None, _) => None,
        };
        // Unstarring takes it out only if it is in Starred.
        if input.starred == Some(false) {
            sqlx::query(
                r#"
                DELETE FROM sidebar_items item
                USING sidebar_sections section
                WHERE item.account_id = $1 AND item.conversation_id = $2
                  AND section.id = item.section_id AND section.kind = 'starred'
                "#,
            )
            .bind(caller)
            .bind(conversation_id)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        } else if let Some(section) = section {
            let count = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM sidebar_items WHERE account_id = $1 AND workspace_id = $2",
            )
            .bind(caller)
            .bind(workspace)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
            if count >= MAX_ITEMS {
                return Err(OpError::invalid(
                    "conversationId",
                    "Your sections hold 500 conversations here already.",
                ));
            }
            // Out of wherever it was, then in at its place, the rest
            // making room.
            sqlx::query("DELETE FROM sidebar_items WHERE account_id = $1 AND conversation_id = $2")
                .bind(caller)
                .bind(conversation_id)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
            let position = match input.position {
                Some(position) => {
                    // The position is an index in the section as shown, so
                    // the rest are numbered 0, 1, 2… again (closing the gap
                    // the move left), skipping the one it takes.
                    sqlx::query(
                        r#"
                        UPDATE sidebar_items item SET position = ordered.index
                            + CASE WHEN ordered.index >= $2 THEN 1 ELSE 0 END
                        FROM (
                            SELECT conversation_id,
                                   (row_number() OVER (ORDER BY position, conversation_id) - 1)::int AS index
                            FROM sidebar_items WHERE section_id = $1
                        ) ordered
                        WHERE item.section_id = $1 AND item.conversation_id = ordered.conversation_id
                        "#,
                    )
                    .bind(section)
                    .bind(position)
                    .execute(&mut **tx)
                    .await
                    .map_err(AppError::from)?;
                    position
                }
                None => sqlx::query_scalar::<_, i32>(
                    "SELECT COALESCE(max(position) + 1, 0) FROM sidebar_items WHERE section_id = $1",
                )
                .bind(section)
                .fetch_one(&mut **tx)
                .await
                .map_err(AppError::from)?,
            };
            sqlx::query(
                r#"
                INSERT INTO sidebar_items (workspace_id, section_id, account_id, conversation_id, position)
                VALUES ($1, $2, $3, $4, $5)
                "#,
            )
            .bind(workspace)
            .bind(section)
            .bind(caller)
            .bind(conversation_id)
            .bind(position)
            .execute(&mut **tx)
            .await
            .map_err(AppError::from)?;
        } else {
            sqlx::query("DELETE FROM sidebar_items WHERE account_id = $1 AND conversation_id = $2")
                .bind(caller)
                .bind(conversation_id)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
        }
        call.audit_target("conversation", conversation_id, serde_json::json!({}));
        announce(call).await?;
        sidebar(call).await
    }
}
