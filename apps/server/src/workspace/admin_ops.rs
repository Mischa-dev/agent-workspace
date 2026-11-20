//! Workspace and space administration as operations (K2.17): modules and
//! the workspace's name, workspace and space templates, invitations,
//! layouts and layout presets, and a space's task workflow. Space
//! membership is K4.8's (`space_member_*`).
//!
//! Most run the command their hand-written tool ran (`ops::wrapped`), as
//! the caller the dispatcher checked. The workspace-wide ones name a space
//! only to find the workspace, so the command checks the caller's
//! workspace role itself (as it did) rather than the dispatcher checking
//! their role in that space. The `_write` tools became a create
//! and an update each; their old names stay as aliases (`ops::aliases`).

use uuid::Uuid;

use crate::{
    mcp::{
        admin, invitation_tools, layout_preset_tools,
        module_tools::{self, BuiltModuleSummary, ModuleDetail, ModuleListValue},
        project_template_tools, workspace_template_tools,
    },
    modules::{self, manifest, state},
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::parse_id,
        wrapped::{names_existing, run_tool, tool_op},
    },
};

/// The workspace module's.
pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<ModuleList>()
        .add::<WorkspaceUpdate>()
        .add::<WorkspaceTemplateList>()
        .add::<WorkspaceTemplateApply>()
        .add::<WorkspaceTemplateSave>();
}

/// The Projects module's.
pub fn register_space_ops(registry: &mut Registry) {
    registry
        .add::<ProjectTemplateList>()
        .add::<ProjectTemplateCreate>()
        .add::<ProjectTemplateUpdate>()
        .add::<InvitationList>()
        .add::<InvitationCreate>()
        .add::<InvitationUpdate>()
        .add::<LayoutUpdate>()
        .add::<LayoutPresetCreate>()
        .add::<LayoutPresetUpdate>();
}

/// The Tasks module's.
pub fn register_task_ops(registry: &mut Registry) {
    registry.add::<WorkflowGet>().add::<WorkflowUpdate>();
}

/// The space a `projectId` names (an id, key or link; the dispatcher turns
/// a key into the id first).
fn space(project_id: &str) -> Target {
    Target::Space(parse_id(project_id).unwrap_or_else(Uuid::nil))
}

macro_rules! in_space {
    ($($arguments:ty),* $(,)?) => {
        $(
            impl Targeted for $arguments {
                fn target(&self) -> Target {
                    space(&self.project_id)
                }
            }
        )*
    };
}

in_space!(
    module_tools::WorkspaceUpdateArguments,
    workspace_template_tools::WorkspaceTemplateListArguments,
    workspace_template_tools::WorkspaceTemplateApplyArguments,
    workspace_template_tools::WorkspaceTemplateSaveArguments,
    project_template_tools::ProjectTemplateListArguments,
    project_template_tools::ProjectTemplateWriteArguments,
    invitation_tools::InvitationListArguments,
    invitation_tools::InvitationWriteArguments,
    admin::ProjectLayoutSetArguments,
    layout_preset_tools::LayoutPresetWriteArguments,
    admin::WorkflowArguments,
    admin::WorkflowUpdateArguments,
);

// ---------------------------------------------------------------------------
// module_list

/// Which modules are on, and what each declares.
pub(crate) struct ModuleList;

impl Targeted for module_tools::ModuleListArguments {
    fn target(&self) -> Target {
        match self.project_id.as_deref().map(str::trim) {
            Some(project) if !project.is_empty() => space(project),
            _ => Target::Workspace(None),
        }
    }
}

impl Operation for ModuleList {
    const NAME: &'static str = "module_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List modules";
    const DESCRIPTION: &'static str = "Which modules are on in the workspace (or, with \
        projectId, in a space, as your role there narrows them) and what each does: its kinds \
        of thing, its tools by what they do, the live blocks it offers, and why one is off; \
        and the built modules there. Pass module for one module's full declaration. Tools of a \
        module that's off aren't listed and say so when called.";

    type Input = module_tools::ModuleListArguments;
    type Output = ModuleListValue;

