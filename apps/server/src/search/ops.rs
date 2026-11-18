//! Saved searches and jump-to (C2.26).
//!
//! A saved search is a query in the search grammar that one account keeps in
//! one workspace; what it finds is read live, with the reader's access, each
//! time it runs. `place_search` (jump-to) finds places by name for the command menu:
//! conversations, people, spaces, tasks by key and pages by title, each
//! only where the caller can open it.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    addresses::{Addresses, Place, Target},
    error::AppError,
    modules::state::ProjectModules,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target as OpTarget,
        Targeted, context::Edge, refs::IdRef,
    },
    spaces::SpaceKind,
};

/// The most searches one account saves in a workspace.
const MAX_SAVED: i64 = 50;
/// The most places `place_search` returns.
const MAX_JUMPS: usize = 20;

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<SavedSearchList>()
        .add::<SavedSearchSet>()
        .add::<PlaceSearch>();
}

/// A search someone saved.
#[derive(Debug, Serialize, JsonSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct SavedSearch {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
    /// The query, in the search grammar, as saved.
    pub query: String,
    pub position: i32,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SavedSearches {
    pub searches: Vec<SavedSearch>,
}

async fn saved(call: &mut OpCall<'_>) -> Result<SavedSearches, OpError> {
    let (account, workspace) = (call.account_id(), call.ctx.workspace_id);
    let tx = call.tx().await?;
    let searches = sqlx::query_as::<_, SavedSearch>(
        r#"
        SELECT id, name, query, position
        FROM saved_searches
        WHERE account_id = $1 AND workspace_id = $2
        ORDER BY position, lower(name), id
        "#,
    )
    .bind(account)
    .bind(workspace)
    .fetch_all(&mut **tx)
    .await
    .map_err(AppError::from)?;
    Ok(SavedSearches { searches })
}

// ---------------------------------------------------------------------------
// saved_search_list

pub struct SavedSearchList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedSearchListInput {}

impl Targeted for SavedSearchListInput {
    fn target(&self) -> OpTarget {
        OpTarget::Workspace(None)
    }
}

impl Operation for SavedSearchList {
    const NAME: &'static str = "saved_search_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List saved searches";
    const DESCRIPTION: &'static str = "Your own saved searches in this workspace, in order. Each \
        is a query in the search grammar; run one with search.";

    type Input = SavedSearchListInput;
    type Output = SavedSearches;

    async fn run(
        call: &mut OpCall<'_>,
        _input: SavedSearchListInput,
    ) -> Result<SavedSearches, OpError> {
        saved(call).await
    }
}

// ---------------------------------------------------------------------------
// saved_search_set

pub struct SavedSearchSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedSearchSetInput {
    /// The saved search to change; left out, a new one is saved (give its
    /// name and query).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_search_id: Option<IdRef>,
    /// Its name, 1 to 80 characters on one line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The query, in the search grammar (as search takes it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Its place among your saved searches, from 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
    /// true removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete: Option<bool>,
}

impl Targeted for SavedSearchSetInput {
    fn target(&self) -> OpTarget {
        OpTarget::Workspace(None)
    }
}

fn saved_name(name: &str) -> Result<String, OpError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(OpError::invalid(
            "name",
            "A saved search's name is 1 to 80 characters, on one line.",
        ));
    }
    Ok(name.to_owned())
}

fn saved_query(query: &str) -> Result<String, OpError> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > super::MAX_QUERY_CHARACTERS {
        return Err(OpError::invalid("query", "Save 1 to 200 characters."));
    }
    let parsed = super::grammar::parse(query);
    if !parsed.errors.is_empty() {
        return Err(OpError::invalid(
            "query",
            "A filter in the query takes other values. The filters are from:, in:, during:, \
             on:, before:, after:, has:, is: and kind:.",
        ));
    }
    Ok(query.to_owned())
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
        crate::outbox::OutboxEventPayload::SavedSearchesChanged(
            crate::outbox::SavedSearchesChangedPayload { actor_id: account },
        ),
    )
    .await?;
    Ok(())
}

fn name_taken(error: sqlx::Error) -> OpError {
    match AppError::from(error) {
        AppError::Conflict(..) | AppError::Database(_) => {
            OpError::invalid("name", "You have a saved search with that name already.")
        }
        other => other.into(),
    }
}

