//! Web addresses (K4.18): the one place the server builds a link into the
//! web app, for notifications, mail, push, search, tool results and
//! anything else that hands a person or an agent somewhere to go.
//!
//! A link names a [`Target`] and the workspace it lives in. The server writes
//! today's shapes (`/projects/<id>/...`, `/messages/<id>`, `/inbox`) until
//! the web app serves workspace addresses; then [`Addresses::of`] changes to
//! [`Addresses::WORKSPACE`] in one step (K4.19), every link names its
//! workspace (`/w/<slug>/s/<space>/tasks/<id>`), and the old shape goes. The new shape is always worked out
//! from the old one by [`workspace_path`], the same function the resolver
//! (`GET /api/v1/links/resolve`, [`resolve`]) uses for links people already
//! have, so a built link and a resolved one can never disagree.
//!
//! The grammar is a contract with the web app (W1.14a's `app/href.ts` and
//! W1.14b's space routes): change a word here and there together.
//!
//! ```text
//! /w/{ws}/home | inbox | later | my-work | search | runs | runs/{id} | pages
//! /w/{ws}/whiteboards/{id} | messages | messages/agents | spaces | spaces/new | new
//! /w/{ws}/dm/{conversationId}                 a DM or group (and ?thread= its threads)
//! /w/{ws}/s/{spaceId}                         a space's home
//! /w/{ws}/s/{spaceId}/tasks[/{id}] | chat/{id} | chat/archived | threads/{id}
//!                   | pages[/{id}] | decisions | collections/{id} | sheets/{id}
//!                   | milestones | timeline | runs | planning/{next,suggestions,graph,import}
//! /w/{ws}/settings/{section}                  workspace settings
//! /w/{ws}/settings/spaces/{spaceId}[/...]     a space's settings
//! /settings/account/...                       no workspace
//! ```
//!
//! `{ws}` is the workspace's slug when the link knows it, else its id: the
//! web accepts `/w/<id>/...` and replaces it with the slug (K4 §3.10).

pub mod resolve;

use std::collections::HashMap;

use sqlx::PgConnection;
use uuid::Uuid;

use crate::{AppState, error::AppError};

/// The workspace a link lives in: its id, and its slug when known.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceAddress {
    pub id: Uuid,
    pub slug: Option<String>,
}

impl WorkspaceAddress {
    /// A workspace known by id only: its links use `/w/<id>/...`.
    pub fn id(id: Uuid) -> Self {
        Self { id, slug: None }
    }

    pub fn new(id: Uuid, slug: impl Into<String>) -> Self {
        Self {
            id,
            slug: Some(slug.into()),
        }
    }

    /// What goes after `/w/`.
    pub fn segment(&self) -> String {
        self.slug.clone().unwrap_or_else(|| self.id.to_string())
    }
}

/// Somewhere in the web app a link can lead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    Home,
    Inbox,
    Later,
    /// Later's scheduled messages.
    LaterScheduled,
    MyWork,
    Search,
    /// The DM list.
    Messages,
    /// The agents people can message.
    MessageAgents,
    /// Every run in the workspace.
    Runs,
    Run(Uuid),
    /// The list of spaces (projects).
    Spaces,
    /// Workspace pages.
    Pages,
    Whiteboard(Uuid),
    /// A DM or group, or (with `thread`) a thread under one.
    Direct {
        id: Uuid,
        thread: Option<Uuid>,
    },
    /// A place inside a space (a project).
    Space {
        space: Uuid,
        place: Place,
    },
    /// A section of the workspace's settings.
    WorkspaceSettings(&'static str),
    /// A section of a space's settings in the settings area.
    SpaceSettings {
        space: Uuid,
        section: &'static str,
    },
}

