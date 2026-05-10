use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{Mutex, Notify};

pub struct SegmentCache {
    pub(super) inner: Mutex<CacheInner>,
    pub(super) inflight: Mutex<HashMap<String, Arc<Inflight>>>,
    hits: AtomicU64,
    pub(super) misses: AtomicU64,
    evictions: AtomicU64,
}

pub(super) struct CacheInner {
    // key insertion order; back = most recently used
    pub(super) order: Vec<String>,
    pub(super) map: HashMap<String, CacheEntry>,
    pub(super) bytes: u64,
    pub(super) budget: u64,
}

#[derive(Clone)]
pub(super) struct CacheEntry {
    pub(super) bytes: Arc<Vec<u8>>,
    pub(super) content_type: &'static str,
}

pub(super) struct Inflight {
    pub(super) outcome: Mutex<Option<std::result::Result<(), String>>>,
    pub(super) notify: Notify,
}

pub struct CacheSnapshot {
    pub size_bytes: u64,
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

impl SegmentCache {
    pub fn new(budget_bytes: u64) -> Self {
        Self {
            inner: Mutex::new(CacheInner {
                order: Vec::new(),
                map: HashMap::new(),
                bytes: 0,
                budget: budget_bytes,
            }),
            inflight: Mutex::new(HashMap::new()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> CacheSnapshot {
        let inner = self.inner.try_lock();
        let (size_bytes, entries) = match inner {
            Ok(g) => (g.bytes, g.map.len()),
            Err(_) => (0, 0),
        };
        CacheSnapshot {
            size_bytes,
            entries,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }

    pub async fn free_bytes(&self) -> u64 {
        let g = self.inner.lock().await;
        g.budget.saturating_sub(g.bytes)
    }

    pub(super) async fn get(&self, key: &str) -> Option<CacheEntry> {
        let mut g = self.inner.lock().await;
        if let Some(entry) = g.map.get(key).cloned() {
            // bump to back
            if let Some(pos) = g.order.iter().position(|k| k == key) {
                let k = g.order.remove(pos);
                g.order.push(k);
            }
            self.hits.fetch_add(1, Ordering::Relaxed);
            Some(entry)
        } else {
            None
        }
    }

    pub(super) async fn insert(
        &self,
        key: String,
        entry: CacheEntry,
        budget_override: Option<u64>,
    ) {
        let mut g = self.inner.lock().await;
        if let Some(b) = budget_override {
            g.budget = b;
        }
        if let Some(prev) = g.map.remove(&key) {
            g.bytes = g.bytes.saturating_sub(prev.bytes.len() as u64);
            if let Some(pos) = g.order.iter().position(|k| k == &key) {
                g.order.remove(pos);
            }
        }
        let size = entry.bytes.len() as u64;
        g.bytes += size;
        g.map.insert(key.clone(), entry);
        g.order.push(key);
        while g.bytes > g.budget && !g.order.is_empty() {
            let victim = g.order.remove(0);
            if let Some(e) = g.map.remove(&victim) {
                g.bytes = g.bytes.saturating_sub(e.bytes.len() as u64);
                self.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

pub(super) async fn claim_inflight(
    state: &super::super::AppState,
    cache_key: &str,
) -> (Arc<Inflight>, bool) {
    let mut map = state.cache.inflight.lock().await;
    if let Some(existing) = map.get(cache_key).cloned() {
        return (existing, false);
    }
    let new = Arc::new(Inflight {
        outcome: Mutex::new(None),
        notify: Notify::new(),
    });
    map.insert(cache_key.to_string(), new.clone());
    (new, true)
}

pub(super) async fn finish_inflight(
    state: &super::super::AppState,
    cache_key: &str,
    inflight: Arc<Inflight>,
    result: &Result<CacheEntry>,
) {
    {
        let mut outcome = inflight.outcome.lock().await;
        *outcome = Some(match result {
            Ok(_) => Ok(()),
            Err(e) => Err(e.to_string()),
        });
    }
    state.cache.inflight.lock().await.remove(cache_key);
    inflight.notify.notify_waiters();
}
