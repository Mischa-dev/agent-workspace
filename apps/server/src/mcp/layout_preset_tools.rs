//! AWD-111: layout_preset_write, the workspace's own layout presets (a
//! built-in preset with parts turned on or off and nouns renamed), saved,
//! changed or removed through MCP by its owners and admins connected through
//! an app as themselves, as "Save as preset" does on the web. Everything goes
//! through `crate::layouts`' preset functions.

use std::collections::BTreeMap;

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{ToolResult, insert_audit, mark_via, parse_id, project_role},
};
use crate::{
    AppState,
    error::AppError,
    layouts::{self, Layout, SavedPreset, Word},
    policy::ActorKind,
    settings,
};

const MANAGERS_ONLY: &str = "Only the workspace's owners and admins save or change its layout presets, as people, on the web or through an app connected as themselves.";

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LayoutPresetWriteArguments {
    /// A project in the workspace: its ID, link or key (such as "AWD").
    pub project_id: String,
    /// One of the workspace's saved presets, by name or ID, to change or
    /// remove. Leave it out to save a new one.
    #[serde(default)]
    pub preset: Option<String>,
    /// The preset's name (a new one needs it).
    #[serde(default)]
    pub name: Option<String>,
    /// What it's for, in a line.
    #[serde(default)]
    pub description: Option<String>,
    /// The preset it starts from, by name (such as "Software team"). A new
    /// one needs it; on a change, it starts the preset over from this one.
    #[serde(default)]
    pub base: Option<String>,
    /// Parts to show (true) or hide (false), by part ID (such as
    /// "area.runs"), where they differ from the base.
    #[serde(default)]
    pub show: Option<BTreeMap<String, bool>>,
    /// Nouns to rename, by noun ID (such as "task"), each with its words
    /// for one and many: {"task": {"one": "to-do", "many": "to-dos"}}.
    #[serde(default)]
    pub words: Option<BTreeMap<String, PresetWord>>,
    /// Remove the preset: it leaves pickers, and layouts and rules that use
    /// it keep working.
    #[serde(default)]
    pub remove: Option<bool>,
    /// The preset's version you last read, when changing it.
    #[serde(default)]
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct PresetWord {
    pub one: String,
    pub many: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LayoutPresetWriteValue {
    /// "saved", "changed" or "removed".
    pub done: String,
    pub id: String,
    pub name: String,
    /// The preset now. Missing once removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<LayoutPresetLine>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LayoutPresetLine {
    pub description: String,
    /// The built-in preset it starts from, by ID.
    pub base: String,
    /// Parts it shows or hides, where they differ from the base.
    pub show: BTreeMap<String, bool>,
    /// Nouns it renames.
    pub words: BTreeMap<String, PresetWord>,
    pub version: i64,
}

impl From<SavedPreset> for LayoutPresetLine {
    fn from(preset: SavedPreset) -> Self {
        Self {
            description: preset.description,
            base: preset.base,
            show: preset.changes,
            words: preset
                .words
                .into_iter()
                .map(|(noun, word)| {
                    (
                        noun,
                        PresetWord {
                            one: word.one,
                            many: word.many,
                        },
                    )
                })
                .collect(),
            version: preset.version,
        }
    }
}

fn words_of(words: Option<BTreeMap<String, PresetWord>>) -> BTreeMap<String, Word> {
    words
        .unwrap_or_default()
        .into_iter()
        .map(|(noun, word)| {
            (
                noun,
                Word {
                    one: word.one,
                    many: word.many,
                },
            )
        })
        .collect()
}

pub(super) async fn layout_preset_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: LayoutPresetWriteArguments,
) -> ToolResult<LayoutPresetWriteValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if principal.actor_kind != ActorKind::Human {
        return ToolResult::denied(MANAGERS_ONLY);
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let remove = arguments.remove == Some(true);
    let reshapes =
        arguments.base.is_some() || arguments.show.is_some() || arguments.words.is_some();
    if remove
        && (reshapes
            || arguments.name.is_some()
            || arguments.description.is_some()
            || arguments.expected_version.is_some())
    {
        return ToolResult::invalid(
            "remove",
            "Remove a preset on its own, naming only which preset.",
        );
    }
    if arguments.preset.is_none() {
        if remove {
            return ToolResult::invalid("preset", "Name the preset to remove.");
        }
        if arguments.name.is_none() {
            return ToolResult::invalid("name", "A new preset needs a name.");
        }
        if arguments.base.is_none() {
            return ToolResult::invalid(
                "base",
                "A new preset needs the preset it starts from, such as \"Software team\".",
            );
        }
        if arguments.expected_version.is_some() {
            return ToolResult::invalid(
                "expectedVersion",
                "A new preset has no version yet; name a preset to change it.",
            );
        }
    } else if !remove && !reshapes && arguments.name.is_none() && arguments.description.is_none() {
        return ToolResult::invalid(
            "preset",
            "Give what to change (name, description, base, show or words), or remove: true.",
        );
    }
    let project_id = principal.project_id;
    let result = async {
        if project_role(state, principal, project_id).await?.is_none() {
            return Ok(ToolResult::not_found("That project"));
        }
        let mut transaction = state.pool.begin().await?;
        let organization_id =
            sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
                .bind(project_id)
                .fetch_one(&mut *transaction)
                .await?;
        settings::authorize_workspace_manage_as(
            &mut *transaction,
            principal.actor_id,
            ActorKind::Human,
            organization_id,
        )
        .await?;
        let base = match arguments.base.as_deref() {
            Some(text) => {
                match layouts::find_preset(&mut transaction, organization_id, text).await? {
                    Some(base) => Some(base),
                    None => {
                        let names =
                            layouts::preset_names(&mut transaction, organization_id).await?;
                        return Ok(ToolResult::invalid(
                            "base",
                            format!(
                                "There's no preset called \"{}\". Pick one of: {}.",
                                text.trim(),
                                names.join(", ")
                            ),
                        ));
                    }
                }
            }
            None => None,
        };
        let preset_id = match arguments.preset.as_deref() {
            Some(text) => {
                let found = layouts::find_preset(&mut transaction, organization_id, text).await?;
                match found.as_deref().map(Uuid::parse_str) {
                    Some(Ok(id)) => Some(id),
                    Some(Err(_)) => {
                        return Ok(ToolResult::invalid(
                            "preset",
                            "That's a built-in preset, which can't be changed. Save a new one based on it instead.",
                        ));
                    }
                    None => return Ok(ToolResult::not_found("That layout preset")),
                }
            }
            None => None,
        };
        let (done, action, id, name, preset) = match preset_id {
            None => {
                let layout = Layout {
                    preset: base.unwrap_or_default(),
                    changes: arguments.show.unwrap_or_default(),
                    words: words_of(arguments.words),
                };
                let preset = layouts::create_preset_in(
                    &mut transaction,
                    principal.actor_id,
                    organization_id,
                    arguments.name.as_deref().unwrap_or_default(),
                    arguments.description.as_deref().unwrap_or_default(),
                    layout,
                )
                .await?;
                (
                    "saved",
                    "layout.preset_created",
                    preset.id,
                    preset.name.clone(),
                    Some(preset),
                )
            }
            Some(id) if remove => {
                let name = layouts::remove_preset_in(
                    &mut transaction,
                    principal.actor_id,
                    organization_id,
                    id,
                )
                .await?;
                ("removed", "layout.preset_removed", id, name, None)
            }
            Some(id) => {
                // Without base, show and words add to what it has now.
                let layout = reshapes.then(|| Layout {
                    preset: base.unwrap_or_else(|| id.to_string()),
                    changes: arguments.show.unwrap_or_default(),
                    words: words_of(arguments.words),
                });
                let preset = layouts::change_preset_in(
                    &mut transaction,
                    principal.actor_id,
                    organization_id,
                    id,
                    arguments.name.as_deref(),
                    arguments.description.as_deref(),
                    layout,
                    arguments.expected_version,
                )
                .await?;
                (
                    "changed",
                    "layout.preset_changed",
                    id,
                    preset.name.clone(),
                    Some(preset),
                )
            }
        };
        mark_via(&mut transaction, principal).await?;
        insert_audit(
            &mut transaction,
            principal,
            project_id,
            action,
            "layout_preset",
            id,
            serde_json::json!({
                "name": name,
                "version": preset.as_ref().map(|preset| preset.version),
                "organizationId": organization_id,
            }),
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(LayoutPresetWriteValue {
            done: done.to_owned(),
            id: id.to_string(),
            name,
            preset: preset.map(LayoutPresetLine::from),
        }))
    }
    .await;
    match result {
        Ok(result) => result,
        Err(AppError::Forbidden) => ToolResult::denied(MANAGERS_ONLY),
        Err(AppError::NotFound("layout preset")) => ToolResult::not_found("That layout preset"),
        Err(AppError::Conflict("layout preset", "name_taken")) => {
            ToolResult::conflict("The workspace already has a preset with that name.")
        }
        Err(AppError::Conflict("layout preset", "too_many")) => ToolResult::conflict(
            "The workspace has as many saved presets as it can hold. Remove one first.",
        ),
        Err(AppError::Validation("preset name")) => ToolResult::invalid(
            "name",
            "A preset's name is 1 to 60 characters, on one line.",
        ),
        Err(AppError::Validation("preset description")) => ToolResult::invalid(
            "description",
            "A preset's description is at most 200 characters, on one line.",
        ),
        Err(AppError::Validation("layout part")) => ToolResult::invalid(
            "show",
            "Those parts aren't valid: use part IDs the app knows, such as \"area.runs\".",
        ),
        Err(AppError::Validation("layout word")) => ToolResult::invalid(
            "words",
            "Those words aren't valid: use noun IDs the app knows, such as \"task\", each with one and many.",
        ),
        Err(AppError::Validation("layout" | "layout preset")) => {
            ToolResult::invalid("base", "That layout isn't valid.")
        }
        Err(error) => ToolResult::from_error(error, "layout_preset_write"),
    }
}
