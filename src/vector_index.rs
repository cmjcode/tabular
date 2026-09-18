//! Indeks vektor lokal berbasis `sqlite-vec`.
//!
//! Dipakai untuk tiga hal:
//! - memilih tabel yang paling relevan dengan pertanyaan user saat menyusun
//!   konteks skema untuk AI assistant (retrieval),
//! - pencarian history query yang mirip secara isi di Quick Open, dan
//! - mencari potongan catatan vault Obsidian yang relevan sebagai memory AI
//!   assistant (lihat [`crate::obsidian`]).
//!
//! Embedding dibuat secara lokal dengan *feature hashing* (token identifier +
//! trigram karakter), bukan lewat API provider: tidak semua provider punya
//! endpoint embeddings, SQL user tidak perlu dikirim keluar, dan cara ini
//! jalan offline di desktop maupun mobile.
//!
//! Vektor disimpan sebagai BLOB float32 di tabel SQLite biasa, lalu diurutkan
//! dengan `vec_distance_cosine`. Jumlah baris per koneksi kecil (ratusan
//! sampai ribuan tabel, maksimal 150 history), jadi scan penuh sudah cepat dan
//! filter/upsert tetap memakai SQL standar.

use std::collections::{HashMap, HashSet};
use std::sync::Once;

use sqlx::SqlitePool;

/// Dimensi vektor embedding.
pub const EMBEDDING_DIM: usize = 256;

/// Naikkan jika algoritma embedding berubah agar semua vektor dihitung ulang.
const EMBEDDER_VERSION: u64 = 1;

/// Jarak cosine maksimum agar history dianggap "mirip".
pub const HISTORY_MAX_DISTANCE: f32 = 0.7;

/// Jarak cosine maksimum untuk pencarian tabel. Dokumen tabel berisi banyak
/// nama kolom sehingga kemiripannya lebih "encer" dibanding nama saja.
pub const TABLE_MAX_DISTANCE: f32 = 0.65;

/// Jarak cosine maksimum untuk potongan catatan. Prosa lebih beragam daripada
/// nama tabel, jadi ambangnya lebih longgar; urutan akhir diperbaiki dengan
/// kecocokan kata kunci di [`search_notes`].
pub const NOTE_MAX_DISTANCE: f32 = 0.85;

static REGISTER: Once = Once::new();

/// Daftarkan `sqlite-vec` sebagai auto-extension untuk semua koneksi SQLite
/// yang dibuka setelah fungsi ini dipanggil. Wajib dipanggil sebelum pool
/// pertama dibuat; aman dipanggil berkali-kali.
pub fn register_sqlite_vec() {
    REGISTER.call_once(|| {
        type EntryPoint = unsafe extern "C" fn(
            *mut libsqlite3_sys::sqlite3,
            *mut *mut std::ffi::c_char,
            *const libsqlite3_sys::sqlite3_api_routines,
        ) -> std::ffi::c_int;

        // SAFETY: `sqlite3_vec_init` adalah entry point extension SQLite dengan
        // signature (db, pzErrMsg, pApi) -> int; binding crate hanya
        // mendeklarasikannya tanpa parameter. Dikompilasi dengan SQLITE_CORE
        // sehingga memakai simbol SQLite bundled yang sama dengan sqlx.
        let rc = unsafe {
            let entry: EntryPoint =
                std::mem::transmute(sqlite_vec::sqlite3_vec_init as unsafe extern "C" fn());
            libsqlite3_sys::sqlite3_auto_extension(Some(entry))
        };
        if rc != libsqlite3_sys::SQLITE_OK {
            log::warn!("sqlite-vec registration failed (rc={rc}); vector search disabled");
        }
    });
}

/// FNV-1a 64-bit. Dipakai karena hasilnya stabil lintas versi Rust/platform
/// (vektor disimpan permanen di disk), berbeda dengan `DefaultHasher`.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Kata umum SQL / bahasa natural yang tidak membantu membedakan tabel.
/// Kata yang lazim jadi nama tabel (order, group, data, count, ...) sengaja
/// tidak dimasukkan.
const STOPWORDS: &[&str] = &[
    "select",
    "from",
    "where",
    "and",
    "or",
    "not",
    "the",
    "a",
    "an",
    "of",
    "to",
    "in",
    "on",
    "by",
    "as",
    "is",
    "for",
    "with",
    "all",
    "me",
    "show",
    "get",
    "find",
    "give",
    "what",
    "which",
    "how",
    "many",
    "query",
    "table",
    "tables",
    "yang",
    "dan",
    "di",
    "ke",
    "dari",
    "untuk",
    "semua",
    "tampilkan",
    "berapa",
    "join",
    "inner",
    "outer",
    "into",
];

