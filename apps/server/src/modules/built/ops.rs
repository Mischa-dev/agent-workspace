//! Built modules as operations (M1.6): `module_create`, `module_update`,
//! `module_open`, `module_part_link`, `module_part_unlink`, `module_set`
//! (switch on or off), `module_archive` and `module_restore`. Building
//! needs `modules.build`; switching, archiving and restoring
//! `modules.manage`. Both are permissions of K1's catalogue, which any role
//! may hold, an agent's included.

use std::{borrow::Cow, collections::BTreeMap};

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{ModuleRow, NavEntry, NavView, PartView};
use crate::{
    collections::{self, CreateOutcome, NewCollection, definition},
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        access_shim,
        error::{ConflictReason, DenyReason},
        refs::IdRef,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<ModuleCreate>()
        .add::<ModuleUpdate>()
        .add::<ModuleOpen>()
        .add::<ModulePartLink>()
        .add::<ModulePartUnlink>()
        .add::<ModuleSet>()
        .add::<ModuleArchive>()
        .add::<ModuleRestore>();
}

const MODULE: &str = "That module";

/// A module as the operations answer.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModuleView {
    #[schemars(with = "String")]
    pub id: Uuid,
    /// `local.<name>`.
    pub key: String,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    /// The space it belongs to; none for a workspace-wide module.
    #[schemars(with = "Option<String>")]
    pub space_id: Option<Uuid>,
    /// Pass it as expectedRevision to module_update.
    pub revision: i64,
    /// Switched on where it lives (module_set).
    pub on: bool,
    pub archived: bool,
    /// How it was archived: "archive" (its parts hidden with it) or "keep".
    pub archived_parts: Option<String>,
    pub parts: Vec<PartView>,
    pub nav: Vec<NavView>,
}

/// The module as the caller sees it: parts in spaces they aren't in, and
/// channels they can't see, are left out.
async fn view(call: &mut OpCall<'_>, module_id: Uuid) -> Result<ModuleView, OpError> {
    let workspace_id = call.ctx.workspace_id;
    let reader = super::Reader::of(&call.ctx.caller);
    let connection = call.tx().await?;
    let row = super::get(connection, workspace_id, module_id)
        .await?
        .ok_or(OpError::not_found(MODULE))?;
    let on = super::is_on(connection, &row).await?;
    Ok(ModuleView {
        id: row.id,
        key: row.module_key.clone(),
        name: row.name.clone(),
        description: row.description.clone(),
        icon: row.icon.clone(),
        space_id: row.space_id,
        revision: row.revision,
        on,
        archived: row.archived_at.is_some(),
        archived_parts: row.archived_parts.clone(),
        parts: super::parts(connection, row.id, reader).await?,
        nav: super::nav(connection, row.id, reader).await?,
    })
}

/// The module the call acts on, locked, in the located workspace.
async fn locked(call: &mut OpCall<'_>, module_id: Uuid) -> Result<ModuleRow, OpError> {
    let workspace = call.ctx.workspace_id;
    let confined = call.ctx.caller.confined_space;
    let tx = call.tx().await?;
    let module = super::lock(tx, workspace, module_id)
        .await?
        .ok_or(OpError::not_found(MODULE))?;
    held_to(confined, module.space_id)?;
    Ok(module)
}

/// A connection held to one space changes only that space's modules: a
/// workspace-wide module's switch, parts and archive reach every space.
fn held_to(confined: Option<Uuid>, module_space: Option<Uuid>) -> Result<(), OpError> {
    match confined {
        Some(held) if module_space != Some(held) => Err(OpError::Denied {
            reason: DenyReason::Delegation,
            message: "This connection is held to one space, so it can only change that space's \
                      modules. Reconnect it for your whole account to change workspace modules."
                .into(),
        }),
        _ => Ok(()),
    }
}