/// A place inside a space.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Place {
    Home,
    Channel(Uuid),
    Thread(Uuid),
    /// The archived conversations.
    ArchivedChat,
    Tasks,
    Task(Uuid),
    Pages,
    Page(Uuid),
    Decisions,
    Collection(Uuid),
    Record {
        collection: Uuid,
        record: Uuid,
    },
    Sheet(Uuid),
    Milestones,
    Timeline,
    /// The space's runs (launches).
    Runs,
    Next,
    Suggestions,
    Graph,
    PlanImport,
    /// The space's settings page.
    Settings,
    /// One repository in the space's settings.
    Repository(Uuid),
    PullRequest(Uuid),
    ExternalRequest(Uuid),
}

impl Target {
    pub fn space(space: Uuid, place: Place) -> Self {
        Self::Space { space, place }
    }

    pub fn task(space: Uuid, id: Uuid) -> Self {
        Self::space(space, Place::Task(id))
    }

    pub fn page(space: Uuid, id: Uuid) -> Self {
        Self::space(space, Place::Page(id))
    }

    /// Where a conversation opens: a DM or group (and a thread under one)
    /// with Messages, a channel or a thread in its space. `in_direct` says
    /// the conversation lives with Messages (today, the hidden Messages
    /// project); `parent` is a thread's conversation. A thread under a DM
    /// opens beside it when `open_thread` is set, else the DM itself opens.
    pub fn conversation(
        space: Uuid,
        kind: &str,
        id: Uuid,
        parent: Option<Uuid>,
        in_direct: bool,
        open_thread: bool,
    ) -> Self {
        if in_direct || matches!(kind, "dm" | "group") {
            return match (kind, parent) {
                ("thread", Some(parent)) => Self::Direct {
                    id: parent,
                    thread: open_thread.then_some(id),
                },
                _ => Self::Direct { id, thread: None },
            };
        }
        Self::space(
            space,
            if kind == "thread" {
                Place::Thread(id)
            } else {
                Place::Channel(id)
            },
        )
    }

    /// Today's address (the shape the web has served since before
    /// workspaces were in the address).
    pub fn legacy_path(&self) -> String {
        match *self {
            Self::Home => "/home".to_owned(),
            Self::Inbox => "/inbox".to_owned(),
            Self::Later => "/later".to_owned(),
            Self::LaterScheduled => "/later?view=scheduled".to_owned(),
            Self::MyWork => "/my-work".to_owned(),
            Self::Search => "/search".to_owned(),
            Self::Messages => "/messages".to_owned(),
            Self::MessageAgents => "/messages/agents".to_owned(),
            Self::Runs => "/runs".to_owned(),
            Self::Run(id) => format!("/runs/{id}"),
            Self::Spaces => "/projects".to_owned(),
            Self::Pages => "/pages".to_owned(),
            Self::Whiteboard(id) => format!("/whiteboards/{id}"),
            Self::Direct { id, thread: None } => format!("/messages/{id}"),
            Self::Direct {
                id,
                thread: Some(thread),
            } => format!("/messages/{id}?thread={thread}"),
            Self::WorkspaceSettings(section) => format!("/settings/workspace/{section}"),
            Self::SpaceSettings { space, section } => {
                format!("/settings/projects/{space}/{section}")
            }
            Self::Space { space, place } => {
                let root = format!("/projects/{space}");
                let tail = match place {
                    Place::Home => String::new(),
                    Place::Channel(id) => format!("/channels/{id}"),
                    Place::Thread(id) => format!("/threads/{id}"),
                    Place::ArchivedChat => "/conversations/archived".to_owned(),
                    Place::Tasks => "/work-items".to_owned(),
                    Place::Task(id) => format!("/work-items/{id}"),
                    Place::Pages => "/pages".to_owned(),
                    Place::Page(id) => format!("/pages/{id}"),
                    Place::Decisions => "/decisions".to_owned(),
                    Place::Collection(id) => format!("/collections/{id}"),
                    Place::Record { collection, record } => {
                        format!("/collections/{collection}?record={record}")
                    }
                    Place::Sheet(id) => format!("/sheets/{id}"),
                    Place::Milestones => "/milestones".to_owned(),
                    Place::Timeline => "/timeline".to_owned(),
                    Place::Runs => "/launches".to_owned(),
                    Place::Next => "/next".to_owned(),
                    Place::Suggestions => "/suggestions".to_owned(),
                    Place::Graph => "/graph".to_owned(),
                    Place::PlanImport => "/plan-import".to_owned(),
                    Place::Settings => "/settings".to_owned(),
                    Place::Repository(id) => format!("/settings/repositories/{id}"),
                    Place::PullRequest(id) => format!("/pull-requests/{id}"),
                    Place::ExternalRequest(id) => format!("/external-execution-requests/{id}"),
                };
                format!("{root}{tail}")
            }
        }
    }
}

