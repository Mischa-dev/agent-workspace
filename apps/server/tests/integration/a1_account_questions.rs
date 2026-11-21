//! A1.17 step 4: a question about a model account goes to the built-in host
//! as a command, and comes back in its acknowledgement. Here the server
//! asking doesn't carry the host itself (as when the host runs as its own
//! program): each question waits for the host's loop to answer it. An API
//! key travels sealed beside its sign-in and is taken once; no command keeps
//! it, and an answered question is gone once read.

use std::{sync::Arc, time::Duration};

use agent_workspace_host_protocol::{
    Ack, AckOutcome, CommandKind,
    account::{AccountAnswer, AccountAsk, AccountQuestion, LoginStarted},
};
use agent_workspace_server::{
    AppState, db,
    harness::manifest::LoginKind,
    hosts::{
        self,
        builtin::{
            accounts::{self, Asked},
            assistant, driver,
        },
        commands::{self, NewCommand},
        protocol::HostError,
    },
    provider::SupervisorRegistry,
    secrets,
    sign_ins::{self, LoginReservation},
    tenancy,
};
use serde_json::json;
use uuid::Uuid;

use crate::support::two_workspaces::two_workspaces;

const API_KEY: &str = "sealed-demo-key-4821";

/// Ask as a server that doesn't carry the host, while the host's loop
/// (`host`) answers the commands it finds.
async fn ask_remote(asking: &AppState, host: &AppState, workspace: Uuid, ask: AccountAsk) -> Asked {
    let asked = tenancy::system("runs.host", accounts::ask(asking, workspace, ask));
    let answering = async {
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            tenancy::system("runs.host", driver::pump(host))
                .await
                .expect("the host's loop");
        }
    };
    tokio::select! {
        asked = asked => asked.expect("ask the host"),
        () = answering => unreachable!(),
    }
}

#[tokio::test]
async fn account_questions_go_to_the_host_and_keys_travel_sealed() {
    let Some(world) = two_workspaces("a1_account_questions").await else {
        return;
    };
    // The host runs the demo harness for every account, so no harness
    // program is needed.
    let mut host_config = (*world.state.config).clone();
    host_config.demo_executor = true;
    let host = AppState {
        supervisors: SupervisorRegistry::new(&host_config).expect("the host's harnesses"),
        config: Arc::new(host_config),
        ..world.state.clone()
    };
    let mut remote_config = (*world.state.config).clone();
    remote_config.builtin_host = false;
    let asking = AppState {
        config: Arc::new(remote_config),
        ..world.state.clone()
    };
    let workspace = world.a.organization;
    let (harness, home, owner) = sqlx::query_as::<_, (String, String, Uuid)>(
        "SELECT adapter_kind, home_path, owner_actor_id FROM provider_accounts WHERE id = $1",
    )
    .bind(world.a.provider_account)
    .fetch_one(&world.pool)
    .await
    .expect("the account");
    let ask = |question| AccountAsk {
        account_id: world.a.provider_account,
        harness: harness.clone(),
        home: home.clone(),
        max_concurrent_runs: 1,
        question,
    };

    // Signed out, the account reads so.
    let out = ask_remote(&asking, &host, workspace, ask(AccountQuestion::Logout)).await;
    assert!(
        matches!(out, Asked::Answered(AccountAnswer::Done)),
        "{out:?}"
    );
    let status = ask_remote(&asking, &host, workspace, ask(AccountQuestion::Status)).await;
    assert!(
        matches!(
            status,
            Asked::Answered(AccountAnswer::Status {
                authenticated: false,
                ..
            })
        ),
        "{status:?}"
    );

    // A sign-in method the harness doesn't have is refused as such.
    let browser = ask_remote(
        &asking,
        &host,
        workspace,
        ask(AccountQuestion::Login {
            kind: LoginKind::Browser,
        }),
    )
    .await;
    assert!(matches!(browser, Asked::Unsupported), "{browser:?}");

    // An API key is sealed beside its sign-in; the host signs in with it.
    let session = Uuid::now_v7();
    let reserved = tenancy::system(
        "runs.host",
        sign_ins::reserve_login(
            &world.pool,
            world.a.provider_account,
            owner,
            session,
            "a1-account-questions",
            LoginKind::ApiKey,
        ),
    )
    .await;
    let Ok(LoginReservation::Reserved(ceremony_id)) = reserved else {
        panic!("reserve a sign-in");
    };
    let keys = secrets::keyring(&host.config).expect("keyring");
    tenancy::system(
        "runs.host",
        sign_ins::seal_api_key(
            &world.pool,
            keys.as_ref(),
            world.a.provider_account,
            ceremony_id,
            API_KEY,
        ),
    )
    .await
    .expect("seal the key");
    let sealed_text = sqlx::query_scalar::<_, bool>(
        "SELECT position($1::bytea IN api_key_ciphertext) > 0 FROM provider_login_ceremonies WHERE ceremony_id = $2",
    )
    .bind(API_KEY.as_bytes())
    .bind(ceremony_id)
    .fetch_one(&world.pool)
    .await
    .expect("the sealed key");
    assert!(!sealed_text, "the key is kept sealed, not as it was typed");
    let signed_in = ask_remote(
        &asking,
        &host,
        workspace,
        ask(AccountQuestion::ApiKeyLogin { ceremony_id }),
    )
    .await;
    assert!(
        matches!(
            signed_in,
            Asked::Answered(AccountAnswer::LoginStarted {
                start: LoginStarted::ApiKey
            })
        ),
        "{signed_in:?}"
    );
    let status = ask_remote(&asking, &host, workspace, ask(AccountQuestion::Status)).await;
    assert!(
        matches!(
            status,
            Asked::Answered(AccountAnswer::Status {
                authenticated: true,
                ..
            })
        ),
        "{status:?}"
    );

    // Signed in, the host lists the account's models; a harness with no
    // environments of its own says so.
    let models = ask_remote(&asking, &host, workspace, ask(AccountQuestion::Models)).await;
    assert!(
        matches!(&models, Asked::Answered(AccountAnswer::Models { models }) if !models.is_empty()),
        "{models:?}"
    );
    let environment = ask_remote(
        &asking,
        &host,
        workspace,
        ask(AccountQuestion::CheckEnvironment {
            environment: "main".to_owned(),
        }),
    )
    .await;
    assert!(matches!(environment, Asked::Unsupported), "{environment:?}");

    // The key was taken once: the sign-in keeps it no longer, and asking
    // again has nothing to sign in with.
    let still_sealed = sqlx::query_scalar::<_, bool>(
        "SELECT api_key_ciphertext IS NOT NULL FROM provider_login_ceremonies WHERE ceremony_id = $1",
    )
    .bind(ceremony_id)
    .fetch_one(&world.pool)
    .await
    .expect("the sign-in");
    assert!(!still_sealed);
    let again = ask_remote(
        &asking,
        &host,
        workspace,
        ask(AccountQuestion::ApiKeyLogin { ceremony_id }),
    )
    .await;
    assert!(matches!(again, Asked::Failed(_)), "{again:?}");

    // Answered questions are gone once read: no command keeps the key or
    // what the host answered, and polls don't pile up.
    let left = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM host_commands \
         WHERE workspace_id = $1 AND kind = 'action' AND body->>'action' = 'account'",
    )
    .bind(workspace)
    .fetch_one(&world.pool)
    .await
    .expect("the commands");
    assert_eq!(left, 0);
}

