//! Process-wide counters surfaced by the dashboard and `/api/status`.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

#[derive(Default)]
pub struct Stats {
    pub circuits_built: AtomicU64,
    pub circuits_failed: AtomicU64,
    pub circuits_active: AtomicI64,
    pub bytes_up: AtomicU64,
    pub bytes_down: AtomicU64,
}

impl Stats {
    pub fn built(&self) {
        self.circuits_built.fetch_add(1, Ordering::Relaxed);
    }
    pub fn failed(&self) {
        self.circuits_failed.fetch_add(1, Ordering::Relaxed);
    }
    pub fn active_inc(&self) {
        self.circuits_active.fetch_add(1, Ordering::Relaxed);
    }
    pub fn active_dec(&self) {
        self.circuits_active.fetch_sub(1, Ordering::Relaxed);
    }
    pub fn add_up(&self, n: u64) {
        self.bytes_up.fetch_add(n, Ordering::Relaxed);
    }
    pub fn add_down(&self, n: u64) {
        self.bytes_down.fetch_add(n, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        return StatsSnapshot {
            circuits_built: self.circuits_built.load(Ordering::Relaxed),
            circuits_failed: self.circuits_failed.load(Ordering::Relaxed),
            circuits_active: self.circuits_active.load(Ordering::Relaxed).max(0) as u64,
            bytes_up: self.bytes_up.load(Ordering::Relaxed),
            bytes_down: self.bytes_down.load(Ordering::Relaxed),
        };
    }
}

pub struct StatsSnapshot {
    pub circuits_built: u64,
    pub circuits_failed: u64,
    pub circuits_active: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
}