fn live(module: &ModuleRow) -> Result<(), OpError> {
    if module.archived_at.is_some() {
        return Err(OpError::Conflict {
            reason: ConflictReason::Archived,
            message: "The module is archived. Restore it first (module_restore).".into(),
            current: None,
        });
    }
    Ok(())
}

fn check_text(name: &str, description: Option<&str>, icon: Option<&str>) -> Result<(), OpError> {
    let name_ok = (1..=60).contains(&name.chars().count())
        && name == name.trim()
        && !name.chars().any(char::is_control);
    if !name_ok {
        return Err(OpError::invalid(
            "name",
            "Name it in 1 to 60 characters, on one line.",
        ));
    }
    if description.is_some_and(|text| text.chars().count() > 500) {
        return Err(OpError::invalid(
            "description",
            "Keep the description to 500 characters.",
        ));
    }
    if icon.is_some_and(|icon| !super::valid_icon(icon)) {
        return Err(OpError::invalid(
            "icon",
            "An icon is a name in lowercase letters, digits and dashes.",
        ));
    }
    Ok(())
}

fn part_kind(kind: &str) -> Result<&'static str, OpError> {
    super::LINKABLE
        .iter()
        .find(|known| **known == kind)
        .copied()
        .ok_or(OpError::invalid(
            "part.kind",
            "A part is a collection, form, automation or page.",
        ))
}

// ------------------------------------------------------------ module_create

pub struct ModuleCreate;

/// An existing object to make a part of the module.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartRef {
    /// collection, form, automation or page.
    pub kind: String,
    /// Its id, or a link to it.
    pub id: IdRef,
}

/// A new collection, made as part of the module.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InlineCollection {
    /// Its name (1 to 60 characters). Forms, automations and nav entries in
    /// the same call name it by this.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// What the title column is called (default Name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_name: Option<String>,
    /// Its fields, as collection_write takes them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<definition::FieldWrite>,
    /// Its views, as collection_write takes them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<definition::ViewWrite>,
}

/// One question of a new form.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InlineFormField {
    /// The collection's field, by name or id.
    pub field: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
}

/// A new form that adds records to a collection.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InlineForm {
    /// Its name (1 to 80 characters).
    pub name: String,
    /// The collection its answers go to: a new collection's name from this
    /// call, or an existing collection's id.
    pub collection: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub intro: String,
    /// The fields it asks for, after the title.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<InlineFormField>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub thanks: String,
}

/// A new automation rule.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InlineAutomation {
    /// The rule as automation_create takes it: name, trigger,
    /// conditions, actions, enabled.
    pub rule: Value,
    /// For a record trigger: the collection, by a new collection's name
    /// from this call (its id goes into the trigger's collectionId).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleCreateInput {
    /// Its key: lowercase letters, digits and underscores, such as
    /// "playtest" (stored as "local.playtest"). Unique in the workspace.
    pub key: String,
    /// What people see (1 to 60 characters).
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// The space it belongs to (id, link or key). Left out, it is
    /// workspace-wide and can only hold existing parts.
    #[serde(default, alias = "projectId", skip_serializing_if = "Option::is_none")]
    pub space_id: Option<IdRef>,
    /// Existing objects to make its parts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PartRef>,
    /// New collections, made in its space.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collections: Vec<InlineCollection>,
    /// New forms.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forms: Vec<InlineForm>,
    /// New automation rules, acting as you.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automations: Vec<InlineAutomation>,
    /// Navigation entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nav: Vec<NavEntry>,
}

impl Targeted for ModuleCreateInput {
    fn target(&self) -> Target {
        match self.space_id {
            Some(space) => Target::Space(space.id()),
            None => Target::Workspace(None),
        }
    }
}