/// How this server writes links: today's shapes, or workspace addresses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Addresses {
    workspace_addresses: bool,
}

impl Addresses {
    /// Today's shapes.
    pub const LEGACY: Self = Self {
        workspace_addresses: false,
    };
    /// Every link names its workspace.
    pub const WORKSPACE: Self = Self {
        workspace_addresses: true,
    };

    /// The shape this server writes. There is no setting: it is today's
    /// shape until the web serves workspace addresses (K4.19).
    pub fn of(_state: &AppState) -> Self {
        Self::LEGACY
    }

    /// The path of `target`, in `workspace` when it is known (see [`link`]).
    pub fn link(self, workspace: Option<Uuid>, target: &Target) -> String {
        match workspace {
            Some(workspace) => self.in_workspace(workspace, target),
            // Today's shape, which the web resolves to its workspace.
            None => target.legacy_path(),
        }
    }

    /// Whether links name their workspace.
    pub fn names_workspace(self) -> bool {
        self.workspace_addresses
    }

    /// The path of `target` in `workspace`.
    pub fn path(self, workspace: &WorkspaceAddress, target: &Target) -> String {
        self.from_legacy(workspace, &target.legacy_path())
    }

    /// The path of `target` in the workspace with id `workspace` (its
    /// links use the id, which the web swaps for the slug).
    pub fn in_workspace(self, workspace: Uuid, target: &Target) -> String {
        self.path(&WorkspaceAddress::id(workspace), target)
    }

    /// The full URL of `target`, on `origin` (no trailing slash).
    pub fn url(self, origin: &str, workspace: &WorkspaceAddress, target: &Target) -> String {
        format!(
            "{}{}",
            origin.trim_end_matches('/'),
            self.path(workspace, target)
        )
    }

    /// A path written in today's shape (a module manifest's thing template,
    /// a stored link) in this server's shape. A path that isn't inside a
    /// workspace (account settings, sign-in) comes back as it is.
    pub fn from_legacy(self, workspace: &WorkspaceAddress, legacy: &str) -> String {
        if !self.workspace_addresses {
            return legacy.to_owned();
        }
        workspace_path(&workspace.segment(), legacy).unwrap_or_else(|| legacy.to_owned())
    }

    /// The workspace `id`, with its slug when links name workspaces. Off,
    /// nothing is read.
    pub async fn workspace(
        self,
        connection: &mut PgConnection,
        id: Uuid,
    ) -> Result<WorkspaceAddress, AppError> {
        if !self.workspace_addresses {
            return Ok(WorkspaceAddress::id(id));
        }
        address(connection, id).await
    }
}

/// Builds links for one piece of work (a response, a mail, a list of
/// results), finding each target's workspace from the target itself and
/// remembering it for the next link into the same place. While links don't
/// name their workspace nothing is read. It lives as long as the work.
#[derive(Debug)]
pub struct Linker {
    addresses: Addresses,
    workspaces: HashMap<(Owner, Uuid), WorkspaceAddress>,
}

/// What a target's workspace is read from.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Owner {
    Workspace,
    Space,
    Conversation,
    Whiteboard,
    Run,
}

impl Linker {
    pub fn new(addresses: Addresses) -> Self {
        Self {
            addresses,
            workspaces: HashMap::new(),
        }
    }

    pub fn of(state: &AppState) -> Self {
        Self::new(Addresses::of(state))
    }

    pub fn addresses(&self) -> Addresses {
        self.addresses
    }

