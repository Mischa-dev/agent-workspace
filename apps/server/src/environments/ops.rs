//! Environments as operations (K2.18): list a space's environments, add,
//! change, archive or check one. Each runs the command the hand-written
//! tools ran (`mcp::launch_tools::environment_list`,
//! `mcp::environment_tools::environment_write`), as the caller the
//! dispatcher checked, which keeps its own checks inside: changing takes
//! the right to manage the space's settings, checking the right to run
//! there. Those that change something write their own audit row. The old
//! `environment_write` tool is an alias.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    mcp::{
        environment_tools::{self, EnvironmentWriteArguments, EnvironmentWriteValue},
        launch_tools::{self, EnvironmentListValue, ProjectArguments},
        task_commands::{principal_for, tool_answer},
    },
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::parse_id,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<EnvironmentList>()
        .add::<EnvironmentCreate>()
        .add::<EnvironmentUpdate>()
        .add::<EnvironmentArchive>()
        .add::<EnvironmentCheck>();
}

fn space(project_id: &str) -> Target {
    Target::Space(parse_id(project_id).unwrap_or_else(Uuid::nil))
}

/// `environment_write` with only what one operation sets.
fn write_arguments(project_id: String, environment: Option<String>) -> EnvironmentWriteArguments {
    EnvironmentWriteArguments {
        project_id,
        environment,
        name: None,
        harness: None,
        provider_environment_id: None,
        repository_id: None,
        branch: None,
        setup_notes: None,
        make_default: None,
        archive: None,
        check: None,
        expected_version: None,
    }
}

async fn write(
    call: &mut OpCall<'_>,
    arguments: EnvironmentWriteArguments,
    audited: bool,
) -> Result<EnvironmentWriteValue, OpError> {
    let principal = principal_for(call.ctx, None);
    let value =
        tool_answer(environment_tools::environment_write(call.state, &principal, arguments).await)?;
    if audited {
        call.audited_by_command();
    }
    Ok(value)
}

// ---------------------------------------------------------------------------
// environment_list

pub(crate) struct EnvironmentList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SpaceInput {
    /// The space's ID, link or key (such as "AWD").
    pub project_id: String,
}

impl Targeted for SpaceInput {
    fn target(&self) -> Target {
        space(&self.project_id)
    }
}

impl Operation for EnvironmentList {
    const NAME: &'static str = "environment_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List environments";
    const DESCRIPTION: &'static str = "Where a space's runs can execute: each environment's \
        harness, repository, branch, setup notes and last check, and which is the default.";

    type Input = SpaceInput;
    type Output = EnvironmentListValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: SpaceInput,
    ) -> Result<EnvironmentListValue, OpError> {
        let principal = principal_for(call.ctx, None);
        tool_answer(
            launch_tools::environment_list(
                call.state,
                &principal,
                ProjectArguments {
                    project_id: Some(input.project_id),
                },
            )
            .await,
        )
    }
}

// ---------------------------------------------------------------------------
// environment_create

pub(crate) struct EnvironmentCreate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateInput {
    /// The space's ID, link or key.
    pub project_id: String,
    pub name: String,
    /// The harness that runs in it, by kind (environment_list shows those
    /// in use).
    pub harness: String,
    /// The provider's own environment ID, for a harness that runs in the
    /// cloud.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_environment_id: Option<String>,
    /// A repository of the space (repository_list), by ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<String>,
    /// The branch runs start from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Notes on setting it up, up to 4000 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_notes: Option<String>,
    /// Make it the space's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub make_default: Option<bool>,
}

impl Targeted for CreateInput {
    fn target(&self) -> Target {
        space(&self.project_id)
    }
}

