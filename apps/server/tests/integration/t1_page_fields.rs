//! T1.36: fields on pages, and page lists through views. A space's pages
//! are the records of its built-in Pages collection: a field added there
//! is a field on every page, and a view filters and sorts pages by it. The
//! silent, costly failures: a page missing from its list, a page's title
//! drifting from its record's, a record that can be made or archived
//! behind its page's back, and a viewer setting a page's fields.

use serde_json::{Value, json};

use super::support::{
    callers::{Call, Caller},
    world::WorldBuilder,
};

#[tokio::test]
async fn pages_have_fields_that_their_list_filters_and_sorts_by() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .agent("rook", "member")
        .agent("vera", "viewer")
        .connected_app("laptop")
        .build("t1_page_fields")
        .await
    else {
        return;
    };
    let app = &world.app;
    let laptop = &world.apps["laptop"];
    let rook = &world.agents["rook"];
    let vera = &world.agents["vera"];
    let tool = |name: &'static str, arguments: Value| Call::Tool { name, arguments };

    let mut pages = Vec::new();
    for title in ["Alpha", "Beta", "Gamma"] {
        let (_, page) = laptop
            .call(
                app,
                tool(
                    "page_write",
                    json!({"projectId": "WORK", "title": title, "body": "Notes."}),
                ),
            )
            .await;
        assert_eq!(page["outcome"], "ok", "{page}");
        pages.push(page["value"]["id"].as_str().expect("page").to_owned());
    }

    // Each page is a record of Pages, with the page's ID and title.
    let query = |arguments: Value| tool("record_query", arguments);
    let (_, listed) = rook
        .call(
            app,
            query(json!({"projectId": "WORK", "collection": "Pages"})),
        )
        .await;
    assert_eq!(listed["outcome"], "ok", "{listed}");
    let ids = |answer: &Value| -> Vec<String> {
        answer["value"]["records"]
            .as_array()
            .unwrap_or_else(|| panic!("{answer}"))
            .iter()
            .map(|record| record["id"].as_str().expect("id").to_owned())
            .collect()
    };
    assert_eq!(ids(&listed), pages, "{listed}");

    // A field on pages, set on two of them.
    let (_, built) = rook
        .call(
            app,
            tool(
                "collection_write",
                json!({"projectId": "WORK", "collection": "Pages",
                       "fields": [{"name": "Review date", "type": "date"}]}),
            ),
        )
        .await;
    assert_eq!(built["outcome"], "ok", "{built}");
    let records = json!([
        {"record": pages[0], "fields": {"Review date": "2026-12-20"}},
        {"record": pages[2], "fields": {"Review date": "2026-12-01"}},
    ]);
    let (_, set) = rook
        .call(
            app,
            tool(
                "record_write",
                json!({"projectId": "WORK", "collection": "Pages", "records": records,
                       "clientRequestId": "review-dates"}),
            ),
        )
        .await;
    assert_eq!(set["outcome"], "ok", "{set}");

    // Filtered to those due before Christmas, soonest first.
    let (_, due) = rook
        .call(
            app,
            query(json!({
                "projectId": "WORK", "collection": "Pages",
                "filters": [{"field": "Review date", "op": "lt", "values": ["2026-12-25"]}],
                "sort": [{"field": "Review date"}],
            })),
        )
        .await;
    assert_eq!(due["outcome"], "ok", "{due}");
    assert_eq!(ids(&due), vec![pages[2].clone(), pages[0].clone()], "{due}");

    // The page owns its title and archiving: a record isn't made, renamed
    // or archived here, and a viewer sets no fields.
    for (caller, entry) in [
        (rook, json!({"title": "Made here"})),
        (rook, json!({"record": pages[1], "title": "Renamed here"})),
        (rook, json!({"record": pages[1], "archived": true})),
        (
            vera,
            json!({"record": pages[1], "fields": {"Review date": "2026-12-02"}}),
        ),
    ] {
        let (_, refused) = caller
            .call(
                app,
                tool(
                    "record_write",
                    json!({"projectId": "WORK", "collection": "Pages", "records": [entry],
                           "clientRequestId": "refused"}),
                ),
            )
            .await;
        assert_ne!(refused["outcome"], "ok", "{refused}");
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM collection_records record JOIN collections collection \
             ON collection.id = record.collection_id WHERE collection.builtin = 'pages'",
        )
        .fetch_one(world.pool())
        .await
        .expect("records"),
        3
    );

    // Renaming and archiving the page carry over to its record, and the
    // page keeps its field.
    let (_, renamed) = laptop
        .call(
            app,
            tool(
                "page_write",
                json!({"pageId": pages[2], "title": "Gamma, reviewed"}),
            ),
        )
        .await;
    assert_eq!(renamed["outcome"], "ok", "{renamed}");
    let (_, archived) = laptop
        .call(
            app,
            tool("page_write", json!({"pageId": pages[0], "archived": true})),
        )
        .await;
    assert_eq!(archived["outcome"], "ok", "{archived}");
    let (_, due) = rook
        .call(
            app,
            query(json!({
                "projectId": "WORK", "collection": "Pages",
                "filters": [{"field": "Review date", "op": "notEmpty"}],
            })),
        )
        .await;
    assert_eq!(ids(&due), vec![pages[2].clone()], "{due}");
    assert_eq!(
        due["value"]["records"][0]["title"], "Gamma, reviewed",
        "{due}"
    );

    world.finish().await;
}