    /// The path of a target that lives somewhere (a space, a DM, a board, a
    /// run). A page of the workspace itself (`Inbox`, `Later`) needs its
    /// workspace named: use [`Linker::path_in`].
    pub async fn path(
        &mut self,
        connection: &mut PgConnection,
        target: &Target,
    ) -> Result<String, AppError> {
        if !self.addresses.workspace_addresses {
            return Ok(target.legacy_path());
        }
        let owner = match *target {
            Target::Space { space, .. } | Target::SpaceSettings { space, .. } => {
                (Owner::Space, space)
            }
            Target::Direct { id, .. } => (Owner::Conversation, id),
            Target::Whiteboard(id) => (Owner::Whiteboard, id),
            Target::Run(id) => (Owner::Run, id),
            _ => {
                return Err(AppError::Internal(
                    "a workspace page's link needs its workspace",
                ));
            }
        };
        let workspace = self.workspace_of(connection, owner).await?;
        Ok(self.addresses.path(&workspace, target))
    }

    /// The path of `target` in the workspace `workspace`.
    pub async fn path_in(
        &mut self,
        connection: &mut PgConnection,
        workspace: Uuid,
        target: &Target,
    ) -> Result<String, AppError> {
        if !self.addresses.workspace_addresses {
            return Ok(target.legacy_path());
        }
        let workspace = self
            .workspace_of(connection, (Owner::Workspace, workspace))
            .await?;
        Ok(self.addresses.path(&workspace, target))
    }

    /// A path in today's shape (a module manifest's thing template) for a
    /// thing in `space`, in this server's shape.
    pub async fn legacy_in_space(
        &mut self,
        connection: &mut PgConnection,
        space: Uuid,
        legacy: &str,
    ) -> Result<String, AppError> {
        if !self.addresses.workspace_addresses {
            return Ok(legacy.to_owned());
        }
        let workspace = self.workspace_of(connection, (Owner::Space, space)).await?;
        Ok(self.addresses.from_legacy(&workspace, legacy))
    }

    async fn workspace_of(
        &mut self,
        connection: &mut PgConnection,
        (owner, id): (Owner, Uuid),
    ) -> Result<WorkspaceAddress, AppError> {
        if let Some(found) = self.workspaces.get(&(owner, id)) {
            return Ok(found.clone());
        }
        // Only the workspace of something the caller already read is looked
        // up, and only its address is used.
        let workspace = match owner {
            Owner::Workspace => Some(id),
            Owner::Space => {
                sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&mut *connection)
                    .await?
            }
            Owner::Conversation => {
                sqlx::query_scalar::<_, Uuid>(
                    r#"
                    -- visibility: the caller already read this conversation;
                    -- only its workspace's address is used.
                    SELECT workspace_id FROM conversations WHERE id = $1
                    "#,
                )
                .bind(id)
                .fetch_optional(&mut *connection)
                .await?
            }
            Owner::Whiteboard => {
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT organization_id FROM whiteboards WHERE id = $1",
                )
                .bind(id)
                .fetch_optional(&mut *connection)
                .await?
            }
            Owner::Run => crate::runs::store::workspace_of(&mut *connection, id).await?,
        }
        .ok_or(AppError::NotFound("workspace"))?;
        let found = address(connection, workspace).await?;
        self.workspaces.insert((owner, id), found.clone());
        Ok(found)
    }
}

/// The path of `target` where a link is built with only the server state
/// at hand: in `workspace` when the caller knows it, else in today's shape,
/// which the web and the resolver still open once links name their
/// workspace (K4.19).
pub fn link(state: &AppState, workspace: Option<Uuid>, target: &Target) -> String {
    Addresses::of(state).link(workspace, target)
}

/// `path` with `query` added to whatever query it has.
pub fn with_query(path: String, query: &str) -> String {
    if query.is_empty() {
        return path;
    }
    let joiner = if path.contains('?') { '&' } else { '?' };
    format!("{path}{joiner}{query}")
}

