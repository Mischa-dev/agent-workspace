use std::io::IsTerminal;

use serde::Serialize;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{auth::password, error::AppError};

const MIN_PASSWORD_BYTES: usize = 12;
const MAX_PASSWORD_BYTES: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapCommand {
    pub login_name: String,
    pub display_name: String,
    pub organization_name: Option<String>,
    pub organization_slug: Option<String>,
    pub adopt_actor_id: Option<Uuid>,
    pub organization_id: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapResult {
    pub user_id: Uuid,
    pub actor_id: Uuid,
    pub organization_id: Uuid,
    pub adopted_existing_actor: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PasswordRotationCommand {
    pub login_name: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordRotationResult {
    pub user_id: Uuid,
    pub authentication_epoch: i64,
    pub revoked_sessions: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("{0}")]
    Usage(&'static str),
    #[error("administrator bootstrap has already completed")]
    AlreadyBootstrapped,
    #[error(
        "existing M0 ownership is ambiguous; pass --adopt-actor and --organization-id explicitly"
    )]
    AmbiguousAdoption,
    #[error("the selected Actor and Organization are not an eligible active M0 ownership pair")]
    InvalidAdoption,
    #[error("the selected Actor is not an eligible active development member")]
    InvalidMemberAdoption,
    #[error("the selected Actor already has a User account")]
    MemberAlreadyProvisioned,
    #[error("bootstrap requires an interactive terminal")]
    InteractiveTerminalRequired,
    #[error("the password entries did not match")]
    PasswordMismatch,
    #[error("the active User or password credential does not exist")]
    PasswordCredentialUnavailable,
    #[error("bootstrap application operation failed")]
    Application(#[from] AppError),
    #[error("bootstrap database operation failed")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, FromRow)]
struct AdoptionCandidate {
    actor_id: Uuid,
    organization_id: Uuid,
}

pub fn parse_command(
    arguments: impl IntoIterator<Item = String>,
) -> Result<BootstrapCommand, BootstrapError> {
    let mut arguments = arguments.into_iter();
    let mut login_name = None;
    let mut display_name = None;
    let mut organization_name = None;
    let mut organization_slug = None;
    let mut adopt_actor_id = None;
    let mut organization_id = None;

    while let Some(argument) = arguments.next() {
        let value = arguments.next().ok_or(BootstrapError::Usage(
            "every bootstrap option requires a value",
        ))?;
        match argument.as_str() {
            "--login-name" => login_name = Some(value),
            "--display-name" => display_name = Some(value),
            "--organization-name" => organization_name = Some(value),
            "--organization-slug" => organization_slug = Some(value),
            "--adopt-actor" => {
                adopt_actor_id = Some(
                    value
                        .parse()
                        .map_err(|_| BootstrapError::Usage("--adopt-actor must be a UUID"))?,
                );
            }
            "--organization-id" => {
                organization_id = Some(
                    value
                        .parse()
                        .map_err(|_| BootstrapError::Usage("--organization-id must be a UUID"))?,
                );
            }
            _ => return Err(BootstrapError::Usage("unknown bootstrap option")),
        }
    }

    if adopt_actor_id.is_some() != organization_id.is_some() {
        return Err(BootstrapError::Usage(
            "--adopt-actor and --organization-id must be supplied together",
        ));
    }
    Ok(BootstrapCommand {
        login_name: normalize_login_name(
            login_name.ok_or(BootstrapError::Usage("--login-name is required"))?,
        )?,
        display_name: normalize_display_name(
            display_name.ok_or(BootstrapError::Usage("--display-name is required"))?,
        )?,
        organization_name: organization_name
            .map(normalize_organization_name)
            .transpose()?,
        organization_slug: organization_slug
            .map(normalize_organization_slug)
            .transpose()?,
        adopt_actor_id,
        organization_id,
    })
}

pub fn parse_member_command(
    arguments: impl IntoIterator<Item = String>,
) -> Result<BootstrapCommand, BootstrapError> {
    let command = parse_command(arguments)?;
    if command.adopt_actor_id.is_none()
        || command.organization_id.is_none()
        || command.organization_name.is_some()
        || command.organization_slug.is_some()
    {
        return Err(BootstrapError::Usage(
            "member bootstrap requires --adopt-actor and --organization-id and does not create an Organization",
        ));
    }
    Ok(command)
}

pub fn parse_password_rotation_command(
    arguments: impl IntoIterator<Item = String>,
) -> Result<PasswordRotationCommand, BootstrapError> {
    let mut arguments = arguments.into_iter();
    let mut login_name = None;
    while let Some(argument) = arguments.next() {
        let value = arguments.next().ok_or(BootstrapError::Usage(
            "every password rotation option requires a value",
        ))?;
        match argument.as_str() {
            "--login-name" => login_name = Some(value),
            _ => return Err(BootstrapError::Usage("unknown password rotation option")),
        }
    }
    Ok(PasswordRotationCommand {
        login_name: normalize_login_name(
            login_name.ok_or(BootstrapError::Usage("--login-name is required"))?,
        )?,
    })
}

pub fn read_password_interactively() -> Result<String, BootstrapError> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Err(BootstrapError::InteractiveTerminalRequired);
    }
    let mut password = rpassword::prompt_password("Password: ")
        .map_err(|_| BootstrapError::InteractiveTerminalRequired)?;
    let mut confirmation = match rpassword::prompt_password("Confirm password: ") {
        Ok(confirmation) => confirmation,
        Err(_) => {
            password.zeroize();
            return Err(BootstrapError::InteractiveTerminalRequired);
        }
    };
    if password != confirmation {
        password.zeroize();
        confirmation.zeroize();
        return Err(BootstrapError::PasswordMismatch);
    }
    confirmation.zeroize();
    if let Err(error) = validate_password(&password) {
        password.zeroize();
        return Err(error);
    }
    Ok(password)
}

pub async fn rotate_password(
    pool: &PgPool,
    command: PasswordRotationCommand,
    mut password: String,
) -> Result<PasswordRotationResult, BootstrapError> {
    if let Err(error) = validate_password(&password) {
        password.zeroize();
        return Err(error);
    }
    let password_phc = password::hash_password(password).await?;
    let mut transaction = pool.begin().await?;
    let user = sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT id, actor_id
        FROM users
        WHERE login_name = $1 AND status = 'active'
        FOR UPDATE
        "#,
    )
    .bind(&command.login_name)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(BootstrapError::PasswordCredentialUnavailable)?;
    let credential = sqlx::query(
        r#"
        UPDATE user_password_credentials
        SET password_phc = $2, version = version + 1, updated_at = now()
        WHERE user_id = $1
        "#,
    )
    .bind(user.0)
    .bind(password_phc)
    .execute(&mut *transaction)
    .await?;
    if credential.rows_affected() != 1 {
        return Err(BootstrapError::PasswordCredentialUnavailable);
    }
    let authentication_epoch = sqlx::query_scalar::<_, i64>(
        r#"
        UPDATE users
        SET authentication_epoch = authentication_epoch + 1, updated_at = now()
        WHERE id = $1 AND status = 'active'
        RETURNING authentication_epoch
        "#,
    )
    .bind(user.0)
    .fetch_one(&mut *transaction)
    .await?;
    let revoked_sessions = sqlx::query(
        r#"
        UPDATE sessions
        SET revoked_at = now(), updated_at = now()
        WHERE user_id = $1 AND revoked_at IS NULL
        "#,
    )
    .bind(user.0)
    .execute(&mut *transaction)
    .await?
    .rows_affected();
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, authentication_class, action, target_kind,
            target_id, outcome, metadata
        ) VALUES (
            $1, $2, 'host_local_rotation', 'identity.password.rotate',
            'user', $3, 'succeeded', $4
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(user.1)
    .bind(user.0)
    .bind(serde_json::json!({ "revokedSessions": revoked_sessions }))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    Ok(PasswordRotationResult {
        user_id: user.0,
        authentication_epoch,
        revoked_sessions,
    })
}

