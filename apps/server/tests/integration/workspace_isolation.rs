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
    collections::{BTreeMap, BTreeSet, HashSet},
    os::unix::fs::PermissionsExt,
    sync::Arc,
    time::Duration,
};

use agent_workspace_server::{
    AppState,
    auth::password::hash_password,
    bootstrap::{BootstrapCommand, bootstrap_admin, bootstrap_development_member},
    config::AppConfig,
    mcp::MCP_PATH,
    provider::SupervisorRegistry,
    rate_limit::RateLimiters,
    realtime::{RealtimeHub, outbox_worker_id, run_outbox_publisher},
    router,
    secrets::FileKeyring,
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use tokio::{net::TcpListener, sync::watch, time::timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message as ClientMessage, client::IntoClientRequest, http::HeaderValue},
};
use tower::ServiceExt;
use uuid::Uuid;

use super::{
    agent_interface::{
        Browser, PASSWORD, connect_app, create_agent, initialize, login, schema_pool, send,
    },
    mcp_admin::{Setup, create_project},
    mcp_task_board::{insert_repository, seed_pull_request},
    mcp_tasks::{add_member, space},
    message_edits::{create_channel, edit, say, text},
};

const MARKER: &str = "zebra";
const ORIGIN: &str = "http://127.0.0.1:3100";

/// Every GET route under `/api/v1` this suite calls, as written in the
/// server's `.route(...)` calls.
const HTTP_ROUTES: &[&str] = &[
    "/api/v1/account/connected-apps",
    "/api/v1/account/profile",
    "/api/v1/auth/session",
    "/api/v1/auth/workos",
    "/api/v1/conversations/{id}/messages/{message_id}",
    "/api/v1/conversations/{id}/messages/{message_id}/revisions",
    "/api/v1/conversations/{id}/pins",
    "/api/v1/conversations/{id}/threads",
    "/api/v1/conversations/{id}/timeline",
    "/api/v1/dev/bootstrap",
    "/api/v1/direct",
    "/api/v1/inbox",
    "/api/v1/inbox/summary",
    "/api/v1/organizations/{organization_id}/personal-secrets",
    "/api/v1/organizations/{organization_id}/secrets",
    "/api/v1/pages/{id}",
    "/api/v1/pages/{id}/revisions/{version}",
    "/api/v1/projects",
    "/api/v1/projects/{id}",
    "/api/v1/projects/{id}/channels",
    "/api/v1/projects/{id}/conversation-sidebar",
    "/api/v1/projects/{id}/decisions",
    "/api/v1/projects/{id}/execution-profiles",
    "/api/v1/projects/{id}/execution-workers",
    "/api/v1/projects/{id}/mentionable-actors",
    "/api/v1/projects/{id}/navigation",
    "/api/v1/projects/{id}/pages",
    "/api/v1/projects/{project_id}/execution-profiles/{profile_id}/account",
    "/api/v1/projects/{project_id}/external-agents",
    "/api/v1/projects/{project_id}/external-agents/{connection_id}/workos",
    "/api/v1/projects/{project_id}/external-execution-requests",
    "/api/v1/projects/{project_id}/integrations/github",
    "/api/v1/projects/{project_id}/integrations/github/automation",
    "/api/v1/projects/{project_id}/invitations",
    "/api/v1/projects/{project_id}/member-candidates",
    "/api/v1/projects/{project_id}/members",
    "/api/v1/projects/{project_id}/repositories",
    "/api/v1/projects/{project_id}/secrets",
    "/api/v1/projects/{project_id}/work-item-assignees",
    "/api/v1/projects/{project_id}/work-item-references",
    "/api/v1/projects/{project_id}/work-item-workflow",
    "/api/v1/projects/{project_id}/work-items",
    "/api/v1/projects/{project_id}/work-items/board",
    "/api/v1/projects/{project_id}/work-items/by-key/{key}",
    "/api/v1/projects/{project_id}/work-items/{work_item_id}",
    "/api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria",
    "/api/v1/projects/{project_id}/work-items/{work_item_id}/claim",
    "/api/v1/projects/{project_id}/work-items/{work_item_id}/timeline",
    "/api/v1/search",
    "/api/v1/settings/effective",
    "/api/v1/settings/registry",
];

