//! K4.16 and K4.17 on data: a workspace as it was before DMs left the
//! hidden Messages project (a DM with a message, a read state, a
//! notification and a saved item, beside an ordinary channel), then the
//! rest of the migrations. The silent, costly failure: a DM that comes out
//! of the move unreadable, with a stray space, or visible to someone new;
//! or its members no longer in the workspace's own space.

use std::borrow::Cow;

use sqlx::{Executor, migrate::Migrator, postgres::PgPoolOptions};
use uuid::Uuid;

use super::support::db::schema_pool;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// The move: everything before it is the old shape.
const FIRST_NEW: i64 = 20261117173117;

const WORKSPACE: &str = "00000000-0000-7000-8000-0000000000a1";
const ANN: &str = "00000000-0000-7000-8000-0000000000b1";
const BO: &str = "00000000-0000-7000-8000-0000000000b2";
const OUTSIDER: &str = "00000000-0000-7000-8000-0000000000b3";
const PROJECT: &str = "00000000-0000-7000-8000-0000000000c1";
const MESSAGES_SPACE: &str = "00000000-0000-7000-8000-0000000000c2";
const DM: &str = "00000000-0000-7000-8000-0000000000d1";
const CHANNEL: &str = "00000000-0000-7000-8000-0000000000d2";
const MESSAGE: &str = "00000000-0000-7000-8000-0000000000e1";

fn seed() -> Vec<String> {
    vec![
        format!(
            "INSERT INTO organizations (id, name, slug) VALUES ('{WORKSPACE}', 'Acme', 'acme')"
        ),
        format!(
            "INSERT INTO actors (id, kind) VALUES ('{ANN}', 'human'), ('{BO}', 'human'), \
             ('{OUTSIDER}', 'human')"
        ),
        format!(
            "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES \
             ('{WORKSPACE}', '{ANN}', 'owner'), ('{WORKSPACE}', '{BO}', 'member'), \
             ('{WORKSPACE}', '{OUTSIDER}', 'member')"
        ),
        format!(
            "INSERT INTO projects (id, organization_id, project_key, name) \
             VALUES ('{PROJECT}', '{WORKSPACE}', 'ACME', 'Acme')"
        ),
        format!(
            "INSERT INTO projects (id, organization_id, project_key, name, kind) \
             VALUES ('{MESSAGES_SPACE}', '{WORKSPACE}', 'DM', 'Messages', 'direct')"
        ),
        format!(
            "INSERT INTO project_counters (project_id) VALUES ('{PROJECT}'), ('{MESSAGES_SPACE}') \
             ON CONFLICT DO NOTHING"
        ),
        format!(
            "INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle) \
             VALUES (gen_random_uuid(), '{PROJECT}', '{ANN}', 'owner', 'Ann', 'ann'), \
                    (gen_random_uuid(), '{MESSAGES_SPACE}', '{ANN}', 'owner', 'Ann', 'ann'), \
                    (gen_random_uuid(), '{MESSAGES_SPACE}', '{BO}', 'member', 'Bo', 'bo'), \
                    (gen_random_uuid(), '{MESSAGES_SPACE}', '{OUTSIDER}', 'member', 'Oz', 'oz') \
             ON CONFLICT DO NOTHING"
        ),
        format!(
            "INSERT INTO conversations (id, project_id, kind, name, visibility, dm_key) \
             VALUES ('{DM}', '{MESSAGES_SPACE}', 'dm', 'Direct message', 'members', '{ANN}:{BO}')"
        ),
        format!(
            "INSERT INTO conversations (id, project_id, kind, name, visibility) \
             VALUES ('{CHANNEL}', '{PROJECT}', 'channel', 'general', 'project')"
        ),
        format!(
            "INSERT INTO conversation_members (project_id, conversation_id, actor_id, added_by_actor_id) \
             VALUES ('{MESSAGES_SPACE}', '{DM}', '{ANN}', '{ANN}'), \
                    ('{MESSAGES_SPACE}', '{DM}', '{BO}', '{ANN}')"
        ),
        format!(
            "INSERT INTO messages (id, conversation_id, author_actor_id, content, plain_text, project_id) \
             VALUES ('{MESSAGE}', '{DM}', '{ANN}', '{{\"type\":\"doc\"}}', 'hi', '{MESSAGES_SPACE}')"
        ),
        format!(
            "INSERT INTO conversation_read_states (project_id, conversation_id, actor_id, highest_seen_sequence) \
             VALUES ('{MESSAGES_SPACE}', '{DM}', '{BO}', 0)"
        ),
        format!(
            "INSERT INTO notifications (id, project_id, recipient_actor_id, kind, subject_kind, \
                 subject_id, conversation_id, message_id, group_key) \
             VALUES (gen_random_uuid(), '{MESSAGES_SPACE}', '{BO}', 'direct_message', 'conversation', \
                 '{DM}', '{DM}', '{MESSAGE}', 'dm:{DM}')"
        ),
        format!(
            "INSERT INTO saved_items (id, organization_id, project_id, actor_id, kind, message_id) \
             VALUES (gen_random_uuid(), '{WORKSPACE}', '{MESSAGES_SPACE}', '{BO}', 'message', '{MESSAGE}')"
        ),
    ]
}

