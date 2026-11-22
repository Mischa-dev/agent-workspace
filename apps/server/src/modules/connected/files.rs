//! File sources (M1.18, ruling R23): an outside store attached to a
//! workspace whose files appear here by reference. A file source is a
//! connected module whose manifest declares `fileSource` and answers the
//! contract's calls, each a signed post to its path under baseUrl:
//!
//! | call | input | answer |
//! | --- | --- | --- |
//! | `list` | `folder` (null for the root), `cursor` | `{items: [item], nextCursor}` |
//! | `get` | `ref` | an item |
//! | `content` | `ref`, `range?` | the bytes, or `{url}` (https, short-lived) |
//! | `thumbnail` | `ref`, `size` | `{url}` or `{url: null}` |
//! | `changes` | `cursor` | `{changes: [{ref, change}], cursor}` |
//! | `put` (optional) | `folder`, `name`, `contentBase64` | an item |
//!
//! An item is `{ref, name, kind: file|folder, size?, mime?, modifiedAt,
//! webUrl?}`. Answers are read into these shapes and written out again, so
//! nothing else a source sends (a token, say) reaches a caller. The source
//! keeps its own grant to its store; this server only holds the signing
//! secrets, in the secret store. `conformance` is the suite any source can
//! run; it runs once a source is installed, and its result is kept on the
//! install (`file_check`). No store is named here.

use std::time::Duration;

use chrono::{DateTime, Utc};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::manifest::{FileSourceSpec, Problem};
use super::remote::{Posted, endpoint_of, message_of, post_signed};
use super::{APP_COLUMNS, AppRow};
use crate::{
    AppState,
    error::AppError,
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::IdRef,
    },
};

const CALL_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_ANSWER_BYTES: usize = 1024 * 1024;
/// The most bytes `content` takes in an answer; larger files come as a URL.
pub const MAX_CONTENT_BYTES: usize = 25 * 1024 * 1024;
/// The most bytes `put` sends.
pub const MAX_PUT_BYTES: usize = 10 * 1024 * 1024;
const MAX_ITEMS: usize = 1000;
const MAX_REF: usize = 500;
const MAX_CURSOR: usize = 2000;
pub const FILE_CHECK_KIND: &str = "connected_module.file_check";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    File,
    Folder,
}

/// A file or folder in a source, by the source's own reference.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileItem {
    #[serde(rename = "ref")]
    pub reference: String,
    pub name: String,
    pub kind: FileKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    pub modified_at: DateTime<Utc>,
    /// Where it opens in the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileList {
    pub items: Vec<FileItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Created,
    Updated,
    Removed,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    #[serde(rename = "ref")]
    pub reference: String,
    pub change: ChangeKind,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileChanges {
    pub changes: Vec<FileChange>,
    /// Where the next `changes` starts.
    pub cursor: String,
}

/// A file's bytes, or where to fetch them for a short while.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    Bytes {
        mime: Option<String>,
        bytes: Vec<u8>,
    },
    Url(String),
}

fn https(url: &str) -> bool {
    url.len() <= 2000 && url::Url::parse(url).is_ok_and(|url| url.scheme() == "https")
}

fn one_line(text: &str, max: usize) -> bool {
    (1..=max).contains(&text.chars().count()) && !text.chars().any(char::is_control)
}

fn item_problem(item: &FileItem) -> Option<&'static str> {
    if !one_line(&item.reference, MAX_REF) {
        return Some("An item's ref is one line of 500 characters at most.");
    }
    if !one_line(&item.name, 255) {
        return Some("An item's name is one line of 255 characters at most.");
    }
    if item
        .mime
        .as_deref()
        .is_some_and(|mime| !one_line(mime, 200))
    {
        return Some("An item's mime type is one short line.");
    }
    if item.web_url.as_deref().is_some_and(|url| !https(url)) {
        return Some("An item's webUrl is an https address.");
    }
    None
}

