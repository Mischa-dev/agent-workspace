//! Ask about a selection (C1.31): `whiteboard_ask` starts a thread in the
//! board's discussion anchored to the chosen elements, with a picture of
//! them, and mentions the account asked, so an agent gets a reply request
//! whose thread is about those elements and carries what they look like.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{render, store};
use crate::{
    discussion::{self, CommentBody, CommentPlace, SubjectKind},
    error::AppError,
    files::{self, Destination, Upload},
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
    ops::{Idempotency, OpCall, OpError, OpKind, Operation, Target, Targeted, refs::IdRef},
};

/// What a question's first call keeps for its retries: the picture and
/// where the thread was anchored.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AskKept {
    file_id: Uuid,
    anchor: serde_json::Value,
}

/// Most elements one question is about (a board anchor's limit).
const MAX_ELEMENTS: usize = 50;
/// The longest question.
const MAX_QUESTION: usize = 4_000;
/// The picture's scale: sharp enough to read stickies.
const PICTURE_SCALE: f64 = 2.0;

pub struct WhiteboardAsk;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WhiteboardAskInput {
    /// The board: its id or a link to it.
    pub whiteboard_id: IdRef,
    /// The elements the question is about, 1 to 50.
    pub element_ids: Vec<String>,
    /// Who is asked: an account in the board's space, such as an agent.
    pub account_id: IdRef,
    /// The question.
    pub question: String,
}

impl Targeted for WhiteboardAskInput {
    fn target(&self) -> Target {
        Target::thing("whiteboard", self.whiteboard_id.id())
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WhiteboardAsked {
    /// The thread about the elements.
    #[schemars(with = "String")]
    pub thread_id: Uuid,
    #[schemars(with = "String")]
    pub message_id: Uuid,
    /// The board's comments conversation.
    #[schemars(with = "String")]
    pub conversation_id: Uuid,
    /// The picture of the elements, attached to the question.
    #[schemars(with = "String")]
    pub file_id: Uuid,
}

impl Operation for WhiteboardAsk {
    const NAME: &'static str = "whiteboard_ask";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "whiteboards";
    const PERMISSION: Option<crate::ops::Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Ask about part of a board";
    const DESCRIPTION: &'static str = "Ask someone, such as an agent, about some elements of a \
        whiteboard: starts a thread in the board's discussion anchored to them, with a picture \
        of them attached, mentioning the account asked (an agent is asked to reply there).";
    const EMITS: &'static [&'static str] = &["message.created"];

    type Input = WhiteboardAskInput;
    type Output = WhiteboardAsked;

    async fn run(
        call: &mut OpCall<'_>,
        input: WhiteboardAskInput,
    ) -> Result<WhiteboardAsked, OpError> {
        let state = call.state;
        let board_id = input.whiteboard_id.id();
        let asked = input.account_id.id();
        let question = input.question.trim();
        if question.is_empty() || question.chars().count() > MAX_QUESTION {
            return Err(OpError::invalid(
                "question",
                "Ask something, in up to 4,000 characters.",
            ));
        }
        let mut ids = input.element_ids.clone();
        ids.sort();
        ids.dedup();
        if ids.is_empty() || ids.len() > MAX_ELEMENTS {
            return Err(OpError::invalid(
                "elementIds",
                "Name 1 to 50 elements of the board.",
            ));
        }
        let caller = super::Caller::from(call.ctx);
        let mut connection = crate::db::scoped_tx(&state.pool)
            .await
            .map_err(AppError::from)?;
        let board = super::access(&mut connection, &caller, board_id, false)
            .await
            .map_err(|_| OpError::not_found("That board"))?;
        if board.organization_id != call.ctx.workspace_id || board.deleted {
            return Err(OpError::not_found("That board"));
        }
        // Only someone in the board's space can be asked about it.
        let reachable = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM project_memberships membership
                JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
                WHERE membership.project_id = $1 AND membership.actor_id = $2
                  AND membership.enabled
            )
            "#,
        )
        .bind(board.project_id)
        .bind(asked)
        .fetch_one(&mut *connection)
        .await
        .map_err(AppError::from)?;
        drop(connection);
        if !reachable {
            // Not found, so an account elsewhere isn't confirmed to exist.
            return Err(OpError::not_found(
                "Someone with that id in the board's space",
            ));
        }

