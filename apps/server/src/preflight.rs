//! `preflight`: what the release script runs with a new build, against the live
//! service configuration and database, before it switches `current` to it.
//!
//! It keeps nothing. It loads the configuration (which already refuses a
//! keyring inside a Git checkout), opens the secrets keyring, looks for the
//! harness CLIs, says whether email is set up and how the last nightly backup
//! went, checks free disk, and compares the build's migrations with the ones
//! the database has run. Settings > Workspace > Server shows the same report.
//! It never prints a configuration value or secret.
//!
//! For a release it then rehearses the migrations still to run ([`rehearse`])
//! on the database's own data, so one that the data stops is found while the
//! old release is still serving, not at the new one's first start.

use sqlx::{PgPool, migrate::Migration};

use crate::{config::AppConfig, secrets, storage_admission};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Default)]
pub struct Report {
    pub lines: Vec<(Level, String)>,
}

impl Report {
    fn push(&mut self, level: Level, line: impl Into<String>) {
        self.lines.push((level, line.into()));
    }

    pub fn failed(&self) -> bool {
        self.lines.iter().any(|(level, _)| *level == Level::Fail)
    }

    pub fn print(&self) {
        for (level, line) in &self.lines {
            let tag = match level {
                Level::Ok => "ok  ",
                Level::Warn => "warn",
                Level::Fail => "FAIL",
            };
            println!("{tag} {line}");
        }
    }
}

