//! Reply requests as operations (K2.9): an agent claims a request, reports
//! progress, replies, passes or releases it; a person stops one or asks the
//! agent again. Each runs the transition the hand-written tool ran
//! (`mcp::replies`, over `replies.rs`), with its generation fence, lease and
//! own audit row, as the caller the dispatcher checked. The web gets the
//! same operations (claim and release included), so people can see and use
//! them. The inbox is a read and stays a tool until K2.16.

use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    mcp::{
        replies::{
            self, ClaimProjection, ProgressProjection, ReplyClaimArguments, ReplyPassArguments,
            ReplyProgressArguments, ReplyReleaseArguments, ReplyRetryArguments, ReplySendArguments,
            ReplyStopArguments, ResolutionProjection, RetryProjection, StopProjection,
        },
        task_commands::{principal_for, tool_answer},
    },
    ops::{
        Idempotency, OpCall, OpError, OpKind, Operation, Perm, Registry, Target, Targeted,
        refs::{IdRef, parse_id},
    },
};

pub fn register_ops(registry: &mut Registry) {
    registry
        .add::<ReplyClaim>()
        .add::<ReplySend>()
        .add::<ReplyProgressReport>()
        .add::<ReplyPass>()
        .add::<ReplyRelease>()
        .add::<ReplyStop>()
        .add::<ReplyStopAll>()
        .add::<ReplyRetry>();
}

/// The caller's own reply request (the agent it asks).
fn request(id: &str) -> Target {
    Target::thing("reply_request", parse_id(id).unwrap_or_else(Uuid::nil))
}

/// A reply request in a conversation the caller sees.
fn request_seen(id: &str) -> Target {
    Target::thing("reply_request_seen", parse_id(id).unwrap_or_else(Uuid::nil))
}

/// One operation over a reply transition that audits itself and is
/// retried through the dispatcher's request ids.
macro_rules! reply_op {
    (
        $op:ident {
            name: $name:literal,
            kind: $kind:ident,
            found: $found:ident,
            title: $title:literal,
            description: $description:expr,
            emits: $emits:literal,
            input: $input:ty,
            output: $output:ty,
            run: $command:path
        }
    ) => {
        pub(crate) struct $op;

        impl Targeted for $input {
            fn target(&self) -> Target {
                $found(&self.request_id)
            }
        }

        impl Operation for $op {
            const NAME: &'static str = $name;
            const KIND: OpKind = OpKind::$kind;
            const MODULE: &'static str = "messages";
            const PERMISSION: Option<Perm> = Some(Perm::ChannelsPost);
            const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
            const TITLE: &'static str = $title;
            const DESCRIPTION: &'static str = $description;
            const EMITS: &'static [&'static str] = &[$emits];

            type Input = $input;
            type Output = $output;

            async fn run(call: &mut OpCall<'_>, input: $input) -> Result<$output, OpError> {
                let principal = principal_for(call.ctx, None);
                // A2.15: answering a request is made for its asker.
                let acting = crate::replies::asker::acting_for_request(
                    call.state,
                    call.ctx.caller.account_id,
                    &input.request_id,
                )
                .await;
                let answer = tool_answer(
                    crate::access::acting::within(acting, $command(call.state, &principal, input))
                        .await
                        .into_tool(),
                )?;
                call.audited_by_command();
                Ok(answer)
            }
        }
    };
}

reply_op!(ReplyClaim {
    name: "reply_claim",
    kind: Change,
    found: request,
    title: "Claim reply request",
    description: "Claim a reply request before answering it. Returns the conversation and \
        recent messages. Claiming again renews the 15-minute lease.",
    emits: "reply.claimed",
    input: ReplyClaimArguments,
    output: ClaimProjection,
    run: replies::reply_claim
});

reply_op!(ReplyProgressReport {
    name: "reply_progress_report",
    kind: Create,
    found: request,
    title: "Report progress",
    description: "Say what you're doing on a claimed reply request, in one short line \
        (\"Running the server tests\"). People see it next to your name until you reply. Also \
        renews the claim, so report progress at least every 15 minutes during long work.",
    emits: "reply.progress",
    input: ReplyProgressArguments,
    output: ProgressProjection,
    run: replies::reply_progress
});

reply_op!(ReplyPass {
    name: "reply_pass",
    kind: Change,
    found: request,
    title: "Pass",
    description: "Pass on an optional group reply request when you have nothing useful to \
        add. Posts nothing.",
    emits: "reply.passed",
    input: ReplyPassArguments,
    output: ResolutionProjection,
    run: replies::reply_pass
});

reply_op!(ReplyRelease {
    name: "reply_release",
    kind: Change,
    found: request,
    title: "Release reply request",
    description: "Give back a reply request you can't answer, with a reason the people in \
        the conversation see.",
    emits: "reply.released",
    input: ReplyReleaseArguments,
    output: ResolutionProjection,
    run: replies::reply_release
});