/// GET routes under `/api/v1` this suite deliberately doesn't call as plain
/// HTTP requests, and why. (Routes outside `/api/v1` aren't workspace data:
/// `/metrics` is a token-gated operator endpoint with process-wide counters,
/// `/health/*`, `/auth.md`, `/connect.md`, OAuth and the WorkOS callback are
/// sign-in plumbing, so the guard only looks under `/api/v1`.)
const HTTP_EXCLUSIONS: &[(&str, &str)] = &[(
    "/api/v1/realtime",
    "WebSocket upgrade; covered by the realtime part of the fixture test, which \
     subscribes as A and checks no B event arrives",
)];

/// Read-only MCP tools this suite deliberately doesn't call, and why.
/// (Every read-only tool currently has a case.)
const MCP_EXCLUSIONS: &[(&str, &str)] = &[];

// ---------------------------------------------------------------------------
// Coverage guard (no database needed)
// ---------------------------------------------------------------------------

#[test]
fn every_get_route_and_read_tool_has_an_isolation_case() {
    let source_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    collect_sources(&source_root, &mut sources);
    assert!(
        !sources.is_empty(),
        "no server sources under {source_root:?}"
    );

    let routes = sources
        .iter()
        .flat_map(|(_, source)| get_routes(source))
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
        .find(|(path, _)| path.ends_with("src/mcp.rs"))
        .map(|(_, source)| source.as_str())
        .expect("src/mcp.rs");
    let read_tools = read_only_tools(mcp_source);
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

fn collect_sources(directory: &std::path::Path, sources: &mut Vec<(String, String)>) {
    let mut entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read {directory:?}: {error}"))
        .map(|entry| entry.expect("directory entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_sources(&path, sources);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = std::fs::read_to_string(&path).expect("read source");
            sources.push((path.to_string_lossy().replace('\\', "/"), source));
        }
    }
}

/// The paths of `.route("...", ...)` calls that register a GET handler.
fn get_routes(source: &str) -> Vec<String> {
    route_blocks(source)
        .into_iter()
        .filter(|block| has_call(block, "get("))
        .filter_map(|block| first_string_literal(block).map(str::to_owned))
        .collect()
}

/// `token` appears as a call, not as the end of a longer name.
fn has_call(block: &str, token: &str) -> bool {
    block.match_indices(token).any(|(index, _)| {
        !block[..index]
            .chars()
            .next_back()
            .is_some_and(|previous| previous.is_ascii_alphanumeric() || previous == '_')
    })
}

/// The same balanced-parenthesis scan `policy_inventory`'s test uses.
fn route_blocks(source: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut remaining = source;
    while let Some(relative_start) = remaining.find(".route(") {
        let start = source.len() - remaining.len() + relative_start;
        let open = start + ".route".len();
        let mut depth = 0_u32;
        let mut in_string = false;
        let mut escaped = false;
        let mut end = None;
        for (offset, character) in source[open..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    in_string = false;
                }
                continue;
            }
            match character {
                '"' => in_string = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + offset + character.len_utf8());
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end.expect("balanced route call");
        blocks.push(&source[start..end]);
        remaining = &source[end..];
    }
    blocks
}

