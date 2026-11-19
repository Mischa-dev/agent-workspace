//! A privacy net inside one workspace (P0.22). `workspace_isolation` checks
//! that nothing crosses between workspaces; this checks that, inside
//! workspace A, a member never reads a conversation they aren't in, and a
//! member whose role can't see tasks or code never reads them.
//!
//! Two callers join A: `outsider`, a member of the project and the direct
//! space who is in no DM or group with the private content, and `narrowed`,
//! a member on a custom role without `tasks.view` and `code.view`. A DM, a
//! group, a thread under the group and files attached there carry
//! [`PRIVATE`]; a task, its comment and a pull request carry [`TASK`]. Both
//! callers walk every GET route and read-only tool `workspace_isolation`
//! walks, with A's own IDs, and every project-scoped one with the direct
//! space's ID too.
//!
//! A leak this finds goes on [`KNOWN_GAPS`] with where it is tracked; the
//! list only shrinks, and an entry that no longer leaks fails until it's
//! taken off.
//!
//! P0.38 (ruling R19) adds the account cases: a person in A only, an app they
//! connected and A's agent never see B's workspace, projects or people
//! through any account list or search, though a person they share A with is
//! also in B; and every GET route that answers with accounts is walked or
//! excused with a reason.

use std::collections::{BTreeMap, BTreeSet};

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use agent_workspace_server::test_support::source::{rust_files, server_src, strip_tests};

use super::{
    message_edits::{say, text},
    support::{
        app::{Browser, connect_app, initialize, send},
        callers::{Agent, Call, Caller, ConnectedApp, Person, drain_outbox},
        two_workspaces::{
            TwoWorkspaces, World, add_person, login_as, string_at, two_workspaces, upload,
        },
        world::{Setup, create_project},
    },
    workspace_isolation::{call_tool, get_text, http_cases, mcp_cases, structured, succeeded},
};

/// Text only the private conversations' members may read.
const PRIVATE: &str = "okapi";
/// Text only people who can see tasks and code may read.
const TASK: &str = "quokka";

/// Leaks found when the net was built, as `"<caller> <GET route | tool>"`
/// and where each is tracked. Fix one and take it off; the list only
/// shrinks.
const KNOWN_GAPS: &[(&str, &str)] = &[
    (
        "narrowed GET /api/v1/projects/{project_id}/collections/{collection_id}/records",
        "a record's task field shows the task's title to a role without tasks.view; \
         found by P0.22, reported in build 0's pull request for a task",
    ),
    (
        "narrowed GET /api/v1/projects/{project_id}/collections/{collection_id}/records/{record_id}",
        "the same record field, opened by id; found by P0.22, reported in build 0's pull \
         request for a task",
    ),
];

/// Who walks, what they must never read, and how they call.
struct Walker {
    name: &'static str,
    browser: Browser,
    token: String,
    session: String,
    /// Words that must not appear.
    markers: Vec<&'static str>,
    /// IDs that must not appear unless the request named them.
    ids: BTreeSet<String>,
}

#[tokio::test]
async fn members_never_read_what_they_are_not_in() {
    let Some(fixture) = two_workspaces("privacy_inside").await else {
        return;
    };
    let TwoWorkspaces {
        app, pool, a, b, ..
    } = &fixture;
    let setup = Setup {
        app: app.clone(),
        browser: Browser {
            cookie: a.browser.cookie.clone(),
            csrf: a.browser.csrf.clone(),
        },
        organization_id: a.organization,
        pool: pool.clone(),
        cleanup: None,
    };
    let owner_session = initialize(app, &a.app_token).await.expect("A app MCP");
    let owner_call = |name: &'static str, arguments: Value| {
        let session = owner_session.clone();
        async move {
            let (status, body) =
                call_tool(app, &a.app_token, &session, name, arguments.clone()).await;
            let value = structured(&body);
            assert!(
                succeeded(&value["outcome"]),
                "privacy fixture {name} {arguments} -> {status} {body}"
            );
            value
        }
    };

    // The private content: a DM, a group, a thread under the group, and a
    // file in each conversation.
    let mut private_ids = BTreeSet::from([a.dm.clone(), a.dm_message.clone(), a.group.clone()]);
    private_ids.insert(
        say(
            &setup,
            &a.browser,
            &a.dm,
            "okapi-dm",
            text(&format!("The {PRIVATE} plan")),
        )
        .await,
    );
    private_ids.insert(
        say(
            &setup,
            &a.browser,
            &a.group,
            "okapi-group",
            text(&format!("Group {PRIVATE} notes")),
        )
        .await,
    );
    let group_thread = owner_call(
        "thread_start",
        json!({"conversationId": a.group, "name": format!("The {PRIVATE} thread"), "text": format!("A {PRIVATE} reply")}),
    )
    .await;
    private_ids.insert(string_at(&group_thread, "/value/id"));
    for conversation in [&a.dm, &a.group] {
        let file = upload(
            app,
            &a.browser,
            &format!("/api/v1/conversations/{conversation}/files?name={PRIVATE}-notes.txt"),
            "text/plain",
            format!("Notes on the {PRIVATE}").into_bytes(),
        )
        .await;
        let (status, sent) = send(
            app,
            a.browser.request(
                "POST",
                &format!("/api/v1/conversations/{conversation}/messages"),
                Some(&format!("okapi-file-{conversation}")),
                json!({"content": text(&format!("{PRIVATE} file")), "attachments": [file]}),
            ),
        )
        .await;
        assert!(status.is_success(), "private file message: {status} {sent}");
        private_ids.insert(file.to_string());
        private_ids.insert(string_at(&sent, "/message/id"));
    }

    // The task and code content.
    let task = owner_call(
        "task_create",
        json!({"projectId": "WORK", "title": format!("The {TASK} migration"), "clientRequestId": "quokka-task"}),
    )
    .await;
    let task_key = string_at(&task, "/value/key");
    owner_call(
        "task_comment",
        json!({"workItemId": task_key, "update": format!("{TASK} is half done"), "clientRequestId": "quokka-comment"}),
    )
    .await;
    // WORK-1, which most of the cases name, carries it too.
    owner_call(
        "task_comment",
        json!({"workItemId": "WORK-1", "update": format!("The {TASK} estimate"), "clientRequestId": "quokka-comment-1"}),
    )
    .await;
    sqlx::query("UPDATE collection_records SET title = title || $2 WHERE id = $1")
        .bind(a.task)
        .bind(format!(" ({TASK})"))
        .execute(pool)
        .await
        .expect("task title");
    sqlx::query("UPDATE github_pull_requests SET title = $2 WHERE id = $1")
        .bind(a.pull_request)
        .bind(format!("Fix the {TASK}"))
        .execute(pool)
        .await
        .expect("pull request title");

    // The callers.
    let outsider = add_person(
        &setup,
        &[&a.project, &a.space],
        "outsider-a",
        "outsider",
        "Outsider",
    )
    .await;
    let narrowed = add_person(
        &setup,
        &[&a.project, &a.space],
        "narrowed-a",
        "narrowed",
        "Narrowed",
    )
    .await;
    narrow(&setup, narrowed).await;
    let mut walkers = Vec::new();
    for (name, login, markers) in [
        ("outsider", "outsider-a", vec![PRIVATE]),
        ("narrowed", "narrowed-a", vec![PRIVATE, TASK]),
    ] {
        let browser = login_as(app, login).await;
        let token = connect_app(app, &browser, &format!("{name} app"), "full").await;
        let session = initialize(app, &token).await.expect("walker MCP");
        walkers.push(Walker {
            name,
            browser,
            token,
            session,
            markers,
            ids: private_ids.clone(),
        });
    }
    let _ = outsider;

    // Positive controls: the owner reads both markers, so a clean walk means
    // something, and the narrowed role really hides the task.
    let (_, timeline) = get_text(
        app,
        &a.browser,
        &format!("/api/v1/conversations/{}/timeline", a.dm),
    )
    .await;
    assert!(
        timeline.to_ascii_lowercase().contains(PRIVATE),
        "owner's DM: {timeline}"
    );
    let task_id = string_at(&task, "/value/workItemId");
    let task_uri = format!("/api/v1/projects/{}/work-items/{task_id}", a.project);
    let (status, item) = get_text(app, &a.browser, &task_uri).await;
    assert!(
        status.is_success() && item.to_ascii_lowercase().contains(TASK),
        "owner's task: {item}"
    );
    let (status, item) = get_text(app, &walkers[1].browser, &task_uri).await;
    assert!(
        !status.is_success(),
        "narrowed reads the task: {status} {item}"
    );

    let mut leaks: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut unwalked = Vec::new();
    let mut requests = 0;

    let mut cases = http_cases(a, b)
        .into_iter()
        .map(|case| (case.route, case.own))
        .collect::<Vec<_>>();
    for (route, extra) in extra_requests(a, &task_id) {
        match cases.iter_mut().find(|(cased, _)| *cased == route) {
            Some((_, own)) => own.extend(extra),
            None => panic!("extra_requests names {route}, which has no isolation case"),
        }
    }
    for (route, own) in cases {
        if own.is_empty() {
            unwalked.push(route);
            continue;
        }
        for uri in with_space(a, &own) {
            for walker in &walkers {
                requests += 1;
                let (status, body) = get_text(app, &walker.browser, &uri).await;
                if let Some(found) = leaked(walker, &uri, &body) {
                    leaks
                        .entry(format!("{} GET {route}", walker.name))
                        .or_default()
                        .push(format!(
                            "GET {uri} -> {status}: {found:?} in {}",
                            excerpt(&body)
                        ));
                }
            }
        }
    }

    for case in mcp_cases(a, b) {
        if case.own.is_empty() {
            unwalked.push(case.tool);
            continue;
        }
        let mut arguments = case.own.clone();
        if case.tool == "search" {
            arguments.extend([PRIVATE, TASK].map(|word| json!({"query": word})));
        }
        for own in &case.own {
            if let Some(direct) = in_space(a, own) {
                arguments.push(direct);
            }
        }
        for walker in &walkers {
            for arguments in &arguments {
                requests += 1;
                let (_, body) = call_tool(
                    app,
                    &walker.token,
                    &walker.session,
                    case.tool,
                    arguments.clone(),
                )
                .await;
                if let Some(found) = leaked(walker, &arguments.to_string(), &body) {
                    leaks
                        .entry(format!("{} {}", walker.name, case.tool))
                        .or_default()
                        .push(format!(
                            "{} {arguments}: {found:?} in {}",
                            case.tool,
                            excerpt(&body)
                        ));
                }
            }
        }
    }

    // `ai_summarize` sends a conversation's text to the AI provider, so a
    // caller outside the DM must be refused before it gets that far.
    for walker in &walkers {
        for (project, conversation) in
            [(&a.space, &a.dm), (&a.space, &a.group), (&a.project, &a.dm)]
        {
            requests += 1;
            let arguments = json!({"projectId": project, "conversationId": conversation});
            let (_, body) = call_tool(
                app,
                &walker.token,
                &walker.session,
                "ai_summarize",
                arguments.clone(),
            )
            .await;
            let outcome = structured(&body)["outcome"].clone();
            // Refused before any text is read: not found, or not allowed.
            if outcome != "not_found" && outcome != "permission_denied" {
                leaks
                    .entry(format!("{} ai_summarize", walker.name))
                    .or_default()
                    .push(format!(
                        "ai_summarize {arguments} -> {outcome} (a refusal expected): {}",
                        excerpt(&body)
                    ));
            }
        }
    }

    eprintln!("privacy net: {requests} requests and calls");
    fixture.finish().await;

    let mut failures = Vec::new();
    if !unwalked.is_empty() {
        failures.push(format!(
            "COVERAGE cases with no request to walk (add one in extra_requests): {unwalked:?}"
        ));
    }
    for (key, found) in &leaks {
        if !KNOWN_GAPS.iter().any(|(gap, _)| gap == key) {
            failures.push(format!("LEAK {key}:\n    {}", found.join("\n    ")));
        }
    }
    for (gap, tracked) in KNOWN_GAPS {
        if !leaks.contains_key(*gap) {
            failures.push(format!(
                "KNOWN_GAPS lists {gap} ({tracked}), which no longer leaks: take it off"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} privacy failures inside a workspace:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Gives `actor` a custom workspace role that keeps everything a member has
/// except tasks and code, applied to their projects.
async fn narrow(setup: &Setup, actor: Uuid) {
    let (status, role) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/organizations/{}/roles", setup.organization_id),
            None,
            json!({
                "name": "No tasks or code",
                "baseRole": "member",
                "permissions": [
                    "projects.view", "channels.post", "channels.create", "pages.edit",
                    "whiteboards.edit", "collections.edit", "sheets.edit", "directory.view"
                ]
            }),
        ),
    )
    .await;
    assert!(status.is_success(), "narrowed role: {status} {role}");
    let role_id = role["role"]["id"].clone();
    let role_id = if role_id.is_null() {
        role["id"].clone()
    } else {
        role_id
    };
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM organization_memberships WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(setup.organization_id)
    .bind(actor)
    .fetch_one(&setup.pool)
    .await
    .expect("membership version");
    let (status, changed) = send(
        &setup.app,
        setup.browser.request(
            "PUT",
            &format!(
                "/api/v1/organizations/{}/members/{actor}/role",
                setup.organization_id
            ),
            None,
            json!({"roleId": role_id, "applyToProjects": true, "expectedVersion": version}),
        ),
    )
    .await;
    assert!(status.is_success(), "narrow {actor}: {status} {changed}");
}

/// Requests this net walks beyond the isolation cases' own: searches for
/// the markers, and a move preview for the marked task (whose isolation
/// case has no request of A's own, since A has one ordinary project there).
fn extra_requests(a: &World, task: &str) -> Vec<(&'static str, Vec<String>)> {
    vec![
        (
            "/api/v1/search",
            vec![
                format!("/api/v1/search?q={PRIVATE}"),
                format!("/api/v1/search?q={TASK}"),
                format!("/api/v1/search?q={TASK}&projectId={}", a.project),
            ],
        ),
        (
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/project-move",
            vec![format!(
                "/api/v1/projects/{}/work-items/{task}/project-move?targetProjectId={}",
                a.project, a.space
            )],
        ),
    ]
}

/// The URIs, and each project-scoped one again with the direct space's ID.
fn with_space(a: &World, uris: &[String]) -> Vec<String> {
    let project = format!("/api/v1/projects/{}", a.project);
    let space = format!("/api/v1/projects/{}", a.space);
    let mut all = uris.to_vec();
    all.extend(
        uris.iter()
            .filter(|uri| uri.contains(&project))
            .map(|uri| uri.replace(&project, &space)),
    );
    all
}

/// `arguments` with the direct space as its project, when it names one.
fn in_space(a: &World, arguments: &Value) -> Option<Value> {
    let project = arguments.get("projectId")?.as_str()?;
    if project != "WORK" && project != a.project {
        return None;
    }
    let mut direct = arguments.clone();
    direct["projectId"] = json!(a.space);
    Some(direct)
}

/// What `walker` mustn't read that `body` carries: a marker, or a private ID
/// the request didn't name itself.
fn leaked(walker: &Walker, input: &str, body: &str) -> Option<Vec<String>> {
    let lower = body.to_ascii_lowercase();
    let input = input.to_ascii_lowercase();
    let mut found = walker
        .markers
        .iter()
        .filter(|marker| lower.contains(*marker))
        .map(|marker| format!("marker {marker:?}"))
        .collect::<Vec<_>>();
    found.extend(
        walker
            .ids
            .iter()
            .filter(|id| lower.contains(id.as_str()) && !input.contains(id.as_str()))
            .map(|id| format!("id {id}")),
    );
    (!found.is_empty()).then_some(found)
}

fn excerpt(body: &str) -> String {
    body.chars().take(300).collect()
}

// ---------------------------------------------------------------------------
// No account reveals its other workspaces (P0.38, ruling R19)

/// A person in A and B: one project in A, two in B named with B's marker.
const SHARED: &str = "Shared Person";
/// A person in B only, whom nobody in A may find.
const STRANGER: &str = "Stranger Bea";
const STRANGER_HANDLE: &str = "strangerbea";

/// GET routes whose answer holds accounts (an `ActorView`, `Person` or
/// `ProfileView`, directly or inside another view) and that
/// [`people_search_shows_only_shared_workspaces`] walks.
const ACCOUNT_ROUTES_WALKED: &[&str] = &[
    "/api/v1/direct",
    "/api/v1/organizations/{organization_id}/members",
    "/api/v1/projects/{id}/mentionable-actors",
    "/api/v1/projects/{project_id}/agent-profiles",
    "/api/v1/projects/{project_id}/members",
    "/api/v1/projects/{project_id}/work-item-assignees",
];

/// The other account-holding GET routes, and why the walk leaves them out.
const ACCOUNT_ROUTES_EXCUSED: &[(&str, &str)] = &[
    (
        "/api/v1/conversations/{id}/timeline",
        "authors and members of a conversation the caller reads; P0.22's net walks it",
    ),
    (
        "/api/v1/conversations/{id}/messages/{message_id}",
        "a message's author; P0.22's net walks it",
    ),
    (
        "/api/v1/conversations/{id}/messages/{message_id}/revisions",
        "a message's editors; P0.22's net walks it",
    ),
    (
        "/api/v1/projects/{project_id}/work-items/{work_item_id}",
        "a task's people (assignees, watchers); P0.22's net walks it",
    ),
    (
        "/api/v1/projects/{project_id}/work-items/by-key/{key}",
        "the same task, by key; P0.22's net walks it",
    ),
    (
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria",
        "who checked each criterion; P0.22's net walks it",
    ),
    (
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/timeline",
        "a task's history; P0.22's net walks it",
    ),
    (
        "/api/v1/projects/{project_id}/work-items/{work_item_id}/comments/{comment_id}/revisions",
        "a task comment's editors; P0.22's net walks it",
    ),
    (
        "/api/v1/projects/{project_id}/timeline",
        "the people on tasks the caller can see; P0.22's net walks it",
    ),
];

/// What one caller in A must never read of B, and how it calls.
struct AccountsWorld {
    fixture: TwoWorkspaces,
    /// B's marker, B's project names and B's ids, and the stranger's name
    /// and handle.
    forbidden: Vec<String>,
    stranger: Uuid,
    b_agent: Uuid,
    shared: Uuid,
    person: Person,
    agent: Agent,
    app: ConnectedApp,
}

async fn accounts_world(name: &str) -> Option<AccountsWorld> {
    let fixture = two_workspaces(name).await?;
    let (a, b) = (&fixture.a, &fixture.b);
    let setup_for = |world: &World| Setup {
        app: fixture.app.clone(),
        browser: Browser {
            cookie: world.browser.cookie.clone(),
            csrf: world.browser.csrf.clone(),
        },
        organization_id: world.organization,
        pool: fixture.pool.clone(),
        cleanup: None,
    };
    let (setup_a, setup_b) = (setup_for(a), setup_for(b));
    let pool = &fixture.pool;

    // `shared` is in A's project and B's two projects.
    let shared = add_person(
        &setup_a,
        &[&a.project, &a.space],
        "shared-a",
        "shared",
        SHARED,
    )
    .await;
    let mut b_projects = Vec::new();
    for (key, label) in [("SHA", "Harbour"), ("SHB", "Lighthouse")] {
        let project = create_project(&setup_b, key, &format!("{label} {}", b.marker)).await;
        sqlx::query(
            r#"
            INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle)
            VALUES ($1, $2, $3, 'member', $4, 'shared')
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(project.parse::<Uuid>().expect("project"))
        .bind(shared)
        .bind(SHARED)
        .execute(pool)
        .await
        .expect("shared joins B's project");
        b_projects.push(project);
    }
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'member')",
    )
    .bind(b.organization)
    .bind(shared)
    .execute(pool)
    .await
    .expect("shared joins B");
    // The stranger is in B only.
    let stranger = add_person(
        &setup_b,
        &[&b.project, &b.space],
        "stranger-b",
        STRANGER_HANDLE,
        STRANGER,
    )
    .await;
    // Today an agent in both is two agents with one name (A's and B's
    // Mara); K1's R19 slice makes it one account, and these checks stay.
    let agent_actor = |connection: Uuid| async move {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT actor_id FROM external_agent_connections WHERE id = $1",
        )
        .bind(connection)
        .fetch_one(pool)
        .await
        .expect("agent actor")
    };
    let a_agent = agent_actor(a.agent_connection).await;
    let b_agent = agent_actor(b.agent_connection).await;

    // The callers: a person in A only, through the browser and through an
    // app they connected, and A's agent.
    add_person(
        &setup_a,
        &[&a.project, &a.space],
        "member-a",
        "membera",
        "Member A",
    )
    .await;
    let browser = login_as(&fixture.app, "member-a").await;
    let app_token = connect_app(&fixture.app, &browser, "member app", "full").await;
    let app_session = initialize(&fixture.app, &app_token)
        .await
        .expect("member app MCP");
    let agent_session = initialize(&fixture.app, &a.agent_token)
        .await
        .expect("agent MCP");
    let member =
        sqlx::query_scalar::<_, Uuid>("SELECT actor_id FROM users WHERE login_name = 'member-a'")
            .fetch_one(pool)
            .await
            .expect("member");

    let mut forbidden = vec![
        b.marker.to_owned(),
        STRANGER.to_owned(),
        STRANGER_HANDLE.to_owned(),
        b.organization.to_string(),
        b.project.clone(),
        b.space.clone(),
        b.rook.to_string(),
        b_agent.to_string(),
        stranger.to_string(),
    ];
    forbidden.extend(b_projects);
    Some(AccountsWorld {
        person: Person {
            actor: member,
            browser,
        },
        agent: Agent {
            actor: a_agent,
            connection: a.agent_connection,
            token: a.agent_token.clone(),
            session: agent_session,
        },
        app: ConnectedApp {
            person: member,
            token: app_token,
            session: app_session,
        },
        forbidden,
        stranger,
        b_agent,
        shared,
        fixture,
    })
}

