use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use super::logical::LogicalQueryPlan;

#[derive(Clone)]
pub struct PlanEntry {
    pub plan: Arc<LogicalQueryPlan>,
    pub sql: String,
    pub headers: Vec<String>,
}

const MAX_ENTRIES: usize = 128;
// Phase A-F upgrades (CTE inlining, canonical fingerprint based cache key semantics)
// Bump cache version to invalidate previous entries whose emitted SQL could differ
const CACHE_VERSION: u8 = 3; // bump: added TableScan alias + rule engine + correlation

#[derive(Default)]
pub struct PlanCache {
    inner: Mutex<(HashMap<String, PlanEntry>, VecDeque<String>)>,
    hits: std::sync::atomic::AtomicU64,
    misses: std::sync::atomic::AtomicU64,
}

impl PlanCache {
    pub fn global() -> &'static PlanCache {
        static INSTANCE: once_cell::sync::Lazy<PlanCache> =
            once_cell::sync::Lazy::new(PlanCache::default);
        &INSTANCE
    }
    pub fn get(&self, key: &str) -> Option<PlanEntry> {
        let versioned = format!("v{}::{}", CACHE_VERSION, key);
        let out = self.inner.lock().ok()?.0.get(&versioned).cloned();
        if out.is_some() {
            self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            self.misses
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        out
    }
    pub fn insert(&self, key: String, entry: PlanEntry) {
        if let Ok(mut guard) = self.inner.lock() {
            let (map, order) = &mut *guard;
            let versioned = format!("v{}::{}", CACHE_VERSION, key);
            if !map.contains_key(&versioned) {
                order.push_back(versioned.clone());
            }
            map.insert(versioned.clone(), entry);
            while order.len() > MAX_ENTRIES {
                if let Some(old_key) = order.pop_front() {
                    map.remove(&old_key);
                }
            }
        }
    }
    pub fn stats(&self) -> (u64, u64) {
        (
            self.hits.load(std::sync::atomic::Ordering::Relaxed),
            self.misses.load(std::sync::atomic::Ordering::Relaxed),
        )
    }
}
