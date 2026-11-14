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

use serde_json::{Value, json};
use uuid::Uuid;

use agent_workspace_server::test_support::source::{rust_files, server_src, strip_tests};

use super::{
    message_edits::{say, text},
    support::{
        app::{Browser, connect_app, initialize, send},
        callers::{Agent, Call, Caller, ConnectedApp, Person},
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
    let task_key = string_at(&task, "/key");
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
    let task_id = string_at(&task, "/workItemId");
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