/// Fails naming the first forbidden word or id in `body`.
fn assert_nothing_of_b(world: &AccountsWorld, what: &str, body: &Value) {
    let text = body.to_string().to_ascii_lowercase();
    for word in &world.forbidden {
        assert!(
            !text.contains(&word.to_ascii_lowercase()),
            "{what} shows {word:?} from workspace B: {body}"
        );
    }
}

/// The account tools, as an agent or a connected app calls them.
fn account_tools(a: &World, queries: &[&str]) -> Vec<(&'static str, Value)> {
    let mut calls = vec![
        ("people_search", json!({})),
        ("agent_list", json!({})),
        ("agent_list", json!({"projectId": a.project})),
        ("agent_profile_list", json!({"projectId": a.project})),
        ("project_members", json!({"projectId": a.project})),
        ("project_members", json!({"projectId": a.space})),
    ];
    for query in queries {
        calls.push(("people_search", json!({"query": query})));
        calls.push((
            "people_search",
            json!({"query": query, "projectId": a.space}),
        ));
        calls.push(("agent_list", json!({"query": query})));
    }
    calls
}

#[tokio::test]
async fn people_search_shows_only_shared_workspaces() {
    let Some(world) = accounts_world("privacy_accounts").await else {
        return;
    };
    let fixture = &world.fixture;
    let (app, a) = (&fixture.app, &fixture.a);

    // Positive control: the shared person is found, so a clean walk means
    // something.
    let (_, found) = world
        .app
        .call(
            app,
            Call::Tool {
                name: "people_search",
                arguments: json!({"query": "shared"}),
            },
        )
        .await;
    assert!(
        found.to_string().contains(&world.shared.to_string()),
        "the shared person is found: {found}"
    );

    let mut uris = vec![
        "/api/v1/direct".to_owned(),
        format!("/api/v1/organizations/{}/members", a.organization),
        format!("/api/v1/projects/{}/agent-profiles", a.project),
        format!("/api/v1/projects/{}/agents", a.project),
        format!("/api/v1/projects/{}/external-agents", a.project),
        format!("/api/v1/conversations/{}/members", a.channel),
        "/api/v1/account/profile".to_owned(),
    ];
    for project in [&a.project, &a.space] {
        uris.push(format!("/api/v1/projects/{project}/members"));
        for query in ["", "shared", "mara"] {
            uris.push(format!(
                "/api/v1/projects/{project}/mentionable-actors?q={query}"
            ));
            uris.push(format!(
                "/api/v1/projects/{project}/work-item-assignees?q={query}"
            ));
            uris.push(format!(
                "/api/v1/projects/{project}/member-candidates?q={query}"
            ));
        }
    }
    for uri in uris {
        let (status, body) = world
            .person
            .call(
                app,
                Call::Http {
                    method: "GET",
                    path: uri.clone(),
                    body: Value::Null,
                },
            )
            .await;
        assert_nothing_of_b(&world, &format!("GET {uri} ({status})"), &body);
    }
    for caller in [&world.agent as &dyn AccountCall, &world.app] {
        for (name, arguments) in account_tools(a, &["shared", "mara", ""]) {
            let (_, body) = caller.tool(app, name, arguments.clone()).await;
            assert_nothing_of_b(&world, &format!("{name} {arguments}"), &body);
        }
    }
    world.fixture.finish().await;
}