/// Pecah teks jadi token identifier: pisah non-alfanumerik, snake_case, dan
/// camelCase; lowercase; buang stopword; singularkan bentuk jamak sederhana.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        if word.is_empty() {
            continue;
        }
        // Pisah camelCase: "orderItems" -> "order", "Items".
        let mut current = String::new();
        let mut prev_lower = false;
        for ch in word.chars() {
            if ch.is_uppercase() && prev_lower && !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            prev_lower = ch.is_lowercase() || ch.is_numeric();
            current.push(ch);
        }
        if !current.is_empty() {
            tokens.push(current);
        }
    }

    tokens
        .into_iter()
        .map(|t| t.to_lowercase())
        .filter(|t| !STOPWORDS.contains(&t.as_str()))
        .map(|t| singularize(&t))
        .collect()
}

fn singularize(token: &str) -> String {
    let n = token.chars().count();
    if n > 4 && token.ends_with("ies") {
        format!("{}y", &token[..token.len() - 3])
    } else if n > 4
        && ["sses", "xes", "ches", "shes"]
            .iter()
            .any(|s| token.ends_with(s))
    {
        token[..token.len() - 2].to_string()
    } else if n > 3
        && token.ends_with('s')
        && !["ss", "us", "is"].iter().any(|s| token.ends_with(s))
    {
        token[..token.len() - 1].to_string()
    } else {
        token.to_string()
    }
}

fn add_feature(vec: &mut [f32], feature: &str, weight: f32) {
    let h = fnv1a(feature.as_bytes());
    let idx = (h % EMBEDDING_DIM as u64) as usize;
    let sign = if h >> 63 == 0 { 1.0 } else { -1.0 };
    vec[idx] += sign * weight;
}

/// Hitung embedding ternormalisasi (L2) untuk teks. `None` jika teks tidak
/// punya token bermakna (vektor nol tidak bisa dibandingkan secara cosine).
pub fn embed_text(text: &str) -> Option<Vec<f32>> {
    let mut vec = vec![0.0f32; EMBEDDING_DIM];
    for token in tokenize(text) {
        add_feature(&mut vec, &format!("w:{token}"), 1.0);
        // Trigram karakter menangkap kemiripan parsial (cust ~ customer).
        let padded: Vec<char> = format!("#{token}#").chars().collect();
        for tri in padded.windows(3) {
            let tri: String = tri.iter().collect();
            add_feature(&mut vec, &format!("t:{tri}"), 0.5);
        }
    }

    let norm = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm == 0.0 {
        return None;
    }
    vec.iter_mut().for_each(|v| *v /= norm);
    Some(vec)
}