impl Operation for ModuleCreate {
    const NAME: &'static str = "module_create";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesBuild);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const MAX_BODY_BYTES: usize = 512 * 1024;
    const TITLE: &'static str = "Build a module";
    const DESCRIPTION: &'static str = "Build a module: a named bundle of collections, forms, \
        automations, pages and navigation entries that is switched on and off as one \
        (module_set) without losing data. Give existing parts by id, or new collections, forms \
        and automations inline; everything is made in one go, as you, or nothing is. Forms \
        made here get their public link from the form's settings. A retry with the same \
        request id changes nothing.";
    const EMITS: &'static [&'static str] = &["module.created"];

    type Input = ModuleCreateInput;
    type Output = ModuleView;

    async fn run(call: &mut OpCall<'_>, input: ModuleCreateInput) -> Result<ModuleView, OpError> {
        let key = super::full_key(&input.key).ok_or(OpError::invalid(
            "key",
            "A key is 1 to 40 lowercase letters, digits and underscores, starting with a letter.",
        ))?;
        let name = input.name.trim().to_owned();
        check_text(&name, input.description.as_deref(), input.icon.as_deref())?;
        let inline = !input.collections.is_empty()
            || !input.forms.is_empty()
            || !input.automations.is_empty();
        let space = match (input.space_id, call.ctx.space_id) {
            (Some(_), Some(space)) => Some(space),
            (Some(_), None) => return Err(OpError::not_found("That space")),
            (None, _) if inline => {
                return Err(OpError::invalid(
                    "spaceId",
                    "New collections, forms and automations are made in a space: name it \
                     (spaceId).",
                ));
            }
            (None, _) => None,
        };
        if !input.collections.is_empty() || !input.forms.is_empty() {
            // Making collections and forms is building collections there.
            let located = crate::ops::Located {
                workspace_id: call.ctx.workspace_id,
                space_id: call.ctx.space_id,
            };
            let ctx = call.ctx;
            let tx = call.tx().await?;
            access_shim::check(
                tx,
                &ctx.caller,
                &ctx.target,
                &located,
                Some(Perm::CollectionsBuild),
                true,
                "Making collections and forms",
            )
            .await?;
        }
        let actor = call.account_id();
        let confined = call.ctx.caller.confined_space;
        held_to(confined, space)?;
        let workspace = call.ctx.workspace_id;
        let maker = crate::events::Caller::from(call.ctx);
        let state = call.state;

        // The space's lock first, as every collection write takes it.
        let project_key = match space {
            Some(space) => {
                let tx = call.tx().await?;
                crate::work_item_commands::lock_membership(tx, actor, space)
                    .await?
                    .ok_or(OpError::not_found("That space"))?;
                sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
                    .bind(space)
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(|error| OpError::logged(Self::NAME, &error.into()))?
            }
            None => String::new(),
        };
        let module_id = {
            let tx = call.tx().await?;
            super::insert(
                tx,
                workspace,
                space,
                &key,
                &name,
                input.description.as_deref().unwrap_or(""),
                input.icon.as_deref(),
                actor,
            )
            .await
            .map_err(|_| OpError::Conflict {
                reason: ConflictReason::State {
                    code: "module_key_taken",
                },
                message: format!("A module called {key} exists here already. Pick another key.")
                    .into(),
                current: None,
            })?
        };
        let mut made: Vec<(&'static str, Uuid)> = Vec::new();
        // Names from this call, one map per kind: a collection and a form
        // may share a name ("Feedback").
        let mut collections_named: BTreeMap<String, Uuid> = BTreeMap::new();
        let mut forms_named: BTreeMap<String, Uuid> = BTreeMap::new();

        // Collections, then their fields and views.
        if let Some(space) = space {
            for (index, inline) in input.collections.iter().enumerate() {
                let collection_name =
                    collections::normalize_name(&inline.name).ok_or_else(|| OpError::Invalid {
                        field: format!("collections[{index}].name").into(),
                        message: "Name a collection in 1 to 60 characters, on one line.".into(),
                    })?;
                let description =
                    collections::normalize_description(inline.description.as_deref().unwrap_or(""))
                        .ok_or_else(|| OpError::Invalid {
                            field: format!("collections[{index}].description").into(),
                            message: "A collection's description is one short line.".into(),
                        })?;
                let title_name = match inline.title_name.as_deref() {
                    Some(value) => collections::normalize_title_name(value).ok_or_else(|| {
                        OpError::Invalid {
                            field: format!("collections[{index}].titleName").into(),
                            message: "Call the title column in a few words, on one line.".into(),
                        }
                    })?,
                    None => collections::DEFAULT_TITLE_NAME.to_owned(),
                };
                if let Some(message) = definition::duplicate_name(&inline.fields, &inline.views) {
                    return Err(OpError::Invalid {
                        field: format!("collections[{index}].fields").into(),
                        message: message.into(),
                    });
                }
                let tx = call.tx().await?;
                let collection = match collections::create(
                    tx,
                    space,
                    actor,
                    NewCollection {
                        name: collection_name.clone(),
                        description,
                        title_name,
                        first_view: inline.views.is_empty(),
                    },
                )
                .await?
                {
                    CreateOutcome::Created(collection) => collection,
                    CreateOutcome::NameTaken(_) => {
                        return Err(OpError::Conflict {
                            reason: ConflictReason::State {
                                code: "collection_name_taken",
                            },
                            message: format!(
                                "A collection is already called \"{collection_name}\" here."
                            )
                            .into(),
                            current: None,
                        });
                    }
                    CreateOutcome::ProjectFull => {
                        return Err(OpError::Conflict {
                            reason: ConflictReason::State {
                                code: "collection_limit",
                            },
                            message: "This space has as many collections as it can hold. \
                                      Archive one first."
                                .into(),
                            current: None,
                        });
                    }
                };
                if !inline.fields.is_empty() || !inline.views.is_empty() {
                    let writer = crate::work_item_fields::Writer {
                        actor_id: actor,
                        project_key: &project_key,
                    };
                    if let Err(problem) = definition::apply(
                        tx,
                        space,
                        &collection,
                        writer,
                        &inline.fields,
                        &inline.views,
                    )
                    .await?
                    {
                        return Err(OpError::Invalid {
                            field: format!("collections[{index}].{}", problem.field).into(),
                            message: problem.message.into(),
                        });
                    }
                }
                crate::outbox::enqueue_collection_updated(tx, space, collection.id, &[]).await?;
                collections_named.insert(collection_name.to_lowercase(), collection.id);
                made.push(("collection", collection.id));
            }

            // Forms, over a collection from this call or an existing one.
            for (index, inline) in input.forms.iter().enumerate() {
                let field =
                    |name: &str| -> Cow<'static, str> { format!("forms[{index}].{name}").into() };
                let form_name = crate::bug_reports::normalize_name(&inline.name).map_err(|_| {
                    OpError::Invalid {
                        field: field("name"),
                        message: "Name a form in 1 to 80 characters, on one line.".into(),
                    }
                })?;
                let collection_id = collections_named
                    .get(&inline.collection.trim().to_lowercase())
                    .copied()
                    .or_else(|| crate::ops::refs::parse_id(&inline.collection))
                    .ok_or_else(|| OpError::Invalid {
                        field: field("collection"),
                        message: "Name a collection made in this call, or give an existing \
                                  collection's id."
                            .into(),
                    })?;
                let tx = call.tx().await?;
                let definitions =
                    collections::field_definitions(tx, space, collection_id, false).await?;
                let mut questions = Vec::with_capacity(inline.fields.len());
                for wanted in &inline.fields {
                    let reference = wanted.field.trim();
                    let found = definitions.iter().find(|definition| {
                        definition.id.to_string() == reference
                            || definition.name.eq_ignore_ascii_case(reference)
                    });
                    let Some(found) = found else {
                        return Err(OpError::Invalid {
                            field: field("fields"),
                            message: format!("The collection has no field \"{reference}\".").into(),
                        });
                    };
                    questions.push(crate::collection_forms::FormField {
                        field: found.id,
                        required: wanted.required,
                        show_if: None,
                    });
                }
                let form = crate::collection_forms::FormConfig {
                    intro: inline.intro.clone(),
                    fields: questions,
                    thanks: inline.thanks.clone(),
                    daily_limit: None,
                    ..Default::default()
                };
                let form =
                    match crate::bug_reports::checked_form_in(tx, space, collection_id, &form)
                        .await?
                    {
                        Ok(form) => form,
                        Err(message) => {
                            return Err(OpError::Invalid {
                                field: field("fields"),
                                message: message.into(),
                            });
                        }
                    };
                // The token is never returned: its stored answer would keep
                // it. The form's settings make a link to share.
                let (form_id, _token) = crate::bug_reports::insert_link(
                    tx,
                    &maker,
                    space,
                    &form_name,
                    Some((collection_id, form)),
                )
                .await?;
                forms_named.insert(form_name.to_lowercase(), form_id);
                made.push(("form", form_id));
            }

            // Automation rules, acting as the caller.
            let fresh = made
                .iter()
                .filter(|(kind, _)| *kind == "collection")
                .map(|(_, id)| *id)
                .collect::<Vec<_>>();
            for (index, inline) in input.automations.iter().enumerate() {
                let mut rule = inline.rule.clone();
                if let Some(collection) = inline.collection.as_deref() {
                    let id = collections_named
                        .get(&collection.trim().to_lowercase())
                        .ok_or_else(|| OpError::Invalid {
                            field: format!("automations[{index}].collection").into(),
                            message: "Name a collection made in this call.".into(),
                        })?;
                    if let Some(trigger) = rule.get_mut("trigger").and_then(Value::as_object_mut) {
                        trigger.insert("collectionId".into(), id.to_string().into());
                    }
                }
                let rule = serde_json::from_value::<crate::automations::CreateRule>(rule).map_err(
                    |error| OpError::Invalid {
                        field: format!("automations[{index}].rule").into(),
                        message: error
                            .to_string()
                            .split(" at line ")
                            .next()
                            .unwrap_or("This rule can't be read.")
                            .to_owned()
                            .into(),
                    },
                )?;
                let prepared =
                    crate::automations::prepare(state, &maker, space, rule, &fresh).await?;
                let tx = call.tx().await?;
                let id =
                    crate::automations::insert(tx, &maker, workspace, space, &prepared).await?;
                made.push(("automation", id));
            }
        }

        // Its parts and its navigation.
        let tx = call.tx().await?;
        let module = super::get(tx, workspace, module_id)
            .await?
            .ok_or(OpError::Unavailable)?;
        for (kind, id) in &made {
            super::link_part(tx, &module, kind, *id, actor, confined).await?;
        }
        for (index, part) in input.parts.iter().enumerate() {
            let kind = part_kind(&part.kind).map_err(|_| OpError::Invalid {
                field: format!("parts[{index}].kind").into(),
                message: "A part is a collection, form, automation or page.".into(),
            })?;
            super::link_part(tx, &module, kind, part.id.id(), actor, confined)
                .await
                .map_err(|error| match error {
                    crate::error::AppError::NotFound(_) => OpError::Invalid {
                        field: format!("parts[{index}].id").into(),
                        message: "No such part here.".into(),
                    },
                    other => other.into(),
                })?;
        }
        for entry in &input.nav {
            let mut target = entry.target.clone();
            // A name from this call becomes the part's id.
            for (slot, named_here) in [("collection", &collections_named), ("form", &forms_named)] {
                if let Some(named) = target.get(slot).and_then(Value::as_str)
                    && let Some(id) = named_here.get(&named.trim().to_lowercase())
                {
                    target[slot] = id.to_string().into();
                }
            }
            super::add_nav(tx, &module, &entry.label, entry.icon.as_deref(), &target).await?;
        }
        call.audit_target(
            "module",
            module_id,
            json!({
                "key": key,
                "parts": made.iter().map(|(kind, _)| *kind).chain(input.parts.iter().map(|_| "linked")).collect::<Vec<_>>(),
                "nav": input.nav.len(),
            }),
        );
        view(call, module_id).await
    }
}

