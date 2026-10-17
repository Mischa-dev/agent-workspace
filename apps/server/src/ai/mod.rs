//! W10: AI planning (docs/plans/roadmap.md, AI1 to AI6 and V7).
//!
//! - [`gateway`]: short AI calls with any provider, on keys from settings,
//!   with structured output, a monthly budget and usage per call (AI1).
//! - [`suggestions`]: the review queue. Every suggestion keeps its sources
//!   and is shown or accepted only for someone who can still see them all
//!   (`ai_suggestion_visible_to`). Accepting goes through the same task
//!   commands as a person's own change, marked "via AI" (AI2).
//! - [`detect`]: finds dependencies, blockers and duplicates. Cheap signals
//!   pick candidate pairs, the model judges them in batches. Runs with no
//!   person behind them read only what every project member can see (AI2),
//!   and triage new tasks (AI4).
//! - [`ready`]: readiness and the ranking of what to do next (AI5), and the
//!   dependency graph (V7).
//! - [`plans`]: plan to tasks, resumable in batches (AI3).
//! - [`summaries`]: catch me up on a thread or a task (AI6).
//!
//! The server never puts prompt or reply text in logs or usage rows, and
//! suggestions store a one-line reason, not the model's reply.

pub mod detect;
pub mod gateway;
pub mod http;
pub mod plans;
pub mod prompts;
pub mod providers;
pub mod ready;
pub mod schema;
pub mod suggestions;
pub mod summaries;

/// The AI jobs, for `jobs::default_registry`.
pub fn register_jobs(registry: &mut crate::jobs::JobRegistry) {
    detect::register_jobs(registry);
    plans::register_jobs(registry);
}

/// Tells one person in their inbox that AI work they asked for finished
/// (roadmap N2's notifications). The row holds the project, a title and a
/// link; reads recheck that they can still see the project.
pub(crate) async fn notify(
    pool: &sqlx::PgPool,
    project_id: uuid::Uuid,
    recipient: uuid::Uuid,
    kind: &'static str,
    subject_id: uuid::Uuid,
    title: String,
    href: String,
) -> Result<(), crate::error::AppError> {
    let mut transaction = pool.begin().await?;
    let notification = crate::notifications::NewNotification {
        project_id,
        recipient_actor_id: recipient,
        kind,
        subject_kind: "project",
        subject_id,
        actor_id: None,
        conversation_id: None,
        message_id: None,
        work_item_id: None,
        comment_id: None,
        agent_run_id: None,
        approval_id: None,
        pull_request_id: None,
        detail: serde_json::json!({"title": title, "href": href}),
        group_key: format!("{kind}:{project_id}"),
        dedupe_key: format!("{kind}:{subject_id}"),
    };
    if crate::notifications::deliver(&mut transaction, &notification).await? {
        crate::outbox::enqueue_typed(
            &mut transaction,
            project_id,
            crate::outbox::OutboxEventPayload::NotificationsUpdated(
                crate::outbox::NotificationsUpdatedPayload {
                    actor_id: recipient,
                },
            ),
        )
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}
