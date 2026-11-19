//! First run in the browser (H1.14). While nobody has an account, the
//! server keeps one setup code: its digest in `server_setup`, the code in
//! `<data root>/setup-code` (readable by its owner only) on the process that
//! issued it, whose log names the file and nothing else. The web app shows
//! `/setup` until someone enters it, with their name, sign-in name and
//! password, and then either:
//!
//! - **a new workspace**: its name, address and what it is for (a built-in
//!   workspace template, which sets its modules, layout and roles; the
//!   onboarding path carries on from there), all in one transaction; or
//! - **an import** (H1.10): the account is made and signed in, the row is
//!   marked as theirs, and their upload of an export archive starts the
//!   import, after which the row goes. Until then they may still choose a
//!   new workspace instead.
//!
//! The code works once, is compared in constant time, and attempts are
//! limited per address like sign-ins. It takes the same lock
//! `bootstrap-admin` takes (a transaction's, so it is pooler-safe), and
//! makes the person and workspace through the same functions. Anyone
//! getting an account another way ends setup: the row goes in that
//! transaction, and the file at the next start.

use std::path::{Path, PathBuf};

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use subtle::ConstantTimeEq;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{
    AppState,
    accounts::{NewPerson, create_person},
    auth::{
        AuthenticatedActor, SessionIdentity, establish_session_in_transaction,
        load_authenticated_actor, password, require_allowed_origin, require_authenticated_write,
        respond_with_established_session,
    },
    config::AppConfig,
    error::{ApiJson, ApiPath, AppError},
    export::import::{self, ImportError, StartImport},
    workspace::lifecycle::{self, NewWorkspace},
    workspace_templates,
};

/// The file, in the data root, that holds the code.
pub const CODE_FILE: &str = "setup-code";
/// The largest archive the browser may upload; a bigger one is imported
/// with `workspace-import --file` on the server.
pub const MAX_ARCHIVE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// Where an uploaded archive waits while the import starts.
const UPLOADS: &str = "imports";
const LOCK: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended('agent-workspace.bootstrap-admin', 0))";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/setup", get(read_status).post(set_up))
        .route("/api/v1/setup/workspace", post(make_workspace))
        .route(
            "/api/v1/setup/import",
            post(upload_archive).layer(DefaultBodyLimit::disable()),
        )
        .route("/api/v1/setup/import/{transfer}", get(read_import))
}

pub fn code_path(config: &AppConfig) -> PathBuf {
    config.data_root.join(CODE_FILE)
}

fn digest(code: &str) -> [u8; 32] {
    let normalized = code
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect::<String>();
    Sha256::digest(normalized.as_bytes()).into()
}

/// 128 random bits, as eight groups of four letters and digits a person
/// can read out: `7k2m-…`.
fn new_code() -> String {
    const ALPHABET: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyz";
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let letters = bytes
        .iter()
        .map(|byte| ALPHABET[usize::from(*byte) % ALPHABET.len()] as char)
        .collect::<Vec<_>>();
    letters
        .chunks(4)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

async fn anyone_signs_in(connection: &mut PgConnection) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM users)")
        .fetch_one(connection)
        .await
}

/// Writes `code` to `path`, readable by its owner alone, replacing what
/// was there.
fn write_code(path: &Path, code: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let staged = path.with_extension("new");
    let _ = std::fs::remove_file(&staged);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&staged)?;
    file.write_all(code.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    std::fs::rename(staged, path)
}

/// What [`prepare`] found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Prepared {
    /// Someone has an account: no setup.
    Done,
    /// A code is waiting in this file.
    Waiting(PathBuf),
    /// Another process issued the code; its file is on that process's disk.
    Elsewhere,
    /// The first account is made and its import not uploaded yet.
    Claimed,
}