#[tokio::test]
async fn people_search_never_finds_strangers() {
    let Some(world) = accounts_world("privacy_strangers").await else {
        return;
    };
    let fixture = &world.fixture;
    let (app, a) = (&fixture.app, &fixture.a);
    let names = [
        "Stranger",
        "stranger",
        "Bea",
        STRANGER_HANDLE,
        "@strangerbea",
    ];
    let named = |body: &Value| {
        let text = body.to_string().to_ascii_lowercase();
        text.contains("stranger") || text.contains(&world.stranger.to_string())
    };
    for query in names {
        for project in [&a.project, &a.space] {
            for path in [
                format!("/api/v1/projects/{project}/mentionable-actors?q={query}"),
                format!("/api/v1/projects/{project}/work-item-assignees?q={query}"),
                format!("/api/v1/projects/{project}/member-candidates?q={query}"),
            ] {
                let (status, body) = world
                    .person
                    .call(
                        app,
                        Call::Http {
                            method: "GET",
                            path: path.clone(),
                            body: Value::Null,
                        },
                    )
                    .await;
                assert!(
                    !named(&body),
                    "GET {path} ({status}) finds the stranger: {body}"
                );
            }
        }
        for caller in [&world.agent as &dyn AccountCall, &world.app] {
            for (name, arguments) in account_tools(a, &[query]) {
                let (_, body) = caller.tool(app, name, arguments.clone()).await;
                assert!(
                    !named(&body),
                    "{name} {arguments} finds the stranger: {body}"
                );
            }
        }
    }
    // Named by id, B's people and agent are still nobody.
    let ids = format!("{},{}", world.stranger, world.b_agent);
    let (status, body) = world
        .person
        .call(
            app,
            Call::Http {
                method: "GET",
                path: format!(
                    "/api/v1/projects/{}/work-item-assignees?ids={ids}",
                    a.project
                ),
                body: Value::Null,
            },
        )
        .await;
    let text = body.to_string().to_ascii_lowercase();
    assert!(
        !text.contains("stranger") && !text.contains(fixture.b.marker),
        "assignees by id ({status}): {body}"
    );
    world.fixture.finish().await;
}

/// A tool call as the agent or the connected app; `Caller`'s `async fn`
/// isn't object safe, so this boxes the one call the walk needs.
trait AccountCall {
    fn tool<'a>(
        &'a self,
        app: &'a axum::Router,
        name: &'static str,
        arguments: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (axum::http::StatusCode, Value)> + 'a>>;
}

impl AccountCall for Agent {
    fn tool<'a>(
        &'a self,
        app: &'a axum::Router,
        name: &'static str,
        arguments: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (axum::http::StatusCode, Value)> + 'a>>
    {
        Box::pin(self.call(app, Call::Tool { name, arguments }))
    }
}

impl AccountCall for ConnectedApp {
    fn tool<'a>(
        &'a self,
        app: &'a axum::Router,
        name: &'static str,
        arguments: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = (axum::http::StatusCode, Value)> + 'a>>
    {
        Box::pin(self.call(app, Call::Tool { name, arguments }))
    }
}

/// Every GET route whose answer holds accounts is walked above or excused
/// with a reason, so a new one can't slip past R19's rule.
#[test]
fn every_account_route_is_walked_or_excused() {
    let found = account_routes();
    let listed = ACCOUNT_ROUTES_WALKED
        .iter()
        .copied()
        .chain(ACCOUNT_ROUTES_EXCUSED.iter().map(|(route, _)| *route))
        .collect::<BTreeSet<_>>();
    let missing = found
        .iter()
        .filter(|route| !listed.contains(route.as_str()))
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "GET routes that answer with accounts, neither walked by \
         people_search_shows_only_shared_workspaces nor in ACCOUNT_ROUTES_EXCUSED: {missing:?}"
    );
    // The walked ones are found too, so the scan is seen to work.
    let stale = ACCOUNT_ROUTES_WALKED
        .iter()
        .copied()
        .chain(ACCOUNT_ROUTES_EXCUSED.iter().map(|(route, _)| *route))
        .filter(|route| !found.contains(*route))
        .collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "excused routes that no longer answer with accounts: {stale:?}"
    );
}

/// The account views a response can hold.
const ACCOUNT_VIEWS: [&str; 3] = ["ActorView", "Person", "ProfileView"];

/// GET routes whose handler returns a type that holds an account view.
fn account_routes() -> BTreeSet<String> {
    let files = rust_files(&server_src());
    let sources = files
        .iter()
        .map(|(path, source)| (path.as_str(), strip_tests(source)))
        .collect::<BTreeMap<_, _>>();
    // Struct name -> the types of its fields, for every struct.
    let mut fields = Vec::new();
    for source in sources.values() {
        fields.extend(struct_field_types(source));
    }
    let mut holders = ACCOUNT_VIEWS
        .map(str::to_owned)
        .into_iter()
        .collect::<BTreeSet<_>>();
    loop {
        let before = holders.len();
        for (name, types) in &fields {
            if !holders.contains(name) && holders.iter().any(|held| has_word(types, held)) {
                holders.insert(name.clone());
            }
        }
        if holders.len() == before {
            break;
        }
    }
    let mut found = BTreeSet::new();
    for (path, source) in &sources {
        for (route, handler) in get_handlers(source) {
            let Some(returns) = handler_return(&sources, path, &handler) else {
                continue;
            };
            if holders.iter().any(|held| has_word(&returns, held)) {
                found.insert(route);
            }
        }
    }
    found
}

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Whether `word` is in `text` with no identifier character either side.
fn has_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].bytes().next_back().is_some_and(is_ident);
        let after = text[at + word.len()..].bytes().next().is_some_and(is_ident);
        !before && !after
    })
}

/// The text inside the bracket that opens at `open`, and the index past it.
fn balanced(source: &str, open: usize) -> Option<(&str, usize)> {
    let bytes = source.as_bytes();
    let (left, right) = match bytes.get(open)? {
        b'(' => (b'(', b')'),
        b'{' => (b'{', b'}'),
        _ => return None,
    };
    let mut depth = 0;
    for (index, byte) in bytes.iter().enumerate().skip(open) {
        if *byte == left {
            depth += 1;
        } else if *byte == right {
            depth -= 1;
            if depth == 0 {
                return Some((&source[open + 1..index], index + 1));
            }
        }
    }
    None
}

fn struct_field_types(source: &str) -> Vec<(String, String)> {
    let mut structs = Vec::new();
    for (at, _) in source.match_indices("struct ") {
        if source[..at].bytes().next_back().is_some_and(is_ident) {
            continue;
        }
        let rest = &source[at + "struct ".len()..];
        let name = rest.bytes().take_while(|byte| is_ident(*byte)).count();
        let Some(open) = rest[name..].find(['{', ';', '(']) else {
            continue;
        };
        if rest.as_bytes()[name + open] != b'{' {
            continue;
        }
        let Some((body, _)) = balanced(rest, name + open) else {
            continue;
        };
        let types = body
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with("//") && !line.starts_with('#'))
            .filter_map(|line| line.split_once(':').map(|(_, kind)| kind))
            .collect::<Vec<_>>()
            .join(" ");
        structs.push((rest[..name].to_owned(), types));
    }
    structs
}

/// `(path, handler)` for every GET a route registration in `source` names.
fn get_handlers(source: &str) -> Vec<(String, String)> {
    let call = concat!(".route", "(");
    let mut found = Vec::new();
    for (at, _) in source.match_indices(call) {
        let Some((arguments, _)) = balanced(source, at + call.len() - 1) else {
            continue;
        };
        let Some(path) = arguments
            .split('"')
            .nth(1)
            .filter(|path| path.starts_with('/'))
        else {
            continue;
        };
        for (get, _) in arguments.match_indices("get(") {
            if arguments[..get].bytes().next_back().is_some_and(is_ident) {
                continue;
            }
            let handler = arguments[get + "get(".len()..]
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == ':')
                .collect::<String>();
            if !handler.is_empty() {
                found.push((path.to_owned(), handler));
            }
        }
    }
    found
}

/// The return type of `handler`, named from the file at `path`.
fn handler_return(sources: &BTreeMap<&str, &str>, path: &str, handler: &str) -> Option<String> {
    let mut segments = handler.split("::").collect::<Vec<_>>();
    let name = segments.pop()?;
    let candidates = if segments.is_empty() {
        vec![path.to_owned()]
    } else {
        let crate_root = segments.first() == Some(&"crate");
        let modules = segments
            .iter()
            .filter(|segment| **segment != "crate")
            .copied()
            .collect::<Vec<_>>()
            .join("/");
        let stem = path.strip_suffix(".rs").unwrap_or(path);
        let stem = stem.strip_suffix("/mod").unwrap_or(stem);
        let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
        let mut bases = vec![String::new()];
        if !crate_root {
            bases.insert(0, format!("{stem}/"));
            if !parent.is_empty() {
                bases.insert(1, format!("{parent}/"));
            }
        }
        bases
            .into_iter()
            .flat_map(|base| {
                [
                    format!("{base}{modules}.rs"),
                    format!("{base}{modules}/mod.rs"),
                ]
            })
            .collect()
    };
    candidates.iter().find_map(|candidate| {
        let source = sources.get(candidate.as_str())?;
        let at = source
            .match_indices(&format!("fn {name}"))
            .map(|(at, _)| at)
            .find(|at| {
                source[at + 3 + name.len()..]
                    .trim_start()
                    .starts_with(['(', '<'])
            })?;
        let open = at + source[at..].find('(')?;
        let (_, after) = balanced(source, open)?;
        let body = after + source[after..].find('{')?;
        Some(source[after..body].to_owned())
    })
}

/// K1.4: holding `agents.manage` doesn't make an agent yours. A member
/// can't name managers at all; an admin who isn't named can't name
/// themselves; an owner can (`agents.manage_any`).
#[tokio::test]
async fn naming_an_agents_managers_follows_the_one_rule() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("ada", "admin")
        .agent("rook", "member")
        .build("privacy_agent_managers")
        .await
    else {
        return;
    };
    let route = format!("/api/v2/w/{}/ops/agent_managers_set", world.organization);
    let rook = world.agents["rook"].actor;
    let attempt = |person: &str, key: &str, manager: Uuid| {
        let browser = if person == "owner" {
            &world.owner.browser
        } else {
            &world.people[person].browser
        };
        browser.request(
            "POST",
            &route,
            Some(key),
            json!({ "agent": rook, "add": [manager] }),
        )
    };
    let lena = world.people["lena"].actor;
    let ada = world.people["ada"].actor;
    // Ada is a workspace admin: she holds `agents.manage` there.
    sqlx::query(
        "UPDATE organization_memberships SET role = 'admin', role_explicit = true \
         WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(world.organization)
    .bind(ada)
    .execute(world.pool())
    .await
    .expect("ada is a workspace admin");
    sqlx::query("DELETE FROM account_managers WHERE account_id = $1 AND manager_account_id = $2")
        .bind(rook)
        .bind(ada)
        .execute(world.pool())
        .await
        .expect("ada isn't named");
    let (status, body) = send(&world.app, attempt("lena", "lena-1", lena)).await;
    assert_ne!(
        body["outcome"], "ok",
        "a member names herself: {status} {body}"
    );
    let (status, body) = send(&world.app, attempt("ada", "ada-1", ada)).await;
    assert_ne!(
        body["outcome"], "ok",
        "an un-named admin names herself: {status} {body}"
    );
    let named = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM account_managers WHERE account_id = $1 AND manager_account_id = ANY($2)",
    )
    .bind(rook)
    .bind(vec![lena, ada])
    .fetch_one(world.pool())
    .await
    .expect("count");
    assert_eq!(named, 0);
    let (status, body) = send(&world.app, attempt("owner", "owner-1", ada)).await;
    assert_eq!(body["outcome"], "ok", "{status} {body}");
    // Ada hears of it in the same transaction, and nobody else does.
    let events = sqlx::query_scalar::<_, Option<Uuid>>(&format!(
        "SELECT (payload->>'actorId')::uuid FROM {n} notice \
         WHERE event_type = 'account.managers.changed'",
        n = crate::support::NOTICES
    ))
    .fetch_all(world.pool())
    .await
    .expect("events");
    assert_eq!(events, vec![Some(ada)]);

    // A2.10: a member who doesn't manage rook can't pause or resume it.
    // A2.18: nor add it to a workspace (she manages no agent and may not
    // invite).
    for (name, key) in [
        ("agent_pause", "lena-pause"),
        ("agent_resume", "lena-resume"),
        ("account_workspace_add", "lena-add"),
    ] {
        let (status, body) = send(
            &world.app,
            world.people["lena"].browser.request(
                "POST",
                &format!("/api/v2/w/{}/ops/{name}", world.organization),
                Some(key),
                json!({ "account": rook }),
            ),
        )
        .await;
        assert_ne!(body["outcome"], "ok", "lena runs {name}: {status} {body}");
    }
    // A2.19: nor run a host's action for it, or approve an agent a host
    // proposed (she can't add agents). agent_register.rs covers both with a
    // real host.
    for (name, input) in [
        (
            "agent_action_run",
            json!({"account": rook, "actionId": "pause"}),
        ),
        (
            "agent_offer_start",
            json!({"host": Uuid::now_v7(), "offerId": "new"}),
        ),
        (
            "agent_register_decide",
            json!({"proposal": Uuid::now_v7(), "decision": "approve"}),
        ),
    ] {
        let (status, body) = send(
            &world.app,
            world.people["lena"].browser.request(
                "POST",
                &format!("/api/v2/w/{}/ops/{name}", world.organization),
                Some(&format!("lena-{name}")),
                input,
            ),
        )
        .await;
        assert_ne!(body["outcome"], "ok", "lena runs {name}: {status} {body}");
    }
    let paused = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM account_statuses WHERE account_id = $1 AND paused_at IS NOT NULL",
    )
    .bind(rook)
    .fetch_one(world.pool())
    .await
    .expect("pauses");
    assert_eq!(paused, 0);
    world.finish().await;
}

