//! Keyword alerts as operations (C2.13): `notification_keyword_list` and
//! `notification_keyword_set`. Each works on the caller's own list in this
//! workspace; the web's notification settings, agents and apps run these.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{MAX_KEYWORDS, normalize};
use crate::ops::{Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<NotificationKeywordList>()
        .add::<NotificationKeywordSet>();
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeywordsView {
    /// Your keywords here, in the order you added them.
    pub keywords: Vec<String>,
    /// How many you may keep.
    pub limit: usize,
}

/// List your keywords.
pub struct NotificationKeywordList;

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationKeywordListInput {}

impl Targeted for NotificationKeywordListInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for NotificationKeywordList {
    const NAME: &'static str = "notification_keyword_list";
    const KIND: OpKind = OpKind::Query;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::None;
    const TITLE: &'static str = "List keyword alerts";
    const DESCRIPTION: &'static str = "Show the words you hear about in this workspace: a message \
        with one of them, in a conversation you can read and haven't muted, notifies you. \
        Change them with notification_keyword_set.";

    type Input = NotificationKeywordListInput;
    type Output = KeywordsView;

    async fn run(
        call: &mut OpCall<'_>,
        _input: NotificationKeywordListInput,
    ) -> Result<KeywordsView, OpError> {
        let (workspace, account) = (call.ctx.workspace_id, call.account_id());
        let connection = &mut **call.tx().await?;
        Ok(KeywordsView {
            keywords: super::list(connection, workspace, account).await?,
            limit: MAX_KEYWORDS,
        })
    }
}

/// Set your keywords.
pub struct NotificationKeywordSet;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationKeywordSetInput {
    /// The whole list, replacing the one there: words or short phrases of
    /// 2 to 60 characters, at most 50. Case doesn't matter; an empty list
    /// turns keyword alerts off.
    pub keywords: Vec<String>,
}

impl Targeted for NotificationKeywordSetInput {
    fn target(&self) -> Target {
        Target::Workspace(None)
    }
}

impl Operation for NotificationKeywordSet {
    const NAME: &'static str = "notification_keyword_set";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = None;
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Set keyword alerts";
    const DESCRIPTION: &'static str = "Replace the words you hear about in this workspace (at \
        most 50, each 2 to 60 characters). A message naming one as a whole word, in a \
        conversation you can read and haven't muted, notifies you; your own messages never do.";
    const EMITS: &'static [&'static str] = &["notification_keywords.changed"];

    type Input = NotificationKeywordSetInput;
    type Output = KeywordsView;

    async fn run(
        call: &mut OpCall<'_>,
        input: NotificationKeywordSetInput,
    ) -> Result<KeywordsView, OpError> {
        let mut keywords: Vec<String> = Vec::new();
        for given in &input.keywords {
            let keyword = normalize(given).ok_or_else(|| {
                OpError::invalid(
                    "keywords",
                    "Each keyword is 2 to 60 characters, such as launch or design review.",
                )
            })?;
            if !keywords
                .iter()
                .any(|kept| kept.to_lowercase() == keyword.to_lowercase())
            {
                keywords.push(keyword);
            }
        }
        if keywords.len() > MAX_KEYWORDS {
            return Err(OpError::invalid("keywords", "Keep up to 50 keywords."));
        }
        let (workspace, account) = (call.ctx.workspace_id, call.account_id());
        let connection = &mut **call.tx().await?;
        super::replace(connection, workspace, account, &keywords).await?;
        let keywords = super::list(connection, workspace, account).await?;
        call.audit_target("account", account, json!({ "keywords": keywords.len() }));
        Ok(KeywordsView {
            keywords,
            limit: MAX_KEYWORDS,
        })
    }
}
