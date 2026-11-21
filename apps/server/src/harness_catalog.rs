//! A1.17: the harnesses the server offers and checks launches against, read
//! from the database rather than from the host's memory, so the host can run
//! as its own program.
//!
//! Two sources make a workspace's catalog: the built-in host's harnesses,
//! which it describes when it takes its lease (`builtin_host_harnesses`,
//! one list for the whole install), and the workspace's registered ones
//! (`custom_harnesses`), whose manifests the server builds from the
//! registration the same way the host does
//! ([`crate::harnesses::manifest_of`]).

use std::collections::BTreeMap;

use serde_json::Value;
use sqlx::{FromRow, PgConnection, PgPool};
use uuid::Uuid;

use crate::{AppState, error::AppError, harness::manifest::AdapterManifest};

/// The most harnesses the built-in host may describe.
pub const MAX_BUILTIN_HARNESSES: usize = 32;
/// The most one described manifest may take, as JSON.
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug)]
struct Entry {
    manifest: AdapterManifest,
    enabled: bool,
    /// The agent a connected-runner harness hands its launches to.
    runner: Option<Uuid>,
}

/// The harnesses a workspace can run (or, from [`Harnesses::visible`], every
/// harness the caller can see, for naming them).
#[derive(Clone, Debug, Default)]
pub struct Harnesses {
    entries: BTreeMap<String, Entry>,
    /// Built-in kinds first, then registered ones, each by kind.
    order: Vec<String>,
}

#[derive(FromRow)]
struct Registered {
    kind: String,
    name: String,
    transport: String,
    manifest: Value,
    runner_actor_id: Option<Uuid>,
    enabled: bool,
}

impl Harnesses {
    /// The workspace's catalog: the built-in host's harnesses and the
    /// workspace's own registered ones.
    pub async fn of(state: &AppState, organization_id: Uuid) -> Result<Self, AppError> {
        crate::db::scoped_one(&state.pool, async |connection| {
            Self::of_on(state, connection, organization_id).await
        })
        .await
    }

    /// [`Self::of`], read on a connection the caller holds (inside its
    /// transaction).
    pub async fn of_on(
        state: &AppState,
        connection: &mut PgConnection,
        organization_id: Uuid,
    ) -> Result<Self, AppError> {
        let registered = sqlx::query_as::<_, Registered>(
            "SELECT kind, name, transport, manifest, runner_actor_id, enabled \
             FROM custom_harnesses WHERE organization_id = $1 ORDER BY kind",
        )
        .bind(organization_id)
        .fetch_all(&mut *connection)
        .await?;
        Self::build(state, connection, registered).await
    }

    /// Every harness the caller can see, to name the harness of a run, a
    /// session or a profile wherever it was registered.
    pub async fn visible(state: &AppState) -> Result<Self, AppError> {
        crate::db::scoped_one(&state.pool, async |connection| {
            Self::visible_on(state, connection).await
        })
        .await
    }

    /// [`Self::visible`], on a connection the caller holds.
    pub async fn visible_on(
        state: &AppState,
        connection: &mut PgConnection,
    ) -> Result<Self, AppError> {
        let registered = sqlx::query_as::<_, Registered>(
            "SELECT kind, name, transport, manifest, runner_actor_id, enabled \
             FROM custom_harnesses ORDER BY kind",
        )
        .fetch_all(&mut *connection)
        .await?;
        Self::build(state, connection, registered).await
    }

    async fn build(
        state: &AppState,
        connection: &mut PgConnection,
        registered: Vec<Registered>,
    ) -> Result<Self, AppError> {
        let mut catalog = Self::default();
        let mut described = builtin(connection).await?;
        // Until the built-in host has described its harnesses (the first
        // second after it takes its lease), a process that holds the host
        // answers for it.
        if described.is_empty() && state.config.builtin_host {
            described = state.supervisors.builtin_manifests();
            described.sort_by(|left, right| left.kind.cmp(&right.kind));
        }
        for manifest in described {
            catalog.insert(manifest, true, None);
        }
        for row in registered {
            // A registration the server can't read (stored before a rule
            // tightened) is left out, as the host leaves it out.
            let Ok(manifest) =
                crate::harnesses::manifest_of(&row.kind, &row.name, &row.transport, &row.manifest)
            else {
                continue;
            };
            let runner = (row.transport == "runner")
                .then_some(row.runner_actor_id)
                .flatten();
            catalog.insert(manifest, row.enabled, runner);
        }
        Ok(catalog)
    }

