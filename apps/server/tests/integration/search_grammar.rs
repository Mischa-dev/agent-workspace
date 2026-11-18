//! C2.25: the search grammar. Each filter narrows to what it names, over
//! HTTP and the agents' `search` tool alike, and a filter never widens what
//! someone can see: `in:#channel` for a private channel they aren't in finds
//! nothing, as does `from:` for messages in a DM that isn't theirs. Jump-to
//! and saved searches (C2.26) keep to the same rules.

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use super::support::{
    app::send,
    callers::{Call, Caller, Person},
    world::WorldBuilder,
};

fn text(value: &str) -> Value {
    json!({ "version": 1, "nodes": [{ "type": "text", "text": value }] })
}

fn encoded(query: &str) -> String {
    query
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b':' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// The ids of the messages `person` finds for `query`.
async fn found(app: &axum::Router, person: &Person, query: &str) -> Vec<String> {
    let path = format!("/api/v1/search?q={}", encoded(query));
    let (status, body) = send(app, person.browser.request("GET", &path, None, Value::Null)).await;
    assert_eq!(status, StatusCode::OK, "{query}: {body}");
    let mut ids = body["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .filter(|hit| hit["kind"] == "message")
        .map(|hit| hit["id"].as_str().expect("id").to_owned())
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

async fn say(app: &axum::Router, who: &Person, conversation: &str, words: &str) -> String {
    let (status, sent) = send(
        app,
        who.browser.request(
            "POST",
            &format!("/api/v1/conversations/{conversation}/messages"),
            Some(&Uuid::now_v7().to_string()),
            json!({"content": text(words)}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{sent}");
    sent["message"]["id"].as_str().expect("message").to_owned()
}

async fn op(app: &axum::Router, by: &Person, name: &str, input: Value) -> (StatusCode, Value) {
    send(
        app,
        by.browser.request(
            "POST",
            &format!("/api/v2/ops/{name}"),
            Some(&Uuid::now_v7().to_string()),
            input,
        ),
    )
    .await
}

fn sorted(ids: &[&String]) -> Vec<String> {
    let mut ids = ids.iter().map(|id| (*id).clone()).collect::<Vec<_>>();
    ids.sort();
    ids
}

#[tokio::test]
async fn each_filter_narrows_and_none_widens() {
    let Some(world) = WorldBuilder::default()
        .project("WORK")
        .person("lena", "member")
        .person("mara", "member")
        .person("nina", "member")
        .agent("rook", "member")
        .dm("lena", "mara")
        .build("c2_25_search_grammar")
        .await
    else {
        return;
    };
    let app = &world.app;
    let work = world.project("WORK");
    let lena = &world.people["lena"];
    let mara = &world.people["mara"];
    let nina = &world.people["nina"];
    let rook = &world.agents["rook"];
    let word = format!("zqgrammar{}", &Uuid::now_v7().simple().to_string()[20..]);

    let channel = |name: &'static str, private: bool| async move {
        let (status, made) = send(
            app,
            lena.browser.request(
                "POST",
                &format!("/api/v1/projects/{work}/channels"),
                Some(&format!("grammar-{name}")),
                json!({"name": name, "private": private}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{made}");
        made["conversation"]["id"]
            .as_str()
            .expect("channel")
            .to_owned()
    };
    let design = channel("design", false).await;
    let secret = channel("secret-ops", true).await;
    let dm = world.conversations["lena+mara"].clone();
    let planned = say(
        app,
        lena,
        &design,
        &format!("{word} plan at https://example.com/plan"),
    )
    .await;
    let reviewed = say(app, mara, &design, &format!("{word} review")).await;
    let whispered = say(app, lena, &dm, &format!("{word} just between us")).await;
    let hidden = say(app, lena, &secret, &format!("{word} the hidden plan")).await;

    // Lena reacts to Mara's, pins her own and saves Mara's.
    let (status, body) = send(
        app,
        lena.browser.request(
            "PUT",
            &format!(
                "/api/v1/conversations/{design}/messages/{reviewed}/reactions/{}",
                encoded("👍")
            ),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "react: {body}");
    let (status, body) = send(
        app,
        lena.browser.request(
            "POST",
            &format!("/api/v1/conversations/{design}/pins"),
            Some("grammar-pin"),
            json!({"kind": "message", "targetId": planned}),
        ),
    )
    .await;
    assert!(status.is_success(), "pin: {body}");
    let (status, body) = send(
        app,
        lena.browser.request(
            "POST",
            "/api/v1/later",
            Some("grammar-save"),
            json!({"messageId": reviewed}),
        ),
    )
    .await;
    assert!(status.is_success(), "save: {body}");

    let all = sorted(&[&planned, &reviewed, &whispered, &hidden]);
    assert_eq!(found(app, lena, &word).await, all);
    for (query, expected) in [
        (format!("{word} from:@mara"), sorted(&[&reviewed])),
        (
            format!("{word} from:me"),
            sorted(&[&planned, &whispered, &hidden]),
        ),
        (format!("{word} is:mine from:@Mara"), all.clone()),
        (format!("{word} in:#design"), sorted(&[&planned, &reviewed])),
        (format!("{word} in:@mara"), sorted(&[&whispered])),
        (format!("{word} in:#secret-ops"), sorted(&[&hidden])),
        (format!("{word} has:link"), sorted(&[&planned])),
        (format!("{word} has:reaction"), sorted(&[&reviewed])),
        (format!("{word} has:pin"), sorted(&[&planned])),
        (format!("{word} has:file"), Vec::new()),
        (format!("{word} is:saved"), sorted(&[&reviewed])),
        (format!("{word} is:dm"), sorted(&[&whispered])),
        (format!("{word} is:thread"), Vec::new()),
        // Lena's own messages are never unread to her.
        (format!("{word} is:unread"), sorted(&[&reviewed])),
        (format!("{word} kind:task"), Vec::new()),
        (format!("{word} kind:message"), all.clone()),
        (format!("{word} before:1999"), Vec::new()),
        (format!("{word} during:9998"), Vec::new()),
        (format!("{word} in:#nowhere"), Vec::new()),
        (format!("{word} from:@nobody"), Vec::new()),
        // "-word" excludes; a quoted phrase must match as one.
        (
            format!("{word} -review"),
            sorted(&[&planned, &whispered, &hidden]),
        ),
        (format!("\"{word} review\""), sorted(&[&reviewed])),
    ] {
        assert_eq!(found(app, lena, &query).await, expected, "{query}");
    }
    // Filters alone list what matches them.
    let listed = found(app, lena, "from:@mara in:#design").await;
    assert!(listed.contains(&reviewed), "{listed:?}");
    assert!(!listed.contains(&planned), "{listed:?}");

    // A filter that isn't one, and words-free with nothing to narrow: refused.
    for query in [format!("{word} has:banana"), "kind:task".to_owned()] {
        let path = format!("/api/v1/search?q={}", encoded(&query));
        let (status, body) = send(app, lena.browser.request("GET", &path, None, Value::Null)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
    }

    // Nina is in the project, not the private channel or the DM: naming
    // them finds nothing, and from:@lena finds only what she can read.
    assert_eq!(
        found(app, nina, &format!("{word} in:#secret-ops")).await,
        Vec::<String>::new()
    );
    assert_eq!(
        found(app, nina, &format!("{word} in:@mara")).await,
        Vec::<String>::new()
    );
    assert_eq!(
        found(app, nina, &format!("{word} from:@lena")).await,
        sorted(&[&planned])
    );
    assert_eq!(
        found(app, nina, &format!("{word} is:dm")).await,
        Vec::<String>::new()
    );

    // The agents' tool reads the same grammar.
    let (_, searched) = rook
        .call(
            app,
            Call::Tool {
                name: "search",
                arguments: json!({"query": format!("{word} in:#design from:@mara")}),
            },
        )
        .await;
    assert_eq!(searched["outcome"], "ok", "{searched}");
    let ids = searched["value"]["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|hit| hit["id"].as_str().expect("id").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![reviewed.clone()], "{searched}");
    let (_, refused) = rook
        .call(
            app,
            Call::Tool {
                name: "search",
                arguments: json!({"query": format!("{word} is:urgent")}),
            },
        )
        .await;
    assert_ne!(refused["outcome"], "ok", "{refused}");

    // C2.26: jump-to finds places by name, only those the caller can open.
    let (status, places) = op(app, lena, "place_search", json!({"q": "design"})).await;
    assert_eq!(status, StatusCode::OK, "{places}");
    let first = &places["value"]["hits"][0];
    assert_eq!(first["kind"], "channel", "{places}");
    assert_eq!(first["id"], design.as_str(), "{places}");
    assert!(first["path"].as_str().is_some_and(|path| path.contains(&design)));
    let (_, places) = op(app, lena, "place_search", json!({"q": "mara"})).await;
    let kinds = places["value"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|hit| (hit["kind"].as_str().unwrap_or("").to_owned(), hit["id"].clone()))
        .collect::<Vec<_>>();
    assert!(kinds.contains(&("person".to_owned(), json!(mara.actor))), "{places}");
    assert!(kinds.contains(&("dm".to_owned(), json!(dm))), "{places}");
    for q in ["secret", "mara", "lena"] {
        let (status, places) = op(app, nina, "place_search", json!({"q": q})).await;
        assert_eq!(status, StatusCode::OK, "{places}");
        assert!(!places.to_string().contains(&secret), "{q}: {places}");
        assert!(!places.to_string().contains(&dm), "{q}: {places}");
    }

    // Saved searches: saved, renamed, moved and removed, each account's own.
    let query = format!("{word} in:#design");
    let (status, saved) = op(
        app,
        lena,
        "saved_search_set",
        json!({"name": "Design talk", "query": query}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let id = saved["value"]["searches"][0]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let (status, refused) = op(
        app,
        lena,
        "saved_search_set",
        json!({"name": "Bad", "query": "has:banana"}),
    )
    .await;
    assert!(!status.is_success(), "{refused}");
    let (status, renamed) = op(
        app,
        lena,
        "saved_search_set",
        json!({"savedSearchId": id, "name": "Design"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["value"]["searches"][0]["name"], "Design");
    assert_eq!(renamed["value"]["searches"][0]["query"], query.as_str());
    let (_, maras) = op(app, mara, "saved_search_list", json!({})).await;
    assert_eq!(maras["value"]["searches"], json!([]), "{maras}");
    let (status, removed) = op(
        app,
        lena,
        "saved_search_set",
        json!({"savedSearchId": id, "delete": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed["value"]["searches"], json!([]), "{removed}");

    world.finish().await;
}
