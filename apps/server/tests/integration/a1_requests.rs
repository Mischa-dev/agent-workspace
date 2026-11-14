//! A1.7: the requests record. The silent failures: an approval or question
//! from before the upgrade missing from `requests`, or copied with the
//! wrong answer or addressee; and an answer given the old way reaching the
//! old table but not the record (or the other way round).

use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    a1_runs_record::{insert_run, schema_before, seed},
    agent_interface::{connect_app, create_agent, initialize, tool},
    mcp_admin::{create_project, setup},
    mcp_tasks::add_member,
    message_edits::add_person,
};

const REQUESTS_MIGRATION: i64 = 20261114074500;

#[derive(Debug, PartialEq, sqlx::FromRow)]
struct Copied {
    kind: String,
    raised_by_account_id: Uuid,
    run_id: Option<Uuid>,
    space_id: Option<Uuid>,
    conversation_id: Option<Uuid>,
    title: String,
    options: Value,
    allow_text: bool,
    answer_permission: String,
    addressed_account_ids: Vec<Uuid>,
    addressed_role_keys: Vec<String>,
    status: String,
    answer: Option<Value>,
    answered_by_account_id: Option<Uuid>,
    legacy_source: Option<String>,
}

async fn copied(pool: &sqlx::PgPool, id: Uuid) -> Copied {
    sqlx::query_as::<_, Copied>(
        r#"
        SELECT kind, raised_by_account_id, run_id, space_id, conversation_id, title, options,
               allow_text, answer_permission, addressed_account_ids, addressed_role_keys,
               status, answer, answered_by_account_id, legacy_source
        FROM requests WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| panic!("request {id}: {error}"))
}

#[tokio::test]
async fn old_approvals_and_questions_are_copied_with_their_ids() {
    let Some((pool, full, root)) = schema_before("a1_requests_upgrade", REQUESTS_MIGRATION).await
    else {
        return;
    };
    let seed = seed(&pool, &root).await;
    let run = insert_run(&pool, &seed, "blocked", None, "Waiting on people").await;
    let approval = |status: &'static str, response: Option<Value>| {
        let pool = pool.clone();
        let seed = &seed;
        async move {
            let id = Uuid::now_v7();
            sqlx::query(
                r#"
                INSERT INTO agent_approvals (
                    id, agent_run_id, requesting_actor_id, provider_request_id, kind, summary,
                    options, eligible_role, human_required, status, response,
                    responded_by_actor_id, responded_at
                )
                VALUES ($1, $2, $3, $4, 'approval', 'Run the migration?',
                        '[{"id": "approve", "label": "Allow"}, {"id": "deny", "label": "Stop"}]',
                        'owner', true, $5, $6,
                        CASE WHEN $5 = 'resolved' THEN $3 END,
                        CASE WHEN $5 = 'resolved' THEN now() END)
                "#,
            )
            .bind(id)
            .bind(run)
            .bind(seed.human_actor_id)
            .bind(format!("provider-{id}"))
            .bind(status)
            .bind(response)
            .execute(&pool)
            .await
            .expect("approval");
            id
        }
    };
    let pending = approval("pending", None).await;
    let resolved = approval("resolved", Some(json!({"decision": "approve"}))).await;
    let canceled = approval("canceled", None).await;
    let question = |answered: bool| {
        let pool = pool.clone();
        let seed = &seed;
        async move {
            let id = Uuid::now_v7();
            sqlx::query(
                r#"
                INSERT INTO person_questions (
                    id, project_id, asked_by_actor_id, person_actor_id, question, options,
                    conversation_id, status, answer_option_id, answer_text, answered_at
                )
                VALUES ($1, $2, $3, $4, 'Ship today?', '[{"id": "1", "label": "Yes"}]', $5,
                        CASE WHEN $6 THEN 'answered' ELSE 'pending' END,
                        CASE WHEN $6 THEN '1' END, CASE WHEN $6 THEN 'After lunch' END,
                        CASE WHEN $6 THEN now() END)
                "#,
            )
            .bind(id)
            .bind(seed.project_id)
            .bind(seed.worker_actor_id)
            .bind(seed.human_actor_id)
            .bind(seed.thread_id)
            .bind(answered)
            .execute(&pool)
            .await
            .expect("question");
            id
        }
    };
    let asked = question(false).await;
    let answered = question(true).await;
    // The longest answer the old table takes: 4,000 characters, each of which
    // JSON writes as six bytes. Its copy must fit, or the upgrade fails.
    let longest_text = "\u{1}".repeat(4_000);
    let longest = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO person_questions (
            id, project_id, asked_by_actor_id, person_actor_id, question, options,
            conversation_id, status, answer_text, answered_at
        )
        VALUES ($1, $2, $3, $4, 'Ship today?', '[{"id": "1", "label": "Yes"}]', $5,
                'answered', $6, now())
        "#,
    )
    .bind(longest)
    .bind(seed.project_id)
    .bind(seed.worker_actor_id)
    .bind(seed.human_actor_id)
    .bind(seed.thread_id)
    .bind(&longest_text)
    .execute(&pool)
    .await
    .expect("the longest answer");

    full.run(&pool)
        .await
        .expect("migrate to the requests record");

    let approval_copy = |status: &str, answer: Option<Value>, by: Option<Uuid>| Copied {
        kind: "approval".to_owned(),
        raised_by_account_id: seed.human_actor_id,
        run_id: Some(run),
        space_id: Some(seed.project_id),
        conversation_id: Some(seed.thread_id),
        title: "Run the migration?".to_owned(),
        options: json!([{"id": "approve", "label": "Allow"}, {"id": "deny", "label": "Stop"}]),
        allow_text: false,
        answer_permission: "runs.approve".to_owned(),
        addressed_account_ids: Vec::new(),
        addressed_role_keys: vec!["owner".to_owned()],
        status: status.to_owned(),
        answer,
        answered_by_account_id: by,
        legacy_source: Some("agent_approval".to_owned()),
    };
    assert_eq!(
        copied(&pool, pending).await,
        approval_copy("open", None, None)
    );
    assert_eq!(
        copied(&pool, resolved).await,
        approval_copy(
            "answered",
            Some(json!({"decision": "approve"})),
            Some(seed.human_actor_id)
        )
    );
    assert_eq!(
        copied(&pool, canceled).await,
        approval_copy("cancelled", None, None)
    );
    let question_copy = |status: &str, answer: Option<Value>, by: Option<Uuid>| Copied {
        kind: "question".to_owned(),
        raised_by_account_id: seed.worker_actor_id,
        run_id: None,
        space_id: Some(seed.project_id),
        conversation_id: Some(seed.thread_id),
        title: "Ship today?".to_owned(),
        options: json!([{"id": "1", "label": "Yes"}]),
        allow_text: true,
        answer_permission: "requests.answer".to_owned(),
        addressed_account_ids: vec![seed.human_actor_id],
        addressed_role_keys: Vec::new(),
        status: status.to_owned(),
        answer,
        answered_by_account_id: by,
        legacy_source: Some("person_question".to_owned()),
    };
    assert_eq!(
        copied(&pool, asked).await,
        question_copy("open", None, None)
    );
    assert_eq!(
        copied(&pool, answered).await,
        question_copy(
            "answered",
            Some(json!({"optionId": "1", "text": "After lunch"})),
            Some(seed.human_actor_id)
        )
    );
    assert_eq!(
        copied(&pool, longest).await.answer,
        Some(json!({"text": longest_text})),
        "the longest old answer is copied whole"
    );
    let total = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM requests")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(total, 6, "each old row is copied once");
}

