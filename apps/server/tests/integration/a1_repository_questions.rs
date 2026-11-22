//! A1.18 follow-up: repository work on the built-in host's disk is asked of
//! the host as commands. Here the server asking doesn't carry the host
//! itself (as when the host runs as its own program): each question waits
//! for the host's loop to answer it. A git token travels sealed beside its
//! command and is taken when the host fetches it; no row keeps it after.

use std::{sync::Arc, time::Duration};

use agent_workspace_server::{
    AppState,
    hosts::builtin::{
        driver,
        repositories::{self, Publication, PublicationCheck},
    },
    tenancy,
};
use secrecy::SecretString;
use uuid::Uuid;

use crate::support::two_workspaces::two_workspaces;

const TOKEN: &str = "ghs_sealed-demo-token-7313";

/// Run `asked` as a server that doesn't carry the host, while the host's
/// loop (`host`) answers what it finds; `before_answering` runs once the
/// question is written, before the host first looks.
async fn with_host<T>(
    host: &AppState,
    asked: impl std::future::Future<Output = T>,
    before_answering: impl std::future::Future<Output = ()>,
) -> T {
    let answering = async {
        before_answering.await;
        loop {
            tenancy::system("runs.host", driver::pump(host))
                .await
                .expect("the host's loop");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::select! {
        asked = tenancy::system("runs.host", asked) => asked,
        () = answering => unreachable!(),
    }
}

#[tokio::test]
async fn repository_work_goes_to_the_host_and_tokens_travel_sealed() {
    let Some(world) = two_workspaces("a1_repository_questions").await else {
        return;
    };
    let host = world.state.clone();
    let mut remote_config = (*world.state.config).clone();
    remote_config.builtin_host = false;
    let asking = AppState {
        config: Arc::new(remote_config),
        ..world.state.clone()
    };
    let workspace = world.a.organization;

    // Removal: the host removes the repository's bytes from its own disk.
    let repository = Uuid::now_v7();
    let mirror = host
        .config
        .data_root
        .join("repositories")
        .join(repository.to_string());
    std::fs::create_dir_all(mirror.join("objects")).expect("a mirror on the host's disk");
    with_host(
        &host,
        repositories::remove(&asking, workspace, repository),
        async {},
    )
    .await
    .expect("the host removes the repository");
    assert!(!mirror.exists(), "the mirror is gone from the host's disk");

    // A worktree the host doesn't keep where the server says is `Moved`.
    let publication = Publication {
        workspace_id: workspace,
        repository_id: repository,
        repository_workspace_id: Uuid::now_v7(),
        worktree_path: "/elsewhere/worktree",
        branch: "aw/run",
        base_commit: "0000000000000000000000000000000000000000",
    };
    let checked = with_host(&host, publication.check(&asking), async {})
        .await
        .expect("the host checks the branch");
    assert!(matches!(checked, PublicationCheck::Moved));

    // A push's token is sealed beside its command until the host fetches it.
    let pool = world.pool.clone();
    let sealed_seen = async {
        loop {
            let sealed = sqlx::query_as::<_, (Vec<u8>,)>(
                "SELECT ciphertext FROM host_command_secrets WHERE workspace_id = $1",
            )
            .bind(workspace)
            .fetch_all(&pool)
            .await
            .expect("the sealed tokens");
            if let Some((ciphertext,)) = sealed.first() {
                assert_eq!(sealed.len(), 1);
                assert!(
                    !String::from_utf8_lossy(ciphertext).contains(TOKEN),
                    "the token is sealed"
                );
                let bodies = sqlx::query_scalar::<_, String>(
                    "SELECT body::text FROM host_commands WHERE workspace_id = $1",
                )
                .bind(workspace)
                .fetch_all(&pool)
                .await
                .expect("the commands");
                assert!(bodies.iter().all(|body| !body.contains(TOKEN)));
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let pushed = with_host(
        &host,
        publication.push(
            &asking,
            "1111111111111111111111111111111111111111",
            "https://github.com/example/repository.git",
            true,
            SecretString::from(TOKEN.to_owned()),
        ),
        sealed_seen,
    )
    .await
    .expect("the host answers the push");
    assert!(
        !pushed,
        "nothing is pushed from a worktree the host doesn't keep"
    );
    let left = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM host_command_secrets WHERE workspace_id = $1",
    )
    .bind(workspace)
    .fetch_one(&world.pool)
    .await
    .expect("count sealed tokens");
    assert_eq!(left, 0, "the host took the token, and no row keeps it");
    let questions = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM host_commands WHERE workspace_id = $1 AND body->>'action' = 'repository'",
    )
    .bind(workspace)
    .fetch_one(&world.pool)
    .await
    .expect("count questions");
    assert_eq!(questions, 0, "answered questions are gone once read");
}