/// A workspace's address: its id and current slug.
pub async fn address(
    connection: &mut PgConnection,
    workspace: Uuid,
) -> Result<WorkspaceAddress, AppError> {
    let slug = sqlx::query_scalar::<_, String>("SELECT slug FROM organizations WHERE id = $1")
        .bind(workspace)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or(AppError::NotFound("workspace"))?;
    Ok(WorkspaceAddress::new(workspace, slug))
}

/// Pages of a workspace that sit at the web's root today and move under
/// `/w/<ws>` as they are (the first segment of their path).
const WORKSPACE_PAGES: &[&str] = &[
    "home",
    "welcome",
    "inbox",
    "later",
    "my-work",
    "search",
    "runs",
    "pages",
    "whiteboards",
    "messages",
    "share",
];

/// The `?query#hash` part of a path, kept apart.
pub(crate) fn split_path(path: &str) -> (&str, &str) {
    match path.find(['?', '#']) {
        Some(at) => (&path[..at], &path[at..]),
        None => (path, ""),
    }
}

fn joined(segments: &[&str]) -> String {
    segments
        .iter()
        .map(|segment| format!("/{segment}"))
        .collect()
}

/// A space's place, from today's words to the grammar's.
fn space_tail(tail: &[&str]) -> String {
    match tail {
        [] => String::new(),
        ["work-items", rest @ ..] => format!("/tasks{}", joined(rest)),
        ["launches", rest @ ..] => format!("/runs{}", joined(rest)),
        ["channels", rest @ ..] => format!("/chat{}", joined(rest)),
        ["conversations", rest @ ..] => format!("/chat{}", joined(rest)),
        ["next" | "suggestions" | "graph", ..] => format!("/planning{}", joined(tail)),
        ["plan-import", rest @ ..] => format!("/planning/import{}", joined(rest)),
        _ => joined(tail),
    }
}