impl Operation for EnvironmentCreate {
    const NAME: &'static str = "environment_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = Some(Perm::SettingsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Add environment";
    const DESCRIPTION: &'static str = "Add an environment where a space's runs execute: a \
        name and a harness, and optionally a repository, branch and setup notes. Takes a role \
        that manages the space's settings.";
    const EMITS: &'static [&'static str] = &["environment.created"];

    type Input = CreateInput;
    type Output = EnvironmentWriteValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: CreateInput,
    ) -> Result<EnvironmentWriteValue, OpError> {
        let mut arguments = write_arguments(input.project_id, None);
        arguments.name = Some(input.name);
        arguments.harness = Some(input.harness);
        arguments.provider_environment_id = input.provider_environment_id;
        arguments.repository_id = input.repository_id;
        arguments.branch = input.branch;
        arguments.setup_notes = input.setup_notes;
        arguments.make_default = input.make_default;
        write(call, arguments, true).await
    }
}

// ---------------------------------------------------------------------------
// environment_update

pub(crate) struct EnvironmentUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UpdateInput {
    /// The space's ID, link or key.
    pub project_id: String,
    /// The environment, by ID or name (environment_list).
    pub environment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// An empty string clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_environment_id: Option<String>,
    /// An empty string clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<String>,
    /// An empty string clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_notes: Option<String>,
    /// Make it the space's default (alone, or with other changes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub make_default: Option<bool>,
    /// The version you last read. Defaults to the current one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<i64>,
}

impl Targeted for UpdateInput {
    fn target(&self) -> Target {
        space(&self.project_id)
    }
}

impl Operation for EnvironmentUpdate {
    const NAME: &'static str = "environment_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = Some(Perm::SettingsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Update environment";
    const DESCRIPTION: &'static str = "Change an environment where a space's runs execute, or \
        make it the default (makeDefault). Takes a role that manages the space's settings.";
    const EMITS: &'static [&'static str] = &["environment.updated"];

    type Input = UpdateInput;
    type Output = EnvironmentWriteValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: UpdateInput,
    ) -> Result<EnvironmentWriteValue, OpError> {
        let mut arguments = write_arguments(input.project_id, Some(input.environment));
        arguments.name = input.name;
        arguments.harness = input.harness;
        arguments.provider_environment_id = input.provider_environment_id;
        arguments.repository_id = input.repository_id;
        arguments.branch = input.branch;
        arguments.setup_notes = input.setup_notes;
        arguments.make_default = input.make_default;
        arguments.expected_version = input.expected_version;
        write(call, arguments, true).await
    }
}

// ---------------------------------------------------------------------------
// environment_archive, environment_check

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct EnvironmentInput {
    /// The space's ID, link or key.
    pub project_id: String,
    /// The environment, by ID or name (environment_list).
    pub environment: String,
}

impl Targeted for EnvironmentInput {
    fn target(&self) -> Target {
        space(&self.project_id)
    }
}

pub(crate) struct EnvironmentArchive;

impl Operation for EnvironmentArchive {
    const NAME: &'static str = "environment_archive";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = Some(Perm::SettingsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Archive environment";
    const DESCRIPTION: &'static str = "Archive an environment; a default that pointed here \
        falls back to the workspace's. Takes a role that manages the space's settings.";
    const EMITS: &'static [&'static str] = &["environment.archived"];

    type Input = EnvironmentInput;
    type Output = EnvironmentWriteValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: EnvironmentInput,
    ) -> Result<EnvironmentWriteValue, OpError> {
        let mut arguments = write_arguments(input.project_id, Some(input.environment));
        arguments.archive = Some(true);
        write(call, arguments, true).await
    }
}

pub(crate) struct EnvironmentCheck;

impl Operation for EnvironmentCheck {
    const NAME: &'static str = "environment_check";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "runs";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Check environment";
    const DESCRIPTION: &'static str = "Check an environment with your own signed-in account \
        for its harness, and record the result (checkOk, checkMessage).";
    const EMITS: &'static [&'static str] = &["environment.checked"];

    type Input = EnvironmentInput;
    type Output = EnvironmentWriteValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: EnvironmentInput,
    ) -> Result<EnvironmentWriteValue, OpError> {
        let mut arguments = write_arguments(input.project_id, Some(input.environment));
        arguments.check = Some(true);
        write(call, arguments, false).await
    }
}