/// Serialisasi vektor ke format BLOB float32 little-endian milik sqlite-vec.
fn to_blob(vec: &[f32]) -> Vec<u8> {
    vec.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn content_hash(text: &str) -> i64 {
    let mut bytes = EMBEDDER_VERSION.to_le_bytes().to_vec();
    bytes.extend_from_slice(text.as_bytes());
    fnv1a(&bytes) as i64
}

/// Teks representasi tabel. Nama tabel diulang agar bobotnya lebih besar
/// daripada nama kolom.
fn table_document(table: &str, columns: &[String]) -> String {
    format!("{table} {table} {}", columns.join(" "))
}

/// Buat tabel penyimpanan embedding jika belum ada.
pub async fn ensure_schema(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS schema_embedding (
            connection_id INTEGER NOT NULL,
            database_name TEXT NOT NULL,
            table_name TEXT NOT NULL,
            content_hash INTEGER NOT NULL,
            embedding BLOB NOT NULL,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (connection_id, database_name, table_name)
        );
        CREATE TABLE IF NOT EXISTS history_embedding (
            history_id INTEGER PRIMARY KEY,
            content_hash INTEGER NOT NULL,
            embedding BLOB NOT NULL
        );
        CREATE TABLE IF NOT EXISTS note_embedding (
            vault_path TEXT NOT NULL,
            rel_path TEXT NOT NULL,
            chunk_idx INTEGER NOT NULL,
            title TEXT NOT NULL,
            heading TEXT NOT NULL,
            text TEXT NOT NULL,
            stamp INTEGER NOT NULL,
            embedding BLOB NOT NULL,
            PRIMARY KEY (vault_path, rel_path, chunk_idx)
        );
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Sinkronkan embedding tabel untuk satu database dari `table_cache` +
/// `column_cache`. Hanya tabel yang isinya berubah yang dihitung ulang, dan
/// tabel yang sudah tidak ada di cache dihapus. Mengembalikan jumlah baris
/// yang ditulis.
pub async fn sync_schema_embeddings(
    pool: &SqlitePool,
    connection_id: i64,
    database_name: &str,
) -> Result<usize, sqlx::Error> {
    ensure_schema(pool).await?;

    let tables: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT table_name FROM table_cache WHERE connection_id = ? AND database_name = ? AND table_type = 'table'",
    )
    .bind(connection_id)
    .bind(database_name)
    .fetch_all(pool)
    .await?;

    let column_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT table_name, column_name FROM column_cache WHERE connection_id = ? AND database_name = ? COLLATE NOCASE ORDER BY table_name, ordinal_position",
    )
    .bind(connection_id)
    .bind(database_name)
    .fetch_all(pool)
    .await?;

    let mut columns: HashMap<String, Vec<String>> = HashMap::new();
    for (table, column) in column_rows {
        columns
            .entry(table.to_lowercase())
            .or_default()
            .push(column);
    }

    let existing: HashMap<String, i64> = sqlx::query_as::<_, (String, i64)>(
        "SELECT table_name, content_hash FROM schema_embedding WHERE connection_id = ? AND database_name = ?",
    )
    .bind(connection_id)
    .bind(database_name)
    .fetch_all(pool)
    .await?
    .into_iter()
    .collect();

    let mut tx = pool.begin().await?;
    let mut written = 0;
    let mut current: HashSet<String> = HashSet::new();

    for (table,) in tables {
        let cols = columns
            .get(&table.to_lowercase())
            .cloned()
            .unwrap_or_default();
        let doc = table_document(&table, &cols);
        let hash = content_hash(&doc);
        current.insert(table.clone());
        if existing.get(&table) == Some(&hash) {
            continue;
        }
        let Some(embedding) = embed_text(&doc) else {
            continue;
        };
        sqlx::query(
            "INSERT INTO schema_embedding (connection_id, database_name, table_name, content_hash, embedding, updated_at)
             VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
             ON CONFLICT (connection_id, database_name, table_name)
             DO UPDATE SET content_hash = excluded.content_hash, embedding = excluded.embedding, updated_at = CURRENT_TIMESTAMP",
        )
        .bind(connection_id)
        .bind(database_name)
        .bind(&table)
        .bind(hash)
        .bind(to_blob(&embedding))
        .execute(&mut *tx)
        .await?;
        written += 1;
    }

    for stale in existing.keys().filter(|t| !current.contains(*t)) {
        sqlx::query(
            "DELETE FROM schema_embedding WHERE connection_id = ? AND database_name = ? AND table_name = ?",
        )
        .bind(connection_id)
        .bind(database_name)
        .bind(stale)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(written)
}

/// Sinkronkan embedding untuk semua pasangan koneksi/database di `table_cache`.
pub async fn sync_all_schema_embeddings(pool: &SqlitePool) -> Result<usize, sqlx::Error> {
    let databases: Vec<(i64, String)> = sqlx::query_as(
        "SELECT DISTINCT connection_id, database_name FROM table_cache WHERE table_type = 'table'",
    )
    .fetch_all(pool)
    .await?;
    let mut written = 0;
    for (connection_id, database_name) in databases {
        written += sync_schema_embeddings(pool, connection_id, &database_name).await?;
    }
    Ok(written)
}

/// Cari tabel yang mirip dengan `query` di semua koneksi (nama tabel + kolom).
/// Mengembalikan `(connection_id, database_name, table_name, distance)` dengan
/// jarak <= `max_distance`.
pub async fn search_tables(
    pool: &SqlitePool,
    query: &str,
    limit: usize,
    max_distance: f32,
) -> Result<Vec<(i64, String, String, f32)>, sqlx::Error> {
    let Some(embedding) = embed_text(query) else {
        return Ok(Vec::new());
    };
    ensure_schema(pool).await?;
    let rows: Vec<(i64, String, String, f64)> = sqlx::query_as(
        "SELECT connection_id, database_name, table_name, distance FROM (
             SELECT connection_id, database_name, table_name, vec_distance_cosine(embedding, ?) AS distance
             FROM schema_embedding
         )
         WHERE distance <= ?
         ORDER BY distance ASC, table_name ASC
         LIMIT ?",
    )
    .bind(to_blob(&embedding))
    .bind(f64::from(max_distance))
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(c, db, t, d)| (c, db, t, d as f32))
        .collect())
}

