//! Run-slot bookkeeping for adapters that start a process per turn (Claude
//! Code, command-line harnesses): how many turns run on the host and per
//! account, refused rather than queued when full, freed when the session
//! drops.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use uuid::Uuid;

use super::adapter::{AccountHandle, HarnessError};

#[derive(Default)]
struct Counts {
    host: usize,
    accounts: HashMap<Uuid, usize>,
    stopped: HashMap<Uuid, u64>,
}

#[derive(Clone)]
pub struct RunSlots {
    limit: usize,
    counts: Arc<Mutex<Counts>>,
}

/// One held run slot; dropping it frees the slot.
pub struct RunSlot {
    slots: RunSlots,
    account_id: Uuid,
}

impl RunSlots {
    pub fn new(host_limit: usize) -> Self {
        Self {
            limit: host_limit.max(1),
            counts: Arc::default(),
        }
    }

    pub fn acquire(&self, account: &AccountHandle) -> Result<RunSlot, HarnessError> {
        let mut counts = self.lock();
        if counts.host >= self.limit {
            return Err(HarnessError::HostCapacity);
        }
        let used = counts.accounts.entry(account.id).or_default();
        if *used >= usize::from(account.max_concurrent_runs.max(1)) {
            return Err(HarnessError::AccountCapacity);
        }
        *used += 1;
        counts.host += 1;
        Ok(RunSlot {
            slots: self.clone(),
            account_id: account.id,
        })
    }

    pub fn running(&self, account_id: Uuid) -> usize {
        self.lock().accounts.get(&account_id).copied().unwrap_or(0)
    }

    /// Marks the account's sessions as stopping (sign-out).
    pub fn stop_account(&self, account_id: Uuid) {
        *self.lock().stopped.entry(account_id).or_default() += 1;
    }

    /// A stamp that changes when the account is stopped.
    pub fn stop_generation(&self, account_id: Uuid) -> u64 {
        self.lock().stopped.get(&account_id).copied().unwrap_or(0)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Counts> {
        self.counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Drop for RunSlot {
    fn drop(&mut self) {
        let mut counts = self.slots.lock();
        counts.host = counts.host.saturating_sub(1);
        if let Some(used) = counts.accounts.get_mut(&self.account_id) {
            *used = used.saturating_sub(1);
        }
    }
}