    fn insert(&mut self, manifest: AdapterManifest, enabled: bool, runner: Option<Uuid>) {
        let kind = manifest.kind.clone();
        // Kinds are unique across the server, built-in ones first.
        if self.entries.contains_key(&kind) {
            return;
        }
        self.order.push(kind.clone());
        self.entries.insert(
            kind,
            Entry {
                manifest,
                enabled,
                runner,
            },
        );
    }

    /// The harness of `kind`, when it can run.
    pub fn manifest(&self, kind: &str) -> Option<&AdapterManifest> {
        self.entries
            .get(kind)
            .filter(|entry| entry.enabled)
            .map(|entry| &entry.manifest)
    }

    /// The harness of `kind`, even switched off: for what a run that started
    /// before it was switched off still reports (its prices).
    pub fn known(&self, kind: &str) -> Option<&AdapterManifest> {
        self.entries.get(kind).map(|entry| &entry.manifest)
    }

    /// Every harness that can run, built-in ones first.
    pub fn manifests(&self) -> impl Iterator<Item = &AdapterManifest> {
        self.order
            .iter()
            .filter_map(|kind| self.entries.get(kind))
            .filter(|entry| entry.enabled)
            .map(|entry| &entry.manifest)
    }

    /// The agent a connected-runner harness hands its launches to.
    pub fn runner_for(&self, kind: &str) -> Option<Uuid> {
        self.entries.get(kind).and_then(|entry| entry.runner)
    }

    /// A name people know a harness by: its manifest's, for ones the
    /// catalog has (even switched off), otherwise the runner's own word
    /// with each part capitalised (`night_runner` reads "Night Runner").
    pub fn name(&self, kind: &str) -> String {
        match self.entries.get(kind) {
            Some(entry) => entry.manifest.display_name.clone(),
            None => readable(kind),
        }
    }
}

fn readable(harness: &str) -> String {
    harness
        .split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(characters).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The built-in host's harnesses, as it last described them. A manifest
/// this server can't read (from a newer host) is left out.
async fn builtin(connection: &mut PgConnection) -> Result<Vec<AdapterManifest>, AppError> {
    let rows =
        sqlx::query_scalar::<_, Value>("SELECT manifest FROM builtin_host_harnesses ORDER BY kind")
            .fetch_all(connection)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|manifest| serde_json::from_value(manifest).ok())
        .collect())
}

/// Whether `kind` names one of the built-in host's harnesses.
pub async fn is_builtin(connection: &mut sqlx::PgConnection, kind: &str) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM builtin_host_harnesses WHERE kind = $1)",
    )
    .bind(kind)
    .fetch_one(connection)
    .await?)
}