/// An answer of about `bytes`, in short strings (one long one would be cut
/// down when it is stored).
fn answer_of(bytes: usize) -> Ack {
    let mut ack = Ack::new(AckOutcome::Done);
    let models = (0..bytes / 100)
        .map(|n| format!("{n:0>96}"))
        .collect::<Vec<_>>();
    ack.result = Some(json!({ "answer": "models", "models": models }));
    ack
}

#[tokio::test]
async fn the_built_in_host_may_answer_a_question_with_a_large_catalog() {
    let Some(world) = two_workspaces("a1_large_answers").await else {
        return;
    };
    let workspace = world.a.organization;
    let issue = |kind: CommandKind| {
        let pool = world.pool.clone();
        tenancy::system("runs.host", async move {
            let mut transaction = db::scoped_tx(&pool).await.expect("begin");
            let host_id = assistant::builtin_host_id(&mut transaction, workspace)
                .await
                .expect("the host")
                .expect("a built-in host");
            let host = hosts::get(&mut transaction, host_id)
                .await
                .expect("the host")
                .expect("the host's row");
            let command = commands::issue(
                &mut transaction,
                NewCommand {
                    workspace_id: workspace,
                    host_id,
                    run_id: None,
                    request_id: None,
                    kind,
                    start_key: None,
                    body: json!({"action": "account"}),
                },
            )
            .await
            .expect("issue");
            transaction.commit().await.expect("commit");
            (host, command.id)
        })
    };
    let ack = |host, command, bytes| {
        let pool = world.pool.clone();
        tenancy::system("runs.host", async move {
            commands::ack(&pool, &host, command, answer_of(bytes)).await
        })
    };

    // A catalog past 64 KiB is kept, as an action's answer, sent the way
    // the host sends it (through the server's routes).
    let (host, action) = issue(CommandKind::Action).await;
    assert!(host.builtin);
    let client = hosts::builtin::transport::client(&world.state);
    let kept = tenancy::system("runs.host", async move {
        client.ack(action, &answer_of(300 * 1024)).await
    })
    .await;
    assert!(kept.is_ok(), "{kept:?}");
    let answer = tenancy::system("runs.host", commands::answer_of(&world.pool, action))
        .await
        .expect("the answer")
        .expect("an answer");
    assert_eq!(answer.outcome, Some(AckOutcome::Done));

    // Past a mebibyte it isn't, and no other command takes past 64 KiB.
    let (host, action) = issue(CommandKind::Action).await;
    let refused = ack(host, action, 1100 * 1024).await;
    assert!(
        matches!(
            refused,
            Err(HostError::Invalid {
                field: "result",
                ..
            })
        ),
        "{refused:?}"
    );
    let (host, inspect) = issue(CommandKind::Inspect).await;
    let refused = ack(host, inspect, 100 * 1024).await;
    assert!(
        matches!(
            refused,
            Err(HostError::Invalid {
                field: "result",
                ..
            })
        ),
        "{refused:?}"
    );
}
