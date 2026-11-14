//! H1.16: a backup set made by `backup create` restores into a new database
//! and data root that read the same messages and files, and `restore`
//! refuses a target that already holds anything. A backup that can't be
//! restored is found out on the worst possible day.

use std::{collections::BTreeMap, path::Path, process::Command};

use agent_workspace_server::{backup, config::AppConfig, seed};
use sqlx::{Executor, PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

/// The major version of a client program, from `<program> --version`.
fn client_major(program: &str) -> Option<u32> {
    let output = Command::new(program).arg("--version").output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    text.split_whitespace()
        .find(|word| word.starts_with(|c: char| c.is_ascii_digit()))?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

fn database_url(base: &str, name: &str) -> String {
    let mut url = url::Url::parse(base).expect("database URL");
    url.set_path(name);
    url.to_string()
}

async fn connect(url: &str) -> PgPool {
    PgPoolOptions::new()
        .max_connections(2)
        .connect(url)
        .await
        .expect("connect")
}

async fn messages(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar::<_, String>("SELECT plain_text FROM messages ORDER BY id")
        .fetch_all(pool)
        .await
        .expect("read messages")
}

#[tokio::test]
async fn a_set_restores_into_an_empty_target_and_refuses_a_full_one() {
    let Ok(base) = std::env::var("AGENT_WORKSPACE_DATABASE_URL") else {
        eprintln!("skipping backup test: AGENT_WORKSPACE_DATABASE_URL is unset");
        return;
    };
    let admin = connect(&base).await;
    let server_major = sqlx::query_scalar::<_, String>("SHOW server_version_num")
        .fetch_one(&admin)
        .await
        .expect("server version")
        .parse::<u32>()
        .expect("version number")
        / 10_000;
    let usable = |program| client_major(program).is_some_and(|major| major >= server_major);
    if !usable("pg_dump") || !usable("pg_restore") || client_major("tar").is_none() {
        eprintln!(
            "skipping backup test: no pg_dump/pg_restore for PostgreSQL {server_major}, or no tar"
        );
        return;
    }

    let suffix = Uuid::now_v7().simple().to_string();
    let names = ["source", "target", "drill"].map(|role| format!("aw_backup_{role}_{suffix}"));
    for name in &names {
        admin
            .execute(format!("CREATE DATABASE {name}").as_str())
            .await
            .expect("create a database");
    }
    let [source_url, target_url, drill_url] = names.clone().map(|name| database_url(&base, &name));

    // A seeded workspace with messages, and files in its data root.
    let source = connect(&source_url).await;
    sqlx::migrate!("./migrations")
        .run(&source)
        .await
        .expect("apply migrations");
    let data_root = tempfile::tempdir().expect("data root");
    let config = AppConfig::from_values(&BTreeMap::from([
        (
            "AGENT_WORKSPACE_DATABASE_URL".to_owned(),
            source_url.clone(),
        ),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            data_root.path().display().to_string(),
        ),
    ]))
    .expect("config");
    let seeded = seed::dev_seed(&source, &config).await.expect("seed");
    for text in ["first message", "second message"] {
        sqlx::query(
            r#"
            INSERT INTO messages (
                id, project_id, conversation_id, author_actor_id,
                content_version, content, plain_text
            )
            VALUES ($1, $2, $3, $4, 1,
                    jsonb_build_object('version', 1, 'nodes',
                        jsonb_build_array(jsonb_build_object('type', 'text', 'text', $5::text))),
                    $5)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(seeded.project_id)
        .bind(seeded.channel_id)
        .bind(seeded.human_actor_id)
        .bind(text)
        .execute(&source)
        .await
        .expect("seed a message");
    }
    let attachment = Path::new("attachments")
        .join(seeded.project_id.to_string())
        .join("note.txt");
    std::fs::create_dir_all(data_root.path().join(attachment.parent().expect("folder")))
        .expect("attachment folder");
    std::fs::write(data_root.path().join(&attachment), b"kept across a restore").expect("file");
    std::fs::create_dir_all(data_root.path().join("files")).expect("files folder");
    std::fs::write(data_root.path().join("files/page.bin"), [0_u8, 1, 2, 255]).expect("file");
    let seeded_messages = messages(&source).await;
    assert_eq!(seeded_messages.len(), 2);
    source.close().await;

    let sets = tempfile::tempdir().expect("backup folder");
    let set = backup::create(
        &config,
        &backup::CreateOptions {
            to: sets.path().to_owned(),
            recipients: None,
            data_paths: backup::DEFAULT_DATA_PATHS
                .iter()
                .map(|path| (*path).to_owned())
                .collect(),
        },
    )
    .await
    .expect("create a backup set");
    assert!(!set.encrypted);
    assert!(set.data_paths.contains(&"attachments".to_owned()));
    let status = std::fs::read_to_string(
        agent_workspace_server::server_status::backup_status_path(data_root.path()),
    )
    .expect("status file");
    assert!(status.contains("\"ok\":true"), "{status}");

    // Restored into a new database and directory, it reads the same.
    let restored_root = tempfile::tempdir().expect("restore root");
    let report = backup::restore(&set.path, &target_url, restored_root.path(), None)
        .await
        .expect("restore");
    assert_eq!(report.messages, 2);
    assert_eq!(report.migration_version, set.migration_version);
    let target = connect(&target_url).await;
    assert_eq!(messages(&target).await, seeded_messages);
    assert_eq!(
        std::fs::read(restored_root.path().join(&attachment)).expect("restored file"),
        b"kept across a restore"
    );
    assert_eq!(
        std::fs::read(restored_root.path().join("files/page.bin")).expect("restored file"),
        [0_u8, 1, 2, 255]
    );
    target.close().await;

    // A target that holds anything is refused: the database now does, and
    // so does a data root with a file in it.
    let empty_root = tempfile::tempdir().expect("empty root");
    let refused = backup::restore(&set.path, &target_url, empty_root.path(), None)
        .await
        .expect_err("a full database is refused");
    assert!(refused.to_string().contains("isn't empty"), "{refused}");
    let refused = backup::restore(&set.path, &drill_url, restored_root.path(), None)
        .await
        .expect_err("a full data root is refused");
    assert!(refused.to_string().contains("isn't empty"), "{refused}");

    // A changed file fails its checksum before anything is restored.
    let tampered = tempfile::tempdir().expect("tampered copy");
    for entry in std::fs::read_dir(&set.path).expect("set") {
        let entry = entry.expect("entry");
        std::fs::copy(entry.path(), tampered.path().join(entry.file_name())).expect("copy");
    }
    std::fs::write(tampered.path().join("backup-info"), "format=1\n").expect("tamper");
    assert!(
        backup::drill(tampered.path(), &drill_url, None, None)
            .await
            .is_err()
    );

    // The drill restores into a spare database and checks it; with `age`
    // installed, from an encrypted set.
    let identity = sets.path().join("identity.txt");
    let encrypted = Command::new("age-keygen")
        .arg("-o")
        .arg(&identity)
        .output()
        .is_ok_and(|output| output.status.success());
    let drilled = if encrypted {
        let public = Command::new("age-keygen")
            .arg("-y")
            .arg(&identity)
            .output()
            .expect("public key");
        let recipients = sets.path().join("recipients.txt");
        std::fs::write(&recipients, &public.stdout).expect("recipients");
        let encrypted_sets = tempfile::tempdir().expect("encrypted sets");
        let set = backup::create(
            &config,
            &backup::CreateOptions {
                to: encrypted_sets.path().to_owned(),
                recipients: Some(recipients),
                data_paths: vec!["attachments".to_owned()],
            },
        )
        .await
        .expect("create an encrypted set");
        assert!(set.encrypted);
        assert!(
            backup::drill(&set.path, &drill_url, None, None)
                .await
                .is_err(),
            "an encrypted set needs its identity"
        );
        backup::drill(&set.path, &drill_url, Some(&identity), None).await
    } else {
        backup::drill(&set.path, &drill_url, None, None).await
    };
    let drilled = drilled.expect("drill");
    assert_eq!(drilled.messages, 2);
    assert!(drilled.data_entries > 0);

    for name in &names {
        admin
            .execute(format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)").as_str())
            .await
            .expect("drop a test database");
    }
    admin.close().await;
}
