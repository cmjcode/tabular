use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::logical::LogicalQueryPlan;

#[derive(Clone)]
pub struct PlanEntry {
    pub plan: Arc<LogicalQueryPlan>,
    pub sql: String,
    pub headers: Vec<String>,
}

#[derive(Default)]
pub struct PlanCache { inner: Mutex<HashMap<String, PlanEntry>> }

impl PlanCache {
    pub fn global() -> &'static PlanCache { static INSTANCE: once_cell::sync::Lazy<PlanCache> = once_cell::sync::Lazy::new(|| PlanCache::default()); &INSTANCE }
    pub fn get(&self, key: &str) -> Option<PlanEntry> { self.inner.lock().ok()?.get(key).cloned() }
    pub fn insert(&self, key: String, entry: PlanEntry) { if let Ok(mut g) = self.inner.lock() { g.insert(key, entry); } }
}