/// Urutkan tabel berdasarkan kemiripan dengan `query` (paling relevan dulu).
/// Mengembalikan `(table_name, cosine_distance)`; kosong jika query tidak
/// punya token bermakna.
pub async fn rank_tables(
    pool: &SqlitePool,
    connection_id: i64,
    database_name: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<(String, f32)>, sqlx::Error> {
    let Some(embedding) = embed_text(query) else {
        return Ok(Vec::new());
    };
    ensure_schema(pool).await?;
    let rows: Vec<(String, f64)> = sqlx::query_as(
        "SELECT table_name, vec_distance_cosine(embedding, ?) AS distance
         FROM schema_embedding
         WHERE connection_id = ? AND database_name = ?
         ORDER BY distance ASC, table_name ASC
         LIMIT ?",
    )
    .bind(to_blob(&embedding))
    .bind(connection_id)
    .bind(database_name)
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(t, d)| (t, d as f32)).collect())
}

/// Sinkronkan embedding untuk seluruh `query_history` dan hapus embedding
/// yang history-nya sudah terhapus. Mengembalikan jumlah baris yang ditulis.
pub async fn sync_history_embeddings(pool: &SqlitePool) -> Result<usize, sqlx::Error> {
    ensure_schema(pool).await?;

    let history: Vec<(i64, String)> = sqlx::query_as("SELECT id, query_text FROM query_history")
        .fetch_all(pool)
        .await?;
    let existing: HashMap<i64, i64> =
        sqlx::query_as::<_, (i64, i64)>("SELECT history_id, content_hash FROM history_embedding")
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();

    let mut tx = pool.begin().await?;
    let mut written = 0;
    for (id, text) in history {
        let hash = content_hash(&text);
        if existing.get(&id) == Some(&hash) {
            continue;
        }
        let Some(embedding) = embed_text(&text) else {
            continue;
        };
        sqlx::query(
            "INSERT INTO history_embedding (history_id, content_hash, embedding) VALUES (?, ?, ?)
             ON CONFLICT (history_id) DO UPDATE SET content_hash = excluded.content_hash, embedding = excluded.embedding",
        )
        .bind(id)
        .bind(hash)
        .bind(to_blob(&embedding))
        .execute(&mut *tx)
        .await?;
        written += 1;
    }
    sqlx::query(
        "DELETE FROM history_embedding WHERE history_id NOT IN (SELECT id FROM query_history)",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(written)
}

/// Cari history yang isinya mirip dengan `query`. Mengembalikan
/// `(query_text, cosine_distance)` dengan jarak <= `max_distance`.
pub async fn search_history(
    pool: &SqlitePool,
    query: &str,
    limit: usize,
    max_distance: f32,
) -> Result<Vec<(String, f32)>, sqlx::Error> {
    let Some(embedding) = embed_text(query) else {
        return Ok(Vec::new());
    };
    ensure_schema(pool).await?;
    let rows: Vec<(String, f64)> = sqlx::query_as(
        "SELECT query_text, distance FROM (
             SELECT h.query_text, h.executed_at, vec_distance_cosine(e.embedding, ?) AS distance
             FROM history_embedding e
             JOIN query_history h ON h.id = e.history_id
         )
         WHERE distance <= ?
         ORDER BY distance ASC, executed_at DESC
         LIMIT ?",
    )
    .bind(to_blob(&embedding))
    .bind(f64::from(max_distance))
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(t, d)| (t, d as f32)).collect())
}

