//! Shared limits (K5.6): request budgets counted in PostgreSQL, so every
//! server instance spends from the same budget and a restart forgets
//! nothing.
//!
//! Two tiers. **Exact** buckets (sign-in, resets, invitations, token
//! attempts, OAuth, webhooks, bug reports) take one round trip per check.
//! **Chunked** buckets (writes, expensive reads, MCP requests, run reports)
//! reserve a slice of the budget, about a twentieth and at most the tier's
//! chunk, and spend it locally: never more than the limit in total, and at
//! most `chunk × (instances − 1)` left unused when a window ends.
//!
//! A window starts with a key's first use and lasts the bucket's window, as
//! the in-memory limiter it replaces did. If the counter can't be read,
//! exact buckets refuse and chunked ones allow; both log.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::error::AppError;

/// How a bucket is counted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tier {
    /// One round trip per check.
    Exact,
    /// Reserve up to `chunk` at a time and spend it locally.
    Chunked { chunk: u32 },
}

/// One budget: `limit` uses per `window` for each key.
#[derive(Debug)]
pub struct Bucket {
    /// Stored with each counter: lower case, digits, `_` and `.`.
    pub name: &'static str,
    pub limit: u32,
    pub window: Duration,
    pub tier: Tier,
}

/// Who a budget belongs to. Only a digest of the key is stored.
#[derive(Clone, Copy, Debug)]
pub enum LimitKey<'a> {
    Global,
    /// A secret or a name, already digested or not; never stored as given.
    Secret(&'a [u8]),
    Address(&'a str),
    Session(Uuid),
    Credential(Uuid),
    Account(Uuid),
    Workspace(Uuid),
}

impl LimitKey<'_> {
    fn digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        match self {
            Self::Global => hasher.update(b"global"),
            Self::Secret(bytes) => {
                hasher.update(b"secret:");
                hasher.update(bytes);
            }
            Self::Address(address) => {
                hasher.update(b"address:");
                hasher.update(address.as_bytes());
            }
            Self::Session(id) => {
                hasher.update(b"session:");
                hasher.update(id.as_bytes());
            }
            Self::Credential(id) => {
                hasher.update(b"credential:");
                hasher.update(id.as_bytes());
            }
            Self::Account(id) => {
                hasher.update(b"account:");
                hasher.update(id.as_bytes());
            }
            Self::Workspace(id) => {
                hasher.update(b"workspace:");
                hasher.update(id.as_bytes());
            }
        }
        hasher.finalize().into()
    }

    fn workspace(&self) -> Option<Uuid> {
        match self {
            Self::Workspace(id) => Some(*id),
            _ => None,
        }
    }
}

/// Where a bucket's number comes from for a key: the bucket's default here;
/// a plan (H1) or a budget (A2) can supply another.
pub trait LimitSource: Send + Sync {
    /// `None` keeps the bucket's own limit.
    fn limit_for(&self, bucket: &Bucket, key: &LimitKey<'_>) -> Option<u32>;
}

/// Every bucket at its default.
pub struct Defaults;

impl LimitSource for Defaults {
    fn limit_for(&self, _bucket: &Bucket, _key: &LimitKey<'_>) -> Option<u32> {
        None
    }
}

/// Keys seen lately (a name that signed in, a token that resolved), kept
/// for `ttl` beside the counters.
#[derive(Debug)]
pub struct Recent {
    pub name: &'static str,
    pub ttl: Duration,
}

/// The shared limiter. Cheap to clone; clones share the store and the
/// local chunks.
#[derive(Clone)]
pub struct Limits {
    pool: Arc<OnceLock<PgPool>>,
    chunks: Arc<Mutex<ChunkBook>>,
    source: Arc<dyn LimitSource>,
}

impl Default for Limits {
    fn default() -> Self {
        Self::new(Arc::new(Defaults))
    }
}

impl Limits {
    pub fn new(source: Arc<dyn LimitSource>) -> Self {
        Self {
            pool: Arc::new(OnceLock::new()),
            chunks: Arc::new(Mutex::new(ChunkBook::default())),
            source,
        }
    }

    /// Counts in `pool` from now on. The first pool given stays.
    pub fn attach(&self, pool: &PgPool) {
        let _ = self.pool.set(pool.clone());
    }

    pub async fn check(&self, bucket: &'static Bucket, key: LimitKey<'_>) -> Result<(), AppError> {
        self.check_via(&mut On::Pool, bucket, key, 1).await
    }

