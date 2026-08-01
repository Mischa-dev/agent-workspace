use std::{
    collections::HashMap,
    hash::Hash,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::error::AppError;

const WINDOW: Duration = Duration::from_secs(60);
const LOGIN_ATTEMPTS_PER_WINDOW: u32 = 10;
const GLOBAL_LOGIN_ATTEMPTS_PER_WINDOW: u32 = 60;
const AUTHENTICATED_WRITES_PER_WINDOW: u32 = 240;
const INVITATION_ATTEMPTS_PER_WINDOW: u32 = 20;
const GLOBAL_INVITATION_ATTEMPTS_PER_WINDOW: u32 = 120;
const MAX_LOGIN_KEYS: usize = 4_096;
const MAX_INVITATION_KEYS: usize = 4_096;
const MAX_SESSION_KEYS: usize = 4_096;
const CONCURRENT_PASSWORD_KDFS: usize = 2;

#[derive(Clone)]
pub struct RateLimiters {
    login: FixedWindowLimiter<[u8; 32]>,
    global_login: FixedWindowLimiter<()>,
    authenticated_writes: FixedWindowLimiter<Uuid>,
    invitation_attempts: FixedWindowLimiter<[u8; 32]>,
    global_invitation_attempts: FixedWindowLimiter<()>,
    password_kdfs: Arc<Semaphore>,
}

impl RateLimiters {
    pub fn check_login(&self, normalized_login: &str) -> Result<(), AppError> {
        self.global_login
            .check(())
            .map_err(AppError::rate_limited)?;
        let key: [u8; 32] = Sha256::digest(normalized_login.as_bytes()).into();
        self.login.check(key).map_err(AppError::rate_limited)
    }

    pub fn try_acquire_password_kdf(&self) -> Result<OwnedSemaphorePermit, AppError> {
        Arc::clone(&self.password_kdfs)
            .try_acquire_owned()
            .map_err(|_| AppError::rate_limited(1))
    }

    pub fn check_authenticated_write(&self, session_id: Uuid) -> Result<(), AppError> {
        self.authenticated_writes
            .check(session_id)
            .map_err(AppError::rate_limited)
    }

    pub fn check_invitation(&self, token_prefix: &str) -> Result<(), AppError> {
        self.global_invitation_attempts
            .check(())
            .map_err(AppError::rate_limited)?;
        let key: [u8; 32] = Sha256::digest(token_prefix.as_bytes()).into();
        self.invitation_attempts
            .check(key)
            .map_err(AppError::rate_limited)
    }
}

impl Default for RateLimiters {
    fn default() -> Self {
        Self {
            login: FixedWindowLimiter::new(LOGIN_ATTEMPTS_PER_WINDOW, WINDOW, MAX_LOGIN_KEYS),
            global_login: FixedWindowLimiter::new(GLOBAL_LOGIN_ATTEMPTS_PER_WINDOW, WINDOW, 1),
            authenticated_writes: FixedWindowLimiter::new(
                AUTHENTICATED_WRITES_PER_WINDOW,
                WINDOW,
                MAX_SESSION_KEYS,
            ),
            invitation_attempts: FixedWindowLimiter::new(
                INVITATION_ATTEMPTS_PER_WINDOW,
                WINDOW,
                MAX_INVITATION_KEYS,
            ),
            global_invitation_attempts: FixedWindowLimiter::new(
                GLOBAL_INVITATION_ATTEMPTS_PER_WINDOW,
                WINDOW,
                1,
            ),
            password_kdfs: Arc::new(Semaphore::new(CONCURRENT_PASSWORD_KDFS)),
        }
    }
}

#[derive(Clone)]
struct FixedWindowLimiter<K> {
    limit: u32,
    window: Duration,
    capacity: usize,
    entries: Arc<Mutex<HashMap<K, WindowEntry>>>,
}

#[derive(Clone, Copy)]
struct WindowEntry {
    started_at: Instant,
    attempts: u32,
}

impl<K> FixedWindowLimiter<K>
where
    K: Eq + Hash,
{
    fn new(limit: u32, window: Duration, capacity: usize) -> Self {
        debug_assert!(limit > 0);
        debug_assert!(!window.is_zero());
        debug_assert!(capacity > 0);
        Self {
            limit,
            window,
            capacity,
            entries: Arc::new(Mutex::new(HashMap::with_capacity(capacity))),
        }
    }

    fn check(&self, key: K) -> Result<(), u64> {
        self.check_at(key, Instant::now())
    }

    fn check_at(&self, key: K, now: Instant) -> Result<(), u64> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if let Some(entry) = entries.get_mut(&key) {
            let elapsed = now.saturating_duration_since(entry.started_at);
            if elapsed >= self.window {
                *entry = WindowEntry {
                    started_at: now,
                    attempts: 1,
                };
                return Ok(());
            }
            if entry.attempts >= self.limit {
                return Err(retry_after_seconds(self.window - elapsed));
            }
            entry.attempts += 1;
            return Ok(());
        }

        if entries.len() >= self.capacity {
            entries
                .retain(|_, entry| now.saturating_duration_since(entry.started_at) < self.window);
        }
        if entries.len() >= self.capacity {
            let retry_after = entries
                .values()
                .map(|entry| {
                    self.window
                        .saturating_sub(now.saturating_duration_since(entry.started_at))
                })
                .min()
                .unwrap_or(self.window);
            return Err(retry_after_seconds(retry_after));
        }

        entries.insert(
            key,
            WindowEntry {
                started_at: now,
                attempts: 1,
            },
        );
        Ok(())
    }
}

