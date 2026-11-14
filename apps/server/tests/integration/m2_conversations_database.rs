use std::{sync::Arc, time::Duration};

use agent_workspace_server::domain::allocate_conversation_sequence;
use sqlx::{PgPool, postgres::PgPoolOptions};
use tokio::sync::Barrier;
use uuid::Uuid;

#[derive(Clone, Copy)]
struct Fixture {
    owner_actor_id: Uuid,
    collaborator_actor_id: Uuid,
    worker_actor_id: Uuid,
    disabled_actor_id: Uuid,
    project_id: Uuid,
    foreign_project_id: Uuid,
    repository_id: Uuid,
    replacement_repository_id: Uuid,
    channel_id: Uuid,
    thread_id: Uuid,
    execution_worker_id: Uuid,
    execution_profile_id: Uuid,
    terminal_run_id: Uuid,
    message_id: Uuid,
    event_id: Uuid,
}

#[tokio::test]
async fn m0_to_m2_upgrade_preserves_history_and_enforces_conversation_invariants() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m2_conversations_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&administrative_pool)
        .await
        .expect("create isolated M2 schema");
    let pool = schema_pool(&database_url, &schema).await;

    apply_through_m1(&pool).await;
    let fixture = insert_m1_history(&pool).await;
    sqlx::raw_sql(include_str!("../../migrations/0005_m2_conversations.sql"))
        .execute(&pool)
        .await
        .expect("upgrade M1 data to M2");

    let repeated_mention_message_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO messages (
            id, conversation_id, author_actor_id, content, plain_text
        )
        VALUES (
            $1, $2, $3,
            jsonb_build_object(
                'version', 1,
                'nodes', jsonb_build_array(
                    jsonb_build_object('type', 'mention', 'actorId', $4::text),
                    jsonb_build_object('type', 'mention', 'actorId', $4::text)
                )
            ),
            '@agent @agent'
        )
        "#,
    )
    .bind(repeated_mention_message_id)
    .bind(fixture.channel_id)
    .bind(fixture.owner_actor_id)
    .bind(fixture.worker_actor_id)
    .execute(&pool)
    .await
    .expect("insert valid 0005 Message with a repeated Actor mention");
    sqlx::query(
        r#"
        INSERT INTO message_mentions (
            project_id, message_id, actor_id, node_index
        )
        VALUES ($1, $2, $3, 0)
        "#,
    )
    .bind(fixture.project_id)
    .bind(repeated_mention_message_id)
    .bind(fixture.worker_actor_id)
    .execute(&pool)
    .await
    .expect("insert distinct-Actor 0005 mention projection row");

    sqlx::raw_sql(include_str!(
        "../../migrations/0006_m2_message_mentions_projection.sql"
    ))
    .execute(&pool)
    .await
    .expect("reconcile M2 mention projection");
    sqlx::raw_sql(include_str!(
        "../../migrations/0007_m2_archived_parent_navigation.sql"
    ))
    .execute(&pool)
    .await
    .expect("apply archived-parent navigation boundary");
    sqlx::raw_sql(include_str!(
        "../../migrations/0008_m2_conversation_lock_boundaries.sql"
    ))
    .execute(&pool)
    .await
    .expect("apply archive serialization boundary");

    let preserved = sqlx::query_as::<_, (Uuid, Uuid, Uuid, i64, String, Uuid, Uuid)>(
        r#"
        SELECT
            message.id,
            message.project_id,
            entry.id,
            entry.sequence,
            message.plain_text,
            event.id,
            event.conversation_id
        FROM messages message
        JOIN conversation_entries entry ON entry.message_id = message.id
        JOIN agent_run_events event ON event.id = $2
        WHERE message.id = $1
        "#,
    )
    .bind(fixture.message_id)
    .bind(fixture.event_id)
    .fetch_one(&pool)
    .await
    .expect("read preserved M0 timeline history");
    assert_eq!(preserved.0, fixture.message_id);
    assert_eq!(preserved.1, fixture.project_id);
    assert_eq!(preserved.3, 1);
    assert_eq!(preserved.4, "Preserved M0 worker response");
    assert_eq!(preserved.5, fixture.event_id);
    assert_eq!(preserved.6, fixture.thread_id);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT version FROM conversations WHERE id = $1",)
            .bind(fixture.thread_id)
            .fetch_one(&pool)
            .await
            .expect("read upgraded Conversation version"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM message_mentions WHERE message_id = $1",
        )
        .bind(repeated_mention_message_id)
        .fetch_one(&pool)
        .await
        .expect("count reconciled distinct-Actor mention projection"),
        1,
        "repeated AST nodes retain one distinct Actor projection through 0006"
    );
    assert!(
        sqlx::query("DELETE FROM message_mentions WHERE message_id = $1")
            .bind(repeated_mention_message_id)
            .execute(&pool)
            .await
            .is_err(),
        "0006 must preserve mention immutability after reconciliation"
    );

    let invalid_channel_repository = sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, repository_id, name)
        VALUES ($1, $2, 'channel', $3, 'invalid repository Channel')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(fixture.project_id)
    .bind(fixture.repository_id)
    .execute(&pool)
    .await;
    assert!(
        invalid_channel_repository.is_err(),
        "a Channel cannot bind a repository"
    );

    let nested_thread = sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, parent_channel_id, name)
        VALUES ($1, $2, 'thread', $3, 'nested Thread')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(fixture.project_id)
    .bind(fixture.thread_id)
    .execute(&pool)
    .await;
    assert!(
        nested_thread.is_err(),
        "a Thread cannot parent another Thread"
    );

    let cross_project_thread = sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, parent_channel_id, name)
        VALUES ($1, $2, 'thread', $3, 'cross Project Thread')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(fixture.foreign_project_id)
    .bind(fixture.channel_id)
    .execute(&pool)
    .await;
    assert!(
        cross_project_thread.is_err(),
        "a Thread parent must belong to the same Project"
    );

    let channel_message_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO messages (
            id, conversation_id, author_actor_id, content, plain_text
        )
        VALUES (
            $1, $2, $3,
            '{"version":1,"nodes":[{"type":"text","text":"origin"}]}'::jsonb,
            'origin'
        )
        "#,
    )
    .bind(channel_message_id)
    .bind(fixture.channel_id)
    .bind(fixture.owner_actor_id)
    .execute(&pool)
    .await
    .expect("insert M2 Channel origin Message without derived Project column");
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM messages WHERE id = $1")
            .bind(channel_message_id)
            .fetch_one(&pool)
            .await
            .expect("read derived Message Project"),
        fixture.project_id
    );

    let linked_thread_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (
            id, project_id, kind, parent_channel_id, origin_message_id, name
        )
        VALUES ($1, $2, 'thread', $3, $4, 'linked Thread')
        "#,
    )
    .bind(linked_thread_id)
    .bind(fixture.project_id)
    .bind(fixture.channel_id)
    .bind(channel_message_id)
    .execute(&pool)
    .await
    .expect("create a Thread from a same-Channel Message");

    let wrong_origin = sqlx::query(
        r#"
        INSERT INTO conversations (
            id, project_id, kind, parent_channel_id, origin_message_id, name
        )
        VALUES ($1, $2, 'thread', $3, $4, 'wrong origin Thread')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(fixture.project_id)
    .bind(fixture.channel_id)
    .bind(fixture.message_id)
    .execute(&pool)
    .await;
    assert!(
        wrong_origin.is_err(),
        "a Thread origin Message must belong to its parent Channel"
    );

    let cross_conversation_entry = sqlx::query(
        r#"
        INSERT INTO conversation_entries (
            id, conversation_id, sequence, kind, message_id
        )
        VALUES ($1, $2, 1, 'message', $3)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(linked_thread_id)
    .bind(channel_message_id)
    .execute(&pool)
    .await;
    assert!(
        cross_conversation_entry.is_err(),
        "a timeline entry cannot link a Message from another Conversation"
    );

    let message_update = sqlx::query("UPDATE messages SET plain_text = 'edited' WHERE id = $1")
        .bind(channel_message_id)
        .execute(&pool)
        .await;
    assert!(
        message_update.is_err(),
        "sent Messages must remain immutable"
    );

    let mention_message_id = Uuid::now_v7();
    let mention_content = serde_json::json!({
        "version": 1,
        "nodes": [
            {"type": "text", "text": "Please ask "},
            {"type": "mention", "actorId": fixture.worker_actor_id},
        ],
    });
    sqlx::query(
        r#"
        INSERT INTO messages (
            id, conversation_id, author_actor_id, content, plain_text
        )
        VALUES ($1, $2, $3, $4, 'Please ask @agent')
        "#,
    )
    .bind(mention_message_id)
    .bind(fixture.channel_id)
    .bind(fixture.owner_actor_id)
    .bind(mention_content)
    .execute(&pool)
    .await
    .expect("insert structured mention Message");
    sqlx::query(
        r#"
        INSERT INTO message_mentions (project_id, message_id, actor_id)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(fixture.project_id)
    .bind(mention_message_id)
    .bind(fixture.worker_actor_id)
    .execute(&pool)
    .await
    .expect("insert enabled same-Project Worker mention");

    let disabled_mention_message_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO messages (
            id, conversation_id, author_actor_id, content, plain_text
        )
        VALUES ($1, $2, $3, $4, 'disabled mention')
        "#,
    )
    .bind(disabled_mention_message_id)
    .bind(fixture.channel_id)
    .bind(fixture.owner_actor_id)
    .bind(serde_json::json!({
        "version": 1,
        "nodes": [{"type": "mention", "actorId": fixture.disabled_actor_id}],
    }))
    .execute(&pool)
    .await
    .expect("insert Message whose mention will be rejected");
    let disabled_mention = sqlx::query(
        r#"
        INSERT INTO message_mentions (project_id, message_id, actor_id)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(fixture.project_id)
    .bind(disabled_mention_message_id)
    .bind(fixture.disabled_actor_id)
    .execute(&pool)
    .await;
    assert!(
        disabled_mention.is_err(),
        "a disabled Project membership cannot be mentioned at send time"
    );

    let derived_event_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_run_events (
            id, agent_run_id, event_key, kind, summary, payload
        )
        VALUES ($1, $2, $3, 'test_result', 'derived Conversation', '{}'::jsonb)
        "#,
    )
    .bind(derived_event_id)
    .bind(fixture.terminal_run_id)
    .bind(format!("m2-derived-{derived_event_id}"))
    .execute(&pool)
    .await
    .expect("insert M0-shaped Agent Run event");
    assert_eq!(
        sqlx::query_scalar::<_, Uuid>(
            "SELECT conversation_id FROM agent_run_events WHERE id = $1",
        )
        .bind(derived_event_id)
        .fetch_one(&pool)
        .await
        .expect("read derived event Conversation"),
        fixture.thread_id
    );

    let active_run_id = Uuid::now_v7();
    insert_run(&pool, fixture, active_run_id, "working")
        .await
        .expect("insert active Agent Run");
    let archive_thread = sqlx::query(
        r#"
        UPDATE conversations
        SET archived_at = now(), archived_by_actor_id = $2, version = version + 1
        WHERE id = $1
        "#,
    )
    .bind(fixture.thread_id)
    .bind(fixture.owner_actor_id)
    .execute(&pool)
    .await;
    assert!(
        archive_thread.is_err(),
        "a Thread with a non-terminal Agent Run cannot be archived"
    );
    let archive_parent = sqlx::query(
        r#"
        UPDATE conversations
        SET archived_at = now(), archived_by_actor_id = $2, version = version + 1
        WHERE id = $1
        "#,
    )
    .bind(fixture.channel_id)
    .bind(fixture.owner_actor_id)
    .execute(&pool)
    .await;
    assert!(
        archive_parent.is_err(),
        "a Channel owning a child Thread with an active run cannot be archived"
    );

    sqlx::query("UPDATE agent_runs SET state = 'failed', finished_at = now() WHERE id = $1")
        .bind(active_run_id)
        .execute(&pool)
        .await
        .expect("make the Agent Run terminal");
    let repository_change =
        sqlx::query("UPDATE conversations SET repository_id = $2 WHERE id = $1")
            .bind(fixture.thread_id)
            .bind(fixture.replacement_repository_id)
            .execute(&pool)
            .await;
    assert!(
        repository_change.is_err(),
        "a Thread repository is immutable after its first Agent Run"
    );
    sqlx::query(
        r#"
        UPDATE conversations
        SET archived_at = now(), archived_by_actor_id = $2, version = version + 1
        WHERE id = $1
        "#,
    )
    .bind(fixture.thread_id)
    .bind(fixture.owner_actor_id)
    .execute(&pool)
    .await
    .expect("archive a Thread after its runs become terminal");

    sqlx::query(
        r#"
        INSERT INTO project_navigation_states (
            project_id, actor_id, location_kind, conversation_id
        )
        VALUES ($1, $2, 'channel', $3)
        "#,
    )
    .bind(fixture.project_id)
    .bind(fixture.owner_actor_id)
    .bind(fixture.channel_id)
    .execute(&pool)
    .await
    .expect("remember an active Channel route");
    let wrong_navigation_kind = sqlx::query(
        r#"
        UPDATE project_navigation_states
        SET location_kind = 'thread', conversation_id = $3, version = version + 1
        WHERE project_id = $1 AND actor_id = $2
        "#,
    )
    .bind(fixture.project_id)
    .bind(fixture.owner_actor_id)
    .bind(fixture.channel_id)
    .execute(&pool)
    .await;
    assert!(
        wrong_navigation_kind.is_err(),
        "remembered navigation must match the Conversation kind"
    );

    let historical_child_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, parent_channel_id, name)
        VALUES ($1, $2, 'thread', $3, 'historical child')
        "#,
    )
    .bind(historical_child_id)
    .bind(fixture.project_id)
    .bind(fixture.channel_id)
    .execute(&pool)
    .await
    .expect("create active child for archived-parent navigation test");
    sqlx::query(
        r#"
        UPDATE project_navigation_states
        SET location_kind = 'thread', conversation_id = $3, version = version + 1
        WHERE project_id = $1 AND actor_id = $2
        "#,
    )
    .bind(fixture.project_id)
    .bind(fixture.owner_actor_id)
    .bind(historical_child_id)
    .execute(&pool)
    .await
    .expect("remember active child Thread");
    sqlx::query(
        r#"
        UPDATE conversations
        SET archived_at = now(), archived_by_actor_id = $2, version = version + 1
        WHERE id = $1
        "#,
    )
    .bind(fixture.channel_id)
    .bind(fixture.owner_actor_id)
    .execute(&pool)
    .await
    .expect("archive parent Channel with settled child history");
    let archived_parent_navigation = sqlx::query(
        r#"
        UPDATE project_navigation_states
        SET location_kind = 'thread', conversation_id = $3, version = version + 1
        WHERE project_id = $1 AND actor_id = $2
        "#,
    )
    .bind(fixture.project_id)
    .bind(fixture.owner_actor_id)
    .bind(historical_child_id)
    .execute(&pool)
    .await;
    assert!(
        archived_parent_navigation.is_err(),
        "remembered navigation cannot target a Thread beneath an archived Channel"
    );

    exercise_read_state(&pool, fixture).await;
    exercise_outbox(&pool, fixture).await;

    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop isolated M2 schema");
}

