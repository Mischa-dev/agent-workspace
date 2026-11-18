//! The host shutting down: adapters stop taking work and wind down.

use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

#[derive(Default)]
pub struct ShutdownSignal {
    shutting_down: AtomicBool,
    notify: Notify,
}

impl ShutdownSignal {
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    pub fn request(&self) {
        self.shutting_down.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    pub async fn requested(&self) {
        let notified = self.notify.notified();
        if self.is_shutting_down() {
            return;
        }
        notified.await;
    }
}
