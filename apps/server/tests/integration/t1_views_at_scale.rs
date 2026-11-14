//! T1.7: the views engine over 10,000 tasks. Every built-in field a task
//! has sorts and every one that groups pages through the Tasks collection
//! without repeats, through the one query compiler (`views::query::page`
//! over `TasksSource`, each record joined to its task); filters on the
//! task's own columns find what the task table holds; and the first page of
//! each list is planned on index paths, under a pinned cost.

use std::{collections::HashSet, time::Instant};

use agent_workspace_server::{
    collections::tasks::{self, TasksSource},
    fields::{FieldClause, FieldOp},
    views::{
        definition::{Direction, SortRule, ViewConfig},
        query::{self, Spec},
    },
};
use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    agent_interface::send,
    mcp_admin::{create_project, setup},
};

/// The most a first page may cost, as PostgreSQL estimates it, pinned from
/// the plans at 10,000 tasks with room to spare. A sort by a column costs
/// about 2,000; one by a value looked up per task (a person's name, a
/// label, whether a blocker is open) about 10 more per task, up to 270,000
/// for who holds the claim. A lookup that reads a whole table per task
/// costs millions (the parent sort did, comparing IDs as text).
const COST_CEILING: f64 = 400_000.0;

/// Collects every node of a plan.
fn nodes<'a>(plan: &'a Value, found: &mut Vec<&'a Value>) {
    found.push(plan);
    for child in plan["Plans"].as_array().into_iter().flatten() {
        nodes(child, found);
    }
}

fn seq_scans(plan: &Value) -> Vec<String> {
    let mut found = Vec::new();
    nodes(plan, &mut found);
    found
        .into_iter()
        .filter(|node| node["Node Type"] == "Seq Scan")
        .filter_map(|node| node["Relation Name"].as_str().map(str::to_owned))
        .filter(|relation| relation == "work_items" || relation == "collection_records")
        .collect()
}

