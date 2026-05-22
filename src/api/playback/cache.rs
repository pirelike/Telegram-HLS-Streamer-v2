use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{Mutex, Notify};

pub struct SegmentCache {
    pub(super) inner: Mutex<CacheInner>,
    pub(super) inflight: Mutex<HashMap<String, Arc<Inflight>>>,
    hits: AtomicU64,
    pub(super) misses: AtomicU64,
    evictions: AtomicU64,
    // Mirrors CacheInner.bytes / map.len() for lock-free reads in snapshot().
    // Updated under the inner lock so they stay consistent with the map;
    // momentary skew between bytes and entries is acceptable for metrics.
    bytes: AtomicU64,
    entries: AtomicUsize,
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
    pub(super) file_path: Option<PathBuf>,
    pub(super) content_type: &'static str,
}

pub(super) struct Inflight {
    pub(super) outcome: Mutex<Option<std::result::Result<Option<CacheEntry>, String>>>,
    pub(super) notify: Notify,
}

impl Inflight {
    pub(super) async fn wait_for_outcome(&self) -> std::result::Result<Option<CacheEntry>, String> {
        loop {
            let notified = self.notify.notified();
            if let Some(outcome) = self.outcome.lock().await.clone() {
                return outcome;
            }
            notified.await;
        }
    }
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
            bytes: AtomicU64::new(0),
            entries: AtomicUsize::new(0),
        }
    }

    pub fn snapshot(&self) -> CacheSnapshot {
        CacheSnapshot {
            size_bytes: self.bytes.load(Ordering::Relaxed),
            entries: self.entries.load(Ordering::Relaxed),
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

    pub(super) async fn get_bytes(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        self.get(key).await.map(|entry| entry.bytes)
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
        let is_new_key = !g.map.contains_key(&key);
        if let Some(prev) = g.map.remove(&key) {
            let prev_size = prev.bytes.len() as u64;
            g.bytes = g.bytes.saturating_sub(prev_size);
            self.bytes.fetch_sub(prev_size, Ordering::Relaxed);
            remove_cache_file(prev.file_path);
            if let Some(pos) = g.order.iter().position(|k| k == &key) {
                g.order.remove(pos);
            }
        }
        let size = entry.bytes.len() as u64;
        g.bytes += size;
        self.bytes.fetch_add(size, Ordering::Relaxed);
        g.map.insert(key.clone(), entry);
        g.order.push(key);
        if is_new_key {
            self.entries.fetch_add(1, Ordering::Relaxed);
        }
        while g.bytes > g.budget && !g.order.is_empty() {
            let victim = g.order.remove(0);
            if let Some(e) = g.map.remove(&victim) {
                let e_size = e.bytes.len() as u64;
                g.bytes = g.bytes.saturating_sub(e_size);
                self.bytes.fetch_sub(e_size, Ordering::Relaxed);
                self.entries.fetch_sub(1, Ordering::Relaxed);
                self.evictions.fetch_add(1, Ordering::Relaxed);
                remove_cache_file(e.file_path);
            }
        }
    }

    pub async fn drop_disk_files(&self) {
        let mut paths = Vec::new();
        {
            let mut g = self.inner.lock().await;
            for entry in g.map.values_mut() {
                if let Some(path) = entry.file_path.take() {
                    paths.push(path);
                }
            }
        }
        for path in paths {
            remove_cache_file(Some(path));
        }
    }
}

fn remove_cache_file(path: Option<PathBuf>) {
    if let Some(path) = path {
        tokio::spawn(async move {
            let _ = tokio::fs::remove_file(path).await;
        });
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
            Ok(entry) => Ok(Some(entry.clone())),
            Err(e) => Err(e.to_string()),
        });
    }
    inflight.notify.notify_waiters();
    state.cache.inflight.lock().await.remove(cache_key);
}
