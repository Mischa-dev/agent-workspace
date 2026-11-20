//! Notes on a board become tasks (C1.32): `whiteboard_to_tasks` makes a
//! task of each note (a labelled shape such as a sticky, or a text on its
//! own), labelled with the name of the frame it sits in, and each task's
//! description links back to the board.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::outline;
use crate::{
    addresses::{Addresses, Target as Address},
    error::AppError,
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Target, Targeted, refs::IdRef},
    work_item_commands::{
        AllocatedNumberMetadata, CommandActor, CreateWorkItemCommand, CreateWorkItemOutcome,
        NewTaskFields, create_work_item,
    },
};

/// Most tasks one call makes.
pub const MAX_TASKS: usize = 100;
/// The longest task title.
const MAX_TITLE: usize = 240;

/// Make tasks from a board's notes.
pub struct WhiteboardToTasks;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhiteboardToTasksInput {
    /// The board: its id or a link to it.
    pub whiteboard_id: IdRef,
    /// The notes to make tasks of, by element id; left out, every note on
    /// the board (up to 100).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub element_ids: Vec<String>,
    /// The space the tasks go in; left out, the board's.
    #[serde(default, alias = "projectId", skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
}

impl Targeted for WhiteboardToTasksInput {
    fn target(&self) -> Target {
        match self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::thing("whiteboard", self.whiteboard_id.id()),
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MadeTask {
    #[schemars(with = "String")]
    pub id: Uuid,
    pub key: String,
    pub title: String,
    /// The note it was made from.
    pub element_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BoardTasks {
    pub tasks: Vec<MadeTask>,
    /// Frame names the space had no label for, made now.
    pub created_labels: Vec<String>,
}

impl Operation for WhiteboardToTasks {
    const NAME: &'static str = "whiteboard_to_tasks";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "tasks";
    const PERMISSION: Option<Perm> = Some(Perm::TasksCreate);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Make tasks from a board's notes";
    const DESCRIPTION: &'static str = "Make a task of each note on a whiteboard (a labelled shape \
        such as a sticky, or a text on its own), or of the notes named, up to 100. Each is \
        labelled with its frame's name and links back to the board.";
    const EMITS: &'static [&'static str] = &["task.created"];

    type Input = WhiteboardToTasksInput;
    type Output = BoardTasks;

    async fn run(
        call: &mut OpCall<'_>,
        input: WhiteboardToTasksInput,
    ) -> Result<BoardTasks, OpError> {
        let board_id = input.whiteboard_id.id();
        let workspace_id = call.ctx.workspace_id;
        let space_id = call
            .ctx
            .space_id
            .ok_or_else(|| OpError::invalid("spaceId", "Say which space the tasks go in."))?;
        let caller = super::Caller::from(call.ctx);
        let actor = CommandActor::from(call.ctx);
        let addresses = Addresses::of(call.state);
        let tx = call.tx().await?;
        let board = super::access(tx, &caller, board_id, false)
            .await
            .map_err(|_| OpError::not_found("That board"))?;
        if board.organization_id != workspace_id {
            return Err(OpError::not_found("That board"));
        }
        let title = sqlx::query_scalar::<_, String>("SELECT title FROM whiteboards WHERE id = $1")
            .bind(board_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(AppError::from)?;
        let elements = super::store::load_rows(tx, board_id).await?;
        let mut notes: Vec<outline::Note> = outline::sections(&elements)
            .into_iter()
            .flat_map(|section| section.notes)
            .collect();
        if !input.element_ids.is_empty() {
            if let Some(missing) = input
                .element_ids
                .iter()
                .find(|wanted| !notes.iter().any(|note| &note.id == *wanted))
            {
                return Err(OpError::Invalid {
                    field: "elementIds".into(),
                    message: format!(
                        "{missing} isn't a note on the board: a labelled shape or a text."
                    )
                    .into(),
                });
            }
            notes.retain(|note| input.element_ids.contains(&note.id));
        }
        if notes.is_empty() {
            return Err(OpError::invalid(
                "whiteboardId",
                "That board has no notes to make tasks of.",
            ));
        }
        if notes.len() > MAX_TASKS {
            return Err(OpError::invalid(
                "elementIds",
                "Make up to 100 tasks at a time: name the notes with elementIds.",
            ));
        }
        let mut frames: Vec<(String, String)> = Vec::new();
        for frame in notes.iter().filter_map(|note| note.frame.clone()) {
            if !frames.iter().any(|(name, _)| *name == frame) {
                frames.push((frame, String::new()));
            }
        }
        let labels =
            crate::work_item_labels::ensure_by_name(tx, space_id, actor.actor_id, &frames).await?;
        // A frame's label, found by name once it surely exists (one whose
        // name only an archived label has is left off).
        let active = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, lower(name) FROM project_labels \
             WHERE project_id = $1 AND archived_at IS NULL AND id = ANY($2)",
        )
        .bind(space_id)
        .bind(&labels.ids)
        .fetch_all(&mut **tx)
        .await
        .map_err(AppError::from)?;
        let label_of = |frame: &str| -> Vec<Uuid> {
            let wanted = frame.trim().to_lowercase();
            active
                .iter()
                .find(|(_, name)| *name == wanted)
                .map(|(id, _)| *id)
                .into_iter()
                .collect()
        };
        let href = addresses.in_workspace(workspace_id, &Address::Whiteboard(board_id));
        let back = format!(
            "From the board [{}]({href}).",
            title.replace(['[', ']'], "")
        );
        let project_key =
            sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
                .bind(space_id)
                .fetch_one(&mut **tx)
                .await
                .map_err(AppError::from)?;
        let mut tasks = Vec::with_capacity(notes.len());
        for note in notes {
            let (first, rest) = note.text.split_once('\n').unwrap_or((&note.text, ""));
            let task_title: String = first.trim().chars().take(MAX_TITLE).collect();
            let description = match rest.trim() {
                "" => back.clone(),
                more => format!("{more}\n\n{back}"),
            };
            let label_ids = note.frame.as_deref().map(label_of).unwrap_or_default();
            let outcome = create_work_item(
                tx,
                &actor,
                CreateWorkItemCommand {
                    collection_id: None,
                    field_values: None,
                    title: &task_title,
                    description: &description,
                    type_id: None,
                    status_id: None,
                    priority: "no_priority",
                    assignee_actor_id: None,
                    parent_work_item_id: None,
                    label_ids: &label_ids,
                    acceptance_criteria: &[],
                    board_placement: None,
                    fields: NewTaskFields::default(),
                    activity_summary: "Work Item created",
                    activity_metadata: json!({"whiteboardId": board_id, "elementId": note.id}),
                    activity_number_metadata: AllocatedNumberMetadata::Omit,
                    audit_action: "work_item.created",
                    audit_metadata: json!({"whiteboardId": board_id, "elementId": note.id}),
                },
            )
            .await?;
            let CreateWorkItemOutcome::Created {
                work_item_id,
                number,
                ..
            } = outcome
            else {
                return Err(AppError::Internal("a board note's task was refused").into());
            };
            tasks.push(MadeTask {
                id: work_item_id,
                key: format!("{project_key}-{number}"),
                title: task_title,
                element_id: note.id,
            });
        }
        call.audit_target(
            "whiteboard",
            board_id,
            json!({"tasks": tasks.iter().map(|task| task.id).collect::<Vec<_>>()}),
        );
        Ok(BoardTasks {
            tasks,
            created_labels: labels
                .created
                .iter()
                .map(|label| label.name.clone())
                .collect(),
        })
    }
}
