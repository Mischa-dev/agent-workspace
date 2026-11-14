//! A workspace to test in: one owner signed in on a fresh schema
//! (`setup`), and worlds of every kind of caller built on it.

use std::collections::BTreeMap;

use agent_workspace_server::{
    AppState,
    bootstrap::{BootstrapCommand, bootstrap_admin},
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

use super::{
    app::{Browser, PASSWORD, app, connect_app, create_agent, initialize, login, send, test_app},
    callers::{Agent, Call, Caller, ConnectedApp, Person, assert_absent, drain_outbox},
    db::{Db, db, schema_pool},
};
use crate::{
    mcp_tasks::{add_member, space},
    message_edits::{add_person, set_role},
};

pub(crate) struct Setup {
    pub(crate) app: axum::Router,
    pub(crate) browser: Browser,
    pub(crate) organization_id: Uuid,
    /// The test's own schema, for checking rows directly.
    pub(crate) pool: PgPool,
}

pub(crate) async fn setup(prefix: &str) -> Option<Setup> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let admin = PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    let schema = format!("{prefix}_{}", Uuid::now_v7().simple());
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("create schema");
    let pool = schema_pool(&database_url, &schema).await;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply migrations");
    let owner = bootstrap_admin(
        &pool,
        BootstrapCommand {
            login_name: "owner".to_owned(),
            display_name: "Owner".to_owned(),
            organization_name: Some("Admin Organization".to_owned()),
            organization_slug: Some(format!("admin-{}", Uuid::now_v7().simple())),
            adopt_actor_id: None,
            organization_id: None,
        },
        PASSWORD.to_owned(),
    )
    .await
    .expect("bootstrap owner");
    let app = test_app(pool.clone(), &database_url);
    let browser = login(&app).await;
    Some(Setup {
        app,
        browser,
        organization_id: owner.organization_id,
        pool,
    })
}

