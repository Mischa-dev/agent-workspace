//! A1.17 step 4: a question about a model account goes to the built-in host
//! as a command, and comes back in its acknowledgement. Here the server
//! asking doesn't carry the host itself (as when the host runs as its own
//! program): each question waits for the host's loop to answer it. An API
//! key travels sealed beside its sign-in and is taken once; no command keeps
//! it, and an answered question is gone once read.

use std::{sync::Arc, time::Duration};

use agent_workspace_host_protocol::account::{
    AccountAnswer, AccountAsk, AccountQuestion, LoginStarted,
};
use agent_workspace_server::{
    AppState,
    harness::manifest::LoginKind,
    hosts::builtin::{
        accounts::{self, Asked},
        driver,
    },
    provider::SupervisorRegistry,
    secrets,
    sign_ins::{self, LoginReservation},
    tenancy,
};
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
