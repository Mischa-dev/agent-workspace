//! H1.25: the one limit check. The silent, costly failures: two asks both
//! taking the last unit of a hard limit, usage counted twice for one
//! thing, and a plan limit that never refuses.

use agent_workspace_server::{
    accounts::{NewPerson, create_person},
    error::AppError,
    metering::{self, Admission, Ask, Recorded, Scope, Who},
    workspace::lifecycle::{self, NewWorkspace},
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::support::db::db;

async fn workspace(pool: &PgPool) -> (Uuid, Uuid) {
    let mut transaction = pool.begin().await.expect("begin");
    let person = create_person(
        &mut transaction,
        NewPerson {
            login_name: "metered",
            display_name: "Metered",
            default_workspace: None,
            created_by: None,
            claimed: true,
        },
    )
    .await
    .expect("person");
    let workspace = lifecycle::create(
        &mut transaction,
        person.account_id,
        NewWorkspace {
            name: "Metered".to_owned(),
            slug: None,
            id: None,
        },
    )
    .await
    .expect("workspace");
    transaction.commit().await.expect("commit");
    (workspace, person.account_id)
}

fn email<'a>(workspace: Uuid, key: &'a str) -> Ask<'a> {
    Ask {
        record: Some(Recorded {
            source_kind: "test",
            source_id: None,
            dedupe_key: key,
        }),
        ..Ask::new(workspace, "emails", 1)
    }
}

#[tokio::test]
async fn the_last_unit_goes_once_and_a_plan_refuses_with_its_numbers() {
    let Some(test) = db("h1_25_metering").await else {
        return;
    };
    let pool = &test.pool;
    let (workspace, owner) = workspace(pool).await;
    let plan = sqlx::query_scalar::<_, String>(
        "SELECT plan_id FROM workspace_plans WHERE workspace_id = $1",
    )
    .bind(workspace)
    .fetch_one(pool)
    .await
    .expect("a new workspace has a plan");
    assert_eq!(plan, "self_hosted");

    // The workspace's own limit: 5 emails a month.
    sqlx::query(
        "INSERT INTO spend_limits (id, workspace_id, scope_kind, scope_id, meter, period, \
         hard_limit, set_by_actor_id) VALUES ($1, $2, 'workspace', $2, 'emails', 'month', 5, $3)",
    )
    .bind(Uuid::now_v7())
    .bind(workspace)
    .bind(owner)
    .execute(pool)
    .await
    .expect("limit");
    for index in 0..4 {
        let key = format!("mail-{index}");
        let mut connection = pool.acquire().await.expect("connection");
        metering::admit(&mut connection, email(workspace, &key))
            .await
            .expect("under the limit");
    }
    // The same thing again counts once.
    let mut connection = pool.acquire().await.expect("connection");
    metering::admit(&mut connection, email(workspace, "mail-0"))
        .await
        .expect("again");
    drop(connection);

    // Two asks for the last one at once: exactly one goes through.
    let ask = |key: &'static str| async move {
        let mut transaction = pool.begin().await.expect("begin");
        let admitted = metering::admit(&mut transaction, email(workspace, key)).await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        transaction.commit().await.expect("commit");
        admitted
    };
    let (first, second) = tokio::join!(ask("last-a"), ask("last-b"));
    let (allowed, refused) = match (first, second) {
        (Ok(allowed), Err(refused)) | (Err(refused), Ok(allowed)) => (allowed, refused),
        other => panic!("exactly one of two may take the last unit: {other:?}"),
    };
    assert!(matches!(allowed, Admission::Allowed), "{allowed:?}");
    // Nearing it was said once, at the fourth.
    let notices = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM limit_notices WHERE workspace_id = $1 AND meter = 'emails'",
    )
    .bind(workspace)
    .fetch_one(pool)
    .await
    .expect("notices");
    assert_eq!(notices, 2, "one notice near the limit, one at it");
    let AppError::LimitReached(refusal) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(
        (refusal.meter, refusal.limit, refusal.used),
        ("emails", 5, 5)
    );
    assert_eq!(refusal.who_can_change, Who::Admins);
    assert!(refusal.resets_at.is_some());
    let counted = sqlx::query_scalar::<_, i64>(
        "SELECT quantity FROM usage_counters WHERE workspace_id = $1 AND meter = 'emails' \
         AND period = 'month' AND scope_kind = 'workspace'",
    )
    .bind(workspace)
    .fetch_one(pool)
    .await
    .expect("counter");
    assert_eq!(counted, 5, "four, the repeat once, and one of the last two");

    // A plan's limit refuses too, saying an owner can change the plan.
    sqlx::query(
        "UPDATE workspace_plans SET plan_id = 'free', overrides = '{\"people\": 1}' \
         WHERE workspace_id = $1",
    )
    .bind(workspace)
    .execute(pool)
    .await
    .expect("plan");
    let mut connection = pool.acquire().await.expect("connection");
    let refused = metering::admit(&mut connection, Ask::new(workspace, "people", 1))
        .await
        .expect_err("the owner is the one person the plan allows");
    let AppError::LimitReached(refusal) = refused else {
        panic!("{refused:?}");
    };
    assert_eq!(
        (
            refusal.scope,
            refusal.limit,
            refusal.used,
            refusal.who_can_change
        ),
        (Scope::Workspace, 1, 1, Who::Owners)
    );
    // The report shows the smallest limit on the workspace.
    let report = metering::usage_for(&mut connection, workspace)
        .await
        .expect("report");
    assert_eq!(report.plan_id, "free");
    let people = report
        .meters
        .iter()
        .find(|meter| meter.meter == "people")
        .expect("people");
    assert_eq!((people.used, people.limit), (1, Some(1)));
    let emails = report
        .meters
        .iter()
        .find(|meter| meter.meter == "emails")
        .expect("emails");
    assert_eq!((emails.used, emails.limit), (5, Some(5)));
    drop(connection);

    test.finish().await;
}
