//! T1.41: records can be proposed, with history. The silent, costly
//! failures: a suggest-only role writing a record directly, a proposal's
//! author deciding it themselves, an accepted proposal written as whoever
//! accepted it (or undoing a change made since), and a version put back
//! wrongly or from values history never kept.

use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::send,
    callers::{Call, Caller},
    world::WorldBuilder,
};

fn ok(answer: &Value) -> &Value {
    assert_eq!(answer["outcome"], "ok", "{answer}");
    &answer["value"]
}

#[tokio::test]
async fn a_suggested_record_change_is_accepted_from_its_card_and_can_be_put_back() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("Codex")
        .agent("sol", "member")
        .custom_role(
            "Member, suggests",
            &[
                "projects.view",
                "collections.suggest",
                "requests.raise",
                "directory.view",
            ],
        )
        .build("t1_record_proposals")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let workspace = world.organization;
    let work = Uuid::parse_str(world.project("WORK")).expect("work");
    let (codex, sol) = (&world.apps["Codex"], &world.agents["sol"]);
    let as_codex = |name: &'static str, arguments: Value| async move {
        codex.call(app, Call::Tool { name, arguments }).await.1
    };
    let as_sol = |name: &'static str, arguments: Value| async move {
        sol.call(app, Call::Tool { name, arguments }).await.1
    };
    sqlx::query(
        "UPDATE project_memberships SET role_id = $1 WHERE project_id = $2 AND actor_id = $3",
    )
    .bind(Uuid::parse_str(&world.roles["Member, suggests"]).expect("role id"))
    .bind(work)
    .bind(sol.actor)
    .execute(pool)
    .await
    .expect("sol only suggests");

    ok(&as_codex(
        "collection_create",
        json!({"projectId": "WORK", "name": "Leads", "fields": [
            {"name": "Stage", "type": "single_choice", "options": [{"name": "New"}, {"name": "Won"}]},
            {"name": "Amount", "type": "number"},
        ]}),
    )
    .await);
    let made = as_codex(
        "record_create_many",
        json!({"projectId": "WORK", "collection": "Leads", "clientRequestId": "acme",
               "records": [{"title": "Acme", "fields": {"Stage": "New", "Amount": 100}}]}),
    )
    .await;
    let record =
        Uuid::parse_str(ok(&made)["records"][0]["id"].as_str().expect("id")).expect("record id");
    let read = || async move {
        sqlx::query_as::<_, (i64, Value)>(
            "SELECT version, field_values FROM collection_records WHERE id = $1",
        )
        .bind(record)
        .fetch_one(pool)
        .await
        .expect("record")
    };
    // The record's one number field.
    let amount = |values: &Value| {
        values
            .as_object()
            .and_then(|object| object.values().find(|value| value.is_number()).cloned())
    };

    // Sol only suggests: writing directly is refused, and record_update
    // keeps the change as a proposal, leaving the record as it was.
    let direct = as_sol(
        "record_update_many",
        json!({"projectId": "WORK", "collection": "Leads", "clientRequestId": "sol-1",
               "records": [{"record": record.to_string(), "fields": {"Amount": 1}}]}),
    )
    .await;
    assert_ne!(direct["outcome"], "ok", "{direct}");
    let (version_before, _) = read().await;
    let proposed = as_sol(
        "record_update",
        json!({"record": record.to_string(), "fields": {"Stage": "Won", "Amount": 250},
               "expectedVersion": version_before, "note": "Signed today"}),
    )
    .await;
    let proposal = ok(&proposed)["proposalId"]
        .as_str()
        .expect("proposal")
        .to_owned();
    assert_eq!(
        ok(&proposed)["version"],
        json!(version_before),
        "{proposed}"
    );
    assert_eq!(
        ok(&proposed)["proposed"].as_array().map(Vec::len),
        Some(2),
        "{proposed}"
    );
    assert!(
        !proposed.to_string().contains("\"was\""),
        "stored values stay inside: {proposed}"
    );
    let (version_now, values) = read().await;
    assert_eq!(version_now, version_before);
    assert_eq!(amount(&values), Some(json!(100)));

    // Its author can't decide it; its card asks the people who edit it.
    let own = as_sol(
        "proposal_decide",
        json!({"proposalId": proposal, "decision": "accept"}),
    )
    .await;
    assert_eq!(own["outcome"], "permission_denied", "{own}");
    let request = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM requests WHERE subject_kind = 'proposal' AND subject_id = $1::uuid \
           AND status = 'open'",
    )
    .bind(&proposal)
    .fetch_one(pool)
    .await
    .expect("the proposal's request card");
    // The card opens the record, where the change shows beside it.
    let hrefs = sqlx::query_scalar::<_, Option<String>>(
        "SELECT detail->>'href' FROM notifications WHERE subject_kind = 'request' AND subject_id = $1",
    )
    .bind(request)
    .fetch_all(pool)
    .await
    .expect("the card's notifications");
    assert!(
        !hrefs.is_empty()
            && hrefs.iter().all(|href| href
                .as_deref()
                .is_some_and(|href| href.contains(&format!("record={record}")))),
        "{hrefs:?}"
    );

    // The owner accepts it from the card: written as Sol's change, and the
    // history says who accepted it.
    ok(&as_codex(
        "request_respond",
        json!({"requestId": request.to_string(), "optionId": "accept"}),
    )
    .await);
    let (accepted_version, values) = read().await;
    assert_eq!(accepted_version, version_before + 1);
    assert_eq!(amount(&values), Some(json!(250)));
    let (status, applied) = sqlx::query_as::<_, (String, Option<i64>)>(
        "SELECT status, applied_version FROM proposals WHERE id = $1::uuid",
    )
    .bind(&proposal)
    .fetch_one(pool)
    .await
    .expect("proposal");
    assert_eq!(
        (status.as_str(), applied),
        ("accepted", Some(accepted_version))
    );
    let (author, accepter) = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
        "SELECT actor_id, accepted_by_actor_id FROM collection_record_history \
         WHERE record_id = $1 AND version = $2",
    )
    .bind(record)
    .bind(accepted_version)
    .fetch_one(pool)
    .await
    .expect("history row");
    assert_eq!(author, sol.actor);
    assert_eq!(accepter, Some(world.owner.actor));
    let history = as_codex("history_list", json!({"thing": record.to_string()})).await;
    let newest = &ok(&history)["entries"][0];
    assert_eq!(newest["version"], json!(accepted_version), "{history}");
    assert!(newest["acceptedBy"].is_string(), "{history}");
    assert_ne!(newest["acceptedBy"], newest["actor"], "{history}");
    assert_eq!(
        ok(&history)["caps"]["restore"],
        json!(true),
        "records restore: {history}"
    );

    // A proposal over a field someone changed since is refused, never
    // undoing that change.
    let second = as_sol(
        "record_update",
        json!({"record": record.to_string(), "fields": {"Amount": 300}}),
    )
    .await;
    let second = ok(&second)["proposalId"]
        .as_str()
        .expect("second proposal")
        .to_owned();
    ok(&as_codex(
        "record_update",
        json!({"record": record.to_string(), "fields": {"Amount": 400}}),
    )
    .await);
    let stale = as_codex(
        "proposal_decide",
        json!({"proposalId": second, "decision": "accept"}),
    )
    .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    let (_, values) = read().await;
    assert_eq!(amount(&values), Some(json!(400)));

    // Version 1 as it was, then put back over MCP: Stage and Amount return.
    let shown = as_codex(
        "history_show",
        json!({"thing": record.to_string(), "version": 1}),
    )
    .await;
    let names = ok(&shown)["content"]["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .filter_map(|field| field["name"].as_str())
        .collect::<Vec<_>>();
    assert!(
        names.contains(&"Stage") && names.contains(&"Amount"),
        "{shown}"
    );
    let (now, _) = read().await;
    let restored = as_codex(
        "history_restore",
        json!({"thing": record.to_string(), "version": 1, "expectedVersion": now}),
    )
    .await;
    assert_eq!(ok(&restored)["version"], json!(now + 1), "{restored}");
    let (now, values) = read().await;
    assert_eq!(amount(&values), Some(json!(100)));
    // A stale expectedVersion is refused.
    let moved = as_codex(
        "history_restore",
        json!({"thing": record.to_string(), "version": 1, "expectedVersion": now - 1}),
    )
    .await;
    assert_eq!(moved["outcome"], "conflict", "{moved}");

    // Over HTTP too: the accepted version comes back.
    let (status, answer) = send(
        app,
        world.owner.browser.request(
            "POST",
            &format!("/api/v2/w/{workspace}/ops/history_restore"),
            Some(&Uuid::now_v7().to_string()),
            json!({"thing": record.to_string(), "version": accepted_version, "expectedVersion": now}),
        ),
    )
    .await;
    assert!(status.is_success(), "{status} {answer}");
    let (_, values) = read().await;
    assert_eq!(amount(&values), Some(json!(250)));

    // Sol may not put a version back: that's editing.
    let (now, _) = read().await;
    let refused = as_sol(
        "history_restore",
        json!({"thing": record.to_string(), "version": 1, "expectedVersion": now}),
    )
    .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");

    // A version needing values history never kept says so.
    sqlx::query(
        "UPDATE collection_record_history \
         SET changes = (SELECT jsonb_agg(change - 'was' - 'is') FROM jsonb_array_elements(changes) change) \
         WHERE record_id = $1 AND version = 2",
    )
    .bind(record)
    .execute(pool)
    .await
    .expect("as before T1.41");
    let not_kept = as_codex(
        "history_show",
        json!({"thing": record.to_string(), "version": 1}),
    )
    .await;
    assert_eq!(not_kept["outcome"], "conflict", "{not_kept}");

    world.finish().await;
}
