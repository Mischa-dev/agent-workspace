//! C1.4 to C1.6 on data: task and page comments as they were kept before
//! they became messages (a reply, an edited comment with its versions, a
//! removed one, a file, notifications; an anchored, resolved page thread
//! with a reply and a deleted comment, and a comment on the whole page),
//! then the rest of the migrations. The silent, costly failure: a comment
//! that comes out with other words, another author, lost versions, a
//! removed one shown again, or a thread that lost its anchor or state.

use std::borrow::Cow;

use serde_json::{Value, json};
use sqlx::{Executor, migrate::Migrator, postgres::PgPoolOptions};
use uuid::Uuid;

use super::support::db::schema_pool;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// The migration that moves the comments.
const FIRST_NEW: i64 = 20261117210100;

fn id(n: u32) -> Uuid {
    Uuid::parse_str(&format!("00000000-0000-7000-8000-{n:012}")).expect("id")
}

#[derive(sqlx::FromRow, Debug, PartialEq)]
struct Moved {
    id: Uuid,
    author: Uuid,
    text: String,
    revision: i32,
    removed: bool,
    reply_to: Option<Uuid>,
    kind: String,
    thing: Uuid,
    anchor: Option<Value>,
    status: Option<String>,
}

#[tokio::test]
async fn comments_keep_their_words_authors_versions_and_threads_as_messages() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("admin");
    let schema = format!("c1_comments_{}", Uuid::now_v7().simple());
    admin
        .execute(format!("CREATE SCHEMA {schema}").as_str())
        .await
        .expect("schema");
    let pool = schema_pool(&database_url, &schema).await;
    let before = Migrator {
        migrations: Cow::Owned(
            MIGRATOR
                .iter()
                .filter(|migration| migration.version < FIRST_NEW)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    };
    before.run(&pool).await.expect("the old shape");
    sqlx::raw_sql(include_str!("../fixtures/c1_comments_old_shape.sql"))
        .execute(&pool)
        .await
        .expect("old comments");

    MIGRATOR.run(&pool).await.expect("the new shape");

    let (owner, bea, task, page) = (id(2), id(3), id(20), id(60));
    let moved = sqlx::query_as::<_, Moved>(
        r#"
        SELECT message.id, message.author_actor_id AS author, message.plain_text AS text,
               message.revision, message.removed_at IS NOT NULL AS removed,
               message.reply_to_message_id AS reply_to,
               COALESCE(parent.subject_kind, place.subject_kind) AS kind,
               COALESCE(parent.subject_id, place.subject_id) AS thing,
               place.anchor, place.thread_status AS status
        FROM messages message
        JOIN conversations place ON place.id = message.conversation_id
        LEFT JOIN conversations parent ON parent.id = place.parent_channel_id
        ORDER BY message.created_at
        "#,
    )
    .fetch_all(&pool)
    .await
    .expect("messages");
    let comment = |n: u32, author: Uuid, text: &str, revision: i32, removed: bool| Moved {
        id: id(n),
        author,
        text: text.to_owned(),
        revision,
        removed,
        reply_to: None,
        kind: "task".to_owned(),
        thing: task,
        anchor: None,
        status: Some("Open".to_owned()),
    };
    let anchor = json!({"kind": "text", "blockId": "block1", "quote": "first slice"});
    let on_page = |moved: Moved, anchor: Option<Value>, status: &str| Moved {
        kind: "page".to_owned(),
        thing: page,
        anchor,
        status: Some(status.to_owned()),
        ..moved
    };
    assert_eq!(
        moved,
        vec![
            comment(31, owner, "Steel or wood?", 1, false),
            Moved {
                reply_to: Some(id(31)),
                ..comment(32, bea, "Steel, surely", 2, false)
            },
            comment(33, bea, "", 1, true),
            on_page(
                comment(61, bea, "Which slice is first?", 1, false),
                Some(anchor.clone()),
                "Resolved"
            ),
            on_page(
                comment(62, owner, "The billing one", 2, false),
                Some(anchor.clone()),
                "Resolved"
            ),
            on_page(comment(63, bea, "", 1, true), Some(anchor), "Resolved"),
            on_page(
                comment(64, owner, "Looks good overall", 1, false),
                None,
                "Open"
            ),
            comment(35, owner, "Posted since", 1, false),
        ]
    );

    // The task's comments conversation kept the comment already in it, and
    // the moved ones follow it without taking its place.
    let order = sqlx::query_scalar::<_, Uuid>(
        "SELECT message_id FROM conversation_entries WHERE conversation_id = $1 ORDER BY sequence",
    )
    .bind(id(70))
    .fetch_all(&pool)
    .await
    .expect("entries");
    assert_eq!(order, [id(35), id(31), id(32), id(33)]);

    // The edited comment keeps both versions; who resolved the thread stays.
    let versions = sqlx::query_scalar::<_, String>(
        "SELECT plain_text FROM message_revisions WHERE message_id = $1 ORDER BY revision",
    )
    .bind(id(32))
    .fetch_all(&pool)
    .await
    .expect("versions");
    assert_eq!(versions, ["Steel", "Steel, surely"]);
    let resolver = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT thread_status_changed_by_actor_id FROM conversations WHERE anchor IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("resolver");
    assert_eq!(resolver, Some(owner));

    // The task's timeline names the messages in the same order.
    let timeline = sqlx::query_scalar::<_, Uuid>(
        "SELECT message_id FROM work_item_timeline_entries WHERE work_item_id = $1 ORDER BY sequence",
    )
    .bind(task)
    .fetch_all(&pool)
    .await
    .expect("timeline");
    assert_eq!(timeline, [id(31), id(32), id(33)]);

    // Each conversation holds its comments in order, read by everyone.
    let unread = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM conversation_entries entry
        JOIN project_memberships member ON member.project_id = $1
        LEFT JOIN conversation_read_states seen
          ON seen.conversation_id = entry.conversation_id AND seen.actor_id = member.actor_id
        WHERE COALESCE(seen.highest_seen_sequence, 0) < entry.sequence
        "#,
    )
    .bind(id(10))
    .fetch_one(&pool)
    .await
    .expect("unread");
    assert_eq!(unread, 0);
    let entries = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM conversation_entries")
        .fetch_one(&pool)
        .await
        .expect("entries");
    assert_eq!(entries, 8);

    // The file and the notifications follow their comment.
    let file = sqlx::query_as::<_, (Option<Uuid>, Option<Uuid>, bool)>(
        r#"
        SELECT file.message_id, file.work_item_id, place.subject_id = $2
        FROM files file JOIN conversations place ON place.id = file.conversation_id
        WHERE file.id = $1
        "#,
    )
    .bind(id(40))
    .bind(task)
    .fetch_one(&pool)
    .await
    .expect("file");
    assert_eq!(file, (Some(id(31)), None, true));
    let notified =
        sqlx::query_scalar::<_, Option<Uuid>>("SELECT message_id FROM notifications ORDER BY id")
            .fetch_all(&pool)
            .await
            .expect("notifications");
    assert_eq!(notified, [Some(id(32)), Some(id(61))]);
    let occurrence =
        sqlx::query_scalar::<_, Option<Uuid>>("SELECT message_id FROM notification_occurrences")
            .fetch_one(&pool)
            .await
            .expect("occurrence");
    assert_eq!(occurrence, Some(id(32)));

    // The old tables are gone.
    let left = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM information_schema.tables \
         WHERE table_schema = current_schema() \
           AND table_name IN ('work_item_comments', 'work_item_comment_revisions', \
                              'work_item_comment_mentions', 'work_item_comment_references', \
                              'project_page_comments')",
    )
    .fetch_one(&pool)
    .await
    .expect("tables");
    assert_eq!(left, 0);

    pool.close().await;
    agent_workspace_server::test_support::drop_schema(&admin, &schema)
        .await
        .expect("drop schema");
}
