//! In-memory query result cache.
//!
//! The process-wide instance is [`QUERY_CACHE`]; call sites should not
//! construct their own `QueryCache` unless they specifically want an
//! isolated cache (tests, benches).
//!
//! Concurrency notes:
//!  - `DashMap` shards internally; `get`/`insert` are lock-free for the
//!    read path and short-critical-section for the write path.
//!  - Expiry and LRU eviction both use `remove_if` so a concurrent
//!    re-insert of the same key cannot be silently dropped.

use dashmap::DashMap;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const MAX_CACHE_SIZE: usize = 1000;
const DEFAULT_TTL_SECS: u64 = 300;

/// Process-wide query cache. TTL 300s, capacity 1000.
///
/// This is the one every call site should use:
///
/// ```ignore
/// if let Some(hit) = crate::cache::QUERY_CACHE.get(&key) {
///     return Ok(hit);
/// }
/// // … compute …
/// crate::cache::QUERY_CACHE.insert(key, results);
/// ```
pub static QUERY_CACHE: LazyLock<QueryCache> = LazyLock::new(QueryCache::default);

pub struct QueryCache {
    inner: DashMap<String, CachedEntry>,
    ttl: Duration,
    start: Instant,
    max_size: usize,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
}

struct CachedEntry {
    results: Arc<Vec<String>>,
    created_at: Instant,
    last_accessed_ms: AtomicU64,
}

impl CachedEntry {
    fn new(results: Vec<String>, now_ms: u64) -> Self {
        Self {
            results: Arc::new(results),
            created_at: Instant::now(),
            last_accessed_ms: AtomicU64::new(now_ms),
        }
    }

    #[inline]
    fn is_expired(&self, ttl: Duration) -> bool {
        self.created_at.elapsed() > ttl
    }
}

impl Default for QueryCache {
    fn default() -> Self {
        Self::new(DEFAULT_TTL_SECS)
    }
}

impl QueryCache {
    pub fn new(ttl_secs: u64) -> Self {
        Self::with_capacity(ttl_secs, MAX_CACHE_SIZE)
    }

    pub fn with_capacity(ttl_secs: u64, max_size: usize) -> Self {
        Self {
            inner: DashMap::with_capacity(max_size.min(64)),
            ttl: Duration::from_secs(ttl_secs),
            start: Instant::now(),
            max_size,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    #[inline]
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// Look up `key`. Returns `None` on miss or expired entry.
    ///
    /// Note the return type is `Arc<Vec<String>>` — cloning it is O(1)
    /// and lets the caller hold the results across a lock boundary
    /// without copying the `Vec`.
    #[inline]
    pub fn get(&self, key: &str) -> Option<Arc<Vec<String>>> {
        let Some(entry) = self.inner.get(key) else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        };

        if entry.is_expired(self.ttl) {
            drop(entry);
            // Only remove if the entry we just observed is *still* the
            // expired one — a concurrent insert of the same key must win.
            self.inner.remove_if(key, |_, e| e.is_expired(self.ttl));
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        }

        entry
            .last_accessed_ms
            .store(self.now_ms(), Ordering::Relaxed);
        let out = entry.results.clone();
        drop(entry);
        self.hits.fetch_add(1, Ordering::Relaxed);
        Some(out)
    }

    #[inline]
    pub fn insert(&self, key: String, results: Vec<String>) {
        if self.max_size == 0 {
            return;
        }
        let now = self.now_ms();
        // Insert first, trim after — two concurrent inserts can no longer
        // both observe `len == max_size` and both evict, overshooting the cap.
        self.inner.insert(key, CachedEntry::new(results, now));
        if self.inner.len() > self.max_size {
            self.evict_one();
        }
    }

    /// Drop expired entries in bulk; if none were expired, evict the single
    /// least-recently-used entry.
    #[cold]
    fn evict_one(&self) {
        let before = self.inner.len();
        self.inner.retain(|_, e| !e.is_expired(self.ttl));
        if self.inner.len() < before {
            return;
        }

        let mut oldest: Option<(String, u64)> = None;
        for entry in &self.inner {
            let ts = entry.last_accessed_ms.load(Ordering::Relaxed);
            if oldest.as_ref().is_none_or(|(_, t)| ts < *t) {
                oldest = Some((entry.key().clone(), ts));
            }
        }

        let Some((key, ts)) = oldest else { return };
        // Re-check the timestamp so we never drop an entry that was touched
        // between the LRU scan and this call.
        let removed = self.inner.remove_if(&key, |_, e| {
            e.last_accessed_ms.load(Ordering::Relaxed) == ts
        });
        if removed.is_some() {
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn clear(&self) {
        self.inner.clear();
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns `(hits, misses, evictions)`.
    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
            self.evictions.load(Ordering::Relaxed),
        )
    }
}