fn first_string_literal(block: &str) -> Option<&str> {
    let start = block.find('"')? + 1;
    let end = block[start..].find('"')? + start;
    Some(&block[start..end])
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

/// Everything one workspace has, as IDs and keys a client would use.
struct World {
    marker: &'static str,
    browser: Browser,
    organization: Uuid,
    rook: Uuid,
    agent_token: String,
    agent_connection: Uuid,
    app_token: String,
    space: String,
    project: String,
    channel: String,
    message: String,
    thread: String,
    dm: String,
    dm_message: String,
    group: String,
    task: Uuid,
    subtask: Uuid,
    page: String,
    repository: Uuid,
    pull_request: Uuid,
    pull_request_url: String,
    profile: Uuid,
    run_thread: Uuid,
    run: Uuid,
    approval: Uuid,
}

#[tokio::test]
async fn one_workspace_never_sees_another() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("workspace_isolation_{}", Uuid::now_v7().simple());
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");

    let temporary = tempfile::tempdir().expect("temporary root");
    let data_root = temporary.path().join("data");
    std::fs::create_dir_all(&data_root).expect("data root");
    // The secrets keyring lives outside the data and web roots, owner-only.
    let keys = tempfile::tempdir().expect("keyring root");
    let keyring_path = keys.path().join("secrets.keyring");
    std::fs::write(
        &keyring_path,
        FileKeyring::generate().serialize().as_bytes(),
    )
    .expect("write keyring");
    std::fs::set_permissions(&keyring_path, std::fs::Permissions::from_mode(0o600))
        .expect("keyring permissions");
    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            database_url.clone(),
        ),
        ("AGENT_WORKSPACE_ENV".to_owned(), "development".to_owned()),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            data_root.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_HOME_ROOT".to_owned(),
            data_root.join("profiles").to_string_lossy().into_owned(),
        ),
        (
            // No Codex here: account reads report the provider unavailable.
            "AGENT_WORKSPACE_CODEX_BINARY".to_owned(),
            temporary
                .path()
                .join("no-codex")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "AGENT_WORKSPACE_WEB_ROOT".to_owned(),
            temporary.path().to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_SECRETS_KEY_PATH".to_owned(),
            keyring_path.to_string_lossy().into_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_REQUEST_TIMEOUT_SECONDS".to_owned(),
            "2".to_owned(),
        ),
        (
            "AGENT_WORKSPACE_CODEX_SHUTDOWN_TIMEOUT_SECONDS".to_owned(),
            "1".to_owned(),
        ),
    ]))
    .expect("test configuration");
    let supervisors = SupervisorRegistry::new(&config).expect("supervisors");
    let realtime = RealtimeHub::new(8192);
    let app = router(AppState {
        config: Arc::new(config),
        pool: pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: realtime.clone(),
        supervisors: supervisors.clone(),
    });

    // Workspace A: the first owner, through the normal bootstrap.
    let a_owner = bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            organization_name: Some("Acme".to_owned()),
            organization_slug: Some(format!("acme-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap A");
    let a_browser = login(&app).await;
    // Workspace B: bootstrap only makes the first workspace, so B's owner is
    // made the way a SaaS sign-up would: a new organization and its owner.
    let b_organization = second_workspace_owner(&pool).await;
    let b_browser = login_as(&app, &format!("owner-{MARKER}")).await;

    let a = build_workspace(&app, &pool, "A", "", a_owner.organization_id, a_browser).await;
    let b = build_workspace(&app, &pool, "B", MARKER, b_organization, b_browser).await;
    let b_ids = sweep_ids(&pool, b.organization).await;
    let a_ids = sweep_ids(&pool, a.organization).await;
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
    check_http(&app, &a, &b, &b_ids, &mut report).await;
    check_mcp(&app, &a, &b, &b_ids, &mut report).await;
    check_realtime(&app, &pool, &realtime, &a, &b, &b_ids, &mut report).await;

    for note in &report.notes {
        eprintln!("note: {note}");
    }
    eprintln!(
        "checked {} HTTP requests and {} MCP calls",
        report.http_requests, report.mcp_calls
    );
    let failures = report.failures.clone();

    supervisors.shutdown_all().await;
    drop(app);
    pool.close().await;
    admin
        .execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
        .await
        .expect("drop schema");

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
fn leaked(body: &str, b_ids: &HashSet<String>) -> Vec<String> {
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

async fn second_workspace_owner(pool: &PgPool) -> Uuid {
    let organization = Uuid::now_v7();
    let actor = Uuid::now_v7();
    let user = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(actor)
        .execute(pool)
        .await
        .expect("B owner actor");
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, $2, $3)")
        .bind(organization)
        .bind(format!("Acme {MARKER}"))
        .bind(format!("acme-{MARKER}-{}", Uuid::now_v7().simple()))
        .execute(pool)
        .await
        .expect("B organization");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role, enabled) VALUES ($1, $2, 'owner', true)",
    )
    .bind(organization)
    .bind(actor)
    .execute(pool)
    .await
    .expect("B owner membership");
    sqlx::query(
        "INSERT INTO users (id, actor_id, login_name, display_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(user)
    .bind(actor)
    .bind(format!("owner-{MARKER}"))
    .bind(format!("Owner {MARKER}"))
    .execute(pool)
    .await
    .expect("B owner user");
    sqlx::query("INSERT INTO user_password_credentials (user_id, password_phc) VALUES ($1, $2)")
        .bind(user)
        .bind(
            hash_password(PASSWORD.to_owned())
                .await
                .expect("hash password"),
        )
        .execute(pool)
        .await
        .expect("B owner password");
    organization
}