impl Operation for SavedSearchSet {
    const NAME: &'static str = "saved_search_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Save a search";
    const DESCRIPTION: &'static str = "Save a search in this workspace (leave savedSearchId out \
        and give a name and a query in the search grammar), or change one: its name, query or \
        place. delete: true removes it. Saved searches are your own. Returns them all.";
    const EMITS: &'static [&'static str] = &["saved_searches.changed"];

    type Input = SavedSearchSetInput;
    type Output = SavedSearches;

    async fn run(
        call: &mut OpCall<'_>,
        input: SavedSearchSetInput,
    ) -> Result<SavedSearches, OpError> {
        let (account, workspace) = (call.account_id(), call.ctx.workspace_id);
        let name = input.name.as_deref().map(saved_name).transpose()?;
        let query = input.query.as_deref().map(saved_query).transpose()?;
        if input.position.is_some_and(|position| position < 0) {
            return Err(OpError::invalid("position", "A position is 0 or more."));
        }
        let tx = call.tx().await?;
        let Some(id) = input.saved_search_id.map(IdRef::id) else {
            if input.delete == Some(true) {
                return Err(OpError::invalid(
                    "savedSearchId",
                    "Say which saved search to remove.",
                ));
            }
            let (Some(name), Some(query)) = (name, query) else {
                return Err(OpError::invalid(
                    "name",
                    "Give the new saved search a name and a query.",
                ));
            };
            let count = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM saved_searches WHERE account_id = $1 AND workspace_id = $2",
            )
            .bind(account)
            .bind(workspace)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
            if count >= MAX_SAVED {
                return Err(OpError::invalid(
                    "name",
                    "You have 50 saved searches here already; remove one first.",
                ));
            }
            let id = Uuid::now_v7();
            sqlx::query(
                r#"
                INSERT INTO saved_searches (id, workspace_id, account_id, name, query, position)
                VALUES ($1, $2, $3, $4, $5,
                        COALESCE($6, (SELECT COALESCE(max(position) + 1, 0) FROM saved_searches
                                      WHERE account_id = $3 AND workspace_id = $2)))
                "#,
            )
            .bind(id)
            .bind(workspace)
            .bind(account)
            .bind(&name)
            .bind(&query)
            .bind(input.position)
            .execute(&mut **tx)
            .await
            .map_err(name_taken)?;
            call.audit_target("saved_search", id, serde_json::json!({}));
            announce(call).await?;
            return saved(call).await;
        };
        // Someone else's, or another workspace's, is not found.
        let found = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM saved_searches WHERE id = $1 AND account_id = $2 AND workspace_id = $3 \
             FOR UPDATE",
        )
        .bind(id)
        .bind(account)
        .bind(workspace)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if found.is_none() {
            return Err(OpError::not_found("That saved search"));
        }
        if input.delete == Some(true) {
            sqlx::query("DELETE FROM saved_searches WHERE id = $1")
                .bind(id)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
        } else {
            sqlx::query(
                r#"
                UPDATE saved_searches
                SET name = COALESCE($2, name),
                    query = COALESCE($3, query),
                    position = COALESCE($4, position),
                    updated_at = now()
                WHERE id = $1
                "#,
            )
            .bind(id)
            .bind(&name)
            .bind(&query)
            .bind(input.position)
            .execute(&mut **tx)
            .await
            .map_err(name_taken)?;
        }
        call.audit_target(
            "saved_search",
            id,
            serde_json::json!({"deleted": input.delete == Some(true)}),
        );
        announce(call).await?;
        saved(call).await
    }
}

// ---------------------------------------------------------------------------
// place_search

pub struct PlaceSearch;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlaceSearchInput {
    /// A name, or part of one, or a task's key (AWD-12).
    pub q: String,
}

impl Targeted for PlaceSearchInput {
    fn target(&self) -> OpTarget {
        OpTarget::Workspace(None)
    }
}

/// A place to jump to.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlaceHit {
    /// space, channel, dm, group, person, task or page.
    pub kind: String,
    #[schemars(with = "String")]
    pub id: Uuid,
    pub title: String,
    /// Where it is: the space, or a task's key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Its address in the web app; none for a person (open your DM with
    /// them, or their profile).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip)]
    score: u8,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlaceSearchResults {
    pub hits: Vec<PlaceHit>,
}

/// 3 for the whole name, 2 for its start, 1 for a word's start, 0 otherwise.
fn score(name: &str, typed: &str) -> u8 {
    let name = name.to_lowercase();
    if name == typed {
        3
    } else if name.starts_with(typed) {
        2
    } else if name
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word.starts_with(typed))
    {
        1
    } else {
        0
    }
}

/// `AWD-12` as its key and number.
fn task_key(typed: &str) -> Option<(String, i64)> {
    let (key, number) = typed.rsplit_once('-')?;
    let ok = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric());
    let number = number.parse::<i64>().ok().filter(|number| *number > 0)?;
    ok.then(|| (key.to_ascii_uppercase(), number))
}

