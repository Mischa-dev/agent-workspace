//! T1.19: the rules a collection's fields hold their values to. The silent,
//! costly failures: two records sharing a value meant to be unique (across
//! one batch, or by case), a required field left empty by one of the ways
//! in, a default that isn't applied, and someone who can't build the
//! collection changing a field only builders change.

use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::send,
    callers::{Call, Caller},
    world::WorldBuilder,
};

#[tokio::test]
async fn field_rules_hold_on_every_way_in() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("rita", "member")
        .connected_app("Codex")
        .custom_role("Records only", &["projects.view", "collections.edit"])
        .build("t1_rules")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let workspace = world.organization;
    let work = Uuid::parse_str(world.project("WORK")).expect("work");
    let codex = &world.apps["Codex"];
    let call = |name: &'static str, arguments: Value| async move {
        codex.call(app, Call::Tool { name, arguments }).await.1
    };
    let write = |records: Value| {
        call(
            "record_write",
            json!({"projectId": "WORK", "collection": "Parts", "records": records}),
        )
    };

    let made = call(
        "collection_write",
        json!({
            "projectId": "WORK",
            "name": "Parts",
            "fields": [
                {"name": "Code", "type": "text",
                 "rules": {"required": true, "unique": true, "pattern": "[A-Z]{3}-[0-9]+"}},
                {"name": "Stock", "type": "number",
                 "rules": {"min": 0, "max": 500, "default": 10}},
                {"name": "Kind", "type": "single_choice", "options": [{"name": "Bolt"}, {"name": "Nut"}],
                 "rules": {"default": "Nut"}},
                {"name": "Owner", "type": "person", "rules": {"default": "me"}},
                {"name": "Added", "type": "date", "rules": {"default": "today"}},
                {"name": "Price", "type": "number", "rules": {"buildersOnly": true}},
            ],
        }),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");

    // A rule that doesn't fit its field is refused.
    let wrong = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Parts",
               "fields": [{"field": "Kind", "rules": {"unique": true}}]}),
    )
    .await;
    assert_ne!(wrong["outcome"], "ok", "{wrong}");

    // Defaults fill what's left out; a value given wins.
    let written =
        write(json!([{"title": "Hex bolt", "fields": {"Code": "HEX-1", "Kind": "Bolt"}}])).await;
    assert_eq!(written["outcome"], "ok", "{written}");
    let fields = &written["value"]["records"][0]["fields"];
    assert_eq!(fields["Stock"], json!(10), "{written}");
    assert_eq!(fields["Kind"], json!("Bolt"), "{written}");
    assert!(fields["Owner"].is_string(), "{written}");
    assert_eq!(
        fields["Added"],
        json!(chrono::Utc::now().date_naive().to_string()),
        "{written}"
    );

    // Unique ignores case, and holds within one batch.
    for (records, why) in [
        (
            json!([{"title": "Again", "fields": {"Code": "hex-1"}}]),
            "case",
        ),
        (
            json!([
                {"title": "One", "fields": {"Code": "NUT-7"}},
                {"title": "Two", "fields": {"Code": "NUT-7"}},
            ]),
            "batch",
        ),
        (json!([{"title": "No code"}]), "required"),
        (
            json!([{"title": "Bad", "fields": {"Code": "bolt"}}]),
            "pattern",
        ),
        (
            json!([{"title": "Many", "fields": {"Code": "MNY-1", "Stock": 900}}]),
            "range",
        ),
    ] {
        let refused = write(records).await;
        assert_ne!(refused["outcome"], "ok", "{why}: {refused}");
    }
    // Nothing of a refused batch was kept.
    let kept = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM collection_records record \
         JOIN collections collection ON collection.id = record.collection_id \
         WHERE collection.name = 'Parts'",
    )
    .fetch_one(pool)
    .await
    .expect("count");
    assert_eq!(kept, 1);
    // A required value can't be cleared; changing the record otherwise is fine.
    let cleared = write(json!([{"record": "#1", "fields": {"Code": null}}])).await;
    assert_ne!(cleared["outcome"], "ok", "{cleared}");
    let renamed = write(json!([{"record": "#1", "title": "Hex bolt M8"}])).await;
    assert_eq!(renamed["outcome"], "ok", "{renamed}");
    // The owner builds, so sets the builders' field.
    let priced = write(json!([{"record": "#1", "fields": {"Price": 2}}])).await;
    assert_eq!(priced["outcome"], "ok", "{priced}");

    // Someone who edits records but can't build them: refused on the
    // builders' field, through the operation and the web route alike.
    sqlx::query(
        "UPDATE project_memberships SET role_id = $1 WHERE project_id = $2 AND actor_id = $3",
    )
    .bind(Uuid::parse_str(&world.roles["Records only"]).expect("role id"))
    .bind(work)
    .bind(world.people["rita"].actor)
    .execute(pool)
    .await
    .expect("narrow rita");
    let parts = sqlx::query_scalar::<_, Uuid>("SELECT id FROM collections WHERE name = 'Parts'")
        .fetch_one(pool)
        .await
        .expect("parts");
    let rita = &world.people["rita"].browser;
    let op = |input: Value| {
        let request = rita.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/record_create"),
            Some(&Uuid::now_v7().to_string()),
            input,
        );
        async move { send(app, request).await }
    };
    let (_, refused) = op(json!({"collection": parts, "title": "Washer",
                                  "fields": {"Code": "WSH-1", "Price": 1}}))
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    let (_, made) =
        op(json!({"collection": parts, "title": "Washer", "fields": {"Code": "WSH-1"}})).await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let (status, refused) = send(
        app,
        rita.request(
            "POST",
            &format!("/api/v1/projects/{work}/collections/{parts}/records"),
            Some(&Uuid::now_v7().to_string()),
            json!({"title": "Spring", "values": {"Code": "SPR-1", "Price": 3}}),
        ),
    )
    .await;
    assert!(!status.is_success(), "{status} {refused}");
    assert!(refused.to_string().contains("Price"), "{refused}");

    // Making a field unique while two records share a value is refused.
    let shared = write(json!([
        {"title": "A", "fields": {"Code": "AAA-1", "Kind": "Bolt"}},
    ]))
    .await;
    assert_eq!(shared["outcome"], "ok", "{shared}");
    let stock_unique = call(
        "collection_write",
        json!({"projectId": "WORK", "collection": "Parts",
               "fields": [{"field": "Stock", "rules": {"unique": true}}]}),
    )
    .await;
    assert_ne!(stock_unique["outcome"], "ok", "{stock_unique}");
    assert!(stock_unique.to_string().contains("#"), "{stock_unique}");

    world.finish().await;
}