async fn login_as(app: &axum::Router, login_name: &str) -> Browser {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("origin", ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({ "loginName": login_name, "password": PASSWORD }))
                        .expect("login body"),
                ))
                .expect("login request"),
        )
        .await
        .expect("login response");
    assert_eq!(response.status(), StatusCode::OK, "login {login_name}");
    let cookies = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok()?.split(';').next().map(str::to_owned))
        .collect::<Vec<_>>();
    let _ = to_bytes(response.into_body(), 100_000).await;
    let csrf = cookies
        .iter()
        .find_map(|cookie| cookie.strip_prefix("agent_workspace_csrf="))
        .expect("CSRF cookie")
        .to_owned();
    Browser {
        cookie: cookies.join("; "),
        csrf,
    }
}

/// A person who can sign in, with the same handle in both workspaces.
async fn add_person(
    setup: &Setup,
    projects: &[&str],
    login_name: &str,
    handle: &str,
    display_name: &str,
) -> Uuid {
    let actor_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', 'active')")
        .bind(actor_id)
        .execute(&setup.pool)
        .await
        .expect("person actor");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(setup.organization_id)
    .bind(actor_id)
    .execute(&setup.pool)
    .await
    .expect("organization membership");
    for project in projects {
        sqlx::query(
            r#"
            INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle)
            VALUES ($1, $2, $3, 'member', $4, $5)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(project.parse::<Uuid>().expect("project"))
        .bind(actor_id)
        .bind(display_name)
        .bind(handle)
        .execute(&setup.pool)
        .await
        .expect("project membership");
    }
    bootstrap_development_member(
        &setup.pool,
        BootstrapCommand {
            login_name: login_name.to_owned(),
            display_name: display_name.to_owned(),
            organization_name: None,
            organization_slug: None,
            adopt_actor_id: Some(actor_id),
            organization_id: Some(setup.organization_id),
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap person");
    actor_id
}

fn string_at(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no {pointer} in {value}"))
        .to_owned()
}

/// Build one workspace's look-alike data. `marker` goes into every name and
/// text B writes; A's `marker` is empty.
async fn build_workspace(
    app: &axum::Router,
    pool: &PgPool,
    label: &'static str,
    marker: &'static str,
    organization: Uuid,
    browser: Browser,
) -> World {
    let m = if marker.is_empty() {
        String::new()
    } else {
        format!(" {marker}")
    };
    let tag = if marker.is_empty() { "a" } else { marker };
    let setup = Setup {
        app: app.clone(),
        browser: Browser {
            cookie: browser.cookie.clone(),
            csrf: browser.csrf.clone(),
        },
        organization_id: organization,
        pool: pool.clone(),
    };
    let space = space(&setup).await;
    let project = create_project(&setup, "WORK", &format!("Work{m}")).await;
    let rook = add_person(
        &setup,
        &[&project, &space],
        &format!("rook-{tag}"),
        "rook",
        &format!("Rook{m}"),
    )
    .await;
    add_person(
        &setup,
        &[&space],
        &format!("wren-{tag}"),
        "wren",
        &format!("Wren{m}"),
    )
    .await;
    let (agent, agent_token) =
        create_agent(app, &browser, &space, &format!("Mara{m}"), "mara").await;
    add_member(&setup, &project, agent, "member").await;
    let agent_connection = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM external_agent_connections WHERE actor_id = $1",
    )
    .bind(agent)
    .fetch_one(pool)
    .await
    .expect("agent connection");

    let app_token = connect_app(app, &browser, &format!("Codex{m}"), "full").await;
    let session = initialize(app, &app_token).await.expect("app MCP");
    let call = |name: &'static str, arguments: Value| {
        let token = app_token.clone();
        let session = session.clone();
        async move {
            let (status, body) = call_tool(app, &token, &session, name, arguments.clone()).await;
            let value = structured(&body);
            assert!(
                succeeded(&value["outcome"]),
                "{label} fixture {name} {arguments} -> {status} {body}"
            );
            value
        }
    };

    call(
        "label_write",
        json!({"projectId": "WORK", "name": format!("Launch{m}")}),
    )
    .await;
    call(
        "task_create",
        json!({
            "projectId": "WORK",
            "title": format!("Launch plan{m}"),
            "description": format!("Ship the launch{m}"),
            "assignee": "@rook",
            "labels": [format!("Launch{m}")],
            "acceptanceCriteria": [format!("Launch checklist{m}")],
            "clientRequestId": "task-1"
        }),
    )
    .await;
    call(
        "task_create",
        json!({"parentId": "WORK-1", "title": format!("Launch subtask{m}"), "clientRequestId": "task-2"}),
    )
    .await;
    let task_number = |number: i64| {
        let project = project.clone();
        async move {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM work_items WHERE project_id = $1::uuid AND number = $2",
            )
            .bind(project)
            .bind(number)
            .fetch_one(pool)
            .await
            .expect("task")
        }
    };
    let task = task_number(1).await;
    let subtask = task_number(2).await;
    call(
        "task_comment",
        json!({"workItemId": "WORK-1", "update": format!("Launch comment{m}"), "clientRequestId": "comment-1"}),
    )
    .await;
    call(
        "milestone_write",
        json!({"projectId": "WORK", "name": format!("Launch milestone{m}"), "addTasks": ["WORK-1"]}),
    )
    .await;
    let page = call(
        "page_write",
        json!({"projectId": "WORK", "title": format!("Launch page{m}"), "body": format!("Launch notes{m}")}),
    )
    .await;
    let page = string_at(&page, "/value/id");
    call(
        "page_write",
        json!({"pageId": page, "body": format!("Launch notes, second draft{m}"), "expectedVersion": 1}),
    )
    .await;

    let channel = create_channel(&setup, &project, "launch").await;
    let message = say(
        &setup,
        &browser,
        &channel,
        "message-1",
        text(&format!("Launch message{m}")),
    )
    .await;
    let (status, edited) = edit(
        &setup,
        &browser,
        &channel,
        &message,
        "edit-1",
        json!({"content": text(&format!("Launch message, edited{m}"))}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{label} edit: {edited}");
    let thread = call(
        "thread_start",
        json!({"conversationId": channel, "name": format!("Launch thread{m}"), "text": format!("Launch thread reply{m}")}),
    )
    .await;
    let thread = string_at(&thread, "/value/id");
    call(
        "decision_record",
        json!({"projectId": "WORK", "title": format!("Launch decision{m}"), "conversationId": channel}),
    )
    .await;
    call(
        "pin_set",
        json!({"conversationId": channel, "messageId": message}),
    )
    .await;

    let (status, dm) = send(
        app,
        browser.request("POST", "/api/v1/direct/dms", None, json!({"actorId": rook})),
    )
    .await;
    assert!(status.is_success(), "{label} DM: {status} {dm}");
    let dm = string_at(&dm, "/conversation/id");
    let dm_message = say(
        &setup,
        &browser,
        &dm,
        "dm-1",
        text(&format!("Launch DM{m}")),
    )
    .await;
    let group = call(
        "conversation_start",
        json!({"people": ["@rook", "@wren"], "name": format!("Launch group{m}"), "text": format!("Launch group hello{m}")}),
    )
    .await;
    let group = string_at(&group, "/value/conversation/id");

    // The agent claims the task, so the claim route has something to show.
    let agent_session = initialize(app, &agent_token).await.expect("agent MCP");
    let (_, claimed) = call_tool(
        app,
        &agent_token,
        &agent_session,
        "task_claim",
        json!({"taskId": "WORK-1", "note": format!("Launch claim{m}")}),
    )
    .await;
    assert!(
        succeeded(&structured(&claimed)["outcome"]),
        "{label} claim: {claimed}"
    );

    let (status, invitation) = send(
        app,
        browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/invitations"),
            Some("invite-1"),
            json!({"role": "viewer", "expiresInHours": 24}),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "{label} invitation: {status} {invitation}"
    );

    // Code: the same repository name in both, and a pull request on it.
    let repository = insert_repository(&setup, &project, "acme/widgets", true, true, None).await;
    let (pull_request, pull_request_url) = seed_pull_request(&setup, &project, repository, 7).await;

    // Runs: a personal Codex profile and a run blocked on an approval in a
    // repository-bound thread.
    let (status, registered) = send(
        app,
        browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/execution-profiles"),
            Some("profile-1"),
            json!({}),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "{label} profile: {status} {registered}"
    );
    let profile = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM execution_profiles WHERE project_id = $1::uuid",
    )
    .bind(&project)
    .fetch_one(pool)
    .await
    .expect("profile");
    sqlx::query("UPDATE execution_profiles SET status = 'available' WHERE id = $1")
        .bind(profile)
        .execute(pool)
        .await
        .expect("profile available");
    sqlx::query("UPDATE execution_workers SET enabled = true, pending_setup = false WHERE project_id = $1::uuid")
        .bind(&project)
        .execute(pool)
        .await
        .expect("worker enabled");
    let worker = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM execution_workers WHERE project_id = $1::uuid",
    )
    .bind(&project)
    .fetch_one(pool)
    .await
    .expect("worker");
    let run_thread = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, parent_channel_id, repository_id, name)
        VALUES ($1, $2::uuid, 'thread', $3::uuid, $4, $5)
        "#,
    )
    .bind(run_thread)
    .bind(&project)
    .bind(&channel)
    .bind(repository)
    .bind(format!("Launch run thread{m}"))
    .execute(pool)
    .await
    .expect("run thread");
    let run = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot, started_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, 'failed', $7, '{}'::jsonb, now())
        "#,
    )
    .bind(run)
    .bind(run_thread)
    .bind(worker)
    .bind(rook)
    .bind(profile)
    .bind(repository)
    .bind(format!("Launch run{m}"))
    .execute(pool)
    .await
    .expect("run");
    // Admission checks a live Codex account on insert; a run that is already
    // waiting on an approval only needs the row.
    sqlx::query("UPDATE agent_runs SET state = 'blocked' WHERE id = $1")
        .bind(run)
        .execute(pool)
        .await
        .expect("run blocked");
    let approval = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_approvals (
            id, agent_run_id, requesting_actor_id, provider_request_id,
            kind, summary, options, eligible_role, human_required
        )
        VALUES ($1, $2, $3, $4, 'approval', $5,
                '[{"id":"approve","label":"Allow"},{"id":"deny","label":"Deny"}]'::jsonb,
                'any_authorized', false)
        "#,
    )
    .bind(approval)
    .bind(run)
    .bind(rook)
    .bind(format!("isolation-{approval}"))
    .bind(format!("Launch approval{m}"))
    .execute(pool)
    .await
    .expect("approval");

    // Settings and secrets at workspace and project scope.
    let model = if marker.is_empty() {
        "launch-model".to_owned()
    } else {
        format!("{marker}-model")
    };
    let secret_name = if marker.is_empty() {
        "launch_key".to_owned()
    } else {
        format!("{marker}_key")
    };
    for (uri, body) in [
        (
            "/api/v1/settings/values/run.model".to_owned(),
            json!({"scope": "workspace", "organizationId": organization, "value": model}),
        ),
        (
            "/api/v1/settings/values/run.model".to_owned(),
            json!({"scope": "project", "projectId": project, "value": format!("{model}-project")}),
        ),
        (
            format!("/api/v1/organizations/{organization}/secrets/{secret_name}"),
            json!({"value": format!("value-{secret_name}"), "description": format!("Launch key{m}")}),
        ),
        (
            format!("/api/v1/organizations/{organization}/personal-secrets/{secret_name}"),
            json!({"value": format!("personal-{secret_name}")}),
        ),
        (
            format!("/api/v1/projects/{project}/secrets/{secret_name}"),
            json!({"value": format!("project-{secret_name}")}),
        ),
    ] {
        let (status, written) = send(app, browser.request("PUT", &uri, None, body)).await;
        assert!(status.is_success(), "{label} PUT {uri}: {status} {written}");
    }

    World {
        marker,
        browser,
        organization,
        rook,
        agent_token,
        agent_connection,
        app_token,
        space,
        project,
        channel,
        message,
        thread,
        dm,
        dm_message,
        group,
        task,
        subtask,
        page,
        repository,
        pull_request,
        pull_request_url,
        profile,
        run_thread,
        run,
        approval,
    }
}