impl Operation for PlaceSearch {
    const NAME: &'static str = "place_search";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Jump to a place";
    const DESCRIPTION: &'static str = "Find places in this workspace by name: channels, DMs and \
        groups, people, spaces, tasks by key (AWD-12) and pages by title, at most 20, best \
        matches first, each only where you can open it. For words inside messages and \
        documents, use search.";

    type Input = PlaceSearchInput;
    type Output = PlaceSearchResults;

    async fn run(
        call: &mut OpCall<'_>,
        input: PlaceSearchInput,
    ) -> Result<PlaceSearchResults, OpError> {
        let typed = input.q.trim().to_lowercase();
        if typed.is_empty() || typed.chars().count() > 80 {
            return Err(OpError::invalid("q", "Type 1 to 80 characters."));
        }
        let (account, workspace) = (call.account_id(), call.ctx.workspace_id);
        let connection = match &call.ctx.edge {
            Edge::Mcp { connection_id, .. } | Edge::PublicApi { connection_id, .. } => {
                *connection_id
            }
            _ => None,
        };
        let pattern = format!(
            "%{}%",
            typed
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let addresses = Addresses::of(call.state);
        let pool = call.state.pool.clone();
        // The spaces this caller reaches here, and where each module shows.
        let reachable = super::reachable_projects(&pool, account, connection).await?;
        let tx = call.tx().await?;
        let spaces = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM projects WHERE id = ANY($1) AND organization_id = $2",
        )
        .bind(&reachable)
        .bind(workspace)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let modules = ProjectModules::load(tx, account, &spaces).await?;
        // Each kind shows where the module whose manifest lists it is on.
        let on = |kind: super::SearchKind, spaces: &[Uuid]| {
            kind.module()
                .map(|module| modules.with(spaces, module))
                .unwrap_or_default()
        };
        let task_spaces = on(
            super::SearchKind::Tasks,
            &super::projects_showing_tasks(&pool, account, &spaces).await?,
        );
        let page_spaces = on(super::SearchKind::Pages, &spaces);
        let chat_spaces = modules.with_conversations(&spaces);
        let tx = call.tx().await?;
        let mut hits = Vec::new();

        // Spaces by name or key, that the caller may read.
        let rows = sqlx::query_as::<_, (Uuid, String, String)>(&format!(
            r#"
            SELECT space.id, space.name, space.kind
            FROM projects space
            WHERE space.organization_id = $1
              AND space.archived_at IS NULL
              AND {readable}
              AND actor_can_access_project($2, $5, space.id)
              AND (lower(space.name) LIKE $3 ESCAPE '\' OR lower(space.project_key) = $4)
            ORDER BY lower(space.name)
            LIMIT 20
            "#,
            readable = crate::spaces::access::readable_sql("space.id", "$2"),
        ))
        .bind(workspace)
        .bind(account)
        .bind(&pattern)
        .bind(&typed)
        .bind(connection)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        for (id, name, kind) in rows {
            if SpaceKind::from_stored(&kind).is_none() {
                continue;
            }
            hits.push(PlaceHit {
                kind: "space".to_owned(),
                id,
                score: score(&name, &typed).max(1),
                detail: None,
                path: Some(addresses.in_workspace(workspace, &Target::space(id, Place::Home))),
                title: name,
            });
        }

        // Channels by name; DMs and groups by the other people in them.
        let rows = sqlx::query_as::<_, (Uuid, String, String, Uuid, String, Option<String>)>(
            r#"
            SELECT conversation.id, conversation.kind, conversation.name, space.id, space.name,
                   (SELECT string_agg(person.display_name, ', ' ORDER BY person.display_name)
                    FROM conversation_members them
                    JOIN project_memberships person
                      ON person.project_id = them.project_id AND person.actor_id = them.actor_id
                    WHERE them.conversation_id = conversation.id AND them.actor_id <> $2) AS people
            FROM conversations conversation
            JOIN projects space ON space.id = conversation.project_id
            WHERE conversation.project_id = ANY($1)
              AND conversation.kind IN ('channel', 'dm', 'group')
              AND conversation.archived_at IS NULL
              AND conversation_visible_to(conversation.id, $2)
              AND (lower(conversation.name) LIKE $3 ESCAPE '\'
                   OR (conversation.kind <> 'channel' AND EXISTS (
                       SELECT 1
                       FROM conversation_members them
                       JOIN project_memberships person
                         ON person.project_id = them.project_id
                        AND person.actor_id = them.actor_id
                       WHERE them.conversation_id = conversation.id
                         AND them.actor_id <> $2
                         AND (lower(person.display_name) LIKE $3 ESCAPE '\'
                              OR lower(person.handle) LIKE $3 ESCAPE '\'))))
            ORDER BY conversation.last_message_sequence DESC
            LIMIT 20
            "#,
        )
        .bind(&chat_spaces)
        .bind(account)
        .bind(&pattern)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        for (id, kind, name, space, space_name, people) in rows {
            let channel = kind == "channel";
            // A DM is named by the other person in it (or is your note to
            // self); a group by its name, else its people.
            let people = people.filter(|people| !people.trim().is_empty());
            let title = if channel {
                format!("#{name}")
            } else if kind == "dm" {
                people.unwrap_or_else(|| "Note to self".to_owned())
            } else if name.trim().is_empty() {
                people.unwrap_or_default()
            } else {
                name.clone()
            };
            let target = if channel {
                Target::space(space, Place::Channel(id))
            } else {
                Target::Direct { id, thread: None }
            };
            hits.push(PlaceHit {
                score: score(if channel { &name } else { &title }, &typed).max(1),
                kind,
                id,
                title,
                detail: channel.then_some(space_name),
                path: Some(addresses.in_workspace(workspace, &target)),
            });
        }

        // People who share a space here with the caller.
        let rows = sqlx::query_as::<_, (Uuid, String, Option<String>)>(
            r#"
            SELECT DISTINCT ON (person.actor_id) person.actor_id, person.display_name, person.handle
            FROM project_memberships person
            WHERE person.project_id = ANY($1)
              AND person.enabled
              AND person.actor_id <> $2
              AND (lower(person.display_name) LIKE $3 ESCAPE '\'
                   OR lower(person.handle) LIKE $3 ESCAPE '\')
            ORDER BY person.actor_id
            LIMIT 20
            "#,
        )
        .bind(&spaces)
        .bind(account)
        .bind(&pattern)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        for (id, name, handle) in rows {
            let by_handle = handle
                .as_deref()
                .map_or(0, |handle| score(handle, typed.trim_start_matches('@')));
            hits.push(PlaceHit {
                kind: "person".to_owned(),
                id,
                score: score(&name, &typed).max(by_handle).max(1),
                detail: handle.map(|handle| format!("@{handle}")),
                path: None,
                title: name,
            });
        }

        // A task by its key.
        if let Some((key, number)) = task_key(&typed) {
            let rows = sqlx::query_as::<_, (Uuid, Uuid, String)>(
                r#"
                SELECT item.id, item.project_id, item.title
                FROM task_rows item
                JOIN projects project ON project.id = item.project_id
                WHERE item.project_id = ANY($1)
                  AND upper(project.project_key) = $2
                  AND item.number = $3
                  AND item.archived_at IS NULL
                "#,
            )
            .bind(&task_spaces)
            .bind(&key)
            .bind(number)
            .fetch_all(&mut **tx)
            .await
            .map_err(AppError::from)?;
            for (id, space, title) in rows {
                hits.push(PlaceHit {
                    kind: "task".to_owned(),
                    id,
                    title,
                    score: 4,
                    detail: Some(format!("{key}-{number}")),
                    path: Some(addresses.in_workspace(workspace, &Target::task(space, id))),
                });
            }
        }

        // Pages by title.
        let rows = sqlx::query_as::<_, (Uuid, Uuid, String, String)>(
            r#"
            SELECT page.id, page.project_id, page.title, project.name
            FROM project_pages page
            JOIN projects project ON project.id = page.project_id
            WHERE page.project_id = ANY($1)
              AND page.archived_at IS NULL
              AND lower(page.title) LIKE $2 ESCAPE '\'
            ORDER BY page.updated_at DESC
            LIMIT 20
            "#,
        )
        .bind(&page_spaces)
        .bind(&pattern)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        for (id, space, title, space_name) in rows {
            hits.push(PlaceHit {
                kind: "page".to_owned(),
                id,
                score: score(&title, &typed).max(1),
                title,
                detail: Some(space_name),
                path: Some(addresses.in_workspace(workspace, &Target::page(space, id))),
            });
        }

        // Best matches first; ties keep the order above.
        hits.sort_by_key(|hit| std::cmp::Reverse(hit.score));
        hits.truncate(MAX_JUMPS);
        Ok(PlaceSearchResults { hits })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_whole_names_over_starts_over_words() {
        assert_eq!(score("Design", "design"), 3);
        assert_eq!(score("Design review", "design"), 2);
        assert_eq!(score("Spring design", "design"), 1);
        assert_eq!(score("Redesign", "design"), 0);
        assert_eq!(task_key("awd-12"), Some(("AWD".to_owned(), 12)));
        assert_eq!(task_key("design-notes"), None);
        assert_eq!(task_key("-12"), None);
    }
}
