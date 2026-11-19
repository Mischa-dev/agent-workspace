//! Saved inbox views (C2.11): tabs a person keeps on their inbox, each a
//! filter (which part of the inbox, which kinds, which spaces) and a
//! density. A view is its owner's alone, in one workspace; the rows it shows
//! are read live with the reader's access, so a view filtered to a space
//! never shows another space's rows.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{Category, View};
use crate::{
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target as OpTarget,
        Targeted, context::Edge, refs::IdRef,
    },
};

/// The most views one account saves in a workspace.
const MAX_VIEWS: i64 = 20;
/// The most spaces one view filters to.
const MAX_SPACES: usize = 20;

pub fn register_ops(registry: &mut Registry) {
    registry.add::<InboxViewList>().add::<InboxViewSet>();
}

/// What a view shows. Unknown keys are refused.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InboxFilter {
    /// Which part of the inbox: inbox (the default), unread, snoozed or
    /// archived.
    #[serde(default)]
    pub view: View,
    /// Which kinds: all (the default), mentions, threads, tasks, runs,
    /// pull_requests or usage.
    #[serde(default)]
    pub category: Category,
    /// Only these spaces (up to 20); left out, every space.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(with = "Vec<String>")]
    pub spaces: Vec<Uuid>,
}

/// How densely a view lists its rows.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    #[default]
    Detailed,
    Compact,
}

impl Density {
    fn stored(self) -> &'static str {
        match self {
            Self::Detailed => "detailed",
            Self::Compact => "compact",
        }
    }

    fn from_stored(value: &str) -> Self {
        if value == "compact" {
            Self::Compact
        } else {
            Self::Detailed
        }
    }
}

/// A saved inbox view.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxViewItem {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub name: String,
    pub position: i32,
    pub filter: InboxFilter,
    pub density: Density,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InboxViews {
    pub views: Vec<InboxViewItem>,
}

