//! Panels (M1.14): the panels a connected module declares, listed by slot
//! (`module_panel_list`), asked for (`module_panel_open`) and acted on
//! (`module_panel_press`). Asking and acting post to the panel's entry
//! under the module's base address, signed as remote operations are, and
//! the module answers with a panel to draw, errors for its form, `done` or
//! `close`; `blocks::read_answer` checks the answer before it is passed on.
//! A request waits 10 seconds at most, holds no transaction while it waits
//! (rule 14) and spends the viewer's panel budget.

use std::time::Duration;

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::blocks::{self, Answer};
use super::remote::{Posted, endpoint_of, message_of, post_signed};
use super::{APP_COLUMNS, AppRow};
use crate::{
    error::AppError,
    limits::LimitKey,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

const PANEL_TIMEOUT: Duration = Duration::from_secs(10);
/// A panel is 64 KB; its answer may carry a little more around it.
const MAX_ANSWER_BYTES: usize = blocks::MAX_PANEL_BYTES + 16 * 1024;
const THE_PANEL: &str = "That panel";

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<ModulePanelList>()
        .add::<ModulePanelOpen>()
        .add::<ModulePanelPress>()
        .add::<super::frames::ModuleFrameCheck>();
}

/// The live installs in a workspace whose panels may be drawn: active, not
/// archived, switched on there, and not switched off in `space`.
async fn drawable(
    connection: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    space_id: Option<Uuid>,
    module_id: Option<Uuid>,
) -> Result<Vec<AppRow>, AppError> {
    Ok(sqlx::query_as::<_, AppRow>(&format!(
        "SELECT {APP_COLUMNS} FROM module_apps app \
         JOIN modules module ON module.id = app.module_id \
         WHERE app.workspace_id = $1 AND app.state = 'active' \
           AND module.archived_at IS NULL \
           AND ($3::uuid IS NULL OR app.module_id = $3) \
           AND EXISTS (SELECT 1 FROM module_installs switch \
                       WHERE switch.workspace_id = app.workspace_id \
                         AND switch.space_id IS NULL \
                         AND switch.module_key = module.module_key AND switch.state = 'on') \
           AND ($2::uuid IS NULL OR NOT EXISTS ( \
                SELECT 1 FROM module_installs off \
                WHERE off.workspace_id = app.workspace_id AND off.space_id = $2 \
                  AND off.module_key = module.module_key AND off.state = 'off')) \
         ORDER BY app.created_at, app.module_id"
    ))
    .bind(workspace_id)
    .bind(space_id)
    .bind(module_id)
    .fetch_all(connection)
    .await?)
}

/// Whether `module` may be drawn in `workspace` and declares `panel` (one
/// it answers for, not a frame-only one).
pub(crate) async fn declares(
    connection: &mut sqlx::PgConnection,
    workspace_id: Uuid,
    module_id: Uuid,
    panel: &str,
) -> Result<bool, AppError> {
    let Some(install) = drawable(connection, workspace_id, None, Some(module_id))
        .await?
        .into_iter()
        .next()
    else {
        return Ok(false);
    };
    Ok(install
        .manifest()?
        .panels
        .iter()
        .any(|spec| spec.id == panel && spec.entry.is_some() && !spec.frame_only))
}

/// Whether a panel in `slot` is drawn where the call is: a space's tab
/// needs a space, a thing's side a thing of its kind.
fn slot_fits(slot: &str, space: Option<Uuid>, thing: Option<&PanelThingInput>) -> bool {
    if let Some(kind) = slot.strip_prefix("thing.side:") {
        return thing.is_some_and(|thing| thing.kind == kind);
    }
    match slot {
        "space.tab" | "conversation.tab" => space.is_some(),
        _ => true,
    }
}

// ------------------------------------------------------------------ list

pub struct ModulePanelList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModulePanelListInput {
    /// Only panels for this slot, such as `space.tab` or `thing.side:acme.build`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<String>,
    /// The space they'd be drawn in; left out, the workspace's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
}