        // The picture, drawn off the async workers.
        let elements = store::live_elements(&state.pool, board_id).await?;
        let chosen = elements
            .iter()
            .filter(|element| ids.iter().any(|id| id == super::scene::id(element)))
            .count();
        if chosen != ids.len() {
            return Err(OpError::invalid(
                "elementIds",
                "Each element must be on the board.",
            ));
        }
        let pictures = store::files_for(state, board_id, &elements).await?;
        let options = render::Options {
            only: Some(ids.clone()),
            background: true,
            dark: false,
        };
        let drawn = tokio::task::spawn_blocking(move || {
            let rendered = render::svg(&elements, &pictures, &options);
            let png = render::png(&rendered.svg, PICTURE_SCALE);
            (rendered.area, png)
        })
        .await
        .map_err(|_| OpError::Unavailable)?;
        let (area, png) = drawn;
        let (png, _, _) = png.map_err(|_| {
            OpError::invalid("elementIds", "Those elements are too big to picture.")
        })?;
        let anchor = json!({
            "kind": "elements",
            "elementIds": ids,
            "at": {"x": area.x + area.width, "y": area.y},
        });

        // The question goes in a new thread about the elements, with the
        // picture uploaded to the board's discussion first.
        let creator = discussion::ops::creator(call.ctx);
        let mut transaction = crate::db::scoped_tx(&state.pool)
            .await
            .map_err(AppError::from)?;
        let (comments_id, project_id) = discussion::open_for(
            &mut transaction,
            &creator,
            SubjectKind::Whiteboard,
            board_id,
        )
        .await?;
        transaction.commit().await.map_err(AppError::from)?;
        let key = match call.ctx.request_id.as_deref() {
            Some(key) => IdempotencyKey::parse(key),
            None => IdempotencyKey::parse(&Uuid::now_v7().to_string()),
        }
        .map_err(|_| OpError::invalid("clientRequestId", "Pass a short request id."))?;
        // A retry reuses the picture and anchor the first call kept, so the
        // question it posts is the same one and replays (the message's
        // request hash counts its files and its anchor), even when the
        // elements have moved since.
        let prepared = files::prepare_upload(Upload {
            name: "selection.png".to_owned(),
            declared_type: Some("image/png".to_owned()),
            bytes: png,
        })
        .await?;
        // The picture's bytes go to the store before any transaction opens;
        // a retry that finds the first picture takes them away again.
        // The board's comments are read by whoever sees the board, not by
        // members alone.
        let destination = Destination::Conversation {
            project_id,
            conversation_id: comments_id,
            members_only: false,
        };
        let staged = files::stage_prepared(state, destination, prepared).await?;
        let mut transaction = match crate::db::scoped_tx(&state.pool).await {
            Ok(transaction) => transaction,
            Err(error) => {
                files::abandon_staged(state, &staged).await;
                return Err(AppError::from(error).into());
            }
        };
        let scope = idempotency::resource_bound_scope("whiteboard", board_id, "ask_picture")?;
        let hash = RequestHash::for_value(&json!({
            "elementIds": ids,
            "accountId": asked,
            "question": question,
        }))?;
        let reserved =
            idempotency::reserve(&mut transaction, caller.actor_id, &scope, &key, hash).await;
        let reserved = match reserved {
            Ok(IdempotencyOutcome::Reserved(reservation)) => {
                IdempotencyOutcome::Reserved(reservation)
            }
            other => {
                files::abandon_staged(state, &staged).await;
                other?
            }
        };
        let (picture_id, anchor) = match reserved {
            IdempotencyOutcome::Replay(replay) => {
                let kept = replay.decode::<AskKept>()?;
                (kept.file_id, kept.anchor)
            }
            IdempotencyOutcome::Reserved(reservation) => {
                let row = files::store_prepared(
                    state,
                    &mut transaction,
                    caller.actor_id,
                    destination,
                    staged,
                )
                .await?;
                idempotency::complete(
                    &mut transaction,
                    &reservation,
                    axum::http::StatusCode::CREATED,
                    &AskKept {
                        file_id: row.id,
                        anchor: anchor.clone(),
                    },
                )
                .await?;
                if let Err(error) = transaction.commit().await {
                    files::discard_stored(state, &row).await;
                    return Err(AppError::from(error).into());
                }
                (row.id, anchor)
            }
        };
        let content = json!({
            "version": 1,
            "nodes": [
                {"type": "mention", "actorId": asked},
                {"type": "text", "text": format!(" {question}")},
            ],
        });
        let posted = discussion::post(
            state,
            call.ctx.into(),
            creator,
            SubjectKind::Whiteboard,
            board_id,
            CommentPlace::NewThread {
                anchor: Some(anchor),
            },
            key,
            CommentBody::Content(content),
            &[picture_id],
            None,
        )
        .await?;
        call.mark_replayed(posted.replayed);
        call.audit_target(
            "message",
            posted.message_id,
            json!({ "whiteboardId": board_id, "threadId": posted.thread_id }),
        );
        Ok(WhiteboardAsked {
            thread_id: posted.thread_id.ok_or(OpError::Unavailable)?,
            message_id: posted.message_id,
            conversation_id: posted.conversation_id,
            file_id: picture_id,
        })
    }
}
