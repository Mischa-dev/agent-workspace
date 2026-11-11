//! Group members, Continue and marking unread through MCP
//! (docs/plans/mcp-tools.md, Phase 3). The costly failures are silent: someone
//! removed from a group still seeing it (or being asked to reply in it), an
//! agent handing out access to a group's history, a retried Continue granting
//! another round, and a read mark moving the wrong way.

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    agent_interface::{connect_app, create_agent, initialize, send, tool},
    mcp_admin::{Setup, create_project, setup},
    message_edits::{
        add_person, count, create_channel, direct_space, get, owner_id, remove, say, set_role, text,
    },
};

/// One MCP connection: its token and session.
struct Mcp {
    token: String,
    session: String,
}

impl Mcp {
    async fn new(setup: &Setup, token: String) -> Self {
        let session = initialize(&setup.app, &token).await.expect("MCP session");
        Self { token, session }
    }

    async fn call(&self, setup: &Setup, name: &str, arguments: Value) -> Value {
        tool(&setup.app, &self.token, &self.session, name, arguments).await
    }
}

async fn codex(setup: &Setup) -> Mcp {
    let token = connect_app(&setup.app, &setup.browser, "Codex", "full").await;
    Mcp::new(setup, token).await
}

async fn create_group(setup: &Setup, key: &str, members: &[Uuid]) -> String {
    let (status, group) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            "/api/v1/direct/groups",
            Some(key),
            json!({ "memberIds": members }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{group}");
    group["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned()
}

async fn visible(pool: &PgPool, conversation: &str, actor: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT conversation_visible_to($1, $2)")
        .bind(conversation.parse::<Uuid>().expect("conversation"))
        .bind(actor)
        .fetch_one(pool)
        .await
        .expect("visibility")
}

async fn version(pool: &PgPool, conversation: &str) -> i64 {
    count(
        pool,
        "SELECT version FROM conversations WHERE id = $1",
        conversation,
    )
    .await
}

/// Claim the agent's one open request and answer it.
async fn answer(setup: &Setup, agent: &Mcp, text: &str) {
    let inbox = agent.call(setup, "inbox_list", json!({})).await;
    let requests = inbox["value"]["requests"].as_array().expect("requests");
    assert_eq!(requests.len(), 1, "{inbox}");
    let request = requests[0]["requestId"].clone();
    let claim = agent
        .call(setup, "reply_claim", json!({ "requestId": request }))
        .await;
    assert_eq!(claim["outcome"], "ok", "{claim}");
    let sent = agent
        .call(
            setup,
            "reply_send",
            json!({
                "requestId": request,
                "generation": claim["value"]["request"]["generation"],
                "text": text,
            }),
        )
        .await;
    assert_eq!(sent["outcome"], "ok", "{sent}");
}

fn handles(members: &Value) -> Vec<String> {
    members
        .as_array()
        .unwrap_or_else(|| panic!("members: {members}"))
        .iter()
        .map(|member| member["handle"].as_str().expect("handle").to_owned())
        .collect()
}

#[tokio::test]
async fn group_members_change_who_can_see_everything() {
    let Some(setup) = setup("mcp_groups_see").await else {
        return;
    };
    let space = direct_space(&setup).await;
    let (_, _mara_token) = create_agent(&setup.app, &setup.browser, &space, "Mara", "mara").await;
    let (rook, rook_token) = create_agent(&setup.app, &setup.browser, &space, "Rook", "rook").await;
    let codex = codex(&setup).await;
    let rook_mcp = Mcp::new(&setup, rook_token).await;

    let started = codex
        .call(
            &setup,
            "conversation_start",
            json!({"people": ["@mara"], "name": "Plans", "text": "zeppelin budget"}),
        )
        .await;
    assert_eq!(started["outcome"], "ok", "{started}");
    let group = started["value"]["conversation"]["id"]
        .as_str()
        .expect("group")
        .to_owned();
    let before = version(&setup.pool, &group).await;

    let opened = rook_mcp
        .call(
            &setup,
            "conversation_open",
            json!({"conversationId": group}),
        )
        .await;
    assert_eq!(opened["outcome"], "not_found", "{opened}");
    let searched = rook_mcp
        .call(&setup, "search", json!({"query": "zeppelin"}))
        .await;
    assert_eq!(searched["outcome"], "ok", "{searched}");
    assert!(
        !searched.to_string().contains("zeppelin budget"),
        "{searched}"
    );

    let added = codex
        .call(
            &setup,
            "conversation_members",
            json!({"conversationId": group, "add": ["@rook"]}),
        )
        .await;
    assert_eq!(added["outcome"], "ok", "{added}");
    assert_eq!(handles(&added["value"]["added"]), ["rook"]);
    assert_eq!(added["value"]["members"].as_array().map(Vec::len), Some(3));
    assert_eq!(added["value"]["version"], before + 1);
    assert_eq!(added["value"]["conversationId"], group);
    assert!(added["value"].get("removed").is_none(), "{added}");

    // Rook reads the whole history, but none of it counts as unread.
    let opened = rook_mcp
        .call(
            &setup,
            "conversation_open",
            json!({"conversationId": group}),
        )
        .await;
    assert_eq!(opened["outcome"], "ok", "{opened}");
    assert!(opened.to_string().contains("zeppelin budget"), "{opened}");
    let briefing = rook_mcp.call(&setup, "briefing", json!({})).await;
    assert_eq!(briefing["outcome"], "ok", "{briefing}");
    assert!(
        !briefing["value"]["unread"].to_string().contains(&group),
        "{briefing}"
    );

    // Adding him again changes nothing.
    let again = codex
        .call(
            &setup,
            "conversation_members",
            json!({"conversationId": group, "add": [rook.to_string()]}),
        )
        .await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert!(again["value"].get("added").is_none(), "{again}");
    assert_eq!(again["value"]["version"], before + 1);

    let removed = codex
        .call(
            &setup,
            "conversation_members",
            json!({"conversationId": group, "remove": ["@rook"]}),
        )
        .await;
    assert_eq!(removed["outcome"], "ok", "{removed}");
    assert_eq!(handles(&removed["value"]["removed"]), ["rook"]);
    assert_eq!(
        removed["value"]["members"].as_array().map(Vec::len),
        Some(2)
    );

    for (name, arguments) in [
        ("conversation_open", json!({"conversationId": group})),
        ("conversation_read", json!({"conversationId": group})),
        ("mark_read", json!({"conversationId": group})),
        (
            "mark_read",
            json!({"conversationId": group, "unread": true}),
        ),
        (
            "conversation_members",
            json!({"conversationId": group, "remove": ["@rook"]}),
        ),
    ] {
        let refused = rook_mcp.call(&setup, name, arguments).await;
        assert!(
            matches!(
                refused["outcome"].as_str(),
                Some("not_found" | "permission_denied" | "forbidden")
            ),
            "{name}: {refused}"
        );
    }
    let searched = rook_mcp
        .call(&setup, "search", json!({"query": "zeppelin"}))
        .await;
    assert!(
        !searched.to_string().contains("zeppelin budget"),
        "{searched}"
    );
    let briefing = rook_mcp.call(&setup, "briefing", json!({})).await;
    assert!(!briefing.to_string().contains(&group), "{briefing}");

    // Only Rook hears that he was removed, with no conversation scope.
    let events = sqlx::query_as::<_, (Option<Uuid>, Uuid, Value)>(
        "SELECT conversation_id, resource_id, payload FROM outbox_events WHERE event_type = 'conversation.member.removed'",
    )
    .fetch_all(&setup.pool)
    .await
    .expect("member removed events");
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, None);
    assert_eq!(events[0].1.to_string(), group);
    assert_eq!(events[0].2["actorId"], rook.to_string());
    assert_eq!(events[0].2["conversationId"], group);
}

#[tokio::test]
async fn removing_an_agent_withdraws_its_reply_requests() {
    let Some(setup) = setup("mcp_groups_withdraw").await else {
        return;
    };
    let space = direct_space(&setup).await;
    let (mara, mara_token) = create_agent(&setup.app, &setup.browser, &space, "Mara", "mara").await;
    let (rook, _) = create_agent(&setup.app, &setup.browser, &space, "Rook", "rook").await;
    let codex = codex(&setup).await;
    let mara_mcp = Mcp::new(&setup, mara_token).await;
    let group = create_group(&setup, "withdraw-group", &[mara, rook]).await;
    say(
        &setup,
        &setup.browser,
        &group,
        "thoughts",
        text("Thoughts?"),
    )
    .await;

    let inbox = mara_mcp.call(&setup, "inbox_list", json!({})).await;
    let request = inbox["value"]["requests"][0]["requestId"].clone();
    let claim = mara_mcp
        .call(&setup, "reply_claim", json!({ "requestId": request }))
        .await;
    assert_eq!(claim["outcome"], "ok", "{claim}");

    let removed = codex
        .call(
            &setup,
            "conversation_members",
            json!({"conversationId": group, "remove": [mara.to_string()]}),
        )
        .await;
    assert_eq!(removed["outcome"], "ok", "{removed}");

    let request_id = request.as_str().expect("request");
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM reply_requests WHERE id = $1")
            .bind(request_id.parse::<Uuid>().expect("request"))
            .fetch_one(&setup.pool)
            .await
            .expect("status"),
        "superseded"
    );
    let sent = mara_mcp
        .call(
            &setup,
            "reply_send",
            json!({
                "requestId": request,
                "generation": claim["value"]["request"]["generation"],
                "text": "Late answer",
            }),
        )
        .await;
    assert_ne!(sent["outcome"], "ok", "{sent}");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM messages WHERE conversation_id = $1 AND author_actor_id = $2",
        )
        .bind(group.parse::<Uuid>().expect("group"))
        .bind(mara)
        .fetch_one(&setup.pool)
        .await
        .expect("messages"),
        0
    );
    let (status, timeline) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/conversations/{group}/timeline"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !timeline["replyRequests"].to_string().contains("Mara"),
        "{timeline}"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM reply_requests WHERE conversation_id = $1 AND agent_actor_id = $2",
        )
        .bind(group.parse::<Uuid>().expect("group"))
        .bind(rook)
        .fetch_one(&setup.pool)
        .await
        .expect("Rook's request"),
        "pending"
    );
}

