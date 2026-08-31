use dashmap::DashMap;
use once_cell::sync::Lazy;
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_CACHE_SIZE: usize = 1000;
const DEFAULT_TTL_SECS: u64 = 300;

pub struct QueryCache {
    inner: DashMap<String, CachedEntry>,
    ttl: Duration,
}

struct CachedEntry {
    results: Arc<Vec<String>>,
    created_at: Instant,
}

impl CachedEntry {
    fn new(results: Vec<String>) -> Self {
        Self {
            results: Arc::new(results),
            created_at: Instant::now(),
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
        }
    }

    #[inline]
    pub fn get(&self, key: &str) -> Option<Arc<Vec<String>>> {
        if let Some(entry) = self.inner.get(key) {
            if !entry.value().is_expired(self.ttl) {
                return Some(entry.value().results.clone());
            }
            drop(entry);
            self.inner.remove(key);
        }
        None
    }

    pub fn insert(&self, key: String, results: Vec<String>) {
        if self.inner.len() >= MAX_CACHE_SIZE {
            self.evict_lru();
        }
        let entry = CachedEntry::new(results);
        self.inner.insert(key, entry);
    }

    fn evict_lru(&self) {
        let mut oldest_key: Option<String> = None;
        let mut oldest_time = Instant::now();

        for entry in self.inner.iter() {
            let entry_ref = entry.value();
            let time = entry_ref.created_at;

            if entry_ref.is_expired(self.ttl) {
                oldest_key = Some(entry.key().clone());
                break;
            }

            if time < oldest_time {
                oldest_time = time;
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

    // Ta bort eller kommentera bort oanvända metoder
    // pub fn len(&self) -> usize { self.inner.len() }
    // pub fn is_empty(&self) -> bool { self.inner.is_empty() }
}

pub static QUERY_CACHE: Lazy<QueryCache> = Lazy::new(|| {
    let ttl = std::env::var("RAG_CACHE_TTL")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_TTL_SECS);
    QueryCache::new(ttl)
});