/// Comment writes (C1.3) keep to what the writer may see inside the
/// workspace: a member who isn't in a DM can't quote its messages in a
/// comment, and a role without `tasks.view` can't comment on a task or
/// settle a discussion on one, though a sheet it may edit takes its
/// comments.
#[tokio::test]
async fn comment_writes_keep_to_what_the_writer_may_see() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .person("omar", "member")
        .person("nate", "member")
        .connected_app("Phone")
        .custom_role(
            "No tasks",
            &[
                "projects.view",
                "sheets.edit",
                "channels.post",
                "directory.view",
            ],
        )
        .dm("lena", "mara")
        .build("privacy_comment_writes")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let workspace = world.organization;
    let work = world.project("WORK").to_owned();
    let phone = &world.apps["Phone"];
    let op = |browser: &Browser, name: &'static str, input: Value| {
        let request = browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/{name}"),
            Some(&Uuid::now_v7().to_string()),
            input,
        );
        async move { send(app, request).await }
    };
    let setup = Setup {
        app: app.clone(),
        browser: Browser {
            cookie: world.owner.browser.cookie.clone(),
            csrf: world.owner.browser.csrf.clone(),
        },
        organization_id: workspace,
        pool: pool.clone(),
        cleanup: None,
    };
    // Nate holds the narrowed role in WORK.
    sqlx::query(
        "UPDATE project_memberships SET role_id = $1 WHERE project_id = $2 AND actor_id = $3",
    )
    .bind(Uuid::parse_str(&world.roles["No tasks"]).expect("role id"))
    .bind(Uuid::parse_str(&work).expect("work"))
    .bind(world.people["nate"].actor)
    .execute(pool)
    .await
    .expect("narrow nate");

    let dm_message = say(
        &setup,
        &world.people["lena"].browser,
        &world.conversations["lena+mara"],
        "private-comment-quote",
        text(&format!("The {PRIVATE} plan")),
    )
    .await;
    let sheet = super::support::app::tool(
        app,
        &phone.token,
        &phone.session,
        "sheet_write",
        json!({"projectId": work, "name": "Budget", "cells": [{"cell": "A1", "input": "Rent"}],
               "clientRequestId": "privacy-sheet"}),
    )
    .await;
    let sheet = string_at(&sheet, "/value/id");
    let task = super::support::app::tool(
        app,
        &phone.token,
        &phone.session,
        "task_create",
        json!({"projectId": "WORK", "title": format!("The {TASK} task"), "clientRequestId": "privacy-task"}),
    )
    .await;
    let task = string_at(&task, "/value/workItemId");
    let (status, thread) = op(
        &world.owner.browser,
        "comment_create",
        json!({"thing": {"kind": "record", "id": task}, "text": "On the title",
               "anchor": {"kind": "field", "field": "title"}}),
    )
    .await;
    assert!(
        status.is_success(),
        "owner comments on the task: {status} {thread}"
    );
    let thread = string_at(&thread, "/value/threadId");

    // Omar isn't in the DM: quoting its message in a comment is refused and
    // shows nothing of it.
    let (status, quoted) = op(
        &world.people["omar"].browser,
        "comment_create",
        json!({"thing": {"kind": "sheet", "id": sheet}, "text": "As said", "replyToMessageId": dm_message}),
    )
    .await;
    assert!(
        !status.is_success(),
        "omar quotes the DM: {status} {quoted}"
    );
    assert!(!quoted.to_string().contains(PRIVATE), "{quoted}");

    // Nate's role can't see tasks: no comment on the task, no reading or
    // settling its discussion, nothing of it in the refusal; the sheet takes his comment.
    for (name, input) in [
        (
            "comment_create",
            json!({"thing": {"kind": "record", "id": task}, "text": "Me too"}),
        ),
        (
            "comment_list",
            json!({"thing": {"kind": "record", "id": task}}),
        ),
        (
            "comment_thread_update",
            json!({"threadId": thread, "status": "Resolved"}),
        ),
    ] {
        let (status, refused) = op(&world.people["nate"].browser, name, input).await;
        assert!(!status.is_success(), "nate {name}: {status} {refused}");
        assert!(!refused.to_string().contains(TASK), "{refused}");
    }
    let (status, commented) = op(
        &world.people["nate"].browser,
        "comment_create",
        json!({"thing": {"kind": "sheet", "id": sheet}, "text": "Rent is fine"}),
    )
    .await;
    assert!(
        status.is_success(),
        "nate on the sheet: {status} {commented}"
    );
    let status_now = sqlx::query_scalar::<_, String>(
        "SELECT thread_status FROM conversations WHERE id = $1::uuid",
    )
    .bind(&thread)
    .fetch_one(pool)
    .await
    .expect("thread status");
    assert_ne!(status_now.to_lowercase(), "resolved");
    world.finish().await;
}

/// The chat writes that keep per-account state (C2.13, C2.15, C2.19, C2.28,
/// C2.2) keep to what the writer may see inside the workspace: a member who
/// isn't in a DM can't keep a draft there, mark it read or hear its words by
/// keyword; nobody sets another account's status; and a role that can't
/// manage settings can't retire someone else's emoji.
#[tokio::test]
async fn chat_writes_keep_to_what_the_writer_may_see() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .person("omar", "member")
        .person("nate", "member")
        .custom_role(
            "Chat only",
            &["projects.view", "channels.post", "directory.view"],
        )
        .dm("lena", "mara")
        .build("privacy_chat_writes")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let workspace = world.organization;
    let work = world.project("WORK").to_owned();
    let dm = world.conversations["lena+mara"].clone();
    let op = |browser: &Browser, name: &'static str, input: Value| {
        let request = browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/{name}"),
            Some(&Uuid::now_v7().to_string()),
            input,
        );
        async move { send(app, request).await }
    };
    let setup = Setup {
        app: app.clone(),
        browser: Browser {
            cookie: world.owner.browser.cookie.clone(),
            csrf: world.owner.browser.csrf.clone(),
        },
        organization_id: workspace,
        pool: pool.clone(),
        cleanup: None,
    };
    sqlx::query(
        "UPDATE project_memberships SET role_id = $1 WHERE project_id = $2 AND actor_id = $3",
    )
    .bind(Uuid::parse_str(&world.roles["Chat only"]).expect("role id"))
    .bind(Uuid::parse_str(&work).expect("work"))
    .bind(world.people["nate"].actor)
    .execute(pool)
    .await
    .expect("narrow nate");
    let omar = &world.people["omar"];

    // Omar listens for a word, then Lena says it in her DM with Mara.
    let (status, set) = op(
        &omar.browser,
        "notification_keyword_set",
        json!({"keywords": ["zephyr"]}),
    )
    .await;
    assert!(status.is_success(), "{set}");
    say(
        &setup,
        &world.people["lena"].browser,
        &dm,
        "private-keyword",
        text(&format!("The zephyr {PRIVATE} plan")),
    )
    .await;
    drain_outbox(&world.state).await;
    let told = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM notifications WHERE recipient_actor_id = $1 AND kind = 'keyword'",
    )
    .bind(omar.actor)
    .fetch_one(pool)
    .await
    .expect("keyword notifications");
    assert_eq!(told, 0, "omar isn't in the DM");

    // No draft kept in it, and nothing of it in the refusal or his list.
    let (status, refused) = op(
        &omar.browser,
        "message_draft_set",
        json!({"conversationId": dm, "draft": {"text": "Peek"}}),
    )
    .await;
    assert!(
        !status.is_success(),
        "omar keeps a draft in the DM: {refused}"
    );
    assert!(!refused.to_string().contains(PRIVATE), "{refused}");
    let (_, listed) = op(&omar.browser, "message_draft_list", json!({})).await;
    assert_eq!(listed["value"]["drafts"], json!([]), "{listed}");

    // Mark all read reads only what he can read: no read mark in the DM.
    let (status, marked) = op(&omar.browser, "conversations_mark_read", json!({})).await;
    assert!(status.is_success(), "{marked}");
    let marks = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM conversation_read_states \
         WHERE actor_id = $1 AND conversation_id = $2::uuid",
    )
    .bind(omar.actor)
    .bind(&dm)
    .fetch_one(pool)
    .await
    .expect("read marks");
    assert_eq!(marks, 0, "omar marked the DM read");

    // Nobody sets a person's status for them.
    let (status, refused) = op(
        &omar.browser,
        "status_set",
        json!({"text": "Out", "account": world.people["lena"].actor}),
    )
    .await;
    assert!(!status.is_success(), "omar sets lena's status: {refused}");
    let lenas =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM account_statuses WHERE account_id = $1")
            .bind(world.people["lena"].actor)
            .fetch_one(pool)
            .await
            .expect("statuses");
    assert_eq!(lenas, 0);

    // Nor their profile; and a profile shows no space the reader can't see.
    let (status, refused) = op(
        &omar.browser,
        "profile_update",
        json!({"title": "Intern", "account": world.people["lena"].actor}),
    )
    .await;
    assert!(!status.is_success(), "omar sets lena's title: {refused}");
    let profiles =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM account_profiles WHERE account_id = $1")
            .bind(world.people["lena"].actor)
            .fetch_one(pool)
            .await
            .expect("profiles");
    assert_eq!(profiles, 0);
    let (status, read) = op(
        &omar.browser,
        "profile_get",
        json!({"account": world.people["lena"].actor}),
    )
    .await;
    assert!(status.is_success(), "{read}");
    assert!(!read.to_string().contains(PRIVATE), "{read}");

    // A role that can't manage settings can't retire the owner's emoji.
    let emoji = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO custom_emojis (id, workspace_id, name, content_type, image, created_by_account_id) \
         VALUES ($1, $2, 'owners_emoji', 'image/png', $3, $4)",
    )
    .bind(emoji)
    .bind(workspace)
    .bind(b"\x89PNG\r\n\x1a\n".to_vec())
    .bind(world.owner.actor)
    .execute(pool)
    .await
    .expect("emoji");
    let (status, refused) = op(
        &world.people["nate"].browser,
        "custom_emoji_update",
        json!({"id": emoji, "retired": true}),
    )
    .await;
    assert!(!status.is_success(), "nate retires it: {refused}");
    let retired = sqlx::query_scalar::<_, bool>(
        "SELECT retired_at IS NOT NULL FROM custom_emojis WHERE id = $1",
    )
    .bind(emoji)
    .fetch_one(pool)
    .await
    .expect("emoji row");
    assert!(!retired);
    world.finish().await;
}

