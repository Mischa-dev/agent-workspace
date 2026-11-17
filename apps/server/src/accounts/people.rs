//! One way to create a person (K1.29). Bootstrap, both kinds of invitation,
//! the identity provider's sign-up and unclaimed accounts all make a person
//! here, so a person account always has the same rows: the account (kind
//! `person`, with who made it) and its sign-in record (`users`). Ways to sign
//! in (a password, a provider binding) are added by the caller.

use sqlx::PgConnection;
use uuid::Uuid;

/// A person to make.
#[derive(Clone, Copy, Debug)]
pub struct NewPerson<'a> {
    pub login_name: &'a str,
    pub display_name: &'a str,
    /// The workspace they land in after signing in, if any.
    pub default_workspace: Option<Uuid>,
    /// Who made the account: the inviter, the admin who added them, or
    /// `None` when they signed themselves up.
    pub created_by: Option<Uuid>,
    /// False for an account made for someone who hasn't signed in yet (an
    /// imported or added person), claimed later through a one-time link.
    pub claimed: bool,
}

/// The rows made for a person.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Person {
    pub account_id: Uuid,
    pub user_id: Uuid,
}

/// Make a person account and its sign-in record. Returns the database
/// error as is, so each caller maps a taken login name its own way.
pub async fn create_person(
    connection: &mut PgConnection,
    new: NewPerson<'_>,
) -> Result<Person, sqlx::Error> {
    let account_id = Uuid::now_v7();
    let user_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO actors (id, kind, account_kind, status, created_by_actor_id) \
         VALUES ($1, 'human', 'person', 'active', $2)",
    )
    .bind(account_id)
    .bind(new.created_by)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO users (id, actor_id, login_name, display_name, default_organization_id, claimed_at)
        VALUES ($1, $2, $3, $4, $5, CASE WHEN $6 THEN now() END)
        "#,
    )
    .bind(user_id)
    .bind(account_id)
    .bind(new.login_name)
    .bind(new.display_name)
    .bind(new.default_workspace)
    .bind(new.claimed)
    .execute(&mut *connection)
    .await?;
    Ok(Person {
        account_id,
        user_id,
    })
}
