//! T1.28: forms as their own definition. The silent, costly failures: a
//! hidden question that still blocks an answer (or whose answer is kept);
//! a form's set values not landing; an internal form a viewer can answer;
//! an archived form still taking answers; and a retry making two records.

use serde_json::{Value, json};

use super::support::{
    callers::{Call, Caller},
    world::WorldBuilder,
};

#[tokio::test]
async fn an_internal_form_asks_by_condition_and_sets_its_values() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter("agent_workspace=error")
        .try_init();
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("vic", "viewer")
        .agent("rook", "member")
        .connected_app("laptop")
        .build("t1_forms")
        .await
    else {
        return;
    };
    let app = &world.app;
    let pool = world.pool();
    let laptop = &world.apps["laptop"];
    let rook = &world.agents["rook"];
    let lena = &world.people["lena"];
    let vic = &world.people["vic"];
    let tool = |name: &'static str, arguments: Value| Call::Tool { name, arguments };
    let op = |name: &str, body: Value| Call::Http {
        method: "POST",
        path: format!("/api/v2/ops/{name}"),
        body,
    };

    let (_, teams) = laptop
        .call(
            app,
            tool(
                "collection_write",
                json!({"projectId": "WORK", "name": "Teams"}),
            ),
        )
        .await;
    assert_eq!(teams["outcome"], "ok", "{teams}");
    let teams = teams["value"]["collection"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{teams}"))
        .to_owned();
    let (_, made) = laptop
        .call(
            app,
            tool(
                "collection_write",
                json!({
                    "projectId": "WORK",
                    "name": "Requests",
                    "fields": [
                        {"name": "Kind", "type": "single_choice", "options": [{"name": "Bug"}, {"name": "Idea"}]},
                        {"name": "Steps", "type": "text"},
                        {"name": "Owner", "type": "person"},
                        {"name": "Area", "type": "text"},
                        {"name": "Team", "type": "relation", "target": "record:Teams"},
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

    // A condition reads an earlier question only.
    let backwards = json!({
        "collection": collection,
        "name": "Backwards",
        "definition": {"fields": [
            {"field": "Steps", "showIf": {"field": "Kind", "isOneOf": ["Bug"]}},
            {"field": "Kind"},
        ]},
        "clientRequestId": "form-backwards",
    });
    let (_, refused) = laptop.call(app, tool("form_create", backwards)).await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");

    // An internal form may ask for a person; Steps shows for bugs only, and
    // every answer is in the Intake area.
    let (_, form) = laptop
        .call(
            app,
            tool(
                "form_create",
                json!({
                    "collection": collection,
                    "name": "Ask the team",
                    "definition": {
                        "intro": "Something you need from us.",
                        "fields": [
                            {"field": "Kind", "required": true},
                            {"field": "Steps", "required": true,
                             "showIf": {"field": "Kind", "isOneOf": ["Bug"]}},
                            {"field": "Owner"},
                            {"field": "Team"},
                        ],
                        "sections": [{"title": "Who", "before": "Owner"}],
                        "sets": [{"field": "Area", "value": "Intake"}],
                    },
                    "clientRequestId": "form-team",
                }),
            ),
        )
        .await;
    assert_eq!(form["outcome"], "ok", "{form}");
    let form_id = form["value"]["id"].as_str().expect("id").to_owned();
    assert_eq!(form["value"]["audience"], "internal", "{form}");
    let questions = form["value"]["questions"].as_array().expect("questions");
    assert_eq!(questions.len(), 4, "{form}");
    assert_eq!(questions[2]["section"]["title"], "Who", "{form}");
    // A relation question says which collection it picks from.
    assert_eq!(questions[3]["type"], "relation", "{form}");
    assert_eq!(questions[3]["collection"], teams.as_str(), "{form}");
    assert!(questions[1]["showIf"].is_object(), "{form}");

    // An idea: the hidden Steps doesn't block, and its answer isn't kept.
    let idea = json!({
        "form": form_id,
        "answers": {"title": "Dark mode", "Kind": "Idea", "Steps": "ignored",
                    "Owner": lena.actor.to_string()},
        "clientRequestId": "answer-idea",
    });
    let (_, answered) = rook.call(app, tool("form_answer_send", idea.clone())).await;
    assert_eq!(answered["outcome"], "ok", "{answered}");
    let record = answered["value"]["recordId"]
        .as_str()
        .expect("record")
        .to_owned();
    let (_, again) = rook.call(app, tool("form_answer_send", idea)).await;
    assert_eq!(again["value"]["recordId"], record.as_str(), "{again}");
    let (_, records) = laptop
        .call(
            app,
            tool(
                "record_query",
                json!({"projectId": "WORK", "collection": collection}),
            ),
        )
        .await;
    let rows = records["value"]["records"]
        .as_array()
        .unwrap_or_else(|| panic!("{records}"));
    assert_eq!(rows.len(), 1, "{records}");
    let text = rows[0].to_string();
    assert!(text.contains("Intake"), "{records}");
    assert!(!text.contains("ignored"), "{records}");
    let author = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT created_by_actor_id FROM collection_records WHERE id = $1::uuid",
    )
    .bind(&record)
    .fetch_one(pool)
    .await
    .expect("author");
    assert_eq!(author, rook.actor);

    // A bug needs its steps.
    let (_, missing) = rook
        .call(
            app,
            tool(
                "form_answer_send",
                json!({"form": form_id, "answers": {"title": "Crash", "Kind": "Bug"},
                       "clientRequestId": "answer-bug"}),
            ),
        )
        .await;
    assert_eq!(missing["outcome"], "invalid_request", "{missing}");
    // The app sends a choice by its option's id; the condition still reads it.
    let mut answers = serde_json::Map::new();
    answers.insert("title".into(), json!("Crash"));
    answers.insert(
        questions[0]["id"].as_str().expect("Kind").to_owned(),
        questions[0]["options"][0]["id"].clone(),
    );
    let (_, by_id) = rook
        .call(
            app,
            tool(
                "form_answer_send",
                json!({"form": form_id, "answers": answers, "clientRequestId": "answer-bug-id"}),
            ),
        )
        .await;
    assert_eq!(by_id["outcome"], "invalid_request", "{by_id}");

    // A viewer reads the form but can't answer it.
    let (_, seen) = vic
        .call(app, op("form_get", json!({"form": form_id})))
        .await;
    assert_eq!(seen["value"]["canAnswer"], false, "{seen}");
    assert_eq!(seen["value"]["canBuild"], false, "{seen}");
    let (status, _) = vic
        .call(
            app,
            op(
                "form_answer_send",
                json!({"form": form_id, "answers": {"title": "Sneaky", "Kind": "Idea"}}),
            ),
        )
        .await;
    assert_eq!(status.as_u16(), 403);
    let (status, body) = super::agent_interface::send(
        app,
        vic.browser.request(
            "POST",
            "/api/v2/ops/form_update",
            Some("vic-rename"),
            json!({"form": form_id, "name": "Mine now"}),
        ),
    )
    .await;
    assert_eq!(status.as_u16(), 403, "a viewer doesn't build forms: {body}");

    // Archived, it takes no answers; it still lists with includeArchived.
    let version = form["value"]["version"].as_i64().expect("version");
    let (_, archived) = laptop
        .call(
            app,
            tool(
                "form_update",
                json!({"form": form_id, "archived": true, "expectedVersion": version}),
            ),
        )
        .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    assert_eq!(archived["value"]["archived"], true, "{archived}");
    let (_, closed) = rook
        .call(
            app,
            tool(
                "form_answer_send",
                json!({"form": form_id, "answers": {"title": "Late", "Kind": "Idea"},
                       "clientRequestId": "answer-late"}),
            ),
        )
        .await;
    assert_eq!(closed["outcome"], "conflict", "{closed}");
    let (_, listed) = laptop
        .call(app, tool("form_list", json!({"projectId": "WORK"})))
        .await;
    assert_eq!(listed["value"]["forms"], json!([]), "{listed}");
    let (_, all) = laptop
        .call(
            app,
            tool(
                "form_list",
                json!({"projectId": "WORK", "includeArchived": true}),
            ),
        )
        .await;
    assert_eq!(all["value"]["forms"][0]["id"], form_id.as_str(), "{all}");

    world.finish().await;
}