/// K4.26: an invitation link admits up to its number of people with its
/// role, and the space it names; a revoked or expired link gets the same
/// answer as an unknown one; someone who may invite but holds less than
/// admin can't make a link that gives admin; the token is in the answer
/// that made the link and nowhere else.
#[tokio::test]
async fn invitation_links_admit_up_to_their_uses_within_their_makers_role() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .build("privacy_invite_tokens")
        .await
    else {
        return;
    };
    let pool = world.pool();
    let route = |op: &str| format!("/api/v2/w/{}/ops/{op}", world.organization);
    let role = |key: &'static str| {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM workspace_roles WHERE organization_id = $1 AND built_in AND key = $2",
        )
        .bind(world.organization)
        .bind(key)
        .fetch_one(pool)
    };
    let member_role = role("member").await.expect("member role");
    let admin_role = role("admin").await.expect("admin role");
    let space = Uuid::parse_str(&world.projects["WORK"]).expect("space id");

    let (status, made) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &route("invite_link_create"),
            Some("link-1"),
            json!({ "roleId": member_role, "space": space, "maxUses": 2 }),
        ),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{status} {made}");
    let token = made["value"]["token"].as_str().expect("token").to_owned();
    let link = made["value"]["link"]["id"].as_str().expect("id").to_owned();
    assert!(
        made["value"]["url"]
            .as_str()
            .expect("url")
            .ends_with(&token)
    );

    // A retry finds the link without its token.
    let (_, again) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &route("invite_link_create"),
            Some("link-1"),
            json!({ "roleId": member_role, "space": space, "maxUses": 2 }),
        ),
    )
    .await;
    assert_eq!(again["value"]["link"]["id"], link.as_str(), "{again}");
    assert!(again["value"].get("token").is_none(), "{again}");
    let (_, listed) = send(
        &world.app,
        world
            .owner
            .browser
            .request("POST", &route("invite_link_list"), None, json!({})),
    )
    .await;
    assert_eq!(listed["value"]["links"][0]["id"], link.as_str(), "{listed}");
    for answer in [&again, &listed] {
        assert!(!answer.to_string().contains(&token[7..]), "{answer}");
    }

    let anonymous = Browser {
        cookie: String::new(),
        csrf: String::new(),
    };
    let join = |token: &str, who: &str| {
        anonymous.request(
            "POST",
            "/api/v1/invite-links/accept-new",
            None,
            json!({
                "token": token,
                "loginName": who,
                "displayName": who,
                "handle": who,
                "password": "a long enough password",
            }),
        )
    };
    let (status, resolved) = send(
        &world.app,
        anonymous.request(
            "POST",
            "/api/v1/invite-links/resolve",
            None,
            json!({ "token": token }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(resolved["role"], "member");
    assert!(resolved["projectName"].is_string(), "{resolved}");
    for who in ["nia", "omar"] {
        let (status, body) = send(&world.app, join(&token, who)).await;
        assert_eq!(status, StatusCode::OK, "{who}: {body}");
    }
    let (status, _) = send(&world.app, join(&token, "pia")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a link admits two people");
    let admitted = sqlx::query_as::<_, (String, bool, bool)>(
        r#"
        SELECT users.login_name, membership.enabled,
               EXISTS (SELECT 1 FROM project_memberships space
                       WHERE space.project_id = $2 AND space.actor_id = users.actor_id
                         AND space.enabled)
        FROM users
        JOIN organization_memberships membership
          ON membership.actor_id = users.actor_id AND membership.organization_id = $1
        WHERE users.login_name IN ('nia', 'omar', 'pia')
        ORDER BY users.login_name
        "#,
    )
    .bind(world.organization)
    .bind(space)
    .fetch_all(pool)
    .await
    .expect("admitted");
    assert_eq!(
        admitted,
        vec![
            ("nia".to_owned(), true, true),
            ("omar".to_owned(), true, true)
        ]
    );

    // Revoked and expired links answer like unknown ones.
    let make = |key: &'static str, body: Value| {
        world
            .owner
            .browser
            .request("POST", &route("invite_link_create"), Some(key), body)
    };
    let (_, revoked) = send(&world.app, make("link-2", json!({}))).await;
    let revoked_token = revoked["value"]["token"]
        .as_str()
        .expect("token")
        .to_owned();
    let (_, done) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &route("invite_link_revoke"),
            Some("revoke-2"),
            json!({ "link": revoked["value"]["link"]["id"] }),
        ),
    )
    .await;
    assert_eq!(done["value"]["active"], false, "{done}");
    let (_, expired) = send(&world.app, make("link-3", json!({}))).await;
    let expired_token = expired["value"]["token"]
        .as_str()
        .expect("token")
        .to_owned();
    sqlx::query(
        "UPDATE invite_tokens SET created_at = now() - interval '2 days', \
         expires_at = now() - interval '1 day' WHERE id = $1",
    )
    .bind(Uuid::parse_str(expired["value"]["link"]["id"].as_str().expect("id")).expect("uuid"))
    .execute(pool)
    .await
    .expect("expire");
    let mut unknown = revoked_token.clone();
    unknown.replace_range(20..24, "AAAA");
    let mut answers = Vec::new();
    for token in [&revoked_token, &expired_token, &unknown] {
        answers.push(send(&world.app, join(token, "quinn")).await);
    }
    assert!(
        answers.windows(2).all(|pair| pair[0] == pair[1]),
        "{answers:?}"
    );
    assert_eq!(answers[0].0, StatusCode::NOT_FOUND);

    // Lena may invite with a role below admin: she can't give admin.
    let inviter = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO workspace_roles (id, organization_id, key, base_role, name, permissions)
        VALUES (gen_random_uuid(), $1, 'inviter', 'admin', 'Inviter',
                ARRAY['projects.view', 'members.invite'])
        RETURNING id
        "#,
    )
    .bind(world.organization)
    .fetch_one(pool)
    .await
    .expect("inviter role");
    sqlx::query(
        "UPDATE organization_memberships SET role_id = $3, role_explicit = true \
         WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(world.organization)
    .bind(world.people["lena"].actor)
    .bind(inviter)
    .execute(pool)
    .await
    .expect("lena invites");
    let lena = |key: &'static str, body: Value| {
        world.people["lena"]
            .browser
            .request("POST", &route("invite_link_create"), Some(key), body)
    };
    let (status, refused) = send(
        &world.app,
        lena("lena-admin", json!({ "roleId": admin_role })),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{status} {refused}");
    let (status, refused) = send(
        &world.app,
        lena("lena-member", json!({ "roleId": member_role })),
    )
    .await;
    assert_ne!(
        refused["outcome"], "ok",
        "a member's role holds more than Inviter: {status} {refused}"
    );
    // Inviter is based on Admin: only an owner lets someone in with it,
    // even though it is her own role.
    let (status, own) = send(&world.app, lena("lena-own", json!({ "roleId": inviter }))).await;
    assert_ne!(
        own["outcome"], "ok",
        "an Admin-tier role comes from an owner: {status} {own}"
    );

    // The token is in no row.
    for (table, column) in [
        ("invite_tokens", "invite_tokens::text"),
        ("audit_events", "audit_events::text"),
        ("idempotency_keys", "idempotency_keys::text"),
    ] {
        let leaked = sqlx::query_scalar::<_, i64>(&format!(
            "SELECT count(*) FROM {table} WHERE {column} LIKE '%' || $1 || '%'"
        ))
        .bind(&token[16..])
        .fetch_one(pool)
        .await
        .expect("leak check");
        assert_eq!(leaked, 0, "{table} holds the token");
    }
    world.finish().await;
}