    pub async fn check_cost(
        &self,
        bucket: &'static Bucket,
        key: LimitKey<'_>,
        cost: u32,
    ) -> Result<(), AppError> {
        self.check_via(&mut On::Pool, bucket, key, cost).await
    }

    /// [`Self::check`] on a connection the caller already holds (inside its
    /// transaction), so a caller holding a pooled connection never waits for
    /// a second one. The spend commits or rolls back with the caller's work.
    pub async fn check_on(
        &self,
        connection: &mut PgConnection,
        bucket: &'static Bucket,
        key: LimitKey<'_>,
    ) -> Result<(), AppError> {
        self.check_via(&mut On::Connection(connection), bucket, key, 1)
            .await
    }

    /// Seconds until `key` may try again, without spending anything.
    pub async fn spent(
        &self,
        bucket: &'static Bucket,
        key: LimitKey<'_>,
    ) -> Result<Option<u64>, AppError> {
        self.spent_via(&mut On::Pool, bucket, key).await
    }

    /// Like [`Self::spent`], but an unreadable counter counts as not spent
    /// (for checks that only narrow who gets through while a flood lasts).
    pub async fn spent_or_open(&self, bucket: &'static Bucket, key: LimitKey<'_>) -> Option<u64> {
        self.spent_or_open_via(&mut On::Pool, bucket, key).await
    }

    /// [`Self::spent_or_open`] on the caller's connection.
    pub async fn spent_or_open_on(
        &self,
        connection: &mut PgConnection,
        bucket: &'static Bucket,
        key: LimitKey<'_>,
    ) -> Option<u64> {
        self.spent_or_open_via(&mut On::Connection(connection), bucket, key)
            .await
    }

    /// Spends one from `key` if any is left; a spent budget is what the next
    /// [`Self::spent`] reports. Failures are logged, never returned.
    pub async fn note(&self, bucket: &'static Bucket, key: LimitKey<'_>) {
        let limit = self.limit(bucket, &key);
        if let Err(error) = self.take(&mut On::Pool, bucket, &key, 1, limit).await {
            tracing::warn!(
                bucket = bucket.name,
                error_category = error.log_category(),
                "limit counter unavailable"
            );
        }
    }

    /// Remembers `key` for `recent.ttl`.
    pub async fn remember(&self, recent: &'static Recent, key: LimitKey<'_>) {
        self.remember_via(&mut On::Pool, recent, key).await;
    }

    /// [`Self::remember`] on the caller's connection.
    pub async fn remember_on(
        &self,
        connection: &mut PgConnection,
        recent: &'static Recent,
        key: LimitKey<'_>,
    ) {
        self.remember_via(&mut On::Connection(connection), recent, key)
            .await;
    }

    /// Whether `key` was remembered within `recent.ttl`. Unreadable counts
    /// as not seen.
    pub async fn seen(&self, recent: &'static Recent, key: LimitKey<'_>) -> bool {
        self.seen_via(&mut On::Pool, recent, key).await
    }

    /// [`Self::seen`] on the caller's connection.
    pub async fn seen_on(
        &self,
        connection: &mut PgConnection,
        recent: &'static Recent,
        key: LimitKey<'_>,
    ) -> bool {
        self.seen_via(&mut On::Connection(connection), recent, key)
            .await
    }

    fn limit(&self, bucket: &Bucket, key: &LimitKey<'_>) -> u32 {
        self.source.limit_for(bucket, key).unwrap_or(bucket.limit)
    }

    fn pool(&self) -> Result<&PgPool, AppError> {
        self.pool.get().ok_or(AppError::Internal(
            "limits used before the database was attached",
        ))
    }

