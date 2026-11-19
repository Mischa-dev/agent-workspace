//! Files as operations (K5.20): direct upload to the store (start, then
//! finish), and a workspace's own bucket (see it, set it, stop using it).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::{
    FileView, Reader,
    upload::{self, Declared, NotDirect, Place, Unfinished},
    workspace_store::{self, NewBucket, Refused, WorkspaceStoreView},
};
use crate::{
    error::AppError,
    events::{Audience, NewEvent, catalog},
    ops::{
        ConflictReason, Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target,
        Targeted, refs::IdRef, target::Located,
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<FileUploadStart>()
        .add::<FileUploadFinish>()
        .add::<WorkspaceStoreGet>()
        .add::<WorkspaceStoreSet>()
        .add::<WorkspaceStoreRevoke>();
}

/// Stages one of these operations' events in the call's transaction.
async fn emit(
    connection: &mut sqlx::PgConnection,
    ty: &'static crate::events::EventType,
    workspace: Uuid,
    space: Option<Uuid>,
    audience: Audience,
    payload: serde_json::Value,
    file: Option<Uuid>,
) -> Result<(), AppError> {
    crate::events::emit(
        connection,
        NewEvent {
            id: Uuid::now_v7(),
            ty,
            workspace_id: Some(workspace),
            space_id: space,
            thing: file.map(|id| (id, "file")),
            access_thing: None,
            version: None,
            changes: &[],
            audience,
            payload,
            actor: None,
            legacy_type: None,
        },
    )
    .await
    .map(drop)
}

fn reader(call: &OpCall<'_>) -> Reader {
    Reader::Browser {
        actor_id: call.account_id(),
    }
}

/// Checks the caller may put a file in `place`: posting in a conversation,
/// editing a task. Its own short transaction, so nothing stays open while a
/// store is asked anything.
async fn destination(
    call: &OpCall<'_>,
    place: Place,
) -> Result<(super::Destination, usize), OpError> {
    let permission = match place {
        Place::Conversation(_) => Perm::ChannelsPost,
        Place::Task { .. } => Perm::TasksEdit,
    };
    let located = Located {
        workspace_id: call.ctx.workspace_id,
        space_id: call.ctx.space_id,
    };
    let mut transaction = crate::db::scoped_tx(&call.state.pool)
        .await
        .map_err(AppError::from)?;
    crate::ops::access_shim::check(
        &mut transaction,
        &call.ctx.caller,
        &call.ctx.target,
        &located,
        Some(permission),
        true,
        call.ctx.op,
    )
    .await?;
    let found = upload::destination(&mut transaction, &call.ctx.into(), &call.ctx.into(), place)
        .await
        .map_err(|error| match error {
            AppError::NotFound(_) | AppError::Forbidden => OpError::not_found(match place {
                Place::Conversation(_) => "That conversation",
                Place::Task { .. } => "That task",
            }),
            other => other.into(),
        })?;
    transaction.commit().await.map_err(AppError::from)?;
    Ok(found)
}

/// Start a direct upload.
pub struct FileUploadStart;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileUploadStartInput {
    /// The conversation the file is for (sent with a message there later).
    #[serde(default)]
    pub conversation_id: Option<IdRef>,
    /// Or the task: its own file, or (`forComment`) for a comment on it.
    #[serde(default)]
    pub task_id: Option<IdRef>,
    #[serde(default)]
    pub for_comment: bool,
    /// The file's name.
    pub name: String,
    /// Its type, such as application/pdf; worked out from the name if left out.
    #[serde(default)]
    pub content_type: Option<String>,
    /// Its exact size in bytes.
    pub size: u64,
    /// Its SHA-256, as 64 hex characters. The store refuses bytes that
    /// don't match.
    pub sha256: String,
}

impl FileUploadStartInput {
    fn place(&self) -> Result<Place, OpError> {
        match (self.conversation_id, self.task_id) {
            (Some(conversation), None) if !self.for_comment => {
                Ok(Place::Conversation(conversation.id()))
            }
            (None, Some(task)) => Ok(Place::Task {
                id: task.id(),
                for_comment: self.for_comment,
            }),
            _ => Err(OpError::invalid(
                "conversationId",
                "Pass either conversationId or taskId (forComment only with a task).",
            )),
        }
    }
}

impl Targeted for FileUploadStartInput {
    fn target(&self) -> Target {
        match (self.conversation_id, self.task_id) {
            (Some(conversation), _) => Target::thing("conversation", conversation.id()),
            (None, Some(task)) => Target::thing("task", task.id()),
            (None, None) => Target::Account,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileUploadStarted {
    /// Pass it to file_upload_finish once the bytes are sent.
    #[schemars(with = "String")]
    pub upload_id: Uuid,
    /// Always PUT.
    pub method: &'static str,
    /// Where to send the bytes, as the whole request body.
    pub url: String,
    /// Headers to send with them, exactly as given.
    pub headers: BTreeMap<String, String>,
    /// The link stops working then; start again for a new one.
    pub expires_at: DateTime<Utc>,
}

impl Operation for FileUploadStart {
    const NAME: &'static str = "file_upload_start";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    // Posting in the conversation, or editing the task: checked in `run`.
    const PERMISSION: Option<Perm> = None;
    // The answer is a signed link, not kept for a replay; a retry starts
    // another upload, and one never finished is swept after a day.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const EMITS: &'static [&'static str] = &["file.upload_started"];
    const TITLE: &'static str = "Start a direct upload";
    const DESCRIPTION: &'static str = "Upload a file of any size straight to the workspace's file \
        store, without sending its bytes through this server. Give its name, exact size and \
        SHA-256; send the bytes as the body of a PUT to the url, with the headers given, within \
        15 minutes; then call file_upload_finish with the uploadId. Where the server keeps files \
        itself this answers direct_upload_unavailable: use file_upload instead.";

    type Input = FileUploadStartInput;
    type Output = FileUploadStarted;

    async fn run(
        call: &mut OpCall<'_>,
        input: FileUploadStartInput,
    ) -> Result<FileUploadStarted, OpError> {
        let place = input.place()?;
        let (destination, limit) = destination(call, place).await?;
        let account = call.account_id();
        let state = call.state;
        let tx = call.tx().await?;
        let started = upload::start(
            state,
            tx,
            account,
            destination,
            limit,
            Declared {
                name: input.name,
                content_type: input.content_type,
                size: input.size,
                sha256: input.sha256,
            },
        )
        .await
        .map_err(|error| match error {
            NotDirect::ThroughServer => OpError::Conflict {
                reason: ConflictReason::State {
                    code: "direct_upload_unavailable",
                },
                message: "This server keeps files itself: upload with file_upload instead.".into(),
                current: None,
            },
            NotDirect::Refused(AppError::Validation("sha256")) => {
                OpError::invalid("sha256", "Give the file's SHA-256 as 64 hex characters.")
            }
            NotDirect::Refused(AppError::Validation("size")) => {
                OpError::invalid("size", "Give the file's size in bytes; it can't be empty.")
            }
            NotDirect::Refused(error) => error.into(),
        })?;
        let workspace = call.ctx.workspace_id;
        let tx = call.tx().await?;
        emit(
            tx,
            &catalog::FILE_UPLOAD_STARTED,
            workspace,
            Some(destination.project_id()),
            Audience::Account(account),
            json!({ "uploadId": started.upload_id }),
            None,
        )
        .await?;
        call.audit_target(
            "file_upload",
            started.upload_id,
            json!({ "size": input.size }),
        );
        Ok(FileUploadStarted {
            upload_id: started.upload_id,
            method: "PUT",
            url: started.url.to_string(),
            headers: started.headers.into_iter().collect(),
            expires_at: started.expires_at,
        })
    }
}

/// Finish a direct upload.
pub struct FileUploadFinish;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileUploadFinishInput {
    /// The uploadId file_upload_start gave.
    pub upload_id: IdRef,
}

impl Targeted for FileUploadFinishInput {
    fn target(&self) -> Target {
        Target::thing("file_upload", self.upload_id.id())
    }
}

impl Operation for FileUploadFinish {
    const NAME: &'static str = "file_upload_finish";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "workspace";
    // The destination is checked again in `run`, as at the start.
    const PERMISSION: Option<Perm> = None;
    // A finished upload answers with its file again.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const EMITS: &'static [&'static str] = &["file.attached"];
    const TITLE: &'static str = "Finish a direct upload";
    const DESCRIPTION: &'static str = "After sending a file's bytes to the link file_upload_start \
        gave, make it a file: the server checks what arrived is the size (and SHA-256) declared, \
        and refuses and deletes it otherwise. A picture loses its location data first. Returns \
        the file; pass its id in message_send's or task_comment's attachmentIds as with \
        file_upload. Calling it again returns the same file.";

    type Input = FileUploadFinishInput;
    type Output = FileView;

    async fn run(call: &mut OpCall<'_>, input: FileUploadFinishInput) -> Result<FileView, OpError> {
        let account = call.account_id();
        let row = {
            let mut connection = crate::db::scoped_tx(&call.state.pool)
                .await
                .map_err(AppError::from)?;
            upload::upload_of(&mut connection, account, input.upload_id.id())
                .await
                .map_err(|_| OpError::not_found("That upload"))?
        };
        let replay = row.file_id.is_some();
        let (destination, _) = destination(call, row.place()).await?;
        let file_id = upload::finish(call.state, &row, destination)
            .await
            .map_err(|error| match error {
                Unfinished::NotArrived => OpError::Conflict {
                    reason: ConflictReason::State {
                        code: "upload_not_arrived",
                    },
                    message: "No bytes have arrived yet: send them to the upload link first."
                        .into(),
                    current: None,
                },
                Unfinished::Mismatch(field) => OpError::invalid(
                    field,
                    "What arrived isn't what was declared, so it was deleted. Start again.",
                ),
                Unfinished::Picture(AppError::PayloadTooLarge) => OpError::invalid(
                    "size",
                    "Pictures are at most 25 MB, since their location data is removed first. It was deleted.",
                ),
                Unfinished::Picture(error) | Unfinished::Failed(error) => error.into(),
            })?;
        call.mark_replayed(replay);
        if !replay
            && let super::Destination::Task {
                project_id,
                work_item_id,
                for_comment: false,
            } = destination
        {
            crate::work_items::notify_files_changed(&call.state.pool, project_id, work_item_id)
                .await?;
        }
        if !replay
            && let super::Destination::Task {
                project_id,
                for_comment: false,
                ..
            } = destination
        {
            let workspace = call.ctx.workspace_id;
            let tx = call.tx().await?;
            emit(
                tx,
                &catalog::FILE_ATTACHED,
                workspace,
                Some(project_id),
                Audience::Space,
                json!({}),
                Some(file_id),
            )
            .await?;
        }
        call.audit_target("file", file_id, json!({ "upload": row.id }));
        let file = upload::file_of(call.state, reader(call), file_id).await?;
        Ok(file.view())
    }
}

/// See the workspace's own bucket.
pub struct WorkspaceStoreGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceStoreGetInput {}

impl Targeted for WorkspaceStoreGetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceStoreOutput {
    /// Where new uploads go; absent when they go to the server's own store.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<WorkspaceStoreView>,
    /// Whether this call started moving the workspace's existing files
    /// into it.
    pub moving: bool,
}

impl Operation for WorkspaceStoreGet {
    const NAME: &'static str = "workspace_store_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::SettingsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "See the workspace's file bucket";
    const DESCRIPTION: &'static str = "Where the workspace's new uploads are stored: its own \
        S3-compatible bucket (address, bucket, region, prefix; never its keys), or nothing when \
        they go to the server's own store.";

    type Input = WorkspaceStoreGetInput;
    type Output = WorkspaceStoreOutput;

    async fn run(
        call: &mut OpCall<'_>,
        _input: WorkspaceStoreGetInput,
    ) -> Result<WorkspaceStoreOutput, OpError> {
        let workspace = call.ctx.workspace_id;
        let tx = call.tx().await?;
        let store = workspace_store::current(tx, workspace).await?;
        Ok(WorkspaceStoreOutput {
            store,
            moving: false,
        })
    }
}

/// Point the workspace's new uploads at its own bucket.
pub struct WorkspaceStoreSet;

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceStoreSetInput {
    /// The S3-compatible service's address alone, such as https://s3.us-east-1.amazonaws.com.
    pub endpoint: String,
    pub bucket: String,
    /// Such as us-east-1 (auto for some services).
    pub region: String,
    /// Put before every key, such as workspace/ (optional).
    #[serde(default)]
    pub prefix: String,
    /// Address the bucket as endpoint/bucket (most self-hosted stores)
    /// rather than bucket.endpoint.
    #[serde(default)]
    pub path_style: bool,
    pub access_key_id: String,
    /// Kept encrypted; never shown again.
    pub secret_access_key: String,
    /// Also move the files already stored into it, a batch at a time.
    #[serde(default)]
    pub move_existing: bool,
}

// The keys never reach a log line, even a debug one.
impl std::fmt::Debug for WorkspaceStoreSetInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceStoreSetInput")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .finish_non_exhaustive()
    }
}

impl Targeted for WorkspaceStoreSetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for WorkspaceStoreSet {
    const NAME: &'static str = "workspace_store_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::SettingsManage);
    // Setting the same bucket again does no harm; the keys are never kept
    // for a replay.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const EMITS: &'static [&'static str] = &["workspace.file_store_changed"];
    const TITLE: &'static str = "Use the workspace's own file bucket";
    const DESCRIPTION: &'static str = "Store the workspace's new uploads in its own S3-compatible \
        bucket. The server first writes, reads and deletes a test object there and refuses the \
        bucket if any step fails. The keys are kept encrypted and never shown again. Files \
        already stored stay where they are unless moveExisting is set. The bucket needs CORS \
        allowing PUT from the app's address for direct uploads from a browser.";

    type Input = WorkspaceStoreSetInput;
    type Output = WorkspaceStoreOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: WorkspaceStoreSetInput,
    ) -> Result<WorkspaceStoreOutput, OpError> {
        let workspace = call.ctx.workspace_id;
        let account = call.account_id();
        // The probe waits on the bucket: before any transaction opens.
        let probed = workspace_store::probe(
            call.state,
            workspace,
            NewBucket {
                endpoint: &input.endpoint,
                bucket: &input.bucket,
                region: &input.region,
                prefix: &input.prefix,
                path_style: input.path_style,
                access_key_id: &input.access_key_id,
                secret_access_key: &input.secret_access_key,
            },
        )
        .await
        .map_err(refused)?;
        let state = call.state;
        let tx = call.tx().await?;
        let store = workspace_store::record(state, tx, workspace, account, probed).await?;
        let moving = if input.move_existing {
            let tx = call.tx().await?;
            super::migrate::start_in(tx, &store.store_id, workspace).await?;
            true
        } else {
            false
        };
        let tx = call.tx().await?;
        emit(
            tx,
            &catalog::WORKSPACE_FILE_STORE_CHANGED,
            workspace,
            None,
            Audience::Workspace,
            json!({ "storeId": store.store_id }),
            None,
        )
        .await?;
        call.audit_target(
            "workspace",
            workspace,
            json!({ "fileStore": store.store_id, "moveExisting": moving }),
        );
        Ok(WorkspaceStoreOutput {
            store: Some(store),
            moving,
        })
    }
}

fn refused(refused: Refused) -> OpError {
    match refused {
        Refused::Field(field, message) => OpError::invalid(field, message),
        Refused::Unreachable => OpError::invalid(
            "endpoint",
            "The bucket couldn't be written, read and deleted with those keys. Check the \
             address, bucket, region and keys, and that the keys may put, get and delete objects.",
        ),
        Refused::Other(error) => error.into(),
    }
}

/// Stop using the workspace's own bucket for new uploads.
pub struct WorkspaceStoreRevoke;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceStoreRevokeInput {}

impl Targeted for WorkspaceStoreRevokeInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for WorkspaceStoreRevoke {
    const NAME: &'static str = "workspace_store_revoke";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::SettingsManage);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const EMITS: &'static [&'static str] = &["workspace.file_store_changed"];
    const TITLE: &'static str = "Stop using the workspace's own file bucket";
    const DESCRIPTION: &'static str = "New uploads go to the server's own store again. Files \
        already in the workspace's bucket stay there and are still read from it, so keep the \
        bucket and its keys until they are moved.";

    type Input = WorkspaceStoreRevokeInput;
    type Output = WorkspaceStoreOutput;

    async fn run(
        call: &mut OpCall<'_>,
        _input: WorkspaceStoreRevokeInput,
    ) -> Result<WorkspaceStoreOutput, OpError> {
        let workspace = call.ctx.workspace_id;
        let tx = call.tx().await?;
        if workspace_store::retire(tx, workspace).await? {
            emit(
                tx,
                &catalog::WORKSPACE_FILE_STORE_CHANGED,
                workspace,
                None,
                Audience::Workspace,
                json!({ "storeId": null }),
                None,
            )
            .await?;
        }
        call.audit_target("workspace", workspace, json!({ "fileStore": null }));
        Ok(WorkspaceStoreOutput {
            store: None,
            moving: false,
        })
    }
}