pub async fn run(config: &AppConfig, pool: &PgPool) -> anyhow::Result<Report> {
    let mut report = Report::default();
    report.push(Level::Ok, "configuration loads");

    match &config.secrets_key_path {
        Some(path) => match secrets::read_keyring(path) {
            Ok(keyring) => report.push(
                Level::Ok,
                format!("secrets keyring opens ({} key(s))", keyring.key_ids().len()),
            ),
            Err(_) => report.push(
                Level::Fail,
                "secrets keyring is configured but can't be read by the service user",
            ),
        },
        None if config.environment.is_development() => report.push(
            Level::Ok,
            "development build: keyring is created on first use",
        ),
        None => report.push(
            Level::Warn,
            "no secrets keyring (AGENT_WORKSPACE_SECRETS_KEY_PATH): two-factor, AI provider keys, \
             push and webhook secrets won't work; see docs/operations/secret-rotation.md",
        ),
    }

    for (name, version) in crate::hosts::builtin::preflight::harness_programs(config).await {
        match version {
            Some(version) => report.push(Level::Ok, format!("{name} CLI: {version}")),
            None => report.push(
                Level::Warn,
                format!(
                    "{name} CLI not found or not runnable; {name} runs on this server won't start"
                ),
            ),
        }
    }

    report.push(
        Level::Ok,
        if config.github.is_some() {
            "GitHub App is configured"
        } else {
            "GitHub App isn't configured (projects can't connect repositories from GitHub)"
        },
    );
    report.push(
        Level::Ok,
        format!(
            "{} trusted webhook origin(s)",
            config.webhook_trusted_origins.len()
        ),
    );

    // H1.15: what the release feed last named, when newer than this build.
    if migrations_table_exists(pool).await?
        && sqlx::query_scalar::<_, bool>("SELECT to_regclass('server_release_checks') IS NOT NULL")
            .fetch_one(crate::db::scoped(pool))
            .await?
        && let Some(newer) = crate::releases::newer(pool).await?
    {
        report.push(
            Level::Warn,
            format!(
                "release {} is out (running {}){}",
                newer.version,
                crate::releases::RUNNING,
                newer
                    .notes_url
                    .map(|notes| format!("; notes: {notes}"))
                    .unwrap_or_default()
            ),
        );
    }

    let mail_workspaces = if migrations_table_exists(pool).await?
        && sqlx::query_scalar::<_, bool>(
            "SELECT to_regclass('workspace_mail_settings') IS NOT NULL",
        )
        .fetch_one(crate::db::scoped(pool))
        .await?
    {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workspace_mail_settings WHERE enabled")
            .fetch_one(crate::db::scoped(pool))
            .await?
    } else {
        0
    };
    report.push(
        Level::Ok,
        match (&config.server_mail, mail_workspaces) {
            (Some(mail), count) => format!(
                "account email goes out through the server's own transport ({}:{}); {count} \
                 workspace(s) send their own notification email",
                mail.smtp.host, mail.smtp.port
            ),
            (None, 0) => "email isn't set up (no password reset emails or email notifications); \
                  set AGENT_WORKSPACE_MAIL_URL, or an owner sets it in Settings > Workspace > Email"
                .to_owned(),
            (None, count) => format!("{count} workspace(s) send email"),
        },
    );
    let (level, line) = crate::server_status::backup_line(&config.data_root);
    report.push(level, line);

    match storage_admission::inspect(
        &config.data_root,
        config.data_quota_bytes,
        config.minimum_free_bytes,
    ) {
        Ok(storage) if storage.admitted => report.push(
            Level::Ok,
            format!("{} GiB free for app data", storage.available_bytes >> 30),
        ),
        Ok(storage) => report.push(
            Level::Fail,
            format!(
                "app data is over its quota or the disk is nearly full ({} GiB free)",
                storage.available_bytes >> 30
            ),
        ),
        Err(_) => report.push(Level::Warn, "couldn't measure the app data folder"),
    }

    let known: Vec<(i64, Vec<u8>, String)> = migrator()
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| {
            (
                migration.version,
                migration.checksum.to_vec(),
                migration.description.to_string(),
            )
        })
        .collect();
    let applied: Vec<(i64, Vec<u8>)> = if migrations_table_exists(pool).await? {
        sqlx::query_as(
            "SELECT version, checksum FROM _sqlx_migrations WHERE success ORDER BY version",
        )
        .fetch_all(crate::db::scoped(pool))
        .await?
    } else {
        Vec::new()
    };
    let plan = compare_migrations(&known, &applied);
    if !plan.unknown.is_empty() {
        report.push(
            Level::Fail,
            format!(
                "the database ran {} migration(s) this build doesn't have (first: {}); it is older \
                 than what's deployed, so don't switch to it",
                plan.unknown.len(),
                plan.unknown[0]
            ),
        );
    }
    if !plan.changed.is_empty() {
        report.push(
            Level::Fail,
            format!(
                "{} migration(s) changed after they ran (first: {}); the server would refuse to start",
                plan.changed.len(),
                plan.changed[0]
            ),
        );
    }
    match plan.pending.as_slice() {
        [] => report.push(Level::Ok, "no migrations to apply"),
        [only] => report.push(Level::Ok, format!("1 migration to apply: {only}")),
        [first, .., last] => report.push(
            Level::Ok,
            format!(
                "{} migrations to apply, {first} through {last}; back up the database first",
                plan.pending.len()
            ),
        ),
    }
    Ok(report)
}

async fn migrations_table_exists(pool: &PgPool) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(crate::db::scoped(pool))
        .await
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct MigrationPlan {
    /// In this build, not yet run: "<version> <description>".
    pub pending: Vec<String>,
    /// Run with a different checksum than this build's copy.
    pub changed: Vec<String>,
    /// Run, but missing from this build (the build is older than the database).
    pub unknown: Vec<String>,
}

pub fn compare_migrations(
    known: &[(i64, Vec<u8>, String)],
    applied: &[(i64, Vec<u8>)],
) -> MigrationPlan {
    let mut plan = MigrationPlan::default();
    for (version, checksum, description) in known {
        match applied.iter().find(|(applied, _)| applied == version) {
            None => plan.pending.push(format!("{version} {description}")),
            Some((_, ran)) if ran != checksum => {
                plan.changed.push(format!("{version} {description}"))
            }
            Some(_) => {}
        }
    }
    for (version, _) in applied {
        if !known.iter().any(|(known, _, _)| known == version) {
            plan.unknown.push(version.to_string());
        }
    }
    plan
}

