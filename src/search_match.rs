//! Pencocokan teks bersama untuk semua kolom search di UI.
//!
//! Aturannya:
//! - substring persis (case-insensitive) selalu cocok dengan skor 1.0, jadi
//!   hasil pencarian lama tidak ada yang hilang;
//! - untuk query >= 3 karakter, teks yang mirip secara isi (cosine similarity
//!   embedding lokal dari [`crate::vector_index::embed_text`]) juga cocok,
//!   dengan skor di bawah 1.0.
//!
//! Banyak filter egui dievaluasi ulang setiap frame, jadi embedding disimpan
//! di cache per thread agar tiap teks hanya di-embed sekali.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Similarity minimum agar teks yang tidak mengandung query dianggap cocok.
pub const MIN_SIMILARITY: f32 = 0.5;

/// Panjang query minimum sebelum similarity dipakai; query lebih pendek
/// terlalu bising dan hanya memakai substring.
const MIN_SIMILARITY_QUERY_CHARS: usize = 3;

/// Batas entri cache; bila penuh cache dikosongkan (sederhana, cukup untuk UI).
const CACHE_LIMIT: usize = 50_000;

thread_local! {
    static EMBEDDING_CACHE: RefCell<HashMap<String, Option<Rc<Vec<f32>>>>> =
        RefCell::new(HashMap::new());
}

fn cached_embedding(text: &str) -> Option<Rc<Vec<f32>>> {
    EMBEDDING_CACHE.with(|cache| {
        if let Some(hit) = cache.borrow().get(text) {
            return hit.clone();
        }
        let embedding = crate::vector_index::embed_text(text).map(Rc::new);
        let mut cache = cache.borrow_mut();
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(text.to_string(), embedding.clone());
        embedding
    })
}

/// Query search yang sudah diproses (lowercase + embedding).
pub struct SearchQuery {
    lower: String,
    embedding: Option<Rc<Vec<f32>>>,
}

impl SearchQuery {
    pub fn new(query: &str) -> Self {
        let trimmed = query.trim();
        let embedding = if trimmed.chars().count() >= MIN_SIMILARITY_QUERY_CHARS {
            cached_embedding(trimmed)
        } else {
            None
        };
        Self {
            lower: trimmed.to_lowercase(),
            embedding,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lower.is_empty()
    }

    /// Skor kecocokan `text`: 1.0 untuk substring persis, similarity cosine
    /// (>= [`MIN_SIMILARITY`]) untuk yang mirip, `None` bila tidak cocok.
    /// Query kosong cocok dengan semua teks.
    pub fn score(&self, text: &str) -> Option<f32> {
        if self.lower.is_empty() || text.to_lowercase().contains(&self.lower) {
            return Some(1.0);
        }
        self.similarity(text).filter(|s| *s >= MIN_SIMILARITY)
    }

    pub fn matches(&self, text: &str) -> bool {
        self.score(text).is_some()
    }

    /// Skor terbaik dari beberapa field (mis. nama + deskripsi).
    pub fn best_score<'a>(&self, texts: impl IntoIterator<Item = &'a str>) -> Option<f32> {
        texts
            .into_iter()
            .filter_map(|t| self.score(t))
            .fold(None, |best, s| Some(best.map_or(s, |b: f32| b.max(s))))
    }

    pub fn matches_any<'a>(&self, texts: impl IntoIterator<Item = &'a str>) -> bool {
        self.best_score(texts).is_some()
    }

    /// Similarity cosine mentah (tanpa ambang); `None` bila query/teks tidak
    /// punya token bermakna.
    pub fn similarity(&self, text: &str) -> Option<f32> {
        let query = self.embedding.as_ref()?;
        let other = cached_embedding(text)?;
        // Kedua vektor sudah ternormalisasi L2, jadi dot product = cosine.
        Some(query.iter().zip(other.iter()).map(|(a, b)| a * b).sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zz_calibration_print() {
        let pairs = [
            ("customer email", "customers.email_address", true),
            ("customer email", "customers", true),
            ("order items", "order_item", true),
            ("invoices", "invoice_lines", true),
            ("cust", "customers", true),
            ("user role", "user_roles", true),
            ("payment method", "payment_methods", true),
            ("prodct", "products", true),
            ("slow query lock", "SELECT * FROM orders FOR UPDATE", false),
            ("customer", "warehouse_stock", false),
            ("invoice", "user_sessions", false),
            ("payment", "product_category", false),
            ("orders", "audit_logs", false),
            ("status", "states", false),
            ("user", "users_audit", true),
            ("category", "created_at", false),
            ("created", "updated_at", false),
            ("product", "production_schedule", false),
            ("stock", "customer_stock_alerts", true),
            ("login", "log_entries", false),
        ];
        for (q, t, want) in pairs {
            let sq = SearchQuery::new(q);
            println!("{:>6.3} want={} {q:?} vs {t:?}", sq.similarity(t).unwrap_or(-9.0), want);
        }
    }

    #[test]
    fn empty_query_matches_everything() {
        let q = SearchQuery::new("  ");
        assert!(q.is_empty());
        assert_eq!(q.score("anything"), Some(1.0));
    }

    #[test]
    fn exact_substring_still_matches() {
        let q = SearchQuery::new("ORD");
        assert_eq!(q.score("sales_orders"), Some(1.0));
        // Query pendek tidak memakai similarity.
        assert!(SearchQuery::new("zq").score("orders").is_none());
    }

    #[test]
    fn similar_text_matches_without_substring() {
        let q = SearchQuery::new("customer email");
        assert!(q.matches("customers.email_address"));
        let q = SearchQuery::new("order items");
        assert!(q.matches("order_item"));
        let q = SearchQuery::new("invoices");
        assert!(q.matches("invoice_lines"));
    }

    #[test]
    fn unrelated_text_does_not_match() {
        for (query, text) in [
            ("customer", "warehouse_stock"),
            ("invoice", "user_sessions"),
            ("payment", "product_category"),
            ("orders", "audit_logs"),
        ] {
            let q = SearchQuery::new(query);
            assert!(
                !q.matches(text),
                "{query} vs {text} similarity {:?}",
                q.similarity(text)
            );
        }
    }

    #[test]
    fn best_score_prefers_exact_field() {
        let q = SearchQuery::new("email");
        assert_eq!(q.best_score(["user_id", "email"]), Some(1.0));
        assert!(q.best_score(["user_id", "created_at"]).is_none());
    }
}