/// Every ID a workspace owns: its organization, its people and agents, and
/// every row (in any table) of its projects, their conversations, tasks and
/// runs. Found from the catalog, so new tables are swept automatically.
async fn sweep_ids(pool: &PgPool, organization: Uuid) -> HashSet<String> {
    let mut ids = BTreeSet::<Uuid>::new();
    ids.insert(organization);
    let projects =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM projects WHERE organization_id = $1")
            .bind(organization)
            .fetch_all(pool)
            .await
            .expect("projects");
    ids.extend(projects.iter().copied());
    let actors = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT actor_id FROM organization_memberships WHERE organization_id = $1
        UNION
        SELECT membership.actor_id FROM project_memberships membership
        JOIN projects project ON project.id = membership.project_id
        WHERE project.organization_id = $1
        "#,
    )
    .bind(organization)
    .fetch_all(pool)
    .await
    .expect("actors");
    ids.extend(actors.iter().copied());
    let users = sqlx::query_scalar::<_, Uuid>("SELECT id FROM users WHERE actor_id = ANY($1)")
        .bind(&actors)
        .fetch_all(pool)
        .await
        .expect("users");
    ids.extend(users);

    // Tables with a UUID `id` and one of these parent columns.
    let tables = sqlx::query_as::<_, (String, String)>(
        r#"
        SELECT parent.table_name::text, parent.column_name::text
        FROM information_schema.columns parent
        JOIN information_schema.columns own
          ON own.table_schema = parent.table_schema
         AND own.table_name = parent.table_name
         AND own.column_name = 'id'
         AND own.data_type = 'uuid'
        JOIN information_schema.tables listed
          ON listed.table_schema = parent.table_schema
         AND listed.table_name = parent.table_name
         AND listed.table_type = 'BASE TABLE'
        WHERE parent.table_schema = current_schema()
          AND parent.data_type = 'uuid'
          AND parent.column_name IN (
              'organization_id', 'project_id', 'conversation_id', 'work_item_id',
              'agent_run_id', 'actor_id', 'repository_id', 'page_id', 'message_id'
          )
        ORDER BY 1, 2
        "#,
    )
    .fetch_all(pool)
    .await
    .expect("catalog");
    // Children can hang off children, so sweep until nothing new turns up.
    loop {
        let before = ids.len();
        let known = ids.iter().copied().collect::<Vec<_>>();
        for (table, column) in &tables {
            let found = sqlx::query_scalar::<_, Uuid>(&format!(
                "SELECT id FROM {table} WHERE {column} = ANY($1)"
            ))
            .bind(&known)
            .fetch_all(pool)
            .await
            .unwrap_or_else(|error| panic!("sweep {table}.{column}: {error}"));
            ids.extend(found);
        }
        if ids.len() == before {
            break;
        }
    }
    ids.into_iter()
        .flat_map(|id| [id.to_string(), id.simple().to_string()])
        .collect()
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