/// Views (T1.6) keep to their owner: someone else's personal view is not
/// listed to you and can't be renamed, deleted or pinned by you, and a
/// role that can't build collections can neither share a view nor set a
/// role's default one.
#[tokio::test]
async fn personal_views_stay_their_owners() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .person("nate", "member")
        .custom_role("Tasks only", &["projects.view", "tasks.view", "tasks.edit"])
        .build("privacy_personal_views")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let workspace = world.organization;
    let work = Uuid::parse_str(world.project("WORK")).expect("work");
    let op = |browser: &Browser, name: &'static str, input: Value| {
        let request = browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/{name}"),
            Some(&Uuid::now_v7().to_string()),
            input,
        );
        async move { send(app, request).await }
    };
    sqlx::query(
        "UPDATE project_memberships SET role_id = $1 WHERE project_id = $2 AND actor_id = $3",
    )
    .bind(Uuid::parse_str(&world.roles["Tasks only"]).expect("role id"))
    .bind(work)
    .bind(world.people["nate"].actor)
    .execute(pool)
    .await
    .expect("narrow nate");
    let tasks = sqlx::query_scalar::<_, Uuid>("SELECT ensure_tasks_collection($1, $2)")
        .bind(work)
        .bind(world.owner.actor)
        .fetch_one(pool)
        .await
        .expect("Tasks collection");

    let (status, made) = op(
        &world.people["lena"].browser,
        "view_create",
        json!({"collection": tasks, "name": "Lena's own", "type": "table"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "lena makes a view: {status} {made}");
    let mine = made["value"]["id"].as_str().expect("view id").to_owned();

    let (_, listed) = op(
        &world.people["mara"].browser,
        "view_list",
        json!({"collection": tasks}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert!(
        !listed.to_string().contains(&mine),
        "mara sees lena's view: {listed}"
    );
    for (name, input) in [
        ("view_update", json!({"view": mine, "name": "Mara's now"})),
        ("view_pin", json!({"view": mine, "pinned": true})),
        ("view_delete", json!({"view": mine})),
    ] {
        let (status, body) = op(&world.people["mara"].browser, name, input).await;
        assert_ne!(body["outcome"], "ok", "mara {name}: {status} {body}");
    }
    let name = sqlx::query_scalar::<_, String>("SELECT name FROM collection_views WHERE id = $1")
        .bind(Uuid::parse_str(&mine).expect("id"))
        .fetch_one(pool)
        .await
        .expect("lena's view");
    assert_eq!(name, "Lena's own");

    let nate = &world.people["nate"].browser;
    let (status, body) = op(
        nate,
        "view_create",
        json!({"collection": tasks, "name": "Nate's shared", "type": "table", "shared": true}),
    )
    .await;
    assert_ne!(body["outcome"], "ok", "nate shares a view: {status} {body}");
    let all_tasks = listed["value"]["views"][0]["id"].clone();
    let (status, body) = op(
        nate,
        "view_default_set",
        json!({"collection": tasks, "view": all_tasks, "role": world.roles["Tasks only"]}),
    )
    .await;
    assert_ne!(
        body["outcome"], "ok",
        "nate sets a role default: {status} {body}"
    );
    world.finish().await;
}

/// Sheet filter views (C1.16) keep to their owner: someone else's personal
/// view of a sheet is not listed to you and can't be renamed or removed by
/// you, and a role that can't edit sheets can keep a view of its own but
/// can't share one or name a range.
#[tokio::test]
async fn personal_sheet_views_stay_their_owners() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .person("nate", "member")
        .custom_role("Sheets reader", &["projects.view", "sheets.comment"])
        .build("privacy_sheet_views")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let workspace = world.organization;
    let work = Uuid::parse_str(world.project("WORK")).expect("work");
    let op = |browser: &Browser, name: &'static str, input: Value| {
        let request = browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/{name}"),
            Some(&Uuid::now_v7().to_string()),
            input,
        );
        async move { send(app, request).await }
    };
    sqlx::query(
        "UPDATE project_memberships SET role_id = $1 WHERE project_id = $2 AND actor_id = $3",
    )
    .bind(Uuid::parse_str(&world.roles["Sheets reader"]).expect("role id"))
    .bind(work)
    .bind(world.people["nate"].actor)
    .execute(pool)
    .await
    .expect("narrow nate");
    let (status, made) = send(
        app,
        world.people["lena"].browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/sheets"),
            Some(&Uuid::now_v7().to_string()),
            json!({"name": "Budget"}),
        ),
    )
    .await;
    let sheet = made
        .pointer("/sheet/id")
        .or_else(|| made.pointer("/id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("lena makes a sheet: {status} {made}"))
        .to_owned();

    let (status, made) = op(
        &world.people["lena"].browser,
        "sheet_view_create",
        json!({"sheetId": sheet, "name": "Lena's own",
               "filters": [{"column": "A", "op": "notEmpty"}]}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "lena makes a view: {status} {made}");
    let mine = made["value"]["id"].as_str().expect("view id").to_owned();

    let (_, listed) = op(
        &world.people["mara"].browser,
        "sheet_view_list",
        json!({"sheetId": sheet}),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert!(
        !listed.to_string().contains(&mine),
        "mara sees lena's view: {listed}"
    );
    for (name, input) in [
        (
            "sheet_view_update",
            json!({"viewId": mine, "name": "Mara's now"}),
        ),
        ("sheet_view_delete", json!({"viewId": mine})),
    ] {
        let (status, body) = op(&world.people["mara"].browser, name, input).await;
        assert_ne!(body["outcome"], "ok", "mara {name}: {status} {body}");
    }
    let name = sqlx::query_scalar::<_, String>("SELECT name FROM sheet_views WHERE id = $1")
        .bind(Uuid::parse_str(&mine).expect("id"))
        .fetch_one(pool)
        .await
        .expect("lena's view");
    assert_eq!(name, "Lena's own");

    let nate = &world.people["nate"].browser;
    let (status, body) = op(
        nate,
        "sheet_view_create",
        json!({"sheetId": sheet, "name": "Nate's own"}),
    )
    .await;
    assert_eq!(body["outcome"], "ok", "nate keeps a view: {status} {body}");
    let (status, body) = op(
        nate,
        "sheet_view_create",
        json!({"sheetId": sheet, "name": "Nate's shared", "shared": true}),
    )
    .await;
    assert_ne!(body["outcome"], "ok", "nate shares a view: {status} {body}");
    // Naming a range (C1.20) changes the sheet for everyone: sheets.edit.
    let (status, body) = op(
        nate,
        "sheet_range_set",
        json!({"sheetId": sheet, "name": "Nates", "range": "A1:B2"}),
    )
    .await;
    assert_ne!(body["outcome"], "ok", "nate names a range: {status} {body}");
    let (status, body) = op(
        &world.people["lena"].browser,
        "sheet_range_set",
        json!({"sheetId": sheet, "name": "Lenas", "range": "A1:B2"}),
    )
    .await;
    assert_eq!(body["outcome"], "ok", "lena names a range: {status} {body}");
    world.finish().await;
}

/// Dashboards (T1.32) keep to their owner and read as the viewer: someone
/// else's personal dashboard is not listed to you and can't be read,
/// changed or removed by you; a role that can't build collections can't
/// make a shared one; and a shared dashboard's tile over tasks shows a role
/// that can't see tasks a refusal, not the figures, while its other tiles
/// still show.
#[tokio::test]
async fn dashboards_stay_their_owners_and_read_as_the_viewer() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .person("nate", "member")
        .custom_role(
            "Tasks only",
            &[
                "projects.view",
                "tasks.view",
                "tasks.edit",
                "dashboards.view",
            ],
        )
        .custom_role("No tasks", &["projects.view", "dashboards.view"])
        .person("olive", "member")
        .build("privacy_dashboards")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let workspace = world.organization;
    let work = Uuid::parse_str(world.project("WORK")).expect("work");
    let op = |browser: &Browser, name: &'static str, input: Value| {
        let request = browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/{name}"),
            Some(&Uuid::now_v7().to_string()),
            input,
        );
        async move { send(app, request).await }
    };
    for (person, role) in [("nate", "Tasks only"), ("olive", "No tasks")] {
        sqlx::query(
            "UPDATE project_memberships SET role_id = $1 WHERE project_id = $2 AND actor_id = $3",
        )
        .bind(Uuid::parse_str(&world.roles[role]).expect("role id"))
        .bind(work)
        .bind(world.people[person].actor)
        .execute(pool)
        .await
        .expect("narrow");
    }
    let tasks = sqlx::query_scalar::<_, Uuid>("SELECT ensure_tasks_collection($1, $2)")
        .bind(work)
        .bind(world.owner.actor)
        .fetch_one(pool)
        .await
        .expect("Tasks collection");
    let shared_view = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM collection_views WHERE collection_id = $1 AND owner_actor_id IS NULL \
         ORDER BY position LIMIT 1",
    )
    .bind(tasks)
    .fetch_one(pool)
    .await
    .expect("a shared task view");

    let lena = &world.people["lena"].browser;
    let (_, view) = op(
        lena,
        "view_create",
        json!({"collection": tasks, "name": "Lena's chart", "type": "chart"}),
    )
    .await;
    assert_eq!(view["outcome"], "ok", "{view}");
    let lenas_view = view["value"]["id"].clone();
    // A personal view goes on a personal dashboard, not a shared one.
    let (_, refused) = op(
        lena,
        "dashboard_create",
        json!({"spaceId": work, "name": "Ours", "tiles": [{"view": lenas_view}]}),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    let (_, made) = op(
        lena,
        "dashboard_create",
        json!({"spaceId": work, "name": "Lena's own", "shared": false, "tiles": [{"view": lenas_view}]}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let mine = made["value"]["id"]
        .as_str()
        .expect("dashboard id")
        .to_owned();

    let mara = &world.people["mara"].browser;
    let (_, listed) = op(mara, "dashboard_list", json!({"spaceId": work})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert!(
        !listed.to_string().contains(&mine),
        "mara sees lena's dashboard: {listed}"
    );
    for (name, input) in [
        ("dashboard_get", json!({"dashboard": mine})),
        (
            "dashboard_update",
            json!({"dashboard": mine, "name": "Mara's now"}),
        ),
        ("dashboard_delete", json!({"dashboard": mine})),
    ] {
        let (status, body) = op(mara, name, input).await;
        assert_ne!(body["outcome"], "ok", "mara {name}: {status} {body}");
    }
    let name = sqlx::query_scalar::<_, String>("SELECT name FROM dashboards WHERE id = $1")
        .bind(Uuid::parse_str(&mine).expect("id"))
        .fetch_one(pool)
        .await
        .expect("lena's dashboard");
    assert_eq!(name, "Lena's own");

    let (status, body) = op(
        &world.people["nate"].browser,
        "dashboard_create",
        json!({"spaceId": work, "name": "Nate's shared"}),
    )
    .await;
    assert_ne!(
        body["outcome"], "ok",
        "nate shares a dashboard: {status} {body}"
    );

    let (_, shared) = op(
        lena,
        "dashboard_create",
        json!({"spaceId": work, "name": "Team", "tiles": [{"view": shared_view}, {"text": "Weekly numbers"}]}),
    )
    .await;
    assert_eq!(shared["outcome"], "ok", "{shared}");
    let team = shared["value"]["id"].clone();
    let (_, seen) = op(mara, "dashboard_get", json!({"dashboard": team})).await;
    assert_eq!(seen["outcome"], "ok", "{seen}");
    assert!(seen["value"]["tiles"][0]["figures"].is_object(), "{seen}");
    let (_, unseen) = op(
        &world.people["olive"].browser,
        "dashboard_get",
        json!({"dashboard": team}),
    )
    .await;
    assert_eq!(unseen["outcome"], "ok", "{unseen}");
    let tile = &unseen["value"]["tiles"][0];
    assert!(
        tile["figures"].is_null() && tile["unseen"].is_string(),
        "{unseen}"
    );
    assert_eq!(
        unseen["value"]["tiles"][1]["text"], "Weekly numbers",
        "{unseen}"
    );

    // Making a shared dashboard someone's own takes the role that shares
    // them; done, the others lose it.
    let (_, kept) = op(
        &world.people["nate"].browser,
        "dashboard_update",
        json!({"dashboard": team, "share": false}),
    )
    .await;
    assert_ne!(
        kept["outcome"], "ok",
        "nate took the team's dashboard: {kept}"
    );
    let (_, taken) = op(
        lena,
        "dashboard_update",
        json!({"dashboard": team, "share": false}),
    )
    .await;
    assert_eq!(taken["outcome"], "ok", "{taken}");
    assert_eq!(taken["value"]["shared"], false, "{taken}");
    let (_, gone) = op(mara, "dashboard_get", json!({"dashboard": team})).await;
    assert_ne!(gone["outcome"], "ok", "mara still reads it: {gone}");
    world.finish().await;
}

/// `history_restore` picks its edit permission from the thing it resolves,
/// so a caller who can read a page but not edit it must be refused and the
/// page left as it was: an agent and a person, each on a viewer role. A
/// member restores it, which shows the refusals are about the role and not
/// a broken call.
#[tokio::test]
async fn history_restore_needs_the_things_edit_permission() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("REST")
        .person("vera", "viewer")
        .agent("rook", "viewer")
        .agent("mara", "member")
        .build("privacy_history_restore")
        .await
    else {
        return;
    };
    let app = &world.app;
    let (rook, mara) = (&world.agents["rook"], &world.agents["mara"]);
    let as_mara = |name: &'static str, arguments: Value| async move {
        structured(
            &call_tool(app, &mara.token, &mara.session, name, arguments)
                .await
                .1,
        )
    };
    let as_rook = |name: &'static str, arguments: Value| async move {
        structured(
            &call_tool(app, &rook.token, &rook.session, name, arguments)
                .await
                .1,
        )
    };

    let written = as_mara(
        "page_write",
        json!({"projectId": "REST", "title": "Gate", "body": "Paint it red."}),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let page = written["value"]["id"].as_str().expect("page").to_owned();
    let edited = as_mara(
        "page_write",
        json!({"pageId": page, "body": "Paint it blue.", "expectedVersion": 1}),
    )
    .await;
    assert_eq!(edited["outcome"], "ok", "{edited}");
    let body = || async {
        sqlx::query_scalar::<_, String>("SELECT body FROM project_pages WHERE id = $1::uuid")
            .bind(&page)
            .fetch_one(world.pool())
            .await
            .expect("page body")
    };
    let arguments = json!({"thing": page, "version": 1, "expectedVersion": 2});

    // The viewer agent reads the history but can't restore.
    let listed = as_rook("history_list", json!({"thing": page})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let refused = as_rook("history_restore", arguments.clone()).await;
    assert_eq!(
        refused["outcome"], "permission_denied",
        "viewer agent: {refused}"
    );
    assert_eq!(body().await, "Paint it blue.");

    // The viewer person, through the web, is refused the same way.
    let (status, refused) = send(
        app,
        world.people["vera"].browser.request(
            "POST",
            "/api/v2/ops/history_restore",
            Some("privacy-restore-vera"),
            arguments.clone(),
        ),
    )
    .await;
    assert_eq!(
        refused["outcome"], "permission_denied",
        "viewer person: {status} {refused}"
    );
    assert_eq!(body().await, "Paint it blue.");

    // A member restores it.
    let restored = as_mara("history_restore", arguments).await;
    assert!(
        matches!(restored["outcome"].as_str(), Some("ok" | "applied")),
        "member: {restored}"
    );
    assert_eq!(body().await, "Paint it red.");
    world.finish().await;
}

/// C1.11: a role that neither edits nor suggests on boards can't draw on
/// one or leave a proposal there, from an agent or from the web.
#[tokio::test]
async fn board_proposals_need_the_suggest_permission() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("DRAW")
        .person("vera", "viewer")
        .agent("rook", "viewer")
        .agent("mara", "member")
        .build("privacy_board_proposals")
        .await
    else {
        return;
    };
    let app = &world.app;
    let (rook, mara) = (&world.agents["rook"], &world.agents["mara"]);
    let made = structured(
        &call_tool(
            app,
            &mara.token,
            &mara.session,
            "whiteboard_create",
            json!({"title": "Plan", "projectId": world.projects["DRAW"],
                   "operations": [{"op": "add", "type": "rectangle", "id": "box", "text": "A"}],
                   "clientRequestId": "privacy-board"}),
        )
        .await
        .1,
    );
    assert_eq!(made["outcome"], "ok", "{made}");
    let board = made["value"]["whiteboard"]["id"].clone();
    let element = json!({"id": "net", "type": "rectangle", "x": 0, "y": 0, "width": 10,
                         "height": 10, "version": 1, "versionNonce": 1});

    for (name, arguments) in [
        (
            "whiteboard_update_elements",
            json!({"whiteboardId": board, "operations": [{"op": "add", "type": "ellipse", "id": "net"}]}),
        ),
        (
            "whiteboard_propose",
            json!({"whiteboardId": board, "elements": [element.clone()]}),
        ),
    ] {
        let refused = structured(
            &call_tool(app, &rook.token, &rook.session, name, arguments)
                .await
                .1,
        );
        assert_eq!(
            refused["outcome"], "permission_denied",
            "viewer agent {name}: {refused}"
        );
    }
    let (status, refused) = send(
        app,
        world.people["vera"].browser.request(
            "POST",
            "/api/v2/ops/whiteboard_propose",
            Some("privacy-propose-vera"),
            json!({"whiteboardId": board, "elements": [element]}),
        ),
    )
    .await;
    assert_eq!(
        refused["outcome"], "permission_denied",
        "viewer person: {status} {refused}"
    );
    let proposals = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM proposals WHERE thing_kind = 'whiteboard'",
    )
    .fetch_one(world.pool())
    .await
    .expect("proposals");
    assert_eq!(proposals, 0);
    world.finish().await;
}

/// H1.24: deleting the workspace needs "Delete the workspace"
/// (`workspace.delete`), which only owners start with. A member and an
/// admin are refused; the owner schedules it and keeps it again.
#[tokio::test]
async fn only_holders_of_workspace_delete_may_delete_or_keep_it() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("ada", "admin")
        .build("privacy_workspace_delete")
        .await
    else {
        return;
    };
    let ada = world.people["ada"].actor;
    sqlx::query(
        "UPDATE organization_memberships SET role = 'admin', role_explicit = true \
         WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(world.organization)
    .bind(ada)
    .execute(world.pool())
    .await
    .expect("ada is a workspace admin");
    let slug = sqlx::query_scalar::<_, String>("SELECT slug FROM organizations WHERE id = $1")
        .bind(world.organization)
        .fetch_one(world.pool())
        .await
        .expect("slug");
    let call = |browser: &super::support::app::Browser, name: &str, key: &str, input| {
        browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/{name}", world.organization),
            Some(key),
            input,
        )
    };
    for person in ["lena", "ada"] {
        let browser = &world.people[person].browser;
        let (status, body) = send(
            &world.app,
            call(
                browser,
                "workspace_delete",
                &format!("{person}-delete"),
                json!({"confirm": slug}),
            ),
        )
        .await;
        assert_ne!(body["outcome"], "ok", "{person} deletes: {status} {body}");
    }
    let lifecycle = || async {
        sqlx::query_scalar::<_, String>("SELECT lifecycle FROM organizations WHERE id = $1")
            .bind(world.organization)
            .fetch_one(world.pool())
            .await
            .expect("lifecycle")
    };
    assert_eq!(lifecycle().await, "active");
    let (status, body) = send(
        &world.app,
        call(
            &world.owner.browser,
            "workspace_delete",
            "owner-delete",
            json!({"confirm": slug}),
        ),
    )
    .await;
    assert_eq!(body["outcome"], "ok", "the owner deletes: {status} {body}");
    assert_eq!(lifecycle().await, "deleting");
    // Nor may they keep it.
    for person in ["lena", "ada"] {
        let browser = &world.people[person].browser;
        let (status, body) = send(
            &world.app,
            call(
                browser,
                "workspace_delete_cancel",
                &format!("{person}-keep"),
                json!({}),
            ),
        )
        .await;
        assert_ne!(body["outcome"], "ok", "{person} keeps it: {status} {body}");
    }
    assert_eq!(lifecycle().await, "deleting");
    let (status, body) = send(
        &world.app,
        call(
            &world.owner.browser,
            "workspace_delete_cancel",
            "owner-keep",
            json!({}),
        ),
    )
    .await;
    assert_eq!(body["outcome"], "ok", "the owner keeps it: {status} {body}");
    assert_eq!(lifecycle().await, "active");
}

