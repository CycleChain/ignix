/*!
 * Server Statistics
 *
 * Counters a server keeps for INFO and CONFIG GET. Counting a command never
 * touches memory another thread writes: each worker thread counts in a
 * counter of its own, and INFO adds them up.
 */

use crossbeam::utils::CachePadded;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// A counter that one thread increments and any thread can read
#[derive(Debug, Default)]
pub(crate) struct LocalCounter(CachePadded<AtomicU64>);

impl LocalCounter {
    /// Add one. Only the thread that owns the counter may call this, so a
    /// plain load and store are enough (no atomic read-modify-write).
    pub(crate) fn increment(&self) {
        let value = self.0.load(Ordering::Relaxed);
        self.0.store(value + 1, Ordering::Relaxed);
    }

    fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Where a server accepts connections, and how it waits for them
#[derive(Debug, Clone, Copy)]
pub(crate) struct Listener {
    pub(crate) addr: SocketAddr,
    /// The event API, reported as INFO `multiplexing_api`
    pub(crate) api: &'static str,
}

/// Statistics of a server, as INFO reports them
#[derive(Debug)]
pub struct Stats {
    started: Instant,
    listener: OnceLock<Listener>,
    connections_received: AtomicU64,
    connected_clients: AtomicU64,
    /// One counter of executed commands per worker thread
    commands: Mutex<Vec<Arc<LocalCounter>>>,
    expired_keys: AtomicU64,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            listener: OnceLock::new(),
            connections_received: AtomicU64::new(0),
            connected_clients: AtomicU64::new(0),
            commands: Mutex::new(Vec::new()),
            expired_keys: AtomicU64::new(0),
        }
    }
}

impl Stats {
    /// How long ago the statistics were created (with the shard)
    pub fn uptime(&self) -> Duration {
        self.started.elapsed()
    }

    /// Connections accepted since the start
    pub fn connections_received(&self) -> u64 {
        self.connections_received.load(Ordering::Relaxed)
    }

    /// Connections open now
    pub fn connected_clients(&self) -> u64 {
        self.connected_clients.load(Ordering::Relaxed)
    }

    /// Commands executed for the server's connections since the start
    pub fn commands_processed(&self) -> u64 {
        let counters = self.commands.lock().unwrap_or_else(PoisonError::into_inner);
        counters.iter().map(|counter| counter.get()).sum()
    }

    /// Keys removed because they expired
    pub fn expired_keys(&self) -> u64 {
        self.expired_keys.load(Ordering::Relaxed)
    }

    pub(crate) fn listener(&self) -> Option<Listener> {
        self.listener.get().copied()
    }

    /// Record where the server listens; only the first call counts
    pub(crate) fn set_listener(&self, listener: Listener) {
        let _ = self.listener.set(listener);
    }

    /// A new counter of executed commands, for one worker thread
    pub(crate) fn command_counter(&self) -> Arc<LocalCounter> {
        let counter = Arc::new(LocalCounter::default());
        let mut counters = self.commands.lock().unwrap_or_else(PoisonError::into_inner);
        counters.push(counter.clone());
        counter
    }

    pub(crate) fn client_connected(&self) {
        self.connections_received.fetch_add(1, Ordering::Relaxed);
        self.connected_clients.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn client_disconnected(&self) {
        self.connected_clients.fetch_sub(1, Ordering::Relaxed);
    }

    pub(crate) fn key_expired(&self) {
        self.expired_keys.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_of_every_worker_are_added_up() {
        let stats = Stats::default();
        let (a, b) = (stats.command_counter(), stats.command_counter());
        a.increment();
        a.increment();
        std::thread::spawn(move || b.increment()).join().unwrap();
        assert_eq!(stats.commands_processed(), 3);
    }

    #[test]
    fn clients_are_counted_while_connected() {
        let stats = Stats::default();
        stats.client_connected();
        stats.client_connected();
        stats.client_disconnected();
        assert_eq!(stats.connected_clients(), 1);
        assert_eq!(stats.connections_received(), 2);
    }
}