/// The migrations this release carries.
fn migrator() -> sqlx::migrate::Migrator {
    sqlx::migrate!()
}

/// How long a rehearsed migration waits for a table the running server is
/// using, before the rehearsal gives up.
const REHEARSAL_LOCK_WAIT: &str = "5s";

/// What a commit would do to the constraints that wait for one, without the
/// commit: every deferrable constraint is checked now, then put back as it
/// was declared, so the next migration starts as it would in a transaction
/// of its own.
const SETTLE_CONSTRAINTS: &str = r"
DO $$
DECLARE
    deferrable_ones text;
    deferred_ones text;
BEGIN
    SELECT string_agg(DISTINCT format('%I.%I', current_schema(), conname), ', '),
           string_agg(DISTINCT format('%I.%I', current_schema(), conname), ', ')
               FILTER (WHERE condeferred)
      INTO deferrable_ones, deferred_ones
      FROM pg_constraint
     WHERE condeferrable AND connamespace = current_schema()::regnamespace;
    IF deferrable_ones IS NOT NULL THEN
        EXECUTE 'SET CONSTRAINTS ' || deferrable_ones || ' IMMEDIATE';
    END IF;
    IF deferred_ones IS NOT NULL THEN
        EXECUTE 'SET CONSTRAINTS ' || deferred_ones || ' DEFERRED';
    END IF;
END $$;
";

/// How a rehearsal of the migrations still to run ended.
#[derive(Debug, Eq, PartialEq)]
pub enum Rehearsal {
    /// Every one applies to this database's data: how many.
    Applies(usize),
    /// This one stops on it, for the database's reason.
    Stops { migration: String, reason: String },
    /// It couldn't be tried here: why.
    NotTried(String),
}

/// What the database says when the rehearsal's transaction is committed.
const NEVER_KEPT: &str = "a rehearsal is never kept";

/// Makes the transaction it runs in impossible to commit. A cursor declared
/// to outlive its transaction is run to its end by the commit, and this
/// one's query fails, so the commit does and everything is rolled back.
/// Whatever a migration says, nothing a rehearsal applied is kept.
const NEVER_COMMITS: &str = "DECLARE a_rehearsal_is_never_kept CURSOR WITH HOLD FOR \
     SELECT ('a rehearsal is never kept' || repeat('', step))::int \
       FROM generate_series(1, 1) AS step";

/// Half the server's lock table. Everything a rehearsed migration touches
/// stays locked until the rollback, where the migrator lets go after each
/// one, and the table is shared: when it is full, the requests the running
/// server is answering fail with "out of shared memory".
const LOCKS_TO_SPARE: &str = "SELECT current_setting('max_locks_per_transaction')::bigint \
            * (current_setting('max_connections')::bigint \
               + current_setting('max_prepared_transactions')::bigint) / 2";

const LOCKS_HELD: &str = "SELECT count(*) FROM pg_locks WHERE pid = pg_backend_pid()";

/// Applies `pending`, in order, to the database's own data in one transaction
/// that is rolled back whatever happens: what the server would do at its next
/// start, without keeping any of it.
///
/// While it runs, the server that is serving waits on the tables a migration
/// changes, as it would on any schema change.
pub async fn rehearse(pool: &PgPool, pending: &[Migration]) -> Result<Rehearsal, sqlx::Error> {
    if let Some(own) = pending
        .iter()
        .find(|migration| migration.no_tx || ends_its_transaction(&migration.sql))
    {
        return Ok(Rehearsal::NotTried(format!(
            "{} runs outside a transaction, so it can't be rolled back",
            name(own)
        )));
    }
    if let Some(prepared) = pending.iter().find(|migration| {
        crate::migration_preflight::PREPARED_FOR_AT_START.contains(&migration.version)
    }) {
        return Ok(Rehearsal::NotTried(format!(
            "{} needs rows the server repairs when it starts, before it migrates",
            name(prepared)
        )));
    }
    let most_locks = sqlx::query_scalar::<_, i64>(LOCKS_TO_SPARE)
        .fetch_one(crate::db::scoped(pool))
        .await?;
    apply_and_roll_back(pool, pending, most_locks).await
}