impl Targeted for ModulePanelListInput {
    fn target(&self) -> Target {
        match &self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PanelEntry {
    #[schemars(with = "String")]
    pub module_id: Uuid,
    /// The module's key (`publisher.module`).
    pub module_key: String,
    /// The panel's id in its manifest.
    pub panel: String,
    pub slot: String,
    pub title: String,
    /// Its layout part, `panel.<module key>.<panel>`, which layouts and
    /// roles hide like any other part.
    pub part: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModulePanelListView {
    pub panels: Vec<PanelEntry>,
}

impl Operation for ModulePanelList {
    const NAME: &'static str = "module_panel_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    // Anyone who can read where the panels are drawn sees them listed.
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List connected module panels";
    const DESCRIPTION: &'static str = "List the panels the outside apps connected here draw, \
        for one slot (a space's tab, a thing's side, the home page, settings) or every slot.";

    type Input = ModulePanelListInput;
    type Output = ModulePanelListView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ModulePanelListInput,
    ) -> Result<ModulePanelListView, OpError> {
        let workspace = call.ctx.workspace_id;
        let space = input.space_id.map(|space| space.id());
        let tx = call.tx().await?;
        let mut panels = Vec::new();
        for install in drawable(tx, workspace, space, None).await? {
            let manifest = install.manifest()?;
            for panel in manifest.panels {
                // Framed panels come with M1.15.
                if panel.entry.is_none() || panel.frame_only {
                    continue;
                }
                if input.slot.as_deref().is_some_and(|slot| slot != panel.slot) {
                    continue;
                }
                panels.push(PanelEntry {
                    module_id: install.module_id,
                    part: format!("panel.{}.{}", install.module_key, panel.id),
                    module_key: install.module_key.clone(),
                    panel: panel.id,
                    slot: panel.slot,
                    title: panel.title,
                });
            }
        }
        Ok(ModulePanelListView { panels })
    }
}

// ------------------------------------------------------------ open, press

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PanelThingInput {
    /// The thing's kind (`task`, `acme.build`).
    pub kind: String,
    pub id: IdRef,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PanelActionInput {
    /// The action's id, as the panel named it.
    pub id: String,
    /// A form's values, or what the action carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Map<String, Value>>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModulePanelOpenInput {
    #[schemars(with = "String")]
    pub module_id: Uuid,
    /// The panel's id in the module's manifest.
    pub panel: String,
    /// The space it's drawn in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
    /// The thing it's drawn beside, for a `thing.side` panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thing: Option<PanelThingInput>,
    /// The `state` the panel's last answer gave, sent back as it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

impl Targeted for ModulePanelOpenInput {
    fn target(&self) -> Target {
        match &self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModulePanelPressInput {
    #[schemars(with = "String")]
    pub module_id: Uuid,
    pub panel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thing: Option<PanelThingInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// The button pressed or form sent.
    pub action: PanelActionInput,
}

impl Targeted for ModulePanelPressInput {
    fn target(&self) -> Target {
        match &self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

/// What the module answered, checked.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PanelAnswer {
    /// Draw this `aw-panel/1` document.
    Panel {
        #[schemars(with = "Map<String, Value>")]
        panel: Value,
    },
    /// Show these messages under the form's fields, by field.
    Errors { errors: Map<String, Value> },
    /// The flow is finished.
    Done {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Option<Map<String, Value>>")]
        result: Option<Value>,
    },
    /// Close the panel.
    Close,
    /// Open the app's page in a frame (M1.15): load `address`, Workspace's
    /// host page for this launch, once. Only `origin` is framed inside it.
    #[serde(rename_all = "camelCase")]
    Frame {
        #[schemars(with = "String")]
        launch_id: Uuid,
        address: String,
        origin: String,
        title: String,
        height: u16,
    },
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PanelView {
    #[schemars(with = "String")]
    pub module_id: Uuid,
    pub panel: String,
    pub answer: PanelAnswer,
}

pub(super) struct Request<'a> {
    pub module_id: Uuid,
    pub panel: &'a str,
    pub space: Option<Uuid>,
    pub thing: Option<&'a PanelThingInput>,
    pub state: Option<&'a str>,
    pub action: Option<&'a PanelActionInput>,
}

/// Asks a module for a panel, or sends it an action, for the call's caller.
pub(super) async fn round_trip(
    call: &mut OpCall<'_>,
    request: Request<'_>,
) -> Result<PanelView, OpError> {
    let state = call.state;
    let workspace = call.ctx.workspace_id;
    let account = call.ctx.caller.account_id;
    if request
        .state
        .is_some_and(|text| text.len() > blocks::MAX_STATE_BYTES)
    {
        return Err(OpError::Invalid {
            field: "state".into(),
            message: "State is 8 KB at most.".into(),
        });
    }
    if let Some(action) = request.action
        && serde_json::to_vec(&action.values).map_or(true, |bytes| bytes.len() > 32 * 1024)
    {
        return Err(OpError::Invalid {
            field: "action.values".into(),
            message: "An action's values are 32 KB at most.".into(),
        });
    }
    state
        .rate_limits
        .limits()
        .check(
            &crate::rate_limit::MODULE_PANELS,
            LimitKey::Account(account),
        )
        .await?;
    let not_found = || OpError::not_found(THE_PANEL);
    // Read what's needed and reveal the secret, then let the connection go
    // before the module is asked.
    let found = crate::db::scoped_one(&state.pool, async |connection| {
        let Some(install) = drawable(
            connection,
            workspace,
            request.space,
            Some(request.module_id),
        )
        .await?
        .into_iter()
        .next() else {
            return Ok(None);
        };
        let manifest = install.manifest()?;
        let Some(panel) = manifest
            .panels
            .into_iter()
            .find(|panel| panel.id == request.panel && !panel.frame_only)
        else {
            return Ok(None);
        };
        let Some(entry) = panel.entry.clone() else {
            return Ok(None);
        };
        if !slot_fits(&panel.slot, request.space, request.thing) {
            return Ok(None);
        }
        // A thing is named only in the space the call reads.
        if let Some(thing) = request.thing {
            let here: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM things thing \
                  WHERE thing.id = $1 AND thing.workspace_id = $2 AND thing.kind = $3 \
                    AND thing.space_id IS NOT DISTINCT FROM $4)",
            )
            .bind(thing.id.id())
            .bind(workspace)
            .bind(&thing.kind)
            .bind(request.space)
            .fetch_one(&mut *connection)
            .await?;
            if !here {
                return Ok(None);
            }
        }
        let space_name: Option<String> = match request.space {
            Some(space) => {
                sqlx::query_scalar(
                    "SELECT name FROM projects WHERE id = $1 AND organization_id = $2",
                )
                .bind(space)
                .bind(workspace)
                .fetch_optional(&mut *connection)
                .await?
            }
            None => None,
        };
        let Some(outbound) = install.outbound_secret_id else {
            return Ok(None);
        };
        let keys = crate::secrets::keyring(&state.config)?;
        let secret = crate::secrets::reveal(
            &mut *connection,
            keys.as_ref(),
            outbound,
            "connected module panel",
        )
        .await?;
        Ok::<_, AppError>(Some((install, panel.slot, entry, space_name, secret)))
    })
    .await
    .map_err(|error| OpError::logged("module_panel_open", &error))?;
    let (install, slot, entry, space_name, secret) = found.ok_or_else(not_found)?;

    let endpoint = endpoint_of(&install.base_url, &entry)?;
    let body = json!({
        "format": "agent-workspace-panel/1",
        "install": install.module_id,
        "workspace": { "id": workspace },
        "space": request.space.map(|id| json!({ "id": id, "name": space_name })),
        "viewer": { "account": account },
        "context": {
            "slot": slot,
            "panel": request.panel,
            "thing": request.thing.map(|thing| json!({ "kind": thing.kind, "id": thing.id.id() })),
        },
        "state": request.state,
        "action": request.action.map(|action| json!({
            "id": action.id,
            "values": action.values.clone().unwrap_or_default(),
        })),
        "requestId": call.ctx.request_id.clone().unwrap_or_else(|| Uuid::now_v7().to_string()),
    })
    .to_string();
    let Posted { status, body } = post_signed(
        state,
        endpoint,
        secret.expose(),
        body,
        PANEL_TIMEOUT,
        MAX_ANSWER_BYTES,
    )
    .await?;
    if !(200..300).contains(&status) {
        return Err(match status {
            400 | 422 => OpError::Invalid {
                field: "action".into(),
                message: message_of(&body)
                    .unwrap_or_else(|| "The app refused that.".into())
                    .into(),
            },
            404 => OpError::not_found(THE_PANEL),
            _ => OpError::Unavailable,
        });
    }
    let refused = |message: String| OpError::Invalid {
        field: "answer".into(),
        message: message.into(),
    };
    let value: Value = serde_json::from_slice(&body)
        .map_err(|_| refused("The app's answer isn't JSON.".into()))?;
    let answer = blocks::read_answer(&value).map_err(|problems| {
        let first = problems
            .first()
            .map(|problem| format!("{}: {}", problem.path, problem.message))
            .unwrap_or_default();
        refused(format!("The app's panel can't be drawn ({first})."))
    })?;
    let mut answer = match answer {
        Answer::Panel(panel) => PanelAnswer::Panel { panel },
        Answer::Errors(errors) => PanelAnswer::Errors { errors },
        Answer::Done { message, result } => PanelAnswer::Done { message, result },
        Answer::Close => PanelAnswer::Close,
        Answer::Frame { url, title, height } => {
            let launch = super::frames::launch(
                state,
                &install,
                super::frames::NewLaunch {
                    account,
                    panel: request.panel,
                    space: request.space,
                    thing: request
                        .thing
                        .map(|thing| (thing.kind.as_str(), thing.id.id())),
                    url,
                    title,
                    height,
                },
            )
            .await?;
            PanelAnswer::Frame {
                launch_id: launch.id,
                address: launch.address,
                origin: launch.origin,
                title: launch.title,
                height: launch.height,
            }
        }
    };
    // M1.16: an agent a host's panel says it made is handed on only
    // beside the waiting proposal it names.
    if let PanelAnswer::Done {
        result: Some(result),
        ..
    } = &mut answer
    {
        let tx = call.tx().await?;
        super::hosts::checked_result(tx, workspace, install.module_id, account, result)
            .await
            .map_err(|error| OpError::logged("module_panel_open", &error))?;
    }
    Ok(PanelView {
        module_id: install.module_id,
        panel: request.panel.to_owned(),
        answer,
    })
}

pub struct ModulePanelOpen;

impl Operation for ModulePanelOpen {
    const NAME: &'static str = "module_panel_open";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Open a connected module's panel";
    const DESCRIPTION: &'static str = "Ask an outside app connected here for one of its panels, \
        as blocks to draw (aw-panel/1). Send back the state its last answer gave.";

    type Input = ModulePanelOpenInput;
    type Output = PanelView;

    async fn run(call: &mut OpCall<'_>, input: ModulePanelOpenInput) -> Result<PanelView, OpError> {
        round_trip(
            call,
            Request {
                module_id: input.module_id,
                panel: &input.panel,
                space: input.space_id.map(|space| space.id()),
                thing: input.thing.as_ref(),
                state: input.state.as_deref(),
                action: None,
            },
        )
        .await
    }
}

pub struct ModulePanelPress;

impl Operation for ModulePanelPress {
    const NAME: &'static str = "module_panel_press";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    // The app decides what an action may do; it hears who pressed it.
    const PERMISSION: Option<Perm> = None;
    // The app receives the request id and keeps it; a retry asks it again.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const AUTOMATION: bool = false;
    const MAX_BODY_BYTES: usize = 48 * 1024;
    const EMITS: &'static [&'static str] = &["module.panel_pressed"];
    const TITLE: &'static str = "Act in a connected module's panel";
    const DESCRIPTION: &'static str = "Press a button or send a form in an outside app's panel. \
        The app answers with the panel to draw next, errors for the form, done or close.";

    type Input = ModulePanelPressInput;
    type Output = PanelView;

    async fn run(
        call: &mut OpCall<'_>,
        input: ModulePanelPressInput,
    ) -> Result<PanelView, OpError> {
        let view = round_trip(
            call,
            Request {
                module_id: input.module_id,
                panel: &input.panel,
                space: input.space_id.map(|space| space.id()),
                thing: input.thing.as_ref(),
                state: input.state.as_deref(),
                action: Some(&input.action),
            },
        )
        .await?;
        let workspace = call.ctx.workspace_id;
        let account = call.ctx.caller.account_id;
        let tx = call.tx().await?;
        let mut pressed = crate::events::NewEvent::thing(
            &crate::events::catalog::MODULE_PANEL_PRESSED,
            workspace,
            "",
            Uuid::nil(),
        )
        .audience(crate::events::Audience::Account(account))
        .workspace(workspace)
        .actor(account)
        .payload(json!({
            "moduleId": view.module_id,
            "panel": view.panel,
            "action": input.action.id,
        }));
        pressed.thing = None;
        pressed.space_id = None;
        crate::events::emit(tx, pressed).await?;
        call.audit_target(
            "module",
            view.module_id,
            json!({ "panel": view.panel, "action": input.action.id }),
        );
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_is_drawn_only_where_it_fits() {
        let space = Some(Uuid::nil());
        let build = PanelThingInput {
            kind: "acme.build".into(),
            id: serde_json::from_value(json!(Uuid::nil().to_string())).expect("id"),
        };
        assert!(slot_fits("space.tab", space, None));
        assert!(!slot_fits("space.tab", None, None));
        assert!(slot_fits("home.card", None, None));
        assert!(slot_fits("thing.side:acme.build", space, Some(&build)));
        assert!(!slot_fits("thing.side:task", space, Some(&build)));
        assert!(!slot_fits("thing.side:acme.build", space, None));
    }
}
