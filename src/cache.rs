use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use lazy_static::lazy_static;

lazy_static! {
    pub static ref QUERY_CACHE: Mutex<QueryCache> = Mutex::new(QueryCache::new(300));
}

#[derive(Clone)]
pub struct QueryCache {
    cache: HashMap<String, (Instant, Vec<String>)>,
    ttl: Duration,
}

impl QueryCache {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            cache: HashMap::new(),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    pub fn get(&mut self, key: &str) -> Option<Vec<String>> {
        if let Some((time, results)) = self.cache.get(key) {
            if time.elapsed() < self.ttl {
                return Some(results.clone());
            }
        }
        None
    }

    pub fn insert(&mut self, key: String, results: Vec<String>) {
        if self.cache.len() > 1000 {
            self.cache.retain(|_, (time, _)| time.elapsed() < self.ttl);
        }
        self.cache.insert(key, (Instant::now(), results));
    }

    pub fn clear(&mut self) {
        self.cache.clear();
    }
}