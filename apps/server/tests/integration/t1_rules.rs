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
        // Records that name one change it; the others are new.
        let name = if records[0].get("record").is_some() {
            "record_update_many"
        } else {
            "record_create_many"
        };
        call(
            name,
            json!({"projectId": "WORK", "collection": "Parts", "records": records}),
        )
    };

    let made = call(
        "collection_create",
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
                {"name": "Note", "type": "text", "rules": {"default": "today"}},
                {"name": "Price", "type": "number", "rules": {"buildersOnly": true}},
            ],
        }),
    )
    .await;
    assert_eq!(made["outcome"], "ok", "{made}");

    // A rule that doesn't fit its field is refused.
    let wrong = call(
        "collection_update",
        json!({"projectId": "WORK", "collection": "Parts",
               "fields": [{"field": "Kind", "rules": {"unique": true}}]}),
    )
    .await;
    assert_ne!(wrong["outcome"], "ok", "{wrong}");

    // The definition (what a workspace template keeps) has the rules, a
    // choice's default by its option's name.
    let described = call(
        "collection_get",
        json!({"projectId": "WORK", "collection": "Parts"}),
    )
    .await;
    let kept = described["value"]["definition"]["fields"]
        .as_array()
        .expect("fields")
        .clone();
    let rules_of = |name: &str| {
        kept.iter()
            .find(|field| field["name"] == name)
            .map(|field| field["rules"].clone())
            .unwrap_or_default()
    };
    assert_eq!(rules_of("Kind")["default"], json!("Nut"), "{described}");
    assert_eq!(
        rules_of("Price")["buildersOnly"],
        json!(true),
        "{described}"
    );
    assert_eq!(rules_of("Added")["default"], json!("today"), "{described}");

    // A default whose option was archived since is left out of the
    // definition, which only writes out active options; the other rules stay.
    for change in [
        json!({"projectId": "WORK", "name": "Bins", "fields": [
            {"name": "Kind", "type": "single_choice", "options": [{"name": "Bolt"}, {"name": "Nut"}],
             "rules": {"default": "Nut", "required": true}}]}),
        json!({"projectId": "WORK", "collection": "Bins", "fields": [
            {"field": "Kind", "options": [{"name": "Bolt"}]}]}),
    ] {
        let name = if change.get("collection").is_some() {
            "collection_update"
        } else {
            "collection_create"
        };
        let changed = call(name, change).await;
        assert_eq!(changed["outcome"], "ok", "{changed}");
    }
    let bins = call(
        "collection_get",
        json!({"projectId": "WORK", "collection": "Bins"}),
    )
    .await;
    let kind = &bins["value"]["definition"]["fields"][0];
    assert_eq!(kind["rules"]["required"], json!(true), "{bins}");
    assert!(kind["rules"]["default"].is_null(), "{bins}");

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
    // Only a date field's "today" is a date.
    assert_eq!(fields["Note"], json!("today"), "{written}");

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

    // A two-way relation whose other side only builders set: setting this
    // side would set that one, so rita is refused here too.
    let suppliers = call(
        "collection_create",
        json!({"projectId": "WORK", "name": "Suppliers",
               "fields": [{"name": "Supplies", "type": "relation", "target": "record:Parts",
                           "twoWay": true, "pairName": "Supplied by"}]}),
    )
    .await;
    assert_eq!(suppliers["outcome"], "ok", "{suppliers}");
    let guarded = call(
        "collection_update",
        json!({"projectId": "WORK", "collection": "Parts",
               "fields": [{"field": "Supplied by", "rules": {"buildersOnly": true}}]}),
    )
    .await;
    assert_eq!(guarded["outcome"], "ok", "{guarded}");
    let suppliers =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM collections WHERE name = 'Suppliers'")
            .fetch_one(pool)
            .await
            .expect("suppliers");
    let (_, refused) = op(json!({"collection": suppliers, "title": "Acme",
                                  "fields": {"Supplies": ["Hex bolt M8"]}}))
    .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");
    assert!(refused.to_string().contains("Supplied by"), "{refused}");
    let (_, made) = op(json!({"collection": suppliers, "title": "Acme"})).await;
    assert_eq!(made["outcome"], "ok", "{made}");
    let paired = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM collection_records WHERE collection_id = $1 \
         AND field_values ? (SELECT id::text FROM field_definitions WHERE name = 'Supplied by')",
    )
    .bind(parts)
    .fetch_one(pool)
    .await
    .expect("paired");
    assert_eq!(paired, 0, "no part was paired");

    // Making a field unique while two records share a value is refused.
    let shared = write(json!([
        {"title": "A", "fields": {"Code": "AAA-1", "Kind": "Bolt"}},
    ]))
    .await;
    assert_eq!(shared["outcome"], "ok", "{shared}");
    let stock_unique = call(
        "collection_update",
        json!({"projectId": "WORK", "collection": "Parts",
               "fields": [{"field": "Stock", "rules": {"unique": true}}]}),
    )
    .await;
    assert_ne!(stock_unique["outcome"], "ok", "{stock_unique}");
    assert!(stock_unique.to_string().contains("#"), "{stock_unique}");

    world.finish().await;
}