/// At start (a process that migrates): while nobody has an account, make
/// sure a setup code exists, issuing one into this process's data root if
/// none does. Once anyone has an account, the row and this process's file
/// go.
pub async fn prepare(pool: &PgPool, config: &AppConfig) -> Result<Prepared, AppError> {
    crate::tenancy::system("server.first_run", async {
        let path = code_path(config);
        let mut transaction = crate::db::scoped_tx(pool).await?;
        sqlx::query(LOCK).execute(&mut *transaction).await?;
        if anyone_signs_in(&mut transaction).await? {
            let claimed = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM server_setup WHERE claimed_by_actor_id IS NOT NULL)",
            )
            .fetch_one(&mut *transaction)
            .await?;
            if !claimed {
                forget(&mut transaction).await?;
            }
            transaction.commit().await?;
            let _ = std::fs::remove_file(&path);
            return Ok(if claimed {
                Prepared::Claimed
            } else {
                Prepared::Done
            });
        }
        let stored =
            sqlx::query_scalar::<_, Vec<u8>>("SELECT code_digest FROM server_setup WHERE id = 1")
                .fetch_optional(&mut *transaction)
                .await?;
        let here = std::fs::read_to_string(&path).ok();
        match (stored, here) {
            (Some(stored), Some(here)) if digest(&here).as_slice() == stored.as_slice() => {
                transaction.commit().await?;
                Ok(Prepared::Waiting(path))
            }
            (Some(_), _) => {
                transaction.commit().await?;
                Ok(Prepared::Elsewhere)
            }
            (None, _) => {
                issue(&mut transaction, &path).await?;
                transaction.commit().await?;
                Ok(Prepared::Waiting(path))
            }
        }
    })
    .await
}

/// A new code in this process's data root, replacing any other (the
/// `setup-code` command, for when the first one is on a process that's
/// gone). Refused once anyone has an account.
pub async fn issue_here(pool: &PgPool, config: &AppConfig) -> Result<PathBuf, AppError> {
    crate::tenancy::system("server.first_run", async {
        let path = code_path(config);
        let mut transaction = crate::db::scoped_tx(pool).await?;
        sqlx::query(LOCK).execute(&mut *transaction).await?;
        if anyone_signs_in(&mut transaction).await? {
            return Err(AppError::Conflict(
                "setup",
                "someone already has an account",
            ));
        }
        issue(&mut transaction, &path).await?;
        transaction.commit().await?;
        Ok(path)
    })
    .await
}

async fn issue(transaction: &mut Transaction<'_, Postgres>, path: &Path) -> Result<(), AppError> {
    let code = new_code();
    sqlx::query(
        "INSERT INTO server_setup (id, code_digest) VALUES (1, $1) \
         ON CONFLICT (id) DO UPDATE SET code_digest = EXCLUDED.code_digest, created_at = now()",
    )
    .bind(digest(&code).to_vec())
    .execute(&mut **transaction)
    .await?;
    write_code(path, &code).map_err(|_| AppError::Internal("the setup code could not be written"))
}

/// Ends setup inside `transaction` (an account was made another way).
/// `bootstrap-admin` also runs against databases from before first run in
/// the browser existed, where there is no setup to end.
pub(crate) async fn forget(connection: &mut PgConnection) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DO $$ BEGIN IF to_regclass('server_setup') IS NOT NULL THEN \
         DELETE FROM server_setup; END IF; END $$",
    )
    .execute(connection)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// The browser's side

/// One shape a new workspace can start in.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Shape {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    what_it_does: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    /// Nobody has an account: the setup page shows.
    needed: bool,
    /// The signed-in person made the first account to import a workspace
    /// and hasn't uploaded it yet.
    awaiting_import: bool,
    /// What a new workspace can be for, while setup is open to the caller.
    shapes: Vec<Shape>,
    /// The largest archive the page may upload.
    max_archive_bytes: u64,
}

fn shapes() -> Vec<Shape> {
    workspace_templates::builtin_definitions()
        .map(|(id, name, description, definition)| Shape {
            id,
            name,
            description,
            what_it_does: workspace_templates::describe(definition),
        })
        .collect()
}

