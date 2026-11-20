//! Two-workspace isolation (roadmap O7). The SaaS keeps many workspaces
//! (`organizations` rows) in one database, so a leak between them would be
//! silent and costly. One fixture builds two workspaces, A and B, with
//! look-alike people, agents, projects (both have `WORK`, both have task
//! `WORK-1`), channels, threads, DMs, groups, tasks with labels, milestones,
//! comments, subtasks and criteria, pages, decisions, repositories, pull
//! requests, runs and approvals. Every name and text B writes carries the
//! marker `zebra` (handles, the project key, the channel name and the
//! repository name are deliberately the same in both), and every row B owns
//! is swept from the catalog into a set of B IDs. Then, signed in
//! as A's owner (and through A's connected app and A's agent):
//!
//! - every GET route under `/api/v1` is called with A's IDs (must succeed) and
//!   with B's IDs, alone and mixed with A's (must not return B's data);
//! - every read-only MCP tool is called the same way;
//! - A's realtime socket receives A's events and none of B's.
//!
//! A response "leaks" when it contains the marker or any B ID.
//!
//! # Adding a feature
//!
//! Each new feature adds its own cases here, in the same change:
//!
//! - a new GET route under `/api/v1`: add its path, exactly as written in
//!   `.route("...")`, to [`HTTP_ROUTES`] and a case to `http_cases` (own
//!   URIs that succeed for A, foreign URIs built from B's IDs). Routes that
//!   can't be called this way go in [`HTTP_EXCLUSIONS`] with the reason.
//! - a new read-only MCP tool (`read_only_hint = true`): add a case to
//!   `mcp_cases`. Tools that can't be called this way go in
//!   [`MCP_EXCLUSIONS`] with the reason.
//! - new kinds of data: create the look-alike in `build_workspace`, with
//!   `marker` in its text, so the sweep and the marker check catch it.
//!
//! `every_get_route_and_read_tool_has_an_isolation_case` reads the server
//! sources and fails until that's done.

use std::{
    collections::{BTreeSet, HashSet},
    time::Duration,
};

use agent_workspace_server::{
    mcp::MCP_PATH,
    realtime::RealtimeHub,
    test_support::source::{routes as shared_routes, rust_files, server_src, strip_tests},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::{net::TcpListener, time::timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message as ClientMessage, client::IntoClientRequest, http::HeaderValue},
};
use tower::ServiceExt;

use super::{
    agent_interface::{Browser, initialize},
    mcp_admin::Setup,
    message_edits::{say, text},
    write_isolation::{
        base_tables, digest_changes, digest_ignoring, refresh_digest, table_fingerprints,
    },
};

pub(super) use super::support::two_workspaces::{
    TwoWorkspaces, World, add_person, login_as, string_at, sweep_ids, tiny_png, two_workspaces,
    two_workspaces_with, upload, url_encode,
};

pub(super) const MARKER: &str = "zebra";
/// A 1x1 PNG, for a whiteboard image.
pub(super) const TINY_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
pub(super) const ORIGIN: &str = "http://127.0.0.1:3100";

/// Every GET route under `/api/v1` this suite calls, as written in the
/// server's `.route(...)` calls.
const HTTP_ROUTES: &[&str] = &[
    "/api/v1/account/connected-apps",
    "/api/v1/account/email",
    "/api/v1/account/profile",
    "/api/v1/account/api-tokens",
    "/api/v1/account/appearance",
    "/api/v1/account/security",
    "/api/v1/account/sessions",
    "/api/v1/accounts/{id}/avatar",
    "/api/v1/auth/password-reset",
    "/api/v1/auth/session",
    "/api/v1/auth/workos",
    "/api/v1/conversations/{id}/messages/{message_id}",
    "/api/v1/conversations/{id}/messages/{message_id}/revisions",
    "/api/v1/conversations/{id}/notification-level",
    "/api/v1/conversations/{id}/pins",
    "/api/v1/conversations/{id}/threads",
    "/api/v1/conversations/{id}/timeline",
    "/api/v1/conversations/{conversation_id}/agent-sessions",
    "/api/v1/custom-emoji/{id}",
    "/api/v1/agent-runs/{run_id}",
    "/api/v1/agent-runs/{run_id}/diff",
    "/api/v1/dev/bootstrap",
    "/api/v1/direct",
    "/api/v1/events",
    "/api/v1/events/catalogue",
    "/api/v1/file-links/{token}",
    "/api/v1/files/{id}",
    "/api/v1/files/{id}/content",
    "/api/v1/files/{id}/thumbnail",
    "/api/v1/home",
    "/api/v1/inbox",
    "/api/v1/inbox/summary",
    "/api/v1/link-previews/{id}/image",
    "/api/v1/later",
    "/api/v1/links/resolve",
    "/api/v1/my-work",
    "/api/v1/notification-settings",
    "/api/v1/organizations/{organization_id}",
    "/api/v1/organizations/{organization_id}/ai",
    "/api/v1/organizations/{organization_id}/audit-events",
    "/api/v1/organizations/{organization_id}/audit-events/facets",
    "/api/v1/organizations/{organization_id}/harnesses",
    "/api/v1/organizations/{organization_id}/invitations",
    "/api/v1/organizations/{organization_id}/layout",
    "/api/v1/organizations/{organization_id}/mail",
    "/api/v1/organizations/{organization_id}/members",
    "/api/v1/organizations/{organization_id}/model-catalog",
    "/api/v1/organizations/{organization_id}/modules",
    "/api/v1/organizations/{organization_id}/onboarding",
    "/api/v1/organizations/{organization_id}/pages-space",
    "/api/v1/organizations/{organization_id}/permissions",
    "/api/v1/organizations/{organization_id}/personal-secrets",
    "/api/v1/organizations/{organization_id}/project-templates",
    "/api/v1/organizations/{organization_id}/roles",
    "/api/v1/organizations/{organization_id}/secrets",
    "/api/v1/organizations/{organization_id}/transfers/{transfer_id}/archive",
    "/api/v1/organizations/{organization_id}/workspace-templates",
    "/api/v1/organizations/{organization_id}/server",
    "/api/v1/pages/{id}",
    "/api/v1/pages/{id}/discussion",
    "/api/v1/pages/{id}/document",
    "/api/v1/pages/{id}/export",
    "/api/v1/pages/{id}/revisions/{version}",
    "/api/v1/projects",
    "/api/v1/projects/{id}",
    "/api/v1/projects/{id}/channels",
    "/api/v1/projects/{id}/conversation-sidebar",
    "/api/v1/projects/{id}/decisions",
    "/api/v1/projects/{id}/execution-profiles",
    "/api/v1/projects/{id}/execution-workers",
    "/api/v1/projects/{id}/harnesses",
    "/api/v1/projects/{id}/mentionable-actors",
    "/api/v1/projects/{id}/navigation",
    "/api/v1/projects/{id}/pages",
    "/api/v1/projects/{project_id}/agent-profiles",
    "/api/v1/projects/{project_id}/agents",
    "/api/v1/projects/{project_id}/ai",
    "/api/v1/projects/{project_id}/ai/graph",
    "/api/v1/projects/{project_id}/ai/plans",
    "/api/v1/projects/{project_id}/ai/plans/{import_id}",
    "/api/v1/projects/{project_id}/ai/ready",
    "/api/v1/projects/{project_id}/ai/suggestions",
    "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account",
    "/api/v1/projects/{project_id}/external-agents",
    "/api/v1/projects/{project_id}/external-agents/{connection_id}/workos",
    "/api/v1/projects/{project_id}/export",
    "/api/v1/projects/{project_id}/external-execution-requests",
    "/api/v1/projects/{project_id}/integrations/github",
    "/api/v1/projects/{project_id}/imports",
    "/api/v1/projects/{project_id}/imports/{import_id}",
    "/api/v1/projects/{project_id}/integrations/github/automation",
    "/api/v1/projects/{project_id}/environments",
    "/api/v1/projects/{project_id}/integrations/github/available-repositories",
    "/api/v1/projects/{project_id}/integrations/github/connection",
    "/api/v1/projects/{project_id}/invitations",
    "/api/v1/projects/{project_id}/labels",
    "/api/v1/projects/{project_id}/fields",
    "/api/v1/projects/{project_id}/collections",
    "/api/v1/projects/{project_id}/collections/{collection_id}",
    "/api/v1/projects/{project_id}/collections/{collection_id}/records",
    "/api/v1/projects/{project_id}/collections/{collection_id}/records/{record_id}",
    "/api/v1/projects/{project_id}/sheets",
    "/api/v1/projects/{project_id}/sheets/{sheet_id}",
    "/api/v1/projects/{project_id}/sheets/{sheet_id}/export",
    "/api/v1/projects/{project_id}/launches",
    "/api/v1/projects/{project_id}/layout",
    "/api/v1/projects/{project_id}/member-candidates",
    "/api/v1/projects/{project_id}/members",
    "/api/v1/projects/{project_id}/milestones",
    "/api/v1/projects/{project_id}/provider-accounts",
    "/api/v1/projects/{project_id}/provider-accounts/{account_id}",
    "/api/v1/projects/{project_id}/repositories",
    "/api/v1/projects/{project_id}/secrets",
    "/api/v1/projects/{project_id}/task-templates",
    "/api/v1/projects/{project_id}/timeline",
    "/api/v1/projects/{project_id}/automations",
    "/api/v1/projects/{project_id}/bug-report-links",
    "/api/v1/projects/{project_id}/automations/{rule_id}/runs",
    "/api/v1/projects/{project_id}/work-item-assignees",
    "/api/v1/projects/{project_id}/work-item-references",
    "/api/v1/projects/{project_id}/work-item-workflow",
    "/api/v1/projects/{project_id}/work-items",
    "/api/v1/projects/{project_id}/work-items/board",
    "/api/v1/projects/{project_id}/work-items/by-key/{key}",
    "/api/v1/projects/{project_id}/work-items/groups",
    "/api/v1/projects/{project_id}/work-items/{work_item_id}",
    "/api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria",
    "/api/v1/projects/{project_id}/work-items/{work_item_id}/claim",
    "/api/v1/projects/{project_id}/work-items/{work_item_id}/files",
    "/api/v1/projects/{project_id}/work-items/{work_item_id}/readiness",
    "/api/v1/projects/{project_id}/work-items/{work_item_id}/project-move",
    "/api/v1/projects/{project_id}/work-items/{work_item_id}/project-move/destinations",
    "/api/v1/projects/{project_id}/work-items/{work_item_id}/launch",
    "/api/v1/projects/{project_id}/work-items/{work_item_id}/timeline",
    "/api/v1/projects/{project_id}/work-items/{work_item_id}/comments/{comment_id}/revisions",
    "/api/v1/push/config",
    "/api/v1/push/subscriptions",
    "/api/v1/runs",
    "/api/v1/runs/usage",
    "/api/v1/scheduled-messages",
    "/api/v1/search",
    "/api/v1/settings/effective",
    "/api/v1/settings/registry",
    "/api/v1/things/{id}",
    "/api/v1/things/{kind}/{id}",
    "/api/v1/usage",
    "/api/v1/usage/accounts/{account_id}",
    "/api/v1/usage/accounts/{account_id}/ready-tasks",
    "/api/v1/usage/accounts/{account_id}/batch",
    "/api/v1/usage/accounts/{account_id}/history",
    "/api/v1/whiteboard-links",
    "/api/v1/whiteboards",
    "/api/v1/whiteboards/{id}",
    "/api/v1/whiteboards/{id}/files/{file_id}",
    "/api/v1/whiteboards/{id}/preview.svg",
    "/api/v1/whiteboards/{id}/scene",
    "/api/v1/workspaces/by-slug/{slug}",
];

/// GET routes under `/api/v1` this suite deliberately doesn't call as plain
/// HTTP requests, and why. (Routes outside `/api/v1` aren't workspace data:
/// `/metrics` is a token-gated operator endpoint with process-wide counters,
/// `/health/*`, `/auth.md`, `/connect.md`, OAuth and the WorkOS callback are
/// sign-in plumbing, so the guard only looks under `/api/v1`.)
const HTTP_EXCLUSIONS: &[(&str, &str)] = &[
    (
        "/api/v1/setup",
        "first run (H1.14): whether nobody has an account yet, and the built-in shapes; no \
         workspace's data; covered in first_run",
    ),
    (
        "/api/v1/setup/import/{transfer}",
        "first run (H1.14): the requester's own import, by its id and their account alone; \
         covered in first_run",
    ),
    (
        "/api/v1/host/commands",
        "a host's own command queue (A1.12), signed in with the host's key, never a \
         person's session; it lists only that host's commands; covered in a1_host_protocol",
    ),
    (
        "/api/v1/host/commands/{command_id}",
        "one of a host's own commands (A1.12), by the host's key; another host's command \
         is not found; covered in a1_host_protocol",
    ),
    (
        "/api/v1/realtime",
        "WebSocket upgrade; covered by the realtime part of the fixture test, which \
         subscribes as A and checks no B event arrives",
    ),
    (
        "/api/v1/shared/{token}",
        "a page opened by a link's secret token, made for that one page; covered in \
         privacy_inside_workspace (a_shared_page_opens_only_itself)",
    ),
    (
        "/api/v1/forms/{token}",
        "a form's public page reads it by the link's secret token, not a session; it holds \
         only the form (the collection's name and the asked fields), never records or \
         people; covered in awd110_collection_forms",
    ),
    (
        "/api/v1/auth/csrf",
        "hands the caller's own request-token cookie back to an allowed web origin (H1.41); \
         reads no row of any workspace; covered in client_config's unit tests",
    ),
    (
        "/api/v1/integrations/github/callback",
        "GitHub's redirect back after sign-in: returns a redirect, never data, and \
         only for a state bound to the caller's browser; covered in settings_accounts",
    ),
];

/// Read-only MCP tools this suite deliberately doesn't call, and why.
/// (Every read-only tool currently has a case.)
const MCP_EXCLUSIONS: &[(&str, &str)] = &[];

/// Columns a read may legitimately change: when a credential or grant was
/// last used, and when a connection last shook hands. A touch (at most every
/// five minutes) moves the row's `updated_at` with it, so a suite that runs
/// longer than that saw a read "change" a credential; a real write always
/// changes a column besides `updated_at`, so leaving it out loses nothing.
const READ_TOUCHES: &[&str] = &[
    "last_used_at",
    "last_seen_at",
    "last_handshake_at",
    "updated_at",
];

/// Tools marked read-only that change rows anyway, each with why. The list
/// only shrinks: fix the tool (or its hint) and take it off.
const READ_TOOLS_THAT_WRITE: &[(&str, &str)] = &[(
    "space_personal_open",
    "opening your own space makes it the first time (K4 3.9: personal spaces are made on \
     first use, never eagerly); only the caller's own rows, in their own workspace",
)];

// ---------------------------------------------------------------------------
// Coverage guard (no database needed)
// ---------------------------------------------------------------------------