    /// Runs one statement on the pool (in a scoped transaction of its own)
    /// or on the caller's connection.
    async fn run<T, F>(&self, on: &mut On<'_>, work: F) -> Result<T, AppError>
    where
        F: AsyncFnOnce(&mut PgConnection) -> Result<T, sqlx::Error>,
    {
        match on {
            On::Pool => Ok(crate::db::scoped_one(self.pool()?, work).await?),
            On::Connection(connection) => Ok(work(connection).await?),
        }
    }

    async fn check_via(
        &self,
        on: &mut On<'_>,
        bucket: &'static Bucket,
        key: LimitKey<'_>,
        cost: u32,
    ) -> Result<(), AppError> {
        let limit = self.limit(bucket, &key);
        if cost > limit {
            return Err(AppError::rate_limited(seconds(bucket.window)));
        }
        match bucket.tier {
            Tier::Exact => match self.take(on, bucket, &key, cost, limit).await {
                Ok(Taken::Granted { .. }) => Ok(()),
                Ok(Taken::Refused { retry_after }) => Err(AppError::rate_limited(retry_after)),
                Err(error) => {
                    tracing::warn!(
                        bucket = bucket.name,
                        error_category = error.log_category(),
                        "limit counter unavailable; refusing"
                    );
                    Err(error)
                }
            },
            Tier::Chunked { chunk } => {
                self.check_chunked(on, bucket, &key, cost, limit, chunk)
                    .await
            }
        }
    }

    async fn spent_via(
        &self,
        on: &mut On<'_>,
        bucket: &'static Bucket,
        key: LimitKey<'_>,
    ) -> Result<Option<u64>, AppError> {
        let limit = self.limit(bucket, &key);
        let digest = key.digest();
        let row = self
            .run(on, async |connection| {
                sqlx::query_as::<_, (i32, f64)>(
                    r#"
                    SELECT used, extract(epoch FROM expires_at - now())::float8
                    FROM rate_limit_counters
                    WHERE bucket = $1 AND key_digest = $2 AND expires_at > now()
                    "#,
                )
                .bind(bucket.name)
                .bind(digest.as_slice())
                .fetch_optional(&mut *connection)
                .await
            })
            .await?;
        Ok(row
            .filter(|(used, _)| i64::from(*used) >= i64::from(limit))
            .map(|(_, left)| seconds_from(left)))
    }

    async fn spent_or_open_via(
        &self,
        on: &mut On<'_>,
        bucket: &'static Bucket,
        key: LimitKey<'_>,
    ) -> Option<u64> {
        match self.spent_via(on, bucket, key).await {
            Ok(spent) => spent,
            Err(error) => {
                tracing::warn!(
                    bucket = bucket.name,
                    error_category = error.log_category(),
                    "limit counter unavailable"
                );
                None
            }
        }
    }

    async fn remember_via(&self, on: &mut On<'_>, recent: &'static Recent, key: LimitKey<'_>) {
        let digest = key.digest();
        let ttl = seconds(recent.ttl) as i32;
        let remembered = self
            .run(on, async |connection| {
                sqlx::query(
                    r#"
                    INSERT INTO rate_limit_counters AS seen
                        (bucket, key_digest, window_start, window_seconds, used, expires_at)
                    VALUES ($1, $2, now(), $3, 1, now() + make_interval(secs => $3))
                    ON CONFLICT (bucket, key_digest) DO UPDATE
                        SET window_start = now(), expires_at = EXCLUDED.expires_at
                        -- Written again only once half its time has gone, so a
                        -- token used on every request isn't a write each time.
                        WHERE seen.expires_at < now() + make_interval(secs => $3 / 2)
                    "#,
                )
                .bind(recent.name)
                .bind(digest.as_slice())
                .bind(ttl)
                .execute(&mut *connection)
                .await
            })
            .await;
        if let Err(error) = remembered {
            tracing::warn!(
                bucket = recent.name,
                error_category = error.log_category(),
                "limit counter unavailable"
            );
        }
    }

    async fn seen_via(&self, on: &mut On<'_>, recent: &'static Recent, key: LimitKey<'_>) -> bool {
        let digest = key.digest();
        self.run(on, async |connection| {
            sqlx::query_scalar::<_, bool>(
                r#"
                SELECT EXISTS (
                    SELECT 1 FROM rate_limit_counters
                    WHERE bucket = $1 AND key_digest = $2 AND expires_at > now()
                )
                "#,
            )
            .bind(recent.name)
            .bind(digest.as_slice())
            .fetch_one(&mut *connection)
            .await
        })
        .await
        .unwrap_or(false)
    }

    async fn check_chunked(
        &self,
        on: &mut On<'_>,
        bucket: &'static Bucket,
        key: &LimitKey<'_>,
        cost: u32,
        limit: u32,
        chunk: u32,
    ) -> Result<(), AppError> {
        let digest = key.digest();
        if self.book().spend(bucket.name, digest, cost, Instant::now()) {
            return Ok(());
        }
        let first = reservation(chunk, limit, cost);
        let mut sizes = vec![first];
        if first > cost {
            sizes.push(cost);
        }
        for size in sizes {
            match self.take(on, bucket, key, size, limit).await {
                Ok(Taken::Granted { left }) => {
                    let ends = Instant::now() + Duration::from_secs_f64(left.max(0.0));
                    self.book()
                        .grant(bucket.name, digest, size - cost, ends, Instant::now());
                    return Ok(());
                }
                Ok(Taken::Refused { retry_after }) if size == cost => {
                    return Err(AppError::rate_limited(retry_after));
                }
                Ok(Taken::Refused { .. }) => {}
                Err(error) => {
                    tracing::warn!(
                        bucket = bucket.name,
                        error_category = error.log_category(),
                        "limit counter unavailable; allowing"
                    );
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn book(&self) -> std::sync::MutexGuard<'_, ChunkBook> {
        self.chunks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Adds `cost` to `key`'s counter if that keeps it within `limit`,
    /// starting a new window when the last one has ended.
    async fn take(
        &self,
        on: &mut On<'_>,
        bucket: &'static Bucket,
        key: &LimitKey<'_>,
        cost: u32,
        limit: u32,
    ) -> Result<Taken, AppError> {
        let digest = key.digest();
        let workspace = key.workspace();
        let window = seconds(bucket.window) as i32;
        let (used, left) = self
            .run(on, async |connection| {
                sqlx::query_as::<_, (Option<i32>, Option<f64>)>(
                    r#"
                    WITH taken AS (
                        INSERT INTO rate_limit_counters AS counter
                            (bucket, key_digest, window_start, window_seconds, used, expires_at,
                             workspace_id)
                        VALUES ($1, $2, now(), $3, $4, now() + make_interval(secs => $3), $6)
                        ON CONFLICT (bucket, key_digest) DO UPDATE
                            SET window_start = CASE WHEN counter.expires_at <= now()
                                    THEN now() ELSE counter.window_start END,
                                window_seconds = EXCLUDED.window_seconds,
                                used = CASE WHEN counter.expires_at <= now()
                                    THEN EXCLUDED.used ELSE counter.used + EXCLUDED.used END,
                                expires_at = CASE WHEN counter.expires_at <= now()
                                    THEN EXCLUDED.expires_at ELSE counter.expires_at END
                            WHERE counter.expires_at <= now()
                               OR counter.used + EXCLUDED.used <= $5
                        RETURNING used,
                                  extract(epoch FROM expires_at - now())::float8 AS remaining
                    )
                    SELECT
                        (SELECT used FROM taken),
                        coalesce(
                            (SELECT remaining FROM taken),
                            (SELECT extract(epoch FROM expires_at - now())::float8
                             FROM rate_limit_counters
                             WHERE bucket = $1 AND key_digest = $2)
                        )
                    "#,
                )
                .bind(bucket.name)
                .bind(digest.as_slice())
                .bind(window)
                .bind(cost as i32)
                .bind(limit as i32)
                .bind(workspace)
                .fetch_one(&mut *connection)
                .await
            })
            .await?;
        let left = left.unwrap_or(f64::from(window));
        Ok(match used {
            Some(_) => Taken::Granted { left },
            None => Taken::Refused {
                retry_after: seconds_from(left),
            },
        })
    }
}

/// Where a limit's statement runs: on the pool, or on a connection the
/// caller holds.
enum On<'c> {
    Pool,
    Connection(&'c mut PgConnection),
}

enum Taken {
    /// `left`: seconds until the window ends.
    Granted {
        left: f64,
    },
    Refused {
        retry_after: u64,
    },
}

/// How much to reserve for a chunked bucket: about a twentieth of the
/// limit, at most `chunk`, at least the cost, never past the limit.
fn reservation(chunk: u32, limit: u32, cost: u32) -> u32 {
    (limit / 20).clamp(1, chunk.max(1)).max(cost).min(limit)
}

/// The slices of chunked budgets this instance holds.
#[derive(Default)]
struct ChunkBook {
    chunks: HashMap<(&'static str, [u8; 32]), Chunk>,
}

struct Chunk {
    left: u32,
    ends: Instant,
}

/// Slices a book keeps before it drops the ended ones.
const BOOK_CAPACITY: usize = 4_096;

impl ChunkBook {
    /// Spends `cost` from a slice already held; false when there is none or
    /// too little.
    fn spend(&mut self, bucket: &'static str, key: [u8; 32], cost: u32, now: Instant) -> bool {
        match self.chunks.get_mut(&(bucket, key)) {
            Some(chunk) if now < chunk.ends && chunk.left >= cost => {
                chunk.left -= cost;
                true
            }
            _ => false,
        }
    }

    /// Holds `left` more of `key`'s budget until `ends`. What was left of an
    /// earlier slice in the same window stays.
    fn grant(
        &mut self,
        bucket: &'static str,
        key: [u8; 32],
        left: u32,
        ends: Instant,
        now: Instant,
    ) {
        if self.chunks.len() >= BOOK_CAPACITY {
            self.chunks.retain(|_, chunk| chunk.ends > now);
        }
        match self.chunks.get_mut(&(bucket, key)) {
            Some(chunk) if chunk.ends > now => {
                chunk.left += left;
                chunk.ends = ends;
            }
            _ => {
                if self.chunks.len() < BOOK_CAPACITY {
                    self.chunks.insert((bucket, key), Chunk { left, ends });
                }
            }
        }
    }
}

/// Deletes counters whose window has ended (job `limits.cleanup`, every
/// minute).
pub async fn clean_up(pool: &PgPool) -> Result<u64, AppError> {
    let deleted = crate::db::scoped_one(pool, async |connection| {
        sqlx::query(
            r#"
            DELETE FROM rate_limit_counters
            WHERE ctid = ANY (ARRAY(
                SELECT ctid FROM rate_limit_counters WHERE expires_at <= now() LIMIT 10000
            ))
            "#,
        )
        .execute(&mut *connection)
        .await
    })
    .await?
    .rows_affected();
    Ok(deleted)
}

pub const CLEAN_UP_KIND: &str = "limits.cleanup";

pub fn register_jobs(registry: &mut crate::jobs::JobRegistry) {
    registry.register(CLEAN_UP_KIND, |context| async move {
        clean_up(&context.state.pool).await?;
        Ok(())
    });
    registry.every(CLEAN_UP_KIND, Duration::from_secs(60));
}

fn seconds(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() > 0))
        .max(1)
}

fn seconds_from(left: f64) -> u64 {
    if left.is_finite() && left > 0.0 {
        (left.ceil() as u64).max(1)
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{ChunkBook, LimitKey, reservation, seconds, seconds_from};

    #[test]
    fn a_reservation_is_a_twentieth_capped_by_the_chunk_and_the_limit() {
        assert_eq!(reservation(10, 240, 1), 10);
        assert_eq!(reservation(10, 120, 1), 6);
        assert_eq!(reservation(10, 5, 1), 1);
        assert_eq!(reservation(10, 240, 30), 30);
        assert_eq!(reservation(10, 4, 9), 4);
    }

    #[test]
    fn a_slice_is_spent_locally_until_it_runs_out_or_its_window_ends() {
        let mut book = ChunkBook::default();
        let now = Instant::now();
        let key = [7; 32];
        assert!(!book.spend("writes", key, 1, now));
        book.grant("writes", key, 2, now + Duration::from_secs(60), now);
        assert!(book.spend("writes", key, 1, now));
        assert!(book.spend("writes", key, 1, now));
        assert!(!book.spend("writes", key, 1, now));
        book.grant("writes", key, 3, now + Duration::from_secs(60), now);
        assert!(!book.spend("writes", key, 1, now + Duration::from_secs(61)));
        assert!(!book.spend("reads", key, 1, now));
    }

    /// However the reservations interleave, the slices handed out never
    /// add up to more than the limit, and what is left unused at the end of
    /// a window is at most a chunk per other instance.
    #[test]
    fn slices_never_add_up_past_the_limit() {
        let limit = 240;
        let chunk = 10;
        let instances = 3;
        let mut used = 0;
        let mut held = vec![0u32; instances];
        let mut spent = 0;
        for round in 0..1_000 {
            let instance = round % instances;
            if held[instance] > 0 {
                held[instance] -= 1;
                spent += 1;
                continue;
            }
            let mut granted = false;
            for size in [reservation(chunk, limit, 1), 1] {
                if used + size <= limit {
                    used += size;
                    held[instance] += size - 1;
                    spent += 1;
                    granted = true;
                    break;
                }
            }
            if !granted {
                break;
            }
        }
        assert!(used <= limit);
        assert!(spent <= limit);
        assert!(held.iter().sum::<u32>() <= chunk * (instances as u32 - 1));
    }

    #[test]
    fn keys_of_different_kinds_never_share_a_counter() {
        let id = uuid::Uuid::now_v7();
        assert_ne!(
            LimitKey::Session(id).digest(),
            LimitKey::Credential(id).digest()
        );
        assert_ne!(
            LimitKey::Secret(b"10.0.0.1").digest(),
            LimitKey::Address("10.0.0.1").digest()
        );
    }

    #[test]
    fn retry_after_rounds_up_to_whole_seconds() {
        assert_eq!(seconds(Duration::from_millis(1)), 1);
        assert_eq!(seconds(Duration::from_millis(1_001)), 2);
        assert_eq!(seconds_from(59.2), 60);
        assert_eq!(seconds_from(-3.0), 1);
    }
}