#[tokio::test]
async fn only_people_add_others_and_agents_can_only_leave() {
    let Some(setup) = setup("mcp_groups_agents").await else {
        return;
    };
    let space = direct_space(&setup).await;
    let (mara, mara_token) = create_agent(&setup.app, &setup.browser, &space, "Mara", "mara").await;
    let (rook, _) = create_agent(&setup.app, &setup.browser, &space, "Rook", "rook").await;
    let (pat, _) = add_person(&setup, &[&space], "pat").await;
    let owner = owner_id(&setup.pool).await;
    let codex = codex(&setup).await;
    let mara_mcp = Mcp::new(&setup, mara_token).await;
    let group = create_group(&setup, "agents-group", &[mara, pat]).await;
    let first = say(&setup, &setup.browser, &group, "first", text("Hello")).await;

    // A thread under the group acts on the group.
    let thread = codex
        .call(
            &setup,
            "thread_start",
            json!({"conversationId": group, "fromMessageId": first, "name": "Side"}),
        )
        .await;
    assert_eq!(thread["outcome"], "ok", "{thread}");
    let thread_id = thread["value"]["id"].as_str().expect("thread").to_owned();

    for arguments in [
        json!({"conversationId": group, "add": ["@rook"]}),
        json!({"conversationId": group, "remove": [owner.to_string()]}),
        json!({"conversationId": thread_id, "remove": ["@pat"]}),
        json!({"conversationId": group, "remove": ["@mara", "@pat"]}),
    ] {
        let denied = mara_mcp
            .call(&setup, "conversation_members", arguments.clone())
            .await;
        assert_eq!(
            denied["outcome"], "permission_denied",
            "{arguments}: {denied}"
        );
    }
    assert!(!visible(&setup.pool, &group, rook).await);

    // A Reader connection can't change anything.
    let reader_token = connect_app(&setup.app, &setup.browser, "Reader", "read").await;
    let reader = Mcp::new(&setup, reader_token).await;
    let denied = reader
        .call(
            &setup,
            "conversation_members",
            json!({"conversationId": group, "add": ["@rook"]}),
        )
        .await;
    assert_eq!(denied["outcome"], "permission_denied", "{denied}");

    // DMs and channels never change members.
    let dm = codex
        .call(&setup, "conversation_start", json!({"people": ["@mara"]}))
        .await;
    let dm_id = dm["value"]["conversation"]["id"].as_str().expect("dm");
    let project = create_project(&setup, "GRP", "Groups").await;
    let channel = create_channel(&setup, &project, "general").await;
    for conversation in [dm_id, channel.as_str()] {
        let refused = codex
            .call(
                &setup,
                "conversation_members",
                json!({"conversationId": conversation, "add": ["@rook"]}),
            )
            .await;
        assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    }
    let refused = codex
        .call(
            &setup,
            "conversation_members",
            json!({"conversationId": group, "add": ["@rook"], "remove": ["@rook"]}),
        )
        .await;
    assert_eq!(refused["outcome"], "invalid_request", "{refused}");

    let through_thread = codex
        .call(
            &setup,
            "conversation_members",
            json!({"conversationId": thread_id, "add": ["@rook"]}),
        )
        .await;
    assert_eq!(through_thread["outcome"], "ok", "{through_thread}");
    assert_eq!(through_thread["value"]["conversationId"], group);
    assert!(visible(&setup.pool, &thread_id, rook).await);

    // Mara leaves, even as a viewer (leaving needs only to see the group);
    // she can't see it afterwards.
    set_role(&setup.pool, &space, mara, "viewer").await;
    let left = mara_mcp
        .call(
            &setup,
            "conversation_members",
            json!({"conversationId": group, "remove": ["@mara"]}),
        )
        .await;
    assert_eq!(left["outcome"], "ok", "{left}");
    assert_eq!(left["value"]["left"], true);
    assert_eq!(left["value"]["members"], json!([]));
    let opened = mara_mcp
        .call(
            &setup,
            "conversation_open",
            json!({"conversationId": group}),
        )
        .await;
    assert_eq!(opened["outcome"], "not_found", "{opened}");

    // The web's add and remove share the command: removing someone who isn't
    // in the group changes nothing.
    let before = version(&setup.pool, &group).await;
    let (status, body) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{group}/members/{mara}/remove"),
            None,
            Value::Null,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], before);
    assert_eq!(version(&setup.pool, &group).await, before);
    let (status, body) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{group}/members"),
            None,
            json!({ "actorId": mara }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["version"], before + 1);
    assert!(visible(&setup.pool, &group, mara).await);
    // New members start read: Mara has seen everything there is.
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT highest_seen_sequence FROM conversation_read_states WHERE conversation_id = $1 AND actor_id = $2",
        )
        .bind(group.parse::<Uuid>().expect("group"))
        .bind(mara)
        .fetch_one(&setup.pool)
        .await
        .expect("Mara's read mark"),
        count(
            &setup.pool,
            "SELECT next_entry_sequence - 1 FROM conversations WHERE id = $1",
            &group
        )
        .await
    );
}