fn name(migration: &Migration) -> String {
    format!("{} {}", migration.version, migration.description)
}

/// The rehearsal itself, for migrations [`rehearse`] has looked over, holding
/// at most `most_locks` at once. Public so a test can show that a commit the
/// look missed still keeps nothing, and that it stops short of the locks the
/// server needs.
#[doc(hidden)]
pub async fn apply_and_roll_back(
    pool: &PgPool,
    pending: &[Migration],
    most_locks: i64,
) -> Result<Rehearsal, sqlx::Error> {
    let mut transaction = crate::db::scoped_tx(pool).await?;
    // As the migrator runs them: in no tenancy scope, with row rules observing.
    sqlx::query(
        "SELECT set_config('app.tenancy', '', true), set_config('app.tenancy_strict', '', true), \
                set_config('lock_timeout', $1, true)",
    )
    .bind(REHEARSAL_LOCK_WAIT)
    .execute(&mut *transaction)
    .await?;
    sqlx::raw_sql(NEVER_COMMITS)
        .execute(&mut *transaction)
        .await?;
    let mut outcome = Rehearsal::Applies(pending.len());
    for (tried, migration) in pending.iter().enumerate() {
        let applied = async {
            sqlx::raw_sql(&migration.sql)
                .execute(&mut *transaction)
                .await?;
            sqlx::raw_sql(SETTLE_CONSTRAINTS)
                .execute(&mut *transaction)
                .await?;
            sqlx::query_scalar::<_, i64>(LOCKS_HELD)
                .fetch_one(&mut *transaction)
                .await
        }
        .await;
        let error = match applied {
            Ok(held) if held > most_locks && tried + 1 < pending.len() => {
                outcome = Rehearsal::NotTried(format!(
                    "the first {} apply; the {} after them weren't tried, since one transaction \
                     holding them all would take more of the server's locks than it can spare \
                     ({held} held, {most_locks} to spare)",
                    tried + 1,
                    pending.len() - tried - 1
                ));
                break;
            }
            Ok(_) => continue,
            Err(error) => error,
        };
        let sqlx::Error::Database(refusal) = &error else {
            return Err(error);
        };
        outcome = match refusal.code().as_deref() {
            // lock_not_available
            Some("55P03") => Rehearsal::NotTried(format!(
                "{} waited more than {REHEARSAL_LOCK_WAIT} for a table the running server is using",
                name(migration)
            )),
            // A new enum value used in the transaction that adds it, or the
            // server out of room (locks, memory): one transaction for all of
            // them is the rehearsal's doing, not the migration's.
            Some(code) if code == "55P04" || code.starts_with("53") => {
                Rehearsal::NotTried(format!(
                    "{} can't share one transaction with the migrations before it: {}",
                    name(migration),
                    refusal.message()
                ))
            }
            _ if refusal.message().contains(NEVER_KEPT) => Rehearsal::NotTried(format!(
                "{} commits its own transaction, so it can't be rolled back",
                name(migration)
            )),
            // The message only: the detail names the rows' values.
            _ => Rehearsal::Stops {
                migration: name(migration),
                reason: refusal.message().to_owned(),
            },
        };
        break;
    }
    transaction.rollback().await?;
    Ok(outcome)
}