pub async fn bootstrap_admin(
    pool: &PgPool,
    command: BootstrapCommand,
    mut password: String,
) -> Result<BootstrapResult, BootstrapError> {
    if let Err(error) = validate_password(&password) {
        password.zeroize();
        return Err(error);
    }
    let password_phc = password::hash_password(password).await?;

    let mut transaction = pool.begin().await?;
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('agent-workspace.bootstrap-admin', 0))",
    )
    .execute(&mut *transaction)
    .await?;
    if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users")
        .fetch_one(&mut *transaction)
        .await?
        != 0
    {
        return Err(BootstrapError::AlreadyBootstrapped);
    }

    let selection = select_actor_and_organization(&mut transaction, &command).await?;
    let (actor_id, organization_id, adopted_existing_actor) = match selection {
        Some(candidate) => (candidate.actor_id, candidate.organization_id, true),
        None => {
            let organization_name =
                command
                    .organization_name
                    .as_ref()
                    .ok_or(BootstrapError::Usage(
                        "--organization-name is required when no eligible M0 owner exists",
                    ))?;
            // Without `--organization-slug`, a readable one from the name
            // with a numeric suffix (K4.5).
            let organization_slug = match command.organization_slug.clone() {
                Some(slug) => slug,
                None => {
                    crate::workspace::slug::readable_for_new(&mut *transaction, organization_name)
                        .await?
                }
            };
            let actor_id = Uuid::now_v7();
            let organization_id = Uuid::now_v7();
            sqlx::query("INSERT INTO actors (id, kind) VALUES ($1, 'human')")
                .bind(actor_id)
                .execute(&mut *transaction)
                .await?;
            sqlx::query("INSERT INTO organizations (id, name, slug) VALUES ($1, $2, $3)")
                .bind(organization_id)
                .bind(organization_name)
                .bind(organization_slug)
                .execute(&mut *transaction)
                .await?;
            (actor_id, organization_id, false)
        }
    };

    let user_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO users (id, actor_id, login_name, display_name)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(user_id)
    .bind(actor_id)
    .bind(&command.login_name)
    .bind(&command.display_name)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO user_password_credentials (user_id, password_phc)
        VALUES ($1, $2)
        "#,
    )
    .bind(user_id)
    .bind(password_phc)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO organization_memberships (
            organization_id, actor_id, role, enabled
        )
        VALUES ($1, $2, 'owner', true)
        ON CONFLICT (organization_id, actor_id) DO UPDATE
        SET role = 'owner', enabled = true, version = organization_memberships.version + 1,
            updated_at = now()
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, authentication_class, action, target_kind,
            target_id, outcome, metadata
        )
        VALUES (
            $1, $2, 'host_local_bootstrap', 'identity.bootstrap_admin',
            'user', $3, 'succeeded', $4
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(actor_id)
    .bind(user_id)
    .bind(serde_json::json!({
        "organization_id": organization_id,
        "adopted_existing_actor": adopted_existing_actor,
    }))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    Ok(BootstrapResult {
        user_id,
        actor_id,
        organization_id,
        adopted_existing_actor,
    })
}