/// Today's address `legacy` as an address in the workspace whose segment
/// (slug or id) is `workspace`, or `None` when it isn't a workspace's page
/// (account settings, sign-in, forms) or is already a workspace address.
pub fn workspace_path(workspace: &str, legacy: &str) -> Option<String> {
    let (path, rest) = split_path(legacy);
    if !path.starts_with('/') {
        return None;
    }
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let inner = match segments.as_slice() {
        [] => "/home".to_owned(),
        ["projects"] => "/spaces".to_owned(),
        ["projects", "new"] => "/spaces/new".to_owned(),
        ["projects", space, "settings", tail @ ..] => {
            format!("/settings/spaces/{space}{}", joined(tail))
        }
        ["projects", space, tail @ ..] => format!("/s/{space}{}", space_tail(tail)),
        ["messages", id] if Uuid::parse_str(id).is_ok() => format!("/dm/{id}"),
        ["dm", id] => format!("/dm/{id}"),
        ["new-task"] => "/new".to_owned(),
        ["settings", "workspace", tail @ ..] => format!("/settings{}", joined(tail)),
        ["settings", "projects", space, tail @ ..] => {
            format!("/settings/spaces/{space}{}", joined(tail))
        }
        [first, ..] if WORKSPACE_PAGES.contains(first) => path.to_owned(),
        _ => return None,
    };
    Some(format!("/w/{workspace}{inner}{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    /// Every target, with its old shape and its workspace shape in the
    /// workspace `acme`.
    fn table() -> Vec<(Target, String, String)> {
        let (s, x, y) = (id(1), id(2), id(3));
        let space = |place| Target::space(s, place);
        vec![
            (Target::Home, "/home".into(), "/w/acme/home".into()),
            (Target::Inbox, "/inbox".into(), "/w/acme/inbox".into()),
            (Target::Later, "/later".into(), "/w/acme/later".into()),
            (
                Target::LaterScheduled,
                "/later?view=scheduled".into(),
                "/w/acme/later?view=scheduled".into(),
            ),
            (Target::MyWork, "/my-work".into(), "/w/acme/my-work".into()),
            (Target::Search, "/search".into(), "/w/acme/search".into()),
            (
                Target::Messages,
                "/messages".into(),
                "/w/acme/messages".into(),
            ),
            (
                Target::MessageAgents,
                "/messages/agents".into(),
                "/w/acme/messages/agents".into(),
            ),
            (Target::Runs, "/runs".into(), "/w/acme/runs".into()),
            (
                Target::Run(x),
                format!("/runs/{x}"),
                format!("/w/acme/runs/{x}"),
            ),
            (Target::Spaces, "/projects".into(), "/w/acme/spaces".into()),
            (Target::Pages, "/pages".into(), "/w/acme/pages".into()),
            (
                Target::Whiteboard(x),
                format!("/whiteboards/{x}"),
                format!("/w/acme/whiteboards/{x}"),
            ),
            (
                Target::Direct {
                    id: x,
                    thread: None,
                },
                format!("/messages/{x}"),
                format!("/w/acme/dm/{x}"),
            ),
            (
                Target::Direct {
                    id: x,
                    thread: Some(y),
                },
                format!("/messages/{x}?thread={y}"),
                format!("/w/acme/dm/{x}?thread={y}"),
            ),
            (
                Target::WorkspaceSettings("members"),
                "/settings/workspace/members".into(),
                "/w/acme/settings/members".into(),
            ),
            (
                Target::SpaceSettings {
                    space: s,
                    section: "automations",
                },
                format!("/settings/projects/{s}/automations"),
                format!("/w/acme/settings/spaces/{s}/automations"),
            ),
            (
                space(Place::Home),
                format!("/projects/{s}"),
                format!("/w/acme/s/{s}"),
            ),
            (
                space(Place::Channel(x)),
                format!("/projects/{s}/channels/{x}"),
                format!("/w/acme/s/{s}/chat/{x}"),
            ),
            (
                space(Place::Thread(x)),
                format!("/projects/{s}/threads/{x}"),
                format!("/w/acme/s/{s}/threads/{x}"),
            ),
            (
                space(Place::ArchivedChat),
                format!("/projects/{s}/conversations/archived"),
                format!("/w/acme/s/{s}/chat/archived"),
            ),
            (
                space(Place::Tasks),
                format!("/projects/{s}/work-items"),
                format!("/w/acme/s/{s}/tasks"),
            ),
            (
                space(Place::Task(x)),
                format!("/projects/{s}/work-items/{x}"),
                format!("/w/acme/s/{s}/tasks/{x}"),
            ),
            (
                space(Place::Pages),
                format!("/projects/{s}/pages"),
                format!("/w/acme/s/{s}/pages"),
            ),
            (
                space(Place::Page(x)),
                format!("/projects/{s}/pages/{x}"),
                format!("/w/acme/s/{s}/pages/{x}"),
            ),
            (
                space(Place::Decisions),
                format!("/projects/{s}/decisions"),
                format!("/w/acme/s/{s}/decisions"),
            ),
            (
                space(Place::Collection(x)),
                format!("/projects/{s}/collections/{x}"),
                format!("/w/acme/s/{s}/collections/{x}"),
            ),
            (
                space(Place::Record {
                    collection: x,
                    record: y,
                }),
                format!("/projects/{s}/collections/{x}?record={y}"),
                format!("/w/acme/s/{s}/collections/{x}?record={y}"),
            ),
            (
                space(Place::Sheet(x)),
                format!("/projects/{s}/sheets/{x}"),
                format!("/w/acme/s/{s}/sheets/{x}"),
            ),
            (
                space(Place::Milestones),
                format!("/projects/{s}/milestones"),
                format!("/w/acme/s/{s}/milestones"),
            ),
            (
                space(Place::Timeline),
                format!("/projects/{s}/timeline"),
                format!("/w/acme/s/{s}/timeline"),
            ),
            (
                space(Place::Runs),
                format!("/projects/{s}/launches"),
                format!("/w/acme/s/{s}/runs"),
            ),
            (
                space(Place::Next),
                format!("/projects/{s}/next"),
                format!("/w/acme/s/{s}/planning/next"),
            ),
            (
                space(Place::Suggestions),
                format!("/projects/{s}/suggestions"),
                format!("/w/acme/s/{s}/planning/suggestions"),
            ),
            (
                space(Place::Graph),
                format!("/projects/{s}/graph"),
                format!("/w/acme/s/{s}/planning/graph"),
            ),
            (
                space(Place::PlanImport),
                format!("/projects/{s}/plan-import"),
                format!("/w/acme/s/{s}/planning/import"),
            ),
            (
                space(Place::Settings),
                format!("/projects/{s}/settings"),
                format!("/w/acme/settings/spaces/{s}"),
            ),
            (
                space(Place::Repository(x)),
                format!("/projects/{s}/settings/repositories/{x}"),
                format!("/w/acme/settings/spaces/{s}/repositories/{x}"),
            ),
            (
                space(Place::PullRequest(x)),
                format!("/projects/{s}/pull-requests/{x}"),
                format!("/w/acme/s/{s}/pull-requests/{x}"),
            ),
            (
                space(Place::ExternalRequest(x)),
                format!("/projects/{s}/external-execution-requests/{x}"),
                format!("/w/acme/s/{s}/external-execution-requests/{x}"),
            ),
        ]
    }

    #[test]
    fn every_target_has_its_old_shape_off_and_the_grammar_on() {
        let acme = WorkspaceAddress::new(id(9), "acme");
        for (target, old, new) in table() {
            assert_eq!(Addresses::LEGACY.path(&acme, &target), old, "{target:?}");
            assert_eq!(Addresses::WORKSPACE.path(&acme, &target), new, "{target:?}");
            assert_eq!(target.legacy_path(), old, "{target:?}");
        }
    }

    #[test]
    fn a_workspace_known_by_id_links_by_id() {
        let workspace = WorkspaceAddress::id(id(9));
        assert_eq!(
            Addresses::WORKSPACE.path(&workspace, &Target::Inbox),
            format!("/w/{}/inbox", id(9))
        );
        assert_eq!(
            Addresses::WORKSPACE.url("https://example.test/", &workspace, &Target::Inbox),
            format!("https://example.test/w/{}/inbox", id(9))
        );
    }

    #[test]
    fn queries_and_hashes_stay() {
        let s = id(1);
        assert_eq!(
            workspace_path("acme", &format!("/projects/{s}/pages/{s}#comment-1")),
            Some(format!("/w/acme/s/{s}/pages/{s}#comment-1"))
        );
        assert_eq!(
            workspace_path("acme", &format!("/projects/{s}/work-items?sort=updated")),
            Some(format!("/w/acme/s/{s}/tasks?sort=updated"))
        );
        assert_eq!(
            workspace_path("acme", "/new-task?title=x"),
            Some("/w/acme/new?title=x".to_owned())
        );
    }

    #[test]
    fn pages_outside_a_workspace_stay_put() {
        for path in [
            "/settings/account/profile",
            "/sign-in",
            "/forms/abc",
            "/w/acme/home",
            "/design-system",
            "AWD-12",
        ] {
            assert_eq!(workspace_path("acme", path), None, "{path}");
            assert_eq!(
                Addresses::WORKSPACE.from_legacy(&WorkspaceAddress::new(id(9), "acme"), path),
                path
            );
        }
        assert_eq!(workspace_path("acme", "/"), Some("/w/acme/home".to_owned()));
    }

    #[test]
    fn conversations_open_where_they_live() {
        let (s, x, p) = (id(1), id(2), id(3));
        assert_eq!(
            Target::conversation(s, "dm", x, None, true, true),
            Target::Direct {
                id: x,
                thread: None
            }
        );
        assert_eq!(
            Target::conversation(s, "thread", x, Some(p), true, true),
            Target::Direct {
                id: p,
                thread: Some(x)
            }
        );
        assert_eq!(
            Target::conversation(s, "thread", x, Some(p), true, false),
            Target::Direct {
                id: p,
                thread: None
            }
        );
        assert_eq!(
            Target::conversation(s, "thread", x, Some(p), false, true),
            Target::space(s, Place::Thread(x))
        );
        assert_eq!(
            Target::conversation(s, "channel", x, None, false, true),
            Target::space(s, Place::Channel(x))
        );
    }
}
