//! K2.2: the operation kernel's pipeline, through a test-only operation
//! (`echo_create`, over a scratch table) so it is checked before any real
//! module's operations exist: a retried request id replays (across any
//! clock edge), a reused one with other input is refused, two ids with the
//! same input both run, a write without an id runs once and says it isn't
//! retry-safe (ruling R52), a caller outside the workspace finds
//! nothing, a read-only credential can't write, and every answer matches the
//! operation's envelope schema. `ops_edges` calls the same test operations
//! through every edge.

use std::{collections::BTreeMap, sync::Arc};

use agent_workspace_server::{
    AppState,
    config::AppConfig,
    ops::{
        self, Caller, DispatchRequest, Edge, Idempotency, OpCall, OpError, OpKind, Operation,
        Registry, Target, Targeted, Unkeyed, envelope, refs::IdRef, registry::register_for_tests,
    },
    policy::{ActorKind, Permission},
    provider::SupervisorRegistry,
    rate_limit::RateLimiters,
    realtime::RealtimeHub,
};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::mcp_admin::{Setup, create_project, setup};

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EchoInput {
    space: IdRef,
    text: String,
}

impl Targeted for EchoInput {
    fn target(&self) -> Target {
        Target::Space(self.space.id())
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct EchoOutput {
    #[schemars(with = "String")]
    id: Uuid,
    text: String,
}

struct EchoCreate;

impl Operation for EchoCreate {
    const NAME: &'static str = "echo_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Permission> = Some(Permission::PagesEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Echo";
    const DESCRIPTION: &'static str = "Writes its text to a scratch table (tests only).";

    type Input = EchoInput;
    type Output = EchoOutput;

    async fn run(call: &mut OpCall<'_>, input: EchoInput) -> Result<EchoOutput, OpError> {
        let id = Uuid::now_v7();
        let tx = call.tx().await?;
        sqlx::query("INSERT INTO ops_kernel_echo (id, text) VALUES ($1, $2)")
            .bind(id)
            .bind(&input.text)
            .execute(&mut **tx)
            .await
            .map_err(|_| OpError::Unavailable)?;
        Ok(EchoOutput {
            id,
            text: input.text,
        })
    }
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct EchoNoteInput {
    text: String,
}

impl Targeted for EchoNoteInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

/// A test operation whose target doesn't say which workspace it acts in.
struct EchoNote;

impl Operation for EchoNote {
    const NAME: &'static str = "echo_note";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "pages";
    const PERMISSION: Option<Permission> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Echo note";
    const DESCRIPTION: &'static str =
        "Writes its text to a scratch table in a workspace (tests only).";

    type Input = EchoNoteInput;
    type Output = EchoOutput;

    async fn run(call: &mut OpCall<'_>, input: EchoNoteInput) -> Result<EchoOutput, OpError> {
        let id = Uuid::now_v7();
        let tx = call.tx().await?;
        sqlx::query("INSERT INTO ops_kernel_echo (id, text) VALUES ($1, $2)")
            .bind(id)
            .bind(&input.text)
            .execute(&mut **tx)
            .await
            .map_err(|_| OpError::Unavailable)?;
        Ok(EchoOutput {
            id,
            text: input.text,
        })
    }
}

/// The test operations, for `register_for_tests`.
pub(super) fn add_echo(registry: &mut Registry) {
    registry.add::<EchoCreate>();
    registry.add::<EchoNote>();
}

/// The scratch table the test operations write to.
pub(super) async fn scratch_table(pool: &sqlx::PgPool) {
    sqlx::query("CREATE TABLE ops_kernel_echo (id UUID PRIMARY KEY, text TEXT NOT NULL)")
        .execute(pool)
        .await
        .expect("scratch table");
}

fn state(setup: &Setup) -> AppState {
    let database_url = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database");
    let config = AppConfig::from_values(&BTreeMap::from([(
        "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
        database_url,
    )]))
    .expect("configuration");
    let supervisors = SupervisorRegistry::new(&config).expect("supervisors");
    AppState {
        config: Arc::new(config),
        pool: setup.pool.clone(),
        rate_limits: RateLimiters::default(),
        realtime: RealtimeHub::new(8),
        supervisors,
    }
}

fn mcp_edge() -> Edge {
    Edge::Mcp {
        credential_id: None,
        oauth_grant_id: None,
        connection_id: None,
        client_name: "kernel test".to_owned(),
        authentication_class: "oauth_access_token",
    }
}

/// `ops::dispatch` as a request runs it: in its caller's scope.
async fn dispatch(
    state: &agent_workspace_server::AppState,
    request: DispatchRequest,
    name: &str,
    input: Value,
) -> ops::Answer {
    let account = request.caller.account_id;
    crate::support::as_member(account, ops::dispatch(state, request, name, input)).await
}

fn request(caller: Caller, request_id: Option<&str>) -> DispatchRequest {
    DispatchRequest {
        caller,
        edge: mcp_edge(),
        trace_id: Some("trace-1".to_owned()),
        workspace: None,
        on_screen: None,
        request_id: request_id.map(str::to_owned),
        unkeyed: Unkeyed::RunOnce,
        preview: false,
        defer_long: false,
    }
}

async fn rows(setup: &Setup) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM ops_kernel_echo")
        .fetch_one(&setup.pool)
        .await
        .expect("count")
}

#[tokio::test]
async fn the_kernel_replays_refuses_and_hides_what_it_should() {
    let Some(setup) = setup("k2_ops_kernel").await else {
        return;
    };
    register_for_tests(add_echo);
    scratch_table(&setup.pool).await;
    let state = state(&setup);
    let space = create_project(&setup, "OPS", "Operations").await;
    let owner: Uuid = sqlx::query_scalar(
        "SELECT actor_id FROM organization_memberships WHERE organization_id = $1 AND role = 'owner'",
    )
    .bind(setup.organization_id)
    .fetch_one(&setup.pool)
    .await
    .expect("owner");
    let person = Caller::person(owner, ActorKind::Human);
    let schema = envelope::schema(
        (ops::registry()
            .get("echo_create")
            .expect("registered")
            .meta
            .output_schema)(),
        false,
    );
    let matches_schema = |answer: &Value| {
        let outcome = answer["outcome"].clone();
        assert!(
            schema["oneOf"]
                .as_array()
                .expect("branches")
                .iter()
                .any(|branch| {
                    let named = &branch["properties"]["outcome"];
                    named["const"] == outcome
                        || named["enum"]
                            .as_array()
                            .is_some_and(|outcomes| outcomes.contains(&outcome))
                }),
            "{answer}"
        );
    };

    // A write runs once; the same request id answers the same again.
    let input = json!({ "space": space, "text": "hello" });
    let first = dispatch(
        &state,
        request(person.clone(), Some("echo-1")),
        "echo_create",
        input.clone(),
    )
    .await;
    assert!(first.is_ok(), "{:?}", first.body);
    assert_eq!(first.body["replayed"], false);
    assert_eq!(first.body["requestId"], "echo-1");
    matches_schema(&first.body);
    let again = dispatch(
        &state,
        request(person.clone(), Some("echo-1")),
        "echo_create",
        input.clone(),
    )
    .await;
    assert!(again.is_ok(), "{:?}", again.body);
    assert_eq!(again.body["replayed"], true);
    assert_eq!(again.body["value"]["id"], first.body["value"]["id"]);
    assert_eq!(rows(&setup).await, 1);

    // The audit row names the operation and its edge.
    let (action, channel, target_kind, correlation): (String, String, String, Option<String>) =
        sqlx::query_as(
            "SELECT action, channel, target_kind, correlation_id FROM audit_events \
             WHERE action = 'echo_create'",
        )
        .fetch_one(&setup.pool)
        .await
        .expect("audit row");
    assert_eq!(
        (action.as_str(), channel.as_str(), target_kind.as_str()),
        ("echo_create", "mcp", "project")
    );
    assert_eq!(correlation.as_deref(), Some("trace-1"));

    // The same request id with other input is a conflict, not a replay.
    let reused = dispatch(
        &state,
        request(person.clone(), Some("echo-1")),
        "echo_create",
        json!({ "space": space, "text": "something else" }),
    )
    .await;
    assert_eq!(reused.outcome(), "conflict", "{:?}", reused.body);
    assert_eq!(
        reused.body["code"], "request_id_reused",
        "{:?}",
        reused.body
    );
    matches_schema(&reused.body);

    // A replay doesn't depend on the clock: the same id an hour later (the
    // reservation aged past any bucket edge) still answers once.
    sqlx::query(
        "UPDATE idempotency_keys SET created_at = created_at - interval '1 hour' \
         WHERE key = 'echo-1'",
    )
    .execute(&setup.pool)
    .await
    .expect("age the reservation");
    let later = dispatch(
        &state,
        request(person.clone(), Some("echo-1")),
        "echo_create",
        input.clone(),
    )
    .await;
    assert_eq!(later.body["replayed"], true, "{:?}", later.body);
    assert_eq!(later.body["value"]["id"], first.body["value"]["id"]);
    assert_eq!(rows(&setup).await, 1);

    // Two ids with the same input are two changes.
    let second = dispatch(
        &state,
        request(person.clone(), Some("echo-1b")),
        "echo_create",
        input.clone(),
    )
    .await;
    assert!(second.is_ok(), "{:?}", second.body);
    assert_eq!(second.body["replayed"], false);
    assert_ne!(second.body["value"]["id"], first.body["value"]["id"]);
    assert_eq!(rows(&setup).await, 2);

    // Without a request id (MCP, v1), a write runs once and says a retry
    // isn't safe: an identical second call is a second change, never a
    // guessed replay.
    let unkeyed = dispatch(
        &state,
        request(person.clone(), None),
        "echo_create",
        json!({ "space": space, "text": "unkeyed" }),
    )
    .await;
    let again_unkeyed = dispatch(
        &state,
        request(person.clone(), None),
        "echo_create",
        json!({ "space": space, "text": "unkeyed" }),
    )
    .await;
    assert!(
        unkeyed.is_ok() && again_unkeyed.is_ok(),
        "{:?} {:?}",
        unkeyed.body,
        again_unkeyed.body
    );
    assert_eq!(unkeyed.body["replaySafe"], false, "{:?}", unkeyed.body);
    assert_eq!(again_unkeyed.body["replayed"], false);
    assert!(
        unkeyed.body.get("requestId").is_none(),
        "{:?}",
        unkeyed.body
    );
    assert_eq!(first.body["replaySafe"], true, "{:?}", first.body);
    matches_schema(&unkeyed.body);
    assert_eq!(rows(&setup).await, 4);

    // An edge that requires one (the web app, v2) refuses a write without.
    let mut required = request(person.clone(), None);
    required.unkeyed = Unkeyed::Refuse("Idempotency-Key");
    let refused_unkeyed = dispatch(
        &state,
        required,
        "echo_create",
        json!({ "space": space, "text": "nope" }),
    )
    .await;
    assert_eq!(
        refused_unkeyed.outcome(),
        "invalid_request",
        "{:?}",
        refused_unkeyed.body
    );
    assert_eq!(refused_unkeyed.body["field"], "Idempotency-Key");
    assert_eq!(rows(&setup).await, 4);

    // A bad input names its field.
    let invalid = dispatch(
        &state,
        request(person.clone(), Some("echo-2")),
        "echo_create",
        json!({ "space": space }),
    )
    .await;
    assert_eq!(invalid.outcome(), "invalid_request", "{:?}", invalid.body);
    assert_eq!(invalid.body["field"], "text");
    matches_schema(&invalid.body);

    // Someone outside the workspace finds nothing there.
    let outsider = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', 'active')")
        .bind(outsider)
        .execute(&setup.pool)
        .await
        .expect("outsider");
    let hidden = dispatch(
        &state,
        request(Caller::person(outsider, ActorKind::Human), Some("echo-3")),
        "echo_create",
        json!({ "space": space, "text": "intruder" }),
    )
    .await;
    assert_eq!(hidden.outcome(), "not_found", "{:?}", hidden.body);
    assert_eq!(hidden.status.as_u16(), 404);

    // A named workspace the space isn't in is not found either.
    let mut elsewhere = request(person.clone(), Some("echo-4"));
    elsewhere.workspace = Some(Uuid::now_v7());
    let wrong = dispatch(
        &state,
        elsewhere,
        "echo_create",
        json!({ "space": space, "text": "x" }),
    )
    .await;
    assert_eq!(wrong.outcome(), "not_found", "{:?}", wrong.body);

    // A read-only credential can't write.
    let mut read_only = person.clone();
    read_only.can_write = false;
    let refused = dispatch(
        &state,
        request(read_only, Some("echo-5")),
        "echo_create",
        json!({ "space": space, "text": "nope" }),
    )
    .await;
    assert_eq!(refused.outcome(), "permission_denied", "{:?}", refused.body);
    assert_eq!(
        refused.body["code"], "read_only_credential",
        "{:?}",
        refused.body
    );
    matches_schema(&refused.body);

    // An app acting for the person is held to their agent limit: it
    // reaches the space, with nothing allowed there.
    let mut limited = person.clone();
    let none = agent_workspace_server::access::catalogue::PermissionSet::EMPTY;
    limited.limits = Some(agent_workspace_server::policy::AgentLimits {
        actor_id: owner,
        organizations: [(setup.organization_id, none)].into_iter().collect(),
        projects: [(space.parse::<Uuid>().expect("space"), none)]
            .into_iter()
            .collect(),
    });
    let narrowed = dispatch(
        &state,
        request(limited, Some("echo-6")),
        "echo_create",
        json!({ "space": space, "text": "limited" }),
    )
    .await;
    assert_eq!(
        narrowed.outcome(),
        "permission_denied",
        "{:?}",
        narrowed.body
    );

    // An unknown operation is not found.
    let unknown = dispatch(&state, request(person, None), "no_such_op", json!({})).await;
    assert_eq!(unknown.outcome(), "not_found");
    assert_eq!(rows(&setup).await, 4);
}
