//! T1.34 and T1.35: outside data. A space connects a database kept
//! elsewhere, saves read-only queries over it, and shares a reviewed
//! query's stored answer. The silent, costly failures: a query that writes
//! to the outside database, a password that comes back out, an answer past
//! its row cap kept whole, someone without data.connect running a draft or
//! reading its SQL, and a draft run showing on a shared dashboard.

use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

use super::support::{
    callers::{Call, Caller},
    run_due_jobs,
    world::WorldBuilder,
};

fn tool(name: &'static str, arguments: Value) -> Call {
    Call::Tool { name, arguments }
}

fn ok(answer: &Value) -> &Value {
    assert_eq!(answer["outcome"], "ok", "{answer}");
    &answer["value"]
}

#[tokio::test]
async fn reviewed_shared_answers_reach_viewers_and_writes_never_reach_the_database() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .connected_app("ada")
        .agent("mo", "member")
        .agent("vera", "viewer")
        .config("AGENT_WORKSPACE_ALLOW_PRIVATE_ADDRESSES", "true")
        .build("t1_outside_data")
        .await
    else {
        return;
    };
    let app = &world.app;
    let work = world.project("WORK").to_owned();
    let (ada, mo, vera) = (
        &world.apps["ada"],
        &world.agents["mo"],
        &world.agents["vera"],
    );

    // The outside database: a table of orders and a sequence, in the test
    // server's base database.
    let base = std::env::var("AGENT_WORKSPACE_DATABASE_URL").expect("database url");
    let outside = PgPoolOptions::new()
        .max_connections(1)
        .connect(&base)
        .await
        .expect("outside database");
    let table = format!("outside_orders_{}", Uuid::now_v7().simple());
    sqlx::query(&format!(
        "CREATE TABLE {table} (team TEXT NOT NULL, amount INTEGER NOT NULL)"
    ))
    .execute(&outside)
    .await
    .expect("table");
    sqlx::query(&format!(
        "INSERT INTO {table} VALUES ('alpha', 10), ('alpha', 20), ('beta', 5), ('gamma', 1)"
    ))
    .execute(&outside)
    .await
    .expect("rows");
    sqlx::query(&format!("CREATE SEQUENCE {table}_seq"))
        .execute(&outside)
        .await
        .expect("sequence");
    let mut address = url::Url::parse(&base).expect("base url");
    address.set_query(Some("sslmode=disable"));
    // As it would appear: the username and the password, colon between.
    let password = format!(
        "{}:{}",
        address.username(),
        address.password().unwrap_or("")
    );

    // Connecting needs data.connect; the address never comes back out.
    let (_, refused) = mo
        .call(
            app,
            tool(
                "data_connection_create",
                json!({"projectId": work, "name": "Warehouse", "address": address.as_str()}),
            ),
        )
        .await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    let (_, made) = ada
        .call(
            app,
            tool(
                "data_connection_create",
                json!({"projectId": work, "name": "Warehouse", "address": address.as_str()}),
            ),
        )
        .await;
    let connection = ok(&made)["id"].as_str().expect("id").to_owned();
    {
        assert!(!made.to_string().contains(&password), "{made}");
        let (_, listed) = ada
            .call(
                app,
                tool("data_connection_list", json!({"projectId": work})),
            )
            .await;
        assert!(!ok(&listed).to_string().contains(&password), "{listed}");
        let stored: String =
            sqlx::query_scalar("SELECT address FROM data_connections WHERE id = $1::uuid")
                .bind(&connection)
                .fetch_one(world.pool())
                .await
                .expect("address");
        assert!(!stored.contains(&password), "{stored}");
    }
    let (_, hidden) = vera
        .call(
            app,
            tool("data_connection_list", json!({"projectId": work})),
        )
        .await;
    assert_eq!(hidden["outcome"], "permission_denied", "{hidden}");

    let save = |name: &str, statement: String, extra: Value| {
        let mut arguments = json!({
            "projectId": work, "connection": connection, "name": name, "statement": statement,
        });
        for (key, value) in extra.as_object().expect("extra") {
            arguments[key] = value.clone();
        }
        tool("data_query_create", arguments)
    };
    let get = |query: &str| tool("data_query_get", json!({"query": query}));
    let run = |query: &str| tool("data_query_run", json!({"query": query}));

    // A query over its row cap keeps the cap and says so.
    let (_, all) = ada
        .call(
            app,
            save(
                "All orders",
                format!("SELECT team, amount FROM {table} ORDER BY amount DESC;"),
                json!({"rowCap": 2}),
            ),
        )
        .await;
    let all = ok(&all)["id"].as_str().expect("id").to_owned();
    let (_, started) = ada.call(app, run(&all)).await;
    assert_eq!(ok(&started)["answer"]["running"], true, "{started}");
    run_due_jobs(&world.state).await;
    let (_, read) = ada.call(app, get(&all)).await;
    let answer = &ok(&read)["answer"];
    assert_eq!(answer["columns"], json!(["team", "amount"]), "{read}");
    assert_eq!(
        answer["rows"],
        json!([["alpha", 20], ["alpha", 10]]),
        "{read}"
    );
    assert_eq!(answer["truncated"], true, "{read}");
    assert_eq!(answer["running"], false, "{read}");

    // A query that writes is refused by the read-only transaction, and the
    // database is untouched.
    for (name, statement) in [
        (
            "Sneaky sequence",
            format!("SELECT nextval('{table}_seq') AS next"),
        ),
        (
            "Sneaky subquery",
            format!("SELECT team FROM {table} WHERE nextval('{table}_seq') > 0"),
        ),
    ] {
        let (_, sneaky) = ada.call(app, save(name, statement, json!({}))).await;
        let sneaky = ok(&sneaky)["id"].as_str().expect("id").to_owned();
        ada.call(app, run(&sneaky)).await;
        run_due_jobs(&world.state).await;
        let (_, read) = ada.call(app, get(&sneaky)).await;
        let error = ok(&read)["answer"]["error"].as_str().unwrap_or_default();
        assert!(error.contains("read-only"), "{read}");
    }
    let (rows, called): (i64, bool) = sqlx::query_as(&format!(
        "SELECT (SELECT count(*) FROM {table}), is_called FROM {table}_seq"
    ))
    .fetch_one(&outside)
    .await
    .expect("untouched");
    assert_eq!((rows, called), (4, false));
    let (_, modifying) = ada
        .call(
            app,
            save(
                "Sneaky delete",
                format!("WITH gone AS (DELETE FROM {table} RETURNING team) SELECT * FROM gone"),
                json!({}),
            ),
        )
        .await;
    let modifying = ok(&modifying)["id"].as_str().expect("id").to_owned();
    ada.call(app, run(&modifying)).await;
    run_due_jobs(&world.state).await;
    let (_, read) = ada.call(app, get(&modifying)).await;
    assert!(ok(&read)["answer"]["error"].is_string(), "{read}");
    let rows: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(&outside)
        .await
        .expect("count");
    assert_eq!(rows, 4);

    // A parameterised query, reviewed and shared.
    let (_, totals) = ada
        .call(
            app,
            save(
                "Totals",
                format!(
                    "SELECT team, sum(amount)::int AS total FROM {table} \
                     WHERE amount >= {{{{least}}}} GROUP BY team ORDER BY team"
                ),
                json!({"parameters": [{"name": "least", "type": "number", "default": 2}]}),
            ),
        )
        .await;
    let totals_entry = ok(&totals);
    let totals = totals_entry["id"].as_str().expect("id").to_owned();
    assert_eq!(totals_entry["review"], "draft", "{totals_entry}");

    // Unshared, a viewer sees nothing of it and can't run it.
    let (_, listed) = vera
        .call(app, tool("data_query_list", json!({"projectId": work})))
        .await;
    assert_eq!(ok(&listed)["queries"], json!([]), "{listed}");
    let (_, refused) = vera.call(app, run(&totals)).await;
    assert_eq!(refused["outcome"], "not_found", "{refused}");

    // Sharing waits for a review.
    let share = || tool("data_query_share", json!({"query": totals, "shared": true}));
    let (_, early) = ada.call(app, share()).await;
    assert_eq!(early["outcome"], "conflict", "{early}");
    let (_, reviewed) = ada
        .call(
            app,
            tool(
                "data_query_review",
                json!({"query": totals, "state": "reviewed",
                       "expectedVersion": totals_entry["version"]}),
            ),
        )
        .await;
    assert_eq!(ok(&reviewed)["review"], "reviewed", "{reviewed}");
    let (_, shared) = ada.call(app, share()).await;
    assert_eq!(ok(&shared)["shared"], true, "{shared}");

    // A viewer runs it with its defaults (never values of their own) and
    // sees the answer without its SQL or its connection.
    let (_, own_values) = vera
        .call(
            app,
            tool(
                "data_query_run",
                json!({"query": totals, "parameters": {"least": 0}}),
            ),
        )
        .await;
    assert_eq!(own_values["outcome"], "invalid_request", "{own_values}");
    let (_, ran) = vera.call(app, run(&totals)).await;
    ok(&ran);
    run_due_jobs(&world.state).await;
    let (_, seen) = vera.call(app, get(&totals)).await;
    let seen = ok(&seen);
    assert!(seen.get("statement").is_none(), "{seen}");
    assert!(seen.get("connection").is_none(), "{seen}");
    assert_eq!(seen["answer"]["reviewed"], true, "{seen}");
    assert_eq!(
        seen["answer"]["rows"],
        json!([["alpha", 30], ["beta", 5]]),
        "{seen}"
    );
    // Someone who connects data trying a value of their own sees that
    // answer; everyone else keeps the reviewed answer for the defaults.
    let (_, tried) = ada
        .call(
            app,
            tool(
                "data_query_run",
                json!({"query": totals, "parameters": {"least": 100}}),
            ),
        )
        .await;
    ok(&tried);
    run_due_jobs(&world.state).await;
    let (_, mine) = ada.call(app, get(&totals)).await;
    assert_eq!(ok(&mine)["answer"]["rows"], json!([]), "{mine}");
    let (_, still) = vera.call(app, get(&totals)).await;
    assert_eq!(
        ok(&still)["answer"]["rows"],
        json!([["alpha", 30], ["beta", 5]]),
        "{still}"
    );

    // On a shared dashboard: the shared query shows; an unshared one may
    // not be put there.
    let (_, unshared_tile) = ada
        .call(
            app,
            tool(
                "dashboard_create",
                json!({"projectId": work, "name": "Not yet",
                       "tiles": [{"query": all}]}),
            ),
        )
        .await;
    assert_eq!(
        unshared_tile["outcome"], "invalid_request",
        "{unshared_tile}"
    );
    let (_, board) = ada
        .call(
            app,
            tool(
                "dashboard_create",
                json!({"projectId": work, "name": "Sales",
                       "tiles": [{"query": totals, "style": "bar"}, {"query": totals}]}),
            ),
        )
        .await;
    let board = ok(&board)["id"].as_str().expect("id").to_owned();
    let read_board = || tool("dashboard_get", json!({"dashboard": board}));
    let (_, shown) = vera.call(app, read_board()).await;
    let tiles = &ok(&shown)["tiles"];
    let groups = tiles[0]["figures"]["groups"]
        .as_array()
        .unwrap_or_else(|| panic!("{shown}"))
        .iter()
        .map(|group| (group["name"].clone(), group["value"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        groups,
        vec![(json!("alpha"), json!(30.0)), (json!("beta"), json!(5.0))],
        "{shown}"
    );
    assert_eq!(
        tiles[1]["figures"]["columns"],
        json!(["team", "total"]),
        "{shown}"
    );

    // An edit makes it a draft: a viewer can't run it, and a draft run by
    // its author never shows on the shared dashboard.
    let (_, edited) = ada
        .call(
            app,
            tool(
                "data_query_update",
                json!({"query": totals, "statement": format!(
                    "SELECT team, (sum(amount) * 100)::int AS total FROM {table} \
                     WHERE amount >= {{{{least}}}} GROUP BY team ORDER BY team"
                )}),
            ),
        )
        .await;
    assert_eq!(ok(&edited)["review"], "draft", "{edited}");
    let (_, refused) = vera.call(app, run(&totals)).await;
    assert_eq!(refused["outcome"], "permission_denied", "{refused}");
    ada.call(app, run(&totals)).await;
    run_due_jobs(&world.state).await;
    let (_, mine) = ada.call(app, get(&totals)).await;
    assert_eq!(ok(&mine)["answer"]["reviewed"], false, "{mine}");
    assert_eq!(
        ok(&mine)["answer"]["rows"],
        json!([["alpha", 3000], ["beta", 500]]),
        "{mine}"
    );
    let (_, still) = vera.call(app, read_board()).await;
    assert_eq!(
        ok(&still)["tiles"][1]["figures"]["rows"],
        json!([["alpha", 30], ["beta", 5]]),
        "{still}"
    );

    sqlx::raw_sql(&format!("DROP TABLE {table}; DROP SEQUENCE {table}_seq"))
        .execute(&outside)
        .await
        .ok();
    world.finish().await;
}