// -------------------------------------------------------------- module_open

pub struct ModuleOpen;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleIdInput {
    /// The module's id.
    pub module_id: IdRef,
}

impl Targeted for ModuleIdInput {
    fn target(&self) -> Target {
        Target::thing("module", self.module_id.id())
    }
}

impl Operation for ModuleOpen {
    const NAME: &'static str = "module_open";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Open a module";
    const DESCRIPTION: &'static str = "Open a built module: its key, whether it is on, its parts \
        (collections, forms, automations, pages) with their names, and its navigation entries. \
        module_list lists them.";

    type Input = ModuleIdInput;
    type Output = ModuleView;

    async fn run(call: &mut OpCall<'_>, input: ModuleIdInput) -> Result<ModuleView, OpError> {
        view(call, input.module_id.id()).await
    }
}

// ------------------------------------------------------------ module_update

pub struct ModuleUpdate;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleUpdateInput {
    pub module_id: IdRef,
    /// The revision you read (module_open); a newer one is a conflict.
    pub expected_revision: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Navigation entries to add.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nav: Vec<NavEntry>,
    /// Navigation entries to take out, by id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_nav: Vec<IdRef>,
}

impl Targeted for ModuleUpdateInput {
    fn target(&self) -> Target {
        Target::Thing {
            kind: "module",
            id: self.module_id.id(),
            expected_version: Some(self.expected_revision),
        }
    }
}