pub(crate) async fn create_project(setup: &Setup, key: &str, name: &str) -> String {
    let (status, project) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/projects",
            Some(&format!("create-{key}")),
            json!({"organizationId": setup.organization_id, "key": key, "name": name}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{project}");
    project["id"].as_str().expect("project").to_owned()
}

/// A workspace with every kind of caller in it, built by [`WorldBuilder`].
/// Projects, people, agents and conversations are named by the keys and
/// handles the test gave.
pub(crate) struct Workspace {
    pub(crate) app: axum::Router,
    pub(crate) state: AppState,
    pub(crate) organization: Uuid,
    pub(crate) owner: Person,
    pub(crate) projects: BTreeMap<String, String>,
    pub(crate) people: BTreeMap<String, Person>,
    pub(crate) agents: BTreeMap<String, Agent>,
    pub(crate) apps: BTreeMap<String, ConnectedApp>,
    pub(crate) roles: BTreeMap<String, String>,
    pub(crate) conversations: BTreeMap<String, String>,
    db: Db,
}

impl Workspace {
    pub(crate) fn pool(&self) -> &PgPool {
        &self.db.pool
    }

    pub(crate) fn project(&self, key: &str) -> &str {
        self.projects
            .get(key)
            .unwrap_or_else(|| panic!("no project {key}"))
    }

    /// Drops the workspace's database.
    pub(crate) async fn finish(self) {
        drop(self.app);
        self.db.finish().await;
    }
}

/// Builds a [`Workspace`]: `.project("WORK")`, `.person("lena", "member")`,
/// `.agent("rook", "member")`, `.connected_app("codex")` (the owner's),
/// `.custom_role("Contractor", &["tasks.view"])`, `.dm("lena", "mara")` and
/// `.private_group("ops", &["lena", "mara"])`. People and agents join every
/// project with their role; a custom role is the workspace's, by name.
#[derive(Default)]
pub(crate) struct WorldBuilder {
    projects: Vec<String>,
    people: Vec<(String, String)>,
    agents: Vec<(String, String)>,
    apps: Vec<String>,
    custom_roles: Vec<(String, Vec<String>)>,
    dms: Vec<(String, String)>,
    groups: Vec<(String, Vec<String>)>,
}

impl WorldBuilder {
    pub(crate) fn project(mut self, key: &str) -> Self {
        self.projects.push(key.to_owned());
        self
    }

    pub(crate) fn person(mut self, handle: &str, role: &str) -> Self {
        self.people.push((handle.to_owned(), role.to_owned()));
        self
    }

    pub(crate) fn agent(mut self, handle: &str, role: &str) -> Self {
        self.agents.push((handle.to_owned(), role.to_owned()));
        self
    }

    pub(crate) fn connected_app(mut self, name: &str) -> Self {
        self.apps.push(name.to_owned());
        self
    }

    pub(crate) fn custom_role(mut self, name: &str, permissions: &[&str]) -> Self {
        self.custom_roles.push((
            name.to_owned(),
            permissions.iter().map(|&value| value.to_owned()).collect(),
        ));
        self
    }

    pub(crate) fn dm(mut self, a: &str, b: &str) -> Self {
        self.dms.push((a.to_owned(), b.to_owned()));
        self
    }

    pub(crate) fn private_group(mut self, name: &str, members: &[&str]) -> Self {
        self.groups.push((
            name.to_owned(),
            members.iter().map(|&value| value.to_owned()).collect(),
        ));
        self
    }

    /// None (and a note) when no database is configured.
    pub(crate) async fn build(self, name: &str) -> Option<Workspace> {
        let db = db(name).await?;
        let owner = bootstrap_admin(
            &db.pool,
            BootstrapCommand {
                login_name: "owner".to_owned(),
                display_name: "Owner".to_owned(),
                organization_name: Some("World".to_owned()),
                organization_slug: Some(format!("world-{}", Uuid::now_v7().simple())),
                adopt_actor_id: None,
                organization_id: None,
            },
            PASSWORD.to_owned(),
        )
        .await
        .expect("bootstrap owner");
        let (app, state) = app(&db).build();
        let setup = Setup {
            app: app.clone(),
            browser: login(&app).await,
            organization_id: owner.organization_id,
            pool: db.pool.clone(),
        };
        let mut projects = BTreeMap::new();
        for key in &self.projects {
            projects.insert(key.clone(), create_project(&setup, key, key).await);
        }
        let project_ids = projects.values().map(String::as_str).collect::<Vec<_>>();
        // The workspace's space holds its DMs and groups, so everyone is in it.
        let space = space(&setup).await;
        let mut people = BTreeMap::new();
        for (handle, role) in &self.people {
            let mut joins = project_ids.clone();
            joins.push(&space);
            let (actor, browser) = add_person(&setup, &joins, handle).await;
            for project in &project_ids {
                set_role(&setup.pool, project, actor, role).await;
            }
            people.insert(handle.clone(), Person { actor, browser });
        }
        let mut agents = BTreeMap::new();
        for (handle, role) in &self.agents {
            let (actor, token) = create_agent(&app, &setup.browser, &space, handle, handle).await;
            for project in &project_ids {
                add_member(&setup, project, actor, role).await;
            }
            let connection = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM external_agent_connections WHERE actor_id = $1 AND status = 'active' ORDER BY created_at LIMIT 1",
            )
            .bind(actor)
            .fetch_one(&setup.pool)
            .await
            .expect("agent connection");
            let session = initialize(&app, &token).await.expect("agent MCP session");
            agents.insert(
                handle.clone(),
                Agent {
                    actor,
                    connection,
                    token,
                    session,
                },
            );
        }
        let mut apps = BTreeMap::new();
        for name in &self.apps {
            let token = connect_app(&app, &setup.browser, name, "full").await;
            let session = initialize(&app, &token).await.expect("app MCP session");
            apps.insert(
                name.clone(),
                ConnectedApp {
                    person: owner.actor_id,
                    token,
                    session,
                },
            );
        }
        let mut roles = BTreeMap::new();
        for (name, permissions) in &self.custom_roles {
            let (status, role) = send(
                &app,
                setup.browser.request(
                    "POST",
                    &format!("/api/v1/organizations/{}/roles", owner.organization_id),
                    None,
                    json!({"name": name, "baseRole": "member", "permissions": permissions}),
                ),
            )
            .await;
            assert!(status.is_success(), "{role}");
            let id = role["role"]["id"]
                .as_str()
                .or_else(|| role["id"].as_str())
                .unwrap_or_else(|| panic!("role id in {role}"));
            roles.insert(name.clone(), id.to_owned());
        }
        let actor_of = |handle: &str| {
            people
                .get(handle)
                .map(|person| person.actor)
                .or_else(|| agents.get(handle).map(|agent| agent.actor))
                .unwrap_or_else(|| panic!("no person or agent {handle}"))
        };
        let mut conversations = BTreeMap::new();
        for (a, b) in &self.dms {
            let opener = people
                .get(a)
                .unwrap_or_else(|| panic!("a DM is opened by a person: {a}"));
            let (status, dm) = send(
                &app,
                opener.browser.request(
                    "POST",
                    "/api/v1/direct/dms",
                    None,
                    json!({"actorId": actor_of(b)}),
                ),
            )
            .await;
            assert!(status.is_success(), "{dm}");
            let id = dm["conversation"]["id"].as_str().expect("DM").to_owned();
            conversations.insert(format!("{a}+{b}"), id);
        }
        for (name, members) in &self.groups {
            let member_ids = members
                .iter()
                .map(|handle| actor_of(handle))
                .collect::<Vec<_>>();
            let (status, group) = send(
                &app,
                setup.browser.request(
                    "POST",
                    "/api/v1/direct/groups",
                    Some(&format!("group-{name}")),
                    json!({"name": name, "memberIds": member_ids}),
                ),
            )
            .await;
            assert!(status.is_success(), "{group}");
            let id = group["conversation"]["id"]
                .as_str()
                .expect("group")
                .to_owned();
            conversations.insert(name.clone(), id);
        }
        Some(Workspace {
            owner: Person {
                actor: owner.actor_id,
                browser: setup.browser,
            },
            app,
            state,
            organization: owner.organization_id,
            projects,
            people,
            agents,
            apps,
            roles,
            conversations,
            db,
        })
    }
}

