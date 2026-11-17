//! Subscriptions at subscribe time (K5.8): what a connection may hear is
//! worked out when it opens and again when access changes, not per event.
//! Routing an event is then a lookup in the connection's own set, with no
//! database query, for every audience but `Thing` (a workspace's own board,
//! which is checked as it comes).
//!
//! A set goes stale only between an access change and the event that says
//! so; the tail delivers events in position order, so the connection
//! recomputes before it routes anything after the change. A recompute that
//! fails closes the connection (fail closed), and every set is recomputed
//! every few minutes as a backstop. The worst a stale set does is let a
//! removed member hear event ids briefly: content always comes through
//! HTTP, which checks access again.

use std::collections::HashSet;

use sqlx::PgPool;
use uuid::Uuid;

use crate::{error::AppError, events::LoggedEvent};

/// How an event is routed: from the log's envelope (K3's audience), so no
/// lookup is needed to decide who hears it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Route {
    pub workspace: Uuid,
    pub space: Option<Uuid>,
    pub audience: RouteAudience,
    /// The type, to see whether it changes who may hear what.
    pub event_type: String,
    /// The account an access change is about, if it names one.
    pub about_account: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RouteAudience {
    Workspace,
    Space,
    Conversation {
        root: Uuid,
        restricted: bool,
    },
    Account(Uuid),
    /// Checked per event, by the thing's own rule (a workspace's board).
    Thing(Uuid),
}

/// Event types after which a connection works out its set again, when the
/// event is about its account (or, for a restricted conversation, may have
/// let it in).
const ACCESS_TYPES: &[&str] = &[
    "space.member.changed",
    "space.modules_changed",
    "conversation.created",
    "conversation.updated",
    "conversation.archived",
    "conversation.member.removed",
    "workspace.lifecycle_changed",
];

impl Route {
    /// The route of a logged event; None when its audience is incomplete,
    /// which no connection then hears.
    pub(crate) fn of_logged(event: &LoggedEvent) -> Option<Self> {
        let audience = match event.audience.as_str() {
            "workspace" => RouteAudience::Workspace,
            "space" => RouteAudience::Space,
            "conversation" => RouteAudience::Conversation {
                root: event.conversation_id?,
                restricted: event.conversation_restricted?,
            },
            "account" => RouteAudience::Account(event.audience_account_id?),
            "thing" => RouteAudience::Thing(event.access_thing_id.or(event.thing_id)?),
            _ => return None,
        };
        let about_account = event
            .audience_account_id
            .or_else(|| account_named(&event.payload));
        Some(Self {
            workspace: event.workspace_id,
            space: event.space_id,
            audience,
            event_type: event.event_type.clone(),
            about_account,
        })
    }
}

fn account_named(payload: &serde_json::Value) -> Option<Uuid> {
    ["actorId", "accountId", "memberActorId"]
        .iter()
        .find_map(|key| payload.get(*key).and_then(serde_json::Value::as_str))
        .and_then(|value| Uuid::parse_str(value).ok())
}

/// Whether a connection hears an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hearing {
    Yes,
    No,
    /// Ask the board's own rule now.
    IfBoardVisible(Uuid),
}

/// What one connection may hear.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SubscriptionSet {
    pub account: Uuid,
    /// Workspaces it is an enabled member of.
    pub workspaces: HashSet<Uuid>,
    /// Spaces whose events it hears: today's rule, an enabled membership
    /// of a space that isn't archived, in a workspace it is enabled in,
    /// with a role that may subscribe.
    pub spaces: HashSet<Uuid>,
    /// Restricted root conversations (DMs, groups, private channels) it
    /// may read.
    pub restricted: HashSet<Uuid>,
}

impl SubscriptionSet {
    pub(crate) fn hears(&self, route: &Route) -> Hearing {
        let heard = match route.audience {
            RouteAudience::Workspace => self.workspaces.contains(&route.workspace),
            RouteAudience::Space => route
                .space
                .is_some_and(|space| self.spaces.contains(&space)),
            RouteAudience::Conversation { root, restricted } => {
                if restricted {
                    self.restricted.contains(&root)
                } else {
                    route
                        .space
                        .is_some_and(|space| self.spaces.contains(&space))
                }
            }
            RouteAudience::Account(account) => account == self.account,
            RouteAudience::Thing(thing) => {
                return if self.workspaces.contains(&route.workspace) {
                    Hearing::IfBoardVisible(thing)
                } else {
                    Hearing::No
                };
            }
        };
        if heard { Hearing::Yes } else { Hearing::No }
    }

    /// Whether this event may change what the connection hears, so it
    /// must work out its set again before routing anything after it.
    pub(crate) fn changed_by(&self, route: &Route) -> bool {
        if !ACCESS_TYPES.contains(&route.event_type.as_str()) {
            return false;
        }
        if route.about_account == Some(self.account) {
            return true;
        }
        match route.audience {
            // A restricted conversation made, changed or archived in one of
            // its workspaces: it may have been let in, or out.
            RouteAudience::Conversation {
                root,
                restricted: true,
            } => self.workspaces.contains(&route.workspace) || self.restricted.contains(&root),
            // A space's modules or a workspace's state changed where it is.
            RouteAudience::Space | RouteAudience::Workspace => {
                route.event_type != "space.member.changed"
                    && (self.workspaces.contains(&route.workspace)
                        || route
                            .space
                            .is_some_and(|space| self.spaces.contains(&space)))
            }
            _ => false,
        }
    }
}