fn revision_conflict(current: i64) -> OpError {
    OpError::Conflict {
        reason: ConflictReason::Version {
            current_version: current,
        },
        message: format!(
            "The module changed first (now revision {current}). Read it again (module_open), \
             then retry with expectedRevision {current}."
        )
        .into(),
        current: None,
    }
}

impl Operation for ModuleUpdate {
    const NAME: &'static str = "module_update";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesBuild);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Change a module";
    const DESCRIPTION: &'static str = "Change a built module's name, description or icon, and \
        add or take out navigation entries. Pass the revision you read as expectedRevision. \
        Parts change with module_part_link and module_part_unlink.";
    const EMITS: &'static [&'static str] = &["module.updated"];

    type Input = ModuleUpdateInput;
    type Output = ModuleView;

    async fn run(call: &mut OpCall<'_>, input: ModuleUpdateInput) -> Result<ModuleView, OpError> {
        let module = locked(call, input.module_id.id()).await?;
        live(&module)?;
        if module.revision != input.expected_revision {
            return Err(revision_conflict(module.revision));
        }
        let name = input.name.as_deref().map(str::trim).unwrap_or(&module.name);
        check_text(name, input.description.as_deref(), input.icon.as_deref())?;
        let tx = call.tx().await?;
        sqlx::query(
            "UPDATE modules SET name = $2, description = COALESCE($3, description), \
             icon = CASE WHEN $4::text IS NULL THEN icon ELSE NULLIF($4, '') END \
             WHERE id = $1",
        )
        .bind(module.id)
        .bind(name)
        .bind(input.description.as_deref())
        .bind(input.icon.as_deref())
        .execute(&mut **tx)
        .await
        .map_err(|error| OpError::logged(Self::NAME, &error.into()))?;
        for entry in &input.remove_nav {
            super::unlink_part(tx, &module, "nav", entry.id()).await?;
        }
        for entry in &input.nav {
            super::add_nav(
                tx,
                &module,
                &entry.label,
                entry.icon.as_deref(),
                &entry.target,
            )
            .await?;
        }
        super::touch(tx, module.id).await?;
        call.audit_target("module", module.id, json!({ "key": module.module_key }));
        view(call, module.id).await
    }
}

