//! Whiteboard operations. The board tools themselves are still hand-written
//! (`mcp/whiteboard_tools.rs`); new reads and writes start here.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::ops::{
    Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted, refs::IdRef,
};

pub fn register_ops(registry: &mut Registry) {
    registry.add::<WhiteboardTemplateList>();
}

/// List what a new board can start from.
pub struct WhiteboardTemplateList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhiteboardTemplateListInput {
    /// A space (a project) in the workspace: its id, link or key. Leave it
    /// out to ask about the workspace you're in.
    #[serde(default, alias = "projectId", skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
}

impl Targeted for WhiteboardTemplateListInput {
    fn target(&self) -> Target {
        match &self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WhiteboardTemplateListOutput {
    /// The built-in templates, then boards marked as templates in the
    /// workspace that you can see.
    pub templates: Vec<super::templates::TemplateSummary>,
}

impl Operation for WhiteboardTemplateList {
    const NAME: &'static str = "whiteboard_template_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "whiteboards";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List whiteboard templates";
    const DESCRIPTION: &'static str = "List what a new whiteboard in a workspace can start \
        from: the built-in templates and boards marked as templates. Pass a template's id to \
        whiteboard_create as fromTemplateId.";

    type Input = WhiteboardTemplateListInput;
    type Output = WhiteboardTemplateListOutput;

    async fn run(
        call: &mut OpCall<'_>,
        _input: WhiteboardTemplateListInput,
    ) -> Result<WhiteboardTemplateListOutput, OpError> {
        let caller = super::Caller::from(call.ctx);
        let workspace_id = call.ctx.workspace_id;
        let templates = super::templates::list(call.tx().await?, &caller, workspace_id).await?;
        Ok(WhiteboardTemplateListOutput { templates })
    }
}