/// The `spaces` the inbox list takes: comma-separated ids, at most 20.
pub(crate) fn parse_spaces(value: &str) -> Result<Vec<Uuid>, AppError> {
    let spaces = value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            Uuid::parse_str(part)
                .map_err(|_| AppError::Invalid("spaces takes space ids, comma-separated"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if spaces.is_empty() || spaces.len() > MAX_SPACES {
        return Err(AppError::Invalid("spaces takes 1 to 20 space ids"));
    }
    Ok(spaces)
}

async fn views(call: &mut OpCall<'_>) -> Result<InboxViews, OpError> {
    let (account, workspace) = (call.account_id(), call.ctx.workspace_id);
    let tx = call.tx().await?;
    let rows = sqlx::query_as::<_, (Uuid, String, i32, serde_json::Value, String)>(
        r#"
        SELECT id, name, position, filter, density
        FROM inbox_views
        WHERE account_id = $1 AND workspace_id = $2
        ORDER BY position, lower(name), id
        "#,
    )
    .bind(account)
    .bind(workspace)
    .fetch_all(&mut **tx)
    .await
    .map_err(AppError::from)?;
    Ok(InboxViews {
        views: rows
            .into_iter()
            .map(|(id, name, position, filter, density)| InboxViewItem {
                id,
                name,
                position,
                // A filter written by an older shape reads as the whole inbox.
                filter: serde_json::from_value(filter).unwrap_or_default(),
                density: Density::from_stored(&density),
            })
            .collect(),
    })
}

// ---------------------------------------------------------------------------
// inbox_view_list

pub struct InboxViewList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InboxViewListInput {}

impl Targeted for InboxViewListInput {
    fn target(&self) -> OpTarget {
        OpTarget::Workspace(None)
    }
}

impl Operation for InboxViewList {
    const NAME: &'static str = "inbox_view_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "inbox";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List saved inbox views";
    const DESCRIPTION: &'static str = "Your own saved inbox views in this workspace, in order: \
        each a name, a filter (view, category, spaces) and a density. Read what one shows with \
        inbox_list.";

    type Input = InboxViewListInput;
    type Output = InboxViews;

    async fn run(call: &mut OpCall<'_>, _input: InboxViewListInput) -> Result<InboxViews, OpError> {
        views(call).await
    }
}

// ---------------------------------------------------------------------------
// inbox_view_set

pub struct InboxViewSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InboxViewSetInput {
    /// The view to change; left out, a new one is saved (give its name and
    /// filter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox_view_id: Option<IdRef>,
    /// Its name, 1 to 40 characters on one line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What it shows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<InboxFilter>,
    /// detailed (the default) or compact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub density: Option<Density>,
    /// Its place among your views, from 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
    /// true removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete: Option<bool>,
}

impl Targeted for InboxViewSetInput {
    fn target(&self) -> OpTarget {
        OpTarget::Workspace(None)
    }
}

fn view_name(name: &str) -> Result<String, OpError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 || name.chars().any(char::is_control) {
        return Err(OpError::invalid(
            "name",
            "A view's name is 1 to 40 characters, on one line.",
        ));
    }
    Ok(name.to_owned())
}

/// Checks a filter's spaces are spaces of this workspace the caller can
/// open, and returns it ready to store.
async fn checked_filter(
    call: &mut OpCall<'_>,
    mut filter: InboxFilter,
) -> Result<serde_json::Value, OpError> {
    filter.spaces.sort_unstable();
    filter.spaces.dedup();
    if filter.spaces.len() > MAX_SPACES {
        return Err(OpError::invalid(
            "filter",
            "A view filters to at most 20 spaces.",
        ));
    }
    if !filter.spaces.is_empty() {
        let (account, workspace) = (call.account_id(), call.ctx.workspace_id);
        let connection = match &call.ctx.edge {
            Edge::Mcp { connection_id, .. } | Edge::PublicApi { connection_id, .. } => {
                *connection_id
            }
            _ => None,
        };
        let tx = call.tx().await?;
        let open = sqlx::query_scalar::<_, i64>(&format!(
            r#"
            SELECT count(*) FROM projects space
            WHERE space.id = ANY($1) AND space.organization_id = $2
              AND {readable}
              AND actor_can_access_project($3, $4, space.id)
            "#,
            readable = crate::spaces::access::readable_sql("space.id", "$3"),
        ))
        .bind(&filter.spaces)
        .bind(workspace)
        .bind(account)
        .bind(connection)
        .fetch_one(&mut **tx)
        .await
        .map_err(AppError::from)?;
        // A space elsewhere, or one the caller can't open, is not found.
        if usize::try_from(open).unwrap_or(0) != filter.spaces.len() {
            return Err(OpError::not_found("That space"));
        }
    }
    serde_json::to_value(&filter).map_err(|_| AppError::Internal("encode an inbox view").into())
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
        crate::outbox::OutboxEventPayload::InboxViewsChanged(
            crate::outbox::InboxViewsChangedPayload { actor_id: account },
        ),
    )
    .await?;
    Ok(())
}

fn name_taken(error: sqlx::Error) -> OpError {
    match AppError::from(error) {
        AppError::Conflict(..) | AppError::Database(_) => {
            OpError::invalid("name", "You have a view with that name already.")
        }
        other => other.into(),
    }
}

