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

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    message_edits::{say, text},
    support::{
        app::{Browser, connect_app, initialize, send},
        two_workspaces::{
            TwoWorkspaces, World, add_person, login_as, string_at, two_workspaces, upload,
        },
        world::Setup,
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
    sqlx::query("UPDATE work_items SET title = title || $2 WHERE id = $1")
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