/// Reads a call's answer into its shape, or says what's wrong with it.
pub fn read_list(value: Value) -> Result<FileList, String> {
    let list: FileList = serde_json::from_value(value)
        .map_err(|error| format!("The list answer isn't {{items, nextCursor}}: {error}."))?;
    if list.items.len() > MAX_ITEMS {
        return Err("A list answers 1000 items at most; page with nextCursor.".into());
    }
    if let Some(problem) = list.items.iter().find_map(item_problem) {
        return Err(problem.into());
    }
    if list
        .next_cursor
        .as_deref()
        .is_some_and(|cursor| !one_line(cursor, MAX_CURSOR))
    {
        return Err("A cursor is one line of 2000 characters at most.".into());
    }
    Ok(list)
}

pub fn read_item(value: Value) -> Result<FileItem, String> {
    let item: FileItem = serde_json::from_value(value)
        .map_err(|error| format!("The item isn't {{ref, name, kind, modifiedAt}}: {error}."))?;
    match item_problem(&item) {
        Some(problem) => Err(problem.into()),
        None => Ok(item),
    }
}

pub fn read_changes(value: Value) -> Result<FileChanges, String> {
    let changes: FileChanges = serde_json::from_value(value)
        .map_err(|error| format!("The changes answer isn't {{changes, cursor}}: {error}."))?;
    if changes.changes.len() > MAX_ITEMS {
        return Err("Changes come 1000 at most at a time.".into());
    }
    if !one_line(&changes.cursor, MAX_CURSOR)
        || changes
            .changes
            .iter()
            .any(|change| !one_line(&change.reference, MAX_REF))
    {
        return Err("A change's ref and the cursor are short single lines.".into());
    }
    Ok(changes)
}

pub fn read_thumbnail(value: Value) -> Result<Option<String>, String> {
    match value.get("url") {
        Some(Value::Null) | None if value.is_object() || value.is_null() => Ok(None),
        Some(Value::String(url)) if https(url) => Ok(Some(url.clone())),
        _ => Err("A thumbnail is {url} with an https address, or {url: null}.".into()),
    }
}

// ------------------------------------------------------------------ calls

/// A live file source in a workspace, with its outbound secret.
pub(crate) struct Source {
    pub install: AppRow,
    pub spec: FileSourceSpec,
    secret: crate::secrets::SecretValue,
    /// The conformance result, when it found problems.
    pub failed: Option<String>,
}

/// The file source `module_id` in a workspace, if it is one and is live.
pub(crate) async fn source(
    state: &AppState,
    workspace_id: Uuid,
    module_id: Uuid,
) -> Result<Option<Source>, AppError> {
    crate::db::scoped_one(&state.pool, async |connection| {
        let found = sqlx::query_as::<_, AppRow>(&format!(
            "SELECT {APP_COLUMNS} FROM module_apps app \
             JOIN modules module ON module.id = app.module_id \
             WHERE app.workspace_id = $1 AND app.module_id = $2 AND app.state = 'active' \
               AND module.archived_at IS NULL \
               AND EXISTS (SELECT 1 FROM module_installs switch \
                           WHERE switch.workspace_id = app.workspace_id \
                             AND switch.space_id IS NULL \
                             AND switch.module_key = module.module_key AND switch.state = 'on')"
        ))
        .bind(workspace_id)
        .bind(module_id)
        .fetch_optional(&mut *connection)
        .await?;
        let Some(install) = found else {
            return Ok(None);
        };
        let Some(spec) = install.manifest()?.file_source else {
            return Ok(None);
        };
        let Some(outbound) = install.outbound_secret_id else {
            return Ok(None);
        };
        let failed = install
            .file_check
            .as_ref()
            .and_then(|check| check.pointer("/problems/0"))
            .map(|problem| {
                format!(
                    "{}: {}",
                    problem["path"].as_str().unwrap_or("source"),
                    problem["message"].as_str().unwrap_or("it failed its check")
                )
            });
        let keys = crate::secrets::keyring(&state.config)?;
        let secret =
            crate::secrets::reveal(&mut *connection, keys.as_ref(), outbound, "file source")
                .await?;
        Ok(Some(Source {
            install,
            spec,
            secret,
            failed,
        }))
    })
    .await
}