#[tokio::test]
async fn concurrent_m2_timeline_writers_allocate_gap_free_unique_sequences() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m2_sequence_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&administrative_pool)
        .await
        .expect("create isolated sequence schema");
    let pool = schema_pool(&database_url, &schema).await;
    apply_every_migration(&pool).await;

    let actor_id = Uuid::now_v7();
    let organization_id = Uuid::now_v7();
    let project_id = Uuid::now_v7();
    let channel_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(actor_id)
        .execute(&pool)
        .await
        .expect("insert sequence Actor");
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'M2 sequence', $2)")
        .bind(organization_id)
        .bind(format!("m2-sequence-{}", organization_id.simple()))
        .execute(&pool)
        .await
        .expect("insert sequence Organization");
    sqlx::query(
        "INSERT INTO projects (id, organization_id, project_key, name) VALUES ($1, $2, 'SQ', 'Sequence Project')",
    )
    .bind(project_id)
    .bind(organization_id)
    .execute(&pool)
    .await
    .expect("insert sequence Project");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'owner', 'Sequence owner', 'sequence-owner')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(actor_id)
    .execute(&pool)
    .await
    .expect("insert sequence membership");
    sqlx::query(
        "INSERT INTO conversations (id, project_id, kind, name) VALUES ($1, $2, 'channel', 'sequence')",
    )
    .bind(channel_id)
    .bind(project_id)
    .execute(&pool)
    .await
    .expect("insert sequence Channel");

    let barrier = Arc::new(Barrier::new(3));
    let mut writers = Vec::new();
    for _ in 0..2 {
        let writer_pool = pool.clone();
        let writer_barrier = Arc::clone(&barrier);
        writers.push(tokio::spawn(async move {
            writer_barrier.wait().await;
            let mut allocated = Vec::new();
            for _ in 0..20 {
                let mut transaction = writer_pool.begin().await.expect("begin timeline append");
                let sequence = allocate_conversation_sequence(&mut transaction, channel_id)
                    .await
                    .expect("allocate timeline sequence");
                sqlx::query(
                    r#"
                    INSERT INTO conversation_entries (
                        id, conversation_id, sequence, kind
                    )
                    VALUES ($1, $2, $3, 'system')
                    "#,
                )
                .bind(Uuid::now_v7())
                .bind(channel_id)
                .bind(sequence)
                .execute(&mut *transaction)
                .await
                .expect("insert allocated timeline entry");
                transaction.commit().await.expect("commit timeline append");
                allocated.push(sequence);
            }
            allocated
        }));
    }
    barrier.wait().await;
    let mut allocated = Vec::new();
    for writer in writers {
        allocated.extend(writer.await.expect("join timeline writer"));
    }
    allocated.sort_unstable();
    assert_eq!(allocated, (1_i64..=40).collect::<Vec<_>>());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM conversation_entries WHERE conversation_id = $1",
        )
        .bind(channel_id)
        .fetch_one(&pool)
        .await
        .expect("count concurrent timeline entries"),
        40
    );

    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop isolated sequence schema");
}