fn retry_after_seconds(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() > 0))
        .max(1)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use uuid::Uuid;

    use super::{CONCURRENT_PASSWORD_KDFS, FixedWindowLimiter, RateLimiters};
    use crate::error::AppError;

    #[test]
    fn fixed_window_allows_the_limit_then_reports_retry_after() {
        let limiter = FixedWindowLimiter::new(2, Duration::from_secs(60), 8);
        let start = Instant::now();

        assert!(limiter.check_at("login", start).is_ok());
        assert!(limiter.check_at("login", start).is_ok());
        assert_eq!(
            limiter.check_at("login", start + Duration::from_millis(1)),
            Err(60)
        );
        assert!(
            limiter
                .check_at("login", start + Duration::from_secs(60))
                .is_ok()
        );
    }

    #[test]
    fn fixed_window_storage_is_bounded_and_expired_keys_are_reclaimed() {
        let limiter = FixedWindowLimiter::new(2, Duration::from_secs(60), 2);
        let start = Instant::now();

        assert!(limiter.check_at("first", start).is_ok());
        assert!(limiter.check_at("second", start).is_ok());
        assert_eq!(limiter.check_at("third", start), Err(60));
        assert!(
            limiter
                .check_at("third", start + Duration::from_secs(60))
                .is_ok()
        );
    }

    #[test]
    fn application_limits_are_ten_logins_and_240_writes_per_minute() {
        let limiters = RateLimiters::default();
        for _ in 0..10 {
            assert!(limiters.check_login("mischa").is_ok());
        }
        assert!(matches!(
            limiters.check_login("mischa"),
            Err(AppError::RateLimited { .. })
        ));
        assert!(
            limiters.check_login("another-login").is_ok(),
            "login names have independent digest keys"
        );

        let session_id = Uuid::now_v7();
        for _ in 0..240 {
            assert!(limiters.check_authenticated_write(session_id).is_ok());
        }
        assert!(matches!(
            limiters.check_authenticated_write(session_id),
            Err(AppError::RateLimited { .. })
        ));
        assert!(
            limiters.check_authenticated_write(Uuid::now_v7()).is_ok(),
            "sessions have independent write keys"
        );
    }

    #[test]
    fn global_login_budget_and_kdf_concurrency_are_bounded() {
        let limiters = RateLimiters::default();
        for index in 0..60 {
            assert!(
                limiters.check_login(&format!("login-{index}")).is_ok(),
                "unique login {index} remains inside the global budget"
            );
        }
        assert!(matches!(
            limiters.check_login("login-over-global-budget"),
            Err(AppError::RateLimited { .. })
        ));

        let mut permits = Vec::new();
        for _ in 0..CONCURRENT_PASSWORD_KDFS {
            permits.push(
                limiters
                    .try_acquire_password_kdf()
                    .expect("bounded KDF permit"),
            );
        }
        assert!(matches!(
            limiters.try_acquire_password_kdf(),
            Err(AppError::RateLimited { .. })
        ));
        drop(permits);
        assert!(limiters.try_acquire_password_kdf().is_ok());
    }

    #[test]
    fn invitation_attempts_are_token_scoped_and_globally_bounded() {
        let limiters = RateLimiters::default();
        for _ in 0..20 {
            assert!(limiters.check_invitation("aw_inv_first").is_ok());
        }
        assert!(matches!(
            limiters.check_invitation("aw_inv_first"),
            Err(AppError::RateLimited { .. })
        ));
        assert!(limiters.check_invitation("aw_inv_second").is_ok());
    }
}
