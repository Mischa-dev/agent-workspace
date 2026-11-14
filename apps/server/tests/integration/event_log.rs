//! K3.2 and K3.3: the event log. A change writes its event in its own
//! transaction; the sequencer gives committed events dense positions in
//! commit order, and a reader that follows positions never skips one.

use serde_json::json;
use uuid::Uuid;

use super::{
    message_edits::create_channel,
    support::{
        app::tool,
        world::{Setup, WorldBuilder},
    },
};

/// Sending a message stages one `message.created` with who sent it and the
/// app they sent it through; a rolled-back transaction stages nothing.
#[tokio::test]
async fn a_message_stages_one_event_with_its_actor_and_app() {
    let Some(world) = WorldBuilder::default()
        .project("LOG")
        .connected_app("helper")
        .build("event_log_message")
        .await
    else {
        return;
    };
    let setup = Setup {
        app: world.app.clone(),
        browser: super::support::app::login(&world.app).await,
        organization_id: world.organization,
        pool: world.pool().clone(),
        cleanup: None,
    };
    let channel = create_channel(&setup, world.project("LOG"), "general").await;
    let helper = &world.apps["helper"];
    let sent = tool(
        &world.app,
        &helper.token,
        &helper.session,
        "message_send",
        json!({"conversationId": channel, "text": "hello log", "clientRequestId": "log-1"}),
    )
    .await;
    assert_eq!(sent["outcome"], "ok", "{sent}");
    let message = sent["value"]["messageId"]
        .as_str()
        .and_then(|id| id.parse::<Uuid>().ok())
        .expect("message id");

    let staged = sqlx::query_as::<
        _,
        (
            String,
            Option<Uuid>,
            Option<Uuid>,
            String,
            Option<Uuid>,
            Option<bool>,
            Uuid,
            Option<String>,
            serde_json::Value,
        ),
    >(
        r#"
        SELECT type, actor_account_id, via_credential_id, audience, conversation_id,
               conversation_restricted, workspace_id, legacy_type, payload
        FROM event_staging WHERE thing_id = $1
        "#,
    )
    .bind(message)
    .fetch_all(world.pool())
    .await
    .expect("staged events");
    assert_eq!(staged.len(), 1, "{staged:?}");
    let (ty, actor, via, audience, conversation, restricted, workspace, legacy, payload) =
        staged.into_iter().next().expect("one");
    assert_eq!(ty, "message.created");
    assert_eq!(actor, Some(helper.person));
    let grant = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT via_oauth_grant_id FROM messages WHERE id = $1",
    )
    .bind(message)
    .fetch_one(world.pool())
    .await
    .expect("message");
    assert!(grant.is_some(), "the message records its app");
    assert_eq!(via, grant, "the event names the app it came through");
    assert_eq!(audience, "conversation");
    assert_eq!(conversation, Some(channel.parse().expect("channel")));
    assert_eq!(restricted, Some(false));
    assert_eq!(workspace, world.organization);
    assert_eq!(legacy.as_deref(), Some("conversation.message.created"));
    assert!(
        !payload.to_string().contains("hello log"),
        "an event never carries message text: {payload}"
    );

    // A transaction that rolls back leaves no event behind.
    let before = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM event_staging")
        .fetch_one(world.pool())
        .await
        .expect("count");
    let mut transaction = world.pool().begin().await.expect("begin");
    agent_workspace_server::events::emit(
        &mut transaction,
        agent_workspace_server::events::NewEvent::thing(
            &agent_workspace_server::events::catalog::TASK_UPDATED,
            world.project("LOG").parse().expect("project"),
            "task",
            Uuid::now_v7(),
        ),
    )
    .await
    .expect("emit");
    transaction.rollback().await.expect("rollback");
    let after = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM event_staging")
        .fetch_one(world.pool())
        .await
        .expect("count");
    assert_eq!(before, after);
    drop(setup);
    world.finish().await;
}