#[tokio::test]
async fn archive_updates_serialize_with_thread_and_navigation_validation() {
    let Some((database_url, administrative_pool)) = integration_pool().await else {
        return;
    };
    let schema = format!("m2_archive_race_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&administrative_pool)
        .await
        .expect("create isolated archive-race schema");
    let pool = schema_pool(&database_url, &schema).await;
    apply_every_migration(&pool).await;

    let actor_id = Uuid::now_v7();
    let organization_id = Uuid::now_v7();
    let project_id = Uuid::now_v7();
    let navigation_channel_id = Uuid::now_v7();
    let existing_thread_id = Uuid::now_v7();
    let creation_channel_id = Uuid::now_v7();
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(actor_id)
        .execute(&pool)
        .await
        .expect("insert archive-race Actor");
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'Archive race', $2)")
        .bind(organization_id)
        .bind(format!("archive-race-{}", organization_id.simple()))
        .execute(&pool)
        .await
        .expect("insert archive-race Organization");
    sqlx::query(
        "INSERT INTO projects (id, organization_id, project_key, name) VALUES ($1, $2, 'RACE', 'Archive Race')",
    )
    .bind(project_id)
    .bind(organization_id)
    .execute(&pool)
    .await
    .expect("insert archive-race Project");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle
        )
        VALUES ($1, $2, $3, 'owner', 'Archive owner', 'archive-owner')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(actor_id)
    .execute(&pool)
    .await
    .expect("insert archive-race membership");
    for (conversation_id, name) in [
        (navigation_channel_id, "Navigation parent"),
        (creation_channel_id, "Creation parent"),
    ] {
        sqlx::query(
            "INSERT INTO conversations (id, project_id, kind, name) VALUES ($1, $2, 'channel', $3)",
        )
        .bind(conversation_id)
        .bind(project_id)
        .bind(name)
        .execute(&pool)
        .await
        .expect("insert archive-race Channel");
    }
    sqlx::query(
        r#"
        INSERT INTO conversations (id, project_id, kind, parent_channel_id, name)
        VALUES ($1, $2, 'thread', $3, 'Existing child')
        "#,
    )
    .bind(existing_thread_id)
    .bind(project_id)
    .bind(navigation_channel_id)
    .execute(&pool)
    .await
    .expect("insert existing child Thread");
    sqlx::query(
        r#"
        INSERT INTO project_navigation_states (project_id, actor_id)
        VALUES ($1, $2)
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .execute(&pool)
    .await
    .expect("insert overview navigation state");

    let mut archive_navigation_parent = pool.begin().await.expect("begin parent archive");
    sqlx::query(
        "UPDATE conversations SET archived_at = now(), archived_by_actor_id = $1 WHERE id = $2",
    )
    .bind(actor_id)
    .bind(navigation_channel_id)
    .execute(&mut *archive_navigation_parent)
    .await
    .expect("stage navigation parent archive");
    let navigation_pool = pool.clone();
    let navigation_write = tokio::spawn(async move {
        sqlx::query(
            r#"
            UPDATE project_navigation_states
               SET location_kind = 'thread', conversation_id = $1
             WHERE project_id = $2 AND actor_id = $3
            "#,
        )
        .bind(existing_thread_id)
        .bind(project_id)
        .bind(actor_id)
        .execute(&navigation_pool)
        .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !navigation_write.is_finished(),
        "navigation validation must wait for the parent archive decision"
    );
    archive_navigation_parent
        .commit()
        .await
        .expect("commit navigation parent archive");
    let navigation_result = tokio::time::timeout(Duration::from_secs(3), navigation_write)
        .await
        .expect("navigation race completion deadline")
        .expect("join navigation writer");
    assert!(
        navigation_result.is_err(),
        "navigation validated after the archive commit must reject the stale child route"
    );

    let mut archive_creation_parent = pool.begin().await.expect("begin creation parent archive");
    sqlx::query(
        "UPDATE conversations SET archived_at = now(), archived_by_actor_id = $1 WHERE id = $2",
    )
    .bind(actor_id)
    .bind(creation_channel_id)
    .execute(&mut *archive_creation_parent)
    .await
    .expect("stage creation parent archive");
    let creation_pool = pool.clone();
    let new_thread_id = Uuid::now_v7();
    let thread_create = tokio::spawn(async move {
        sqlx::query(
            r#"
            INSERT INTO conversations (id, project_id, kind, parent_channel_id, name)
            VALUES ($1, $2, 'thread', $3, 'Racing child')
            "#,
        )
        .bind(new_thread_id)
        .bind(project_id)
        .bind(creation_channel_id)
        .execute(&creation_pool)
        .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !thread_create.is_finished(),
        "Thread validation must wait for the parent archive decision"
    );
    archive_creation_parent
        .commit()
        .await
        .expect("commit creation parent archive");
    let create_result = tokio::time::timeout(Duration::from_secs(3), thread_create)
        .await
        .expect("Thread race completion deadline")
        .expect("join Thread creator");
    assert!(
        create_result.is_err(),
        "Thread validated after the archive commit must reject the archived parent"
    );

    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&administrative_pool, &schema)
        .await
        .expect("drop isolated archive-race schema");
}

async fn exercise_read_state(pool: &PgPool, fixture: Fixture) {
    sqlx::query(
        r#"
        INSERT INTO conversation_read_states (
            project_id, conversation_id, actor_id, highest_seen_sequence
        )
        VALUES ($1, $2, $3, 1)
        "#,
    )
    .bind(fixture.project_id)
    .bind(fixture.thread_id)
    .bind(fixture.owner_actor_id)
    .execute(pool)
    .await
    .expect("insert first per-Actor read state");
    sqlx::query(
        r#"
        UPDATE conversation_read_states
        SET highest_seen_sequence = 2, version = 2, updated_at = now()
        WHERE project_id = $1 AND conversation_id = $2 AND actor_id = $3
        "#,
    )
    .bind(fixture.project_id)
    .bind(fixture.thread_id)
    .bind(fixture.owner_actor_id)
    .execute(pool)
    .await
    .expect("advance read state monotonically");

    sqlx::query(
        r#"
        INSERT INTO conversation_read_states (
            project_id, conversation_id, actor_id, highest_seen_sequence
        )
        VALUES ($1, $2, $3, 1)
        "#,
    )
    .bind(fixture.project_id)
    .bind(fixture.thread_id)
    .bind(fixture.collaborator_actor_id)
    .execute(pool)
    .await
    .expect("insert independent collaborator read state");
    let cursors = sqlx::query_as::<_, (Uuid, i64)>(
        r#"
        SELECT actor_id, highest_seen_sequence
        FROM conversation_read_states
        WHERE project_id = $1 AND conversation_id = $2
          AND actor_id IN ($3, $4)
        ORDER BY actor_id
        "#,
    )
    .bind(fixture.project_id)
    .bind(fixture.thread_id)
    .bind(fixture.owner_actor_id)
    .bind(fixture.collaborator_actor_id)
    .fetch_all(pool)
    .await
    .expect("load independent read states");
    assert_eq!(cursors.len(), 2);
    assert!(
        cursors
            .iter()
            .any(|row| row == &(fixture.owner_actor_id, 2))
    );
    assert!(
        cursors
            .iter()
            .any(|row| row == &(fixture.collaborator_actor_id, 1))
    );

    let backwards = sqlx::query(
        r#"
        UPDATE conversation_read_states
        SET highest_seen_sequence = 1, version = 3
        WHERE project_id = $1 AND conversation_id = $2 AND actor_id = $3
        "#,
    )
    .bind(fixture.project_id)
    .bind(fixture.thread_id)
    .bind(fixture.owner_actor_id)
    .execute(pool)
    .await;
    assert!(backwards.is_err(), "a read cursor cannot move backwards");

    let future_sequence = sqlx::query(
        r#"
        UPDATE conversation_read_states
        SET highest_seen_sequence = 3, version = 3
        WHERE project_id = $1 AND conversation_id = $2 AND actor_id = $3
        "#,
    )
    .bind(fixture.project_id)
    .bind(fixture.thread_id)
    .bind(fixture.owner_actor_id)
    .execute(pool)
    .await;
    assert!(
        future_sequence.is_err(),
        "a read cursor cannot move beyond the durable timeline"
    );

    let disabled_reader = sqlx::query(
        r#"
        INSERT INTO conversation_read_states (
            project_id, conversation_id, actor_id, highest_seen_sequence
        )
        VALUES ($1, $2, $3, 1)
        "#,
    )
    .bind(fixture.project_id)
    .bind(fixture.thread_id)
    .bind(fixture.disabled_actor_id)
    .execute(pool)
    .await;
    assert!(
        disabled_reader.is_err(),
        "disabled memberships cannot create read state"
    );
}

async fn exercise_outbox(pool: &PgPool, fixture: Fixture) {
    let first_event_id = Uuid::now_v7();
    let second_event_id = Uuid::now_v7();
    let first_cursor = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO outbox_events (
            event_id, project_id, conversation_id, resource_kind,
            resource_id, event_type, payload
        )
        VALUES (
            $1, $2, $3, 'message', $4,
            'conversation.message.created', '{"sequence":1}'::jsonb
        )
        RETURNING event_cursor
        "#,
    )
    .bind(first_event_id)
    .bind(fixture.project_id)
    .bind(fixture.thread_id)
    .bind(fixture.message_id)
    .fetch_one(pool)
    .await
    .expect("insert first outbox event");
    let second_cursor = sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO outbox_events (
            event_id, project_id, conversation_id, resource_kind,
            resource_id, event_type, payload
        )
        VALUES (
            $1, $2, $3, 'conversation', $3,
            'conversation.archived', '{}'::jsonb
        )
        RETURNING event_cursor
        "#,
    )
    .bind(second_event_id)
    .bind(fixture.project_id)
    .bind(fixture.thread_id)
    .fetch_one(pool)
    .await
    .expect("insert second outbox event");
    assert!(
        second_cursor > first_cursor,
        "publication ordering must use the database cursor"
    );

    let count_before_rollback = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM outbox_events")
        .fetch_one(pool)
        .await
        .expect("count outbox events before rollback");
    let mut transaction = pool.begin().await.expect("begin outbox rollback");
    sqlx::query(
        r#"
        INSERT INTO outbox_events (
            event_id, project_id, conversation_id, resource_kind,
            resource_id, event_type
        )
        VALUES ($1, $2, $3, 'conversation', $3, 'conversation.changed')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(fixture.project_id)
    .bind(fixture.thread_id)
    .execute(&mut *transaction)
    .await
    .expect("insert rolled-back outbox event");
    transaction
        .rollback()
        .await
        .expect("roll back outbox event");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM outbox_events")
            .fetch_one(pool)
            .await
            .expect("count outbox events after rollback"),
        count_before_rollback,
        "a rolled-back domain transaction must not emit an outbox row"
    );

    let foreign_scope = sqlx::query(
        r#"
        INSERT INTO outbox_events (
            event_id, project_id, conversation_id, resource_kind,
            resource_id, event_type
        )
        VALUES ($1, $2, $3, 'conversation', $3, 'conversation.changed')
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(fixture.foreign_project_id)
    .bind(fixture.thread_id)
    .execute(pool)
    .await;
    assert!(
        foreign_scope.is_err(),
        "outbox Conversation scope must match its Project"
    );

    let non_v7_event = sqlx::query(
        r#"
        INSERT INTO outbox_events (
            event_id, project_id, conversation_id, resource_kind,
            resource_id, event_type
        )
        VALUES ($1, $2, $3, 'conversation', $3, 'conversation.changed')
        "#,
    )
    .bind(Uuid::from_u128(1))
    .bind(fixture.project_id)
    .bind(fixture.thread_id)
    .execute(pool)
    .await;
    assert!(non_v7_event.is_err(), "outbox event IDs must be UUIDv7");

    let oversized_payload = sqlx::query(
        r#"
        INSERT INTO outbox_events (
            event_id, project_id, conversation_id, resource_kind,
            resource_id, event_type, payload
        )
        VALUES (
            $1, $2, $3, 'conversation', $3, 'conversation.changed',
            jsonb_build_object('value', repeat('x', 65537))
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(fixture.project_id)
    .bind(fixture.thread_id)
    .execute(pool)
    .await;
    assert!(
        oversized_payload.is_err(),
        "outbox payloads must remain bounded"
    );
}

async fn insert_m1_history(pool: &PgPool) -> Fixture {
    let owner_actor_id = Uuid::now_v7();
    let collaborator_actor_id = Uuid::now_v7();
    let worker_actor_id = Uuid::now_v7();
    let disabled_actor_id = Uuid::now_v7();
    let organization_id = Uuid::now_v7();
    let project_id = Uuid::now_v7();
    let foreign_project_id = Uuid::now_v7();
    let repository_id = Uuid::now_v7();
    let replacement_repository_id = Uuid::now_v7();
    let channel_id = Uuid::now_v7();
    let thread_id = Uuid::now_v7();
    let provider_connection_id = Uuid::now_v7();
    let execution_profile_id = Uuid::now_v7();
    let execution_worker_id = Uuid::now_v7();
    let terminal_run_id = Uuid::now_v7();
    let message_id = Uuid::now_v7();
    let event_id = Uuid::now_v7();

    let mut transaction = pool.begin().await.expect("begin M1 fixture");
    sqlx::query(
        r#"
        INSERT INTO actors (id, kind)
        VALUES
            ($1, 'human'),
            ($2, 'execution_worker'),
            ($3, 'human')
        "#,
    )
    .bind(owner_actor_id)
    .bind(worker_actor_id)
    .bind(disabled_actor_id)
    .execute(&mut *transaction)
    .await
    .expect("insert M1 Actors");
    sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
        .bind(collaborator_actor_id)
        .execute(&mut *transaction)
        .await
        .expect("insert collaborator Actor");
    sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, 'M2 upgrade', $2)")
        .bind(organization_id)
        .bind(format!("m2-upgrade-{}", organization_id.simple()))
        .execute(&mut *transaction)
        .await
        .expect("insert M1 Organization");
    sqlx::query(
        r#"
        INSERT INTO projects (id, organization_id, project_key, name)
        VALUES
            ($1, $3, 'M2', 'M2 Project'),
            ($2, $3, 'FX', 'Foreign Project')
        "#,
    )
    .bind(project_id)
    .bind(foreign_project_id)
    .bind(organization_id)
    .execute(&mut *transaction)
    .await
    .expect("insert M1 Projects");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle, enabled
        )
        VALUES
            ($1, $4, $5, 'owner', 'M2 owner', 'm2-owner', true),
            ($2, $4, $6, 'worker', 'M2 Worker', 'agent', true),
            ($3, $4, $7, 'member', 'Disabled reader', 'disabled-reader', false),
            ($8, $9, $5, 'owner', 'Foreign owner', 'foreign-owner', true)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(owner_actor_id)
    .bind(worker_actor_id)
    .bind(disabled_actor_id)
    .bind(Uuid::now_v7())
    .bind(foreign_project_id)
    .execute(&mut *transaction)
    .await
    .expect("insert M1 Project memberships");
    sqlx::query(
        r#"
        INSERT INTO project_memberships (
            id, project_id, actor_id, role, display_name, handle, enabled
        )
        VALUES ($1, $2, $3, 'member', 'M2 collaborator', 'm2-collaborator', true)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(project_id)
    .bind(collaborator_actor_id)
    .execute(&mut *transaction)
    .await
    .expect("insert collaborator Project membership");
    sqlx::query(
        r#"
        INSERT INTO repositories (
            id, project_id, owner, name, clone_url, mirror_path, trusted
        )
        VALUES
            ($1, $3, 'example', 'primary', 'https://example.invalid/primary.git', '/tmp/m2-primary.git', true),
            ($2, $3, 'example', 'replacement', 'https://example.invalid/replacement.git', '/tmp/m2-replacement.git', true)
        "#,
    )
    .bind(repository_id)
    .bind(replacement_repository_id)
    .bind(project_id)
    .execute(&mut *transaction)
    .await
    .expect("insert M1 repositories");
    sqlx::query(
        "INSERT INTO conversations (id, project_id, kind, name) VALUES ($1, $2, 'channel', 'implementation')",
    )
    .bind(channel_id)
    .bind(project_id)
    .execute(&mut *transaction)
    .await
    .expect("insert M1 Channel");
    sqlx::query(
        r#"
        INSERT INTO conversations (
            id, project_id, kind, parent_channel_id, repository_id, name
        )
        VALUES ($1, $2, 'thread', $3, $4, 'preserved M0 Thread')
        "#,
    )
    .bind(thread_id)
    .bind(project_id)
    .bind(channel_id)
    .bind(repository_id)
    .execute(&mut *transaction)
    .await
    .expect("insert M1 Thread");
    sqlx::query(
        r#"
        INSERT INTO provider_connections (
            id, project_id, kind, host, capabilities
        )
        VALUES ($1, $2, 'codex_app_server', 'local_project_host', '{}'::jsonb)
        "#,
    )
    .bind(provider_connection_id)
    .bind(project_id)
    .execute(&mut *transaction)
    .await
    .expect("insert provider connection");
    sqlx::query(
        r#"
        INSERT INTO execution_profiles (
            id, project_id, provider_connection_id, owner_actor_id,
            name, codex_home_path, status
        )
        VALUES ($1, $2, $3, $4, 'M2 profile', '/tmp/m2-codex', 'available')
        "#,
    )
    .bind(execution_profile_id)
    .bind(project_id)
    .bind(provider_connection_id)
    .bind(owner_actor_id)
    .execute(&mut *transaction)
    .await
    .expect("insert execution profile");
    sqlx::query(
        r#"
        INSERT INTO execution_workers (
            id, project_id, actor_id, provider_connection_id, execution_profile_id
        )
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(execution_worker_id)
    .bind(project_id)
    .bind(worker_actor_id)
    .bind(provider_connection_id)
    .bind(execution_profile_id)
    .execute(&mut *transaction)
    .await
    .expect("insert execution Worker");
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot, finished_at
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, 'completed',
            'preserved run', '{}'::jsonb, now()
        )
        "#,
    )
    .bind(terminal_run_id)
    .bind(thread_id)
    .bind(execution_worker_id)
    .bind(owner_actor_id)
    .bind(execution_profile_id)
    .bind(repository_id)
    .execute(&mut *transaction)
    .await
    .expect("insert preserved Agent Run");
    sqlx::query(
        r#"
        INSERT INTO messages (
            id, conversation_id, author_actor_id, content,
            plain_text, agent_run_id
        )
        VALUES (
            $1, $2, $3,
            '{"version":1,"nodes":[{"type":"text","text":"Preserved M0 worker response"}]}'::jsonb,
            'Preserved M0 worker response', $4
        )
        "#,
    )
    .bind(message_id)
    .bind(thread_id)
    .bind(worker_actor_id)
    .bind(terminal_run_id)
    .execute(&mut *transaction)
    .await
    .expect("insert preserved M0 Message");
    sqlx::query(
        r#"
        INSERT INTO agent_run_events (
            id, agent_run_id, event_key, kind, summary, payload
        )
        VALUES ($1, $2, 'preserved-event', 'run_completed', 'Preserved event', '{}'::jsonb)
        "#,
    )
    .bind(event_id)
    .bind(terminal_run_id)
    .execute(&mut *transaction)
    .await
    .expect("insert preserved M0 event");
    sqlx::query(
        r#"
        INSERT INTO conversation_entries (
            id, conversation_id, sequence, kind, message_id
        )
        VALUES ($1, $2, 1, 'message', $3)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(thread_id)
    .bind(message_id)
    .execute(&mut *transaction)
    .await
    .expect("insert preserved Message entry");
    sqlx::query(
        r#"
        INSERT INTO conversation_entries (
            id, conversation_id, sequence, kind, agent_run_event_id
        )
        VALUES ($1, $2, 2, 'worker_event', $3)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(thread_id)
    .bind(event_id)
    .execute(&mut *transaction)
    .await
    .expect("insert preserved Worker-event entry");
    sqlx::query("UPDATE conversations SET next_entry_sequence = 3 WHERE id = $1")
        .bind(thread_id)
        .execute(&mut *transaction)
        .await
        .expect("preserve next timeline sequence");
    transaction.commit().await.expect("commit M1 fixture");

    Fixture {
        owner_actor_id,
        collaborator_actor_id,
        worker_actor_id,
        disabled_actor_id,
        project_id,
        foreign_project_id,
        repository_id,
        replacement_repository_id,
        channel_id,
        thread_id,
        execution_worker_id,
        execution_profile_id,
        terminal_run_id,
        message_id,
        event_id,
    }
}

async fn insert_run(
    pool: &PgPool,
    fixture: Fixture,
    run_id: Uuid,
    state: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'M2 invariant test', '{}'::jsonb)
        "#,
    )
    .bind(run_id)
    .bind(fixture.thread_id)
    .bind(fixture.execution_worker_id)
    .bind(fixture.owner_actor_id)
    .bind(fixture.execution_profile_id)
    .bind(fixture.repository_id)
    .bind(state)
    .execute(pool)
    .await?;
    Ok(())
}

async fn apply_through_m1(pool: &PgPool) {
    for migration in [
        include_str!("../../migrations/0001_foundation.sql"),
        include_str!("../../migrations/0002_walking_skeleton.sql"),
        include_str!("../../migrations/0003_m0_consistency.sql"),
        include_str!("../../migrations/0004_m1_identity_projects.sql"),
    ] {
        sqlx::raw_sql(migration)
            .execute(pool)
            .await
            .expect("apply migration through M1");
    }
}

/// Today's schema: every migration, as the server runs them. Race tests use
/// it so they test the code that runs now, not an early version of it.
async fn apply_every_migration(pool: &PgPool) {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .expect("apply every migration");
}

async fn integration_pool() -> Option<(String, PgPool)> {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&database_url)
        .await
        .expect("connect to PostgreSQL");
    Some((database_url, pool))
}

async fn schema_pool(database_url: &str, schema: &str) -> PgPool {
    let schema = schema.to_owned();
    PgPoolOptions::new()
        .max_connections(8)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                sqlx::query("SELECT set_config('search_path', $1, false)")
                    .bind(schema)
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(database_url)
        .await
        .expect("connect to isolated M2 schema")
}