/// One space membership, as `compute` reads it.
#[derive(sqlx::FromRow)]
struct Holding {
    space: Uuid,
    workspace: Uuid,
    role: String,
}

/// Works out what `account` may hear now. `allows` is the role rule for
/// subscribing to a space (the realtime policy).
pub(crate) async fn compute(
    pool: &PgPool,
    account: Uuid,
    allows: impl Fn(&str, Uuid) -> bool,
) -> Result<SubscriptionSet, AppError> {
    let mut connection = crate::db::scoped_tx(pool).await?;
    let workspaces = sqlx::query_scalar::<_, Uuid>(
        "SELECT organization_id FROM organization_memberships WHERE actor_id = $1 AND enabled",
    )
    .bind(account)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .collect::<HashSet<_>>();
    let holdings = sqlx::query_as::<_, Holding>(
        r#"
        SELECT membership.project_id AS space, space.organization_id AS workspace,
               membership.role
        FROM project_memberships membership
        JOIN projects space ON space.id = membership.project_id
        WHERE membership.actor_id = $1 AND membership.enabled
          AND space.archived_at IS NULL
        "#,
    )
    .bind(account)
    .fetch_all(&mut *connection)
    .await?;
    let spaces = holdings
        .into_iter()
        .filter(|holding| workspaces.contains(&holding.workspace))
        .filter(|holding| allows(&holding.role, holding.space))
        .map(|holding| holding.space)
        .collect::<HashSet<_>>();
    // Restricted roots it belongs to, each passed through the one gate.
    let restricted = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT conversation.id
        FROM conversation_members member
        JOIN conversations conversation ON conversation.id = member.conversation_id
        WHERE member.actor_id = $1
          AND conversation.parent_channel_id IS NULL
          AND conversation.visibility <> 'project'
          AND conversation_visible_to(conversation.id, $1)
        "#,
    )
    .bind(account)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .collect::<HashSet<_>>();
    Ok(SubscriptionSet {
        account,
        workspaces,
        spaces,
        restricted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(audience: RouteAudience, space: Option<Uuid>, workspace: Uuid) -> Route {
        Route {
            workspace,
            space,
            audience,
            event_type: "conversation.message_posted".to_owned(),
            about_account: None,
        }
    }

    #[test]
    fn each_audience_reaches_only_its_own() {
        let (me, workspace, space, other_space, group, other_group) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let set = SubscriptionSet {
            account: me,
            workspaces: HashSet::from([workspace]),
            spaces: HashSet::from([space]),
            restricted: HashSet::from([group]),
        };
        let other_workspace = Uuid::now_v7();
        for (audience, space_of, workspace_of, heard) in [
            (RouteAudience::Space, Some(space), workspace, Hearing::Yes),
            (
                RouteAudience::Space,
                Some(other_space),
                workspace,
                Hearing::No,
            ),
            (RouteAudience::Workspace, None, workspace, Hearing::Yes),
            (RouteAudience::Workspace, None, other_workspace, Hearing::No),
            (
                RouteAudience::Conversation {
                    root: Uuid::now_v7(),
                    restricted: false,
                },
                Some(space),
                workspace,
                Hearing::Yes,
            ),
            (
                RouteAudience::Conversation {
                    root: group,
                    restricted: true,
                },
                Some(other_space),
                workspace,
                Hearing::Yes,
            ),
            // A restricted conversation in a space it hears is still
            // members only.
            (
                RouteAudience::Conversation {
                    root: other_group,
                    restricted: true,
                },
                Some(space),
                workspace,
                Hearing::No,
            ),
            (RouteAudience::Account(me), None, workspace, Hearing::Yes),
            (
                RouteAudience::Account(Uuid::now_v7()),
                None,
                workspace,
                Hearing::No,
            ),
            (
                RouteAudience::Thing(group),
                None,
                workspace,
                Hearing::IfBoardVisible(group),
            ),
            (
                RouteAudience::Thing(group),
                None,
                other_workspace,
                Hearing::No,
            ),
        ] {
            assert_eq!(
                set.hears(&route(audience, space_of, workspace_of)),
                heard,
                "{audience:?}"
            );
        }
    }

    #[test]
    fn access_changes_about_the_account_or_its_restricted_conversations_recompute() {
        let (me, workspace, space) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let set = SubscriptionSet {
            account: me,
            workspaces: HashSet::from([workspace]),
            spaces: HashSet::from([space]),
            restricted: HashSet::new(),
        };
        let mut membership = route(RouteAudience::Space, Some(space), workspace);
        membership.event_type = "space.member.changed".to_owned();
        assert!(!set.changed_by(&membership), "someone else's membership");
        membership.about_account = Some(me);
        assert!(set.changed_by(&membership), "its own membership");
        let mut group = route(
            RouteAudience::Conversation {
                root: Uuid::now_v7(),
                restricted: true,
            },
            Some(space),
            workspace,
        );
        group.event_type = "conversation.updated".to_owned();
        assert!(set.changed_by(&group), "a group it may have joined");
        let mut elsewhere = group.clone();
        elsewhere.workspace = Uuid::now_v7();
        assert!(!set.changed_by(&elsewhere), "a group in another workspace");
        let message = route(
            RouteAudience::Conversation {
                root: Uuid::now_v7(),
                restricted: true,
            },
            Some(space),
            workspace,
        );
        assert!(!set.changed_by(&message), "a message never recomputes");
    }
}