/// Whether a migration's SQL ends the transaction it runs in. A commit would
/// fail ([`NEVER_COMMITS`]); a rollback would let the statements after it run
/// outside any transaction and be kept.
fn ends_its_transaction(sql: &str) -> bool {
    statements(sql).split(';').any(|statement| {
        let mut words = statement.split_whitespace().map(str::to_ascii_uppercase);
        match words.next().as_deref() {
            Some("COMMIT" | "END" | "ABORT") => true,
            Some("ROLLBACK") => words.next().as_deref() != Some("TO"),
            Some("PREPARE") => words.next().as_deref() == Some("TRANSACTION"),
            _ => false,
        }
    })
}

/// `sql` with its comments, quoted text and function bodies blanked, so a
/// `;` left in it ends a statement.
fn statements(sql: &str) -> String {
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let mut bare = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(next) = rest.chars().next() {
        let skipped = if rest.starts_with("--") {
            rest.find('\n').unwrap_or(rest.len())
        } else if rest.starts_with("/*") {
            block_comment(rest)
        } else if next == '\'' || next == '"' {
            // E'...' is the one kind of text where a backslash escapes a quote.
            let mut before = bare.chars().rev();
            let escapes = next == '\''
                && matches!(before.next(), Some('E' | 'e'))
                && !before.next().is_some_and(word);
            quoted(rest, next, escapes)
        } else if let Some(tag) = dollar_tag(rest).filter(|_| !bare.ends_with(word)) {
            let body = &rest[tag.len()..];
            tag.len() + body.find(tag).map_or(body.len(), |end| end + tag.len())
        } else {
            0
        };
        if skipped == 0 {
            bare.push(next);
            rest = &rest[next.len_utf8()..];
        } else {
            bare.push(' ');
            rest = &rest[skipped..];
        }
    }
    bare
}

/// The length of the `/* ... */` comment `text` starts with; they nest.
fn block_comment(text: &str) -> usize {
    let mut depth = 0;
    let mut at = 0;
    while at < text.len() {
        if text[at..].starts_with("/*") {
            depth += 1;
            at += 2;
        } else if text[at..].starts_with("*/") {
            depth -= 1;
            at += 2;
            if depth == 0 {
                return at;
            }
        } else {
            at += text[at..].chars().next().map_or(1, char::len_utf8);
        }
    }
    text.len()
}

/// The length of the quoted text `text` starts with; a doubled quote is part
/// of it.
fn quoted(text: &str, quote: char, backslash_escapes: bool) -> usize {
    let mut chars = text.char_indices().skip(1).peekable();
    while let Some((at, c)) = chars.next() {
        if backslash_escapes && c == '\\' {
            chars.next();
        } else if c == quote {
            if chars.peek().is_some_and(|(_, next)| *next == quote) {
                chars.next();
            } else {
                return at + c.len_utf8();
            }
        }
    }
    text.len()
}

/// The `$tag$` that `text` starts with, if it opens a dollar-quoted body
/// (`$1` is a parameter, not a tag).
fn dollar_tag(text: &str) -> Option<&str> {
    let inside = text.strip_prefix('$')?;
    let end = inside.find('$')?;
    let tag = &inside[..end];
    let named = tag.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !tag.starts_with(|c: char| c.is_ascii_digit());
    named.then(|| &text[..end + 2])
}