// ------------------------------------------- module_part_link and _unlink

pub struct ModulePartLink;
pub struct ModulePartUnlink;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModulePartInput {
    pub module_id: IdRef,
    /// The part: {kind: collection, form, automation or page; id}.
    pub part: PartRef,
}

impl Targeted for ModulePartInput {
    fn target(&self) -> Target {
        Target::thing("module", self.module_id.id())
    }
}

impl Operation for ModulePartLink {
    const NAME: &'static str = "module_part_link";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesBuild);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Add a part to a module";
    const DESCRIPTION: &'static str = "Make an existing collection, form, automation or page a \
        part of a built module, so it is switched on and off with it. The part must be in the \
        module's workspace (and space, for a space's module) and in no other module. Adding \
        it again changes nothing.";
    const EMITS: &'static [&'static str] = &["module.updated"];

    type Input = ModulePartInput;
    type Output = ModuleView;

    async fn run(call: &mut OpCall<'_>, input: ModulePartInput) -> Result<ModuleView, OpError> {
        let kind = part_kind(&input.part.kind)?;
        let module = locked(call, input.module_id.id()).await?;
        live(&module)?;
        let actor = call.account_id();
        let confined = call.ctx.caller.confined_space;
        let tx = call.tx().await?;
        if super::link_part(tx, &module, kind, input.part.id.id(), actor, confined).await? {
            super::touch(tx, module.id).await?;
            // Joining a switched-off module hides it: its readers refetch.
            announce_part(tx, kind, input.part.id.id()).await?;
        }
        call.audit_target(
            "module",
            module.id,
            json!({ "part": kind, "partId": input.part.id.id() }),
        );
        view(call, module.id).await
    }
}