    async fn run(
        call: &mut OpCall<'_>,
        input: module_tools::ModuleListArguments,
    ) -> Result<ModuleListValue, OpError> {
        let detail_of = match input.module.as_deref() {
            Some(id) => match manifest::get(id) {
                Some(module) => Some(module),
                None => {
                    let known = manifest::core()
                        .iter()
                        .map(|module| module.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(OpError::Invalid {
                        field: "module".into(),
                        message: format!("There's no module {id}. Use one of {known}.").into(),
                    });
                }
            },
            None => None,
        };
        let space = call.ctx.space_id;
        let account = call.account_id();
        let reader = modules::built::Reader::of(&call.ctx.caller);
        // Its own read, as the tool's was.
        let mut connection = crate::db::scoped_tx(&call.state.pool)
            .await
            .map_err(crate::error::AppError::from)?;
        let tx = &mut connection;
        let workspace = call.ctx.workspace_id;
        let switches = state::switches(tx, workspace).await?;
        // In a space, the caller's role there narrows the workspace's
        // switches, as calls there are checked; otherwise their workspace
        // role does.
        let set = match space {
            Some(space) => state::for_project(tx, space, account)
                .await?
                .unwrap_or_default(),
            None => state::for_member(tx, workspace, account).await?,
        };
        let rows = modules::built::list(tx, workspace, space, false).await?;
        let counts = modules::built::part_counts(
            tx,
            &rows.iter().map(|row| row.id).collect::<Vec<_>>(),
            reader,
        )
        .await?;
        let mut built = Vec::with_capacity(rows.len());
        for row in rows {
            let on = modules::built::is_on(tx, &row).await?;
            built.push(BuiltModuleSummary {
                parts: counts.get(&row.id).copied().unwrap_or(0),
                id: row.id,
                key: row.module_key,
                name: row.name,
                description: row.description,
                icon: row.icon,
                on,
                space_id: row.space_id,
            });
        }
        Ok(ModuleListValue {
            built,
            workspace_id: workspace,
            modules: manifest::core()
                .iter()
                .map(|module| modules::http::ModuleView::of(module, set, &switches))
                .collect(),
            preset: modules::http::matching_preset(&switches),
            detail: detail_of.map(|module| ModuleDetail {
                things: module.things.clone(),
                embeds: module.embeds.clone(),
                permissions: module.permissions.clone(),
                listens: module.events.listens.clone(),
            }),
        })
    }
}

// ---------------------------------------------------------------------------
// workspace_update

tool_op!(WorkspaceUpdate {
    name: "workspace_update",
    module: "workspace",
    kind: Change,
    permission: None,
    idempotency: Dispatcher,
    audited: true,
    title: "Change workspace",
    description: "Rename the workspace or change its address, or switch its modules on or \
        off (by id from module_list, or from a preset); inProjectOnly switches them in one \
        space. Off hides a module without deleting anything. Takes managing the workspace's \
        settings.",
    emits: ["workspace.updated"],
    input: module_tools::WorkspaceUpdateArguments,
    output: module_tools::WorkspaceUpdateValue,
    run: |call, input| { run_tool!(call, module_tools::workspace_update, input) }
});

// ---------------------------------------------------------------------------
// Workspace templates

tool_op!(WorkspaceTemplateList {
    name: "workspace_template_list",
    module: "workspace",
    kind: Query,
    permission: None,
    idempotency: None,
    title: "List workspace templates",
    description: "The workspace's templates, built-in and saved: what using each does \
        (modules, layout, roles, the role new members get, a starter space) with its full \
        definition, and whether you may use it.",
    emits: [],
    input: workspace_template_tools::WorkspaceTemplateListArguments,
    output: workspace_template_tools::WorkspaceTemplateListValue,
    run: |call, input| {
        run_tool!(
            call,
            workspace_template_tools::workspace_template_list,
            input
        )
    }
});

tool_op!(WorkspaceTemplateApply {
    name: "workspace_template_apply",
    module: "workspace",
    kind: Change,
    permission: None,
    idempotency: Dispatcher,
    audited: true,
    title: "Use workspace template",
    description: "Use a workspace template: switch modules, set the default layout, add or \
        update roles and the role new members get, and make its starter space. Nothing is \
        deleted and what exists by name is reused, so using it twice changes nothing more.",
    emits: ["workspace.updated"],
    input: workspace_template_tools::WorkspaceTemplateApplyArguments,
    output: crate::workspace_templates::Applied,
    run: |call, input| {
        run_tool!(
            call,
            workspace_template_tools::workspace_template_apply,
            input
        )
    }
});

tool_op!(WorkspaceTemplateSave {
    name: "workspace_template_save",
    module: "workspace",
    kind: Create,
    permission: None,
    idempotency: Dispatcher,
    audited: true,
    title: "Save workspace template",
    description: "Save the workspace as a template (modules, layout, changed roles, the \
        default role, and with includeProject the space's channels and collections), or \
        remove a saved one (remove).",
    emits: ["workspace.updated"],
    input: workspace_template_tools::WorkspaceTemplateSaveArguments,
    output: workspace_template_tools::WorkspaceTemplateSaveValue,
    run: |call, input| {
        run_tool!(
            call,
            workspace_template_tools::workspace_template_save,
            input
        )
    }
});

// ---------------------------------------------------------------------------
// Space templates

tool_op!(ProjectTemplateList {
    name: "project_template_list",
    module: "projects",
    kind: Query,
    permission: None,
    idempotency: None,
    title: "List space templates",
    description: "The workspace's space templates (name a space in it): each one's workflow, \
        how many labels, settings and task templates it sets up, and its channels. Start a \
        space from one with project_create's template.",
    emits: [],
    input: project_template_tools::ProjectTemplateListArguments,
    output: project_template_tools::ProjectTemplateListValue,
    run: |call, input| { run_tool!(call, project_template_tools::project_template_list, input) }
});

tool_op!(ProjectTemplateCreate {
    name: "project_template_create",
    module: "projects",
    kind: Create,
    permission: None,
    idempotency: Dispatcher,
    audited: true,
    title: "Save space template",
    description: "Save a space you manage as a template for its workspace: its workflow, \
        labels, channel names, settings and task templates.",
    emits: ["space.updated"],
    input: project_template_tools::ProjectTemplateWriteArguments,
    output: project_template_tools::ProjectTemplateWriteValue,
    run: |call, input| {
        names_existing(input.template.is_some(), true, "template")?;
        run_tool!(call, project_template_tools::project_template_write, input)
    }
});

tool_op!(ProjectTemplateUpdate {
    name: "project_template_update",
    module: "projects",
    kind: Change,
    permission: None,
    idempotency: Dispatcher,
    audited: true,
    title: "Update space template",
    description: "Rename, describe, archive or restore a space template: one you made, or \
        any if you manage the workspace's settings.",
    emits: ["space.updated"],
    input: project_template_tools::ProjectTemplateWriteArguments,
    output: project_template_tools::ProjectTemplateWriteValue,
    run: |call, input| {
        names_existing(input.template.is_some(), false, "template")?;
        run_tool!(call, project_template_tools::project_template_write, input)
    }
});

// ---------------------------------------------------------------------------
// Invitations

tool_op!(InvitationList {
    name: "invitation_list",
    module: "projects",
    kind: Query,
    permission: None,
    idempotency: None,
    title: "List invitations",
    description: "A space's invitations (or, with wholeWorkspace: true, its workspace's), newest \
        first, with each one's role, status and version, never its link.",
    emits: [],
    input: invitation_tools::InvitationListArguments,
    output: invitation_tools::InvitationListValue,
    run: |call, input| { run_tool!(call, invitation_tools::invitation_list, input) }
});

tool_op!(InvitationCreate {
    name: "invitation_create",
    module: "projects",
    kind: Create,
    permission: None,
    idempotency: Dispatcher,
    audited: true,
    // A join link is a bearer credential: never kept for a replay.
    shown_once: ["joinUrl"],
    title: "Invite",
    description: "Invite someone to a space with a join link (role admin, member, viewer or a \
        custom role, never above yours), or to its workspace (wholeWorkspace: true, with a note). \
        The link is returned once, to pass on privately.",
    emits: ["space.member.changed"],
    input: invitation_tools::InvitationWriteArguments,
    output: invitation_tools::InvitationWriteValue,
    run: |call, input| {
        names_existing(input.invitation.is_some(), true, "invitation")?;
        run_tool!(call, invitation_tools::invitation_write, input)
    }
});

tool_op!(InvitationUpdate {
    name: "invitation_update",
    module: "projects",
    kind: Change,
    permission: None,
    idempotency: Dispatcher,
    destructive: true,
    audited: true,
    // A join link is a bearer credential: never kept for a replay.
    shown_once: ["joinUrl"],
    title: "Revoke or replace invitation",
    description: "Revoke an invitation (revoke: true), or replace it with a new link \
        (replace: true, returned once), with the version invitation_list shows.",
    emits: ["space.member.changed"],
    input: invitation_tools::InvitationWriteArguments,
    output: invitation_tools::InvitationWriteValue,
    run: |call, input| {
        names_existing(input.invitation.is_some(), false, "invitation")?;
        run_tool!(call, invitation_tools::invitation_write, input)
    }
});

// ---------------------------------------------------------------------------
// Layouts

tool_op!(LayoutUpdate {
    name: "layout_update",
    module: "projects",
    kind: Change,
    permission: None,
    idempotency: Dispatcher,
    audited: true,
    title: "Set layout",
    description: "Set a space's layout preset (such as Software team), which decides the \
        parts people see there and words like \"to-dos\" for tasks; leave preset out to follow \
        the workspace's default. With wholeWorkspace: true, set the workspace's default; with \
        rules, which preset each role or person gets. Visual only.",
    emits: ["space.updated"],
    input: admin::ProjectLayoutSetArguments,
    output: admin::ProjectLayoutValue,
    run: |call, input| { run_tool!(call, admin::project_layout_set, input) }
});

tool_op!(LayoutPresetCreate {
    name: "layout_preset_create",
    module: "projects",
    kind: Create,
    permission: None,
    idempotency: Dispatcher,
    audited: true,
    title: "Save layout preset",
    description: "Save a workspace layout preset: one it starts from (base) with parts shown \
        or hidden (show) and nouns renamed (words), for layout_update to pick by name. Visual \
        only.",
    emits: ["workspace.updated"],
    input: layout_preset_tools::LayoutPresetWriteArguments,
    output: layout_preset_tools::LayoutPresetWriteValue,
    run: |call, input| {
        names_existing(input.preset.is_some(), true, "preset")?;
        run_tool!(call, layout_preset_tools::layout_preset_write, input)
    }
});

tool_op!(LayoutPresetUpdate {
    name: "layout_preset_update",
    module: "projects",
    kind: Change,
    permission: None,
    idempotency: Dispatcher,
    destructive: true,
    audited: true,
    title: "Update layout preset",
    description: "Change a saved layout preset (show and words add to what it has, unless \
        you give base) or remove it (remove: true; layouts using it keep working).",
    emits: ["workspace.updated"],
    input: layout_preset_tools::LayoutPresetWriteArguments,
    output: layout_preset_tools::LayoutPresetWriteValue,
    run: |call, input| {
        names_existing(input.preset.is_some(), false, "preset")?;
        run_tool!(call, layout_preset_tools::layout_preset_write, input)
    }
});

// ---------------------------------------------------------------------------
// A space's task workflow

tool_op!(WorkflowGet {
    name: "workflow_get",
    module: "tasks",
    kind: Describe,
    permission: Some(Perm::TasksView),
    idempotency: None,
    title: "Get workflow",
    description: "A space's task types, statuses (in board order, with their category) and \
        the status changes allowed. Use the status names with task_update and task_list.",
    emits: [],
    input: admin::WorkflowArguments,
    output: admin::WorkflowValue,
    run: |call, input| { run_tool!(call, admin::workflow_get, input) }
});

tool_op!(WorkflowUpdate {
    name: "workflow_update",
    module: "tasks",
    kind: Change,
    permission: Some(Perm::TasksWorkflow),
    idempotency: Dispatcher,
    destructive: true,
    audited: true,
    title: "Update workflow",
    description: "Change a space's workflow: statuses (in board order, each with a \
        category), task types, allowed status changes and estimate unit. Send the whole \
        workflow as workflow_get shows it, with expectedVersion; a status or type left out is \
        retired, and replacements say where its open tasks go.",
    emits: ["space.updated"],
    input: admin::WorkflowUpdateArguments,
    output: admin::WorkflowUpdateValue,
    run: |call, input| { run_tool!(call, admin::workflow_update, input) }
});