#[tokio::test]
async fn ten_thousand_tasks_page_by_every_field_on_index_paths() {
    let Some(setup) = setup("t1_views_at_scale").await else {
        return;
    };
    let project = create_project(&setup, "SCALE", "Scale").await;
    let project_id = Uuid::parse_str(&project).expect("project");
    let (status, seed) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/work-items"),
            Some("seed-task"),
            json!({"title": "Seed task"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{seed}");
    let seed_id = Uuid::parse_str(seed["id"].as_str().expect("seed id")).expect("seed id");
    let (status, label) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/projects/{project}/labels"),
            Some("label-ops"),
            json!({"name": "Ops"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{label}");
    let label_id = Uuid::parse_str(label["id"].as_str().expect("label id")).expect("label");
    let pool = &setup.pool;
    let milestone = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO project_milestones (id, project_id, name, created_by_actor_id) \
         SELECT $1, project_id, 'Launch', created_by_actor_id FROM work_items WHERE id = $2",
    )
    .bind(milestone)
    .bind(seed_id)
    .execute(pool)
    .await
    .expect("milestone");

    // 10,000 more tasks straight into the tables, like a long-lived project.
    let started = Instant::now();
    // A task's record comes first (T1.3), in the same transaction.
    let mut transaction = pool.begin().await.expect("begin");
    sqlx::query(
        r#"
        INSERT INTO collection_records (id, project_id, collection_id, number, title,
                                        created_by_actor_id, updated_by_actor_id,
                                        created_at, updated_at)
        SELECT gen_random_uuid(), seed.project_id, seed_record.collection_id, seed.number + n,
               'Task ' || n || CASE WHEN n % 97 = 0 THEN ' login bug' ELSE '' END,
               seed.created_by_actor_id, seed.created_by_actor_id,
               now() - make_interval(secs => n), now() - make_interval(secs => n)
        FROM work_items seed
        JOIN collection_records seed_record ON seed_record.id = seed.id,
             generate_series(1, 10000) n
        WHERE seed.id = $1
        "#,
    )
    .bind(seed_id)
    .execute(&mut *transaction)
    .await
    .expect("insert 10,000 records");
    sqlx::query(
        r#"
        INSERT INTO work_items (id, project_id, number, description, type_id, status_id,
                                priority, assignee_actor_id, created_by_actor_id, board_rank,
                                due_date, start_date, estimate, milestone_id, created_at, updated_at)
        SELECT record.id, seed.project_id, record.number,
               '', seed.type_id,
               (SELECT id FROM work_item_statuses s WHERE s.project_id = seed.project_id
                 ORDER BY s.position OFFSET (n % 7) LIMIT 1),
               (ARRAY['no_priority', 'low', 'medium', 'high', 'urgent'])[1 + n % 5],
               CASE WHEN n % 3 = 0 THEN seed.created_by_actor_id END,
               seed.created_by_actor_id, 1000 + n,
               CASE WHEN n % 4 = 0 THEN DATE '2026-10-01' + (n % 90) END,
               CASE WHEN n % 6 = 0 THEN DATE '2026-09-01' + (n % 60) END,
               CASE WHEN n % 5 = 0 THEN (n % 13)::double precision END,
               CASE WHEN n % 8 = 0 THEN $2::uuid END,
               record.created_at, record.updated_at
        FROM work_items seed
        JOIN collection_records seed_record ON seed_record.id = seed.id
        JOIN collection_records record
          ON record.collection_id = seed_record.collection_id AND record.number > seed.number
        CROSS JOIN LATERAL (SELECT (record.number - seed.number)::integer AS n) step
        WHERE seed.id = $1
        "#,
    )
    .bind(seed_id)
    .bind(milestone)
    .execute(&mut *transaction)
    .await
    .expect("insert 10,000 tasks");
    transaction.commit().await.expect("commit 10,000 tasks");
    sqlx::query(
        "UPDATE project_counters SET next_work_item_number = next_work_item_number + 10000 \
         WHERE project_id = $1",
    )
    .bind(project_id)
    .execute(pool)
    .await
    .expect("counter");
    sqlx::query(
        "INSERT INTO work_item_labels (project_id, work_item_id, label_id, added_by_actor_id) \
         SELECT item.project_id, item.id, $2, item.created_by_actor_id \
         FROM task_rows item WHERE item.project_id = $1 AND item.number % 5 = 0",
    )
    .bind(project_id)
    .bind(label_id)
    .execute(pool)
    .await
    .expect("label every fifth task");
    for table in ["work_items", "collection_records", "work_item_labels"] {
        sqlx::query(&format!("ANALYZE {table}"))
            .execute(pool)
            .await
            .expect("analyze");
    }
    eprintln!("seeded 10,000 tasks in {:?}", started.elapsed());

    let collection_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM collections WHERE project_id = $1 AND builtin = 'tasks'",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await
    .expect("tasks collection");
    let reader =
        sqlx::query_scalar::<_, Uuid>("SELECT created_by_actor_id FROM work_items WHERE id = $1")
            .bind(seed_id)
            .fetch_one(pool)
            .await
            .expect("reader");
    let source = TasksSource {
        project_id,
        collection_id,
    };
    let mut connection = pool.acquire().await.expect("connection");
    let definitions = tasks::system_fields(&mut connection, project_id)
        .await
        .expect("fields");
    assert!(definitions.iter().any(|field| field.id == tasks::KEY));

    // Every sort, by each built-in field and the record's own keys, and
    // every grouping.
    let mut cases = Vec::new();
    for key in ["title", "number", "updated", "rank"] {
        cases.push((
            format!("sort {key}"),
            ViewConfig {
                sort: vec![SortRule {
                    field: key.to_owned(),
                    direction: Direction::Desc,
                }],
                ..ViewConfig::default()
            },
        ));
    }
    for field in &definitions {
        cases.push((
            format!("sort {}", field.name),
            ViewConfig {
                sort: vec![SortRule {
                    field: field.id.to_string(),
                    direction: Direction::Asc,
                }],
                ..ViewConfig::default()
            },
        ));
        if field.can_group() {
            cases.push((
                format!("group {}", field.name),
                ViewConfig {
                    group_by: Some(field.id),
                    sort: vec![SortRule {
                        field: tasks::DUE_DATE.to_string(),
                        direction: Direction::Asc,
                    }],
                    ..ViewConfig::default()
                },
            ));
        }
    }
    let mut slowest = (String::new(), std::time::Duration::ZERO);
    let mut costly = Vec::new();
    for (name, config) in &cases {
        agent_workspace_server::views::definition::check(config, &definitions)
            .unwrap_or_else(|problem| panic!("{name}: {problem}"));
        let spec = Spec::from_config(config, &definitions, reader);
        let mut cursor: Option<String> = None;
        let mut seen = HashSet::new();
        let mut case_time = std::time::Duration::ZERO;
        for _ in 0..3 {
            let started = Instant::now();
            let page = query::page(&mut *connection, &source, &spec, cursor.as_deref(), 200)
                .await
                .expect("page")
                .unwrap_or_else(|()| panic!("{name}: cursor refused"));
            let took = started.elapsed();
            case_time = case_time.max(took);
            if took > slowest.1 {
                slowest = (name.clone(), took);
            }
            for (id, group) in &page.rows {
                let group = group
                    .as_ref()
                    .map(|group| group.key.clone())
                    .unwrap_or_default();
                assert!(seen.insert((group, *id)), "{name} repeated a row");
            }
            cursor = page.next_cursor;
        }
        assert_eq!(seen.len(), 600, "{name}");
        // One project holds every task here, so reading the whole table is
        // the cheapest plan the planner could pick; with sequential scans
        // priced out it shows whether an index path exists at all (a scan
        // with no other way in still appears, and costs over 10^10).
        let mut transaction = pool.begin().await.expect("transaction");
        sqlx::query("SET LOCAL enable_seqscan = off")
            .execute(&mut *transaction)
            .await
            .expect("price out scans");
        let plan = query::explain_first_page(&mut *transaction, &source, &spec, 50)
            .await
            .expect("explain");
        transaction.rollback().await.expect("rollback");
        let top = &plan[0]["Plan"];
        let cost = top["Total Cost"].as_f64().expect("cost");
        let scans = seq_scans(top);
        eprintln!("cost {cost:>10.0}  {case_time:>12?}  {name}");
        // Grouped by whether a task is blocked, a page once took two
        // seconds, all of it compiling the query (JIT); the list queries
        // turn that off.
        if name.as_str() == "group Blocked" {
            assert!(
                case_time < std::time::Duration::from_millis(1000),
                "{name}: a page took {case_time:?}"
            );
        }
        if cost >= COST_CEILING || !scans.is_empty() {
            costly.push(format!(
                "{name}: first page costs {cost}, reads every row of {scans:?}"
            ));
        }
    }
    eprintln!("slowest page: {} in {:?}", slowest.0, slowest.1);
    assert!(costly.is_empty(), "{costly:#?}");

    // The lists that read the task's own columns start from an index.
    let first_status = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM work_item_statuses WHERE project_id = $1 ORDER BY position LIMIT 1",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await
    .expect("status");
    let filtered = |field: Uuid, op: FieldOp, values: &[&str]| ViewConfig {
        filters: vec![FieldClause {
            field,
            op,
            values: values.iter().map(|value| (*value).to_owned()).collect(),
        }]
        .into(),
        ..ViewConfig::default()
    };
    let status_text = first_status.to_string();
    for (name, config, expected) in [
        (
            "newest first",
            ViewConfig {
                sort: vec![SortRule {
                    field: "number".to_owned(),
                    direction: Direction::Desc,
                }],
                ..ViewConfig::default()
            },
            None,
        ),
        (
            "one status",
            filtered(tasks::STATUS, FieldOp::Is, &[&status_text]),
            Some(format!(
                "SELECT count(*) FROM task_rows WHERE project_id = '{project_id}' AND status_id = '{first_status}' AND archived_at IS NULL"
            )),
        ),
        (
            "my tasks",
            filtered(tasks::ASSIGNEE, FieldOp::Is, &["me"]),
            Some(format!(
                "SELECT count(*) FROM task_rows WHERE project_id = '{project_id}' AND assignee_actor_id = '{reader}' AND archived_at IS NULL"
            )),
        ),
        (
            "due in October",
            ViewConfig {
                filters: vec![
                    FieldClause {
                        field: tasks::DUE_DATE,
                        op: FieldOp::Gte,
                        values: vec!["2026-10-01".to_owned()],
                    },
                    FieldClause {
                        field: tasks::DUE_DATE,
                        op: FieldOp::Lt,
                        values: vec!["2026-11-01".to_owned()],
                    },
                ]
                .into(),
                ..ViewConfig::default()
            },
            Some(format!(
                "SELECT count(*) FROM task_rows WHERE project_id = '{project_id}' AND due_date >= DATE '2026-10-01' AND due_date < DATE '2026-11-01' AND archived_at IS NULL"
            )),
        ),
        (
            "labelled Ops",
            filtered(tasks::LABELS, FieldOp::Is, &[&label_id.to_string()]),
            Some("SELECT 2000::bigint".to_owned()),
        ),
        (
            "started",
            filtered(tasks::STATUS, FieldOp::CategoryIs, &["started"]),
            Some(format!(
                "SELECT count(*) FROM task_rows item JOIN work_item_statuses status ON status.id = item.status_id WHERE item.project_id = '{project_id}' AND status.semantic_category = 'started' AND item.archived_at IS NULL"
            )),
        ),
    ] {
        agent_workspace_server::views::definition::check(&config, &definitions)
            .unwrap_or_else(|problem| panic!("{name}: {problem}"));
        let spec = Spec::from_config(&config, &definitions, reader);
        if let Some(expected) = expected {
            let expected = sqlx::query_scalar::<_, i64>(&expected)
                .fetch_one(pool)
                .await
                .expect("expected count");
            let total = query::total(&mut *connection, &source, &spec)
                .await
                .expect("total");
            assert_eq!(total, expected, "{name}");
            assert!(total > 0, "{name}");
        }
        let plan = query::explain_first_page(&mut *connection, &source, &spec, 50)
            .await
            .expect("explain");
        let scans = seq_scans(&plan[0]["Plan"]);
        assert!(
            scans.is_empty(),
            "{name} reads every row of {scans:?}\n{plan:#}"
        );
    }

    // A record in two groups counts once in the total; each group counts
    // its own.
    let by_label = Spec::from_config(
        &ViewConfig {
            group_by: Some(tasks::LABELS),
            ..ViewConfig::default()
        },
        &definitions,
        reader,
    );
    assert_eq!(
        query::total(&mut *connection, &source, &by_label)
            .await
            .expect("total"),
        10_001
    );
    let groups = query::group_counts(&mut *connection, &source, &by_label)
        .await
        .expect("groups");
    assert_eq!(groups[0].label, "Ops");
    assert_eq!(groups[0].count, 2000);
}
