//! Old and new checks side by side (K1 §4.3). Each area moves in three
//! steps, set per area by `AGENT_WORKSPACE_ACCESS_<AREA>=old|compare|new`
//! at start: `old` runs only the old check; `compare` lets the old check
//! decide and runs `access` beside it, recording any difference in
//! `access_disagreements`; `new` lets `access` decide.
//!
//! A difference is logged (`access.disagreement`, ids and keys only) and
//! counted on one row per fingerprint. Recording never fails a request: an
//! error is logged and the old answer stands.

use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::{Caller, Denied, Grant, Need, Target};

/// How an area decides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Old,
    Compare,
    New,
}

impl Mode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "old" => Some(Self::Old),
            "compare" => Some(Self::Compare),
            "new" => Some(Self::New),
            _ => None,
        }
    }
}

/// The areas that have a switch, with the mode each starts in. An area's
/// slice adds its line; the flip to `new` is its own change.
pub const AREAS: &[(&str, Mode)] = &[
    // K1.2: the operation dispatcher (`ops::dispatch`).
    ("ops", Mode::Compare),
];

/// What one comparison was about, for the record.
pub struct Compared<'a> {
    pub area: &'static str,
    pub operation: &'a str,
    pub caller: &'a Caller,
    pub need: &'a Need,
    pub target: &'a Target,
    pub workspace_id: Option<Uuid>,
    pub space_id: Option<Uuid>,
    pub old_allowed: bool,
}

/// Compare the old answer with the new one; record a difference.
pub async fn compare(pool: &PgPool, compared: Compared<'_>, new: &Result<Grant, Denied>) {
    let new_allowed = new.is_ok();
    if new_allowed == compared.old_allowed {
        return;
    }
    let reason = match new {
        Ok(_) => "allowed".to_owned(),
        Err(denied) => denied.key().to_owned(),
    };
    let permission = compared
        .need
        .permission
        .map_or("(see)", |permission| permission.key());
    let (target_kind, target_id) = compared.target.parts();
    tracing::warn!(
        target: "access.disagreement",
        area = compared.area,
        operation = compared.operation,
        account_id = %compared.caller.account_id,
        permission,
        target_kind,
        old_allowed = compared.old_allowed,
        new_allowed,
        reason = %reason,
        "old and new access checks disagree"
    );
    if let Err(error) = record(
        pool,
        &compared,
        permission,
        target_kind,
        target_id,
        new_allowed,
        &reason,
    )
    .await
    {
        tracing::warn!(error = %error, "could not record an access disagreement");
    }
}

/// The fingerprint one row counts: the same area, operation, account,
/// permission, kind of target and answers.
pub fn fingerprint(
    area: &str,
    operation: &str,
    account_id: Uuid,
    permission: &str,
    target_kind: &str,
    old_allowed: bool,
    new_allowed: bool,
) -> Vec<u8> {
    let mut hash = Sha256::new();
    for part in [
        area,
        operation,
        &account_id.to_string(),
        permission,
        target_kind,
    ] {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    hash.update([u8::from(old_allowed), u8::from(new_allowed)]);
    hash.finalize().to_vec()
}

async fn record(
    pool: &PgPool,
    compared: &Compared<'_>,
    permission: &str,
    target_kind: &str,
    target_id: Option<Uuid>,
    new_allowed: bool,
    reason: &str,
) -> Result<(), sqlx::Error> {
    let fingerprint = fingerprint(
        compared.area,
        compared.operation,
        compared.caller.account_id,
        permission,
        target_kind,
        compared.old_allowed,
        new_allowed,
    );
    sqlx::query(
        r#"
        INSERT INTO access_disagreements
            (id, area, operation, workspace_id, space_id, account_id, account_kind, permission,
             target_kind, target_id, old_allowed, new_allowed, new_reason, fingerprint)
        VALUES ($1, $2, $3, $4, $5, $6, (SELECT kind FROM actors WHERE id = $6), $7,
                $8, $9, $10, $11, $12, $13)
        ON CONFLICT (fingerprint) DO UPDATE
        SET count = access_disagreements.count + 1, last_seen_at = now(),
            target_id = EXCLUDED.target_id
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(compared.area)
    .bind(compared.operation)
    .bind(compared.workspace_id)
    .bind(compared.space_id)
    .bind(compared.caller.account_id)
    .bind(permission)
    .bind(target_kind)
    .bind(target_id)
    .bind(compared.old_allowed)
    .bind(new_allowed)
    .bind(reason)
    .bind(fingerprint)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse() {
        assert_eq!(Mode::parse("compare"), Some(Mode::Compare));
        assert_eq!(Mode::parse(" new"), Some(Mode::New));
        assert_eq!(Mode::parse("sometimes"), None);
    }

    #[test]
    fn the_fingerprint_separates_answers() {
        let account = Uuid::nil();
        let one = fingerprint(
            "ops",
            "task_create",
            account,
            "tasks.create",
            "space",
            true,
            false,
        );
        let two = fingerprint(
            "ops",
            "task_create",
            account,
            "tasks.create",
            "space",
            false,
            true,
        );
        assert_ne!(one, two);
        assert_eq!(
            one,
            fingerprint(
                "ops",
                "task_create",
                account,
                "tasks.create",
                "space",
                true,
                false
            )
        );
    }
}
