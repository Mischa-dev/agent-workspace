//! H1.16: a backup's dump and a master key rotation exclude each other
//! through the keyring lease, a row with an expiry rather than a session
//! advisory lock (rule 14). While another process holds it, both are
//! refused; once it has expired, they go ahead. (`backup_restore` takes over
//! an expired lease with a whole backup, where pg_dump is installed.)

use std::collections::BTreeMap;

use agent_workspace_server::{
    backup,
    config::AppConfig,
    error::AppError,
    maintenance_lease::{KEYRING, Lease, LeaseError},
    secrets::{self, FileKeyring, KeyEncryption as _},
};
use sqlx::PgPool;
use uuid::Uuid;

use crate::support;

/// Another process's hold on the keyring lease, for `seconds` from now
/// (negative: expired that long ago).
async fn held_elsewhere(pool: &PgPool, seconds: f64) -> Uuid {
    let holder = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO maintenance_leases (name, holder, acquired_at, expires_at)
        VALUES ($1, $2, now() - interval '2 minutes', now() + make_interval(secs => $3))
        ON CONFLICT (name) DO UPDATE
        SET holder = EXCLUDED.holder, acquired_at = EXCLUDED.acquired_at,
            expires_at = EXCLUDED.expires_at
        "#,
    )
    .bind(KEYRING)
    .bind(holder)
    .bind(seconds)
    .execute(pool)
    .await
    .expect("hold the lease");
    holder
}

#[tokio::test]
async fn the_keyring_lease_refuses_a_second_holder_until_it_expires() {
    let Some(db) = support::db::db("maintenance_lease").await else {
        return;
    };
    let keys = tempfile::tempdir().expect("key directory");
    let keyring_path = keys.path().join("secrets.keyring");
    secrets::write_new_keyring(&keyring_path, &FileKeyring::generate()).expect("keyring");
    let data_root = tempfile::tempdir().expect("data root");
    let config = AppConfig::from_values(&BTreeMap::from([
        ("AGENT_WORKSPACE_DATABASE_URL".to_owned(), db.url.clone()),
        (
            "AGENT_WORKSPACE_DATA_ROOT".to_owned(),
            data_root.path().display().to_string(),
        ),
        (
            "AGENT_WORKSPACE_SECRETS_KEY_PATH".to_owned(),
            keyring_path.to_string_lossy().into_owned(),
        ),
    ]))
    .expect("config");

    // One process holds it: a second take is refused, and so are a key
    // rotation and a backup.
    let lease = Lease::take(&db.pool, KEYRING).await.expect("take");
    assert!(matches!(
        Lease::take(&db.pool, KEYRING).await,
        Err(LeaseError::Held { .. })
    ));
    assert!(matches!(
        secrets::rotate_master_key(&db.pool, &config).await,
        Err(AppError::Conflict("keyring", _))
    ));
    let sets = tempfile::tempdir().expect("backup folder");
    let options = backup::CreateOptions {
        to: sets.path().to_owned(),
        recipients: None,
        data_paths: Vec::new(),
    };
    let refused = backup::create(&config, &options)
        .await
        .expect_err("a backup while the lease is held");
    assert!(
        format!("{refused:#}").contains("keyring lease"),
        "{refused:#}"
    );
    assert_eq!(
        std::fs::read_dir(sets.path()).expect("sets").count(),
        0,
        "a refused backup leaves no set behind"
    );
    lease.renew(&db.pool).await.expect("the holder renews");
    lease.release(&db.pool).await.expect("release");

    // Released, the rotation goes ahead and gives it back.
    let before = secrets::read_keyring(&keyring_path)
        .expect("keyring")
        .active_key_id()
        .to_owned();
    let rotated = secrets::rotate_master_key(&db.pool, &config)
        .await
        .expect("rotate after release");
    assert_ne!(rotated.active_key_id, before);

    // Held by a process that is still within its time: refused.
    held_elsewhere(&db.pool, 60.0).await;
    assert!(matches!(
        secrets::rotate_master_key(&db.pool, &config).await,
        Err(AppError::Conflict("keyring", _))
    ));
    assert!(backup::create(&config, &options).await.is_err());

    // Its holder died and the lease expired: the next taker goes ahead, and
    // the old holder can no longer renew it.
    let stale = Lease::take(&db.pool, "test.stale").await.expect("take");
    sqlx::query(
        "UPDATE maintenance_leases SET acquired_at = now() - interval '2 minutes', \
         expires_at = now() - interval '1 second' WHERE name = 'test.stale'",
    )
    .execute(&db.pool)
    .await
    .expect("expire");
    assert!(matches!(
        stale.renew(&db.pool).await,
        Err(LeaseError::Lost("test.stale"))
    ));
    Lease::take(&db.pool, "test.stale")
        .await
        .expect("take an expired lease");
    held_elsewhere(&db.pool, -1.0).await;
    secrets::rotate_master_key(&db.pool, &config)
        .await
        .expect("rotate once the lease expired");
    let left =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM maintenance_leases WHERE name = $1")
            .bind(KEYRING)
            .fetch_one(&db.pool)
            .await
            .expect("count");
    assert_eq!(left, 0, "the rotation releases the lease");

    db.finish().await;
}