/// Provisions credentials for an existing human Actor during loopback-only
/// development acceptance. The caller must enforce that runtime boundary.
pub async fn bootstrap_development_member(
    pool: &PgPool,
    command: BootstrapCommand,
    mut password: String,
) -> Result<BootstrapResult, BootstrapError> {
    if let Err(error) = validate_password(&password) {
        password.zeroize();
        return Err(error);
    }
    let actor_id = command
        .adopt_actor_id
        .ok_or(BootstrapError::InvalidMemberAdoption)?;
    let organization_id = command
        .organization_id
        .ok_or(BootstrapError::InvalidMemberAdoption)?;
    let password_phc = password::hash_password(password).await?;

    let mut transaction = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("agent-workspace.bootstrap-member:{actor_id}"))
        .execute(&mut *transaction)
        .await?;
    let eligible = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM actors actor
            JOIN organization_memberships organization_member
              ON organization_member.actor_id = actor.id
             AND organization_member.organization_id = $2
             AND organization_member.enabled
            JOIN projects project ON project.organization_id = $2
            JOIN project_memberships project_member
              ON project_member.project_id = project.id
             AND project_member.actor_id = actor.id
             AND project_member.enabled
            WHERE actor.id = $1
              AND actor.kind = 'human'
              AND actor.status = 'active'
        )
        "#,
    )
    .bind(actor_id)
    .bind(organization_id)
    .fetch_one(&mut *transaction)
    .await?;
    if !eligible {
        return Err(BootstrapError::InvalidMemberAdoption);
    }
    if sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM users WHERE actor_id = $1)")
        .bind(actor_id)
        .fetch_one(&mut *transaction)
        .await?
    {
        return Err(BootstrapError::MemberAlreadyProvisioned);
    }

    let user_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO users (id, actor_id, login_name, display_name)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(user_id)
    .bind(actor_id)
    .bind(&command.login_name)
    .bind(&command.display_name)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO user_password_credentials (user_id, password_phc)
        VALUES ($1, $2)
        "#,
    )
    .bind(user_id)
    .bind(password_phc)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, authentication_class, action, target_kind,
            target_id, outcome, metadata
        )
        VALUES (
            $1, $2, 'host_local_bootstrap', 'identity.bootstrap_development_member',
            'user', $3, 'succeeded', $4
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(actor_id)
    .bind(user_id)
    .bind(serde_json::json!({ "organization_id": organization_id }))
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;

    Ok(BootstrapResult {
        user_id,
        actor_id,
        organization_id,
        adopted_existing_actor: true,
    })
}