/// A2.11: a follow-up is its account's alone, and only about what that
/// account can see. Another member can't change, cancel or list it, nor set
/// one about a DM they aren't in.
#[tokio::test]
async fn follow_ups_are_their_accounts_own() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("rook", "member")
        .build("privacy_schedules")
        .await
    else {
        return;
    };
    let rook = world.agents["rook"].actor;
    let schedule = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agent_schedules (id, workspace_id, account_id, note, next_due_at) \
         VALUES ($1, $2, $3, 'Rook checks back', now() + interval '1 day')",
    )
    .bind(schedule)
    .bind(world.organization)
    .bind(rook)
    .execute(world.pool())
    .await
    .expect("rook's follow-up");
    let (status, dm) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            "/api/v1/direct/dms",
            None,
            json!({ "actorId": rook }),
        ),
    )
    .await;
    assert!(status.is_success(), "{dm}");
    let dm = dm["conversation"]["id"].as_str().expect("DM").to_owned();
    let op = |name: &str| format!("/api/v2/w/{}/ops/{name}", world.organization);
    let lena = &world.people["lena"].browser;
    for (name, key, body) in [
        (
            "schedule_update",
            "s-1",
            json!({ "scheduleId": schedule, "cancel": true }),
        ),
        (
            "schedule_create",
            "s-2",
            json!({ "inMinutes": 30, "about": dm }),
        ),
    ] {
        let (status, body) =
            send(&world.app, lena.request("POST", &op(name), Some(key), body)).await;
        assert_ne!(body["outcome"], "ok", "{name}: {status} {body}");
    }
    let (_, listed) = send(
        &world.app,
        lena.request("POST", &op("schedule_list"), None, json!({})),
    )
    .await;
    assert!(
        !listed.to_string().contains(&schedule.to_string()),
        "{listed}"
    );
    let state = sqlx::query_scalar::<_, String>("SELECT state FROM agent_schedules WHERE id = $1")
        .bind(schedule)
        .fetch_one(world.pool())
        .await
        .expect("state");
    assert_eq!(state, "active");
    world.finish().await;
}

/// A2: an account's delivery connections are its own and its managers'.
/// Another member can't add one for an agent, change, re-key, test or remove
/// its connection, list them or read and resend its deliveries; the owner,
/// who manages every agent, can.
#[tokio::test]
async fn only_an_account_and_its_managers_touch_its_connections() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("rook", "member")
        .build("privacy_connections")
        .await
    else {
        return;
    };
    let rook = world.agents["rook"].actor;
    let connection = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM agent_connections WHERE account_id = $1 AND is_default \
             AND source_connection_id IS NULL",
    )
    .bind(rook)
    .fetch_one(world.pool())
    .await
    .expect("rook's connection");
    // Its default, and the one for its credential.
    let untouched = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM agent_connections WHERE account_id = $1",
    )
    .bind(rook)
    .fetch_one(world.pool())
    .await
    .expect("count");
    assert_eq!(untouched, 2);
    let op = |name: &str| format!("/api/v2/w/{}/ops/{name}", world.organization);
    let lena = &world.people["lena"].browser;
    for (name, key, body) in [
        (
            "connection_create",
            "l-1",
            json!({ "account": rook, "name": "Lena's" }),
        ),
        (
            "connection_update",
            "l-2",
            json!({ "connectionId": connection, "name": "Lena's" }),
        ),
        (
            "connection_secret_rotate",
            "l-3",
            json!({ "connectionId": connection }),
        ),
        (
            "connection_remove",
            "l-4",
            json!({ "connectionId": connection }),
        ),
        ("connection_list", "l-5", json!({ "account": rook })),
        // A2.6: its delivery log, a test ping and a redelivery too.
        (
            "connection_delivery_list",
            "l-6",
            json!({ "connectionId": connection }),
        ),
        (
            "connection_test",
            "l-7",
            json!({ "connectionId": connection }),
        ),
        (
            "connection_redeliver",
            "l-8",
            json!({ "connectionId": connection, "eventPosition": 1 }),
        ),
    ] {
        let (status, body) =
            send(&world.app, lena.request("POST", &op(name), Some(key), body)).await;
        assert_ne!(body["outcome"], "ok", "{name}: {status} {body}");
        assert!(
            !body.to_string().contains(&connection.to_string()),
            "{name}: {body}"
        );
    }
    let unchanged = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM agent_connections WHERE account_id = $1 AND state = 'active' AND version = 1",
    )
    .bind(rook)
    .fetch_one(world.pool())
    .await
    .expect("count");
    assert_eq!(unchanged, untouched, "rook's connections are as they were");
    let (status, body) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &op("connection_list"),
            Some("o-1"),
            json!({ "account": rook }),
        ),
    )
    .await;
    assert_eq!(body["outcome"], "ok", "{status} {body}");
    assert!(body.to_string().contains(&connection.to_string()), "{body}");
    world.finish().await;
}

/// K5.20: a direct upload goes only into a conversation the caller may post
/// in (never a private group they aren't in), and only someone who may
/// manage settings sees or changes the workspace's own bucket.
#[tokio::test]
async fn direct_uploads_and_the_workspace_bucket_stay_inside_their_rules() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("ana", "member")
        .person("lena", "member")
        .private_group("secret", &["ana"])
        .build("privacy_direct_uploads")
        .await
    else {
        return;
    };
    let route = |op: &str| format!("/api/v2/w/{}/ops/{op}", world.organization);
    let secret = world.conversations["secret"].clone();
    let lena = &world.people["lena"];
    let (status, refused) = send(
        &world.app,
        lena.browser.request(
            "POST",
            &route("file_upload_start"),
            Some("lena-upload"),
            json!({ "conversationId": secret, "name": "a.txt", "size": 1,
                    "sha256": "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb" }),
        ),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{status} {refused}");
    for (op, body) in [
        ("workspace_store_get", json!({})),
        ("workspace_store_revoke", json!({})),
        (
            "workspace_store_set",
            json!({ "endpoint": "https://s3.example.com", "bucket": "mine",
                    "region": "us-east-1", "accessKeyId": "a", "secretAccessKey": "b" }),
        ),
    ] {
        let (status, refused) = send(
            &world.app,
            lena.browser
                .request("POST", &route(op), Some(&format!("lena-{op}")), body),
        )
        .await;
        assert_ne!(refused["outcome"], "ok", "{op}: {status} {refused}");
    }

    // A group's file is read by its members only: it is marked so and can
    // never name a workspace's own bucket.
    let ana = &world.people["ana"];
    let file = upload(
        &world.app,
        &ana.browser,
        &format!("/api/v1/conversations/{secret}/files?name=plan.txt"),
        "text/plain",
        b"the plan".to_vec(),
    )
    .await;
    let pool = &world.state.pool;
    let (members_only, store) = sqlx::query_as::<_, (bool, String)>(
        "SELECT members_only, store_id FROM files WHERE id = $1",
    )
    .bind(file)
    .fetch_one(pool)
    .await
    .expect("the group's file");
    assert!(members_only, "a group's file is for its members");
    assert!(!store.starts_with("ws-"), "{store}");
    let forced = sqlx::query("UPDATE files SET members_only = false WHERE id = $1")
        .bind(file)
        .execute(pool)
        .await;
    assert!(forced.is_ok());
    let still = sqlx::query_scalar::<_, bool>("SELECT members_only FROM files WHERE id = $1")
        .bind(file)
        .fetch_one(pool)
        .await
        .expect("members only");
    assert!(still, "the conversation decides, not the writer");
    let into_bucket = sqlx::query("UPDATE files SET store_id = 'ws-0123' WHERE id = $1")
        .bind(file)
        .execute(pool)
        .await;
    assert!(
        into_bucket.is_err(),
        "a group's file can't name a workspace's bucket"
    );
    world.finish().await;
}

/// K3.26: only someone who may manage automations makes or deletes an
/// incoming hook; a member sees the space's hooks but never a secret.
#[tokio::test]
async fn incoming_hooks_need_automations_manage_and_never_show_a_secret() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .build("privacy_incoming_hooks")
        .await
    else {
        return;
    };
    let route = |op: &str| format!("/api/v2/w/{}/ops/{op}", world.organization);
    let space = world.projects["WORK"].clone();
    let (status, made) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &route("incoming_hook_create"),
            Some("hook-1"),
            json!({ "spaceId": space, "name": "Alerts" }),
        ),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{status} {made}");
    let hook = made["value"]["hook"]["id"]
        .as_str()
        .expect("hook")
        .to_owned();
    let secret = made["value"]["secret"].as_str().expect("secret").to_owned();
    let lena = &world.people["lena"];
    let (_, listed) = send(
        &world.app,
        lena.browser.request(
            "POST",
            &route("incoming_hook_list"),
            None,
            json!({ "spaceId": space }),
        ),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert_eq!(listed["value"]["hooks"][0]["id"], hook.as_str(), "{listed}");
    assert!(!listed.to_string().contains(&secret), "{listed}");
    for (op, body) in [
        (
            "incoming_hook_create",
            json!({ "spaceId": space, "name": "Mine" }),
        ),
        (
            "incoming_hook_delete",
            json!({ "spaceId": space, "hookId": hook }),
        ),
    ] {
        let (status, refused) = send(
            &world.app,
            lena.browser
                .request("POST", &route(op), Some(&format!("lena-{op}")), body),
        )
        .await;
        assert_ne!(refused["outcome"], "ok", "{op}: {status} {refused}");
    }
    let still = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM incoming_hooks")
        .fetch_one(world.pool())
        .await
        .expect("hooks");
    assert_eq!(still, 1);
    world.finish().await;
}

/// K3.15: linking needs edit on the thing the link starts at and sight of
/// the other end. A viewer can't link, and nobody can link to a private
/// group they aren't in (the answer is the same as for no such thing).
#[tokio::test]
async fn links_by_hand_need_edit_on_the_start_and_sight_of_the_end() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("vera", "viewer")
        .person("ana", "member")
        .private_group("secret", &["ana"])
        .build("privacy_hand_links")
        .await
    else {
        return;
    };
    let route = |op: &str| format!("/api/v2/w/{}/ops/{op}", world.organization);
    let work = world.project("WORK").to_owned();
    let mut pages = Vec::new();
    for title in ["Plan", "Notes"] {
        let (status, page) = send(
            &world.app,
            world.owner.browser.request(
                "POST",
                &format!("/api/v1/projects/{work}/pages"),
                None,
                json!({ "title": title, "body": "" }),
            ),
        )
        .await;
        assert!(status.is_success(), "{page}");
        pages.push(page["id"].as_str().expect("page").to_owned());
    }
    let (plan, notes) = (&pages[0], &pages[1]);
    let link = |person: &str, op: &str, to: &str, key: &str| {
        let browser = &world.people[person].browser;
        send(
            &world.app,
            browser.request(
                "POST",
                &route(op),
                Some(key),
                json!({ "from": plan, "to": to, "kind": "about" }),
            ),
        )
    };

    let (status, refused) = link("vera", "thing_link", notes, "vera-link").await;
    assert_ne!(refused["outcome"], "ok", "{status} {refused}");
    let secret = world.conversations["secret"].clone();
    let (status, hidden) = link("lena", "thing_link", &secret, "lena-secret").await;
    assert_ne!(hidden["outcome"], "ok", "{status} {hidden}");
    assert!(
        !hidden.to_string().contains("conversation"),
        "a hidden end reads as no such thing: {hidden}"
    );
    let held = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM links WHERE from_thing_id = $1::uuid AND origin = 'explicit'",
    )
    .bind(plan)
    .fetch_one(world.pool())
    .await
    .expect("links");
    assert_eq!(held, 0, "no refused link was kept");

    let (status, made) = link("lena", "thing_link", notes, "lena-link").await;
    assert_eq!(made["outcome"], "ok", "{status} {made}");
    assert_eq!(made["value"]["changed"], true, "{made}");
    let (status, vera_unlink) = link("vera", "thing_unlink", notes, "vera-unlink").await;
    assert_ne!(vera_unlink["outcome"], "ok", "{status} {vera_unlink}");
    let (status, gone) = link("lena", "thing_unlink", notes, "lena-unlink").await;
    assert_eq!(gone["outcome"], "ok", "{status} {gone}");
    assert_eq!(gone["value"]["changed"], true, "{gone}");
    world.finish().await;
}