/// Hasil satu kali sinkronisasi indeks vault.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct NoteSyncStats {
    /// Jumlah catatan `.md` di vault.
    pub notes: usize,
    /// Jumlah potongan yang terindeks setelah sinkronisasi.
    pub chunks: usize,
    /// Catatan yang dibaca ulang karena baru atau berubah.
    pub updated: usize,
}

/// Potongan catatan hasil pencarian.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct NoteHit {
    pub rel_path: String,
    pub title: String,
    pub heading: String,
    pub text: String,
    pub distance: f32,
}

/// Teks representasi potongan catatan. Judul, alias, tag dan heading diulang
/// agar bobotnya lebih besar daripada isi.
fn note_document(note: &crate::obsidian::ParsedNote, chunk: &crate::obsidian::NoteChunk) -> String {
    format!(
        "{title} {title} {aliases} {tags} {heading} {heading} {text}",
        title = note.title,
        aliases = note.aliases.join(" "),
        tags = note.tags.join(" "),
        heading = chunk.heading,
        text = chunk.text
    )
}

/// Sinkronkan indeks dengan isi vault di `root`. Hanya catatan yang mtime /
/// ukurannya berubah yang dibaca ulang; catatan yang hilang dan baris milik
/// vault lain dihapus. Melakukan I/O file sinkron, jadi panggil dari thread
/// latar.
pub async fn sync_note_embeddings(
    pool: &SqlitePool,
    root: &std::path::Path,
) -> Result<NoteSyncStats, String> {
    let files = crate::obsidian::scan_vault(root)?;
    let vault = root.to_string_lossy().to_string();
    let db = |e: sqlx::Error| format!("note index error: {e}");

    ensure_schema(pool).await.map_err(db)?;
    // Hanya satu vault yang aktif; indeks vault sebelumnya tidak dipakai lagi.
    sqlx::query("DELETE FROM note_embedding WHERE vault_path <> ?")
        .bind(&vault)
        .execute(pool)
        .await
        .map_err(db)?;

    let existing: HashMap<String, i64> = sqlx::query_as::<_, (String, i64)>(
        "SELECT DISTINCT rel_path, stamp FROM note_embedding WHERE vault_path = ?",
    )
    .bind(&vault)
    .fetch_all(pool)
    .await
    .map_err(db)?
    .into_iter()
    .collect();

    let mut tx = pool.begin().await.map_err(db)?;
    let mut updated = 0;
    let mut current: HashSet<&str> = HashSet::new();

    for file in &files {
        current.insert(file.rel_path.as_str());
        let stamp = content_hash(&format!("{}:{}", file.mtime, file.size));
        if existing.get(&file.rel_path) == Some(&stamp) {
            continue;
        }
        let raw = match crate::obsidian::read_note(root, &file.rel_path) {
            Ok(raw) => raw,
            Err(e) => {
                log::warn!("[OBSIDIAN] skipped: {e}");
                continue;
            }
        };
        let note = crate::obsidian::parse_note(&file.rel_path, &raw);
        sqlx::query("DELETE FROM note_embedding WHERE vault_path = ? AND rel_path = ?")
            .bind(&vault)
            .bind(&file.rel_path)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        for (idx, chunk) in note.chunks.iter().enumerate() {
            let Some(embedding) = embed_text(&note_document(&note, chunk)) else {
                continue;
            };
            sqlx::query(
                "INSERT INTO note_embedding (vault_path, rel_path, chunk_idx, title, heading, text, stamp, embedding)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&vault)
            .bind(&file.rel_path)
            .bind(idx as i64)
            .bind(&note.title)
            .bind(&chunk.heading)
            .bind(&chunk.text)
            .bind(stamp)
            .bind(to_blob(&embedding))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }
        updated += 1;
    }

    for stale in existing.keys().filter(|p| !current.contains(p.as_str())) {
        sqlx::query("DELETE FROM note_embedding WHERE vault_path = ? AND rel_path = ?")
            .bind(&vault)
            .bind(stale)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
    }
    tx.commit().await.map_err(db)?;

    let (_, chunks) = count_notes(pool, root).await.map_err(db)?;
    Ok(NoteSyncStats {
        notes: files.len(),
        chunks,
        updated,
    })
}

/// `(jumlah catatan, jumlah potongan)` yang terindeks untuk vault `root`.
pub async fn count_notes(
    pool: &SqlitePool,
    root: &std::path::Path,
) -> Result<(usize, usize), sqlx::Error> {
    ensure_schema(pool).await?;
    let (notes, chunks): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(DISTINCT rel_path), COUNT(*) FROM note_embedding WHERE vault_path = ?",
    )
    .bind(root.to_string_lossy().to_string())
    .fetch_one(pool)
    .await?;
    Ok((notes as usize, chunks as usize))
}