/// Who asks, for the source to hear.
#[derive(Clone, Copy, Debug)]
pub struct Asker {
    pub account: Uuid,
    pub requested_by: Option<Uuid>,
}

/// A call's failure: the source refused, couldn't be reached, or answered
/// outside the contract.
#[derive(Debug)]
pub enum CallError {
    Refused {
        status: u16,
        message: Option<String>,
    },
    Unreachable,
    Malformed(String),
}

impl From<CallError> for OpError {
    fn from(error: CallError) -> Self {
        match error {
            CallError::Refused { status: 404, .. } => OpError::not_found("That file"),
            CallError::Refused {
                status: 400 | 422,
                message,
            } => OpError::Invalid {
                field: "ref".into(),
                message: message
                    .unwrap_or_else(|| "The file source refused that.".into())
                    .into(),
            },
            CallError::Refused { .. } | CallError::Unreachable => OpError::Unavailable,
            CallError::Malformed(message) => OpError::Invalid {
                field: "answer".into(),
                message: format!("The file source answered outside the contract: {message}").into(),
            },
        }
    }
}

impl Source {
    async fn post(
        &self,
        state: &AppState,
        asker: Asker,
        call: &str,
        input: Value,
        max: usize,
    ) -> Result<Posted, CallError> {
        let path = self
            .spec
            .calls
            .path(call)
            .ok_or_else(|| CallError::Malformed(format!("it has no \"{call}\" call")))?;
        let endpoint =
            endpoint_of(&self.install.base_url, path).map_err(|_| CallError::Unreachable)?;
        let body = json!({
            "format": "agent-workspace-files/1",
            "call": call,
            "input": input,
            "caller": { "account": asker.account, "requestedBy": asker.requested_by },
            "workspace": { "id": self.install.workspace_id },
            "module": { "id": self.install.module_id, "key": self.install.module_key },
            "requestId": Uuid::now_v7(),
        })
        .to_string();
        let posted = post_signed(
            state,
            endpoint,
            self.secret.expose(),
            body,
            CALL_TIMEOUT,
            max,
        )
        .await
        .map_err(|error| match error {
            OpError::Invalid { message, .. } => CallError::Malformed(message.into_owned()),
            _ => CallError::Unreachable,
        })?;
        if !(200..300).contains(&posted.status) {
            return Err(CallError::Refused {
                status: posted.status,
                message: message_of(&posted.body),
            });
        }
        Ok(posted)
    }

    async fn json(
        &self,
        state: &AppState,
        asker: Asker,
        call: &str,
        input: Value,
    ) -> Result<Value, CallError> {
        let posted = self
            .post(state, asker, call, input, MAX_ANSWER_BYTES)
            .await?;
        serde_json::from_slice(&posted.body)
            .map_err(|_| CallError::Malformed(format!("its \"{call}\" answer isn't JSON")))
    }