#[test]
fn every_get_route_and_read_tool_has_an_isolation_case() {
    let sources = rust_files(&server_src());
    assert!(!sources.is_empty(), "no server sources");

    let routes = sources
        .iter()
        .flat_map(|(_, source)| shared_routes(strip_tests(source)))
        .filter(|route| route.methods.contains(&"GET"))
        .map(|route| route.path)
        .filter(|path| path.starts_with("/api/v1"))
        .collect::<BTreeSet<_>>();
    let covered = HTTP_ROUTES
        .iter()
        .map(|path| (*path).to_owned())
        .collect::<BTreeSet<_>>();
    let excluded = HTTP_EXCLUSIONS
        .iter()
        .map(|(path, _)| (*path).to_owned())
        .collect::<BTreeSet<_>>();
    let missing = routes
        .iter()
        .filter(|path| !covered.contains(*path) && !excluded.contains(*path))
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "GET routes without a workspace-isolation case (add them to HTTP_ROUTES and \
         http_cases, or to HTTP_EXCLUSIONS with a reason): {missing:#?}"
    );
    let stale = covered
        .union(&excluded)
        .filter(|path| !routes.contains(*path))
        .collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "isolation routes that no longer exist: {stale:#?}"
    );

    let mcp_source = sources
        .iter()
        .find(|(path, _)| path == "mcp.rs")
        .map(|(_, source)| source.as_str())
        .expect("src/mcp.rs");
    // Hand-written tools by their annotation, and operations by their kind.
    let read_tools = read_only_tools(mcp_source)
        .into_iter()
        .chain(
            agent_workspace_server::ops::registry()
                .entries()
                .filter(|entry| !entry.meta.kind.writes())
                .map(|entry| entry.meta.name.to_owned())
                // ops_kernel's test operations, registered by other tests.
                .filter(|name| !name.starts_with("echo_")),
        )
        .collect::<BTreeSet<_>>();
    assert!(read_tools.len() > 10, "parsed read tools: {read_tools:?}");
    let cased = mcp_case_names()
        .into_iter()
        .chain(MCP_EXCLUSIONS.iter().map(|(name, _)| (*name).to_owned()))
        .collect::<BTreeSet<_>>();
    let missing = read_tools.difference(&cased).collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "read-only MCP tools without a workspace-isolation case (add them to mcp_cases, \
         or to MCP_EXCLUSIONS with a reason): {missing:#?}"
    );
    let stale = cased.difference(&read_tools).collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "isolation cases for tools that aren't read-only tools: {stale:#?}"
    );
}

/// Names of `#[tool(...)]` methods annotated `read_only_hint = true`.
fn read_only_tools(source: &str) -> BTreeSet<String> {
    let mut tools = BTreeSet::new();
    for (index, _) in source.match_indices("#[tool(") {
        let rest = &source[index..];
        let Some(function) = rest.find("async fn ") else {
            continue;
        };
        let attribute = rest[..function].split_whitespace().collect::<String>();
        if !attribute.contains("read_only_hint=true") {
            continue;
        }
        let name_start = function + "async fn ".len();
        let name = rest[name_start..]
            .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .next()
            .expect("tool name");
        // An explicit `name = "..."` first in the attribute wins over the
        // method name.
        let explicit = attribute
            .strip_prefix("#[tool(name=\"")
            .and_then(|rest| rest.split('"').next());
        tools.insert(explicit.unwrap_or(name).to_owned());
    }
    tools
}

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

/// Which edge one test sweeps. The three run as separate tests, each on its
/// own pair of workspaces, so they run side by side and each stays well
/// inside the per-test time limit as routes and tools are added.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SweptEdge {
    Http,
    /// MCP as A's connected app (acting as A's owner).
    McpApp,
    /// MCP as A's agent.
    McpAgent,
    /// MCP with an account-wide token issued for A, held by B's owner, who
    /// belongs to both: the token reaches A only, though its account sees B.
    McpToken,
    Realtime,
}

#[tokio::test]
async fn one_workspace_never_sees_another_over_http() {
    one_workspace_never_sees_another("workspace_isolation_http", SweptEdge::Http).await;
}

#[tokio::test]
async fn one_workspace_never_sees_another_through_an_app() {
    one_workspace_never_sees_another("workspace_isolation_app", SweptEdge::McpApp).await;
}

#[tokio::test]
async fn one_workspace_never_sees_another_through_an_agent() {
    one_workspace_never_sees_another("workspace_isolation_agent", SweptEdge::McpAgent).await;
}

#[tokio::test]
async fn a_token_for_one_workspace_never_sees_another_its_account_is_in() {
    one_workspace_never_sees_another("workspace_isolation_token", SweptEdge::McpToken).await;
}

#[tokio::test]
async fn one_workspace_never_sees_another_over_realtime() {
    one_workspace_never_sees_another("workspace_isolation_realtime", SweptEdge::Realtime).await;
}