/// Cari potongan catatan yang relevan dengan `query`. Kandidat diambil lewat
/// jarak cosine (<= `max_distance`), lalu diurutkan ulang: tiap kata kunci
/// query yang benar-benar muncul di potongan mengurangi jaraknya sedikit,
/// karena feature hashing saja cukup berisik untuk prosa.
pub async fn search_notes(
    pool: &SqlitePool,
    root: &std::path::Path,
    query: &str,
    limit: usize,
    max_distance: f32,
) -> Result<Vec<NoteHit>, sqlx::Error> {
    let Some(embedding) = embed_text(query) else {
        return Ok(Vec::new());
    };
    ensure_schema(pool).await?;
    let rows: Vec<(String, String, String, String, f64)> = sqlx::query_as(
        "SELECT rel_path, title, heading, text, distance FROM (
             SELECT rel_path, title, heading, text, chunk_idx, vec_distance_cosine(embedding, ?) AS distance
             FROM note_embedding
             WHERE vault_path = ?
         )
         WHERE distance <= ?
         ORDER BY distance ASC, rel_path ASC, chunk_idx ASC
         LIMIT ?",
    )
    .bind(to_blob(&embedding))
    .bind(root.to_string_lossy().to_string())
    .bind(f64::from(max_distance))
    .bind((limit.max(1) * 4) as i64)
    .fetch_all(pool)
    .await?;

    let query_tokens: HashSet<String> = tokenize(query).into_iter().collect();
    let mut hits: Vec<(f32, NoteHit)> = rows
        .into_iter()
        .map(|(rel_path, title, heading, text, distance)| {
            let tokens: HashSet<String> = tokenize(&format!("{title} {heading} {text}"))
                .into_iter()
                .collect();
            let overlap = query_tokens.intersection(&tokens).count().min(5);
            let distance = distance as f32;
            let hit = NoteHit {
                rel_path,
                title,
                heading,
                text,
                distance,
            };
            (distance - 0.04 * overlap as f32, hit)
        })
        .collect();
    hits.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(hits.into_iter().take(limit).map(|(_, hit)| hit).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> SqlitePool {
        register_sqlite_vec();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("pool in-memory");
        sqlx::query(
            r#"
            CREATE TABLE table_cache (id INTEGER PRIMARY KEY AUTOINCREMENT, connection_id INTEGER NOT NULL, database_name TEXT NOT NULL, table_name TEXT NOT NULL, table_type TEXT NOT NULL);
            CREATE TABLE column_cache (id INTEGER PRIMARY KEY AUTOINCREMENT, connection_id INTEGER NOT NULL, database_name TEXT NOT NULL, table_name TEXT NOT NULL, column_name TEXT NOT NULL, data_type TEXT NOT NULL, ordinal_position INTEGER NOT NULL);
            CREATE TABLE query_history (id INTEGER PRIMARY KEY AUTOINCREMENT, query_text TEXT NOT NULL, connection_id INTEGER NOT NULL, connection_name TEXT NOT NULL, executed_at DATETIME DEFAULT CURRENT_TIMESTAMP);
            "#,
        )
        .execute(&pool)
        .await
        .expect("schema tes");
        pool
    }

    async fn add_table(pool: &SqlitePool, table: &str, columns: &[&str]) {
        sqlx::query("INSERT INTO table_cache (connection_id, database_name, table_name, table_type) VALUES (1, 'shop', ?, 'table')")
            .bind(table)
            .execute(pool)
            .await
            .unwrap();
        for (i, col) in columns.iter().enumerate() {
            sqlx::query("INSERT INTO column_cache (connection_id, database_name, table_name, column_name, data_type, ordinal_position) VALUES (1, 'shop', ?, ?, 'text', ?)")
                .bind(table)
                .bind(col)
                .bind(i as i64)
                .execute(pool)
                .await
                .unwrap();
        }
    }

    #[test]
    fn tokenize_splits_identifiers_and_drops_stopwords() {
        assert_eq!(
            tokenize("SELECT orderItems FROM customer_addresses"),
            vec!["order", "item", "customer", "address"]
        );
        assert_eq!(
            tokenize("categories boxes status"),
            vec!["category", "box", "status"]
        );
    }

    #[test]
    fn embed_text_is_normalized_and_deterministic() {
        let a = embed_text("customer email").unwrap();
        let b = embed_text("customer email").unwrap();
        assert_eq!(a, b);
        let norm: f32 = a.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4);
        assert!(embed_text("select from where").is_none());
        assert!(embed_text("  ,;  ").is_none());
    }

    #[tokio::test]
    async fn sqlite_vec_is_registered() {
        let pool = test_pool().await;
        let (version,): (String,) = sqlx::query_as("SELECT vec_version()")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(version.starts_with('v'));
    }

    #[tokio::test]
    async fn rank_tables_prefers_relevant_schema() {
        let pool = test_pool().await;
        add_table(&pool, "customers", &["id", "full_name", "email", "phone"]).await;
        add_table(
            &pool,
            "invoices",
            &["id", "customer_id", "total_amount", "due_date"],
        )
        .await;
        add_table(
            &pool,
            "warehouse_stock",
            &["sku", "quantity", "bin_location"],
        )
        .await;

        assert_eq!(sync_schema_embeddings(&pool, 1, "shop").await.unwrap(), 3);
        // Tanpa perubahan, sinkronisasi kedua tidak menulis apa pun.
        assert_eq!(sync_schema_embeddings(&pool, 1, "shop").await.unwrap(), 0);

        let ranked = rank_tables(&pool, 1, "shop", "show customer emails", 3)
            .await
            .unwrap();
        assert_eq!(ranked[0].0, "customers");

        let ranked = rank_tables(&pool, 1, "shop", "stock quantity per bin", 3)
            .await
            .unwrap();
        assert_eq!(ranked[0].0, "warehouse_stock");

        // Koneksi lain tidak ikut.
        assert!(
            rank_tables(&pool, 2, "shop", "customer", 3)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn search_tables_matches_columns_across_connections() {
        let pool = test_pool().await;
        add_table(&pool, "customers", &["id", "full_name", "email", "phone"]).await;
        add_table(
            &pool,
            "warehouse_stock",
            &["sku", "quantity", "bin_location"],
        )
        .await;
        assert_eq!(sync_all_schema_embeddings(&pool).await.unwrap(), 2);

        let hits = search_tables(&pool, "customer email", 10, TABLE_MAX_DISTANCE)
            .await
            .unwrap();
        let names: Vec<&str> = hits.iter().map(|h| h.2.as_str()).collect();
        assert_eq!(names, vec!["customers"]);

        // Cocok lewat nama kolom saja.
        let hits = search_tables(&pool, "bin location", 10, TABLE_MAX_DISTANCE)
            .await
            .unwrap();
        assert_eq!(hits.first().map(|h| h.2.as_str()), Some("warehouse_stock"));
    }

    #[tokio::test]
    async fn sync_schema_removes_dropped_tables() {
        let pool = test_pool().await;
        add_table(&pool, "customers", &["email"]).await;
        add_table(&pool, "legacy_logs", &["message"]).await;
        sync_schema_embeddings(&pool, 1, "shop").await.unwrap();

        sqlx::query("DELETE FROM table_cache WHERE table_name = 'legacy_logs'")
            .execute(&pool)
            .await
            .unwrap();
        sync_schema_embeddings(&pool, 1, "shop").await.unwrap();

        let ranked = rank_tables(&pool, 1, "shop", "legacy logs message", 10)
            .await
            .unwrap();
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].0, "customers");
    }

    #[tokio::test]
    async fn search_history_finds_similar_queries_and_drops_orphans() {
        let pool = test_pool().await;
        for q in [
            "SELECT * FROM invoices WHERE due_date < now()",
            "SELECT email FROM customers WHERE email LIKE '%@gmail.com'",
            "UPDATE warehouse_stock SET quantity = 0 WHERE sku = 'X1'",
        ] {
            sqlx::query("INSERT INTO query_history (query_text, connection_id, connection_name) VALUES (?, 1, 'c')")
                .bind(q)
                .execute(&pool)
                .await
                .unwrap();
        }
        assert_eq!(sync_history_embeddings(&pool).await.unwrap(), 3);

        let hits = search_history(&pool, "overdue invoice due date", 5, HISTORY_MAX_DISTANCE)
            .await
            .unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].0.contains("invoices"));

        sqlx::query("DELETE FROM query_history")
            .execute(&pool)
            .await
            .unwrap();
        sync_history_embeddings(&pool).await.unwrap();
        let hits = search_history(&pool, "invoice", 5, 2.0).await.unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn note_index_syncs_incrementally_and_finds_relevant_chunks() {
        use crate::obsidian::tests::TempVault;

        let pool = test_pool().await;
        let vault = TempVault::new("index");
        vault.write(
            "db/Transactions.md",
            "---\naliases: [trx_h]\ntags: [sales]\n---\n# Status codes\nIn trx_h, status 3 means the transaction was voided.\n\n# Owner\nMaintained by the finance team.",
        );
        vault.write(
            "Recipes/Rendang.md",
            "Slow cooked beef with coconut milk and chili paste.",
        );

        let stats = sync_note_embeddings(&pool, &vault.0).await.unwrap();
        assert_eq!((stats.notes, stats.chunks, stats.updated), (2, 3, 2));
        // Tidak ada yang berubah: tidak ada catatan yang dibaca ulang.
        let stats = sync_note_embeddings(&pool, &vault.0).await.unwrap();
        assert_eq!((stats.notes, stats.chunks, stats.updated), (2, 3, 0));

        let hits = search_notes(
            &pool,
            &vault.0,
            "total voided transactions this month",
            3,
            NOTE_MAX_DISTANCE,
        )
        .await
        .unwrap();
        assert_eq!(
            hits.first().map(|h| h.rel_path.as_str()),
            Some("db/Transactions.md")
        );
        assert_eq!(hits[0].heading, "Status codes");
        assert!(hits.iter().all(|h| h.rel_path != "Recipes/Rendang.md"));
        // Alias di frontmatter ikut terindeks.
        let hits = search_notes(&pool, &vault.0, "trx_h", 1, NOTE_MAX_DISTANCE)
            .await
            .unwrap();
        assert_eq!(hits[0].rel_path, "db/Transactions.md");

        // Isi berubah (ukuran beda) -> diindeks ulang; file terhapus -> hilang dari indeks.
        vault.write(
            "db/Transactions.md",
            "# Status codes\nStatus 9 means refunded to the customer wallet.",
        );
        std::fs::remove_file(vault.0.join("Recipes/Rendang.md")).unwrap();
        let stats = sync_note_embeddings(&pool, &vault.0).await.unwrap();
        assert_eq!((stats.notes, stats.chunks, stats.updated), (1, 1, 1));
        let hits = search_notes(&pool, &vault.0, "refunded wallet", 3, NOTE_MAX_DISTANCE)
            .await
            .unwrap();
        assert!(hits[0].text.contains("Status 9"));
        assert_eq!(count_notes(&pool, &vault.0).await.unwrap(), (1, 1));

        // Pindah vault: indeks vault lama dibuang.
        let other = TempVault::new("index-other");
        other.write("a.md", "alpha note");
        sync_note_embeddings(&pool, &other.0).await.unwrap();
        assert_eq!(count_notes(&pool, &vault.0).await.unwrap(), (0, 0));
        assert!(
            sync_note_embeddings(&pool, &vault.0.join("missing"))
                .await
                .is_err()
        );
    }
}
