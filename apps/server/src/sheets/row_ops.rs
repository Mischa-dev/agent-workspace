//! `sheet_row_add` (C2.22): add a row under a tab's last one, with values by
//! header, or make one from a message: the message's text goes under a
//! Message column, the row's sheet links back through the conversation's
//! "made into" line when the caller says so.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use uuid::Uuid;

use super::{Change, ProblemKind};
use crate::ops::{
    ConflictReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm, Target, Targeted,
    refs::{IdRef, MessageRef},
};

const SHEET: &str = "That sheet";
/// The header a message's text goes under when no values are given.
const MESSAGE_HEADER: &str = "Message";
const MAX_VALUES: usize = 50;

pub struct SheetRowAdd;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SheetRowAddInput {
    /// The sheet: its id or link.
    pub sheet_id: IdRef,
    /// The tab, by name; leave out for the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    /// Values by header (row 1), up to 50. A header that isn't there yet is
    /// added. Made from a message and left out: its text under "Message".
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub values: serde_json::Map<String, Json>,
    /// The message the row is made from (its id, or a link), in a
    /// conversation of the sheet's space that you can read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_message_id: Option<MessageRef>,
    /// With fromMessageId: also say in that conversation that you made the
    /// message into a row of this sheet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub say_so: bool,
}

impl Targeted for SheetRowAddInput {
    fn target(&self) -> Target {
        Target::thing("sheet", self.sheet_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SheetRowAdded {
    #[schemars(with = "String")]
    pub sheet_id: Uuid,
    /// The row it went in (1 is the header row).
    pub row: i32,
    /// The sheet's revision now.
    pub revision: i64,
}

impl Operation for SheetRowAdd {
    const NAME: &'static str = "sheet_row_add";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "sheets";
    const PERMISSION: Option<Perm> = Some(Perm::SheetsEdit);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Add a sheet row";
    const DESCRIPTION: &'static str = "Add a row under a sheet tab's last one, with values by \
        header (a missing header is added to row 1), or make one from a message (fromMessageId; \
        its text goes under Message unless you pass values). With saySo the conversation says \
        you made it.";
    const EMITS: &'static [&'static str] = &["sheet.updated"];

    type Input = SheetRowAddInput;
    type Output = SheetRowAdded;

    async fn run(call: &mut OpCall<'_>, input: SheetRowAddInput) -> Result<SheetRowAdded, OpError> {
        let sheet_id = input.sheet_id.id();
        let actor = call.account_id();
        let space = call.ctx.space_id.ok_or(OpError::not_found(SHEET))?;
        if input.values.len() > MAX_VALUES {
            return Err(OpError::invalid("values", "Give up to 50 values."));
        }
        let tx = call.tx().await?;
        let sheet = super::lock(tx, space, sheet_id)
            .await?
            .ok_or(OpError::not_found(SHEET))?;
        if sheet.archived {
            return Err(OpError::Conflict {
                reason: ConflictReason::State { code: "archived" },
                message: format!("{} is archived. Restore it to change it.", sheet.name).into(),
                current: None,
            });
        }
        let source = match input.from_message_id {
            None => None,
            Some(message) => Some(
                crate::discussion::message_in_space(tx, message.id(), space, actor)
                    .await?
                    .ok_or_else(|| {
                        OpError::invalid(
                            "fromMessageId",
                            "That isn't a message you can read in this sheet's space.",
                        )
                    })?,
            ),
        };
        let mut values = input.values.into_iter().collect::<Vec<_>>();
        if values.is_empty() {
            match &source {
                Some((_, _, text)) if !text.trim().is_empty() => {
                    values.push((MESSAGE_HEADER.to_owned(), json!(text.trim())));
                }
                _ => {
                    return Err(OpError::invalid(
                        "values",
                        "Give the row's values by header, or a message to make it from.",
                    ));
                }
            }
        }
        let applied = super::apply(
            tx,
            &sheet,
            actor,
            &[Change::Row {
                tab: input.tab.filter(|tab| !tab.trim().is_empty()),
                key_column: None,
                values,
            }],
            // Every cell it sets, a new header or two among them.
            2 * MAX_VALUES,
        )
        .await?
        .map_err(|problem| match problem.kind {
            ProblemKind::NotFound => OpError::Invalid {
                field: "tab".into(),
                message: problem.message.into(),
            },
            ProblemKind::Conflict | ProblemKind::Full => OpError::Conflict {
                reason: ConflictReason::State {
                    code: "sheet_limit",
                },
                message: problem.message.into(),
                current: None,
            },
            ProblemKind::Invalid => OpError::Invalid {
                field: "values".into(),
                message: problem.message.into(),
            },
        })?;
        let row = applied
            .changed
            .iter()
            .map(|cell| cell.row)
            .max()
            .unwrap_or(0);
        if applied.revision != sheet.revision {
            crate::outbox::enqueue_sheet_updated(tx, space, sheet.id, applied.revision).await?;
        }
        if let Some((message_id, conversation_id, _)) = source {
            crate::conversations::system_entries::made_from_message(
                tx,
                conversation_id,
                actor,
                "sheet_row",
                sheet.id,
                message_id,
                input.say_so,
            )
            .await?;
        }
        call.audit_target("sheet", sheet.id, json!({ "row": row }));
        Ok(SheetRowAdded {
            sheet_id: sheet.id,
            row,
            revision: applied.revision,
        })
    }
}
