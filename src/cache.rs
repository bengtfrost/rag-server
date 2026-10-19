use dashmap::DashMap;
use once_cell::sync::Lazy;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const MAX_CACHE_SIZE: usize = 1000;
const DEFAULT_TTL_SECS: u64 = 300;

pub struct QueryCache {
    inner: DashMap<String, CachedEntry>,
    ttl: Duration,
    start: Instant,
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

impl QueryCache {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            inner: DashMap::with_capacity(MAX_CACHE_SIZE),
            ttl: Duration::from_secs(ttl_secs),
            start: Instant::now(),
        }
    }

    #[inline]
    pub fn get(&self, key: &str) -> Option<Arc<Vec<String>>> {
        let entry = self.inner.get(key)?;
        if entry.is_expired(self.ttl) {
            drop(entry);
            self.inner.remove(key);
            return None;
        }
        let now_ms = self.start.elapsed().as_millis() as u64;
        entry.last_accessed_ms.store(now_ms, Ordering::Relaxed);
        Some(entry.results.clone())
    }

    pub fn insert(&self, key: String, results: Vec<String>) {
        if self.inner.len() >= MAX_CACHE_SIZE {
            self.evict_lru();
        }
        let now_ms = self.start.elapsed().as_millis() as u64;
        self.inner.insert(key, CachedEntry::new(results, now_ms));
    }

    fn evict_lru(&self) {
        let mut oldest_key: Option<String> = None;
        let mut oldest_access = u64::MAX;

        for entry in self.inner.iter() {
            // Prefer evicting an already-expired entry.
            if entry.is_expired(self.ttl) {
                oldest_key = Some(entry.key().clone());
                break;
            }
            let access = entry.last_accessed_ms.load(Ordering::Relaxed);
            if access < oldest_access {
                oldest_access = access;
                oldest_key = Some(entry.key().clone());
            }
        }

        if let Some(key) = oldest_key {
            self.inner.remove(&key);
        }
    }

    #[inline]
    pub fn clear(&self) {
        self.inner.clear();
    }
}

pub static QUERY_CACHE: Lazy<QueryCache> = Lazy::new(|| {
    let ttl = std::env::var("RAG_CACHE_TTL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_TTL_SECS);
    QueryCache::new(ttl)
});