/// Why a described list can't be stored: the field and what to do.
#[derive(Debug, PartialEq, Eq)]
pub struct Refused(pub &'static str, pub String);

/// Stores the built-in host's harnesses, replacing the list it sent before:
/// a kind it no longer lists is gone.
pub async fn store_builtin(pool: &PgPool, harnesses: &[AdapterManifest]) -> Result<(), StoreError> {
    let rows = checked(harnesses).map_err(StoreError::Refused)?;
    let mut transaction = crate::db::scoped_tx(pool).await?;
    let kinds = rows
        .iter()
        .map(|(kind, _)| kind.clone())
        .collect::<Vec<_>>();
    sqlx::query("DELETE FROM builtin_host_harnesses WHERE NOT (kind = ANY($1))")
        .bind(&kinds)
        .execute(&mut *transaction)
        .await?;
    for (kind, manifest) in rows {
        sqlx::query(
            "INSERT INTO builtin_host_harnesses (kind, manifest) VALUES ($1, $2) \
             ON CONFLICT (kind) DO UPDATE SET manifest = EXCLUDED.manifest, described_at = now()",
        )
        .bind(kind)
        .bind(manifest)
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

#[derive(Debug)]
pub enum StoreError {
    Refused(Refused),
    App(AppError),
}

impl From<AppError> for StoreError {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl From<sqlx::Error> for StoreError {
    fn from(error: sqlx::Error) -> Self {
        Self::App(error.into())
    }
}

/// The rows a described list stores, once it is checked.
fn checked(harnesses: &[AdapterManifest]) -> Result<Vec<(String, Value)>, Refused> {
    if harnesses.len() > MAX_BUILTIN_HARNESSES {
        return Err(Refused(
            "harnesses",
            format!("Describe at most {MAX_BUILTIN_HARNESSES} harnesses."),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut rows = Vec::with_capacity(harnesses.len());
    for manifest in harnesses {
        let kind = manifest.kind.as_str();
        let well_formed = (1..=64).contains(&kind.len())
            && kind.starts_with(|first: char| first.is_ascii_lowercase())
            && kind
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
        if !well_formed {
            return Err(Refused(
                "harnesses.kind",
                "Give each harness a kind of lowercase letters, digits and underscores.".to_owned(),
            ));
        }
        if !seen.insert(kind) {
            return Err(Refused(
                "harnesses.kind",
                "Describe each harness kind once.".to_owned(),
            ));
        }
        if manifest.display_name.trim().is_empty() {
            return Err(Refused(
                "harnesses.displayName",
                "Give each harness a display name.".to_owned(),
            ));
        }
        let value = serde_json::to_value(manifest)
            .map_err(|_| Refused("harnesses", "A harness's manifest is not JSON.".to_owned()))?;
        if serde_json::to_vec(&value).map_or(true, |bytes| bytes.len() > MAX_MANIFEST_BYTES) {
            return Err(Refused(
                "harnesses",
                format!(
                    "Keep each harness's manifest under {} KiB.",
                    MAX_MANIFEST_BYTES / 1024
                ),
            ));
        }
        rows.push((manifest.kind.clone(), value));
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(kind: &str) -> AdapterManifest {
        crate::harness::registered::command_line_manifest(
            kind,
            "A harness",
            &serde_json::json!({}),
            crate::harness::manifest::RunLocation::LocalHost,
        )
        .expect("manifest")
    }

    #[test]
    fn described_lists_are_checked() {
        assert!(checked(&[manifest("codex_app_server"), manifest("demo")]).is_ok());
        assert_eq!(
            checked(&[manifest("demo"), manifest("demo")]).map(|_| ()),
            Err(Refused(
                "harnesses.kind",
                "Describe each harness kind once.".to_owned()
            ))
        );
        assert!(checked(&[manifest("Bad-Kind")]).is_err());
        assert!(checked(&[manifest("")]).is_err());
        let many = (0..=MAX_BUILTIN_HARNESSES)
            .map(|index| manifest(&format!("kind_{index}")))
            .collect::<Vec<_>>();
        assert!(checked(&many).is_err());
    }

    #[test]
    fn manifests_survive_the_round_trip() {
        let sent = manifest("demo");
        let stored = serde_json::to_value(&sent).expect("json");
        let read: AdapterManifest = serde_json::from_value(stored).expect("read back");
        assert_eq!(read, sent);
    }

    #[test]
    fn unknown_harnesses_read_by_their_word() {
        let catalog = Harnesses::default();
        assert_eq!(catalog.name("night_runner"), "Night Runner");
        assert!(catalog.manifest("night_runner").is_none());
        let mut catalog = Harnesses::default();
        catalog.insert(manifest("x_off"), false, None);
        assert!(catalog.manifest("x_off").is_none());
        assert!(catalog.known("x_off").is_some());
        assert_eq!(catalog.name("x_off"), "A harness");
    }
}