#[tokio::test]
async fn group_member_cap_holds_for_adds() {
    let Some(setup) = setup("mcp_groups_cap").await else {
        return;
    };
    let space = direct_space(&setup).await;
    let mut people = Vec::new();
    // People who never sign in: a member of the Messages space is enough.
    for index in 0..20 {
        let actor = Uuid::now_v7();
        sqlx::query("INSERT INTO actors (id, kind, status) VALUES ($1, 'human', 'active')")
            .bind(actor)
            .execute(&setup.pool)
            .await
            .expect("person");
        sqlx::query(
            "INSERT INTO organization_memberships (organization_id, actor_id, role) VALUES ($1, $2, 'member')",
        )
        .bind(setup.organization_id)
        .bind(actor)
        .execute(&setup.pool)
        .await
        .expect("organization membership");
        sqlx::query(
            r#"
            INSERT INTO project_memberships (id, project_id, actor_id, role, display_name, handle)
            VALUES ($1, $2, $3, 'member', $4, $4)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(space.parse::<Uuid>().expect("space"))
        .bind(actor)
        .bind(format!("p{index}"))
        .execute(&setup.pool)
        .await
        .expect("space membership");
        people.push(actor);
    }
    // The owner plus 18 people: 19 members.
    let group = create_group(&setup, "cap-group", &people[..18]).await;
    let codex = codex(&setup).await;
    let full = codex
        .call(
            &setup,
            "conversation_members",
            json!({"conversationId": group, "add": ["@p18"]}),
        )
        .await;
    assert_eq!(full["outcome"], "ok", "{full}");
    let over = codex
        .call(
            &setup,
            "conversation_members",
            json!({"conversationId": group, "add": ["@p19"]}),
        )
        .await;
    assert_eq!(over["outcome"], "conflict", "{over}");
    let (status, body) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{group}/members"),
            None,
            json!({ "actorId": people[19] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    // Swapping one for another in a single change fits.
    let swapped = codex
        .call(
            &setup,
            "conversation_members",
            json!({"conversationId": group, "add": ["@p19"], "remove": ["@p0"]}),
        )
        .await;
    assert_eq!(swapped["outcome"], "ok", "{swapped}");
    assert_eq!(
        swapped["value"]["members"].as_array().map(Vec::len),
        Some(20)
    );
}

#[tokio::test]
async fn member_changes_replay_without_undoing_later_ones() {
    let Some(setup) = setup("mcp_groups_replay").await else {
        return;
    };
    let space = direct_space(&setup).await;
    let (mara, _) = create_agent(&setup.app, &setup.browser, &space, "Mara", "mara").await;
    let (rook, _) = create_agent(&setup.app, &setup.browser, &space, "Rook", "rook").await;
    let codex = codex(&setup).await;
    let group = create_group(&setup, "replay-group", &[mara]).await;

    let add = json!({"conversationId": group, "add": ["@rook"], "clientRequestId": "add-rook-1"});
    let added = codex
        .call(&setup, "conversation_members", add.clone())
        .await;
    assert_eq!(added["outcome"], "ok", "{added}");
    let removed = codex
        .call(
            &setup,
            "conversation_members",
            json!({"conversationId": group, "remove": ["@rook"], "clientRequestId": "rm-rook-1"}),
        )
        .await;
    assert_eq!(removed["outcome"], "ok", "{removed}");

    let replayed = codex.call(&setup, "conversation_members", add).await;
    assert_eq!(replayed["outcome"], "ok", "{replayed}");
    assert_eq!(replayed["value"]["idempotentReplay"], true);
    assert!(
        !visible(&setup.pool, &group, rook).await,
        "the replay re-added nobody"
    );

    let mismatch = codex
        .call(
            &setup,
            "conversation_members",
            json!({"conversationId": group, "add": ["@rook", "@mara"], "clientRequestId": "add-rook-1"}),
        )
        .await;
    assert_eq!(mismatch["outcome"], "conflict", "{mismatch}");

    let current = version(&setup.pool, &group).await;
    let stale = codex
        .call(
            &setup,
            "conversation_members",
            json!({"conversationId": group, "add": ["@rook"], "expectedVersion": current - 1}),
        )
        .await;
    assert_eq!(stale["outcome"], "conflict", "{stale}");
    assert!(!visible(&setup.pool, &group, rook).await);
}

#[tokio::test]
async fn continue_is_for_people_and_asks_each_agent_once() {
    let Some(setup) = setup("mcp_groups_continue").await else {
        return;
    };
    let space = direct_space(&setup).await;
    let (mara, mara_token) = create_agent(&setup.app, &setup.browser, &space, "Mara", "mara").await;
    let (rook, _) = create_agent(&setup.app, &setup.browser, &space, "Rook", "rook").await;
    let codex = codex(&setup).await;
    let mara_mcp = Mcp::new(&setup, mara_token).await;
    let group = create_group(&setup, "continue-group", &[mara, rook]).await;
    say(&setup, &setup.browser, &group, "plan", text("Plan?")).await;
    sqlx::query("UPDATE conversations SET agent_turns_remaining = 1 WHERE id = $1::uuid")
        .bind(&group)
        .execute(&setup.pool)
        .await
        .expect("shrink allowance");
    // Mara's answer uses the last turn, so Rook isn't asked about it.
    answer(&setup, &mara_mcp, "Inbox first.").await;
    let maras = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM messages WHERE conversation_id = $1 AND author_actor_id = $2",
    )
    .bind(group.parse::<Uuid>().expect("group"))
    .bind(mara)
    .fetch_one(&setup.pool)
    .await
    .expect("Mara's message");

    let denied = mara_mcp
        .call(
            &setup,
            "conversation_continue",
            json!({"conversationId": group}),
        )
        .await;
    assert_eq!(denied["outcome"], "permission_denied", "{denied}");

    let continued = codex
        .call(
            &setup,
            "conversation_continue",
            json!({"conversationId": group, "clientRequestId": "c1"}),
        )
        .await;
    assert_eq!(continued["outcome"], "ok", "{continued}");
    assert_eq!(continued["value"]["agentTurnsRemaining"], 6);
    assert_eq!(handles(&continued["value"]["asked"]), ["rook"]);
    assert_eq!(continued["value"]["latestMessageId"], maras.to_string());

    let open_for_rook = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM reply_requests WHERE agent_actor_id = $1 AND status IN ('pending', 'claimed')",
        )
        .bind(rook)
        .fetch_one(&setup.pool)
        .await
        .expect("Rook's requests")
    };
    assert_eq!(open_for_rook().await, 1);
    let again = codex
        .call(
            &setup,
            "conversation_continue",
            json!({"conversationId": group, "clientRequestId": "c2"}),
        )
        .await;
    assert_eq!(again["outcome"], "ok", "{again}");
    assert_eq!(handles(&again["value"]["asked"]), ["rook"]);
    assert_eq!(open_for_rook().await, 1, "asked once, not superseded");

    // A replay doesn't grant another round.
    sqlx::query("UPDATE conversations SET agent_turns_remaining = 0 WHERE id = $1::uuid")
        .bind(&group)
        .execute(&setup.pool)
        .await
        .expect("use up allowance");
    let replayed = codex
        .call(
            &setup,
            "conversation_continue",
            json!({"conversationId": group, "clientRequestId": "c1"}),
        )
        .await;
    assert_eq!(replayed["outcome"], "ok", "{replayed}");
    assert_eq!(replayed["value"]["idempotentReplay"], true);
    assert_eq!(
        count(
            &setup.pool,
            "SELECT agent_turns_remaining::bigint FROM conversations WHERE id = $1",
            &group
        )
        .await,
        0
    );

    // A removed newest message is skipped: nobody is asked about it.
    let (status, body) = remove(&setup, &setup.browser, &group, &maras.to_string()).await;
    assert!(status.is_success(), "{body}");
    let after_removal = codex
        .call(
            &setup,
            "conversation_continue",
            json!({"conversationId": group}),
        )
        .await;
    assert_eq!(after_removal["outcome"], "ok", "{after_removal}");
    assert_ne!(after_removal["value"]["latestMessageId"], maras.to_string());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM reply_requests WHERE message_id = $1 AND status IN ('pending', 'claimed')",
        )
        .bind(maras)
        .fetch_one(&setup.pool)
        .await
        .expect("requests on the removed message"),
        0
    );

    // DMs and channels have no turns.
    let dm = codex
        .call(&setup, "conversation_start", json!({"people": ["@mara"]}))
        .await;
    let dm_id = dm["value"]["conversation"]["id"].as_str().expect("dm");
    let project = create_project(&setup, "CNT", "Continue").await;
    let channel = create_channel(&setup, &project, "general").await;
    for conversation in [dm_id, channel.as_str()] {
        let refused = codex
            .call(
                &setup,
                "conversation_continue",
                json!({"conversationId": conversation}),
            )
            .await;
        assert_eq!(refused["outcome"], "invalid_request", "{refused}");
    }
}