async fn one<T>(pool: &sqlx::PgPool, sql: &str) -> T
where
    T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + Send + Unpin,
{
    sqlx::query_scalar::<_, T>(sql)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"))
}

#[tokio::test]
async fn pre_r20_dms_are_personal_and_leave_their_space() {
    let Ok(database_url) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping PostgreSQL integration test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .expect("admin");
    let schema = format!("k4_dms_{}", Uuid::now_v7().simple());
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
    for statement in seed() {
        pool.execute(statement.as_str())
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }

    MIGRATOR.run(&pool).await.expect("the new shape");

    // The DM is in no space, still its workspace's, and personal (not kept).
    let (space, workspace, kept) = sqlx::query_as::<_, (Option<Uuid>, Uuid, bool)>(&format!(
        "SELECT project_id, workspace_id, kept FROM conversations WHERE id = '{DM}'"
    ))
    .fetch_one(&pool)
    .await
    .expect("the DM");
    assert_eq!(space, None);
    assert_eq!(workspace.to_string(), WORKSPACE);
    assert!(!kept);
    // Everything in it followed it out.
    for table in [
        "messages",
        "conversation_read_states",
        "conversation_members_update",
    ] {
        let left = one::<i64>(
            &pool,
            &format!(
                "SELECT count(*) FROM {table} WHERE conversation_id = '{DM}' AND project_id IS NOT NULL"
            ),
        )
        .await;
        assert_eq!(left, 0, "{table} still names a space");
    }
    for table in ["notifications", "saved_items"] {
        let moved = one::<i64>(
            &pool,
            &format!(
                "SELECT count(*) FROM {table} WHERE message_id = '{MESSAGE}' AND project_id IS NULL"
            ),
        )
        .await;
        assert_eq!(moved, 1, "{table} lost its row or kept a space");
    }
    // The channel kept its space.
    let channel = one::<Option<Uuid>>(
        &pool,
        &format!("SELECT project_id FROM conversations WHERE id = '{CHANNEL}'"),
    )
    .await;
    assert_eq!(channel.map(|id| id.to_string()).as_deref(), Some(PROJECT));
    // No hidden Messages project is left.
    let hidden = one::<i64>(
        &pool,
        &format!("SELECT count(*) FROM projects WHERE id = '{MESSAGES_SPACE}'"),
    )
    .await;
    assert_eq!(hidden, 0);
    // Its members are in the workspace's own space.
    let in_workspace_space = one::<i64>(
        &pool,
        &format!(
            "SELECT count(*) FROM project_memberships membership \
             JOIN projects space ON space.id = membership.project_id \
             WHERE space.organization_id = '{WORKSPACE}' AND space.kind = 'workspace' \
               AND membership.actor_id IN ('{ANN}', '{BO}', '{OUTSIDER}') AND membership.enabled"
        ),
    )
    .await;
    assert_eq!(in_workspace_space, 3);
    // Who sees it is unchanged: its two members, never the third.
    for (account, sees) in [(ANN, true), (BO, true), (OUTSIDER, false)] {
        let visible = one::<bool>(
            &pool,
            &format!("SELECT conversation_visible_to('{DM}', '{account}')"),
        )
        .await;
        assert_eq!(visible, sees, "{account}");
    }
    // Its members may still post there, through their workspace roles.
    let may_post =
        one::<Vec<String>>(&pool, &format!("SELECT direct_permissions('{DM}', '{BO}')")).await;
    assert!(
        may_post.iter().any(|key| key == "channels.post"),
        "{may_post:?}"
    );
    // A new DM goes in the direct scope, with no space; one with a space
    // is refused.
    let refused = pool
        .execute(
            format!(
                "INSERT INTO conversations (id, project_id, kind, name, visibility) \
                 VALUES (gen_random_uuid(), '{PROJECT}', 'dm', 'Direct message', 'members')"
            )
            .as_str(),
        )
        .await;
    assert!(refused.is_err(), "a DM in a space");
}