    pub async fn list(
        &self,
        state: &AppState,
        asker: Asker,
        folder: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<FileList, CallError> {
        let value = self
            .json(
                state,
                asker,
                "list",
                json!({ "folder": folder, "cursor": cursor }),
            )
            .await?;
        read_list(value).map_err(CallError::Malformed)
    }

    pub async fn get(
        &self,
        state: &AppState,
        asker: Asker,
        reference: &str,
    ) -> Result<FileItem, CallError> {
        let value = self
            .json(state, asker, "get", json!({ "ref": reference }))
            .await?;
        read_item(value).map_err(CallError::Malformed)
    }

    pub async fn content(
        &self,
        state: &AppState,
        asker: Asker,
        reference: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Content, CallError> {
        let input = json!({
            "ref": reference,
            "range": range.map(|(start, end)| json!({ "start": start, "end": end })),
        });
        let posted = self
            .post(state, asker, "content", input, MAX_CONTENT_BYTES)
            .await?;
        // A short JSON answer naming a URL; anything else is the bytes.
        if posted.body.len() <= 4096
            && let Ok(value) = serde_json::from_slice::<Value>(&posted.body)
            && let Some(url) = value.get("url").and_then(Value::as_str)
        {
            return if https(url) {
                Ok(Content::Url(url.to_owned()))
            } else {
                Err(CallError::Malformed(
                    "a content URL is an https address".into(),
                ))
            };
        }
        Ok(Content::Bytes {
            mime: None,
            bytes: posted.body,
        })
    }

    pub async fn thumbnail(
        &self,
        state: &AppState,
        asker: Asker,
        reference: &str,
        size: u32,
    ) -> Result<Option<String>, CallError> {
        let value = self
            .json(
                state,
                asker,
                "thumbnail",
                json!({ "ref": reference, "size": size }),
            )
            .await?;
        read_thumbnail(value).map_err(CallError::Malformed)
    }

    pub async fn changes(
        &self,
        state: &AppState,
        asker: Asker,
        cursor: Option<&str>,
    ) -> Result<FileChanges, CallError> {
        let value = self
            .json(state, asker, "changes", json!({ "cursor": cursor }))
            .await?;
        read_changes(value).map_err(CallError::Malformed)
    }

    // C1.36 saves new files into a source through this.
    #[allow(dead_code)]
    pub async fn put(
        &self,
        state: &AppState,
        asker: Asker,
        folder: Option<&str>,
        name: &str,
        bytes: &[u8],
    ) -> Result<FileItem, CallError> {
        use base64::Engine as _;
        if !self.spec.put {
            return Err(CallError::Malformed("it doesn't take new files".into()));
        }
        if bytes.len() > MAX_PUT_BYTES {
            return Err(CallError::Malformed("a new file is 10 MB at most".into()));
        }
        let value = self
            .json(
                state,
                asker,
                "put",
                json!({
                    "folder": folder,
                    "name": name,
                    "contentBase64": base64::engine::general_purpose::STANDARD.encode(bytes),
                }),
            )
            .await?;
        read_item(value).map_err(CallError::Malformed)
    }
}

// ------------------------------------------------------------ operations

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<FileSourceList>()
        .add::<FileSourceGet>()
        .add::<ConnectedModuleTest>();
}

/// The source a call names, refused while its check found problems.
async fn usable(call: &OpCall<'_>, module_id: Uuid) -> Result<(Source, Asker), OpError> {
    let found = source(call.state, call.ctx.workspace_id, module_id)
        .await
        .map_err(|error| OpError::logged("file_source", &error))?
        .ok_or(OpError::not_found("That file source"))?;
    if let Some(reason) = &found.failed {
        return Err(OpError::Conflict {
            reason: crate::ops::ConflictReason::State {
                code: "file_source_failed",
            },
            message: format!("This file source failed its check ({reason}).").into(),
            current: None,
        });
    }
    let asker = Asker {
        account: call.ctx.caller.account_id,
        requested_by: call.ctx.caller.requested_by,
    };
    Ok((found, asker))
}

pub struct FileSourceList;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileSourceListInput {
    /// The file source (its module id).
    pub module_id: IdRef,
    /// A folder's ref; left out, the source's root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// The `nextCursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

impl Targeted for FileSourceListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for FileSourceList {
    const NAME: &'static str = "file_source_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List a file source's files";
    const DESCRIPTION: &'static str = "List the files and folders in an outside store connected \
        here as a file source, a folder at a time. Files stay in the source.";

    type Input = FileSourceListInput;
    type Output = FileList;

    async fn run(call: &mut OpCall<'_>, input: FileSourceListInput) -> Result<FileList, OpError> {
        let (source, asker) = usable(call, input.module_id.id()).await?;
        Ok(source
            .list(
                call.state,
                asker,
                input.folder.as_deref(),
                input.cursor.as_deref(),
            )
            .await?)
    }
}

pub struct FileSourceGet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileSourceGetInput {
    /// The file source (its module id).
    pub module_id: IdRef,
    /// The file's or folder's ref in the source.
    #[serde(rename = "ref")]
    pub reference: String,
}

impl Targeted for FileSourceGetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for FileSourceGet {
    const NAME: &'static str = "file_source_get";
    const KIND: OpKind = OpKind::Describe;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "Get a file source's file";
    const DESCRIPTION: &'static str = "Get one file or folder from an outside store connected \
        here as a file source, by its ref.";