struct HttpCase {
    route: &'static str,
    /// A's own requests: must succeed and carry nothing of B's.
    own: Vec<String>,
    /// Requests naming B's resources: must not return B's data (404 or 403
    /// expected; a success without B data is reported as a note).
    foreign: Vec<String>,
    /// Requests that legitimately succeed (a search for B's words, an
    /// optional filter naming B), but must not return B's data.
    filtered: Vec<String>,
}

fn http_cases(a: &World, b: &World) -> Vec<HttpCase> {
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
            "/api/v1/auth/session",
            vec!["/api/v1/auth/session".to_owned()],
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
            "/api/v1/organizations/{organization_id}/secrets",
            vec![format!("/api/v1/organizations/{}/secrets", a.organization)],
            vec![format!("/api/v1/organizations/{}/secrets", b.organization)],
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
            vec![format!("{}/work-item-assignees?q={MARKER}", p(a))],
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
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/timeline",
            "/timeline",
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
    cases.push(case(
        "/api/v1/pages/{id}",
        vec![format!("/api/v1/pages/{}", a.page)],
        vec![format!("/api/v1/pages/{}", b.page)],
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

async fn get_text(app: &axum::Router, browser: &Browser, uri: &str) -> (StatusCode, String) {
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

struct McpCase {
    tool: &'static str,
    /// Who must succeed with `own`: most tools are for A's connected app
    /// (acting as A's owner); a few are only for agents.
    agent_only: bool,
    own: Vec<Value>,
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
    "approval_get",
    "approval_list",
    "briefing",
    "conversation_list",
    "conversation_open",
    "conversation_read",
    "decision_list",
    "external_event_poll",
    "inbox_list",
    "label_list",
    "message_history",
    "milestone_list",
    "notification_list",
    "page_get",
    "page_list",
    "people_search",
    "project_members",
    "project_open",
    "projects_list",
    "pull_request_get",
    "pull_request_list",
    "repository_get",
    "repository_list",
    "run_get",
    "run_list",
    "search",
    "settings_get",
    "task_list",
    "task_open",
    "workflow_get",
];

fn mcp_cases(a: &World, b: &World) -> Vec<McpCase> {
    fn case(
        tool: &'static str,
        own: Vec<Value>,
        foreign: Vec<Value>,
        filtered: Vec<Value>,
    ) -> McpCase {
        McpCase {
            tool,
            agent_only: matches!(tool, "inbox_list" | "external_event_poll"),
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
            "briefing",
            vec![json!({}), json!({"projectId": "WORK"})],
            vec![json!({"projectId": b.project})],
            vec![],
        ),
        case(
            "projects_list",
            vec![json!({}), json!({"includeArchived": true})],
            vec![],
            vec![],
        ),
        by_project("project_open"),
        by_project("workflow_get"),
        by_project("project_members"),
        by_project("label_list"),
        by_project("page_list"),
        by_project("decision_list"),
        by_project("milestone_list"),
        by_project("settings_get"),
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
            "notification_list",
            vec![
                json!({}),
                json!({"view": "archived"}),
                json!({"projectId": a.project}),
            ],
            vec![],
            vec![json!({"projectId": b.project})],
        ),
        case(
            "task_list",
            vec![json!({}), json!({"projectId": "WORK"})],
            vec![
                json!({"projectId": b.project}),
                json!({"projectIds": [b.project]}),
            ],
            vec![json!({"projectId": "WORK", "assignee": b.rook.to_string()})],
        ),
        case(
            "task_open",
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
            "search",
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
            "approval_list",
            vec![json!({}), json!({"projectId": "WORK"})],
            vec![json!({"projectId": b.project})],
            vec![],
        ),
        case(
            "approval_get",
            vec![json!({"approvalId": a.approval.to_string()})],
            vec![
                json!({"approvalId": b.approval.to_string()}),
                json!({"approvalId": b.approval.to_string(), "projectId": a.project}),
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
    ]
}

/// Every case runs as A's connected app (acting as A's owner) and as A's
/// agent. Own calls must succeed for whoever the tool is for; for the other,
/// only leaks count.
async fn check_mcp(
    app: &axum::Router,
    a: &World,
    b: &World,
    b_ids: &HashSet<String>,
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

    for (who, token, session, is_agent) in [
        ("A's app", &a.app_token, &app_session, false),
        ("A's agent", &a.agent_token, &agent_session, true),
    ] {
        for case in &cases {
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
        }
    }
}

async fn mcp_post(
    app: &axum::Router,
    token: &str,
    session: &str,
    payload: Value,
) -> (StatusCode, String) {
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
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 8_000_000)
        .await
        .expect("body");
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Call a tool without asserting anything; returns the raw response.
async fn call_tool(
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

fn succeeded(outcome: &Value) -> bool {
    matches!(
        outcome.as_str(),
        Some("ok" | "found" | "applied" | "events")
    )
}

fn json_body(text: &str) -> Value {
    let json = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| !line.trim().is_empty())
        .unwrap_or(text);
    serde_json::from_str(json).unwrap_or(Value::Null)
}

/// A tool result's structured content, or Null.
fn structured(text: &str) -> Value {
    json_body(text)
        .pointer("/result/structuredContent")
        .cloned()
        .unwrap_or(Value::Null)
}

async fn list_tools(app: &axum::Router, token: &str, session: &str) -> Vec<Value> {
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

/// A subscribes over the real WebSocket, then the outbox publisher starts:
/// the whole fixture's backlog (both workspaces) is broadcast, B changes a
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

    let (stop_publisher, publisher_stopped) = watch::channel(false);
    let publisher = tokio::spawn(run_outbox_publisher(
        pool.clone(),
        realtime.clone(),
        outbox_worker_id(),
        publisher_stopped,
    ));

    // More changes in B while A listens.
    let b_setup = Setup {
        app: app.clone(),
        browser: Browser {
            cookie: b.browser.cookie.clone(),
            csrf: b.browser.csrf.clone(),
        },
        organization_id: b.organization,
        pool: pool.clone(),
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

    // Then A's sentinel.
    let a_setup = Setup {
        app: app.clone(),
        browser: Browser {
            cookie: a.browser.cookie.clone(),
            csrf: a.browser.csrf.clone(),
        },
        organization_id: a.organization,
        pool: pool.clone(),
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

    let _ = stop_publisher.send(true);
    let _ = publisher.await;
    let _ = socket.close(None).await;
    let _ = stop_server.send(());
    let _ = server.await;
}
