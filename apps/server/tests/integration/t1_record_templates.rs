//! T1.30: record templates and recurring records. The silent, costly
//! failures: a recurring record that makes two records for one occurrence
//! (a job run twice), or keeps acting for someone who can no longer add
//! records; and a template whose values a record would refuse.

use serde_json::{Value, json};

use super::support::{
    callers::{Call, Caller},
    world::WorldBuilder,
};

#[tokio::test]
async fn a_recurring_record_makes_one_record_per_occurrence_and_pauses_when_its_owner_leaves() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("agent_workspace=error")
        .try_init();
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("vic", "viewer")
        .agent("rook", "member")
        .connected_app("laptop")
        .build("t1_record_templates")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let laptop = &world.apps["laptop"];
    let rook = &world.agents["rook"];
    let vic = &world.people["vic"];
    let tool = |name: &'static str, arguments: Value| Call::Tool { name, arguments };

    let (_, made) = laptop
        .call(
            app,
            tool(
                "collection_create",
                json!({
                    "projectId": "WORK",
                    "name": "Check-ins",
                    "fields": [
                        {"name": "Mood", "type": "single_choice", "options": [{"name": "Good"}, {"name": "Rough"}]},
                        {"name": "Notes", "type": "text"},
                    ],
                }),
            ),
        )
        .await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let collection = made["value"]["collection"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{made}"))
        .to_owned();

    // A value a record would refuse is refused on the template too.
    let (_, refused) = laptop
        .call(
            app,
            tool(
                "record_template_create",
                json!({"collection": collection, "name": "Bad", "title": "Bad",
                       "fields": {"Mood": "Ecstatic"}, "clientRequestId": "template-bad"}),
            ),
        )
        .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    let (_, template) = laptop
        .call(
            app,
            tool(
                "record_template_create",
                json!({"collection": collection, "name": "Weekly", "title": "Check-in {date}",
                       "fields": {"Mood": "Good", "Notes": "All fine"},
                       "clientRequestId": "template-weekly"}),
            ),
        )
        .await;
    assert_eq!(template["outcome"], "ok", "{template}");
    let template_id = template["value"]["id"].as_str().expect("id").to_owned();

    // A record from it: its title with today's date, its values under what's given.
    let (_, record) = rook
        .call(
            app,
            tool(
                "record_create",
                json!({"collection": collection, "template": template_id,
                       "fields": {"Notes": "Tired"}, "clientRequestId": "from-template"}),
            ),
        )
        .await;
    assert_eq!(record["outcome"], "ok", "{record}");
    let today = chrono::Utc::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    assert_eq!(
        record["value"]["title"],
        format!("Check-in {today}"),
        "{record}"
    );

    // Every day at nine, as Rook.
    let (_, recurring) = rook
        .call(
            app,
            tool(
                "recurring_record_create",
                json!({"template": template_id, "frequency": "daily", "timeOfDay": "09:00",
                       "timezone": "Europe/Amsterdam", "clientRequestId": "daily"}),
            ),
        )
        .await;
    assert_eq!(recurring["outcome"], "ok", "{recurring}");
    assert!(recurring["value"]["nextRunAt"].is_string(), "{recurring}");
    let recurring_id = recurring["value"]["id"].as_str().expect("id").to_owned();

    // A viewer can't set one up.
    let (status, _) = super::agent_interface::send(
        app,
        vic.browser.request(
            "POST",
            "/api/v2/ops/recurring_record_create",
            Some("vic-daily"),
            json!({"template": template_id, "frequency": "daily"}),
        ),
    )
    .await;
    assert_eq!(status.as_u16(), 403);

    let made_count = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM collection_records WHERE collection_id = $1::uuid",
        )
        .bind(&collection)
        .fetch_one(pool)
        .await
        .expect("count")
    };
    let make_due = || async {
        sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "UPDATE recurring_records SET next_run_at = date_trunc('second', now()) - interval '1 minute' \
             WHERE id = $1::uuid RETURNING next_run_at",
        )
        .bind(&recurring_id)
        .fetch_one(pool)
        .await
        .expect("due")
    };
    let sweep = || async {
        agent_workspace_server::jobs::enqueue(
            pool,
            agent_workspace_server::jobs::NewJob::new("collections.recurring_sweep"),
        )
        .await
        .expect("queue sweep");
        super::support::run_due_jobs(&world.state).await;
    };

    let due = make_due().await;
    sweep().await;
    assert_eq!(made_count().await, 2);
    let (next, author) = sqlx::query_as::<_, (Option<chrono::DateTime<chrono::Utc>>, uuid::Uuid)>(
        "SELECT recurring.next_run_at, record.created_by_actor_id \
         FROM recurring_records recurring \
         JOIN collection_records record ON record.id = recurring.last_record_id \
         WHERE recurring.id = $1::uuid",
    )
    .bind(&recurring_id)
    .fetch_one(pool)
    .await
    .expect("after run");
    assert!(next.expect("next run") > chrono::Utc::now());
    assert_eq!(author, rook.actor);

    // The same occurrence again (a job delivered twice) makes nothing new.
    sqlx::query("UPDATE recurring_records SET next_run_at = $2 WHERE id = $1::uuid")
        .bind(&recurring_id)
        .bind(due)
        .execute(pool)
        .await
        .expect("rewind");
    sweep().await;
    assert_eq!(made_count().await, 2);

    // Once Rook can't add records here, it pauses instead of acting.
    sqlx::query(
        "UPDATE project_memberships SET enabled = false WHERE actor_id = $1 AND project_id = $2::uuid",
    )
    .bind(rook.actor)
    .bind(world.project("WORK"))
    .execute(pool)
    .await
    .expect("remove rook");
    make_due().await;
    sweep().await;
    assert_eq!(made_count().await, 2);
    let (_, listed) = laptop
        .call(
            app,
            tool("recurring_record_list", json!({"collection": collection})),
        )
        .await;
    let paused = &listed["value"]["recurring"][0];
    assert_eq!(paused["enabled"], false, "{listed}");
    assert!(
        paused["pausedReason"]
            .as_str()
            .is_some_and(|reason| reason.contains("no longer")),
        "{listed}"
    );

    // Started again by someone else, it acts as them.
    let (_, restarted) = laptop
        .call(
            app,
            tool(
                "recurring_record_update",
                json!({"recurring": recurring_id, "enabled": true,
                       "clientRequestId": "restart"}),
            ),
        )
        .await;
    assert_eq!(restarted["outcome"], "ok", "{restarted}");
    assert_ne!(
        restarted["value"]["ownerId"],
        rook.actor.to_string(),
        "{restarted}"
    );

    world.finish().await;
}