/// The report the release script acts on: [`run`], and then, unless that
/// already failed or `rehearsing` is off, the migrations still to run are
/// rehearsed on this database's data.
pub async fn before_a_release(
    config: &AppConfig,
    pool: &PgPool,
    rehearsing: bool,
) -> anyhow::Result<Report> {
    let mut report = run(config, pool).await?;
    if report.failed() || !migrations_table_exists(pool).await? {
        return Ok(report);
    }
    let applied: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success")
            .fetch_all(crate::db::scoped(pool))
            .await?;
    let pending: Vec<Migration> = migrator()
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .filter(|migration| !applied.contains(&migration.version))
        .cloned()
        .collect();
    if pending.is_empty() {
        return Ok(report);
    }
    if !rehearsing {
        report.push(
            Level::Warn,
            "the migrations weren't tried on this database's data (--no-rehearsal)",
        );
        return Ok(report);
    }
    match rehearse(pool, &pending).await? {
        Rehearsal::Applies(count) => report.push(
            Level::Ok,
            format!(
                "they apply to this database's data ({count} rehearsed in a transaction, then \
                 rolled back)"
            ),
        ),
        Rehearsal::Stops { migration, reason } => report.push(
            Level::Fail,
            format!(
                "migration {migration} stops on this database's data: {reason}. Nothing was \
                 changed. Repair the data in a new migration that sorts before it"
            ),
        ),
        Rehearsal::NotTried(why) => report.push(
            Level::Warn,
            format!("the migrations weren't tried on this database's data: {why}"),
        ),
    }
    Ok(report)
}

/// The newest migration this release carries: the schema version it
/// expects (readiness, K5.7).
pub fn expected_schema_version() -> i64 {
    migrator()
        .iter()
        .filter(|migration| !migration.migration_type.is_down_migration())
        .map(|migration| migration.version)
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(version: i64, checksum: u8) -> (i64, Vec<u8>, String) {
        (version, vec![checksum], format!("m{version}"))
    }

    #[test]
    fn sorts_migrations_into_pending_changed_and_unknown() {
        let plan = compare_migrations(
            &[known(1, 1), known(2, 2), known(3, 3)],
            &[(1, vec![1]), (2, vec![9]), (7, vec![7])],
        );
        assert_eq!(plan.pending, vec!["3 m3"]);
        assert_eq!(plan.changed, vec!["2 m2"]);
        assert_eq!(plan.unknown, vec!["7"]);
    }

    #[test]
    fn a_migration_that_ends_its_transaction_is_not_rehearsed() {
        for ends in [
            "CREATE TABLE a (id int);\nCOMMIT;\n",
            "CREATE TABLE a (id int); COMMIT;",
            "CREATE TABLE a (id int); /* done */ end;",
            "  rollback ;",
            "ABORT;",
            "SELECT 'it''s'; COMMIT;",
            "SELECT E'it\\'s'; COMMIT;",
            "DO $$ BEGIN NULL; END $$; COMMIT",
            "PREPARE TRANSACTION 'later';",
        ] {
            assert!(ends_its_transaction(ends), "{ends}");
        }
        for stays in [
            "SAVEPOINT a;\nROLLBACK TO a;\n-- COMMIT;\nCREATE TABLE b (\n    commit_sha TEXT,\n    \
             committed_at TIMESTAMPTZ\n);",
            "DO $$ BEGIN\n    IF true THEN NULL; END IF;\nEND $$;",
            "CREATE FUNCTION f() RETURNS void LANGUAGE plpgsql AS $body$\nBEGIN\n    NULL;\nEND;\n\
             $body$;",
            "SELECT '; COMMIT;', \"end;\" FROM t; /* ; COMMIT; /* nested */ ; ABORT; */",
            "SELECT 'C:\\'; SELECT 'COMMIT';",
            "PREPARE find AS SELECT $1::int; CREATE TABLE price$ (id int);",
        ] {
            assert!(!ends_its_transaction(stays), "{stays}");
        }
    }

    #[test]
    fn no_migration_here_is_taken_for_one_that_ends_its_transaction() {
        let mistaken: Vec<String> = migrator()
            .iter()
            .filter(|migration| !migration.no_tx && ends_its_transaction(&migration.sql))
            .map(name)
            .collect();
        assert_eq!(mistaken, Vec::<String>::new());
    }

    #[test]
    fn an_up_to_date_database_has_nothing_to_do() {
        let plan = compare_migrations(&[known(1, 1)], &[(1, vec![1])]);
        assert_eq!(plan, MigrationPlan::default());
    }
}