async fn one_workspace_never_sees_another(name: &str, edge: SweptEdge) {
    let Some(fixture) = two_workspaces(name).await else {
        return;
    };
    let TwoWorkspaces {
        app,
        pool,
        realtime,
        a,
        b,
        ..
    } = &fixture;
    let b_ids = sweep_ids(pool, b.organization).await;
    let a_ids = sweep_ids(pool, a.organization).await;
    assert!(
        a_ids.is_disjoint(&b_ids),
        "the sweeps overlap: {:?}",
        a_ids.intersection(&b_ids).collect::<Vec<_>>()
    );
    for id in [b.project.as_str(), b.channel.as_str(), b.dm.as_str()] {
        assert!(b_ids.contains(id), "sweep missed {id}");
    }
    eprintln!("B owns {} IDs", b_ids.len());

    let mut report = Report::default();
    match edge {
        SweptEdge::Http => check_http(app, a, b, &b_ids, &mut report).await,
        SweptEdge::McpApp => check_mcp(app, pool, a, b, &b_ids, false, &mut report).await,
        SweptEdge::McpAgent => check_mcp(app, pool, a, b, &b_ids, true, &mut report).await,
        SweptEdge::McpToken => check_token(app, pool, a, b, &b_ids, &mut report).await,
        SweptEdge::Realtime => check_realtime(app, pool, realtime, a, b, &b_ids, &mut report).await,
    }

    for note in &report.notes {
        eprintln!("note: {note}");
    }
    eprintln!(
        "checked {} HTTP requests and {} MCP calls",
        report.http_requests, report.mcp_calls
    );
    let failures = report.failures.clone();
    fixture.finish().await;

    assert!(
        failures.is_empty(),
        "{} workspace isolation failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[derive(Default)]
struct Report {
    failures: Vec<String>,
    notes: Vec<String>,
    http_requests: usize,
    mcp_calls: usize,
}

impl Report {
    /// Record a leak if `body` carries B's marker or IDs. IDs the caller put
    /// in the request itself (`input`) don't count: echoing them back in an
    /// error ("Project ... wasn't found") tells A nothing new.
    fn leak(&mut self, what: &str, input: &str, body: &str, b_ids: &HashSet<String>) -> bool {
        let input = input.to_ascii_lowercase();
        let found = leaked(body, b_ids)
            .into_iter()
            .filter(|item| {
                item.strip_prefix("id ")
                    .is_none_or(|id| !input.contains(id))
            })
            .collect::<Vec<_>>();
        if found.is_empty() {
            return false;
        }
        let excerpt = body.chars().take(600).collect::<String>();
        self.failures.push(format!(
            "LEAK {what}: carries B data {found:?}\n    body: {excerpt}"
        ));
        true
    }
}

/// The B marker or B IDs found in `body`.
pub(super) fn leaked(body: &str, b_ids: &HashSet<String>) -> Vec<String> {
    let lower = body.to_ascii_lowercase();
    let mut found = Vec::new();
    if lower.contains(MARKER) {
        found.push(format!("marker {MARKER:?}"));
    }
    let bytes = lower.as_bytes();
    let mut seen = BTreeSet::new();
    let mut start = 0;
    while start < bytes.len() {
        if !bytes[start].is_ascii_hexdigit() {
            start += 1;
            continue;
        }
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_hexdigit() || bytes[end] == b'-') {
            end += 1;
        }
        let run = &lower[start..end];
        for width in [36, 32] {
            if run.len() >= width {
                for offset in 0..=run.len() - width {
                    let candidate = &run[offset..offset + width];
                    if b_ids.contains(candidate) {
                        seen.insert(candidate.to_owned());
                    }
                }
            }
        }
        start = end;
    }
    found.extend(seen.into_iter().map(|id| format!("id {id}")));
    found
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

pub(super) struct HttpCase {
    pub(super) route: &'static str,
    /// A's own requests: must succeed and carry nothing of B's.
    pub(super) own: Vec<String>,
    /// Requests naming B's resources: must not return B's data (404 or 403
    /// expected; a success without B data is reported as a note).
    foreign: Vec<String>,
    /// Requests that legitimately succeed (a search for B's words, an
    /// optional filter naming B), but must not return B's data.
    filtered: Vec<String>,
}

pub(super) fn http_cases(a: &World, b: &World) -> Vec<HttpCase> {
    fn case(
        route: &'static str,
        own: Vec<String>,
        foreign: Vec<String>,
        filtered: Vec<String>,
    ) -> HttpCase {
        HttpCase {
            route,
            own,
            foreign,
            filtered,
        }
    }
    let p = |w: &World| format!("/api/v1/projects/{}", w.project);
    // `outer` names the project, `inner` the resource inside it.
    let pair = |f: &dyn Fn(&World, &World) -> String| {
        let mut foreign = vec![f(b, b), f(a, b), f(b, a)];
        foreign.dedup();
        (vec![f(a, a)], foreign)
    };
    let project_route = |route: &'static str, suffix: &'static str| {
        case(
            route,
            vec![format!("{}{suffix}", p(a))],
            vec![format!("{}{suffix}", p(b))],
            vec![],
        )
    };
    let mut cases = vec![
        case(
            "/api/v1/account/connected-apps",
            vec!["/api/v1/account/connected-apps".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/account/profile",
            vec!["/api/v1/account/profile".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/account/email",
            vec!["/api/v1/account/email".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/account/api-tokens",
            vec!["/api/v1/account/api-tokens".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/account/appearance",
            vec!["/api/v1/account/appearance".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/account/security",
            vec!["/api/v1/account/security".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/account/sessions",
            vec!["/api/v1/account/sessions".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/auth/session",
            vec!["/api/v1/auth/session".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/auth/password-reset",
            vec!["/api/v1/auth/password-reset".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/auth/workos",
            vec!["/api/v1/auth/workos".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/dev/bootstrap",
            vec!["/api/v1/dev/bootstrap".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/direct",
            vec!["/api/v1/direct".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/inbox",
            vec![
                "/api/v1/inbox".to_owned(),
                "/api/v1/inbox?view=archived&category=mentions".to_owned(),
                format!("/api/v1/inbox?projectId={}", a.project),
            ],
            vec![],
            vec![format!("/api/v1/inbox?projectId={}", b.project)],
        ),
        case(
            "/api/v1/inbox/summary",
            vec!["/api/v1/inbox/summary".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/later",
            vec![
                "/api/v1/later".to_owned(),
                "/api/v1/later?view=done".to_owned(),
            ],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/scheduled-messages",
            vec![
                "/api/v1/scheduled-messages".to_owned(),
                format!("/api/v1/scheduled-messages?conversationId={}", a.channel),
            ],
            vec![],
            vec![format!(
                "/api/v1/scheduled-messages?conversationId={}",
                b.channel
            )],
        ),
        case(
            "/api/v1/conversations/{id}/notification-level",
            vec![
                format!("/api/v1/conversations/{}/notification-level", a.channel),
                format!("/api/v1/conversations/{}/notification-level", a.dm),
            ],
            vec![
                format!("/api/v1/conversations/{}/notification-level", b.channel),
                format!("/api/v1/conversations/{}/notification-level", b.dm),
            ],
            vec![],
        ),
        case(
            "/api/v1/notification-settings",
            vec![
                "/api/v1/notification-settings".to_owned(),
                format!(
                    "/api/v1/notification-settings?organizationId={}",
                    a.organization
                ),
            ],
            vec![format!(
                "/api/v1/notification-settings?organizationId={}",
                b.organization
            )],
            vec![],
        ),
        case(
            "/api/v1/push/config",
            vec!["/api/v1/push/config".to_owned()],
            vec![format!(
                "/api/v1/push/config?organizationId={}",
                b.organization
            )],
            vec![],
        ),
        case(
            "/api/v1/push/subscriptions",
            vec!["/api/v1/push/subscriptions".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/usage",
            vec![
                "/api/v1/usage".to_owned(),
                format!("/api/v1/usage?organizationId={}", a.organization),
            ],
            vec![],
            vec![format!("/api/v1/usage?organizationId={}", b.organization)],
        ),
        case(
            "/api/v1/usage/accounts/{account_id}",
            vec![format!("/api/v1/usage/accounts/{}", a.provider_account)],
            vec![format!("/api/v1/usage/accounts/{}", b.provider_account)],
            vec![],
        ),
        case(
            "/api/v1/usage/accounts/{account_id}/ready-tasks",
            vec![format!(
                "/api/v1/usage/accounts/{}/ready-tasks",
                a.provider_account
            )],
            vec![format!(
                "/api/v1/usage/accounts/{}/ready-tasks",
                b.provider_account
            )],
            vec![],
        ),
        case(
            "/api/v1/usage/accounts/{account_id}/batch",
            vec![
                format!("/api/v1/usage/accounts/{}/batch", a.provider_account),
                format!(
                    "/api/v1/usage/accounts/{}/batch?windowKey=codex.secondary",
                    a.provider_account
                ),
            ],
            vec![format!(
                "/api/v1/usage/accounts/{}/batch",
                b.provider_account
            )],
            vec![],
        ),
        case(
            "/api/v1/usage/accounts/{account_id}/history",
            vec![format!(
                "/api/v1/usage/accounts/{}/history",
                a.provider_account
            )],
            vec![format!(
                "/api/v1/usage/accounts/{}/history",
                b.provider_account
            )],
            vec![],
        ),
        case(
            "/api/v1/home",
            vec!["/api/v1/home".to_owned()],
            vec![],
            vec![],
        ),
        // A2: what the caller hears, across its own workspaces only.
        case(
            "/api/v1/events",
            vec![
                "/api/v1/events".to_owned(),
                "/api/v1/events?limit=10".to_owned(),
            ],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/events/catalogue",
            vec!["/api/v1/events/catalogue".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/onboarding",
            vec![format!(
                "/api/v1/organizations/{}/onboarding",
                a.organization
            )],
            vec![format!(
                "/api/v1/organizations/{}/onboarding",
                b.organization
            )],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/pages-space",
            vec![format!(
                "/api/v1/organizations/{}/pages-space",
                a.organization
            )],
            vec![format!(
                "/api/v1/organizations/{}/pages-space",
                b.organization
            )],
            vec![],
        ),
        case(
            "/api/v1/projects",
            vec![
                "/api/v1/projects".to_owned(),
                "/api/v1/projects?q=work".to_owned(),
            ],
            vec![],
            vec![format!("/api/v1/projects?q={MARKER}")],
        ),
        case(
            "/api/v1/search",
            vec![
                "/api/v1/search?q=launch".to_owned(),
                format!("/api/v1/search?q=launch&projectId={}", a.project),
            ],
            vec![format!("/api/v1/search?q=launch&projectId={}", b.project)],
            vec![
                format!("/api/v1/search?q={MARKER}"),
                format!("/api/v1/search?q={MARKER}&projectId={}", a.project),
                "/api/v1/search?q=widgets".to_owned(),
            ],
        ),
        project_route("/api/v1/projects/{id}", ""),
        project_route("/api/v1/projects/{id}/channels", "/channels"),
        project_route("/api/v1/projects/{id}/decisions", "/decisions"),
        project_route(
            "/api/v1/projects/{id}/execution-profiles",
            "/execution-profiles",
        ),
        project_route(
            "/api/v1/projects/{id}/execution-workers",
            "/execution-workers",
        ),
        project_route("/api/v1/projects/{id}/harnesses", "/harnesses"),
        project_route(
            "/api/v1/projects/{project_id}/provider-accounts",
            "/provider-accounts",
        ),
        project_route("/api/v1/projects/{id}/navigation", "/navigation"),
        project_route("/api/v1/projects/{id}/pages", "/pages"),
        project_route(
            "/api/v1/projects/{project_id}/external-execution-requests",
            "/external-execution-requests",
        ),
        project_route(
            "/api/v1/projects/{project_id}/integrations/github",
            "/integrations/github",
        ),
        project_route(
            "/api/v1/projects/{project_id}/integrations/github/automation",
            "/integrations/github/automation",
        ),
        project_route(
            "/api/v1/projects/{project_id}/integrations/github/connection",
            "/integrations/github/connection",
        ),
        project_route(
            "/api/v1/projects/{project_id}/integrations/github/available-repositories",
            "/integrations/github/available-repositories",
        ),
        project_route("/api/v1/projects/{project_id}/invitations", "/invitations"),
        project_route(
            "/api/v1/projects/{project_id}/member-candidates",
            "/member-candidates",
        ),
        project_route("/api/v1/projects/{project_id}/members", "/members"),
        project_route("/api/v1/projects/{project_id}/secrets", "/secrets"),
        project_route(
            "/api/v1/projects/{project_id}/repositories",
            "/repositories",
        ),
        case(
            "/api/v1/organizations/{organization_id}",
            vec![format!("/api/v1/organizations/{}", a.organization)],
            vec![format!("/api/v1/organizations/{}", b.organization)],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/members",
            vec![format!("/api/v1/organizations/{}/members", a.organization)],
            vec![format!("/api/v1/organizations/{}/members", b.organization)],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/invitations",
            vec![format!(
                "/api/v1/organizations/{}/invitations",
                a.organization
            )],
            vec![format!(
                "/api/v1/organizations/{}/invitations",
                b.organization
            )],
            vec![],
        ),
        // H1.9: an export's archive is its requester's alone; B's never
        // downloads in A, nor A's under B's address.
        case(
            "/api/v1/organizations/{organization_id}/transfers/{transfer_id}/archive",
            vec![format!(
                "/api/v1/organizations/{}/transfers/{}/archive",
                a.organization, a.export
            )],
            vec![
                format!(
                    "/api/v1/organizations/{}/transfers/{}/archive",
                    b.organization, b.export
                ),
                format!(
                    "/api/v1/organizations/{}/transfers/{}/archive",
                    a.organization, b.export
                ),
            ],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/audit-events/facets",
            vec![format!(
                "/api/v1/organizations/{}/audit-events/facets",
                a.organization
            )],
            vec![format!(
                "/api/v1/organizations/{}/audit-events/facets",
                b.organization
            )],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/audit-events",
            vec![
                format!("/api/v1/organizations/{}/audit-events", a.organization),
                format!(
                    "/api/v1/organizations/{}/audit-events?projectId={}",
                    a.organization, a.project
                ),
            ],
            vec![format!(
                "/api/v1/organizations/{}/audit-events",
                b.organization
            )],
            vec![
                format!(
                    "/api/v1/organizations/{}/audit-events?projectId={}",
                    a.organization, b.project
                ),
                format!(
                    "/api/v1/organizations/{}/audit-events?actorId={}",
                    a.organization, b.rook
                ),
            ],
        ),
        case(
            "/api/v1/organizations/{organization_id}/ai",
            vec![format!("/api/v1/organizations/{}/ai", a.organization)],
            vec![format!("/api/v1/organizations/{}/ai", b.organization)],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/secrets",
            vec![format!("/api/v1/organizations/{}/secrets", a.organization)],
            vec![format!("/api/v1/organizations/{}/secrets", b.organization)],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/mail",
            vec![format!("/api/v1/organizations/{}/mail", a.organization)],
            vec![format!("/api/v1/organizations/{}/mail", b.organization)],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/server",
            vec![format!("/api/v1/organizations/{}/server", a.organization)],
            vec![format!("/api/v1/organizations/{}/server", b.organization)],
            vec![],
        ),
        case(
            "/api/v1/workspaces/by-slug/{slug}",
            vec![format!("/api/v1/workspaces/by-slug/{}", a.slug)],
            vec![format!("/api/v1/workspaces/by-slug/{}", b.slug)],
            vec![],
        ),
        // K4.18: an old address of B's never resolves for A, not even to
        // B's workspace slug.
        case(
            "/api/v1/links/resolve",
            vec![format!(
                "/api/v1/links/resolve?path=/projects/{}/work-items/{}",
                a.project, a.task
            )],
            vec![
                format!(
                    "/api/v1/links/resolve?path=/projects/{}/work-items/{}",
                    b.project, b.task
                ),
                format!("/api/v1/links/resolve?path=/w/{}/inbox", b.slug),
                format!("/api/v1/links/resolve?path=/messages/{}", b.channel),
            ],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/harnesses",
            vec![format!(
                "/api/v1/organizations/{}/harnesses",
                a.organization
            )],
            vec![format!(
                "/api/v1/organizations/{}/harnesses",
                b.organization
            )],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/model-catalog",
            vec![format!(
                "/api/v1/organizations/{}/model-catalog",
                a.organization
            )],
            vec![format!(
                "/api/v1/organizations/{}/model-catalog",
                b.organization
            )],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/personal-secrets",
            vec![format!(
                "/api/v1/organizations/{}/personal-secrets",
                a.organization
            )],
            vec![format!(
                "/api/v1/organizations/{}/personal-secrets",
                b.organization
            )],
            vec![],
        ),
        case(
            "/api/v1/settings/registry",
            vec!["/api/v1/settings/registry".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/settings/effective",
            vec![
                format!(
                    "/api/v1/settings/effective?organizationId={}",
                    a.organization
                ),
                format!("/api/v1/settings/effective?projectId={}", a.project),
                format!(
                    "/api/v1/settings/effective?organizationId={}&projectId={}",
                    a.organization, a.project
                ),
            ],
            vec![
                format!(
                    "/api/v1/settings/effective?organizationId={}",
                    b.organization
                ),
                format!("/api/v1/settings/effective?projectId={}", b.project),
                format!(
                    "/api/v1/settings/effective?organizationId={}&projectId={}",
                    a.organization, b.project
                ),
                format!(
                    "/api/v1/settings/effective?organizationId={}&projectId={}",
                    b.organization, a.project
                ),
            ],
            vec![],
        ),
        project_route(
            "/api/v1/projects/{project_id}/work-item-workflow",
            "/work-item-workflow",
        ),
        project_route(
            "/api/v1/projects/{project_id}/work-items/board",
            "/work-items/board",
        ),
        case(
            "/api/v1/projects/{id}/conversation-sidebar",
            vec![
                format!("{}/conversation-sidebar", p(a)),
                format!(
                    "{}/conversation-sidebar?selectedChannelId={}&expandedChannelId={}",
                    p(a),
                    a.channel,
                    a.channel
                ),
            ],
            vec![format!("{}/conversation-sidebar", p(b))],
            vec![format!(
                "{}/conversation-sidebar?selectedChannelId={}&expandedChannelId={}",
                p(a),
                b.channel,
                b.channel
            )],
        ),
        case(
            "/api/v1/projects/{id}/mentionable-actors",
            vec![
                format!("{}/mentionable-actors", p(a)),
                format!("{}/mentionable-actors?q=rook", p(a)),
            ],
            vec![format!("{}/mentionable-actors?q=rook", p(b))],
            vec![format!("{}/mentionable-actors?q={MARKER}", p(a))],
        ),
        case(
            "/api/v1/projects/{project_id}/external-agents",
            vec![
                format!("{}/external-agents", p(a)),
                format!("/api/v1/projects/{}/external-agents", a.space),
            ],
            vec![
                format!("{}/external-agents", p(b)),
                format!("/api/v1/projects/{}/external-agents", b.space),
            ],
            vec![],
        ),
        case(
            "/api/v1/projects/{project_id}/work-items",
            vec![
                format!("{}/work-items", p(a)),
                format!("{}/work-items?q=launch", p(a)),
            ],
            vec![format!("{}/work-items", p(b))],
            vec![
                format!("{}/work-items?q={MARKER}", p(a)),
                format!("{}/work-items?assigneeActorId={}", p(a), b.rook),
            ],
        ),
        case(
            "/api/v1/projects/{project_id}/work-item-assignees",
            vec![format!("{}/work-item-assignees?q=rook", p(a))],
            vec![format!("{}/work-item-assignees?q=rook", p(b))],
            vec![
                format!("{}/work-item-assignees?q={MARKER}", p(a)),
                format!("{}/work-item-assignees?ids={}", p(a), b.rook),
            ],
        ),
        case(
            "/api/v1/projects/{project_id}/work-item-references",
            vec![format!("{}/work-item-references?q=launch", p(a))],
            vec![format!("{}/work-item-references?q=launch", p(b))],
            vec![format!("{}/work-item-references?q={MARKER}", p(a))],
        ),
        case(
            "/api/v1/projects/{project_id}/work-items/by-key/{key}",
            vec![format!("{}/work-items/by-key/WORK-1", p(a))],
            vec![format!("{}/work-items/by-key/WORK-1", p(b))],
            vec![],
        ),
        project_route("/api/v1/projects/{project_id}/labels", "/labels"),
        project_route("/api/v1/projects/{project_id}/fields", "/fields"),
        case(
            "/api/v1/projects/{project_id}/sheets",
            vec![
                format!("{}/sheets", p(a)),
                format!("{}/sheets?includeArchived=true", p(a)),
            ],
            vec![format!("{}/sheets", p(b))],
            vec![],
        ),
        {
            let (mut own, foreign) =
                pair(&|outer, inner| format!("{}/sheets/{}", p(outer), inner.sheet));
            own.push(format!("{}/sheets/{}?tab=Sheet1", p(a), a.sheet));
            case(
                "/api/v1/projects/{project_id}/sheets/{sheet_id}",
                own,
                foreign,
                vec![],
            )
        },
        {
            let (mut own, foreign) = pair(&|outer, inner| {
                format!("{}/sheets/{}/export?format=csv", p(outer), inner.sheet)
            });
            own.push(format!("{}/sheets/{}/export?format=xlsx", p(a), a.sheet));
            case(
                "/api/v1/projects/{project_id}/sheets/{sheet_id}/export",
                own,
                foreign,
                vec![],
            )
        },
        case(
            "/api/v1/projects/{project_id}/collections",
            vec![
                format!("{}/collections", p(a)),
                format!("{}/collections?includeArchived=true", p(a)),
            ],
            vec![format!("{}/collections", p(b))],
            vec![],
        ),
        {
            let (own, foreign) =
                pair(&|outer, inner| format!("{}/collections/{}", p(outer), inner.collection));
            case(
                "/api/v1/projects/{project_id}/collections/{collection_id}",
                own,
                foreign,
                vec![],
            )
        },
        {
            let (mut own, foreign) = pair(&|outer, inner| {
                format!("{}/collections/{}/records", p(outer), inner.collection)
            });
            let records = format!("{}/collections/{}/records", p(a), a.collection);
            own.push(format!("{records}?view={}", a.collection_view));
            own.push(format!("{records}?search=crash&archived=include&limit=1"));
            case(
                "/api/v1/projects/{project_id}/collections/{collection_id}/records",
                own,
                foreign,
                vec![
                    // B's view, B's field, B's people, task and page, and
                    // B's words never find anything here.
                    format!("{records}?view={}", b.collection_view),
                    format!("{records}?search={MARKER}"),
                    format!("{records}?groupBy={}", b.collection_field),
                    format!(
                        "{records}?filters={}",
                        url_encode(
                            &json!([{"field": b.collection_field, "op": "notEmpty"}]).to_string()
                        )
                    ),
                    format!(
                        "{records}?sort={}",
                        url_encode(
                            &json!([{"field": b.collection_field.to_string(), "direction": "asc"}])
                                .to_string()
                        )
                    ),
                ],
            )
        },
        {
            let (own, foreign) = pair(&|outer, inner| {
                format!(
                    "{}/collections/{}/records/{}",
                    p(outer),
                    inner.collection,
                    inner.record
                )
            });
            let mut foreign = foreign;
            // A's collection with B's record, and B's collection with A's.
            foreign.push(format!(
                "{}/collections/{}/records/{}",
                p(a),
                a.collection,
                b.record
            ));
            foreign.push(format!(
                "{}/collections/{}/records/{}",
                p(a),
                b.collection,
                a.record
            ));
            foreign.dedup();
            case(
                "/api/v1/projects/{project_id}/collections/{collection_id}/records/{record_id}",
                own,
                foreign,
                vec![],
            )
        },
        project_route("/api/v1/projects/{project_id}/launches", "/launches"),
        project_route(
            "/api/v1/projects/{project_id}/agent-profiles",
            "/agent-profiles",
        ),
        project_route("/api/v1/projects/{project_id}/agents", "/agents"),
        case(
            "/api/v1/runs",
            vec![
                "/api/v1/runs".to_owned(),
                format!("/api/v1/runs?organizationId={}", a.organization),
                format!("/api/v1/runs?projectId={}&status=needs_you", a.project),
            ],
            vec![],
            vec![
                format!("/api/v1/runs?organizationId={}", b.organization),
                format!("/api/v1/runs?projectId={}", b.project),
            ],
        ),
        case(
            "/api/v1/runs/usage",
            vec![
                "/api/v1/runs/usage".to_owned(),
                "/api/v1/runs/usage?groupBy=model".to_owned(),
                "/api/v1/runs/usage?groupBy=account".to_owned(),
                format!(
                    "/api/v1/runs/usage?groupBy=project&organizationId={}",
                    a.organization
                ),
            ],
            vec![],
            vec![
                format!(
                    "/api/v1/runs/usage?organizationId={}&groupBy=model",
                    b.organization
                ),
                format!("/api/v1/runs/usage?projectId={}", b.project),
            ],
        ),
        case(
            "/api/v1/agent-runs/{run_id}",
            vec![format!("/api/v1/agent-runs/{}", a.run)],
            vec![format!("/api/v1/agent-runs/{}", b.run)],
            vec![],
        ),
        case(
            "/api/v1/agent-runs/{run_id}/diff",
            vec![format!("/api/v1/agent-runs/{}/diff", a.run)],
            vec![format!("/api/v1/agent-runs/{}/diff", b.run)],
            vec![],
        ),
        project_route(
            "/api/v1/projects/{project_id}/environments",
            "/environments",
        ),
        project_route("/api/v1/projects/{project_id}/milestones", "/milestones"),
        project_route("/api/v1/projects/{project_id}/ai", "/ai"),
        project_route("/api/v1/projects/{project_id}/ai/plans", "/ai/plans"),
        case(
            "/api/v1/projects/{project_id}/ai/suggestions",
            vec![
                format!("{}/ai/suggestions", p(a)),
                format!("{}/ai/suggestions?workItemId={}&status=all", p(a), a.task),
            ],
            vec![format!("{}/ai/suggestions", p(b))],
            vec![format!("{}/ai/suggestions?workItemId={}", p(a), b.task)],
        ),
        case(
            "/api/v1/projects/{project_id}/ai/ready",
            vec![
                format!("{}/ai/ready", p(a)),
                format!("{}/ai/ready?scope=all", p(a)),
            ],
            vec![format!("{}/ai/ready?scope=all", p(b))],
            vec![],
        ),
        case(
            "/api/v1/projects/{project_id}/ai/graph",
            vec![
                format!("{}/ai/graph", p(a)),
                format!("{}/ai/graph?includeFinished=true", p(a)),
            ],
            vec![format!("{}/ai/graph", p(b))],
            vec![],
        ),
        project_route("/api/v1/projects/{project_id}/automations", "/automations"),
        project_route(
            "/api/v1/projects/{project_id}/bug-report-links",
            "/bug-report-links",
        ),
        project_route("/api/v1/projects/{project_id}/layout", "/layout"),
        {
            let (own, foreign) =
                pair(&|outer, inner| format!("{}/automations/{}/runs", p(outer), inner.rule));
            case(
                "/api/v1/projects/{project_id}/automations/{rule_id}/runs",
                own,
                foreign,
                vec![],
            )
        },
        project_route("/api/v1/projects/{project_id}/timeline", "/timeline"),
        project_route(
            "/api/v1/projects/{project_id}/task-templates",
            "/task-templates",
        ),
        project_route("/api/v1/projects/{project_id}/imports", "/imports"),
        case(
            "/api/v1/projects/{project_id}/export",
            vec![
                format!("{}/export", p(a)),
                format!("{}/export?format=csv&includeArchived=true", p(a)),
            ],
            vec![
                format!("{}/export", p(b)),
                format!("{}/export?format=csv", p(b)),
            ],
            vec![],
        ),
        case(
            "/api/v1/my-work",
            vec!["/api/v1/my-work".to_owned()],
            vec![],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/layout",
            vec![format!("/api/v1/organizations/{}/layout", a.organization)],
            vec![format!("/api/v1/organizations/{}/layout", b.organization)],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/modules",
            vec![format!("/api/v1/organizations/{}/modules", a.organization)],
            vec![format!("/api/v1/organizations/{}/modules", b.organization)],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/workspace-templates",
            vec![format!(
                "/api/v1/organizations/{}/workspace-templates",
                a.organization
            )],
            vec![format!(
                "/api/v1/organizations/{}/workspace-templates",
                b.organization
            )],
            vec![],
        ),
        case(
            "/api/v1/things/{id}",
            vec![
                format!("/api/v1/things/{}", a.task),
                format!("/api/v1/things/{}", a.page),
                format!("/api/v1/things/{}", a.project_whiteboard),
            ],
            vec![
                format!("/api/v1/things/{}", b.task),
                format!("/api/v1/things/{}", b.page),
                format!("/api/v1/things/{}", b.thread),
                format!("/api/v1/things/{}", b.dm),
                format!("/api/v1/things/{}", b.whiteboard),
            ],
            vec![],
        ),
        case(
            "/api/v1/things/{kind}/{id}",
            vec![
                format!("/api/v1/things/task/{}", a.task),
                format!("/api/v1/things/page/{}", a.page),
                format!("/api/v1/things/whiteboard/{}", a.project_whiteboard),
            ],
            vec![
                format!("/api/v1/things/task/{}", b.task),
                format!("/api/v1/things/page/{}", b.page),
                format!("/api/v1/things/thread/{}", b.thread),
                format!("/api/v1/things/conversation/{}", b.dm),
                format!("/api/v1/things/whiteboard/{}", b.whiteboard),
            ],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/roles",
            vec![format!("/api/v1/organizations/{}/roles", a.organization)],
            vec![format!("/api/v1/organizations/{}/roles", b.organization)],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/permissions",
            vec![
                format!("/api/v1/organizations/{}/permissions", a.organization),
                format!(
                    "/api/v1/organizations/{}/permissions?projectId={}&actorId={}",
                    a.organization, a.project, a.rook
                ),
            ],
            vec![
                format!("/api/v1/organizations/{}/permissions", b.organization),
                format!(
                    "/api/v1/organizations/{}/permissions?projectId={}",
                    a.organization, b.project
                ),
                format!(
                    "/api/v1/organizations/{}/permissions?actorId={}",
                    a.organization, b.rook
                ),
            ],
            vec![],
        ),
        case(
            "/api/v1/organizations/{organization_id}/project-templates",
            vec![format!(
                "/api/v1/organizations/{}/project-templates?includeArchived=true",
                a.organization
            )],
            vec![format!(
                "/api/v1/organizations/{}/project-templates",
                b.organization
            )],
            vec![],
        ),
        case(
            "/api/v1/projects/{project_id}/work-items/groups",
            ["status", "assignee", "label", "milestone", "parent"]
                .iter()
                .map(|by| format!("{}/work-items/groups?groupBy={by}", p(a)))
                .collect(),
            vec![format!("{}/work-items/groups?groupBy=label", p(b))],
            vec![
                format!("{}/work-items/groups?groupBy=label&q={MARKER}", p(a)),
                format!(
                    "{}/work-items/groups?groupBy=assignee&assigneeActorId={}",
                    p(a),
                    b.rook
                ),
            ],
        ),
    ];

    for (route, suffix) in [
        (
            "/api/v1/projects/{project_id}/work-items/{work_item_id}",
            "",
        ),
        (
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria",
            "/acceptance-criteria",
        ),
        (
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/claim",
            "/claim",
        ),
        (
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/readiness",
            "/readiness",
        ),
        (
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/launch",
            "/launch",
        ),
        (
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/timeline",
            "/timeline",
        ),
        (
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/files",
            "/files",
        ),
    ] {
        let (mut own, mut foreign) = pair(&|outer, inner| {
            format!(
                "/api/v1/projects/{}/work-items/{}{suffix}",
                outer.project, inner.task
            )
        });
        if suffix != "/claim" {
            own.push(format!(
                "/api/v1/projects/{}/work-items/{}{suffix}",
                a.project, a.subtask
            ));
            foreign.push(format!(
                "/api/v1/projects/{}/work-items/{}{suffix}",
                a.project, b.subtask
            ));
        }
        cases.push(case(route, own, foreign, vec![]));
    }
    // AWD-102: a move's preview. A has one ordinary project in this fixture
    // and a move needs two, so A's own preview is in awd102_task_move; here
    // B's task, project or target project never answer.
    {
        let (_, mut foreign) = pair(&|outer, inner| {
            format!(
                "/api/v1/projects/{}/work-items/{}/project-move?targetProjectId={}",
                outer.project, inner.task, a.project
            )
        });
        foreign.push(format!(
            "/api/v1/projects/{}/work-items/{}/project-move?targetProjectId={}",
            a.project, a.task, b.project
        ));
        cases.push(case(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/project-move",
            vec![],
            foreign,
            vec![],
        ));
        // Where A's task could go: never one of B's projects.
        let (own, foreign) = pair(&|outer, inner| {
            format!(
                "/api/v1/projects/{}/work-items/{}/project-move/destinations",
                outer.project, inner.task
            )
        });
        cases.push(case(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/project-move/destinations",
            own,
            foreign,
            vec![],
        ));
    }

    let (own, foreign) = pair(&|outer, inner| {
        format!(
            "/api/v1/projects/{}/work-items/{}/comments/{}/revisions",
            outer.project, outer.task, inner.comment
        )
    });
    cases.push(case(
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/comments/{comment_id}/revisions",
        own,
        foreign,
        vec![],
    ));

    let (own, foreign) =
        pair(&|outer, inner| format!("/api/v1/projects/{}/ai/plans/{}", outer.project, inner.plan));
    cases.push(case(
        "/api/v1/projects/{project_id}/ai/plans/{import_id}",
        own,
        foreign,
        vec![],
    ));
    let (own, foreign) = pair(&|outer, inner| {
        format!(
            "/api/v1/projects/{}/execution-profiles/{}/account",
            outer.project, inner.profile
        )
    });
    cases.push(case(
        "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account",
        own,
        foreign,
        vec![],
    ));
    let (own, foreign) = pair(&|outer, inner| {
        format!(
            "/api/v1/projects/{}/provider-accounts/{}",
            outer.project, inner.provider_account
        )
    });
    cases.push(case(
        "/api/v1/projects/{project_id}/provider-accounts/{account_id}",
        own,
        foreign,
        vec![],
    ));
    let (own, foreign) = pair(&|outer, inner| {
        format!(
            "/api/v1/projects/{}/external-agents/{}/workos",
            outer.space, inner.agent_connection
        )
    });
    cases.push(case(
        "/api/v1/projects/{project_id}/external-agents/{connection_id}/workos",
        own,
        foreign,
        vec![],
    ));

    let conversations = |w: &World| {
        vec![
            w.channel.clone(),
            w.thread.clone(),
            w.dm.clone(),
            w.group.clone(),
            w.run_thread.to_string(),
        ]
    };
    cases.push(case(
        "/api/v1/conversations/{id}/timeline",
        conversations(a)
            .iter()
            .map(|id| format!("/api/v1/conversations/{id}/timeline"))
            .collect(),
        conversations(b)
            .iter()
            .map(|id| format!("/api/v1/conversations/{id}/timeline"))
            .collect(),
        vec![],
    ));
    cases.push(case(
        "/api/v1/conversations/{id}/threads",
        vec![format!("/api/v1/conversations/{}/threads", a.channel)],
        vec![
            format!("/api/v1/conversations/{}/threads", b.channel),
            format!("/api/v1/conversations/{}/threads", b.dm),
        ],
        vec![],
    ));
    cases.push(case(
        "/api/v1/conversations/{id}/pins",
        vec![format!("/api/v1/conversations/{}/pins", a.channel)],
        vec![
            format!("/api/v1/conversations/{}/pins", b.channel),
            format!("/api/v1/conversations/{}/pins", b.dm),
        ],
        vec![],
    ));
    cases.push(case(
        "/api/v1/conversations/{conversation_id}/agent-sessions",
        vec![format!("/api/v1/conversations/{}/agent-sessions", a.thread)],
        vec![
            format!("/api/v1/conversations/{}/agent-sessions", b.thread),
            format!("/api/v1/conversations/{}/agent-sessions", b.dm),
        ],
        vec![],
    ));
    for (route, suffix) in [
        ("/api/v1/conversations/{id}/messages/{message_id}", ""),
        (
            "/api/v1/conversations/{id}/messages/{message_id}/revisions",
            "/revisions",
        ),
    ] {
        let uri = |conversation: &str, message: &str| {
            format!("/api/v1/conversations/{conversation}/messages/{message}{suffix}")
        };
        cases.push(case(
            route,
            vec![uri(&a.channel, &a.message), uri(&a.dm, &a.dm_message)],
            vec![
                uri(&b.channel, &b.message),
                uri(&a.channel, &b.message),
                uri(&b.channel, &a.message),
                uri(&b.dm, &b.dm_message),
                uri(&a.dm, &b.dm_message),
            ],
            vec![],
        ));
    }
    let (own, foreign) = pair(&|outer, inner| format!("{}/imports/{}", p(outer), inner.import));
    cases.push(case(
        "/api/v1/projects/{project_id}/imports/{import_id}",
        own,
        foreign,
        vec![],
    ));
    cases.push(case(
        "/api/v1/pages/{id}",
        vec![format!("/api/v1/pages/{}", a.page)],
        vec![format!("/api/v1/pages/{}", b.page)],
        vec![],
    ));
    cases.push(case(
        "/api/v1/pages/{id}/document",
        vec![format!("/api/v1/pages/{}/document", a.page)],
        vec![format!("/api/v1/pages/{}/document", b.page)],
        vec![],
    ));
    cases.push(case(
        "/api/v1/pages/{id}/export",
        vec![
            format!("/api/v1/pages/{}/export", a.page),
            format!("/api/v1/pages/{}/export?format=docx", a.page),
        ],
        vec![
            format!("/api/v1/pages/{}/export", b.page),
            format!("/api/v1/pages/{}/export?format=docx", b.page),
        ],
        vec![],
    ));
    cases.push(case(
        "/api/v1/pages/{id}/discussion",
        vec![format!("/api/v1/pages/{}/discussion", a.page)],
        vec![format!("/api/v1/pages/{}/discussion", b.page)],
        vec![],
    ));
    let boards = |w: &World, suffix: &str| {
        vec![
            format!("/api/v1/whiteboards/{}{suffix}", w.whiteboard),
            format!("/api/v1/whiteboards/{}{suffix}", w.project_whiteboard),
        ]
    };
    cases.push(case(
        "/api/v1/whiteboards",
        vec![
            "/api/v1/whiteboards".to_owned(),
            "/api/v1/whiteboards?q=Sketch".to_owned(),
            "/api/v1/whiteboards?deleted=true".to_owned(),
            format!("/api/v1/whiteboards?projectId={}", a.project),
        ],
        vec![],
        vec![
            format!("/api/v1/whiteboards?organizationId={}", b.organization),
            format!("/api/v1/whiteboards?projectId={}", b.project),
            format!("/api/v1/whiteboards?q={MARKER}"),
        ],
    ));
    cases.push(case(
        "/api/v1/whiteboards/{id}",
        boards(a, ""),
        boards(b, ""),
        vec![],
    ));
    cases.push(case(
        "/api/v1/whiteboards/{id}/scene",
        [boards(a, "/scene"), boards(a, "/scene?since=0")].concat(),
        [boards(b, "/scene"), boards(b, "/scene?since=0")].concat(),
        vec![],
    ));
    cases.push(case(
        "/api/v1/whiteboards/{id}/files/{file_id}",
        vec![format!("/api/v1/whiteboards/{}/files/photo1", a.whiteboard)],
        vec![format!("/api/v1/whiteboards/{}/files/photo1", b.whiteboard)],
        vec![],
    ));
    let files = |w: &World, suffix: &str| {
        vec![
            format!("/api/v1/files/{}{suffix}", w.file),
            format!("/api/v1/files/{}{suffix}", w.task_file),
        ]
    };
    cases.push(case(
        "/api/v1/files/{id}",
        files(a, ""),
        files(b, ""),
        vec![],
    ));
    cases.push(case(
        "/api/v1/files/{id}/content",
        [files(a, "/content"), files(a, "/content?download=1")].concat(),
        files(b, "/content"),
        vec![],
    ));
    // A signed link is its own credential (K5.12): A's owner's link to A's
    // file opens; one made for A's owner naming B's file never does.
    cases.push(case(
        "/api/v1/file-links/{token}",
        vec![a.file_link.clone()],
        vec![a.foreign_file_link.clone()],
        vec![],
    ));
    cases.push(case(
        "/api/v1/files/{id}/thumbnail",
        // Only images have one.
        vec![format!("/api/v1/files/{}/thumbnail", a.task_file)],
        files(b, "/thumbnail"),
        vec![],
    ));
    cases.push(case(
        "/api/v1/link-previews/{id}/image",
        vec![format!("/api/v1/link-previews/{}/image", a.link_preview)],
        vec![format!("/api/v1/link-previews/{}/image", b.link_preview)],
        vec![],
    ));
    cases.push(case(
        "/api/v1/whiteboards/{id}/preview.svg",
        boards(a, "/preview.svg"),
        boards(b, "/preview.svg"),
        vec![],
    ));
    let linked = |w: &World| {
        vec![
            format!(
                "/api/v1/whiteboard-links?targetKind=task&targetId={}",
                w.task
            ),
            format!(
                "/api/v1/whiteboard-links?targetKind=project&targetId={}",
                w.project
            ),
            format!(
                "/api/v1/whiteboard-links?targetKind=page&targetId={}",
                w.page
            ),
        ]
    };
    cases.push(case(
        "/api/v1/whiteboard-links",
        linked(a),
        linked(b),
        vec![],
    ));
    // C2.17: an avatar shows only to people who share a workspace.
    cases.push(case(
        "/api/v1/accounts/{id}/avatar",
        vec![format!("/api/v1/accounts/{}/avatar", a.rook)],
        vec![format!("/api/v1/accounts/{}/avatar", b.rook)],
        vec![],
    ));
    // C2.19: a workspace's own emoji shows only to its members.
    cases.push(case(
        "/api/v1/custom-emoji/{id}",
        vec![format!("/api/v1/custom-emoji/{}", a.custom_emoji)],
        vec![format!("/api/v1/custom-emoji/{}", b.custom_emoji)],
        vec![],
    ));
    cases.push(case(
        "/api/v1/pages/{id}/revisions/{version}",
        vec![
            format!("/api/v1/pages/{}/revisions/1", a.page),
            format!("/api/v1/pages/{}/revisions/2", a.page),
        ],
        vec![
            format!("/api/v1/pages/{}/revisions/1", b.page),
            format!("/api/v1/pages/{}/revisions/2", b.page),
        ],
        vec![],
    ));
    cases
}

pub(super) async fn get_text(
    app: &axum::Router,
    browser: &Browser,
    uri: &str,
) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(browser.request("GET", uri, None, Value::Null))
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 8_000_000)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

async fn check_http(
    app: &axum::Router,
    a: &World,
    b: &World,
    b_ids: &HashSet<String>,
    report: &mut Report,
) {
    let cases = http_cases(a, b);
    let cased = cases.iter().map(|case| case.route).collect::<BTreeSet<_>>();
    let listed = HTTP_ROUTES.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(
        cased, listed,
        "HTTP_ROUTES and http_cases must name the same routes"
    );

    // Positive control: B reading its own data is caught by the detector, so
    // a clean result for A means something.
    let (status, body) = get_text(
        app,
        &b.browser,
        &format!("/api/v1/conversations/{}/timeline", b.channel),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "B's own timeline: {body}");
    let detected = leaked(&body, b_ids);
    assert!(
        detected.iter().any(|item| item.starts_with("marker"))
            && detected.iter().any(|item| item.starts_with("id ")),
        "the leak detector misses B's own timeline: {detected:?}"
    );

    for case in &cases {
        for uri in &case.own {
            report.http_requests += 1;
            let (status, body) = get_text(app, &a.browser, uri).await;
            if !status.is_success() {
                report.failures.push(format!(
                    "OWN {} GET {uri} -> {status} (A's own request must succeed): {}",
                    case.route,
                    body.chars().take(300).collect::<String>()
                ));
            }
            report.leak(&format!("A GET {uri} -> {status}"), uri, &body, b_ids);
        }
        for uri in &case.foreign {
            report.http_requests += 1;
            let (status, body) = get_text(app, &a.browser, uri).await;
            let leaked = report.leak(
                &format!("A GET {uri} (B's IDs) -> {status}"),
                uri,
                &body,
                b_ids,
            );
            if status.is_success() && !leaked {
                report.notes.push(format!(
                    "{} GET {uri} with B's IDs answered {status} (no B data, but 404/403 was expected)",
                    case.route
                ));
            }
        }
        for uri in &case.filtered {
            report.http_requests += 1;
            let (status, body) = get_text(app, &a.browser, uri).await;
            report.leak(&format!("A GET {uri} -> {status}"), uri, &body, b_ids);
        }
    }
}

// ---------------------------------------------------------------------------
// MCP
// ---------------------------------------------------------------------------

pub(super) struct McpCase {
    pub(super) tool: &'static str,
    /// Who must succeed with `own`: most tools are for A's connected app
    /// (acting as A's owner); a few are only for agents.
    agent_only: bool,
    pub(super) own: Vec<Value>,
    foreign: Vec<Value>,
    filtered: Vec<Value>,
}

/// The tool names `mcp_cases` covers, for the static guard.
fn mcp_case_names() -> BTreeSet<String> {
    MCP_CASE_TOOLS
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

const MCP_CASE_TOOLS: &[&str] = &[
    "agent_list",
    "agent_profile_list",
    "agent_template_list",
    "ai_suggestions_list",
    "automation_history",
    "automation_list",
    "incoming_hook_list",
    "briefing_get",
    "bug_report_link_list",
    "change_list",
    "connection_list",
    "proposal_list",
    "connection_delivery_list",
    "schedule_list",
    "follow_list",
    "guard_list",
    "memory_page_open",
    "conversation_agent_settings_list",
    "conversation_list",
    "conversation_open",
    "conversation_read",
    "credential_list",
    "decision_list",
    "dependency_graph_get",
    "environment_list",
    "event_kind_list",
    "event_list",
    "external_event_poll",
    "file_list",
    "file_read",
    "file_link_get",
    "history_list",
    "history_show",
    "activity_list",
    "invite_link_list",
    "job_get",
    "collection_list",
    "collection_describe",
    "formula_preview",
    "button_get",
    "record_query",
    "sheet_list",
    "sheet_read",
    "sheet_search",
    "sheet_view_list",
    "host_list",
    "account_joinable_list",
    "agent_offer_list",
    "agent_action_get",
    "agent_recipe_list",
    "inbox_list",
    "invitation_list",
    "label_list",
    "field_list",
    "later_list",
    "request_list",
    "request_get",
    "launch_list",
    "message_history",
    "conversation_member_list",
    "conversation_task_list",
    "conversation_file_list",
    "conversation_tab_list",
    "channel_list",
    "channel_get",
    "message_draft_list",
    "custom_emoji_list",
    "theme_list",
    "theme_get",
    "comment_list",
    "dashboard_get",
    "dashboard_list",
    "data_connection_list",
    "data_query_list",
    "data_query_get",
    "whiteboard_template_list",
    "milestone_list",
    "cycle_list",
    "triage_rule_list",
    "initiative_list",
    "project_status_get",
    "form_list",
    "form_get",
    "record_template_list",
    "recurring_record_list",
    "module_list",
    "module_open",
    "my_permissions",
    "my_work_get",
    "notification_list",
    "directory_list",
    "notification_keyword_list",
    "sidebar_list",
    "profile_get",
    "profile_avatar_list",
    "group_list",
    "saved_search_list",
    "inbox_view_list",
    "place_search",
    "dm_standing_get",
    "account_block_list",
    "package_export",
    "package_list",
    "package_preview",
    "page_blocks",
    "page_comments",
    "page_get",
    "page_suggestions",
    "page_list",
    "page_template_list",
    "people_search",
    "project_export",
    "project_members",
    "project_get",
    "project_template_list",
    "project_list",
    "pull_request_get",
    "pull_request_list",
    "repository_get",
    "repository_list",
    "role_get",
    "role_list",
    "run_activity_list",
    "run_detail_get",
    "run_get",
    "run_list",
    "run_steps",
    "run_usage",
    "agent_usage_list",
    "workspace_search",
    "settings_get",
    "space_list",
    "space_open",
    "space_personal_open",
    "status_list",
    "skill_get",
    "skill_list",
    "spend_limit_list",
    "task_list",
    "task_get",
    "task_ready_list",
    "template_list",
    "thing_open",
    "thread_followed_list",
    "usage_batch",
    "usage_windows",
    "view_list",
    "whiteboard_list",
    "whiteboard_read",
    "whiteboard_render",
    "workflow_get",
    "workspace_store_get",
    "workspace_template_list",
    "workspace_transfer_get",
    "workspace_transfer_list",
    "workspace_usage_get",
];

pub(super) fn mcp_cases(a: &World, b: &World) -> Vec<McpCase> {
    fn case(
        tool: &'static str,
        own: Vec<Value>,
        foreign: Vec<Value>,
        filtered: Vec<Value>,
    ) -> McpCase {
        McpCase {
            tool,
            agent_only: matches!(
                tool,
                "inbox_list" | "external_event_poll" | "credential_list"
            ),
            own,
            foreign,
            filtered,
        }
    }
    let by_project = |tool: &'static str| {
        case(
            tool,
            vec![
                json!({"projectId": "WORK"}),
                json!({"projectId": a.project}),
            ],
            vec![
                json!({"projectId": b.project}),
                json!({"projectId": b.space}),
            ],
            vec![],
        )
    };
    let conversations = |w: &World| {
        vec![
            w.channel.clone(),
            w.thread.clone(),
            w.dm.clone(),
            w.group.clone(),
            w.run_thread.to_string(),
        ]
    };
    vec![
        case(
            "briefing_get",
            vec![json!({}), json!({"projectId": "WORK"})],
            vec![json!({"projectId": b.project})],
            vec![],
        ),
        case(
            "project_list",
            vec![json!({}), json!({"includeArchived": true})],
            vec![],
            vec![],
        ),
        by_project("project_get"),
        // K4.7: another workspace's spaces are out of reach.
        case(
            "space_list",
            vec![
                json!({}),
                json!({"includeArchived": true, "kinds": ["project", "team"]}),
            ],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        case(
            "space_open",
            vec![json!({"spaceId": "WORK"}), json!({"spaceId": a.project})],
            vec![json!({"spaceId": b.project}), json!({"spaceId": b.space})],
            vec![],
        ),
        // K4.9: your own space is in your workspace only.
        case(
            "space_personal_open",
            vec![json!({})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        by_project("workflow_get"),
        by_project("project_members"),
        by_project("invitation_list"),
        by_project("role_list"),
        by_project("my_permissions"),
        by_project("workspace_template_list"),
        by_project("project_template_list"),
        case(
            "role_get",
            vec![
                json!({"projectId": "WORK", "role": "member"}),
                json!({"projectId": a.project, "role": "Viewer"}),
            ],
            vec![json!({"projectId": b.project, "role": "member"})],
            vec![],
        ),
        by_project("label_list"),
        by_project("field_list"),
        case(
            "collection_list",
            vec![
                json!({"projectId": "WORK"}),
                json!({"projectId": a.project, "includeArchived": true}),
            ],
            vec![
                json!({"projectId": b.project}),
                json!({"projectId": b.space}),
            ],
            vec![],
        ),
        case(
            "sheet_list",
            vec![
                json!({"projectId": "WORK"}),
                json!({"projectId": a.project, "includeArchived": true}),
            ],
            vec![
                json!({"projectId": b.project}),
                json!({"projectId": b.space}),
            ],
            vec![],
        ),
        case(
            "sheet_read",
            vec![
                json!({"projectId": "WORK", "sheet": "Budget"}),
                json!({"projectId": a.project, "sheet": a.sheet, "range": "A1:B3"}),
                json!({"projectId": "WORK", "sheet": "Budget", "asRows": true, "where": {"Item": "Rent"}}),
            ],
            vec![
                json!({"projectId": "WORK", "sheet": b.sheet}),
                json!({"projectId": b.project, "sheet": b.sheet}),
            ],
            vec![],
        ),
        case(
            "collection_describe",
            vec![
                json!({"projectId": "WORK", "collection": "Playtests"}),
                json!({"projectId": a.project, "collection": a.collection, "includeArchived": true}),
            ],
            vec![
                json!({"projectId": "WORK", "collection": b.collection}),
                json!({"projectId": b.project, "collection": b.collection}),
            ],
            vec![],
        ),
        case(
            "formula_preview",
            vec![
                json!({"collection": a.collection, "formula": "1 + 1"}),
                json!({"collection": a.collection, "formula": "{Title} & \"!\"", "record": 1}),
            ],
            vec![json!({"collection": b.collection, "formula": "1 + 1"})],
            vec![],
        ),
        case(
            "button_get",
            vec![json!({"collection": a.collection, "field": a.collection_button})],
            vec![
                json!({"collection": b.collection, "field": b.collection_button}),
                json!({"collection": a.collection, "field": b.collection_button}),
            ],
            vec![],
        ),
        case(
            "record_query",
            vec![
                json!({"projectId": "WORK", "collection": "Playtests"}),
                json!({"projectId": "WORK", "collection": a.collection, "records": ["#1", a.record], "includeHistory": true}),
                json!({"projectId": "WORK", "collection": "Playtests", "view": a.collection_view, "search": "crash"}),
                json!({
                    "projectId": "WORK",
                    "collection": "Playtests",
                    "filters": [
                        {"field": "Tester", "op": "is", "values": ["@rook"]},
                        {"field": "Build", "op": "is", "values": ["WORK-1"]},
                        {"field": "Spec", "op": "is", "values": [a.page]},
                    ],
                    "groupBy": "Severity",
                }),
            ],
            vec![
                json!({"projectId": "WORK", "collection": b.collection}),
                json!({"projectId": "WORK", "collection": a.collection, "records": [b.record]}),
                json!({"projectId": "WORK", "collection": a.collection, "view": b.collection_view}),
                json!({"projectId": "WORK", "collection": a.collection, "groupBy": b.collection_field}),
                json!({"projectId": "WORK", "collection": a.collection, "filters": [{"field": b.collection_field, "op": "notEmpty"}]}),
                json!({"projectId": "WORK", "collection": a.collection, "filters": [{"field": "Spec", "op": "is", "values": [b.page]}]}),
                json!({"projectId": "WORK", "collection": a.collection, "filters": [{"field": "Tester", "op": "is", "values": [b.rook]}]}),
                json!({"projectId": b.project, "collection": b.collection}),
            ],
            vec![
                json!({"projectId": "WORK", "collection": a.collection, "search": MARKER}),
                json!({"projectId": "WORK", "collection": a.collection, "filters": [{"field": "Build", "op": "is", "values": [b.task]}]}),
            ],
        ),
        by_project("launch_list"),
        by_project("agent_profile_list"),
        by_project("environment_list"),
        by_project("page_list"),
        by_project("skill_list"),
        by_project("decision_list"),
        by_project("incoming_hook_list"),
        // C1.24: the template galleries of a space or a workspace.
        by_project("page_template_list"),
        case(
            "whiteboard_template_list",
            vec![json!({}), json!({"spaceId": a.project})],
            vec![
                json!({"spaceId": b.project}),
                json!({"workspace": b.organization}),
            ],
            vec![],
        ),
        // K5.20: B's own bucket never shows in A.
        case(
            "workspace_store_get",
            vec![json!({})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        // H1.25: B's usage never shows in A.
        case(
            "workspace_usage_get",
            vec![json!({})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        // H1.27: B's spend limits never show in A.
        case(
            "spend_limit_list",
            vec![json!({})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        // H1.9: B's exports never show in A.
        case(
            "workspace_transfer_list",
            vec![json!({})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        case(
            "workspace_transfer_get",
            vec![json!({"transferId": a.export})],
            vec![
                json!({"workspace": b.organization, "transferId": b.export}),
                json!({"transferId": b.export}),
            ],
            vec![],
        ),
        // C2.19: B's own emoji never list in A.
        case(
            "custom_emoji_list",
            vec![json!({}), json!({"includeRetired": true})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        // B's themes never list or read in A, offered or not.
        case(
            "theme_list",
            vec![json!({})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        case(
            "theme_get",
            vec![json!({"id": a.theme})],
            vec![
                json!({"id": b.theme}),
                json!({"workspace": b.organization, "id": b.theme}),
            ],
            vec![],
        ),
        // C1.3: B's things' discussions never list in A.
        case(
            "comment_list",
            vec![json!({"thing": {"kind": "sheet", "id": a.sheet}})],
            vec![
                json!({"thing": {"kind": "sheet", "id": b.sheet}}),
                json!({"thing": {"kind": "whiteboard", "id": b.whiteboard}}),
            ],
            vec![],
        ),
        // C2.28: B's drafts never show in A, by conversation or listed.
        case(
            "message_draft_list",
            vec![json!({}), json!({"conversationId": a.channel})],
            vec![
                json!({"conversationId": b.channel}),
                json!({"workspace": b.organization}),
            ],
            vec![],
        ),
        // C2.15: B's accounts' statuses never show in A.
        case(
            "status_list",
            vec![json!({"accounts": [a.rook]})],
            vec![json!({"workspace": b.organization, "accounts": [b.rook]})],
            vec![json!({"accounts": [b.rook, a.rook]})],
        ),
        case(
            "package_export",
            vec![json!({"spaceId": "WORK"}), json!({"wholeWorkspace": true})],
            vec![json!({"spaceId": b.project}), json!({"spaceId": b.space})],
            vec![],
        ),
        case(
            "module_open",
            vec![json!({"moduleId": a.module.to_string()})],
            vec![json!({"moduleId": b.module.to_string()})],
            vec![],
        ),
        by_project("milestone_list"),
        by_project("cycle_list"),
        by_project("triage_rule_list"),
        by_project("project_status_get"),
        // T1.28: B's forms neither list nor open in A.
        by_project("form_list"),
        case(
            "form_get",
            vec![json!({"form": a.form})],
            vec![json!({"form": b.form})],
            vec![],
        ),
        // T1.30: B's templates and schedules never list in A.
        case(
            "record_template_list",
            vec![json!({"collection": a.collection})],
            vec![json!({"collection": b.collection})],
            vec![],
        ),
        case(
            "recurring_record_list",
            vec![json!({"collection": a.collection})],
            vec![json!({"collection": b.collection})],
            vec![],
        ),
        // T1.26: B's initiatives never list in A.
        case(
            "initiative_list",
            vec![json!({}), json!({"workspace": a.organization})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        by_project("task_ready_list"),
        by_project("dependency_graph_get"),
        case(
            "ai_suggestions_list",
            vec![
                json!({"projectId": "WORK"}),
                json!({"projectId": a.project, "taskId": "WORK-1", "status": "all"}),
            ],
            vec![json!({"projectId": b.project})],
            vec![json!({"projectId": a.project, "taskId": b.task.to_string()})],
        ),
        case(
            "view_list",
            vec![json!({"collection": a.collection})],
            vec![json!({"collection": b.collection})],
            vec![],
        ),
        // T1.32: B's dashboards neither list nor open in A.
        case(
            "dashboard_list",
            vec![json!({"spaceId": a.project})],
            vec![json!({"spaceId": b.project})],
            vec![],
        ),
        case(
            "dashboard_get",
            vec![json!({"dashboard": a.dashboard})],
            vec![json!({"dashboard": b.dashboard})],
            vec![],
        ),
        // T1.34: B's connections and queries neither list nor open in A.
        case(
            "data_connection_list",
            vec![json!({"spaceId": a.project})],
            vec![json!({"spaceId": b.project})],
            vec![],
        ),
        case(
            "data_query_list",
            vec![json!({"spaceId": a.project})],
            vec![json!({"spaceId": b.project})],
            vec![],
        ),
        case(
            "data_query_get",
            vec![json!({"query": a.data_query})],
            vec![json!({"query": b.data_query})],
            vec![],
        ),
        by_project("automation_list"),
        by_project("bug_report_link_list"),
        case(
            "automation_history",
            vec![json!({"projectId": a.project, "ruleId": a.rule.to_string()})],
            vec![
                json!({"projectId": b.project, "ruleId": b.rule.to_string()}),
                json!({"projectId": a.project, "ruleId": b.rule.to_string()}),
            ],
            vec![],
        ),
        // K2.21: a long call's answer is its caller's alone.
        case(
            "job_get",
            vec![json!({"jobId": a.job.to_string()})],
            vec![json!({"jobId": b.job.to_string()})],
            vec![],
        ),
        // K4.26: another workspace's invitation links are out of reach.
        case(
            "invite_link_list",
            vec![
                json!({}),
                json!({"workspace": a.organization, "includeInactive": true}),
            ],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        // A2.18: the agents you could add here, asked of this workspace;
        // another workspace can't be asked.
        case(
            "account_joinable_list",
            vec![json!({}), json!({"workspace": a.organization})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        // A2.19: the offers of the hosts you may use here; another
        // workspace's aren't listed, and another's action reply isn't read.
        case(
            "agent_offer_list",
            vec![json!({}), json!({"workspace": a.organization})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        // A2.17: the same built-in list everywhere, but only asked where
        // the caller is.
        case(
            "agent_recipe_list",
            vec![json!({})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        case(
            "agent_action_get",
            vec![json!({"commandId": a.host_action})],
            vec![
                json!({"commandId": b.host_action}),
                json!({"commandId": b.host_action, "workspace": b.organization}),
            ],
            vec![],
        ),
        // A1.11: another workspace's hosts are out of reach.
        case(
            "host_list",
            vec![
                json!({}),
                json!({"workspace": a.organization}),
                json!({"mineOnly": true}),
            ],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        case(
            "whiteboard_list",
            vec![
                json!({}),
                json!({"query": "Sketch"}),
                json!({"projectId": a.project}),
                json!({"linkedToKind": "task", "linkedToId": a.task}),
            ],
            vec![
                json!({"projectId": b.project}),
                json!({"linkedToKind": "task", "linkedToId": b.task}),
                json!({"linkedToKind": "page", "linkedToId": b.page}),
            ],
            vec![
                json!({"workspaceId": b.organization}),
                json!({"query": MARKER}),
            ],
        ),
        case(
            "whiteboard_read",
            vec![
                json!({"whiteboardId": a.whiteboard}),
                json!({"whiteboardId": a.project_whiteboard, "raw": true}),
            ],
            vec![
                json!({"whiteboardId": b.whiteboard, "raw": true}),
                json!({"whiteboardId": b.project_whiteboard}),
            ],
            vec![],
        ),
        case(
            "whiteboard_render",
            vec![
                json!({"whiteboardId": a.whiteboard, "scale": 0.25}),
                json!({"whiteboardId": a.project_whiteboard, "format": "svg"}),
            ],
            vec![
                json!({"whiteboardId": b.whiteboard, "format": "svg"}),
                json!({"whiteboardId": b.project_whiteboard}),
            ],
            vec![],
        ),
        by_project("template_list"),
        by_project("project_export"),
        case(
            "my_work_get",
            vec![json!({}), json!({"projectId": "WORK"})],
            vec![json!({"projectId": b.project})],
            vec![],
        ),
        by_project("settings_get"),
        by_project("module_list"),
        by_project("package_list"),
        by_project("agent_template_list"),
        // A's directory lists its own URL list and never B's.
        case(
            "directory_list",
            vec![json!({})],
            vec![],
            vec![json!({}), json!({"query": "lists"})],
        ),
        case(
            "package_preview",
            vec![json!({"package": {
                "format": "agent-workspace-package/1", "id": "own_tiny", "version": "1.0.0",
                "name": "Tiny", "target": "space", "contents": []},
                "target": {"spaceId": a.project}})],
            vec![json!({"package": {
                "format": "agent-workspace-package/1", "id": "net_tiny", "version": "1.0.0",
                "name": "Tiny", "target": "space", "contents": []},
                "target": {"spaceId": b.project}})],
            vec![],
        ),
        case(
            "thing_open",
            vec![
                json!({"kind": "task", "id": a.task}),
                json!({"kind": "page", "id": a.page}),
                json!({"kind": "thread", "id": a.thread}),
            ],
            vec![
                json!({"kind": "task", "id": b.task}),
                json!({"kind": "conversation", "id": b.dm}),
                json!({"kind": "whiteboard", "id": b.whiteboard}),
            ],
            vec![],
        ),
        case(
            "usage_windows",
            vec![json!({})],
            vec![json!({"accountId": b.provider_account})],
            vec![],
        ),
        case(
            "usage_batch",
            vec![json!({"accountId": a.provider_account})],
            vec![json!({"accountId": b.provider_account})],
            vec![],
        ),
        case(
            "conversation_list",
            vec![
                json!({}),
                json!({"projectId": "WORK"}),
                json!({"query": "launch"}),
                json!({"withPerson": "@rook"}),
            ],
            vec![
                json!({"projectId": b.project}),
                json!({"withPerson": b.rook.to_string()}),
            ],
            vec![json!({"query": MARKER})],
        ),
        case(
            "conversation_open",
            conversations(a)
                .into_iter()
                .map(|id| json!({"conversationId": id}))
                .collect(),
            conversations(b)
                .into_iter()
                .map(|id| json!({"conversationId": id}))
                .collect(),
            vec![],
        ),
        case(
            "conversation_read",
            conversations(a)
                .into_iter()
                .map(|id| json!({"conversationId": id}))
                .collect(),
            conversations(b)
                .into_iter()
                .map(|id| json!({"conversationId": id}))
                .chain([json!({"conversationId": a.channel, "aroundMessageId": b.message})])
                .collect(),
            vec![],
        ),
        case(
            "message_history",
            vec![
                json!({"messageId": a.message}),
                json!({"messageId": a.dm_message}),
            ],
            vec![
                json!({"messageId": b.message}),
                json!({"messageId": b.dm_message}),
            ],
            vec![],
        ),
        // K1.23: an account's secrets. B's agent and B's owner aren't A's
        // agent's to list.
        case(
            "credential_list",
            vec![json!({})],
            vec![json!({"account": b.agent}), json!({"account": b.rook})],
            vec![],
        ),
        case(
            "channel_list",
            vec![json!({"spaceId": a.project})],
            vec![json!({"spaceId": b.project})],
            vec![],
        ),
        case(
            "channel_get",
            vec![json!({"channelId": a.channel})],
            vec![json!({"channelId": b.channel})],
            vec![],
        ),
        case(
            "conversation_member_list",
            vec![json!({"conversationId": a.dm})],
            vec![
                json!({"conversationId": b.dm}),
                json!({"conversationId": b.channel}),
            ],
            vec![],
        ),
        case(
            "conversation_tab_list",
            vec![json!({"conversationId": a.channel})],
            vec![json!({"conversationId": b.channel})],
            vec![],
        ),
        case(
            "conversation_file_list",
            vec![json!({"conversationId": a.dm})],
            vec![
                json!({"conversationId": b.dm}),
                json!({"conversationId": b.channel}),
            ],
            vec![],
        ),
        case(
            "conversation_task_list",
            vec![json!({"conversationId": a.channel})],
            vec![
                json!({"conversationId": b.channel}),
                json!({"conversationId": b.dm}),
            ],
            vec![],
        ),
        case(
            "agent_list",
            vec![
                json!({}),
                json!({"projectId": "WORK"}),
                json!({"query": "mara"}),
            ],
            vec![
                json!({"projectId": b.project}),
                json!({"projectId": b.space}),
            ],
            vec![json!({"query": MARKER})],
        ),
        case(
            "people_search",
            vec![
                json!({}),
                json!({"query": "rook"}),
                json!({"projectId": "WORK"}),
            ],
            vec![json!({"projectId": b.project})],
            vec![json!({"query": MARKER})],
        ),
        case(
            "inbox_list",
            vec![json!({})],
            vec![json!({"projectId": b.project})],
            vec![],
        ),
        case(
            "later_list",
            vec![
                json!({}),
                json!({"view": "done"}),
                json!({"view": "scheduled"}),
            ],
            vec![],
            vec![],
        ),
        // A1.8: a request (the run's approval) is read where its run is.
        case(
            "request_get",
            vec![json!({"requestId": a.approval.to_string()})],
            vec![json!({"requestId": b.approval.to_string()})],
            vec![],
        ),
        case(
            "request_list",
            vec![
                json!({}),
                json!({"role": "raised"}),
                json!({"role": "raised", "status": "answered"}),
            ],
            vec![],
            vec![],
        ),
        case(
            "notification_list",
            vec![
                json!({}),
                json!({"view": "archived"}),
                json!({"projectId": a.project}),
            ],
            vec![],
            vec![json!({"projectId": b.project})],
        ),
        // C2.13: keywords are kept per workspace; B's never list in A.
        case(
            "notification_keyword_list",
            vec![json!({})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        // C2.17: B's people's profiles never read from A, and A's
        // avatars list only A's people.
        case(
            "profile_get",
            vec![json!({}), json!({"account": a.rook})],
            vec![
                json!({"account": b.rook}),
                json!({"workspace": b.organization}),
            ],
            vec![],
        ),
        case(
            "profile_avatar_list",
            vec![json!({})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        // C2.18: a workspace's groups list only in it.
        case(
            "group_list",
            vec![json!({}), json!({"members": true})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        // C2.26: saved searches, and places to jump to, stay in their workspace.
        case(
            "saved_search_list",
            vec![json!({})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        // C2.11: saved inbox views stay in their workspace.
        case(
            "inbox_view_list",
            vec![json!({})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        case(
            "place_search",
            vec![json!({"q": "a"})],
            vec![json!({"workspace": b.organization, "q": "a"})],
            vec![],
        ),
        // C2.39: B's DMs have no standing to read from A, and blocks list
        // only in the workspace called.
        case(
            "dm_standing_get",
            vec![json!({"conversationId": a.dm})],
            vec![json!({"conversationId": b.dm})],
            vec![],
        ),
        case(
            "account_block_list",
            vec![json!({})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        // C2.9: sidebar sections are kept per workspace; B's never list in A.
        case(
            "sidebar_list",
            vec![json!({})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        // C2.36: threads followed in B never list in A.
        case(
            "thread_followed_list",
            vec![json!({}), json!({"openOnly": true})],
            vec![json!({"workspace": b.organization})],
            vec![],
        ),
        case(
            "task_list",
            vec![json!({}), json!({"projectId": "WORK"})],
            vec![
                json!({"projectId": b.project}),
                json!({"projectIds": [b.project]}),
            ],
            vec![
                json!({"projectId": "WORK", "assignee": b.rook.to_string()}),
                json!({"projectId": "WORK", "view": "My tasks"}),
            ],
        ),
        case(
            "task_get",
            vec![
                json!({"taskId": "WORK-1"}),
                json!({"taskId": a.task.to_string()}),
                json!({"taskId": a.subtask.to_string()}),
            ],
            vec![
                json!({"taskId": b.task.to_string()}),
                json!({"taskId": b.subtask.to_string()}),
            ],
            vec![],
        ),
        case(
            "workspace_search",
            vec![
                json!({"query": "launch"}),
                json!({"query": "launch", "projectId": "WORK"}),
            ],
            vec![
                json!({"query": "launch", "projectId": b.project}),
                json!({"query": "launch", "conversationId": b.channel}),
                json!({"query": "launch", "conversationId": b.dm}),
            ],
            vec![
                json!({"query": MARKER}),
                json!({"query": "launch", "from": b.rook.to_string()}),
            ],
        ),
        case(
            "page_get",
            vec![
                json!({"pageId": a.page}),
                json!({"pageId": a.page, "version": 1}),
            ],
            vec![
                json!({"pageId": b.page}),
                json!({"pageId": b.page, "version": 1}),
            ],
            vec![],
        ),
        case(
            "page_blocks",
            vec![
                json!({"pageId": a.page}),
                json!({"pageId": a.page, "includeMarkdown": true}),
            ],
            vec![
                json!({"pageId": b.page}),
                json!({"pageId": b.page, "includeMarkdown": true}),
            ],
            vec![],
        ),
        case(
            "page_comments",
            vec![json!({"pageId": a.page, "includeResolved": true})],
            vec![json!({"pageId": b.page, "includeResolved": true})],
            vec![],
        ),
        case(
            "page_suggestions",
            vec![json!({"pageId": a.page, "includeDecided": true})],
            vec![json!({"pageId": b.page, "includeDecided": true})],
            vec![],
        ),
        case(
            "change_list",
            vec![json!({"thingKind": "page", "thingId": a.page})],
            vec![json!({"thingKind": "page", "thingId": b.page})],
            vec![],
        ),
        case(
            "proposal_list",
            vec![
                json!({"thingKind": "page", "thingId": a.page, "includeDecided": true}),
                json!({}),
            ],
            vec![json!({"thingKind": "page", "thingId": b.page, "includeDecided": true})],
            vec![],
        ),
        case(
            "skill_get",
            vec![
                json!({"pageId": a.page}),
                json!({"projectId": a.project, "name": "launch-page"}),
            ],
            vec![
                json!({"pageId": b.page}),
                json!({"projectId": b.project, "name": format!("launch-page-{}", b.marker)}),
            ],
            vec![],
        ),
        case(
            "run_get",
            vec![json!({"runId": a.run.to_string()})],
            vec![json!({"runId": b.run.to_string()})],
            vec![],
        ),
        case(
            "run_list",
            vec![json!({}), json!({"projectId": "WORK"})],
            vec![json!({"projectId": b.project})],
            vec![],
        ),
        case(
            "run_steps",
            vec![
                json!({"runId": a.run.to_string()}),
                json!({"runId": a.run.to_string(), "includeDiff": true}),
            ],
            vec![json!({"runId": b.run.to_string()})],
            vec![],
        ),
        // A1.5: the run view and its activities, for any run.
        case(
            "run_detail_get",
            vec![json!({"runId": a.run.to_string()})],
            vec![json!({"runId": b.run.to_string()})],
            vec![],
        ),
        case(
            "run_activity_list",
            vec![json!({"runId": a.run.to_string(), "afterSeq": 0})],
            vec![json!({"runId": b.run.to_string()})],
            vec![],
        ),
        case(
            "run_usage",
            vec![
                json!({}),
                json!({"projectId": "WORK", "groupBy": "model"}),
                json!({"groupBy": "account"}),
                json!({"groupBy": "project"}),
            ],
            vec![json!({"projectId": b.project})],
            vec![],
        ),
        // A1.15: usage by agent, only from runs in spaces the caller opens.
        case(
            "agent_usage_list",
            vec![json!({})],
            vec![],
            vec![json!({"days": 400})],
        ),
        case(
            "repository_list",
            vec![json!({}), json!({"projectId": "WORK"})],
            vec![json!({"projectId": b.project})],
            vec![],
        ),
        case(
            "repository_get",
            vec![
                json!({"repositoryId": a.repository.to_string()}),
                json!({"repositoryId": "acme/widgets"}),
            ],
            vec![
                json!({"repositoryId": b.repository.to_string()}),
                json!({"repositoryId": b.repository.to_string(), "projectId": a.project}),
                json!({"repositoryId": "acme/widgets", "projectId": b.project}),
            ],
            vec![],
        ),
        case(
            "pull_request_list",
            vec![
                json!({}),
                json!({"projectId": "WORK"}),
                json!({"repositoryId": a.repository.to_string()}),
            ],
            vec![
                json!({"projectId": b.project}),
                json!({"repositoryId": b.repository.to_string()}),
                json!({"taskId": b.task.to_string()}),
            ],
            vec![],
        ),
        case(
            "pull_request_get",
            vec![
                json!({"identifier": a.pull_request.to_string()}),
                json!({"identifier": a.pull_request_url}),
            ],
            vec![
                json!({"identifier": b.pull_request.to_string()}),
                json!({"identifier": b.pull_request.to_string(), "projectId": a.project}),
            ],
            vec![],
        ),
        case("external_event_poll", vec![json!({})], vec![], vec![]),
        // A2: the caller's own events and connections; B's agent isn't
        // one A's caller manages.
        case(
            "event_list",
            vec![json!({}), json!({"limit": 10})],
            vec![],
            vec![],
        ),
        case("event_kind_list", vec![json!({})], vec![], vec![]),
        case(
            "connection_list",
            vec![json!({})],
            vec![json!({"account": b.agent})],
            vec![],
        ),
        // A2.6: a webhook's delivery log is its account's; B's isn't A's.
        case(
            "connection_delivery_list",
            vec![json!({"connectionId": a.webhook})],
            vec![json!({"connectionId": b.webhook})],
            vec![],
        ),
        case(
            "schedule_list",
            vec![json!({}), json!({"includeFinished": true})],
            vec![],
            vec![],
        ),
        // A2.9: what the caller follows, in A; B's space isn't found.
        case(
            "follow_list",
            vec![json!({}), json!({"space": a.project})],
            vec![json!({"space": b.project})],
            vec![],
        ),
        // A2.27: the guards of the workspace called.
        case("guard_list", vec![json!({})], vec![], vec![]),
        // A2.16: memory of A's channel or space; B's aren't found.
        case(
            "memory_page_open",
            vec![
                json!({"conversation": a.channel}),
                json!({"space": a.project}),
            ],
            vec![
                json!({"conversation": b.channel}),
                json!({"space": b.project}),
            ],
            vec![],
        ),
        // A2.9: ambient mode in A's channel; B's channel isn't found.
        case(
            "conversation_agent_settings_list",
            vec![json!({"conversation": a.channel})],
            vec![json!({"conversation": b.channel})],
            vec![],
        ),
        case(
            "file_list",
            vec![
                json!({"messageId": a.file_message}),
                json!({"taskId": a.task.to_string()}),
            ],
            vec![
                json!({"messageId": b.file_message}),
                json!({"taskId": b.task.to_string()}),
            ],
            vec![],
        ),
        case(
            "file_read",
            vec![
                json!({"fileId": a.file.to_string()}),
                json!({"fileId": a.task_file.to_string()}),
            ],
            vec![
                json!({"fileId": b.file.to_string()}),
                json!({"fileId": b.task_file.to_string()}),
            ],
            vec![],
        ),
        // A2.12: a link to a file only for a file the caller can read.
        case(
            "file_link_get",
            vec![json!({"fileId": a.file.to_string()})],
            vec![json!({"fileId": b.file.to_string()})],
            vec![],
        ),
        // K3.19: any thing's history by id alone.
        case(
            "history_list",
            vec![json!({"thing": a.page})],
            vec![json!({"thing": b.page})],
            vec![],
        ),
        case(
            "history_show",
            vec![json!({"thing": a.page, "version": 1})],
            vec![json!({"thing": b.page, "version": 1})],
            vec![],
        ),
        // K3.20: any thing's activity from the log.
        case(
            "activity_list",
            vec![json!({"thing": a.page}), json!({"thing": a.task})],
            vec![json!({"thing": b.page}), json!({"thing": b.task})],
            vec![],
        ),
        case(
            "sheet_search",
            vec![json!({"sheetId": a.sheet, "query": "Rent"})],
            vec![json!({"sheetId": b.sheet, "query": "Rent"})],
            vec![],
        ),
        case(
            "sheet_view_list",
            vec![json!({"sheetId": a.sheet})],
            vec![json!({"sheetId": b.sheet})],
            vec![],
        ),
    ]
}

/// Every case runs as A's connected app (acting as A's owner) or, with
/// `as_agent`, as A's agent. Own calls must succeed for whoever the tool is
/// for; for the other, only leaks count.
async fn check_mcp(
    app: &axum::Router,
    pool: &PgPool,
    a: &World,
    b: &World,
    b_ids: &HashSet<String>,
    as_agent: bool,
    report: &mut Report,
) {
    let cases = mcp_cases(a, b);
    let cased = cases.iter().map(|case| case.tool).collect::<BTreeSet<_>>();
    let listed = MCP_CASE_TOOLS.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(cased, listed, "MCP_CASE_TOOLS and mcp_cases must agree");

    let app_session = initialize(app, &a.app_token).await.expect("A app MCP");
    let agent_session = initialize(app, &a.agent_token).await.expect("A agent MCP");

    // The live tool list agrees with the cases: every read-only tool is
    // covered, as the static guard also checks.
    let tools = list_tools(app, &a.app_token, &app_session).await;
    let read_only = tools
        .iter()
        .filter(|tool| tool.pointer("/annotations/readOnlyHint") == Some(&Value::Bool(true)))
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect::<BTreeSet<_>>();
    let excluded = MCP_EXCLUSIONS
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    let uncovered = read_only
        .iter()
        .filter(|name| !listed.contains(name.as_str()) && !excluded.contains(*name))
        .collect::<Vec<_>>();
    if !uncovered.is_empty() {
        report.failures.push(format!(
            "COVERAGE read-only MCP tools without an isolation case: {uncovered:?}"
        ));
    }

    // P0.23: a read-only tool changes nothing. A's rows are digested before
    // and after each tool's calls; only the columns reads may touch differ.
    let a_ids = sweep_ids(pool, a.organization).await;
    let a_list = a_ids.iter().cloned().collect::<Vec<_>>();
    // Fingerprints first, so a write landing during the digest is caught
    // by the next refresh rather than missed.
    // The schema's tables, read once: they don't change during the sweep.
    let tables = base_tables(pool).await;
    let mut fingerprints = table_fingerprints(pool, &tables).await;
    let mut a_rows = digest_ignoring(pool, &a_list, READ_TOUCHES).await;
    for (who, token, session, is_agent) in [
        ("A's app", &a.app_token, &app_session, false),
        ("A's agent", &a.agent_token, &agent_session, true),
    ]
    .into_iter()
    .filter(|(_, _, _, is_agent)| *is_agent == as_agent)
    {
        for case in &cases {
            let read_only_check = !READ_TOOLS_THAT_WRITE
                .iter()
                .any(|(tool, _)| *tool == case.tool);
            let before = a_rows.clone();
            for arguments in &case.own {
                report.mcp_calls += 1;
                let (status, body) =
                    call_tool(app, token, session, case.tool, arguments.clone()).await;
                let outcome = structured(&body)["outcome"].clone();
                if case.agent_only == is_agent && !succeeded(&outcome) {
                    report.failures.push(format!(
                        "OWN {who} {} {arguments} -> {status} outcome {outcome} (A's own call must succeed): {}",
                        case.tool,
                        body.chars().take(300).collect::<String>()
                    ));
                }
                report.leak(
                    &format!("{who} {} {arguments}", case.tool),
                    &arguments.to_string(),
                    &body,
                    b_ids,
                );
            }
            for arguments in &case.foreign {
                report.mcp_calls += 1;
                let (status, body) =
                    call_tool(app, token, session, case.tool, arguments.clone()).await;
                let leaked = report.leak(
                    &format!("{who} {} {arguments} (B's IDs)", case.tool),
                    &arguments.to_string(),
                    &body,
                    b_ids,
                );
                let outcome = structured(&body)["outcome"].clone();
                if succeeded(&outcome) && !leaked {
                    report.notes.push(format!(
                        "{who} {} {arguments} with B's IDs -> {status} outcome ok (no B data, but a refusal was expected)",
                        case.tool
                    ));
                }
            }
            for arguments in &case.filtered {
                report.mcp_calls += 1;
                let (_, body) = call_tool(app, token, session, case.tool, arguments.clone()).await;
                report.leak(
                    &format!("{who} {} {arguments}", case.tool),
                    &arguments.to_string(),
                    &body,
                    b_ids,
                );
            }
            fingerprints = refresh_digest(
                pool,
                &a_list,
                READ_TOUCHES,
                &tables,
                &fingerprints,
                &mut a_rows,
            )
            .await;
            let changes = digest_changes(&before, &a_rows);
            if read_only_check && !changes.is_empty() {
                report.failures.push(format!(
                    "READ-ONLY {who} {} changed A's rows:\n    {}",
                    case.tool,
                    changes.join("\n    ")
                ));
            }
        }
    }
}

/// B's owner joins A, makes an account-wide secret there and exchanges it
/// for a token, which is held to A: every case's calls with B's IDs, and its
/// filtered listings, carry nothing of B's though the account can see it all.
async fn check_token(
    app: &axum::Router,
    pool: &PgPool,
    a: &World,
    b: &World,
    b_ids: &HashSet<String>,
    report: &mut Report,
) {
    let b_owner = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT actor_id FROM organization_memberships \
         WHERE organization_id = $1 AND role = 'owner' LIMIT 1",
    )
    .bind(b.organization)
    .fetch_one(pool)
    .await
    .expect("B's owner");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) \
         VALUES ($1, $2, 'member')",
    )
    .bind(a.organization)
    .bind(b_owner)
    .execute(pool)
    .await
    .expect("B's owner joins A");
    // Known in A by A's own profile, not the one B gave it.
    sqlx::query(
        "INSERT INTO organization_actor_profiles (organization_id, actor_id, display_name, handle) \
         VALUES ($1, $2, 'Visitor', 'visitor') \
         ON CONFLICT (organization_id, actor_id) \
         DO UPDATE SET display_name = EXCLUDED.display_name, handle = EXCLUDED.handle",
    )
    .bind(a.organization)
    .bind(b_owner)
    .execute(pool)
    .await
    .expect("B's owner's profile in A");
    {
        let space = &a.project;
        let (status, added) = send_json(
            app,
            a.browser.request(
                "POST",
                &format!("/api/v1/projects/{space}/members"),
                None,
                json!({"actorId": b_owner, "role": "member"}),
            ),
        )
        .await;
        assert!(status.is_success(), "{status} {added}");
    }
    // B's DMs and groups are kept in B (ruling R20): B's, not the account's
    // own personal ones, which a token for any workspace reaches.
    sqlx::query(
        "UPDATE conversations SET kept = true, workspace_id = $1 \
         WHERE project_id IS NULL AND id IN \
             (SELECT conversation_id FROM conversation_members WHERE actor_id = $2)",
    )
    .bind(b.organization)
    .bind(b_owner)
    .execute(pool)
    .await
    .expect("B keeps its DMs");
    let (status, made) = send_json(
        app,
        b.browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/credential_create", a.organization),
            Some("b-owner-in-a"),
            json!({"name": "across", "space": a.project, "reach": "account"}),
        ),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{status} {made}");
    let secret = made["value"]["secret"].as_str().expect("secret");
    let (status, exchanged) = send_json(
        app,
        Request::builder()
            .method("POST")
            .uri("/oauth/token")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(format!(
                "grant_type=client_credentials&client_secret={}",
                url_encode(secret)
            )))
            .expect("token request"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{exchanged}");
    let token = exchanged["access_token"]
        .as_str()
        .expect("token")
        .to_owned();
    let session = initialize(app, &token).await.expect("token MCP");
    // The account itself is B's and A's: its own ID isn't B's data.
    let mut b_ids = b_ids.clone();
    b_ids.remove(&b_owner.to_string());
    b_ids.remove(&b_owner.simple().to_string());
    for case in mcp_cases(a, b) {
        for arguments in case.foreign.iter().chain(&case.filtered) {
            report.mcp_calls += 1;
            let (_, body) = call_tool(app, &token, &session, case.tool, arguments.clone()).await;
            report.leak(
                &format!("A's token (B's owner) {} {arguments}", case.tool),
                &arguments.to_string(),
                &body,
                &b_ids,
            );
        }
    }
    // Writes too: the account's own saved items and scheduled messages in B
    // stay out of reach of A's token, and are left as they were.
    let saved = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM saved_items WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(b.organization)
    .bind(b_owner)
    .fetch_all(pool)
    .await
    .expect("B's saved items");
    assert!(!saved.is_empty(), "B's owner saved something in B");
    let scheduled = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT scheduled.id FROM scheduled_messages scheduled \
         JOIN projects space ON space.id = scheduled.project_id \
         WHERE space.organization_id = $1 AND scheduled.author_actor_id = $2",
    )
    .bind(b.organization)
    .bind(b_owner)
    .fetch_all(pool)
    .await
    .expect("B's scheduled messages");
    assert!(!scheduled.is_empty(), "B's owner scheduled something in B");
    let writes = saved
        .iter()
        .flat_map(|id| {
            [
                json!({"id": id, "done": true}),
                json!({"id": id, "text": "Moved by A"}),
                json!({"id": id, "remove": true}),
            ]
            .map(|arguments| ("later_update", arguments))
        })
        .chain(scheduled.iter().flat_map(|id| {
            [
                json!({"id": id, "text": "Moved by A"}),
                json!({"id": id, "cancel": true}),
            ]
            .map(|arguments| ("scheduled_message_update", arguments))
        }));
    for (tool, arguments) in writes {
        report.mcp_calls += 1;
        let (_, body) = call_tool(app, &token, &session, tool, arguments.clone()).await;
        if succeeded(&structured(&body)["outcome"]) {
            report.failures.push(format!(
                "WRITE A's token (B's owner) changed B with {tool} {arguments}: {body}"
            ));
        }
        report.leak(
            &format!("A's token (B's owner) {tool} {arguments}"),
            &arguments.to_string(),
            &body,
            &b_ids,
        );
    }
    let touched = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM saved_items \
         WHERE id = ANY($1) AND (done_at IS NOT NULL OR note = 'Moved by A')",
    )
    .bind(&saved)
    .fetch_one(pool)
    .await
    .expect("saved items after");
    let kept = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM saved_items WHERE id = ANY($1)")
        .bind(&saved)
        .fetch_one(pool)
        .await
        .expect("saved items kept");
    assert_eq!(
        (touched, kept),
        (0, saved.len() as i64),
        "A's token changed B's saved items"
    );

    // A message saved and one scheduled in a DM: while the DM is kept in B,
    // A's token doesn't see them in Later; once it's the account's own
    // personal DM, it does, and can change them.
    let dm_message = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM messages WHERE conversation_id = $1::uuid ORDER BY created_at LIMIT 1",
    )
    .bind(&b.dm)
    .fetch_one(pool)
    .await
    .expect("a message in B's DM");
    let (status, saved_dm) = send_json(
        app,
        b.browser.request(
            "POST",
            "/api/v1/later",
            None,
            json!({"messageId": dm_message, "note": "From the DM"}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status} {saved_dm}");
    let saved_dm = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM saved_items WHERE actor_id = $1 AND message_id = $2",
    )
    .bind(b_owner)
    .bind(dm_message)
    .fetch_one(pool)
    .await
    .expect("the saved DM message");
    let soon = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
    let (status, scheduled_dm) = send_json(
        app,
        b.browser.request(
            "POST",
            &format!("/api/v1/conversations/{}/scheduled-messages", b.dm),
            None,
            json!({
                "content": {"version": 1, "nodes": [{"type": "text", "text": "Later in the DM"}]},
                "sendAt": soon,
            }),
        ),
    )
    .await;
    assert!(status.is_success(), "{status} {scheduled_dm}");
    let scheduled_dm = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM scheduled_messages WHERE author_actor_id = $1 AND conversation_id = $2::uuid",
    )
    .bind(b_owner)
    .bind(&b.dm)
    .fetch_one(pool)
    .await
    .expect("the scheduled DM message");
    async fn later(app: &axum::Router, token: &str, session: &str, view: &str) -> String {
        call_tool(app, token, session, "later_list", json!({"view": view}))
            .await
            .1
    }
    let (open, waiting) = (
        later(app, &token, &session, "open").await,
        later(app, &token, &session, "scheduled").await,
    );
    if open.contains(&saved_dm.to_string()) || waiting.contains(&scheduled_dm.to_string()) {
        report.failures.push(format!(
            "A's token saw Later items from a DM kept in B: {open} {waiting}"
        ));
    }
    sqlx::query(
        "UPDATE conversations SET kept = false, workspace_id = direct_scope_id() WHERE id = $1::uuid",
    )
    .bind(&b.dm)
    .execute(pool)
    .await
    .expect("the DM becomes personal");
    let (open, waiting) = (
        later(app, &token, &session, "open").await,
        later(app, &token, &session, "scheduled").await,
    );
    assert!(
        open.contains(&saved_dm.to_string()),
        "A's token lists a message saved from a personal DM: {open}"
    );
    assert!(
        waiting.contains(&scheduled_dm.to_string()),
        "A's token lists a message scheduled in a personal DM: {waiting}"
    );
    let (_, done) = call_tool(
        app,
        &token,
        &session,
        "later_update",
        json!({"id": saved_dm, "done": true}),
    )
    .await;
    assert!(
        succeeded(&structured(&done)["outcome"]),
        "A's token marks a personal DM's saved message done: {done}"
    );
}

async fn send_json(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4_000_000)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

pub(super) async fn mcp_post(
    app: &axum::Router,
    token: &str,
    session: &str,
    payload: Value,
) -> (StatusCode, String) {
    // The sweeps make more calls with one credential than its per-minute
    // budget holds; a refused call waits as long as it's told and goes
    // again, as a real client would.
    let mut attempts = 0;
    let response = loop {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(MCP_PATH)
                    .header(header::HOST, "127.0.0.1")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::ACCEPT, "application/json, text/event-stream")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header("mcp-session-id", session)
                    .header("mcp-protocol-version", "2025-06-18")
                    .body(Body::from(serde_json::to_vec(&payload).expect("JSON")))
                    .expect("MCP request"),
            )
            .await
            .expect("MCP response");
        attempts += 1;
        if response.status() != StatusCode::TOO_MANY_REQUESTS || attempts > 3 {
            break response;
        }
        let wait = response
            .headers()
            .get(header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(5)
            .clamp(1, 65);
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
    };
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 8_000_000)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Call a tool without asserting anything; returns the raw response.
pub(super) async fn call_tool(
    app: &axum::Router,
    token: &str,
    session: &str,
    name: &str,
    arguments: Value,
) -> (StatusCode, String) {
    mcp_post(
        app,
        token,
        session,
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }),
    )
    .await
}

pub(super) fn succeeded(outcome: &Value) -> bool {
    matches!(
        outcome.as_str(),
        Some("ok" | "found" | "applied" | "events")
    )
}

pub(super) fn json_body(text: &str) -> Value {
    let json = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| !line.trim().is_empty())
        .unwrap_or(text);
    serde_json::from_str(json).unwrap_or(Value::Null)
}

/// A tool result's structured content, or Null.
pub(super) fn structured(text: &str) -> Value {
    json_body(text)
        .pointer("/result/structuredContent")
        .cloned()
        .unwrap_or(Value::Null)
}

pub(super) async fn list_tools(app: &axum::Router, token: &str, session: &str) -> Vec<Value> {
    let (status, body) = mcp_post(
        app,
        token,
        session,
        json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "tools/list: {body}");
    json_body(&body)
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("tools/list: {body}"))
}

// ---------------------------------------------------------------------------
// Realtime
// ---------------------------------------------------------------------------

/// A subscribes over the real WebSocket, then the log's tail starts: the whole fixture's backlog (both workspaces) is
/// delivered from the start of the log, B changes a
/// few more things, and A posts a sentinel message. A must see its sentinel
/// and nothing of B's.
async fn check_realtime(
    app: &axum::Router,
    pool: &PgPool,
    realtime: &RealtimeHub,
    a: &World,
    b: &World,
    b_ids: &HashSet<String>,
    report: &mut Report,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback listener");
    let address = listener.local_addr().expect("listener address");
    let (stop_server, server_stopped) = tokio::sync::oneshot::channel::<()>();
    let served = app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, served)
            .with_graceful_shutdown(async {
                let _ = server_stopped.await;
            })
            .await
            .expect("serve");
    });

    let mut request = format!("ws://{address}/api/v1/realtime")
        .into_client_request()
        .expect("WebSocket request");
    request
        .headers_mut()
        .insert("origin", HeaderValue::from_static(ORIGIN));
    request.headers_mut().insert(
        "cookie",
        HeaderValue::from_str(&a.browser.cookie).expect("cookie header"),
    );
    let (mut socket, _) = connect_async(request).await.expect("A's WebSocket");

    let (stop_log, log) = crate::support::live::follow_log(pool, realtime, Some(0)).await;

    // More changes in B while A listens.
    let b_setup = Setup {
        app: app.clone(),
        browser: Browser {
            cookie: b.browser.cookie.clone(),
            csrf: b.browser.csrf.clone(),
        },
        organization_id: b.organization,
        pool: pool.clone(),
        cleanup: None,
    };
    say(
        &b_setup,
        &b.browser,
        &b.channel,
        "live-1",
        text(&format!("Live launch news {}", b.marker)),
    )
    .await;
    say(
        &b_setup,
        &b.browser,
        &b.dm,
        "live-2",
        text(&format!("Live DM {}", b.marker)),
    )
    .await;
    let b_session = initialize(app, &b.app_token).await.expect("B app MCP");
    let (_, created) = call_tool(
        app,
        &b.app_token,
        &b_session,
        "task_create",
        json!({"projectId": "WORK", "title": format!("Live task {}", b.marker), "clientRequestId": "live-task"}),
    )
    .await;
    assert!(succeeded(&structured(&created)["outcome"]), "{created}");
    let (_, drawn) = call_tool(
        app,
        &b.app_token,
        &b_session,
        "whiteboard_edit",
        json!({"whiteboardId": b.whiteboard, "operations": [
            {"op": "add", "type": "text", "id": "live", "text": format!("Live note {}", b.marker)},
        ]}),
    )
    .await;
    assert!(succeeded(&structured(&drawn)["outcome"]), "{drawn}");

    // Then A's sentinel.
    let a_setup = Setup {
        app: app.clone(),
        browser: Browser {
            cookie: a.browser.cookie.clone(),
            csrf: a.browser.csrf.clone(),
        },
        organization_id: a.organization,
        pool: pool.clone(),
        cleanup: None,
    };
    let sentinel = say(
        &a_setup,
        &a.browser,
        &a.channel,
        "sentinel",
        text("Sentinel"),
    )
    .await;

    let mut frames = Vec::new();
    let mut saw_sentinel = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        match timeout(Duration::from_secs(2), socket.next()).await {
            Ok(Some(Ok(ClientMessage::Text(frame)))) => {
                let frame = frame.as_str().to_owned();
                if frame.contains(&sentinel) {
                    saw_sentinel = true;
                }
                frames.push(frame);
                if saw_sentinel {
                    // Give anything queued behind the sentinel a moment.
                    while let Ok(Some(Ok(ClientMessage::Text(more)))) =
                        timeout(Duration::from_millis(500), socket.next()).await
                    {
                        frames.push(more.as_str().to_owned());
                    }
                    break;
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(error))) => {
                report
                    .failures
                    .push(format!("REALTIME A's socket failed: {error}"));
                break;
            }
            Ok(None) => {
                report
                    .failures
                    .push("REALTIME A's socket closed before the sentinel (lagged?)".to_owned());
                break;
            }
            Err(_) => {}
        }
    }
    if !saw_sentinel {
        report.failures.push(format!(
            "REALTIME A never received its own sentinel message, so the realtime check proves nothing ({} frames)",
            frames.len()
        ));
    }
    eprintln!("A received {} realtime frames", frames.len());
    let own_project = frames.iter().any(|frame| frame.contains(&a.project));
    if !own_project {
        report
            .notes
            .push("no realtime frame named A's WORK project".to_owned());
    }
    for frame in &frames {
        report.leak("realtime frame to A", "", frame, b_ids);
    }

    let _ = stop_log.send(true);
    let _ = log.await;
    let _ = socket.close(None).await;
    let _ = stop_server.send(());
    let _ = server.await;
}
