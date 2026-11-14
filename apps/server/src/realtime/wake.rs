//! The wake (K5 3.4): one listening connection per instance, outside the
//! pool, on this schema's `log`, `events` and `control` channels. A
//! notification only says "look now": the sequencer and the tail also poll,
//! so a lost wake costs a poll's delay and never an event (ruling R24 rule
//! 3). This is the only place that runs `LISTEN`.
//!
//! The listener reconnects with back-off from 100 ms to 5 s and wakes
//! everyone after each reconnect, since notifications sent while it was
//! away are gone. With `AGENT_WORKSPACE_DATABASE_LISTEN_URL=off` it never
//! connects and the instance runs on the polls alone, as behind a
//! transaction pooler.

use std::{str::FromStr, sync::Arc, time::Duration};

use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use tokio::sync::{Notify, watch};

/// The channels the listener hears, by suffix (`realtime_channel`).
pub const CHANNELS: [&str; 3] = ["log", "events", "control"];

const FIRST_RETRY: Duration = Duration::from_millis(100);
const LAST_RETRY: Duration = Duration::from_secs(5);

/// Who to wake for each channel.
#[derive(Clone, Default)]
pub struct Wakes {
    /// New staged events: run the sequencer.
    pub log: Arc<Notify>,
    /// New positions: run the tail.
    pub events: Arc<Notify>,
    /// A lease released, a host kick: every loop waiting on it looks.
    pub control: Arc<Notify>,
}

impl Wakes {
    /// Wake everyone, as after a reconnect.
    pub fn all(&self) {
        for suffix in CHANNELS {
            self.wake(suffix);
        }
    }

    /// Wake whoever follows `suffix`; false for a channel nobody follows.
    fn wake(&self, suffix: &str) -> bool {
        match suffix {
            // One loop each: a stored permit means a wake is never lost.
            "log" => self.log.notify_one(),
            "events" => self.events.notify_one(),
            // Any number of lease loops.
            "control" => self.control.notify_waiters(),
            _ => return false,
        }
        true
    }
}

/// Listen until shutdown, reconnecting as needed. `url` None: polls only.
/// `name` becomes the connection's `application_name`, so an operator can
/// tell the listeners apart.
pub async fn run(
    url: Option<String>,
    name: String,
    wakes: Wakes,
    mut shutdown: watch::Receiver<bool>,
) {
    let Some(url) = url else {
        crate::metrics::listener_up(false);
        return;
    };
    let mut retry = FIRST_RETRY;
    loop {
        if *shutdown.borrow() {
            break;
        }
        match listen(&url, &name, &wakes, &mut shutdown, &mut retry).await {
            Ok(()) => break,
            Err(error) => {
                crate::metrics::listener_up(false);
                tracing::warn!(
                    error_category = crate::error::AppError::from(error).log_category(),
                    "event listener lost; the polls carry on"
                );
            }
        }
        // Anything sent while the listener was away is gone.
        wakes.all();
        tokio::select! {
            () = tokio::time::sleep(retry) => {}
            _ = shutdown.changed() => {}
        }
        retry = (retry * 2).min(LAST_RETRY);
    }
    crate::metrics::listener_up(false);
}

async fn listen(
    url: &str,
    name: &str,
    wakes: &Wakes,
    shutdown: &mut watch::Receiver<bool>,
    retry: &mut Duration,
) -> Result<(), sqlx::Error> {
    let options = PgConnectOptions::from_str(url)?
        .application_name(name)
        .statement_cache_capacity(0);
    // A pool of one, used only to reconnect.
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .min_connections(0)
        .max_lifetime(None)
        .idle_timeout(None)
        .connect_with(options)
        .await?;
    let mut listener = PgListener::connect_with(&pool).await?;
    listener.ignore_pool_close_event(true);
    let channels = sqlx::query_scalar::<_, String>(
        "SELECT realtime_channel(suffix) FROM unnest($1::text[]) suffix",
    )
    .bind(CHANNELS.map(str::to_owned).to_vec())
    .fetch_all(&mut listener)
    .await?;
    let prefix = channels
        .first()
        .and_then(|channel| channel.strip_suffix(CHANNELS[0]))
        .unwrap_or_default()
        .to_owned();
    listener
        .listen_all(channels.iter().map(String::as_str))
        .await?;
    crate::metrics::listener_up(true);
    *retry = FIRST_RETRY;
    // Whatever happened before we listened.
    wakes.all();
    loop {
        tokio::select! {
            notification = listener.try_recv() => match notification? {
                Some(notification) => {
                    let suffix = notification.channel().strip_prefix(prefix.as_str()).unwrap_or("");
                    wakes.wake(suffix);
                }
                // The connection dropped; the next receive reconnects.
                None => {
                    crate::metrics::listener_up(false);
                    wakes.all();
                }
            },
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return Ok(());
                }
            }
        }
    }
}

/// The listen URL from configuration: the main URL unless set, None when
/// set to `off`.
pub fn listen_url(configured: Option<&str>, main: &str) -> Option<String> {
    match configured.map(str::trim) {
        Some("off") => None,
        Some(url) if !url.is_empty() => Some(url.to_owned()),
        _ => Some(main.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listen_url_defaults_to_the_main_one_and_can_be_off() {
        assert_eq!(listen_url(None, "main").as_deref(), Some("main"));
        assert_eq!(listen_url(Some(""), "main").as_deref(), Some("main"));
        assert_eq!(
            listen_url(Some("direct"), "main").as_deref(),
            Some("direct")
        );
        assert_eq!(listen_url(Some("off"), "main"), None);
    }

    #[test]
    fn every_channel_wakes_someone() {
        let wakes = Wakes::default();
        assert!(CHANNELS.iter().all(|channel| wakes.wake(channel)));
        assert!(!wakes.wake("elsewhere"));
    }

    #[test]
    fn channel_names_fit_postgres_identifiers_for_any_schema() {
        // `realtime_channel`: 'aw_' || left(md5(schema), 12) || '_' || suffix,
        // so the schema's length never matters; PostgreSQL keeps 63 bytes.
        let longest = CHANNELS
            .iter()
            .map(|suffix| suffix.len())
            .max()
            .unwrap_or(0);
        assert!("aw_".len() + 12 + 1 + longest < 63);
    }
}
