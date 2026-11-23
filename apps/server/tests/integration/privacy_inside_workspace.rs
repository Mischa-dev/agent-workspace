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
        "task_comment_create",
        json!({"workItemId": task_key, "update": format!("{TASK} is half done"), "clientRequestId": "quokka-comment"}),
    )
    .await;
    // WORK-1, which most of the cases name, carries it too.
    owner_call(
        "task_comment_create",
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
    // A1.17: what A's run changed, as its host reported it, is code: only
    // a reader who sees the run's thread and its code reads the files.
    let reported = sqlx::query(
        r#"
        INSERT INTO run_changes (
            run_id, workspace_id, space_id, files, patch, truncated, reported_by_account_id
        )
        SELECT run.id, run.workspace_id, run.space_id, $2, $3, false, agent_run.requesting_actor_id
        FROM runs run JOIN agent_runs agent_run ON agent_run.id = run.id
        WHERE run.id = $1
        "#,
    )
    .bind(a.run)
    .bind(
        json!([{"path": format!("src/{TASK}.rs"), "status": "modified", "added": 1, "removed": 0}]),
    )
    .bind(format!("diff --git a/src/{TASK}.rs b/src/{TASK}.rs\n"))
    .execute(pool)
    .await
    .expect("run changes");
    assert_eq!(reported.rows_affected(), 1, "A's run has its runs row");

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

    let mut tool_cases = mcp_cases(a, b);
    for (tool, extra) in extra_tool_calls(a) {
        match tool_cases.iter_mut().find(|case| case.tool == tool) {
            Some(case) => case.own.extend(extra),
            None => panic!("extra_tool_calls names {tool}, which has no isolation case"),
        }
    }
    for case in tool_cases {
        if case.own.is_empty() {
            unwalked.push(case.tool);
            continue;
        }
        let mut arguments = case.own.clone();
        if case.tool == "workspace_search" {
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
    let (status, role) = setup
        .browser
        .op_in(
            &setup.app,
            setup.organization_id,
            "role_create",
            json!({
                "name": "No tasks or code",
                "baseRole": "member",
                "permissions": [
                    "projects.view", "channels.post", "channels.create", "pages.edit",
                    "whiteboards.edit", "collections.edit", "sheets.edit", "directory.view"
                ]
            }),
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
    let (status, changed) = setup.browser.op_in(&setup.app, setup.organization_id, "role_assign", json!({"account": actor, "role": role_id, "applyToProjects": true, "expectedVersion": version})).await;
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

/// Tool calls walked here that the isolation net can't make as A's own,
/// because A's call there has to succeed: a connected module's panels and
/// files are asked of the module live, and no module answers in this world.
/// Walking them still checks the gate in front of the module.
fn extra_tool_calls(a: &World) -> Vec<(&'static str, Vec<Value>)> {
    let module = a.module.to_string();
    vec![
        (
            "module_panel_open",
            vec![
                json!({"moduleId": module, "panel": "main"}),
                json!({"moduleId": module, "panel": "main", "spaceId": a.project}),
            ],
        ),
        ("file_source_list", vec![json!({"moduleId": module})]),
        // M1.10: an upgrade's plan, walked against the fixture install.
        (
            "package_upgrade_preview",
            vec![json!({"installId": a.package_install, "package": {
                "format": "agent-workspace-package/1", "id": "fixture", "version": "2.0.0",
                "name": "Fixture", "target": "space", "contents": []}})],
        ),
        (
            "file_source_get",
            vec![json!({"moduleId": module, "ref": "f1"})],
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
    (
        "/api/v1/my-work",
        "the caller's own tasks, and the leads and update authors of spaces they are in (T1.43); \
         workspace_isolation walks it",
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
    // The direct scope (K4.17) is one id every workspace shares, not B's.
    let direct = agent_workspace_server::spaces::DIRECT.to_string();
    forbidden.retain(|word| *word != direct);
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

/// The account views a response can hold (a member list's `MemberView`
/// names the account it lists).
const ACCOUNT_VIEWS: [&str; 4] = ["ActorView", "Person", "ProfileView", "MemberView"];

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
    // invite). K1.33: nor take it over.
    for (name, key) in [
        ("agent_pause", "lena-pause"),
        ("agent_resume", "lena-resume"),
        ("account_workspace_add", "lena-add"),
        ("account_owner_take", "lena-take"),
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
        .project("HOME")
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
        "sheet_create",
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

    // Nate's role doesn't edit pages: he keeps no file on one (C1.33); Lena
    // does.
    let page = super::support::app::tool(
        app,
        &phone.token,
        &phone.session,
        "page_create",
        json!({"projectId": work, "title": "Plan", "body": "Ship it."}),
    )
    .await;
    assert_eq!(page["outcome"], "ok", "{page}");
    let page = string_at(&page, "/value/id");
    let file = |name: &str| json!({"pageId": page, "name": name, "text": "notes"});
    let (status, refused) = op(
        &world.people["nate"].browser,
        "page_file_upload",
        file("nate.txt"),
    )
    .await;
    assert!(
        !status.is_success(),
        "nate keeps a file: {status} {refused}"
    );
    let (status, kept) = op(
        &world.people["lena"].browser,
        "page_file_upload",
        file("lena.txt"),
    )
    .await;
    assert!(status.is_success(), "lena keeps a file: {status} {kept}");

    // Nor does he move a page to another space (C1.37); Lena does.
    let home = world.project("HOME").to_owned();
    let version =
        sqlx::query_scalar::<_, i64>("SELECT version FROM project_pages WHERE id = $1::uuid")
            .bind(&page)
            .fetch_one(pool)
            .await
            .expect("page version");
    let moving = json!({"pageId": page, "toSpaceId": home, "expectedVersion": version});
    let (status, refused) = op(&world.people["nate"].browser, "page_move", moving.clone()).await;
    assert!(
        !status.is_success(),
        "nate moves a page: {status} {refused}"
    );
    let (status, moved) = op(&world.people["lena"].browser, "page_move", moving).await;
    assert!(status.is_success(), "lena moves a page: {status} {moved}");

    // Nor does a role without tasks make tasks of a board's notes (C1.32).
    let board = super::support::app::tool(
        app,
        &phone.token,
        &phone.session,
        "whiteboard_create",
        json!({"projectId": work, "title": "Ideas"}),
    )
    .await;
    assert_eq!(board["outcome"], "ok", "{board}");
    let board = string_at(&board, "/value/whiteboard/id");
    let drawn = super::support::app::tool(
        app,
        &phone.token,
        &phone.session,
        "whiteboard_update_elements",
        json!({"whiteboardId": board, "operations": [
            {"op": "add", "type": "text", "id": "idea", "x": 0, "y": 0, "text": "Try it"}]}),
    )
    .await;
    assert_eq!(drawn["outcome"], "ok", "{drawn}");
    let notes = json!({"whiteboardId": board});
    let (status, refused) = op(
        &world.people["nate"].browser,
        "whiteboard_to_tasks",
        notes.clone(),
    )
    .await;
    assert!(!status.is_success(), "nate makes tasks: {status} {refused}");
    let (status, made) = op(&world.people["lena"].browser, "whiteboard_to_tasks", notes).await;
    assert!(status.is_success(), "lena makes tasks: {status} {made}");

    // Asking about part of the board (C1.31): someone out of its space
    // neither asks nor is asked; Lena asks Mara.
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE project_id = $1::uuid AND actor_id = $2",
    )
    .bind(&work)
    .bind(world.people["omar"].actor)
    .execute(pool)
    .await
    .expect("omar leaves WORK");
    let ask = |who: Uuid| {
        json!({"whiteboardId": board, "elementIds": ["idea"], "accountId": who,
               "question": "Worth it?"})
    };
    let (status, refused) = op(
        &world.people["omar"].browser,
        "whiteboard_ask",
        ask(world.people["mara"].actor),
    )
    .await;
    assert!(!status.is_success(), "omar asks: {status} {refused}");
    let (status, refused) = op(
        &world.people["lena"].browser,
        "whiteboard_ask",
        ask(world.people["omar"].actor),
    )
    .await;
    assert!(!status.is_success(), "lena asks omar: {status} {refused}");
    let (status, asked) = op(
        &world.people["lena"].browser,
        "whiteboard_ask",
        ask(world.people["mara"].actor),
    )
    .await;
    assert!(status.is_success(), "lena asks mara: {status} {asked}");
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
    // A chart (C1.18) changes the sheet for everyone too: sheets.edit.
    let (status, body) = op(
        nate,
        "sheet_chart_set",
        json!({"sheetId": sheet, "title": "Nate's", "source": "A1:B2"}),
    )
    .await;
    assert_ne!(body["outcome"], "ok", "nate charts: {status} {body}");
    let (status, body) = op(
        &world.people["lena"].browser,
        "sheet_chart_set",
        json!({"sheetId": sheet, "title": "Lena's", "source": "Lenas"}),
    )
    .await;
    assert_eq!(
        body["outcome"], "ok",
        "lena charts her range: {status} {body}"
    );
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
        "page_create",
        json!({"projectId": "REST", "title": "Gate", "body": "Paint it red."}),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let page = written["value"]["id"].as_str().expect("page").to_owned();
    let edited = as_mara(
        "page_update",
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

/// K1.32: a page shared with a member outside its space opens for them, and
/// nothing else in the space does; a link opens only its page, for whoever
/// is signed in with it, until it is revoked. Only someone who may edit the
/// page shares it.
#[tokio::test]
async fn a_shared_page_opens_only_itself() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("SECRET")
        .person("lena", "member")
        .person("mara", "member")
        .build("privacy_thing_share")
        .await
    else {
        return;
    };
    let secret = world.project("SECRET").to_owned();
    let lena = &world.people["lena"];
    let mara = &world.people["mara"];
    sqlx::query("DELETE FROM project_memberships WHERE project_id = $1 AND actor_id = ANY($2)")
        .bind(secret.parse::<Uuid>().expect("space"))
        .bind(vec![lena.actor, mara.actor])
        .execute(world.pool())
        .await
        .expect("lena and mara aren't in SECRET");
    let mut pages = Vec::new();
    for title in ["Plan", "Salaries"] {
        let (status, page) = send(
            &world.app,
            world.owner.browser.request(
                "POST",
                &format!("/api/v1/projects/{secret}/pages"),
                None,
                json!({"title": title, "body": "text"}),
            ),
        )
        .await;
        assert!(status.is_success(), "{status} {page}");
        pages.push(page["id"].as_str().expect("page").to_owned());
    }
    let (plan, salaries) = (&pages[0], &pages[1]);
    // Salaries sits inside Plan: sharing Plan doesn't tell its title.
    sqlx::query("UPDATE project_pages SET parent_page_id = $1::uuid WHERE id = $2::uuid")
        .bind(plan)
        .bind(salaries)
        .execute(world.pool())
        .await
        .expect("nest salaries");
    let read = |browser: &Browser, uri: String| browser.request("GET", &uri, None, Value::Null);
    let op = |browser: &Browser, name: &str, key: &str, body: Value| {
        browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/{name}", world.organization),
            Some(key),
            body,
        )
    };
    let (status, _) = send(
        &world.app,
        read(&lena.browser, format!("/api/v1/pages/{plan}")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "not shared yet");

    // Lena can't share what she can't edit.
    let (_, refused) = send(
        &world.app,
        op(
            &lena.browser,
            "thing_share_add",
            "lena-share",
            json!({"kind": "page", "id": plan, "account": mara.actor}),
        ),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");

    let (_, shared) = send(
        &world.app,
        op(
            &world.owner.browser,
            "thing_share_add",
            "owner-share",
            json!({"kind": "page", "id": plan, "account": lena.actor}),
        ),
    )
    .await;
    assert_eq!(shared["outcome"], "ok", "{shared}");
    // Sharing tells the space (ids only), in the same transaction.
    let signalled: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM events WHERE type = 'thing.share_changed' \
                 AND thing_id = $1) \
              + (SELECT count(*) FROM event_staging WHERE type = 'thing.share_changed' \
                 AND thing_id = $1)",
    )
    .bind(plan.parse::<Uuid>().expect("page"))
    .fetch_one(world.pool())
    .await
    .expect("share events");
    assert_eq!(signalled, 1, "sharing a page sends no thing.share_changed");
    let (status, page) = send(
        &world.app,
        read(&lena.browser, format!("/api/v1/pages/{plan}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["title"], "Plan");
    assert!(
        !page.to_string().contains("Salaries"),
        "a shared page names no page inside it: {page}"
    );
    for uri in [
        format!("/api/v1/pages/{salaries}"),
        format!("/api/v1/projects/{secret}/pages"),
    ] {
        let (status, body) = send(&world.app, read(&lena.browser, uri.clone())).await;
        assert!(
            matches!(status, StatusCode::NOT_FOUND | StatusCode::FORBIDDEN),
            "{uri}: {status} {body}"
        );
    }
    let (status, _) = send(
        &world.app,
        read(&mara.browser, format!("/api/v1/pages/{plan}")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "shared with Lena only");

    // A link opens the one page for Mara, until it is revoked.
    let (_, link) = send(
        &world.app,
        op(
            &world.owner.browser,
            "thing_link_create",
            "owner-link",
            json!({"kind": "page", "id": plan, "expiresInHours": 2}),
        ),
    )
    .await;
    assert_eq!(link["outcome"], "ok", "{link}");
    let token = link["value"]["token"].as_str().expect("token").to_owned();
    let (status, page) = send(
        &world.app,
        read(&mara.browser, format!("/api/v1/shared/{token}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["id"], plan.as_str());
    assert!(
        !page.to_string().contains("Salaries"),
        "a linked page names no page inside it: {page}"
    );
    let (status, _) = send(
        &world.app,
        read(&mara.browser, format!("/api/v1/pages/{salaries}")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &world.app,
        read(&mara.browser, format!("/api/v1/shared/{token}x")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a wrong token opens nothing");
    let (_, revoked) = send(
        &world.app,
        op(
            &world.owner.browser,
            "thing_share_remove",
            "owner-revoke",
            json!({"kind": "page", "id": plan, "link": link["value"]["id"]}),
        ),
    )
    .await;
    assert_eq!(revoked["outcome"], "ok", "{revoked}");
    let (status, _) = send(
        &world.app,
        read(&mara.browser, format!("/api/v1/shared/{token}")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a revoked link stops at once"
    );

    // C1.27: a link opens without signing in only when it is public, and
    // the share list names links, never their secrets.
    let anonymous = |path: String| {
        axum::http::Request::builder()
            .method("GET")
            .uri(path)
            .header(axum::http::header::HOST, "127.0.0.1")
            .body(axum::body::Body::empty())
            .expect("request")
    };
    let (_, private_link) = send(
        &world.app,
        op(
            &world.owner.browser,
            "thing_link_create",
            "owner-private-link",
            json!({"kind": "page", "id": plan}),
        ),
    )
    .await;
    let private_token = private_link["value"]["token"]
        .as_str()
        .expect("token")
        .to_owned();
    let (status, _) = send(
        &world.app,
        anonymous(format!("/api/v1/public/pages/{private_token}")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a signed-in link isn't public"
    );
    let (_, public_link) = send(
        &world.app,
        op(
            &world.owner.browser,
            "thing_link_create",
            "owner-public-link",
            json!({"kind": "page", "id": plan, "public": true}),
        ),
    )
    .await;
    assert_eq!(public_link["outcome"], "ok", "{public_link}");
    let public_token = public_link["value"]["token"]
        .as_str()
        .expect("token")
        .to_owned();
    let (status, page) = send(
        &world.app,
        anonymous(format!("/api/v1/public/pages/{public_token}")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["id"], plan.as_str());
    assert!(page.get("projectId").is_none(), "{page}");
    assert!(!page.to_string().contains("Salaries"), "{page}");
    let (_, listed) = send(
        &world.app,
        op(
            &world.owner.browser,
            "thing_share_list",
            "owner-list",
            json!({"kind": "page", "id": plan}),
        ),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert!(
        listed["value"]["links"]
            .as_array()
            .expect("links")
            .iter()
            .any(|link| link["id"] == public_link["value"]["id"] && link["public"] == true),
        "{listed}"
    );
    assert!(!listed.to_string().contains(&public_token), "{listed}");
    let (_, denied) = send(
        &world.app,
        op(
            &mara.browser,
            "thing_share_list",
            "mara-list",
            json!({"kind": "page", "id": plan}),
        ),
    )
    .await;
    assert_ne!(
        denied["outcome"], "ok",
        "only editors see the shares: {denied}"
    );
    let (_, revoked) = send(
        &world.app,
        op(
            &world.owner.browser,
            "thing_share_remove",
            "owner-revoke-public",
            json!({"kind": "page", "id": plan, "link": public_link["value"]["id"]}),
        ),
    )
    .await;
    assert_eq!(revoked["outcome"], "ok", "{revoked}");
    let (status, _) = send(
        &world.app,
        anonymous(format!("/api/v1/public/pages/{public_token}")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a revoked public link stops at once"
    );

    let (_, unshared) = send(
        &world.app,
        op(
            &world.owner.browser,
            "thing_share_remove",
            "owner-unshare",
            json!({"kind": "page", "id": plan, "account": lena.actor}),
        ),
    )
    .await;
    assert_eq!(unshared["outcome"], "ok", "{unshared}");
    let (status, _) = send(
        &world.app,
        read(&lena.browser, format!("/api/v1/pages/{plan}")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    world.finish().await;
}

/// C1.27: a page shared on its own does what its level says and no more:
/// `see` reads it, `comment` reads and writes its comments, `edit` changes
/// its words too. None of them lists, renames, shares or reaches another
/// page of the space.
#[tokio::test]
async fn a_shared_page_allows_only_its_level() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("SECRET")
        .person("lena", "member")
        .build("privacy_share_levels")
        .await
    else {
        return;
    };
    let secret = world.project("SECRET").to_owned();
    let lena = &world.people["lena"];
    sqlx::query("DELETE FROM project_memberships WHERE project_id = $1 AND actor_id = $2")
        .bind(secret.parse::<Uuid>().expect("space"))
        .bind(lena.actor)
        .execute(world.pool())
        .await
        .expect("lena isn't in SECRET");
    let mut pages = Vec::new();
    for title in ["Plan", "Other"] {
        let (status, page) = send(
            &world.app,
            world.owner.browser.request(
                "POST",
                &format!("/api/v1/projects/{secret}/pages"),
                None,
                json!({"title": title, "body": "first words"}),
            ),
        )
        .await;
        assert!(status.is_success(), "{status} {page}");
        pages.push(page["id"].as_str().expect("page").to_owned());
    }
    let (plan, other) = (pages[0].clone(), pages[1].clone());
    let read = |uri: String| lena.browser.request("GET", &uri, None, Value::Null);
    let op = |browser: &Browser, name: &str, key: &str, body: Value| {
        browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/{name}", world.organization),
            Some(key),
            body,
        )
    };
    let share = |level: &'static str| {
        op(
            &world.owner.browser,
            "thing_share_add",
            level,
            json!({"kind": "page", "id": plan, "account": lena.actor, "level": level}),
        )
    };
    let comment = |key: &str, page: &str| {
        op(
            &lena.browser,
            "page_comment_create",
            key,
            json!({"pageId": page, "text": "a thought"}),
        )
    };
    let append = |key: &str, page: &str| {
        op(
            &lena.browser,
            "page_update_blocks",
            key,
            json!({"pageId": page, "edits": [{"op": "append", "markdown": "more words"}]}),
        )
    };

    // see: the page, its document, nothing else.
    let (_, shared) = send(&world.app, share("see")).await;
    assert_eq!(shared["outcome"], "ok", "{shared}");
    assert_eq!(shared["value"]["accounts"][0]["level"], "see", "{shared}");
    let (status, page) = send(&world.app, read(format!("/api/v1/pages/{plan}"))).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["sharedLevel"], "see", "{page}");
    let (status, document) = send(&world.app, read(format!("/api/v1/pages/{plan}/document"))).await;
    assert_eq!(status, StatusCode::OK, "{document}");
    assert_eq!(document["canEdit"], false, "{document}");
    let (status, _) = send(&world.app, read(format!("/api/v1/pages/{plan}/discussion"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "see doesn't read comments");
    let (_, refused) = send(&world.app, comment("see-comment", &plan)).await;
    assert_ne!(refused["outcome"], "ok", "see doesn't comment: {refused}");
    let (_, refused) = send(&world.app, append("see-edit", &plan)).await;
    assert_ne!(refused["outcome"], "ok", "see doesn't edit: {refused}");

    // comment: its discussion too, and still nothing of the space's.
    let (_, shared) = send(&world.app, share("comment")).await;
    assert_eq!(shared["outcome"], "ok", "{shared}");
    let (_, commented) = send(&world.app, comment("comment-comment", &plan)).await;
    assert_eq!(commented["outcome"], "ok", "{commented}");
    let (status, discussion) =
        send(&world.app, read(format!("/api/v1/pages/{plan}/discussion"))).await;
    assert_eq!(status, StatusCode::OK, "{discussion}");
    assert_eq!(discussion["canComment"], true, "{discussion}");
    let (_, listed) = send(
        &world.app,
        op(
            &lena.browser,
            "page_comment_list",
            "comment-list",
            json!({"pageId": plan}),
        ),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert!(listed.to_string().contains("a thought"), "{listed}");
    let (_, refused) = send(&world.app, comment("comment-other", &other)).await;
    assert_ne!(refused["outcome"], "ok", "only the shared page: {refused}");
    let (_, refused) = send(&world.app, append("comment-edit", &plan)).await;
    assert_ne!(refused["outcome"], "ok", "comment doesn't edit: {refused}");

    // edit: its words too, but not its title, its shares or another page.
    let (_, shared) = send(&world.app, share("edit")).await;
    assert_eq!(shared["outcome"], "ok", "{shared}");
    let (status, document) = send(&world.app, read(format!("/api/v1/pages/{plan}/document"))).await;
    assert_eq!(status, StatusCode::OK, "{document}");
    assert_eq!(document["canEdit"], true, "{document}");
    let (_, edited) = send(&world.app, append("edit-edit", &plan)).await;
    assert_eq!(edited["outcome"], "ok", "{edited}");
    let body: String = sqlx::query_scalar("SELECT body FROM project_pages WHERE id = $1::uuid")
        .bind(&plan)
        .fetch_one(world.pool())
        .await
        .expect("body");
    assert!(body.contains("more words"), "{body}");
    let (_, refused) = send(&world.app, append("edit-other", &other)).await;
    assert_ne!(refused["outcome"], "ok", "only the shared page: {refused}");
    for (name, body) in [
        ("page_update", json!({"pageId": plan, "title": "Mine now"})),
        (
            "thing_share_add",
            json!({"kind": "page", "id": plan, "account": world.owner.actor}),
        ),
        ("thing_share_list", json!({"kind": "page", "id": plan})),
        ("page_list", json!({"projectId": secret})),
    ] {
        let (_, refused) = send(&world.app, op(&lena.browser, name, name, body)).await;
        assert_ne!(refused["outcome"], "ok", "{name}: {refused}");
    }
    let title: String = sqlx::query_scalar("SELECT title FROM project_pages WHERE id = $1::uuid")
        .bind(&plan)
        .fetch_one(world.pool())
        .await
        .expect("title");
    assert_eq!(title, "Plan");
    // Its history stays with its space: no earlier version by number.
    let (_, refused) = send(
        &world.app,
        op(
            &lena.browser,
            "page_get",
            "edit-old-version",
            json!({"pageId": plan, "version": 1}),
        ),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "no earlier version: {refused}");
    // An archived space takes no changes, shared or not.
    sqlx::query(
        "UPDATE projects SET archived_at = now(), archived_by_actor_id = $2 WHERE id = $1::uuid",
    )
    .bind(&secret)
    .bind(world.owner.actor)
    .execute(world.pool())
    .await
    .expect("archive SECRET");
    let (_, refused) = send(&world.app, append("archived-edit", &plan)).await;
    assert_ne!(refused["outcome"], "ok", "archived: {refused}");
    let (_, document) = send(&world.app, read(format!("/api/v1/pages/{plan}/document"))).await;
    assert_ne!(document["canEdit"], true, "archived: {document}");
    sqlx::query(
        "UPDATE projects SET archived_at = NULL, archived_by_actor_id = NULL WHERE id = $1::uuid",
    )
    .bind(&secret)
    .execute(world.pool())
    .await
    .expect("restore SECRET");

    // A link only reads.
    let violated = sqlx::query(
        "UPDATE thing_grants SET level = 'edit' WHERE thing_id = $1::uuid AND link_digest IS NULL",
    )
    .bind(&plan)
    .execute(world.pool())
    .await;
    assert!(violated.is_ok(), "a person's share may edit");
    let (_, link) = send(
        &world.app,
        op(
            &world.owner.browser,
            "thing_link_create",
            "owner-link",
            json!({"kind": "page", "id": plan}),
        ),
    )
    .await;
    assert_eq!(link["outcome"], "ok", "{link}");
    let refused = sqlx::query("UPDATE thing_grants SET level = 'edit' WHERE id = $1::uuid")
        .bind(link["value"]["id"].as_str().expect("link id"))
        .execute(world.pool())
        .await;
    assert!(refused.is_err(), "a link stays read-only");

    // Unshared, all of it stops.
    let (_, unshared) = send(
        &world.app,
        op(
            &world.owner.browser,
            "thing_share_remove",
            "owner-unshare",
            json!({"kind": "page", "id": plan, "account": lena.actor}),
        ),
    )
    .await;
    assert_eq!(unshared["outcome"], "ok", "{unshared}");
    let (_, refused) = send(&world.app, comment("after-comment", &plan)).await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    let (status, _) = send(&world.app, read(format!("/api/v1/pages/{plan}/document"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    world.finish().await;
}

/// C2.27: an answer from search reads only what the asker can see. A page
/// in a space they aren't in never reaches the model, so never the answer.
#[tokio::test]
async fn search_answers_read_only_what_the_asker_sees() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .project("SECRET")
        .person("lena", "member")
        .build("privacy_search_answers")
        .await
    else {
        return;
    };
    let (work, secret) = (
        world.project("WORK").to_owned(),
        world.project("SECRET").to_owned(),
    );
    let lena = &world.people["lena"];
    sqlx::query("DELETE FROM project_memberships WHERE project_id = $1 AND actor_id = $2")
        .bind(secret.parse::<Uuid>().expect("space"))
        .bind(lena.actor)
        .execute(world.pool())
        .await
        .expect("lena isn't in SECRET");
    for (space, title, body) in [
        (&work, "Zebra plan", "The zebra launch is on Friday."),
        (&secret, "Zebra vault", "The zebra launch code is 4242."),
    ] {
        let (status, page) = send(
            &world.app,
            world.owner.browser.request(
                "POST",
                &format!("/api/v1/projects/{space}/pages"),
                None,
                json!({"title": title, "body": body}),
            ),
        )
        .await;
        assert!(status.is_success(), "{status} {page}");
    }
    let op = |browser: &Browser, name: &str, body: Value| {
        browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/{name}", world.organization),
            Some(&Uuid::now_v7().to_string()),
            body,
        )
    };
    let (_, set) = send(
        &world.app,
        op(
            &world.owner.browser,
            "setting_update",
            json!({"key": "ai.provider", "scope": "workspace", "value": "fake"}),
        ),
    )
    .await;
    assert_eq!(set["outcome"], "ok", "{set}");

    let (_, answered) = send(
        &world.app,
        op(
            &lena.browser,
            "search_answer",
            json!({"question": "zebra launch"}),
        ),
    )
    .await;
    assert_eq!(answered["outcome"], "ok", "{answered}");
    let text = answered.to_string();
    assert!(text.contains("Friday"), "{answered}");
    assert!(
        !text.contains("4242") && !text.contains("Zebra vault"),
        "{answered}"
    );
    let sources = answered["value"]["sources"].as_array().expect("sources");
    assert_eq!(sources.len(), 1, "{answered}");
    assert_eq!(sources[0]["title"], "Zebra plan", "{answered}");

    // The owner sees both, so both are read.
    let (_, answered) = send(
        &world.app,
        op(
            &world.owner.browser,
            "search_answer",
            json!({"question": "zebra launch"}),
        ),
    )
    .await;
    assert_eq!(answered["outcome"], "ok", "{answered}");
    assert_eq!(answered["value"]["read"], 2, "{answered}");
    world.finish().await;
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

/// Themes: someone's own theme is theirs alone until the workspace offers
/// it. Another member never reads, changes or deletes it; only a holder of
/// the settings permission offers one; and an offered theme is still only
/// its maker's to change.
#[tokio::test]
async fn themes_are_their_makers_until_offered() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("rook", "member")
        .build("privacy_themes")
        .await
    else {
        return;
    };
    let op = |name: &str| format!("/api/v2/w/{}/ops/{name}", world.organization);
    let owner = &world.owner.browser;
    let lena = &world.people["lena"].browser;
    let document = json!({
        "format": 1,
        "light": {"base": "quiet", "tokens": {"accent": "#224466"}},
        "dark": {"base": "quiet-dark", "tokens": {}},
    });
    let (status, made) = send(
        &world.app,
        owner.request(
            "POST",
            &op("theme_create"),
            Some("t-1"),
            json!({ "name": "Owner's own", "document": document }),
        ),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{status} {made}");
    let theme = made["value"]["id"].as_str().expect("theme id").to_owned();

    // Before it's offered, Lena can't see it, change it or offer it.
    let refused = |name: &'static str, key: &'static str, body: Value| {
        let request = lena.request("POST", &op(name), Some(key), body);
        let app = world.app.clone();
        async move {
            let (status, body) = send(&app, request).await;
            assert_ne!(body["outcome"], "ok", "{name}: {status} {body}");
        }
    };
    refused("theme_get", "l-1", json!({ "id": theme })).await;
    refused(
        "theme_update",
        "l-2",
        json!({ "id": theme, "expectedVersion": 1, "name": "Mine now" }),
    )
    .await;
    refused("theme_delete", "l-3", json!({ "id": theme })).await;
    refused(
        "workspace_theme_set",
        "l-4",
        json!({ "themeId": theme, "offered": true }),
    )
    .await;
    let (_, listed) = send(
        &world.app,
        lena.request("POST", &op("theme_list"), None, json!({})),
    )
    .await;
    assert!(!listed.to_string().contains(&theme), "{listed}");

    // A theme that loads from elsewhere is refused for anyone.
    let (_, leaky) = send(
        &world.app,
        owner.request(
            "POST",
            &op("theme_create"),
            Some("t-2"),
            json!({ "name": "Leaky", "document": {
                "format": 1,
                "light": {"base": "quiet", "tokens": {}},
                "dark": {"base": "quiet-dark", "tokens": {}},
                "css": "input[value^='a'] { background: url(https://example.com/a); }",
            }}),
        ),
    )
    .await;
    assert_ne!(leaky["outcome"], "ok", "{leaky}");

    // Offered, Lena reads it and lists it, but it stays the owner's to change.
    let (_, offered) = send(
        &world.app,
        owner.request(
            "POST",
            &op("workspace_theme_set"),
            Some("t-3"),
            json!({ "themeId": theme, "offered": true }),
        ),
    )
    .await;
    assert_eq!(offered["outcome"], "ok", "{offered}");
    let (_, read) = send(
        &world.app,
        lena.request("POST", &op("theme_get"), None, json!({ "id": theme })),
    )
    .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    assert_eq!(read["value"]["mine"], false, "{read}");
    refused(
        "theme_update",
        "l-5",
        json!({ "id": theme, "expectedVersion": 2, "name": "Mine now" }),
    )
    .await;
    refused("theme_delete", "l-6", json!({ "id": theme })).await;
    refused(
        "workspace_theme_set",
        "l-7",
        json!({ "themeId": theme, "offered": false }),
    )
    .await;
    let name = sqlx::query_scalar::<_, String>("SELECT name FROM themes WHERE id = $1::uuid")
        .bind(&theme)
        .fetch_one(world.pool())
        .await
        .expect("theme");
    assert_eq!(name, "Owner's own");
    world.finish().await;
}

/// K2.17: roles and caps change only for whoever the role rules allow. A
/// member (no "Manage roles", no "Manage members", no member list) makes,
/// changes, removes and gives no role, caps nobody else's apps and reads
/// nobody else's permissions; they cap their own and read their own.
#[tokio::test]
async fn roles_and_caps_change_only_for_those_the_rules_allow() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("milo", "member")
        .build("privacy_roles")
        .await
    else {
        return;
    };
    let op = |name: &str| format!("/api/v2/w/{}/ops/{name}", world.organization);
    let owner = &world.owner.browser;
    let lena = &world.people["lena"].browser;
    let milo = world.people["milo"].actor.to_string();
    let (status, made) = send(
        &world.app,
        owner.request(
            "POST",
            &op("role_create"),
            Some("r-1"),
            json!({"name": "Helpers", "baseRole": "member", "permissions": ["tasks.view", "tasks.edit"]}),
        ),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{status} {made}");
    let role = made["value"]["id"].as_str().expect("role id").to_owned();
    let version = made["value"]["version"].clone();

    let refused = |name: &'static str, key: &'static str, body: Value| {
        let request = lena.request("POST", &op(name), Some(key), body);
        let app = world.app.clone();
        async move {
            let (status, body) = send(&app, request).await;
            assert_ne!(body["outcome"], "ok", "{name}: {status} {body}");
        }
    };
    refused(
        "role_create",
        "l-1",
        json!({"name": "Mine", "baseRole": "member", "permissions": ["tasks.view"]}),
    )
    .await;
    refused(
        "role_update",
        "l-2",
        json!({"role": role, "expectedVersion": version, "permissions": ["tasks.view", "tasks.edit", "tasks.delete"]}),
    )
    .await;
    refused("role_delete", "l-3", json!({"role": role})).await;
    refused("role_assign", "l-4", json!({"account": milo, "role": role})).await;
    refused(
        "role_assign",
        "l-5",
        json!({"account": "@lena", "role": "admin"}),
    )
    .await;
    refused(
        "app_limit_set",
        "l-6",
        json!({"account": milo, "permissions": ["tasks.view"]}),
    )
    .await;
    refused("permissions_get", "l-7", json!({"account": milo})).await;

    // Their own: a cap on their apps, and what they may do.
    let (_, capped) = send(
        &world.app,
        lena.request(
            "POST",
            &op("app_limit_set"),
            Some("l-8"),
            json!({"permissions": ["tasks.view"]}),
        ),
    )
    .await;
    assert_eq!(capped["outcome"], "ok", "{capped}");
    let (_, mine) = send(
        &world.app,
        lena.request("POST", &op("permissions_get"), None, json!({})),
    )
    .await;
    assert_eq!(mine["outcome"], "ok", "{mine}");
    // Required keys (reading pages, seeing conversations) come with any cap.
    let cap = mine["value"]["personalAgentLimit"].to_string();
    assert!(
        cap.contains("\"tasks.view\"") && !cap.contains("tasks.create"),
        "{mine}"
    );

    // Nothing of the owner's changed.
    let (name, permissions) = sqlx::query_as::<_, (String, Vec<String>)>(
        "SELECT name, permissions FROM workspace_roles WHERE id = $1::uuid",
    )
    .bind(&role)
    .fetch_one(world.pool())
    .await
    .expect("role");
    assert_eq!(name, "Helpers");
    assert!(
        !permissions.contains(&"tasks.delete".to_owned()),
        "{permissions:?}"
    );
    let held = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM organization_memberships WHERE role_id = $1::uuid",
    )
    .bind(&role)
    .fetch_one(world.pool())
    .await
    .expect("holders");
    assert_eq!(held, 0);
    let milo_cap = sqlx::query_scalar::<_, Option<Vec<String>>>(
        "SELECT agent_limit FROM organization_memberships WHERE organization_id = $1 AND actor_id = $2::uuid",
    )
    .bind(world.organization)
    .bind(&milo)
    .fetch_one(world.pool())
    .await
    .expect("cap");
    assert_eq!(milo_cap, None);
    world.finish().await;
}

/// M1.12: connected modules are managed only with `modules.manage`. A
/// role that manages settings and members but not modules lists, installs,
/// re-reads, approves and revokes nothing, and its refusal comes before the
/// module is looked up or its manifest fetched; the owner, asking the same,
/// gets past the gate.
#[tokio::test]
async fn connected_modules_are_managed_only_by_holders_of_modules_manage() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .build("privacy_connected_modules")
        .await
    else {
        return;
    };
    let op = |name: &str| format!("/api/v2/w/{}/ops/{name}", world.organization);
    let owner = &world.owner.browser;
    let lena = &world.people["lena"].browser;
    let (status, made) = send(
        &world.app,
        owner.request(
            "POST",
            &op("role_create"),
            Some("cm-role"),
            json!({"name": "Settings, not modules", "baseRole": "admin",
                   "permissions": ["projects.view", "settings.manage", "members.view"]}),
        ),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{status} {made}");
    let role = made["value"]["id"].as_str().expect("role id").to_owned();
    let (status, assigned) = send(
        &world.app,
        owner.request(
            "POST",
            &op("role_assign"),
            Some("cm-assign"),
            json!({"account": "@lena", "role": role}),
        ),
    )
    .await;
    assert_eq!(assigned["outcome"], "ok", "{status} {assigned}");

    let unknown = Uuid::now_v7().to_string();
    // An address nothing answers at: reaching the fetch at all is the
    // owner's, never the narrowed role's.
    let url = "https://modules.invalid/manifest.json";
    let calls = [
        ("connected_module_list", json!({})),
        ("connected_module_install", json!({"url": url})),
        ("connected_module_refresh", json!({"moduleId": unknown})),
        ("connected_module_approve", json!({"moduleId": unknown})),
        ("connected_module_revoke", json!({"moduleId": unknown})),
    ];
    for (index, (name, body)) in calls.iter().enumerate() {
        let key = format!("cm-lena-{index}");
        let (status, refused) = send(
            &world.app,
            lena.request("POST", &op(name), Some(&key), body.clone()),
        )
        .await;
        assert_eq!(
            refused["outcome"], "permission_denied",
            "{name} as the narrowed role: {status} {refused}"
        );
        let key = format!("cm-owner-{index}");
        let (status, answered) = send(
            &world.app,
            owner.request("POST", &op(name), Some(&key), body.clone()),
        )
        .await;
        assert_ne!(
            answered["outcome"], "permission_denied",
            "{name} as the owner: {status} {answered}"
        );
    }
    let installed =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM module_apps WHERE workspace_id = $1")
            .bind(world.organization)
            .fetch_one(world.pool())
            .await
            .expect("module apps");
    assert_eq!(installed, 0);
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
        .person("tess", "member")
        .custom_role("Read only", &["projects.view", "directory.view"])
        .custom_role(
            "Tasks, no posting",
            &[
                "projects.view",
                "directory.view",
                "tasks.view",
                "tasks.create",
            ],
        )
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
        // C2.35: nor stop its agents.
        ("reply_stop_all", json!({"conversationId": secret})),
    ] {
        let (status, body) = op(&omar.browser, name, input).await;
        assert!(!status.is_success(), "{name}: {body}");
        assert!(!body.to_string().contains("secret-plans"), "{name}: {body}");
    }
    // C2.35: Stop all takes `replies.steer` where the conversation is; a
    // member holds it, a read-only role doesn't.
    let (status, body) = op(
        &nate.browser,
        "reply_stop_all",
        json!({"conversationId": open}),
    )
    .await;
    assert!(!status.is_success(), "{body}");
    let (status, body) = op(
        &lena.browser,
        "reply_stop_all",
        json!({"conversationId": open}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
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
    // T1.28: an internal form is answered by those who may add records, as
    // themselves; a read-only role neither answers nor changes it.
    let (status, form) = op(
        &lena.browser,
        "form_create",
        json!({"collection": collection, "name": "Intake"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{form}");
    let form = form["value"]["id"].as_str().expect("form").to_owned();
    let (status, answered) = op(
        &omar.browser,
        "form_answer_send",
        json!({"form": form, "answers": {"title": "From Omar"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    for (name, input) in [
        (
            "form_answer_send",
            json!({"form": form, "answers": {"title": "From Nate"}}),
        ),
        ("form_update", json!({"form": form, "name": "Nate's"})),
        (
            "form_create",
            json!({"collection": collection, "name": "Nate's own"}),
        ),
        // T1.30: nor templates or schedules.
        (
            "record_template_create",
            json!({"collection": collection, "name": "Nate's", "title": "Nate's"}),
        ),
    ] {
        let (status, refused) = op(&nate.browser, name, input).await;
        assert!(!status.is_success(), "{name}: {refused}");
    }
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

    // C1.25, C2.22: a page, a sheet row or a sticky is made only from a
    // message its maker reads, and saying so puts a line in the channel.
    let (status, sheet) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/sheets"),
            Some(&Uuid::now_v7().to_string()),
            json!({"name": "Plans"}),
        ),
    )
    .await;
    let sheet = sheet
        .pointer("/sheet/id")
        .or_else(|| sheet.pointer("/id"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("lena makes a sheet: {status} {sheet}"))
        .to_owned();
    let (status, board) = op(
        &lena.browser,
        "whiteboard_create",
        json!({"title": "Plans", "projectId": work}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{board}");
    let board = board["value"]["whiteboard"]["id"]
        .as_str()
        .expect("board")
        .to_owned();
    let sticky = |from: &str| {
        json!({"whiteboardId": board, "fromMessageId": from, "saySo": true, "operations": [
            {"op": "add", "type": "rectangle", "text": "From the channel"},
        ]})
    };
    for (name, input) in [
        ("page_make", json!({"messages": [hidden], "saySo": true})),
        (
            "sheet_row_add",
            json!({"sheetId": sheet, "fromMessageId": hidden, "saySo": true}),
        ),
        ("whiteboard_update_elements", sticky(&hidden)),
    ] {
        let (status, refused) = op(&omar.browser, name, input).await;
        assert!(!status.is_success(), "{name}: {refused}");
        assert!(
            !refused.to_string().contains("secret-plans"),
            "{name}: {refused}"
        );
    }
    let mut made_things = Vec::new();
    for (name, input) in [
        ("page_make", json!({"messages": [shown], "saySo": true})),
        (
            "sheet_row_add",
            json!({"sheetId": sheet, "fromMessageId": shown, "saySo": true}),
        ),
        ("whiteboard_update_elements", sticky(&shown)),
    ] {
        let (status, made) = op(&omar.browser, name, input).await;
        assert_eq!(status, StatusCode::OK, "{name}: {made}");
        made_things.push(made);
    }
    let cell = sqlx::query_scalar::<_, String>(
        "SELECT cell.input FROM sheet_cells cell JOIN sheet_tabs tab ON tab.id = cell.tab_id \
         WHERE tab.sheet_id = $1::uuid AND cell.row_number = $2",
    )
    .bind(&sheet)
    .bind(
        made_things[1]["value"]["row"]
            .as_i64()
            .and_then(|row| i32::try_from(row).ok())
            .expect("row"),
    )
    .fetch_one(pool)
    .await
    .expect("the row");
    assert_eq!(cell, "Open plans launch day");
    let request = lena.browser.request(
        "GET",
        &format!("/api/v1/conversations/{open}/timeline"),
        None,
        Value::Null,
    );
    let (_, timeline) = send(app, request).await;
    let said = timeline["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter_map(|entry| entry["system"]["thingKind"].as_str())
        .collect::<Vec<_>>();
    for kind in ["page", "sheet_row", "sticky"] {
        assert!(said.contains(&kind), "a line for the {kind}: {timeline}");
    }
    // Made without saying so, a sticky still links back: the message shows
    // what it was made into, though the channel has no line for it.
    let (status, quiet_board) = op(
        &lena.browser,
        "whiteboard_create",
        json!({"title": "Quiet plans", "projectId": work}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{quiet_board}");
    let quiet_board = quiet_board["value"]["whiteboard"]["id"].clone();
    let (status, made) = op(
        &omar.browser,
        "whiteboard_update_elements",
        json!({"whiteboardId": quiet_board, "fromMessageId": shown, "operations": [
            {"op": "add", "type": "rectangle", "text": "From the channel, quietly"},
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let request = lena.browser.request(
        "GET",
        &format!("/api/v1/conversations/{open}/timeline"),
        None,
        Value::Null,
    );
    let (_, timeline) = send(app, request).await;
    let entries = timeline["entries"].as_array().expect("entries");
    assert!(
        !entries
            .iter()
            .any(|entry| entry["system"]["thingId"] == quiet_board),
        "nothing was said: {timeline}"
    );
    let chips = entries
        .iter()
        .find(|entry| entry["message"]["id"] == json!(shown))
        .map(|entry| entry["message"]["madeInto"].clone())
        .expect("the message");
    assert!(
        chips
            .as_array()
            .expect("chips")
            .contains(&json!({"kind": "sticky", "id": quiet_board})),
        "{chips}"
    );
    // Someone who may make tasks but not post in the channel makes one from
    // a message quietly: saySo posts nothing for them.
    let tess = &world.people["tess"];
    sqlx::query(
        "UPDATE project_memberships SET role_id = $1 WHERE project_id = $2 AND actor_id = $3",
    )
    .bind(Uuid::parse_str(&world.roles["Tasks, no posting"]).expect("role id"))
    .bind(Uuid::parse_str(&work).expect("work"))
    .bind(tess.actor)
    .execute(pool)
    .await
    .expect("tess's role");
    let system_entries = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM conversation_entries WHERE conversation_id = $1::uuid AND kind = 'system'",
        )
        .bind(&open)
        .fetch_one(pool)
        .await
        .expect("system entries")
    };
    let before = system_entries().await;
    let (status, quiet) = op(
        &tess.browser,
        "task_create",
        json!({"projectId": work, "title": "Quietly", "fromMessageId": shown, "saySo": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{quiet}");
    assert_eq!(
        system_entries().await,
        before,
        "a reader announced in the channel"
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

/// H1.27: only someone who may manage settings lists or sets budgets, and
/// never on a direct conversation they aren't in.
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
        ("budget_list", json!({})),
        (
            "budget_set",
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
            &route("budget_set"),
            Some("owner-secret-limit"),
            json!({"scope": "conversation", "scopeId": secret, "meter": "messages",
                   "period": "minute", "eachAccount": true, "hardLimit": 1}),
        ),
    )
    .await;
    assert_eq!(refused["outcome"], "not_found", "{status} {refused}");
    world.finish().await;
}

/// K1.27: a role given to someone's group adds to their own seat in a
/// space, in chat as everywhere else: a Viewer whose group is a Member
/// there posts in its channels.
#[tokio::test]
async fn a_group_role_adds_to_a_viewer_seat_in_chat() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("TEAM")
        .person("lena", "viewer")
        .build("privacy_group_posting")
        .await
    else {
        return;
    };
    let team = world.project("TEAM").to_owned();
    let lena = &world.people["lena"];
    let op = |browser: &Browser, name: &str, key: &str, body: Value| {
        browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/{name}", world.organization),
            Some(key),
            body,
        )
    };
    let (_, channel) = send(
        &world.app,
        op(
            &world.owner.browser,
            "channel_create",
            "owner-channel",
            json!({"projectId": team, "name": "design"}),
        ),
    )
    .await;
    assert_eq!(channel["outcome"], "ok", "{channel}");
    let channel = channel["value"]["id"].as_str().expect("channel").to_owned();
    let post = |key: &'static str| {
        op(
            &lena.browser,
            "message_send",
            key,
            json!({"conversationId": channel, "text": "hello"}),
        )
    };
    let (_, refused) = send(&world.app, post("lena-viewer")).await;
    assert_ne!(refused["outcome"], "ok", "a Viewer doesn't post: {refused}");
    for (name, key, body) in [
        (
            "group_create",
            "owner-group",
            json!({"handle": "design", "name": "Design", "members": [lena.actor]}),
        ),
        (
            "space_group_add",
            "owner-grant",
            json!({"spaceId": team, "group": "@design", "role": "member"}),
        ),
    ] {
        let (_, done) = send(&world.app, op(&world.owner.browser, name, key, body)).await;
        assert_eq!(done["outcome"], "ok", "{name}: {done}");
    }
    let (_, sent) = send(&world.app, post("lena-member")).await;
    assert_eq!(sent["outcome"], "ok", "her group's role posts: {sent}");

    world.finish().await;
}

/// K1.27: a group given a role in a space gives its members that space,
/// and taking someone out of the group takes it away; a guest sees only
/// the spaces they're added to (not open ones, the workspace's own or the
/// directory), and a membership whose time passes is turned off by the job.
/// K1.27 follow-up: a group's role in a space counts in every check, the
/// older seat-built ones too. Lena's own seat in TEAM is a Viewer's; her
/// group is a Member there, so she writes pages and tasks like a Member,
/// and once she leaves the group she can't.
#[tokio::test]
async fn a_groups_role_counts_beside_a_lower_seat() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("TEAM")
        .person("lena", "member")
        .build("privacy_group_beside_seat")
        .await
    else {
        return;
    };
    let team = world.project("TEAM").to_owned();
    let lena = &world.people["lena"];
    sqlx::query(
        "UPDATE project_memberships SET role = 'viewer', \
           role_id = (SELECT id FROM workspace_roles WHERE organization_id = $1 AND key = 'viewer') \
         WHERE project_id = $2::uuid AND actor_id = $3",
    )
    .bind(world.organization)
    .bind(&team)
    .bind(lena.actor)
    .execute(world.pool())
    .await
    .expect("lena views TEAM");
    let op = |browser: &Browser, name: &str, key: &str, body: Value| {
        browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/{name}", world.organization),
            Some(key),
            body,
        )
    };
    async fn write(app: &axum::Router, lena: &Browser, team: &str, key: &str) -> (bool, bool) {
        let (page, _) = send(
            app,
            lena.request(
                "POST",
                &format!("/api/v1/projects/{team}/pages"),
                Some(&format!("{key}-page")),
                json!({"title": format!("Plan {key}"), "body": "Notes."}),
            ),
        )
        .await;
        let (task, _) = crate::support::tasks::send(
            app,
            lena,
            "POST",
            &format!("/api/v1/projects/{team}/work-items"),
            Some(&format!("{key}-task")),
            json!({"title": format!("Task {key}")}),
        )
        .await;
        (page.is_success(), task.is_success())
    }
    assert_eq!(
        write(&world.app, &lena.browser, &team, "viewer").await,
        (false, false),
        "a Viewer seat alone"
    );

    let (_, group) = send(
        &world.app,
        op(
            &world.owner.browser,
            "group_create",
            "owner-group",
            json!({"handle": "writers", "name": "Writers", "members": [lena.actor]}),
        ),
    )
    .await;
    assert_eq!(group["outcome"], "ok", "{group}");
    let (_, granted) = send(
        &world.app,
        op(
            &world.owner.browser,
            "space_group_add",
            "owner-grant",
            json!({"spaceId": team, "group": "@writers", "role": "member"}),
        ),
    )
    .await;
    assert_eq!(granted["outcome"], "ok", "{granted}");
    assert_eq!(
        write(&world.app, &lena.browser, &team, "grouped").await,
        (true, true),
        "her group's Member role"
    );

    let (_, out) = send(
        &world.app,
        op(
            &world.owner.browser,
            "group_members_set",
            "owner-out",
            json!({"group": "writers", "remove": [lena.actor]}),
        ),
    )
    .await;
    assert_eq!(out["outcome"], "ok", "{out}");
    assert_eq!(
        write(&world.app, &lena.browser, &team, "left").await,
        (false, false),
        "out of the group"
    );

    world.finish().await;
}

#[tokio::test]
async fn groups_give_spaces_and_guests_see_only_theirs() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("TEAM")
        .project("OPEN")
        .person("lena", "member")
        .person("gus", "member")
        .build("privacy_groups_guests")
        .await
    else {
        return;
    };
    let team = world.project("TEAM").to_owned();
    let open = world.project("OPEN").to_owned();
    let team_id = team.parse::<Uuid>().expect("team");
    let lena = &world.people["lena"];
    let gus = &world.people["gus"];
    sqlx::query(
        "DELETE FROM project_memberships WHERE project_id = ANY($1) AND actor_id = ANY($2)",
    )
    .bind(vec![team_id, open.parse::<Uuid>().expect("open")])
    .bind(vec![lena.actor, gus.actor])
    .execute(world.pool())
    .await
    .expect("lena and gus start in neither");
    sqlx::query("UPDATE projects SET visibility = 'workspace' WHERE id = $1::uuid")
        .bind(&open)
        .execute(world.pool())
        .await
        .expect("OPEN is open to the workspace");
    let op = |browser: &Browser, name: &str, key: &str, body: Value| {
        browser.request(
            "POST",
            &format!("/api/v2/w/{}/ops/{name}", world.organization),
            Some(key),
            body,
        )
    };
    let opens = |browser: &Browser, space: &str, key: &str| {
        op(browser, "space_open", key, json!({"spaceId": space}))
    };

    let (_, before) = send(&world.app, opens(&lena.browser, &team, "lena-1")).await;
    assert_ne!(before["outcome"], "ok", "not in TEAM yet: {before}");
    // Making a group is "manage groups": an owner's, not a member's.
    let (_, refused) = send(
        &world.app,
        op(
            &lena.browser,
            "group_create",
            "lena-group",
            json!({"handle": "mine", "name": "Mine"}),
        ),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let (_, group) = send(
        &world.app,
        op(
            &world.owner.browser,
            "group_create",
            "owner-group",
            json!({"handle": "design", "name": "Design", "members": [lena.actor]}),
        ),
    )
    .await;
    assert_eq!(group["outcome"], "ok", "{group}");
    let (_, taken) = send(
        &world.app,
        op(
            &world.owner.browser,
            "group_create",
            "owner-group-2",
            json!({"handle": "lena", "name": "Lena"}),
        ),
    )
    .await;
    assert_eq!(taken["outcome"], "conflict", "a person's handle: {taken}");
    let (_, granted) = send(
        &world.app,
        op(
            &world.owner.browser,
            "space_group_add",
            "owner-grant",
            json!({"spaceId": team, "group": "@design", "role": "member"}),
        ),
    )
    .await;
    assert_eq!(granted["outcome"], "ok", "{granted}");
    assert_eq!(granted["value"]["seats"], 1, "{granted}");
    let (_, opened) = send(&world.app, opens(&lena.browser, &team, "lena-2")).await;
    assert_eq!(
        opened["outcome"], "ok",
        "the group gives Lena TEAM: {opened}"
    );
    // She's in through the group: removing her alone is refused.
    let (_, removal) = send(
        &world.app,
        op(
            &world.owner.browser,
            "space_member_remove",
            "owner-remove",
            json!({"spaceId": team, "account": lena.actor}),
        ),
    )
    .await;
    assert_eq!(removal["outcome"], "conflict", "{removal}");
    let (_, out) = send(
        &world.app,
        op(
            &world.owner.browser,
            "group_members_set",
            "owner-out",
            json!({"group": "design", "remove": [lena.actor]}),
        ),
    )
    .await;
    assert_eq!(out["outcome"], "ok", "{out}");
    let (_, after) = send(&world.app, opens(&lena.browser, &team, "lena-3")).await;
    assert_ne!(
        after["outcome"], "ok",
        "out of the group, out of TEAM: {after}"
    );
    // Back in, then the group taken out of the space.
    let (_, back) = send(
        &world.app,
        op(
            &world.owner.browser,
            "group_members_set",
            "owner-back",
            json!({"group": "design", "add": ["@lena"]}),
        ),
    )
    .await;
    assert_eq!(back["outcome"], "ok", "{back}");
    let (_, reopened) = send(&world.app, opens(&lena.browser, &team, "lena-4")).await;
    assert_eq!(reopened["outcome"], "ok", "{reopened}");
    // Taken out of the group while TEAM is archived: bringing TEAM back
    // doesn't bring her seat back with it.
    let (_, archived) = send(
        &world.app,
        op(
            &world.owner.browser,
            "space_archive",
            "owner-archive",
            json!({"spaceId": team}),
        ),
    )
    .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    let (_, out_while_archived) = send(
        &world.app,
        op(
            &world.owner.browser,
            "group_members_set",
            "owner-out-archived",
            json!({"group": "design", "remove": [lena.actor]}),
        ),
    )
    .await;
    assert_eq!(out_while_archived["outcome"], "ok", "{out_while_archived}");
    let (_, restored) = send(
        &world.app,
        op(
            &world.owner.browser,
            "space_restore",
            "owner-restore",
            json!({"spaceId": team}),
        ),
    )
    .await;
    assert_eq!(restored["outcome"], "ok", "{restored}");
    let (_, stale) = send(&world.app, opens(&lena.browser, &team, "lena-4b")).await;
    assert_ne!(
        stale["outcome"], "ok",
        "removed from the group while TEAM was archived: {stale}"
    );
    let (_, again) = send(
        &world.app,
        op(
            &world.owner.browser,
            "group_members_set",
            "owner-back-again",
            json!({"group": "design", "add": ["@lena"]}),
        ),
    )
    .await;
    assert_eq!(again["outcome"], "ok", "{again}");
    let (_, reopened_again) = send(&world.app, opens(&lena.browser, &team, "lena-4c")).await;
    assert_eq!(reopened_again["outcome"], "ok", "{reopened_again}");
    let (_, ungranted) = send(
        &world.app,
        op(
            &world.owner.browser,
            "space_group_remove",
            "owner-ungrant",
            json!({"spaceId": team, "group": "design"}),
        ),
    )
    .await;
    assert_eq!(ungranted["value"]["removed"], true, "{ungranted}");
    let (_, gone) = send(&world.app, opens(&lena.browser, &team, "lena-5")).await;
    assert_ne!(gone["outcome"], "ok", "{gone}");

    // Gus becomes a guest: the open space, the workspace's own and the
    // directory are closed to him.
    let (_, gus_opens) = send(&world.app, opens(&gus.browser, &open, "gus-1")).await;
    assert_eq!(
        gus_opens["outcome"], "ok",
        "a member reads an open space: {gus_opens}"
    );
    sqlx::query(
        "UPDATE organization_memberships SET role_id = \
           (SELECT id FROM workspace_roles WHERE organization_id = $1 AND key = 'guest') \
         WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(world.organization)
    .bind(gus.actor)
    .execute(world.pool())
    .await
    .expect("gus is a guest");
    let workspace_space = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM projects WHERE organization_id = $1 AND kind = 'workspace'",
    )
    .bind(world.organization)
    .fetch_one(world.pool())
    .await
    .expect("workspace space")
    .to_string();
    for (space, key) in [(&open, "gus-2"), (&workspace_space, "gus-3")] {
        let (_, closed) = send(&world.app, opens(&gus.browser, space, key)).await;
        assert_ne!(closed["outcome"], "ok", "a guest: {closed}");
    }
    let (status, members) = send(
        &world.app,
        gus.browser.request(
            "GET",
            &format!("/api/v1/organizations/{}/members", world.organization),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{members}");
    assert_eq!(
        members["members"].as_array().map(Vec::len),
        Some(1),
        "a guest sees only themselves: {members}"
    );
    let (_, listed) = send(
        &world.app,
        op(&gus.browser, "space_list", "gus-4", json!({})),
    )
    .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert!(
        !listed.to_string().contains(&open) && !listed.to_string().contains(&workspace_space),
        "{listed}"
    );

    // Added to TEAM for a while, he reads it until his time passes.
    let soon = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let (_, added) = send(
        &world.app,
        op(
            &world.owner.browser,
            "space_member_add",
            "owner-add-gus",
            json!({"spaceId": team, "account": gus.actor, "expiresAt": soon}),
        ),
    )
    .await;
    assert_eq!(added["outcome"], "ok", "{added}");
    assert!(added["value"]["expiresAt"].is_string(), "{added}");
    let (_, reads) = send(&world.app, opens(&gus.browser, &team, "gus-5")).await;
    assert_eq!(reads["outcome"], "ok", "{reads}");
    let (_, until) = send(
        &world.app,
        op(
            &world.owner.browser,
            "workspace_member_update",
            "owner-expire-gus",
            json!({"account": "@gus", "expiresAt": soon}),
        ),
    )
    .await;
    assert_eq!(until["outcome"], "ok", "{until}");
    sqlx::query(
        "UPDATE project_memberships SET expires_at = now() - interval '1 minute' \
         WHERE project_id = $1 AND actor_id = $2",
    )
    .bind(team_id)
    .bind(gus.actor)
    .execute(world.pool())
    .await
    .expect("his time in TEAM passes");
    agent_workspace_server::memberships::expiry::expire_due(world.pool(), 10)
        .await
        .expect("expire");
    let (_, expired) = send(&world.app, opens(&gus.browser, &team, "gus-6")).await;
    assert_ne!(expired["outcome"], "ok", "{expired}");
    sqlx::query(
        "UPDATE organization_memberships SET expires_at = now() - interval '1 minute' \
         WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(world.organization)
    .bind(gus.actor)
    .execute(world.pool())
    .await
    .expect("his time in the workspace passes");
    agent_workspace_server::memberships::expiry::expire_due(world.pool(), 10)
        .await
        .expect("expire");
    let enabled = sqlx::query_scalar::<_, bool>(
        "SELECT enabled FROM organization_memberships WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(world.organization)
    .bind(gus.actor)
    .fetch_one(world.pool())
    .await
    .expect("membership");
    assert!(!enabled, "an expired workspace membership is turned off");

    // An end date is a removal on a timer: an admin can't set one on
    // another admin, as they can't remove them; an owner can.
    sqlx::query(
        "UPDATE organization_memberships SET enabled = true, expires_at = NULL, role = 'admin', \
           role_id = (SELECT id FROM workspace_roles WHERE organization_id = $1 AND key = 'admin') \
         WHERE organization_id = $1 AND actor_id = ANY($2)",
    )
    .bind(world.organization)
    .bind(vec![lena.actor, gus.actor])
    .execute(world.pool())
    .await
    .expect("lena and gus run the workspace");
    let (_, admin_on_admin) = send(
        &world.app,
        op(
            &lena.browser,
            "workspace_member_update",
            "lena-expire-gus",
            json!({"account": "@gus", "expiresAt": soon}),
        ),
    )
    .await;
    assert_ne!(admin_on_admin["outcome"], "ok", "{admin_on_admin}");
    let (_, owner_on_admin) = send(
        &world.app,
        op(
            &world.owner.browser,
            "workspace_member_update",
            "owner-expire-gus-admin",
            json!({"account": "@gus", "expiresAt": soon}),
        ),
    )
    .await;
    assert_eq!(owner_on_admin["outcome"], "ok", "{owner_on_admin}");

    // An owner's membership lasts: taking the workspace clears gus's time,
    // and an owner's time that passes anyway never turns them off.
    let (status, transferred) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!(
                "/api/v1/organizations/{}/transfer-ownership",
                world.organization
            ),
            None,
            json!({"actorId": gus.actor}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{transferred}");
    let cleared = sqlx::query_scalar::<_, Option<chrono::DateTime<chrono::Utc>>>(
        "SELECT expires_at FROM organization_memberships \
         WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(world.organization)
    .bind(gus.actor)
    .fetch_one(world.pool())
    .await
    .expect("gus's membership");
    assert!(cleared.is_none(), "the new owner's time is cleared");
    sqlx::query(
        "UPDATE organization_memberships SET expires_at = now() - interval '1 minute' \
         WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(world.organization)
    .bind(gus.actor)
    .execute(world.pool())
    .await
    .expect("an owner's time passes");
    agent_workspace_server::memberships::expiry::expire_due(world.pool(), 10)
        .await
        .expect("expire");
    let still = sqlx::query_scalar::<_, bool>(
        "SELECT enabled FROM organization_memberships WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(world.organization)
    .bind(gus.actor)
    .fetch_one(world.pool())
    .await
    .expect("membership");
    assert!(still, "an owner's membership never expires");

    world.finish().await;
}

/// T1.23: triage decisions need `tasks.edit`, and a space's triage rules
/// and queue `tasks.workflow`, whatever the account. A viewer reads the
/// rules but triages nothing and writes no rule; the owner does both.
#[tokio::test]
async fn triage_keeps_to_the_edit_and_workflow_permissions() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("TRI")
        .person("vera", "viewer")
        .agent("rook", "viewer")
        .connected_app("laptop")
        .build("privacy_triage")
        .await
    else {
        return;
    };
    let app = &world.app;
    let (rook, laptop) = (&world.agents["rook"], &world.apps["laptop"]);
    let as_owner = |name: &'static str, arguments: Value| async move {
        laptop.call(app, Call::Tool { name, arguments }).await.1
    };
    let as_rook = |name: &'static str, arguments: Value| async move {
        structured(
            &call_tool(app, &rook.token, &rook.session, name, arguments)
                .await
                .1,
        )
    };
    let enabled = as_rook("triage_enable", json!({"spaceId": "TRI"})).await;
    assert_eq!(enabled["outcome"], "permission_denied", "{enabled}");
    let enabled = as_owner("triage_enable", json!({"spaceId": "TRI"})).await;
    assert_eq!(enabled["outcome"], "ok", "{enabled}");
    let made = as_owner(
        "task_create",
        json!({"projectId": "TRI", "title": "Arrived", "status": "Triage", "clientRequestId": "tri-1"}),
    )
    .await;
    let key = made["value"]["key"].as_str().expect("key").to_owned();
    let rule = json!({"spaceId": "TRI", "name": "Keep", "action": {"kind": "keep"}});

    let listed = as_rook("triage_rule_list", json!({"spaceId": "TRI"})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let refused = as_rook("triage_rule_set", rule.clone()).await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let accept = json!({"taskIds": [key], "action": "accept"});
    let refused = as_rook("triage_decide", accept.clone()).await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let (status, refused) = send(
        app,
        world.people["vera"].browser.request(
            "POST",
            "/api/v2/ops/triage_decide",
            Some("privacy-triage-vera"),
            accept.clone(),
        ),
    )
    .await;
    assert_eq!(
        refused["outcome"], "permission_denied",
        "{status} {refused}"
    );

    let written = as_owner("triage_rule_set", rule).await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let accepted = as_owner("triage_decide", accept).await;
    assert_eq!(accepted["outcome"], "ok", "{accepted}");

    // Cycles (T1.24): the same split. A viewer reads them, sets nothing
    // up and puts no task in one.
    let refused = as_rook("cycle_set", json!({"spaceId": "TRI", "enabled": true})).await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let enabled = as_owner("cycle_set", json!({"spaceId": "TRI", "enabled": true})).await;
    assert_eq!(enabled["outcome"], "ok", "{enabled}");
    let listed = as_rook("cycle_list", json!({"spaceId": "TRI"})).await;
    assert_eq!(
        listed["value"]["cycles"].as_array().map(Vec::len),
        Some(3),
        "{listed}"
    );
    let opened = as_owner("task_get", json!({"taskId": key})).await;
    let planned = json!({"taskId": key, "expectedVersion": opened["value"]["task"]["version"], "cycle": "current"});
    let refused = as_rook("task_update", planned.clone()).await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let planned = as_owner("task_update", planned).await;
    assert_eq!(planned["outcome"], "ok", "{planned}");

    // Project status (T1.43): a viewer reads it, posts no update and sets
    // nothing; an update in a private channel they aren't in stays out of
    // what they read.
    let private = as_owner(
        "channel_create",
        json!({"projectId": "TRI", "name": "leads", "private": true}),
    )
    .await;
    assert_eq!(private["outcome"], "ok", "{private}");
    let posted = as_owner(
        "project_update_create",
        json!({"spaceId": "TRI", "health": "off_track", "text": "Quietly off track.",
               "channelId": private["value"]["id"], "clientRequestId": "tri-update"}),
    )
    .await;
    assert_eq!(posted["outcome"], "ok", "{posted}");
    let read = as_rook("project_status_get", json!({"spaceId": "TRI"})).await;
    assert_eq!(read["value"]["health"], "off_track", "{read}");
    assert!(read["value"]["latestUpdate"].is_null(), "{read}");
    let refused = as_rook(
        "project_update_create",
        json!({"spaceId": "TRI", "health": "on_track", "text": "Fine.",
               "clientRequestId": "rook-update"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let refused = as_rook(
        "project_status_set",
        json!({"spaceId": "TRI", "health": "on_track"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");

    // Initiatives (T1.26) are records of the workspace's own space: a
    // viewer there reads them and starts none.
    let started = as_owner(
        "initiative_create",
        json!({"title": "Owner's plan", "clientRequestId": "owner-plan"}),
    )
    .await;
    assert_eq!(started["outcome"], "ok", "{started}");
    sqlx::query(
        "UPDATE project_memberships SET role = 'viewer', role_id = NULL \
         WHERE actor_id = $1 \
           AND project_id = (SELECT project_id FROM collections WHERE role = 'initiatives')",
    )
    .bind(rook.actor)
    .execute(world.pool())
    .await
    .expect("viewer in the workspace space");
    // Milestones (T1.42): a viewer can't reorder them.
    let made = as_owner(
        "milestone_create",
        json!({"projectId": "TRI", "name": "Ordered", "clientRequestId": "ordered"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let refused = as_rook(
        "milestone_reorder",
        json!({"milestoneId": made["value"]["id"], "position": 1}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let refused = as_rook(
        "initiative_create",
        json!({"title": "Viewer's plan", "clientRequestId": "viewer-plan"}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let listed = as_rook("initiative_list", json!({})).await;
    assert_eq!(
        listed["value"]["initiatives"].as_array().map(Vec::len),
        Some(1),
        "{listed}"
    );
    world.finish().await;
}

/// Outside data (T1.34, T1.35) keeps to the roles that connect it: someone
/// without data.connect doesn't see connections, draft queries or any
/// query's SQL, and can't change, review, run or remove a draft; a role
/// that connects data but doesn't share it can't share an answer.
#[tokio::test]
async fn outside_data_keeps_to_roles_that_connect_it() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("mara", "member")
        .custom_role_on("Connects data", "admin", &["projects.view", "data.connect"])
        .person("cleo", "member")
        .build("privacy_outside_data")
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
    .bind(Uuid::parse_str(&world.roles["Connects data"]).expect("role id"))
    .bind(work)
    .bind(world.people["cleo"].actor)
    .execute(pool)
    .await
    .expect("narrow");
    let owner = &world.owner.browser;
    let (_, made) = op(
        owner,
        "data_connection_create",
        json!({"spaceId": work, "name": "Warehouse",
               "address": "postgresql://reader:hunter2@db.example.com/sales"}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    assert!(!made.to_string().contains("hunter2"), "{made}");
    let connection = made["value"]["id"].clone();
    let (_, saved) = op(
        owner,
        "data_query_create",
        json!({"spaceId": work, "connection": connection, "name": "Secret sauce",
               "statement": "SELECT 'secret-sauce-sql' AS recipe"}),
    )
    .await;
    assert_eq!(saved["outcome"], "ok", "{saved}");
    let query = saved["value"]["id"].clone();

    let mara = &world.people["mara"].browser;
    let (_, listed) = op(mara, "data_query_list", json!({"spaceId": work})).await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    assert!(!listed.to_string().contains("Secret sauce"), "{listed}");
    for (name, input) in [
        ("data_connection_list", json!({"spaceId": work})),
        (
            "data_connection_update",
            json!({"connection": connection, "name": "Mine"}),
        ),
        ("data_connection_delete", json!({"connection": connection})),
        ("data_query_get", json!({"query": query})),
        (
            "data_query_update",
            json!({"query": query, "statement": "SELECT 2"}),
        ),
        (
            "data_query_review",
            json!({"query": query, "state": "reviewed"}),
        ),
        ("data_query_run", json!({"query": query})),
        ("data_query_delete", json!({"query": query})),
    ] {
        let (status, body) = op(mara, name, input).await;
        assert_ne!(body["outcome"], "ok", "mara {name}: {status} {body}");
        assert!(!body.to_string().contains("secret-sauce-sql"), "{body}");
    }

    // Cleo connects data but doesn't share it.
    let cleo = &world.people["cleo"].browser;
    let (_, reviewed) = op(
        cleo,
        "data_query_review",
        json!({"query": query, "state": "reviewed"}),
    )
    .await;
    assert_eq!(reviewed["outcome"], "ok", "{reviewed}");
    let (_, refused) = op(
        cleo,
        "data_query_share",
        json!({"query": query, "shared": true}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");

    // Shared by the owner, its answer is mara's to see, its SQL still not.
    let (_, shared) = op(
        owner,
        "data_query_share",
        json!({"query": query, "shared": true}),
    )
    .await;
    assert_eq!(shared["outcome"], "ok", "{shared}");
    let (_, seen) = op(mara, "data_query_get", json!({"query": query})).await;
    assert_eq!(seen["outcome"], "ok", "{seen}");
    assert!(!seen.to_string().contains("secret-sauce-sql"), "{seen}");
    let (_, still) = op(
        mara,
        "data_query_update",
        json!({"query": query, "name": "Mara's"}),
    )
    .await;
    assert_ne!(still["outcome"], "ok", "{still}");
    world.finish().await;
}

/// T1.41: a role without edit on records changes none, a suggest-only role
/// only proposes, and neither decides a proposal or puts a version back.
#[tokio::test]
async fn record_changes_keep_to_roles_that_edit_records() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("nell", "member")
        .person("sage", "member")
        .connected_app("Desk")
        .custom_role("Reads only", &["projects.view", "directory.view"])
        .custom_role(
            "Suggests",
            &[
                "projects.view",
                "collections.suggest",
                "requests.raise",
                "directory.view",
            ],
        )
        .build("privacy_record_changes")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let workspace = world.organization;
    let work = Uuid::parse_str(world.project("WORK")).expect("work");
    for (person, role) in [("nell", "Reads only"), ("sage", "Suggests")] {
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
    let desk = &world.apps["Desk"];
    let tool = |name: &'static str, arguments: Value| async move {
        desk.call(app, Call::Tool { name, arguments }).await.1
    };
    let made = tool(
        "collection_create",
        json!({"projectId": "WORK", "name": "Accounts", "fields": [{"name": "Tier", "type": "text"}]}),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let written = tool(
        "record_create_many",
        json!({"projectId": "WORK", "collection": "Accounts", "clientRequestId": "acct",
               "records": [{"title": "Globex", "fields": {"Tier": "gold"}}]}),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let record = written["value"]["records"][0]["id"].clone();
    let op = |browser: &Browser, name: &'static str, input: Value| {
        let request = browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/{name}"),
            Some(&Uuid::now_v7().to_string()),
            input,
        );
        async move { send(app, request).await }
    };
    let (nell, sage) = (&world.people["nell"].browser, &world.people["sage"].browser);
    let (_, refused) = op(
        nell,
        "record_update",
        json!({"record": record, "fields": {"Tier": "lead"}}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let (_, proposed) = op(
        sage,
        "record_update",
        json!({"record": record, "fields": {"Tier": "platinum"}}),
    )
    .await;
    assert_eq!(proposed["outcome"], "ok", "{proposed}");
    let proposal = proposed["value"]["proposalId"].clone();
    assert!(proposal.is_string(), "{proposed}");
    for (who, browser) in [("nell", nell), ("sage", sage)] {
        let (_, decided) = op(
            browser,
            "proposal_decide",
            json!({"proposalId": proposal, "decision": "accept"}),
        )
        .await;
        assert_ne!(decided["outcome"], "ok", "{who}: {decided}");
        let (_, restored) = op(
            browser,
            "history_restore",
            json!({"thing": record, "version": 1, "expectedVersion": 1}),
        )
        .await;
        assert_ne!(restored["outcome"], "ok", "{who}: {restored}");
        // Nor upload a file to it (T1.20): that's editing.
        let (status, uploaded) = send(
            app,
            browser.request(
                "POST",
                &format!(
                    "/api/v1/things/record/{}/files?name=sneaky.txt",
                    record.as_str().expect("id")
                ),
                None,
                json!("sneaky"),
            ),
        )
        .await;
        assert_eq!(status.as_u16(), 404, "{who}: {uploaded}");
    }
    let files = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM files WHERE thing_kind = 'record' AND thing_id = $1::uuid",
    )
    .bind(record.as_str().expect("id"))
    .fetch_one(pool)
    .await
    .expect("files");
    assert_eq!(files, 0);
    let tier: Value =
        sqlx::query_scalar("SELECT field_values FROM collection_records WHERE id = $1::uuid")
            .bind(record.as_str().expect("id"))
            .fetch_one(pool)
            .await
            .expect("record");
    assert!(tier.to_string().contains("gold"), "{tier}");
    world.finish().await;
}

/// A2.9: following and ambient mode keep to what the caller may see and
/// do. A member can't follow a DM they aren't in, nor set ambient mode
/// there; a viewer (no `replies.steer`) can't set it in a channel; and what
/// the member follows never lists the DM.
#[tokio::test]
async fn following_and_ambient_mode_keep_to_what_the_caller_may_see() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("vera", "viewer")
        .agent("rook", "member")
        .build("privacy_follows")
        .await
    else {
        return;
    };
    let rook = world.agents["rook"].actor;
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
    let (status, made) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v1/projects/{}/channels", world.project("WORK")),
            Some("privacy-follows-channel"),
            json!({ "name": "general" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let channel = made["conversation"]["id"]
        .as_str()
        .or_else(|| made["id"].as_str())
        .expect("channel")
        .to_owned();
    let op = |name: &str| format!("/api/v2/w/{}/ops/{name}", world.organization);
    let lena = &world.people["lena"].browser;
    let vera = &world.people["vera"].browser;
    for (who, name, body) in [
        (
            lena,
            "follow_set",
            json!({ "kind": "conversation", "id": dm, "level": "everything" }),
        ),
        (
            lena,
            "conversation_agent_settings_set",
            json!({ "conversation": dm, "account": rook, "ambient": true }),
        ),
        (
            vera,
            "conversation_agent_settings_set",
            json!({ "conversation": channel, "account": rook, "ambient": true }),
        ),
        // A2.15: only the DM's participants say which apps hear it.
        (
            lena,
            "app_consent_set",
            json!({ "conversation": dm, "app": rook }),
        ),
        // A2.27: guards are set by those who manage the workspace's settings.
        (
            lena,
            "guard_set",
            json!({ "name": "Mine", "action": "deny", "accounts": [rook] }),
        ),
        // A2.14: budgets are set by those who manage the workspace's
        // settings; a DM someone isn't in isn't one they can budget.
        (
            lena,
            "budget_set",
            json!({ "scope": "account", "scopeId": rook, "meter": "messages",
                    "period": "day", "hardLimit": 1 }),
        ),
        // A2.16: a viewer doesn't write a channel's memory.
        (
            vera,
            "memory_page_set",
            json!({ "conversation": channel, "body": "Viewer's note." }),
        ),
    ] {
        let key = Uuid::now_v7().to_string();
        let (status, body) =
            send(&world.app, who.request("POST", &op(name), Some(&key), body)).await;
        assert_ne!(body["outcome"], "ok", "{name}: {status} {body}");
    }
    let (_, listed) = send(
        &world.app,
        lena.request("POST", &op("follow_list"), None, json!({})),
    )
    .await;
    assert!(!listed.to_string().contains(&dm), "{listed}");
    let settings = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM conversation_agent_settings")
        .fetch_one(world.pool())
        .await
        .expect("settings");
    assert_eq!(settings, 0);
    world.finish().await;
}

/// A2.16: a private channel's memory page is as private as the channel.
/// Someone in the space but not the channel can't open it, find it in the
/// space's pages or search, or read it by its id; a space's memory is the
/// space's.
#[tokio::test]
async fn a_private_channels_memory_stays_with_its_members() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .agent("rook", "member")
        .build("privacy_memory")
        .await
    else {
        return;
    };
    let work = world.project("WORK");
    let owner = &world.owner.browser;
    let lena = &world.people["lena"].browser;
    let (status, made) = send(
        &world.app,
        owner.request(
            "POST",
            &format!("/api/v1/projects/{work}/channels"),
            Some("privacy-memory-channel"),
            json!({ "name": "plans", "private": true }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{made}");
    let channel = made["conversation"]["id"]
        .as_str()
        .expect("channel")
        .to_owned();
    let op = |name: &str| format!("/api/v2/w/{}/ops/{name}", world.organization);
    let word = format!("zqmemory{}", &Uuid::now_v7().simple().to_string()[20..]);
    let key = Uuid::now_v7().to_string();
    let (status, written) = send(
        &world.app,
        owner.request(
            "POST",
            &op("memory_page_set"),
            Some(&key),
            json!({ "conversation": channel, "body": format!("Launch is {word}.") }),
        ),
    )
    .await;
    assert_eq!(written["outcome"], "ok", "{status} {written}");
    let page = written["value"]["pageId"]
        .as_str()
        .expect("page")
        .to_owned();
    let key = Uuid::now_v7().to_string();
    let (_, space_memory) = send(
        &world.app,
        owner.request(
            "POST",
            &op("memory_page_set"),
            Some(&key),
            json!({ "space": work, "body": "Ship on Fridays." }),
        ),
    )
    .await;
    assert_eq!(space_memory["outcome"], "ok", "{space_memory}");

    let (_, opened) = send(
        &world.app,
        owner.request(
            "POST",
            &op("memory_page_open"),
            None,
            json!({ "conversation": channel }),
        ),
    )
    .await;
    assert_eq!(opened["value"]["page"]["pageId"], page.as_str(), "{opened}");

    let (_, refused) = send(
        &world.app,
        lena.request(
            "POST",
            &op("memory_page_open"),
            None,
            json!({ "conversation": channel }),
        ),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    let key = Uuid::now_v7().to_string();
    let (_, refused) = send(
        &world.app,
        lena.request(
            "POST",
            &op("memory_page_set"),
            Some(&key),
            json!({ "conversation": channel, "body": "Overwritten." }),
        ),
    )
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    let (status, read) = send(
        &world.app,
        lena.request("GET", &format!("/api/v1/pages/{page}"), None, Value::Null),
    )
    .await;
    assert!(!status.is_success(), "{status} {read}");
    // Nor change its blocks through an agent tool, which would answer with
    // them.
    let (_, edited) = world.agents["rook"]
        .call(
            &world.app,
            Call::Tool {
                name: "page_update_blocks",
                arguments: json!({
                    "pageId": page,
                    "edits": [{ "op": "append", "markdown": "Overwritten." }],
                }),
            },
        )
        .await;
    assert_ne!(edited["outcome"], "ok", "{edited}");
    assert!(!edited.to_string().contains(&word), "{edited}");
    // Nor a file on it, even by its id.
    let file = upload(
        &world.app,
        owner,
        &format!("/api/v1/things/page/{page}/files?name=plan.txt"),
        "text/plain",
        format!("Plan: {word}").into_bytes(),
    )
    .await;
    for path in [
        format!("/api/v1/files/{file}"),
        format!("/api/v1/files/{file}/content"),
    ] {
        let (status, read) = send(&world.app, lena.request("GET", &path, None, Value::Null)).await;
        assert!(!status.is_success(), "{path}: {status} {read}");
        assert!(!read.to_string().contains(&word), "{path}: {read}");
    }
    // Nor list it among the space's Pages records, by any view of them.
    let (_, records) = world.agents["rook"]
        .call(
            &world.app,
            Call::Tool {
                name: "record_list",
                arguments: json!({ "projectId": "WORK", "collection": "Pages" }),
            },
        )
        .await;
    assert_eq!(records["outcome"], "ok", "{records}");
    assert!(!records.to_string().contains(&page), "{records}");
    assert!(!records.to_string().contains("plans"), "{records}");
    // Nor its live document, its discussion or an export of it.
    for path in ["document", "discussion", "export?format=markdown"] {
        let (status, read) = send(
            &world.app,
            lena.request(
                "GET",
                &format!("/api/v1/pages/{page}/{path}"),
                None,
                Value::Null,
            ),
        )
        .await;
        assert!(!status.is_success(), "{path}: {status} {read}");
        assert!(!read.to_string().contains(&word), "{path}: {read}");
    }
    let (status, read) = send(
        &world.app,
        owner.request(
            "GET",
            &format!("/api/v1/pages/{page}/document"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert!(status.is_success(), "{status} {read}");
    // Nor any of its versions.
    let (status, read) = send(
        &world.app,
        lena.request(
            "GET",
            &format!("/api/v1/pages/{page}/revisions/1"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert!(!status.is_success(), "{status} {read}");
    assert!(!read.to_string().contains(&word), "{read}");
    let (status, read) = send(
        &world.app,
        owner.request(
            "GET",
            &format!("/api/v1/pages/{page}/revisions/1"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert!(status.is_success(), "{status} {read}");
    let (_, listed) = send(
        &world.app,
        lena.request(
            "GET",
            &format!("/api/v1/projects/{work}/pages"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert!(!listed.to_string().contains(&page), "{listed}");
    let (_, found) = send(
        &world.app,
        lena.request(
            "GET",
            &format!("/api/v1/search?q={word}"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert!(!found.to_string().contains(&word), "{found}");
    // Made a template by a reader, it still isn't hers: not listed, not
    // copied into a page of her own.
    let (status, marked) = send(
        &world.app,
        owner.request(
            "PATCH",
            &format!("/api/v1/pages/{page}"),
            None,
            json!({ "isTemplate": true }),
        ),
    )
    .await;
    assert!(status.is_success(), "{status} {marked}");
    let (_, templates) = send(
        &world.app,
        lena.request(
            "POST",
            &op("page_template_list"),
            None,
            json!({ "spaceId": work }),
        ),
    )
    .await;
    assert_eq!(templates["outcome"], "ok", "{templates}");
    assert!(!templates.to_string().contains(&page), "{templates}");
    assert!(!templates.to_string().contains(&word), "{templates}");
    let (_, copied) = send(
        &world.app,
        lena.request(
            "POST",
            &format!("/api/v1/projects/{work}/pages"),
            None,
            json!({ "fromTemplateId": page }),
        ),
    )
    .await;
    assert!(!copied.to_string().contains(&word), "{copied}");
    // The space's memory is the space's.
    let (_, opened) = send(
        &world.app,
        lena.request(
            "POST",
            &op("memory_page_open"),
            None,
            json!({ "space": work }),
        ),
    )
    .await;
    assert_eq!(
        opened["value"]["page"]["body"], "Ship on Fridays.",
        "{opened}"
    );
    world.finish().await;
}

/// App accounts are workspace members like people and agents: the Members
/// page lists them, and an owner changes their role and turns them off
/// through the same controls. The workspace isn't handed to one, since an
/// owner has to sign in.
#[tokio::test]
async fn an_owner_manages_an_app_account_like_any_member() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .build("privacy_app_member_managed")
        .await
    else {
        return;
    };
    let app_account = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO actors (id, kind, account_kind, status, owner_workspace_id) \
         VALUES (gen_random_uuid(), 'integration', 'app', 'active', $1) RETURNING id",
    )
    .bind(world.organization)
    .fetch_one(world.pool())
    .await
    .expect("app account");
    sqlx::query(
        "INSERT INTO organization_memberships (organization_id, actor_id, role) \
         VALUES ($1, $2, 'member')",
    )
    .bind(world.organization)
    .bind(app_account)
    .execute(world.pool())
    .await
    .expect("the app joins");
    let version = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT version FROM organization_memberships \
             WHERE organization_id = $1 AND actor_id = $2",
        )
        .bind(world.organization)
        .bind(app_account)
        .fetch_one(world.pool())
        .await
        .expect("membership version")
    };
    let member = format!(
        "/api/v1/organizations/{}/members/{app_account}",
        world.organization
    );
    let expected = version().await;
    let (status, changed) = send(
        &world.app,
        world.owner.browser.request(
            "PATCH",
            &member,
            None,
            json!({"expectedVersion": expected, "role": "admin"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "role change: {changed}");
    // But the workspace can't be handed to it: an owner has to sign in.
    let (status, handed) = send(
        &world.app,
        world.owner.browser.request(
            "POST",
            &format!(
                "/api/v1/organizations/{}/transfer-ownership",
                world.organization
            ),
            None,
            json!({"actorId": app_account}),
        ),
    )
    .await;
    assert!(
        status.is_client_error(),
        "handed to an app: {status} {handed}"
    );
    let (_, listed) = send(
        &world.app,
        world.owner.browser.request(
            "GET",
            &format!("/api/v1/organizations/{}/members", world.organization),
            None,
            Value::Null,
        ),
    )
    .await;
    let entry = listed["members"]
        .as_array()
        .and_then(|members| {
            members
                .iter()
                .find(|member| member["actorId"] == app_account.to_string())
        })
        .cloned()
        .unwrap_or_default();
    assert_eq!(entry["mayOwn"], false, "{listed}");
    let expected = version().await;
    let (status, removed) = send(
        &world.app,
        world.owner.browser.request(
            "PATCH",
            &member,
            None,
            json!({"expectedVersion": expected, "enabled": false}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "turned off: {removed}");
    let (role, enabled) = sqlx::query_as::<_, (String, bool)>(
        "SELECT role, enabled FROM organization_memberships \
         WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(world.organization)
    .bind(app_account)
    .fetch_one(world.pool())
    .await
    .expect("membership after");
    assert_eq!((role.as_str(), enabled), ("admin", false));
    world.finish().await;
}

/// C2.37: a guest reads only the channels they were added to. Another open
/// channel in the same space isn't listed, opened, joined or found for them,
/// and its mentions don't reach them; a member still reads it unasked.
#[tokio::test]
async fn a_guest_reads_only_the_channels_they_were_added_to() {
    let Some(world) = super::support::world::WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("gus", "member")
        .person("morgan", "member")
        .build("privacy_guest_channels")
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
    let gus = &world.people["gus"];
    let mo = &world.people["morgan"];
    sqlx::query(
        "UPDATE organization_memberships SET role_id = \
           (SELECT id FROM workspace_roles WHERE organization_id = $1 AND key = 'guest') \
         WHERE organization_id = $1 AND actor_id = $2",
    )
    .bind(workspace)
    .bind(gus.actor)
    .execute(pool)
    .await
    .expect("gus is a guest");
    let made = |name: &'static str| {
        let request = lena.browser.request(
            "POST",
            &format!("/api/v1/projects/{work}/channels"),
            Some(&Uuid::now_v7().to_string()),
            json!({"name": name}),
        );
        async move {
            let (status, made) = send(app, request).await;
            assert_eq!(status, StatusCode::CREATED, "{made}");
            made["conversation"]["id"].as_str().expect("id").to_owned()
        }
    };
    let room = made("guest-room").await;
    let lobby = made("team-lobby").await;
    sqlx::query(
        "INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id) \
         VALUES ($1::uuid, $2::uuid, $3, $4)",
    )
    .bind(&work)
    .bind(&room)
    .bind(gus.actor)
    .bind(lena.actor)
    .execute(pool)
    .await
    .expect("gus joins the guest room");

    let (status, listed) = op(&gus.browser, "channel_list", json!({"spaceId": work})).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(listed.to_string().contains(&room), "{listed}");
    assert!(!listed.to_string().contains(&lobby), "{listed}");
    for (name, input) in [
        ("channel_get", json!({"channelId": lobby})),
        ("channel_join", json!({"channelId": lobby})),
        ("conversation_member_list", json!({"conversationId": lobby})),
    ] {
        let (status, body) = op(&gus.browser, name, input).await;
        assert!(!status.is_success(), "{name}: {body}");
    }
    let (status, places) = op(&gus.browser, "place_search", json!({"q": "lobby"})).await;
    assert_eq!(status, StatusCode::OK, "{places}");
    assert!(!places.to_string().contains(&lobby), "{places}");
    // A member reads the open channel without joining it.
    let (status, listed) = op(&mo.browser, "channel_list", json!({"spaceId": work})).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(listed.to_string().contains(&lobby), "{listed}");

    // @everyone in the lobby reaches the member, never the guest.
    let request = lena.browser.request(
        "POST",
        &format!("/api/v1/conversations/{lobby}/messages"),
        Some(&Uuid::now_v7().to_string()),
        json!({"content": text("@everyone standup")}),
    );
    let (status, sent) = send(app, request).await;
    assert!(status.is_success(), "{sent}");
    agent_workspace_server::events::catch_up(pool)
        .await
        .expect("catch up with the log");
    let lobby_id = &lobby;
    let notified = |actor: Uuid| async move {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM notifications WHERE recipient_actor_id = $1 AND conversation_id = $2::uuid",
        )
        .bind(actor)
        .bind(lobby_id)
        .fetch_one(pool)
        .await
        .expect("notifications")
    };
    assert_eq!(notified(mo.actor).await, 1);
    assert_eq!(notified(gus.actor).await, 0);
    world.finish().await;
}