impl Operation for InboxViewSet {
    const NAME: &'static str = "inbox_view_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "inbox";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const AUTOMATION: bool = false;
    const TITLE: &'static str = "Save an inbox view";
    const DESCRIPTION: &'static str = "Save an inbox view in this workspace (leave inboxViewId \
        out and give a name and a filter: view, category and spaces), or change one: its name, \
        filter, density or place. delete: true removes it. Views are your own. Returns them all.";
    const EMITS: &'static [&'static str] = &["inbox_views.changed"];

    type Input = InboxViewSetInput;
    type Output = InboxViews;

    async fn run(call: &mut OpCall<'_>, input: InboxViewSetInput) -> Result<InboxViews, OpError> {
        let (account, workspace) = (call.account_id(), call.ctx.workspace_id);
        let name = input.name.as_deref().map(view_name).transpose()?;
        if input.position.is_some_and(|position| position < 0) {
            return Err(OpError::invalid("position", "A position is 0 or more."));
        }
        let filter = match input.filter {
            Some(filter) if input.delete != Some(true) => Some(checked_filter(call, filter).await?),
            _ => None,
        };
        let density = input.density.map(Density::stored);
        let tx = call.tx().await?;
        let Some(id) = input.inbox_view_id.map(IdRef::id) else {
            if input.delete == Some(true) {
                return Err(OpError::invalid("inboxViewId", "Say which view to remove."));
            }
            let (Some(name), Some(filter)) = (name, filter) else {
                return Err(OpError::invalid(
                    "name",
                    "Give the new view a name and a filter.",
                ));
            };
            let count = sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM inbox_views WHERE account_id = $1 AND workspace_id = $2",
            )
            .bind(account)
            .bind(workspace)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
            if count >= MAX_VIEWS {
                return Err(OpError::invalid(
                    "name",
                    "You have 20 inbox views here already; remove one first.",
                ));
            }
            let id = Uuid::now_v7();
            sqlx::query(
                r#"
                INSERT INTO inbox_views (id, workspace_id, account_id, name, filter, density, position)
                VALUES ($1, $2, $3, $4, $5, COALESCE($6, 'detailed'),
                        COALESCE($7, (SELECT COALESCE(max(position) + 1, 0) FROM inbox_views
                                      WHERE account_id = $3 AND workspace_id = $2)))
                "#,
            )
            .bind(id)
            .bind(workspace)
            .bind(account)
            .bind(&name)
            .bind(&filter)
            .bind(density)
            .bind(input.position)
            .execute(&mut **tx)
            .await
            .map_err(name_taken)?;
            call.audit_target("inbox_view", id, serde_json::json!({}));
            announce(call).await?;
            return views(call).await;
        };
        // Someone else's, or another workspace's, is not found.
        let found = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM inbox_views WHERE id = $1 AND account_id = $2 AND workspace_id = $3 \
             FOR UPDATE",
        )
        .bind(id)
        .bind(account)
        .bind(workspace)
        .fetch_optional(&mut **tx)
        .await
        .map_err(AppError::from)?;
        if found.is_none() {
            return Err(OpError::not_found("That inbox view"));
        }
        if input.delete == Some(true) {
            sqlx::query("DELETE FROM inbox_views WHERE id = $1")
                .bind(id)
                .execute(&mut **tx)
                .await
                .map_err(AppError::from)?;
        } else {
            sqlx::query(
                r#"
                UPDATE inbox_views
                SET name = COALESCE($2, name),
                    filter = COALESCE($3, filter),
                    density = COALESCE($4, density),
                    position = COALESCE($5, position),
                    updated_at = now()
                WHERE id = $1
                "#,
            )
            .bind(id)
            .bind(&name)
            .bind(&filter)
            .bind(density)
            .bind(input.position)
            .execute(&mut **tx)
            .await
            .map_err(name_taken)?;
        }
        call.audit_target(
            "inbox_view",
            id,
            serde_json::json!({"deleted": input.delete == Some(true)}),
        );
        announce(call).await?;
        views(call).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_filter_refuses_unknown_keys() {
        let filter: InboxFilter =
            serde_json::from_value(serde_json::json!({"view": "unread", "category": "mentions"}))
                .expect("a known filter");
        assert_eq!(filter.view, View::Unread);
        assert_eq!(filter.category, Category::Mentions);
        assert!(
            serde_json::from_value::<InboxFilter>(serde_json::json!({"view": "unread", "who": 1}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<InboxFilter>(serde_json::json!({"category": "everything"}))
                .is_err()
        );
    }

    #[test]
    fn spaces_are_ids_comma_separated() {
        let one = Uuid::now_v7();
        let two = Uuid::now_v7();
        assert_eq!(
            parse_spaces(&format!("{one}, {two}")).expect("two ids"),
            vec![one, two]
        );
        assert!(parse_spaces("not-an-id").is_err());
        assert!(parse_spaces("").is_err());
    }
}