#[tokio::test]
async fn a_question_asked_and_answered_the_old_way_is_on_both_records() {
    let Some(setup) = setup("a1_requests_dual").await else {
        return;
    };
    let app = &setup.app;
    let project = create_project(&setup, "ASK", "Asking").await;
    let (_mara, mara_browser) = add_person(&setup, &[&project], "mara").await;
    let (scout, scout_token) = create_agent(app, &setup.browser, &project, "Scout", "scout").await;
    add_member(&setup, &project, scout, "member").await;
    let scout_session = initialize(app, &scout_token).await.expect("Scout MCP");
    let asked = tool(
        app,
        &scout_token,
        &scout_session,
        "ask_person",
        json!({"projectId": project, "person": "@mara", "question": "Ship it?",
               "options": ["Yes", "No"]}),
    )
    .await;
    assert_eq!(asked["outcome"], "ok", "{asked}");
    let question = asked["value"]["questionId"]
        .as_str()
        .expect("question")
        .parse::<Uuid>()
        .expect("question UUID");
    let record = copied(&setup.pool, question).await;
    assert_eq!(record.status, "open");
    assert_eq!(record.raised_by_account_id, scout);
    assert_eq!(record.legacy_source.as_deref(), Some("person_question"));

    let mara_app = connect_app(app, &mara_browser, "Mara's app", "full").await;
    let mara_session = initialize(app, &mara_app).await.expect("Mara MCP");
    let answered = tool(
        app,
        &mara_app,
        &mara_session,
        "question_answer",
        json!({"questionId": question, "optionId": "1", "text": "Go"}),
    )
    .await;
    assert_eq!(answered["outcome"], "ok", "{answered}");
    let record = copied(&setup.pool, question).await;
    assert_eq!(record.status, "answered");
    assert_eq!(record.answer, Some(json!({"optionId": "1", "text": "Go"})));
    let old = sqlx::query_scalar::<_, String>("SELECT status FROM person_questions WHERE id = $1")
        .bind(question)
        .fetch_one(&setup.pool)
        .await
        .expect("old row");
    assert_eq!(old, "answered");
}