/// H1.9: only someone who may export the workspace starts an export, and
/// only the one who asked sees or downloads it.
#[tokio::test]
async fn an_export_is_for_its_requester_alone() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .build("privacy_workspace_export")
        .await
    else {
        return;
    };
    let route = |op: &str| format!("/api/v2/w/{}/ops/{op}", world.organization);
    let lena = &world.people["lena"];
    let (status, refused) = send(
        &world.app,
        lena.browser.request(
            "POST",
            &route("workspace_export_start"),
            Some("lena-export"),
            json!({}),
        ),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{status} {refused}");
    let (status, started) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &route("workspace_export_start"),
            Some("owner-export"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(started["outcome"], "ok", "{status} {started}");
    let id = started["value"]["id"].as_str().expect("id").to_owned();
    crate::support::run_export(&world.state, id.parse().expect("uuid"))
        .await
        .expect("export runs");
    for (op, body) in [
        ("workspace_transfer_get", json!({ "transferId": id })),
        ("workspace_transfer_list", json!({})),
    ] {
        let (status, refused) = send(
            &world.app,
            lena.browser.request("POST", &route(op), None, body),
        )
        .await;
        assert_ne!(refused["outcome"], "ok", "{op}: {status} {refused}");
    }
    let archive = agent_workspace_server::export::transfer::download_path(
        world.organization,
        id.parse().expect("uuid"),
    );
    let (status, _) = send(
        &world.app,
        lena.browser.request("GET", &archive, None, Value::Null),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a member never downloads it");
    world.finish().await;
}

/// C2.6 and C2.7: someone outside a private channel can't read its details
/// or members, join it, leave it or set its topic, and browsing doesn't
/// list it; a role that can't post sets no topic anywhere.
#[tokio::test]
async fn channel_writes_keep_to_private_channels() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("omar", "member")
        .person("nate", "member")
        .custom_role("Read only", &["projects.view", "directory.view"])
        .build("privacy_channel_writes")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let workspace = world.organization;
    let work = world.project("WORK").to_owned();
    let op = |browser: &Browser, name: &'static str, input: Value| {
        let request = browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/{name}"),
            Some(&Uuid::now_v7().to_string()),
            input,
        );
        async move { send(app, request).await }
    };
    let lena = &world.people["lena"];
    let omar = &world.people["omar"];
    let nate = &world.people["nate"];
    sqlx::query(
        "UPDATE project_memberships SET role_id = $1 WHERE project_id = $2 AND actor_id = $3",
    )
    .bind(Uuid::parse_str(&world.roles["Read only"]).expect("role id"))
    .bind(Uuid::parse_str(&work).expect("work"))
    .bind(nate.actor)
    .execute(pool)
    .await
    .expect("narrow nate");

    let made = |name: &'static str, private: bool| {
        let request = lena.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/channels"),
            Some(&Uuid::now_v7().to_string()),
            json!({"name": name, "private": private}),
        );
        async move {
            let (status, made) = send(app, request).await;
            assert_eq!(status, StatusCode::CREATED, "{made}");
            made["conversation"]["id"].as_str().expect("id").to_owned()
        }
    };
    let secret = made("secret-plans", true).await;
    let open = made("open-plans", false).await;

    for (name, input) in [
        ("channel_get", json!({"channelId": secret})),
        (
            "conversation_member_list",
            json!({"conversationId": secret}),
        ),
        ("channel_join", json!({"channelId": secret})),
        ("channel_leave", json!({"channelId": secret})),
        (
            "channel_update",
            json!({"channelId": secret, "topic": "Mine now"}),
        ),
        (
            "sidebar_item_move",
            json!({"conversationId": secret, "starred": true}),
        ),
        // C2.39: a conversation Omar can't read has no standing for him,
        // and no request of his to answer.
        ("dm_standing_get", json!({"conversationId": secret})),
        // C2.8: nor its details' tabs.
        ("conversation_file_list", json!({"conversationId": secret})),
        ("conversation_tab_list", json!({"conversationId": secret})),
        (
            "dm_request_respond",
            json!({"conversationId": secret, "accept": true}),
        ),
    ] {
        let (status, body) = op(&omar.browser, name, input).await;
        assert!(!status.is_success(), "{name}: {body}");
        assert!(!body.to_string().contains("secret-plans"), "{name}: {body}");
    }
    let (status, listed) = op(&omar.browser, "channel_list", json!({"spaceId": work})).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(!listed.to_string().contains(&secret), "{listed}");
    assert!(listed.to_string().contains(&open), "{listed}");
    // C2.26: jump-to finds the open channel by name, never the private one.
    let (status, places) = op(&omar.browser, "place_search", json!({"q": "plans"})).await;
    assert_eq!(status, StatusCode::OK, "{places}");
    assert!(!places.to_string().contains(&secret), "{places}");
    assert!(places.to_string().contains(&open), "{places}");
    // Saved searches are each account's own: Omar can't change Lena's.
    let (status, saved) = op(
        &lena.browser,
        "saved_search_set",
        json!({"name": "Plans", "query": "plans in:#secret-plans"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let lenas = saved["value"]["searches"][0]["id"]
        .as_str()
        .expect("saved")
        .to_owned();
    for input in [
        json!({"savedSearchId": lenas, "name": "Mine now"}),
        json!({"savedSearchId": lenas, "delete": true}),
    ] {
        let (status, refused) = op(&omar.browser, "saved_search_set", input).await;
        assert!(!status.is_success(), "{refused}");
    }
    let (_, omars) = op(&omar.browser, "saved_search_list", json!({})).await;
    assert_eq!(omars["value"]["searches"], json!([]), "{omars}");

    // C2.21: a record is made only from a message its maker can read, and
    // only by a role that may add records.
    let request = lena.browser.request(
        "POST",
        &format!("/api/v1/projects/{work}/collections"),
        Some(&Uuid::now_v7().to_string()),
        json!({"name": "Notes"}),
    );
    let (status, collection) = send(app, request).await;
    assert_eq!(status, StatusCode::CREATED, "{collection}");
    let collection = collection["collection"]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let said = |channel: &str, words: &'static str| {
        let request = lena.browser.request(
            "POST",
            &format!("/api/v1/conversations/{channel}/messages"),
            Some(&Uuid::now_v7().to_string()),
            json!({"content": text(words)}),
        );
        async move {
            let (status, sent) = send(app, request).await;
            assert!(status.is_success(), "{sent}");
            sent["message"]["id"].as_str().expect("message").to_owned()
        }
    };
    let hidden = said(&secret, "secret-plans launch day").await;
    let shown = said(&open, "Open plans launch day").await;
    let (status, refused) = op(
        &omar.browser,
        "record_create",
        json!({"collection": collection, "fromMessageId": hidden, "saySo": true}),
    )
    .await;
    assert!(!status.is_success(), "{refused}");
    assert!(!refused.to_string().contains("secret-plans"), "{refused}");
    let (status, made) = op(
        &omar.browser,
        "record_create",
        json!({"collection": collection, "fromMessageId": shown}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    assert_eq!(made["value"]["title"], "Open plans launch day", "{made}");
    let (status, refused) = op(
        &nate.browser,
        "record_create",
        json!({"collection": collection, "fromMessageId": shown}),
    )
    .await;
    assert!(!status.is_success(), "{refused}");
    let made_from = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT source_message_id FROM collection_records WHERE collection_id = $1::uuid",
    )
    .bind(&collection)
    .fetch_all(pool)
    .await
    .expect("records");
    assert_eq!(
        made_from,
        vec![Some(Uuid::parse_str(&shown).expect("shown"))]
    );
    let request = lena.browser.request(
        "GET",
        &format!("/api/v1/conversations/{open}/timeline"),
        None,
        Value::Null,
    );
    let (_, timeline) = send(app, request).await;
    let chips = timeline["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["message"]["id"] == json!(shown))
        .map(|entry| entry["message"]["madeInto"].clone());
    assert_eq!(
        chips,
        Some(json!([{"kind": "record", "id": made["value"]["id"]}])),
        "{timeline}"
    );

    // C2.8: a channel's Tasks tab lists what its tasks are only to people
    // who read the channel and may see tasks.
    let (status, task) = op(
        &lena.browser,
        "task_create",
        json!({"projectId": work, "title": "Plan launch", "fromMessageId": hidden}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{task}");
    let (status, listed) = op(
        &lena.browser,
        "conversation_task_list",
        json!({"conversationId": secret}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["value"]["tasks"][0]["how"], "made_from", "{listed}");
    assert_eq!(
        listed["value"]["tasks"][0]["title"], "Plan launch",
        "{listed}"
    );
    let (status, refused) = op(
        &omar.browser,
        "conversation_task_list",
        json!({"conversationId": secret}),
    )
    .await;
    assert!(!status.is_success(), "{refused}");
    assert!(!refused.to_string().contains("Plan launch"), "{refused}");
    let (status, refused) = op(
        &nate.browser,
        "conversation_task_list",
        json!({"conversationId": open}),
    )
    .await;
    assert!(!status.is_success(), "{refused}");

    // C1.26: a page tab shows to everyone who reads the channel; only a
    // poster sets one, and never on a channel they can't read.
    let request = lena.browser.request(
        "POST",
        &format!("/api/v1/projects/{work}/pages"),
        None,
        json!({"title": "Launch plan", "body": "Dates"}),
    );
    let (status, page) = send(app, request).await;
    assert!(status.is_success(), "{page}");
    let page = page["id"]
        .as_str()
        .or_else(|| page["page"]["id"].as_str())
        .expect("page")
        .to_owned();
    let (status, tabs) = op(
        &lena.browser,
        "conversation_tab_set",
        json!({"conversationId": open, "page": page}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{tabs}");
    let (status, seen) = op(
        &omar.browser,
        "conversation_tab_list",
        json!({"conversationId": open}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{seen}");
    assert_eq!(seen["value"]["tabs"][0]["title"], "Launch plan", "{seen}");
    // Moving the last tab to the front shifts the others along.
    let mut pages = vec![page.clone()];
    for title in ["Budget", "Retro"] {
        let request = lena.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/pages"),
            None,
            json!({"title": title, "body": "Notes"}),
        );
        let (status, made) = send(app, request).await;
        assert!(status.is_success(), "{made}");
        let id = made["id"]
            .as_str()
            .or_else(|| made["page"]["id"].as_str())
            .expect("page")
            .to_owned();
        let (status, tabs) = op(
            &lena.browser,
            "conversation_tab_set",
            json!({"conversationId": open, "page": id}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{tabs}");
        pages.push(id);
    }
    let (status, moved) = op(
        &lena.browser,
        "conversation_tab_set",
        json!({"conversationId": open, "page": pages[2], "position": 0}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    let titles: Vec<&str> = moved["value"]["tabs"]
        .as_array()
        .expect("tabs")
        .iter()
        .filter_map(|tab| tab["title"].as_str())
        .collect();
    assert_eq!(titles, ["Retro", "Launch plan", "Budget"], "{moved}");
    for (who, conversation) in [(&omar.browser, &secret), (&nate.browser, &open)] {
        let (status, refused) = op(
            who,
            "conversation_tab_set",
            json!({"conversationId": conversation, "page": page}),
        )
        .await;
        assert!(!status.is_success(), "{refused}");
    }

    // Nate reads but can't post, so sets no topic, even on an open channel.
    let (status, refused) = op(
        &nate.browser,
        "channel_update",
        json!({"channelId": open, "topic": "Nate's now"}),
    )
    .await;
    assert!(!status.is_success(), "{refused}");
    let topic =
        sqlx::query_scalar::<_, Option<String>>("SELECT topic FROM conversations WHERE id = $1")
            .bind(Uuid::parse_str(&open).expect("open"))
            .fetch_one(pool)
            .await
            .expect("topic");
    assert_eq!(topic, None);

    world.finish().await;
}

/// H1.27: only someone who may manage settings lists or sets spend limits,
/// and never on a direct conversation they aren't in.
#[tokio::test]
async fn spend_limits_need_settings_manage_and_keep_to_what_the_setter_sees() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("ana", "member")
        .person("lena", "member")
        .dm("ana", "lena")
        .build("privacy_spend_limits")
        .await
    else {
        return;
    };
    let route = |op: &str| format!("/api/v2/w/{}/ops/{op}", world.organization);
    let lena = &world.people["lena"];
    for (op, body) in [
        ("spend_limit_list", json!({})),
        (
            "spend_limit_set",
            json!({"scope": "workspace", "meter": "messages", "period": "day", "hardLimit": 5}),
        ),
    ] {
        let (status, refused) = send(
            &world.app,
            lena.browser
                .request("POST", &route(op), Some(&format!("lena-{op}")), body),
        )
        .await;
        assert_ne!(refused["outcome"], "ok", "{op}: {status} {refused}");
    }
    let secret = world.conversations["ana+lena"].clone();
    let (status, refused) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &route("spend_limit_set"),
            Some("owner-secret-limit"),
            json!({"scope": "conversation", "scopeId": secret, "meter": "messages",
                   "period": "minute", "perAccount": true, "hardLimit": 1}),
        ),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{status} {refused}");
    world.finish().await;
}