impl Operation for ModulePartUnlink {
    const NAME: &'static str = "module_part_unlink";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesBuild);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Take a part out of a module";
    const DESCRIPTION: &'static str = "Take a part out of a built module. The collection, form, \
        automation or page stays, as a plain one that the module's switch no longer hides.";
    const EMITS: &'static [&'static str] = &["module.updated"];

    type Input = ModulePartInput;
    type Output = ModuleView;

    async fn run(call: &mut OpCall<'_>, input: ModulePartInput) -> Result<ModuleView, OpError> {
        let kind = part_kind(&input.part.kind)?;
        let module = locked(call, input.module_id.id()).await?;
        live(&module)?;
        let tx = call.tx().await?;
        if super::unlink_part(tx, &module, kind, input.part.id.id()).await? {
            super::touch(tx, module.id).await?;
            // Leaving a switched-off module shows it again: readers refetch.
            announce_part(tx, kind, input.part.id.id()).await?;
        }
        call.audit_target(
            "module",
            module.id,
            json!({ "part": kind, "partId": input.part.id.id() }),
        );
        view(call, module.id).await
    }
}

// --------------------------------------------------------------- module_set

pub struct ModuleSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleSetInput {
    pub module_id: IdRef,
    /// On (true) or off (false).
    pub on: bool,
}

impl Targeted for ModuleSetInput {
    fn target(&self) -> Target {
        Target::thing("module", self.module_id.id())
    }
}

/// Every collection part changed for its readers: tabs refetch them.
async fn announce(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    module_id: Uuid,
) -> Result<(), OpError> {
    for (space, collection) in super::collection_parts(tx, module_id).await? {
        crate::outbox::enqueue_collection_updated(tx, space, collection, &[]).await?;
    }
    Ok(())
}

/// A collection that joined or left a module changed for its readers.
async fn announce_part(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    kind: &str,
    object_id: Uuid,
) -> Result<(), OpError> {
    if kind != "collection" {
        return Ok(());
    }
    let space = sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM collections WHERE id = $1")
        .bind(object_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| OpError::logged("module_part_link", &error.into()))?;
    if let Some(space) = space {
        crate::outbox::enqueue_collection_updated(tx, space, object_id, &[]).await?;
    }
    Ok(())
}