/// A person, an agent and a connected app each make the same read; each
/// answers, and the person's and the app's agree (the app acts as them).
#[tokio::test]
async fn a_world_has_every_kind_of_caller() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .agent("rook", "member")
        .connected_app("codex")
        .custom_role("Contractor", &["tasks.view"])
        .dm("lena", "mara")
        .private_group("ops", &["lena", "rook"])
        .build("world_callers")
        .await
    else {
        return;
    };
    let app = &world.app;
    let (status, by_http) = world
        .owner
        .call(
            app,
            Call::Http {
                method: "GET",
                path: format!("/api/v1/projects/{}", world.project("WORK")),
                body: Value::Null,
            },
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{by_http}");
    let read = || Call::Tool {
        name: "project_open",
        arguments: json!({"projectId": "WORK"}),
    };
    let (_, by_app) = world.apps["codex"].call(app, read()).await;
    assert_eq!(by_app["outcome"], "ok", "{by_app}");
    let (_, by_agent) = world.agents["rook"].call(app, read()).await;
    assert_eq!(by_agent["outcome"], "ok", "{by_agent}");
    assert_eq!(
        by_app["value"]["project"]["id"],
        by_agent["value"]["project"]["id"]
    );
    let (status, by_api) = world.agents["rook"]
        .call(
            app,
            Call::PublicApi {
                tool: "project_open",
                arguments: json!({"projectId": "WORK"}),
            },
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{by_api}");
    let (status, by_person) = world.people["lena"]
        .call(
            app,
            Call::Http {
                method: "GET",
                path: format!("/api/v1/projects/{}", world.project("WORK")),
                body: Value::Null,
            },
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{by_person}");
    assert_absent(&by_person, &["correct horse battery staple"]);
    // The private group is hidden from the person outside it.
    let (_, opened) = world.people["mara"]
        .call(
            app,
            Call::Http {
                method: "GET",
                path: format!("/api/v1/conversations/{}", world.conversations["ops"]),
                body: Value::Null,
            },
        )
        .await;
    assert_absent(&opened, &["\"ops\""]);
    assert!(world.conversations.contains_key("lena+mara"));
    assert!(world.roles.contains_key("Contractor"));
    drain_outbox(&world.state).await;
    world.finish().await;
}