/// `GET /api/v1/setup`: whether setup is open, and to whom.
async fn read_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Status>, AppError> {
    let principal = load_authenticated_actor(&state, &headers).await?;
    let (needed, awaiting_import) = crate::tenancy::system("server.first_run", async {
        crate::db::scoped_one(&state.pool, async |connection| {
            let needed = !anyone_signs_in(&mut *connection).await?;
            let awaiting = match &principal {
                Some(principal) => {
                    sqlx::query_scalar::<_, bool>(
                        "SELECT EXISTS (SELECT 1 FROM server_setup \
                         WHERE claimed_by_actor_id = $1)",
                    )
                    .bind(principal.actor_id)
                    .fetch_one(&mut *connection)
                    .await?
                }
                None => false,
            };
            Ok::<_, sqlx::Error>((needed, awaiting))
        })
        .await
    })
    .await?;
    Ok(Json(Status {
        needed,
        awaiting_import,
        shapes: if needed || awaiting_import {
            shapes()
        } else {
            Vec::new()
        },
        max_archive_bytes: MAX_ARCHIVE_BYTES,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersonInput {
    display_name: String,
    login_name: String,
    password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkspaceInput {
    name: String,
    /// Its address; one is made from the name when left out.
    #[serde(default)]
    slug: Option<String>,
    /// A built-in workspace template's id; "everything" when left out.
    #[serde(default)]
    shape: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetupRequest {
    code: String,
    person: PersonInput,
    /// The new workspace; leave it out to import one next.
    #[serde(default)]
    workspace: Option<WorkspaceInput>,
}

fn display_name(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 120 || value.chars().any(char::is_control) {
        return Err(AppError::Refused {
            field: "person.displayName",
            message: "Enter your name, up to 120 characters.",
        });
    }
    Ok(value.to_owned())
}

fn login_name(value: &str) -> Result<String, AppError> {
    crate::auth::normalize_login_name(value).map_err(|_| AppError::Refused {
        field: "person.loginName",
        message: "A sign-in name is 3 to 64 letters, digits, dots, dashes or underscores, \
                  starting and ending with a letter or digit.",
    })
}

/// `POST /api/v1/setup`: the code, the first person, and their workspace
/// (or none, to import one next). Answers like a sign-in, with the session.
async fn set_up(
    State(state): State<AppState>,
    headers: HeaderMap,
    ApiJson(mut request): ApiJson<SetupRequest>,
) -> Result<Response, AppError> {
    let checked = check_setup(&state, &headers, &request).await;
    let (display, login) = match checked {
        Ok(names) => names,
        Err(error) => {
            request.person.password.zeroize();
            return Err(error);
        }
    };
    let _permit = match state.rate_limits.try_acquire_password_kdf() {
        Ok(permit) => permit,
        Err(error) => {
            request.person.password.zeroize();
            return Err(error);
        }
    };
    let password_phc =
        password::hash_password(std::mem::take(&mut request.person.password)).await?;
    crate::tenancy::system("server.first_run", async {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        sqlx::query(LOCK).execute(&mut *transaction).await?;
        if anyone_signs_in(&mut transaction).await? {
            return Err(AppError::Conflict("setup", "this server is already set up"));
        }
        let stored = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT code_digest FROM server_setup WHERE id = 1 AND claimed_by_actor_id IS NULL",
        )
        .fetch_optional(&mut *transaction)
        .await?;
        let given = digest(&request.code);
        let matches = stored.is_some_and(|stored| bool::from(stored.as_slice().ct_eq(&given)));
        if !matches {
            drop(transaction);
            state.rate_limits.note_login_failure().await;
            return Err(AppError::Refused {
                field: "code",
                message: "That setup code isn't right. Copy it again from the server's setup-code file.",
            });
        }
        let person = create_person(
            &mut transaction,
            NewPerson {
                login_name: &login,
                display_name: &display,
                default_workspace: None,
                created_by: None,
                claimed: true,
            },
        )
        .await?;
        sqlx::query("INSERT INTO user_password_credentials (user_id, password_phc) VALUES ($1, $2)")
            .bind(person.user_id)
            .bind(password_phc)
            .execute(&mut *transaction)
            .await?;
        // The setup code comes from the server's own files, so whoever
        // holds it runs the server, as `bootstrap-admin` makes its user
        // (K1.30).
        sqlx::query(
            "INSERT INTO server_roles (account_id, role) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(person.account_id)
        .bind(crate::server_status::OPERATOR)
        .execute(&mut *transaction)
        .await?;
        let established = establish_session_in_transaction(
            &state.config,
            &headers,
            &mut transaction,
            SessionIdentity {
                user_id: person.user_id,
                actor_id: person.account_id,
                actor_kind: "human".to_owned(),
                login_name: login.clone(),
                display_name: display.clone(),
                authentication_epoch: 1,
            },
        )
        .await?;
        crate::account_security::record_sign_in_method(
            &mut transaction,
            established.principal.session_id,
            "password",
        )
        .await?;
        match &request.workspace {
            Some(workspace) => {
                if let Err(refusal) =
                    new_workspace(&mut transaction, &headers, &established.principal, workspace)
                        .await?
                {
                    return Ok(refusal);
                }
                forget(&mut transaction).await?;
            }
            None => {
                sqlx::query(
                    "UPDATE server_setup SET claimed_by_actor_id = $1, claimed_at = now() \
                     WHERE id = 1",
                )
                .bind(person.account_id)
                .execute(&mut *transaction)
                .await?;
            }
        }
        transaction.commit().await?;
        if request.workspace.is_some() {
            let _ = tokio::fs::remove_file(code_path(&state.config)).await;
        }
        respond_with_established_session(&state, established).await
    })
    .await
}

/// Origin, the address's budget and the fields, before any hashing.
async fn check_setup(
    state: &AppState,
    headers: &HeaderMap,
    request: &SetupRequest,
) -> Result<(String, String), AppError> {
    require_allowed_origin(&state.config, headers)?;
    state
        .rate_limits
        .check_login_sender(crate::rate_limit::client_address(&state.config, headers))
        .await?;
    let display = display_name(&request.person.display_name)?;
    let login = login_name(&request.person.login_name)?;
    if !(12..=1024).contains(&request.person.password.len()) {
        return Err(AppError::Refused {
            field: "person.password",
            message: "Choose a password of at least 12 characters.",
        });
    }
    if let Some(workspace) = &request.workspace {
        workspace_name(&workspace.name)?;
    }
    Ok((display, login))
}

fn workspace_name(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 120 || value.chars().any(char::is_control) {
        return Err(AppError::Refused {
            field: "workspace.name",
            message: "Name the workspace, up to 120 characters.",
        });
    }
    Ok(value.to_owned())
}

/// The workspace, owned by `principal`, in the shape they chose. A
/// template's refusal comes back as the response to send.
async fn new_workspace(
    transaction: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    input: &WorkspaceInput,
) -> Result<Result<Uuid, Response>, AppError> {
    let name = workspace_name(&input.name)?;
    let slug = match input
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|slug| !slug.is_empty())
    {
        Some(slug) => crate::workspace::slug::check_new(slug)?,
        None => crate::workspace::slug::readable_for_new(transaction, &name).await?,
    };
    let workspace = lifecycle::create(
        transaction,
        principal.actor_id,
        NewWorkspace {
            name,
            slug: Some(slug),
            id: None,
        },
    )
    .await?;
    sqlx::query("UPDATE users SET default_organization_id = $2 WHERE id = $1")
        .bind(principal.user_id)
        .bind(workspace)
        .execute(&mut **transaction)
        .await?;
    let shape = input.shape.as_deref().unwrap_or("everything");
    let template = workspace_templates::find(transaction, workspace, shape)
        .await?
        .filter(|template| template.built_in)
        .ok_or(AppError::Refused {
            field: "workspace.shape",
            message: "Choose what the workspace is for from the list.",
        })?;
    let caller = crate::roles::standing(transaction, workspace, principal.actor_id).await?;
    let auditor = crate::roles::Auditor::Browser { headers, principal };
    match workspace_templates::apply(transaction, &auditor, caller, workspace, &template).await {
        Ok(_) => {}
        Err(workspace_templates::ApplyError::Problem(problem)) => {
            return Ok(Err(workspace_templates::refused(problem).into_response()));
        }
        Err(workspace_templates::ApplyError::App(error)) => return Err(error),
    }
    crate::workspace::audit(
        transaction,
        headers,
        principal,
        workspace,
        "server.first_run",
        "organization",
        Some(workspace),
        json!({ "shape": template.id }),
    )
    .await?;
    Ok(Ok(workspace))
}

/// The signed-in person who made the first account to import, while they
/// haven't: locks their claim.
async fn claim_of(
    connection: &mut PgConnection,
    principal: &AuthenticatedActor,
) -> Result<(), AppError> {
    sqlx::query_scalar::<_, i16>(
        "SELECT id FROM server_setup WHERE claimed_by_actor_id = $1 FOR UPDATE",
    )
    .bind(principal.actor_id)
    .fetch_optional(connection)
    .await?
    .map(|_| ())
    .ok_or(AppError::NotFound("setup"))
}

/// `POST /api/v1/setup/workspace`: having made the first account to import,
/// a new workspace after all.
async fn make_workspace(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiJson(input): ApiJson<WorkspaceInput>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    crate::tenancy::system("server.first_run", async {
        let mut transaction = crate::db::scoped_tx(&state.pool).await?;
        sqlx::query(LOCK).execute(&mut *transaction).await?;
        claim_of(&mut transaction, &principal).await?;
        let workspace = match new_workspace(&mut transaction, &headers, &principal, &input).await? {
            Ok(workspace) => workspace,
            Err(refusal) => return Ok(refusal),
        };
        forget(&mut transaction).await?;
        let slug = sqlx::query_scalar::<_, String>("SELECT slug FROM organizations WHERE id = $1")
            .bind(workspace)
            .fetch_one(&mut *transaction)
            .await?;
        transaction.commit().await?;
        let _ = tokio::fs::remove_file(code_path(&state.config)).await;
        Ok((
            StatusCode::CREATED,
            Json(json!({ "workspaceId": workspace, "slug": slug })),
        )
            .into_response())
    })
    .await
}

/// `POST /api/v1/setup/import`: the archive, as the request's body, from
/// the person who made the first account to import it. It is checked whole,
/// then the import runs in the background; `GET …/import/{transfer}` shows
/// how far it got.
async fn upload_archive(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    body: Body,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal).await?;
    crate::tenancy::system("server.first_run", async {
        crate::db::scoped_one(&state.pool, async |connection| {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM server_setup WHERE claimed_by_actor_id = $1)",
            )
            .bind(principal.actor_id)
            .fetch_one(connection)
            .await
        })
        .await?
        .then_some(())
        .ok_or(AppError::NotFound("setup"))
    })
    .await?;
    let folder = state.config.data_root.join(UPLOADS);
    tokio::fs::create_dir_all(&folder)
        .await
        .map_err(|_| AppError::Internal("the upload folder could not be made"))?;
    let path = folder.join(format!("setup-{}.awx", Uuid::now_v7().simple()));
    let received = receive(&path, body).await;
    let started = match received {
        Ok(()) => {
            import::start_import(
                &state,
                StartImport {
                    path: &path,
                    owner: principal.actor_id,
                    slug: None,
                },
            )
            .await
        }
        Err(error) => Err(ImportError::App(error)),
    };
    let _ = tokio::fs::remove_file(&path).await;
    let started = match started {
        Ok(started) => started,
        Err(ImportError::Refused(reason)) => {
            return Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "error": {
                        "code": "archive_refused",
                        "message": format!("That archive can't be imported: {reason}."),
                    }
                })),
            )
                .into_response());
        }
        Err(ImportError::App(error)) => return Err(error),
    };
    import::enqueue(&state, started.workspace_id, started.transfer.id).await?;
    crate::tenancy::system("server.first_run", async {
        crate::db::scoped_one(&state.pool, async |connection| {
            sqlx::query("DELETE FROM server_setup WHERE claimed_by_actor_id = $1")
                .bind(principal.actor_id)
                .execute(&mut *connection)
                .await?;
            sqlx::query("UPDATE users SET default_organization_id = $2 WHERE id = $1")
                .bind(principal.user_id)
                .bind(started.workspace_id)
                .execute(&mut *connection)
                .await
        })
        .await
    })
    .await?;
    let _ = tokio::fs::remove_file(code_path(&state.config)).await;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "transferId": started.transfer.id,
            "workspaceId": started.workspace_id,
        })),
    )
        .into_response())
}

/// The request's body into `path`, up to [`MAX_ARCHIVE_BYTES`].
async fn receive(path: &Path, body: Body) -> Result<(), AppError> {
    let failed = |_| AppError::Internal("the archive could not be saved");
    let mut file = tokio::fs::File::create(path).await.map_err(failed)?;
    let mut stream = body.into_data_stream();
    let mut written = 0_u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| AppError::Invalid("body"))?;
        written += chunk.len() as u64;
        if written > MAX_ARCHIVE_BYTES {
            return Err(AppError::PayloadTooLarge);
        }
        file.write_all(&chunk).await.map_err(failed)?;
    }
    file.flush().await.map_err(failed)?;
    if written == 0 {
        return Err(AppError::Refused {
            field: "archive",
            message: "Choose an export archive (.awx) to upload.",
        });
    }
    Ok(())
}

/// `GET /api/v1/setup/import/{transfer}`: how the requester's import is
/// going.
async fn read_import(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(transfer): ApiPath<Uuid>,
) -> Result<Json<import::Progress>, AppError> {
    import::progress(&state.pool, transfer, principal.actor_id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound("transfer"))
}