impl Operation for ModuleSet {
    const NAME: &'static str = "module_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Switch a module on or off";
    const DESCRIPTION: &'static str = "Switch a built module on or off where it lives (its space, \
        or the workspace). Off hides its collections and their records, closes its forms and \
        holds its automations, keeping every record; on brings them all back.";
    const EMITS: &'static [&'static str] = &["module.switched"];

    type Input = ModuleSetInput;
    type Output = ModuleView;

    async fn run(call: &mut OpCall<'_>, input: ModuleSetInput) -> Result<ModuleView, OpError> {
        let module = locked(call, input.module_id.id()).await?;
        live(&module)?;
        let actor = call.account_id();
        let workspace = call.ctx.workspace_id;
        let tx = call.tx().await?;
        let scope = match module.space_id {
            Some(space) => crate::modules::state::SwitchScope::Space(space),
            None => crate::modules::state::SwitchScope::Workspace,
        };
        let changed = crate::modules::state::write_switch(
            tx,
            workspace,
            scope,
            &module.module_key,
            input.on,
            actor,
        )
        .await?;
        if changed {
            super::touch(tx, module.id).await?;
            announce(tx, module.id).await?;
        }
        call.audit_target(
            "module",
            module.id,
            json!({ "on": input.on, "changed": changed }),
        );
        view(call, module.id).await
    }
}

// --------------------------------------------- module_archive and _restore

pub struct ModuleArchive;
pub struct ModuleRestore;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveParts {
    /// Hide the parts with the module, until module_restore.
    Archive,
    /// Keep the parts as plain collections, forms, automations and pages.
    Keep,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModuleArchiveInput {
    pub module_id: IdRef,
    /// archive (hidden with it, restorable) or keep (they live on as plain
    /// objects).
    pub parts: ArchiveParts,
}

impl Targeted for ModuleArchiveInput {
    fn target(&self) -> Target {
        Target::thing("module", self.module_id.id())
    }
}

impl Operation for ModuleArchive {
    const NAME: &'static str = "module_archive";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const DESTRUCTIVE: bool = true;
    const TITLE: &'static str = "Archive a module";
    const DESCRIPTION: &'static str = "Archive a built module. parts: archive hides its parts with \
        it (module_restore brings them back); keep takes them out of it, so they live on as \
        plain collections, forms, automations and pages. Nothing is deleted.";
    const EMITS: &'static [&'static str] = &["module.removed"];

    type Input = ModuleArchiveInput;
    type Output = ModuleView;

    async fn run(call: &mut OpCall<'_>, input: ModuleArchiveInput) -> Result<ModuleView, OpError> {
        let module = locked(call, input.module_id.id()).await?;
        live(&module)?;
        let tx = call.tx().await?;
        // Readers of its collections hear of it before they go or stay.
        announce(tx, module.id).await?;
        super::archive(tx, &module, input.parts == ArchiveParts::Keep).await?;
        call.audit_target("module", module.id, json!({ "parts": input.parts }));
        view(call, module.id).await
    }
}

impl Operation for ModuleRestore {
    const NAME: &'static str = "module_restore";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Restore a module";
    const DESCRIPTION: &'static str = "Bring an archived built module back, with the parts it \
        hid. One archived with parts: keep comes back empty, since its parts left it.";
    const EMITS: &'static [&'static str] = &["module.updated"];

    type Input = ModuleIdInput;
    type Output = ModuleView;

    async fn run(call: &mut OpCall<'_>, input: ModuleIdInput) -> Result<ModuleView, OpError> {
        let module = locked(call, input.module_id.id()).await?;
        if module.archived_at.is_none() {
            return view(call, module.id).await;
        }
        let tx = call.tx().await?;
        super::restore(tx, &module).await?;
        announce(tx, module.id).await?;
        call.audit_target("module", module.id, json!({}));
        view(call, module.id).await
    }
}