    type Input = FileSourceGetInput;
    type Output = FileItem;

    async fn run(call: &mut OpCall<'_>, input: FileSourceGetInput) -> Result<FileItem, OpError> {
        let (source, asker) = usable(call, input.module_id.id()).await?;
        Ok(source.get(call.state, asker, &input.reference).await?)
    }
}

/// Runs a file source's conformance suite again, and keeps what it found.
pub struct ConnectedModuleTest;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileCheckView {
    #[schemars(with = "String")]
    pub module_id: Uuid,
    /// What the suite found; empty when the source passes.
    pub problems: Vec<Problem>,
}

impl Operation for ConnectedModuleTest {
    const NAME: &'static str = "connected_module_test";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "workspace";
    const PERMISSION: Option<Perm> = Some(Perm::ModulesManage);
    // Running it again only checks again.
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const AUTOMATION: bool = false;
    const EMITS: &'static [&'static str] = &["module.connected"];
    const TITLE: &'static str = "Check a file source";
    const DESCRIPTION: &'static str = "Run the file source conformance suite against an outside \
        store connected here, and keep what it finds. A source with problems is refused until \
        it passes.";

    type Input = super::ops::ConnectedModuleRef;
    type Output = FileCheckView;

    async fn run(
        call: &mut OpCall<'_>,
        input: super::ops::ConnectedModuleRef,
    ) -> Result<FileCheckView, OpError> {
        let module_id = input.module_id.id();
        if source(call.state, call.ctx.workspace_id, module_id)
            .await
            .map_err(|error| OpError::logged("connected_module_test", &error))?
            .is_none()
        {
            return Err(OpError::not_found("That file source"));
        }
        call.key_honoured();
        let problems = check_and_keep(call.state, call.ctx.workspace_id, module_id)
            .await
            .map_err(|error| OpError::logged("connected_module_test", &error))?
            .ok_or(OpError::Unavailable)?;
        call.audit_target("module", module_id, json!({ "fileCheck": problems.len() }));
        Ok(FileCheckView {
            module_id,
            problems,
        })
    }
}

// ------------------------------------------------------------ conformance

fn problem(path: &str, message: impl Into<String>) -> Problem {
    Problem {
        path: path.to_owned(),
        message: message.into(),
    }
}

/// What the suite found, or that the source couldn't be reached to check.
pub(crate) enum Checked {
    Problems(Vec<Problem>),
    Unreachable,
}

/// The conformance suite: lists the root, gets an item it listed, reads a
/// little of a file and its thumbnail, and asks for changes from the start.
/// Each call's answer is held to the contract.
pub(crate) async fn conformance(state: &AppState, source: &Source, asker: Asker) -> Checked {
    let mut problems = Vec::new();
    let failed = |call: &str, error: CallError, problems: &mut Vec<Problem>| match error {
        CallError::Unreachable => true,
        CallError::Refused { status, message } => {
            problems.push(problem(
                call,
                message.unwrap_or_else(|| format!("It answered {status}.")),
            ));
            false
        }
        CallError::Malformed(message) => {
            problems.push(problem(call, message));
            false
        }
    };
    let list = match source.list(state, asker, None, None).await {
        Ok(list) => Some(list),
        Err(error) => {
            if failed("list", error, &mut problems) {
                return Checked::Unreachable;
            }
            None
        }
    };
    let first = list.as_ref().and_then(|list| list.items.first());
    if let Some(first) = first {
        match source.get(state, asker, &first.reference).await {
            Ok(item) if item.reference == first.reference => {}
            Ok(_) => problems.push(problem(
                "get",
                "It answered another item than the one asked for.",
            )),
            Err(error) => {
                if failed("get", error, &mut problems) {
                    return Checked::Unreachable;
                }
            }
        }
    }
    let file = list
        .as_ref()
        .and_then(|list| list.items.iter().find(|item| item.kind == FileKind::File));
    if let Some(file) = file {
        if let Err(error) = source
            .content(state, asker, &file.reference, Some((0, 1023)))
            .await
            && failed("content", error, &mut problems)
        {
            return Checked::Unreachable;
        }
        if let Err(error) = source.thumbnail(state, asker, &file.reference, 128).await
            && failed("thumbnail", error, &mut problems)
        {
            return Checked::Unreachable;
        }
    }
    if let Err(error) = source.changes(state, asker, None).await
        && failed("changes", error, &mut problems)
    {
        return Checked::Unreachable;
    }
    Checked::Problems(problems)
}

/// Runs the suite for an installed source and keeps what it found on the
/// install. `false` when the source couldn't be reached.
pub async fn check_and_keep(
    state: &AppState,
    workspace_id: Uuid,
    module_id: Uuid,
) -> Result<Option<Vec<Problem>>, AppError> {
    let Some(source) = source(state, workspace_id, module_id).await? else {
        return Ok(Some(Vec::new()));
    };
    let asker = Asker {
        account: source.install.account_id,
        requested_by: None,
    };
    let Checked::Problems(problems) = conformance(state, &source, asker).await else {
        return Ok(None);
    };
    crate::db::scoped_one(&state.pool, async |connection| {
        sqlx::query(
            "UPDATE module_apps SET file_check = $2, updated_at = now() WHERE module_id = $1",
        )
        .bind(module_id)
        .bind(json!({ "checkedAt": Utc::now(), "problems": problems }))
        .execute(&mut *connection)
        .await?;
        Ok::<_, AppError>(())
    })
    .await?;
    Ok(Some(problems))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> Value {
        json!({"ref": "f1", "name": "plan.md", "kind": "file", "size": 12,
               "mime": "text/markdown", "modifiedAt": "2026-11-19T08:00:00Z",
               "webUrl": "https://files.example.com/f1"})
    }

    #[test]
    fn answers_are_read_into_the_contract_and_nothing_else_passes() {
        let mut leaky = item();
        leaky["accessToken"] = json!("secret-from-the-store");
        let list = read_list(json!({"items": [leaky], "nextCursor": "2"})).expect("list");
        let written = serde_json::to_string(&list).expect("json");
        assert!(!written.contains("secret-from-the-store"), "{written}");
        assert_eq!(list.next_cursor.as_deref(), Some("2"));
        assert!(read_list(json!({"files": []})).is_err());
        let mut plain = item();
        plain["webUrl"] = json!("javascript:alert(1)");
        assert!(read_item(plain).is_err());
        let mut folder = item();
        folder["kind"] = json!("drive");
        assert!(read_item(folder).is_err());
        assert!(
            read_changes(json!({"changes": [{"ref": "f1", "change": "updated"}], "cursor": "c2"}))
                .is_ok()
        );
        assert!(read_changes(json!({"changes": []})).is_err());
        assert_eq!(read_thumbnail(json!({"url": null})), Ok(None));
        assert!(read_thumbnail(json!({"url": "http://x.test/t.png"})).is_err());
    }

    #[test]
    fn no_store_is_named_in_the_app() {
        // R23: a file source is any store; core code names none.
        let names = [
            "google drive",
            "drive.google",
            "googleapis.com/drive",
            "dropbox",
            "onedrive",
            "sharepoint",
            "box.com",
            "icloud",
        ];
        let roots = [
            concat!(env!("CARGO_MANIFEST_DIR"), "/src"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../web/src"),
        ];
        let mut found = Vec::new();
        let mut stack: Vec<std::path::PathBuf> = roots.iter().map(Into::into).collect();
        while let Some(path) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if !path.ends_with("node_modules") && !path.ends_with("provider") {
                        stack.push(path);
                    }
                    continue;
                }
                let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
                    continue;
                };
                if !matches!(extension, "rs" | "ts" | "tsx" | "json" | "css") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let lower = text.to_lowercase();
                for name in names {
                    if lower.contains(name) && !path.ends_with("files.rs") {
                        found.push(format!("{}: {name}", path.display()));
                    }
                }
            }
        }
        assert!(found.is_empty(), "{found:#?}");
    }
}