#[tokio::test]
async fn marking_unread_brings_a_conversation_back_and_never_marks_anything_read() {
    let Some(setup) = setup("mcp_groups_unread").await else {
        return;
    };
    let space = direct_space(&setup).await;
    let (mara, mara_token) = create_agent(&setup.app, &setup.browser, &space, "Mara", "mara").await;
    let (rook, _) = create_agent(&setup.app, &setup.browser, &space, "Rook", "rook").await;
    let codex = codex(&setup).await;
    let mara_mcp = Mcp::new(&setup, mara_token).await;
    let group = create_group(&setup, "unread-group", &[mara, rook]).await;
    let other = create_group(&setup, "other-group", &[mara]).await;
    let elsewhere = say(
        &setup,
        &setup.browser,
        &other,
        "elsewhere",
        text("Elsewhere"),
    )
    .await;

    let mut sent = Vec::new();
    for message in ["One", "Two for @owner", "Three"] {
        let result = mara_mcp
            .call(
                &setup,
                "message_send",
                json!({"conversationId": group, "text": message}),
            )
            .await;
        assert_eq!(result["outcome"], "ok", "{result}");
        sent.push(
            result["value"]["messageId"]
                .as_str()
                .expect("id")
                .to_owned(),
        );
    }
    let mara_mark = || async {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT highest_seen_sequence, version FROM conversation_read_states WHERE conversation_id = $1 AND actor_id = $2",
        )
        .bind(group.parse::<Uuid>().expect("group"))
        .bind(mara)
        .fetch_optional(&setup.pool)
        .await
        .expect("Mara's mark")
    };
    let mara_before = mara_mark().await;

    let read = codex
        .call(&setup, "mark_read", json!({"conversationId": group}))
        .await;
    assert_eq!(read["outcome"], "ok", "{read}");
    assert_eq!(read["value"]["lastReadSequence"], 3);
    assert_eq!(read["value"]["unreadCount"], 0);
    let (_, timeline) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/conversations/{group}/timeline"),
    )
    .await;
    let read_version = timeline["readState"]["version"].as_i64().expect("version");

    let unread = codex
        .call(
            &setup,
            "mark_read",
            json!({"conversationId": group, "unread": true, "fromMessageId": sent[1]}),
        )
        .await;
    assert_eq!(unread["outcome"], "ok", "{unread}");
    assert_eq!(unread["value"]["lastReadSequence"], 1);
    assert_eq!(unread["value"]["unreadCount"], 2);
    let briefing = codex.call(&setup, "briefing", json!({})).await;
    let listed = briefing["value"]["unread"]
        .as_array()
        .expect("unread")
        .iter()
        .find(|conversation| conversation["id"] == group)
        .unwrap_or_else(|| panic!("the group is unread again: {briefing}"))
        .clone();
    assert_eq!(listed["unreadCount"], 2);
    assert!(
        briefing["value"]["mentions"].to_string().contains(&sent[1]),
        "the @mention is back: {briefing}"
    );

    // Never forwards: from a later message, or from the newest, nothing moves.
    let link = format!("http://127.0.0.1:3100/messages/{group}#message-{}", sent[2]);
    for arguments in [
        json!({"conversationId": group, "unread": true, "fromMessageId": link}),
        json!({"conversationId": group, "unread": true}),
    ] {
        let kept = codex.call(&setup, "mark_read", arguments).await;
        assert_eq!(kept["outcome"], "ok", "{kept}");
        assert_eq!(kept["value"]["lastReadSequence"], 1, "{kept}");
    }

    for arguments in [
        json!({"conversationId": group, "unread": true, "upToSequence": 1}),
        json!({"conversationId": group, "unread": true, "fromMessageId": elsewhere}),
        json!({"conversationId": group, "fromMessageId": sent[0]}),
        json!({"conversationId": group, "unread": true, "fromMessageId": "nope"}),
    ] {
        let refused = codex.call(&setup, "mark_read", arguments.clone()).await;
        assert_eq!(
            refused["outcome"], "invalid_request",
            "{arguments}: {refused}"
        );
    }
    let reader_token = connect_app(&setup.app, &setup.browser, "Reader", "read").await;
    let reader = Mcp::new(&setup, reader_token).await;
    let denied = reader
        .call(
            &setup,
            "mark_read",
            json!({"conversationId": group, "unread": true}),
        )
        .await;
    assert_eq!(denied["outcome"], "permission_denied", "{denied}");

    assert_eq!(
        mara_mark().await,
        mara_before,
        "only the caller's mark moves"
    );
    let (_, timeline) = get(
        &setup,
        &setup.browser,
        &format!("/api/v1/conversations/{group}/timeline"),
    )
    .await;
    assert_eq!(timeline["readState"]["highestSeenSequence"], 1);
    assert!(timeline["readState"]["version"].as_i64().expect("version") > read_version);

    // The web's Mark unread shares the command.
    let (status, body) = send(
        &setup.app,
        setup.browser.request(
            "POST",
            &format!("/api/v1/conversations/{group}/mark-unread"),
            None,
            json!({ "messageId": sent[0] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["highestSeenSequence"], 0);

    // Nothing else can move a mark backwards.
    sqlx::query(
        "UPDATE conversation_read_states SET highest_seen_sequence = 2, version = version + 1 WHERE conversation_id = $1 AND actor_id = $2",
    )
    .bind(group.parse::<Uuid>().expect("group"))
    .bind(owner_id(&setup.pool).await)
    .execute(&setup.pool)
    .await
    .expect("forward");
    let backwards = sqlx::query(
        "UPDATE conversation_read_states SET highest_seen_sequence = 0, version = version + 1 WHERE conversation_id = $1 AND actor_id = $2",
    )
    .bind(group.parse::<Uuid>().expect("group"))
    .bind(owner_id(&setup.pool).await)
    .execute(&setup.pool)
    .await;
    let error = backwards.expect_err("a stray backwards update is refused");
    assert!(
        error.to_string().contains("cannot move backwards"),
        "{error}"
    );
    // Nor can a flag naming someone else's mark.
    let mut transaction = setup.pool.begin().await.expect("transaction");
    sqlx::query("SELECT set_config('aw.read_rewind', $1, true)")
        .bind(format!("{group}:{mara}"))
        .execute(&mut *transaction)
        .await
        .expect("flag");
    let wrong_flag = sqlx::query(
        "UPDATE conversation_read_states SET highest_seen_sequence = 0, version = version + 1 WHERE conversation_id = $1 AND actor_id = $2",
    )
    .bind(group.parse::<Uuid>().expect("group"))
    .bind(owner_id(&setup.pool).await)
    .execute(&mut *transaction)
    .await;
    assert!(
        wrong_flag.is_err(),
        "the flag is for one conversation and actor"
    );
    drop(transaction);

    // "Unread from the newest" means the newest message, even when a system
    // or worker entry came after it.
    let group_id = group.parse::<Uuid>().expect("group");
    let mut transaction = setup.pool.begin().await.expect("transaction");
    let sequence = sqlx::query_scalar::<_, i64>(
        "UPDATE conversations SET next_entry_sequence = next_entry_sequence + 1 WHERE id = $1 RETURNING next_entry_sequence - 1",
    )
    .bind(group_id)
    .fetch_one(&mut *transaction)
    .await
    .expect("allocate a sequence");
    assert_eq!(sequence, 4);
    sqlx::query(
        "INSERT INTO conversation_entries (id, conversation_id, sequence, kind) VALUES ($1, $2, $3, 'system')",
    )
    .bind(Uuid::now_v7())
    .bind(group_id)
    .bind(sequence)
    .execute(&mut *transaction)
    .await
    .expect("a system entry after the last message");
    transaction.commit().await.expect("commit");
    let read = codex
        .call(&setup, "mark_read", json!({"conversationId": group}))
        .await;
    assert_eq!(read["value"]["lastReadSequence"], 4, "{read}");
    let unread = codex
        .call(
            &setup,
            "mark_read",
            json!({"conversationId": group, "unread": true}),
        )
        .await;
    assert_eq!(unread["outcome"], "ok", "{unread}");
    assert_eq!(unread["value"]["lastReadSequence"], 2, "{unread}");
}