async fn select_actor_and_organization(
    transaction: &mut Transaction<'_, Postgres>,
    command: &BootstrapCommand,
) -> Result<Option<AdoptionCandidate>, BootstrapError> {
    if let (Some(actor_id), Some(organization_id)) =
        (command.adopt_actor_id, command.organization_id)
    {
        let candidate = sqlx::query_as::<_, AdoptionCandidate>(
            r#"
            SELECT $1::uuid AS actor_id, $2::uuid AS organization_id
            WHERE EXISTS (
                SELECT 1
                FROM actors a
                JOIN project_memberships pm ON pm.actor_id = a.id
                JOIN projects p ON p.id = pm.project_id
                WHERE a.id = $1
                  AND a.kind = 'human'
                  AND a.status = 'active'
                  AND pm.role = 'owner'
                  AND pm.enabled
                  AND p.organization_id = $2
            )
            AND NOT EXISTS (SELECT 1 FROM users WHERE actor_id = $1)
            "#,
        )
        .bind(actor_id)
        .bind(organization_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(BootstrapError::InvalidAdoption)?;
        return Ok(Some(candidate));
    }

    let candidates = sqlx::query_as::<_, AdoptionCandidate>(
        r#"
        SELECT pm.actor_id, p.organization_id
        FROM project_memberships pm
        JOIN actors a ON a.id = pm.actor_id
        JOIN projects p ON p.id = pm.project_id
        WHERE pm.role = 'owner'
          AND pm.enabled
          AND a.kind = 'human'
          AND a.status = 'active'
          AND NOT EXISTS (SELECT 1 FROM users WHERE actor_id = pm.actor_id)
        GROUP BY pm.actor_id, p.organization_id
        ORDER BY pm.actor_id, p.organization_id
        LIMIT 2
        "#,
    )
    .fetch_all(&mut **transaction)
    .await?;
    match candidates.as_slice() {
        [] => Ok(None),
        [candidate] => Ok(Some(AdoptionCandidate {
            actor_id: candidate.actor_id,
            organization_id: candidate.organization_id,
        })),
        _ => Err(BootstrapError::AmbiguousAdoption),
    }
}

fn validate_password(password: &str) -> Result<(), BootstrapError> {
    if !(MIN_PASSWORD_BYTES..=MAX_PASSWORD_BYTES).contains(&password.len()) {
        return Err(BootstrapError::Usage(
            "password must contain between 12 and 1024 bytes",
        ));
    }
    Ok(())
}

fn normalize_login_name(value: String) -> Result<String, BootstrapError> {
    let normalized = value.trim().to_ascii_lowercase();
    let bytes = normalized.as_bytes();
    if !(3..=64).contains(&normalized.len())
        || !bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        || !bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        || !normalized.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
    {
        return Err(BootstrapError::Usage("login name is invalid"));
    }
    Ok(normalized)
}

fn normalize_display_name(value: String) -> Result<String, BootstrapError> {
    let normalized = value.trim();
    if normalized.is_empty() || normalized.len() > 120 || normalized.chars().any(char::is_control) {
        return Err(BootstrapError::Usage("display name is invalid"));
    }
    Ok(normalized.to_owned())
}

fn normalize_organization_name(value: String) -> Result<String, BootstrapError> {
    let normalized = value.trim();
    if normalized.is_empty() || normalized.len() > 120 || normalized.chars().any(char::is_control) {
        return Err(BootstrapError::Usage("Organization name is invalid"));
    }
    Ok(normalized.to_owned())
}

/// A slug given on the command line follows the rules for a new slug
/// (`workspace::slug`, K4.5).
fn normalize_organization_slug(value: String) -> Result<String, BootstrapError> {
    crate::workspace::slug::check_new(&value)
        .map_err(|_| BootstrapError::Usage("Organization slug is invalid or reserved"))
}

#[cfg(test)]
mod tests {
    use super::{
        BootstrapError, parse_command, parse_member_command, parse_password_rotation_command,
        validate_password,
    };
    use uuid::Uuid;

    #[test]
    fn command_contains_no_password_option_and_normalizes_non_secret_values() {
        let command = parse_command([
            "--login-name".to_owned(),
            " Mischa.N ".to_owned(),
            "--display-name".to_owned(),
            " Mischa Nelson ".to_owned(),
            "--organization-name".to_owned(),
            " Personal ".to_owned(),
            "--organization-slug".to_owned(),
            " Personal-Workspace ".to_owned(),
        ])
        .expect("valid command");
        assert_eq!(command.login_name, "mischa.n");
        assert_eq!(command.display_name, "Mischa Nelson");
        assert_eq!(
            command.organization_slug.as_deref(),
            Some("personal-workspace")
        );

        assert!(matches!(
            parse_command(["--password".to_owned(), "secret".to_owned()]),
            Err(BootstrapError::Usage(_))
        ));
        assert!(
            parse_command([
                "--login-name".to_owned(),
                "mischa.".to_owned(),
                "--display-name".to_owned(),
                "Mischa".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn password_length_is_bounded() {
        assert!(validate_password("short").is_err());
        assert!(validate_password("correct horse battery staple").is_ok());
        assert!(validate_password(&"x".repeat(1025)).is_err());
    }

    #[test]
    fn password_rotation_accepts_only_a_normalized_login_name() {
        let command = parse_password_rotation_command([
            "--login-name".to_owned(),
            " Mischa.Nelson07 ".to_owned(),
        ])
        .expect("valid rotation command");
        assert_eq!(command.login_name, "mischa.nelson07");
        assert!(matches!(
            parse_password_rotation_command([
                "--password".to_owned(),
                "must-never-be-an-argument".to_owned(),
            ]),
            Err(BootstrapError::Usage(_))
        ));
    }

    #[test]
    fn member_command_requires_an_existing_actor_and_never_accepts_a_password() {
        let actor_id = Uuid::now_v7();
        let organization_id = Uuid::now_v7();
        let command = parse_member_command([
            "--login-name".to_owned(),
            "m2.collaborator".to_owned(),
            "--display-name".to_owned(),
            "M2 Collaborator".to_owned(),
            "--adopt-actor".to_owned(),
            actor_id.to_string(),
            "--organization-id".to_owned(),
            organization_id.to_string(),
        ])
        .expect("valid member command");
        assert_eq!(command.adopt_actor_id, Some(actor_id));
        assert_eq!(command.organization_id, Some(organization_id));
        assert!(
            parse_member_command([
                "--login-name".to_owned(),
                "member".to_owned(),
                "--display-name".to_owned(),
                "Member".to_owned(),
            ])
            .is_err()
        );
    }
}