reply_op!(ReplyStop {
    name: "reply_stop",
    kind: Change,
    found: request_seen,
    title: "Stop reply request",
    description: "Stop an agent's reply request, and the follow-ups queued behind it, like \
        the Stop button on its status line.",
    emits: "reply.stopped",
    input: ReplyStopArguments,
    output: StopProjection,
    run: replies::reply_stop
});

/// C2.35: Stop all in a conversation.
pub(crate) struct ReplyStopAll;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReplyStopAllInput {
    /// The conversation or thread whose agents to stop.
    pub conversation_id: IdRef,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReplyStopAllOutput {
    /// Every reply request stopped; empty when no agent was working there.
    pub stopped_request_ids: Vec<String>,
}

impl Targeted for ReplyStopAllInput {
    fn target(&self) -> Target {
        Target::thing("conversation", self.conversation_id.id())
    }
}

impl Operation for ReplyStopAll {
    const NAME: &'static str = "reply_stop_all";
    const KIND: OpKind = OpKind::Change;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = Some(Perm::ChannelsPost);
    const IDEMPOTENCY: Idempotency = Idempotency::Dispatcher;
    const TITLE: &'static str = "Stop all agents";
    const DESCRIPTION: &'static str = "Stop every agent's waiting or working reply request in \
        a conversation and its threads, like Stop all on its status row.";
    const EMITS: &'static [&'static str] = &["reply.stopped"];

    type Input = ReplyStopAllInput;
    type Output = ReplyStopAllOutput;

    async fn run(
        call: &mut OpCall<'_>,
        input: ReplyStopAllInput,
    ) -> Result<ReplyStopAllOutput, OpError> {
        let conversation_id = input.conversation_id.id();
        let actor_id = call.account_id();
        let stopped =
            crate::replies::stop_all_in(call.tx().await?, actor_id, conversation_id).await?;
        call.audit_target(
            "conversation",
            conversation_id,
            serde_json::json!({ "stopped": stopped.len() }),
        );
        Ok(ReplyStopAllOutput {
            stopped_request_ids: stopped.iter().map(Uuid::to_string).collect(),
        })
    }
}

reply_op!(ReplyRetry {
    name: "reply_retry",
    kind: Change,
    found: request_seen,
    title: "Ask again",
    description: "Ask an agent again about a reply request that expired, was released or \
        was stopped, like the Retry button on its status line.",
    emits: "reply.retried",
    input: ReplyRetryArguments,
    output: RetryProjection,
    run: replies::reply_retry
});

// ---------------------------------------------------------------------------
// reply_send keeps its own retry key (derived from the request and its
// generation when the caller sends none), so the dispatcher passes the
// caller's through.

pub(crate) struct ReplySend;

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReplySendInput {
    pub request_id: String,
    /// The generation returned by reply_claim.
    pub generation: i32,
    /// Your reply, as plain text.
    pub text: String,
    /// Defaults to true: the reply answers the request and closes it. Pass
    /// false for an interim message (a plan, a question, a link to a pull
    /// request) and keep working; the claim stays yours and its lease renews.
    #[serde(rename = "final", default, skip_serializing_if = "Option::is_none")]
    pub final_reply: Option<bool>,
    /// Files to attach (up to 10), uploaded first with file_upload to the
    /// request's conversation (or its thread), such as a run's output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_ids: Option<Vec<String>>,
}

impl Targeted for ReplySendInput {
    fn target(&self) -> Target {
        request(&self.request_id)
    }
}

impl Operation for ReplySend {
    const NAME: &'static str = "reply_send";
    const KIND: OpKind = OpKind::Create;
    const MODULE: &'static str = "messages";
    const PERMISSION: Option<Perm> = Some(Perm::ChannelsPost);
    const IDEMPOTENCY: Idempotency = Idempotency::Command;
    const TITLE: &'static str = "Send reply";
    const DESCRIPTION: &'static str = "Answer a claimed reply request. Posts your text in the \
        conversation as you; @handles become mentions. Closes the request unless you pass \
        final: false, which posts an interim message and keeps your claim. Resending the same \
        reply is safe without a clientRequestId.";
    const EMITS: &'static [&'static str] = &["message.created"];

    type Input = ReplySendInput;
    type Output = ResolutionProjection;

    async fn run(
        call: &mut OpCall<'_>,
        input: ReplySendInput,
    ) -> Result<ResolutionProjection, OpError> {
        let principal = principal_for(call.ctx, None);
        // A2.15: a reply is made for the request's asker.
        let acting = crate::replies::asker::acting_for_request(
            call.state,
            call.ctx.caller.account_id,
            &input.request_id,
        )
        .await;
        let answer = tool_answer(
            crate::access::acting::within(
                acting,
                replies::reply_send(
                    call.state,
                    &principal,
                    ReplySendArguments {
                        request_id: input.request_id,
                        generation: input.generation,
                        text: input.text,
                        final_reply: input.final_reply,
                        attachment_ids: input.attachment_ids,
                        client_request_id: call.ctx.request_id.clone(),
                    },
                ),
            )
            .await
            .into_tool(),
        )?;
        // Keyed by the caller's id or by the request and its generation.
        call.key_honoured();
        call.audited_by_command();
        Ok(answer)
    }
}
