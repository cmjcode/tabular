//! Vault Obsidian sebagai memory AI assistant.
//!
//! Modul ini hanya mengurus sisi file: memindai folder vault, mem-parsing
//! markdown format Obsidian (frontmatter, `[[wikilink]]`, `![[embed]]`, `#tag`,
//! callout, `%%comment%%`) menjadi potongan teks per heading, membaca satu
//! catatan dengan aman, dan menulis catatan memory baru. Pengindeksan dan
//! pencarian ada di [`crate::vector_index`].
//!
//! Vault tetap milik user: Tabular hanya membaca, kecuali [`save_memory_note`]
//! yang menulis file baru di subfolder [`MEMORY_FOLDER`] dan tidak pernah
//! menimpa file yang sudah ada.

use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use once_cell::sync::Lazy;
use regex::Regex;

/// Subfolder vault tempat AI boleh menulis catatan memory.
pub const MEMORY_FOLDER: &str = "Tabular Memory";

/// File yang lebih besar dari ini dilewati (biasanya ekspor/log, bukan catatan).
pub const MAX_NOTE_BYTES: u64 = 512 * 1024;

/// Batas jumlah catatan yang dipindai dari satu vault.
pub const MAX_VAULT_FILES: usize = 5_000;

/// Ukuran maksimum satu potongan teks yang di-embed.
pub const MAX_CHUNK_BYTES: usize = 1_500;

/// Batas potongan per catatan, supaya satu file raksasa tidak mendominasi indeks.
const MAX_CHUNKS_PER_NOTE: usize = 200;

/// Kedalaman rekursi folder maksimum saat memindai.
const MAX_SCAN_DEPTH: usize = 12;

/// Satu file catatan hasil pemindaian vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultFile {
    /// Path relatif terhadap root vault, selalu dengan pemisah `/`.
    pub rel_path: String,
    /// Detik sejak epoch; 0 bila tidak tersedia.
    pub mtime: i64,
    pub size: i64,
}

/// Potongan catatan: teks di bawah satu heading (atau bagiannya).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteChunk {
    /// Jalur heading, mis. `Orders > Status codes`; kosong untuk teks sebelum
    /// heading pertama.
    pub heading: String,
    pub text: String,
}

/// Catatan Obsidian yang sudah di-parse.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedNote {
    pub title: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    /// Target `[[wikilink]]` (nama catatan, tanpa `#heading` dan alias).
    pub links: Vec<String>,
    pub chunks: Vec<NoteChunk>,
}

static WIKILINK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"!?\[\[([^\]\|#]*)(#[^\]\|]*)?(?:\|([^\]]*))?\]\]").expect("wikilink regex")
});
static TAG_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:^|\s)#([A-Za-z_][A-Za-z0-9_/\-]*)").expect("tag regex"));
static CALLOUT_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(\s*>\s*)\[!([A-Za-z]+)\][+-]?\s*").expect("callout regex"));

/// Pindai vault: semua file `.md` di bawah `root`, tanpa folder tersembunyi
/// (`.obsidian`, `.trash`, `.git`), tanpa mengikuti symlink, terurut menurut
/// path. Error I/O per folder dicatat dan dilewati.
pub fn scan_vault(root: &Path) -> Result<Vec<VaultFile>, String> {
    if !root.is_dir() {
        return Err(format!("vault folder not found: {}", root.display()));
    }
    let mut files = Vec::new();
    scan_dir(root, root, 0, &mut files);
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(files)
}

fn scan_dir(root: &Path, dir: &Path, depth: usize, out: &mut Vec<VaultFile>) {
    if depth > MAX_SCAN_DEPTH || out.len() >= MAX_VAULT_FILES {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("[OBSIDIAN] cannot read {}: {e}", dir.display());
            return;
        }
    };
    for entry in entries.flatten() {
        if out.len() >= MAX_VAULT_FILES {
            log::warn!("[OBSIDIAN] vault has more than {MAX_VAULT_FILES} notes; the rest is skipped");
            return;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        // `file_type()` tidak mengikuti symlink, jadi symlink otomatis dilewati.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            scan_dir(root, &path, depth + 1, out);
        } else if file_type.is_file()
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.len() > MAX_NOTE_BYTES {
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            out.push(VaultFile {
                rel_path: rel_to_string(rel),
                mtime,
                size: meta.len() as i64,
            });
        }
    }
}

fn rel_to_string(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// Pisahkan frontmatter YAML (di antara dua baris `---`) dari isi catatan.
fn split_frontmatter(raw: &str) -> (&str, &str) {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let Some(rest) = raw.strip_prefix("---") else {
        return ("", raw);
    };
    let Some(rest) = rest.strip_prefix('\n').or_else(|| rest.strip_prefix("\r\n")) else {
        return ("", raw);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            return (&rest[..offset], &rest[offset + line.len()..]);
        }
        offset += line.len();
    }
    ("", raw)
}

fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim()
        .to_string()
}

/// Nilai YAML sederhana jadi daftar: `[a, b]`, `a, b`, atau skalar tunggal.
fn yaml_inline_list(value: &str) -> Vec<String> {
    let value = value.trim();
    let inner = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or(value);
    inner
        .split(',')
        .map(unquote)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Ambil `title`, `aliases` dan `tags` dari frontmatter. Hanya subset YAML
/// yang lazim dipakai Obsidian (skalar, list inline, list `- item`); kunci
/// lain diabaikan.
fn parse_frontmatter(front: &str, note: &mut ParsedNote) {
    let mut current_key = String::new();
    for line in front.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(item) = trimmed.strip_prefix("- ") {
            let item = unquote(item);
            let item = item.trim_start_matches('#').to_string();
            if item.is_empty() {
                continue;
            }
            match current_key.as_str() {
                "tags" | "tag" => note.tags.push(item),
                "aliases" | "alias" => note.aliases.push(item),
                _ => {}
            }
            continue;
        }
        // Baris ber-indent tanpa `- ` adalah lanjutan nilai kunci lain.
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        current_key = key.trim().to_lowercase();
        match current_key.as_str() {
            "title" => {
                let title = unquote(value);
                if !title.is_empty() {
                    note.title = title;
                }
            }
            "tags" | "tag" => note.tags.extend(
                yaml_inline_list(value)
                    .into_iter()
                    .map(|t| t.trim_start_matches('#').to_string()),
            ),
            "aliases" | "alias" => note.aliases.extend(yaml_inline_list(value)),
            _ => {}
        }
    }
}

/// Buang komentar Obsidian `%% ... %%` (inline maupun multi-baris).
fn strip_comments(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("%%") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("%%") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Ubah sintaks Obsidian di satu baris jadi teks biasa, sambil mencatat
/// target wikilink dan tag.
fn clean_line(line: &str, note: &mut ParsedNote) -> String {
    for cap in TAG_RE.captures_iter(line) {
        note.tags.push(cap[1].to_string());
    }
    let line = CALLOUT_RE.replace(line, |caps: &regex::Captures| {
        format!("{}{}: ", &caps[1], caps[2].to_uppercase())
    });
    WIKILINK_RE
        .replace_all(&line, |caps: &regex::Captures| {
            let target = caps.get(1).map_or("", |m| m.as_str()).trim();
            let heading = caps
                .get(2)
                .map_or("", |m| m.as_str())
                .trim_start_matches('#')
                .trim();
            if !target.is_empty() {
                note.links.push(target.to_string());
            }
            match caps.get(3).map(|m| m.as_str().trim()) {
                Some(alias) if !alias.is_empty() => alias.to_string(),
                _ if target.is_empty() => heading.to_string(),
                _ if heading.is_empty() => target.to_string(),
                _ => format!("{target} > {heading}"),
            }
        })
        .into_owned()
}

/// `## Judul` -> `(2, "Judul")`.
fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &line[level..];
    if !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    Some((level, rest.trim().trim_end_matches('#').trim()))
}

/// Posisi potong di batas karakter terdekat yang <= `max` byte.
fn floor_char_boundary(text: &str, max: usize) -> usize {
    if text.len() <= max {
        return text.len();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

fn flush_chunk(heading: &str, current: &mut String, chunks: &mut Vec<NoteChunk>) {
    let body = current.trim();
    if !body.is_empty() && chunks.len() < MAX_CHUNKS_PER_NOTE {
        chunks.push(NoteChunk {
            heading: heading.to_string(),
            text: body.to_string(),
        });
    }
    current.clear();
}

/// Pecah satu bagian teks jadi potongan <= [`MAX_CHUNK_BYTES`], sebisa mungkin
/// di batas paragraf.
fn push_section(heading: &str, text: &str, chunks: &mut Vec<NoteChunk>) {
    let mut current = String::new();
    for paragraph in text.split("\n\n") {
        let mut paragraph = paragraph.trim_matches('\n');
        if paragraph.trim().is_empty() {
            continue;
        }
        if !current.is_empty() && current.len() + paragraph.len() + 2 > MAX_CHUNK_BYTES {
            flush_chunk(heading, &mut current, chunks);
        }
        // Paragraf tunggal yang terlalu panjang dipotong paksa.
        while paragraph.len() > MAX_CHUNK_BYTES {
            let cut = floor_char_boundary(paragraph, MAX_CHUNK_BYTES);
            current.push_str(&paragraph[..cut]);
            flush_chunk(heading, &mut current, chunks);
            paragraph = &paragraph[cut..];
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(paragraph);
    }
    flush_chunk(heading, &mut current, chunks);
}

/// Parse satu catatan. `rel_path` dipakai untuk judul bawaan (nama file).
pub fn parse_note(rel_path: &str, raw: &str) -> ParsedNote {
    let mut note = ParsedNote {
        title: Path::new(rel_path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| rel_path.to_string()),
        ..Default::default()
    };

    let (front, body) = split_frontmatter(raw);
    parse_frontmatter(front, &mut note);
    let body = strip_comments(body);

    let mut chunks = Vec::new();
    let mut heading_stack: Vec<(usize, String)> = Vec::new();
    let mut heading_path = String::new();
    let mut section = String::new();
    let mut in_code = false;

    for line in body.lines() {
        let fence = line.trim_start();
        if fence.starts_with("```") || fence.starts_with("~~~") {
            in_code = !in_code;
            section.push_str(line);
            section.push('\n');
            continue;
        }
        // Isi blok kode (SQL, dsb.) dibiarkan apa adanya.
        if in_code {
            section.push_str(line);
            section.push('\n');
            continue;
        }
        if let Some((level, title)) = parse_heading(line) {
            push_section(&heading_path, &section, &mut chunks);
            section.clear();
            heading_stack.retain(|(l, _)| *l < level);
            heading_stack.push((level, clean_line(title, &mut note)));
            heading_path = heading_stack
                .iter()
                .map(|(_, t)| t.as_str())
                .collect::<Vec<_>>()
                .join(" > ");
            continue;
        }
        section.push_str(&clean_line(line, &mut note));
        section.push('\n');
    }
    push_section(&heading_path, &section, &mut chunks);

    note.chunks = chunks;
    dedup_keep_order(&mut note.tags);
    dedup_keep_order(&mut note.aliases);
    dedup_keep_order(&mut note.links);
    note
}

fn dedup_keep_order(items: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(item.to_lowercase()));
}

/// Ubah path relatif dari agent/user jadi path absolut di dalam vault.
/// Menolak path absolut, `..`, komponen tersembunyi, dan (lewat canonicalize)
/// symlink yang keluar dari vault.
pub fn resolve_in_vault(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim().trim_start_matches("./");
    if rel.is_empty() {
        return Err("empty note path".to_string());
    }
    let rel_path = Path::new(rel);
    for component in rel_path.components() {
        match component {
            Component::Normal(name) if !name.to_string_lossy().starts_with('.') => {}
            _ => return Err(format!("path `{rel}` is not allowed; use a path relative to the vault")),
        }
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("vault folder not accessible: {e}"))?;
    let full = root.join(rel_path);
    if full.symlink_metadata().is_ok() {
        let canonical = full
            .canonicalize()
            .map_err(|e| format!("cannot resolve `{rel}`: {e}"))?;
        if !canonical.starts_with(&root) {
            return Err(format!("path `{rel}` points outside the vault"));
        }
        return Ok(canonical);
    }
    Ok(full)
}

/// Cari catatan berdasarkan path relatif (dengan/tanpa `.md`) atau, seperti
/// wikilink Obsidian, berdasarkan nama file saja di folder mana pun.
/// Mengembalikan path relatif catatan.
pub fn find_note(root: &Path, name_or_path: &str) -> Result<String, String> {
    let wanted = name_or_path
        .trim()
        .trim_start_matches("[[")
        .trim_end_matches("]]");
    let wanted = wanted.split(['#', '|']).next().unwrap_or("").trim();
    if wanted.is_empty() {
        return Err("empty note name".to_string());
    }
    let with_ext = if wanted.to_lowercase().ends_with(".md") {
        wanted.replace('\\', "/")
    } else {
        format!("{}.md", wanted.replace('\\', "/"))
    };
    if let Ok(path) = resolve_in_vault(root, &with_ext)
        && path.is_file()
    {
        return Ok(with_ext);
    }

    let file_name = with_ext.rsplit('/').next().unwrap_or("").to_lowercase();
    scan_vault(root)?
        .into_iter()
        .find(|f| {
            f.rel_path
                .rsplit('/')
                .next()
                .is_some_and(|name| name.to_lowercase() == file_name)
        })
        .map(|f| f.rel_path)
        .ok_or_else(|| format!("note `{wanted}` not found in the vault; use search_notes to find it"))
}

/// Baca isi mentah satu catatan (dibatasi [`MAX_NOTE_BYTES`]).
pub fn read_note(root: &Path, rel_path: &str) -> Result<String, String> {
    let path = resolve_in_vault(root, rel_path)?;
    let meta = std::fs::metadata(&path).map_err(|e| format!("cannot read `{rel_path}`: {e}"))?;
    if !meta.is_file() {
        return Err(format!("`{rel_path}` is not a file"));
    }
    if meta.len() > MAX_NOTE_BYTES {
        return Err(format!("`{rel_path}` is too large ({} bytes)", meta.len()));
    }
    std::fs::read_to_string(&path).map_err(|e| format!("cannot read `{rel_path}`: {e}"))
}

/// Nama file aman dari judul: buang karakter yang dilarang Obsidian / OS.
fn slugify_title(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '^' | '[' | ']' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim_matches('.').trim();
    let cut = floor_char_boundary(cleaned, 80);
    cleaned[..cut].trim().to_string()
}

/// Simpan catatan memory baru di `<vault>/Tabular Memory/<judul>.md` dan
/// kembalikan path relatifnya. Tidak pernah menimpa: bila nama sudah dipakai,
/// akhiran ` 2`, ` 3`, ... ditambahkan.
pub fn save_memory_note(
    root: &Path,
    title: &str,
    content: &str,
    tags: &[String],
) -> Result<String, String> {
    if !root.is_dir() {
        return Err(format!("vault folder not found: {}", root.display()));
    }
    let content = content.trim();
    if content.is_empty() {
        return Err("note content is empty".to_string());
    }
    if content.len() as u64 > MAX_NOTE_BYTES {
        return Err("note content is too large".to_string());
    }
    let mut name = slugify_title(title);
    if name.is_empty() {
        name = format!("Memory {}", chrono::Local::now().format("%Y-%m-%d %H%M"));
    }

    let dir = root.join(MEMORY_FOLDER);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create `{MEMORY_FOLDER}`: {e}"))?;

    let mut all_tags = vec!["tabular-memory".to_string()];
    all_tags.extend(
        tags.iter()
            .map(|t| {
                t.trim()
                    .trim_start_matches('#')
                    .replace(char::is_whitespace, "-")
            })
            .filter(|t| !t.is_empty()),
    );
    dedup_keep_order(&mut all_tags);
    let body = format!(
        "---\ncreated: {}\nsource: tabular-ai\ntags: [{}]\n---\n\n{}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M"),
        all_tags.join(", "),
        content
    );

    for attempt in 1..=99 {
        let file_name = if attempt == 1 {
            format!("{name}.md")
        } else {
            format!("{name} {attempt}.md")
        };
        // `create_new` gagal bila file sudah ada, jadi tidak ada race menimpa.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(&file_name))
        {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(body.as_bytes())
                    .map_err(|e| format!("cannot write note: {e}"))?;
                return Ok(format!("{MEMORY_FOLDER}/{file_name}"));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("cannot write note: {e}")),
        }
    }
    Err(format!("too many notes named `{name}`"))
}

/// Subfolder catatan skema hasil generate, di dalam [`MEMORY_FOLDER`].
pub const SCHEMA_FOLDER: &str = "Schemas";
/// Penanda frontmatter catatan skema; hanya file bertanda ini yang boleh ditimpa.
const SCHEMA_MARKER: &str = "source: tabular-schema";

/// Simpan atau perbarui catatan skema di
/// `<vault>/Tabular Memory/Schemas/<judul>.md` dan kembalikan path relatifnya.
///
/// Berbeda dengan [`save_memory_note`], file lama ditimpa supaya skema tetap
/// mutakhir, tetapi hanya bila file itu juga hasil generate Tabular (ada
/// [`SCHEMA_MARKER`] di frontmatter). Catatan buatan user dengan nama sama
/// tidak disentuh; akhiran ` 2`, ` 3`, ... dipakai sebagai gantinya.
pub fn save_schema_note(
    root: &Path,
    title: &str,
    body: &str,
    properties: &[(&str, &str)],
) -> Result<String, String> {
    if !root.is_dir() {
        return Err(format!("vault folder not found: {}", root.display()));
    }
    if body.len() as u64 > MAX_NOTE_BYTES {
        return Err("schema note is too large".to_string());
    }
    let name = slugify_title(title);
    if name.is_empty() {
        return Err("schema note needs a title".to_string());
    }
    let dir = root.join(MEMORY_FOLDER).join(SCHEMA_FOLDER);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create `{MEMORY_FOLDER}/{SCHEMA_FOLDER}`: {e}"))?;

    let mut front = format!("---\n{SCHEMA_MARKER}\n");
    for (key, value) in properties {
        front.push_str(&format!("{key}: \"{}\"\n", value.replace('"', "'")));
    }
    front.push_str(&format!(
        "updated: {}\ntags: [tabular-memory, tabular-schema]\n---\n\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M")
    ));
    let contents = format!("{front}{}\n", body.trim_end());

    for attempt in 1..=99 {
        let file_name = if attempt == 1 {
            format!("{name}.md")
        } else {
            format!("{name} {attempt}.md")
        };
        let path = dir.join(&file_name);
        if path.exists() {
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            if !is_schema_note(&existing) {
                continue;
            }
        }
        std::fs::write(&path, contents.as_bytes())
            .map_err(|e| format!("cannot write schema note: {e}"))?;
        return Ok(format!("{MEMORY_FOLDER}/{SCHEMA_FOLDER}/{file_name}"));
    }
    Err(format!("too many notes named `{name}`"))
}

/// Catatan diawali frontmatter yang memuat [`SCHEMA_MARKER`].
fn is_schema_note(raw: &str) -> bool {
    let Some(rest) = raw.strip_prefix("---") else {
        return false;
    };
    let front = rest.split("\n---").next().unwrap_or("");
    front.lines().any(|l| l.trim() == SCHEMA_MARKER)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Folder sementara unik; dihapus saat di-drop.
    pub(crate) struct TempVault(pub PathBuf);

    impl TempVault {
        pub(crate) fn new(label: &str) -> Self {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "tabular-vault-{label}-{}-{n}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp vault");
            Self(dir)
        }

        pub(crate) fn write(&self, rel: &str, content: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, content).expect("write note");
        }
    }

    impl Drop for TempVault {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn schema_note_overwrites_only_its_own_file() {
        let vault = TempVault::new("schema-note");
        let props = [("connection", "Shop \"prod\""), ("database", "shop")];
        let first = save_schema_note(&vault.0, "Shop - shop", "v1", &props).unwrap();
        assert_eq!(first, "Tabular Memory/Schemas/Shop - shop.md");
        let second = save_schema_note(&vault.0, "Shop - shop", "v2", &props).unwrap();
        assert_eq!(second, first);
        let text = std::fs::read_to_string(vault.0.join(&first)).unwrap();
        assert!(text.starts_with("---\nsource: tabular-schema\nconnection: \"Shop 'prod'\"\n"));
        assert!(text.trim_end().ends_with("v2"));

        // Catatan user dengan nama sama tidak pernah ditimpa.
        vault.write("Tabular Memory/Schemas/Mine.md", "# my own notes\n");
        let saved = save_schema_note(&vault.0, "Mine", "generated", &props).unwrap();
        assert_eq!(saved, "Tabular Memory/Schemas/Mine 2.md");
        assert_eq!(
            std::fs::read_to_string(vault.0.join("Tabular Memory/Schemas/Mine.md")).unwrap(),
            "# my own notes\n"
        );
    }

    #[test]
    fn parses_frontmatter_lists_and_title() {
        let raw = "---\ntitle: \"Order Rules\"\naliases: [orders, trx]\ntags:\n  - sales\n  - '#db/schema'\nother: x\n---\nBody text\n";
        let note = parse_note("db/orders.md", raw);
        assert_eq!(note.title, "Order Rules");
        assert_eq!(note.aliases, vec!["orders", "trx"]);
        assert_eq!(note.tags, vec!["sales", "db/schema"]);
        assert_eq!(note.chunks.len(), 1);
        assert_eq!(note.chunks[0].text, "Body text");
    }

    #[test]
    fn title_defaults_to_file_stem_and_unclosed_frontmatter_is_body() {
        let note = parse_note("folder/My Note.md", "---\nnot closed\ntext");
        assert_eq!(note.title, "My Note");
        assert!(note.chunks[0].text.contains("not closed"));
    }

    #[test]
    fn converts_wikilinks_embeds_tags_callouts_and_comments() {
        let raw = "See [[Customers|the customer table]] and [[Orders#Status codes]].\n\
                   ![[diagram.png]] %%hidden note%% visible #finance #2024\n\
                   > [!warning]- Careful\n> status 3 = void\n%%\nmulti\nline\n%%\nEnd";
        let note = parse_note("n.md", raw);
        let text = &note.chunks[0].text;
        assert!(text.contains("See the customer table and Orders > Status codes."));
        assert!(text.contains("diagram.png"));
        assert!(!text.contains("hidden note"));
        assert!(!text.contains("multi"));
        assert!(text.contains("> WARNING: Careful"));
        assert!(text.ends_with("End"));
        assert_eq!(note.links, vec!["Customers", "Orders", "diagram.png"]);
        // `#2024` bukan tag (harus diawali huruf).
        assert_eq!(note.tags, vec!["finance"]);
    }

    #[test]
    fn chunks_by_heading_path_and_keeps_code_blocks_verbatim() {
        let raw = "intro\n# Orders\ntop\n## Status codes\n3 = void\n```sql\n# not a heading [[x]]\nSELECT 1;\n```\n# Customers\ncust";
        let note = parse_note("n.md", raw);
        let headings: Vec<&str> = note.chunks.iter().map(|c| c.heading.as_str()).collect();
        assert_eq!(headings, vec!["", "Orders", "Orders > Status codes", "Customers"]);
        assert!(note.chunks[2].text.contains("# not a heading [[x]]"));
        assert!(note.links.is_empty());
    }

    #[test]
    fn long_sections_are_split_on_char_boundaries() {
        let paragraph = "é".repeat(MAX_CHUNK_BYTES); // 2 byte per karakter
        let raw = format!("# H\n{paragraph}\n\nshort tail");
        let note = parse_note("n.md", &raw);
        assert!(note.chunks.len() >= 2);
        assert!(note.chunks.iter().all(|c| c.text.len() <= MAX_CHUNK_BYTES));
        assert!(note.chunks.iter().all(|c| c.heading == "H"));
    }

    #[test]
    fn scan_skips_hidden_folders_and_non_markdown() {
        let vault = TempVault::new("scan");
        vault.write("a.md", "a");
        vault.write("sub/B.MD", "b");
        vault.write(".obsidian/config.md", "x");
        vault.write(".trash/old.md", "x");
        vault.write("image.png", "x");
        let files = scan_vault(&vault.0).expect("scan");
        let paths: Vec<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["a.md", "sub/B.MD"]);
        assert!(scan_vault(&vault.0.join("missing")).is_err());
    }

    #[test]
    fn resolve_rejects_traversal_and_hidden_paths() {
        let vault = TempVault::new("resolve");
        vault.write("ok.md", "fine");
        assert!(resolve_in_vault(&vault.0, "ok.md").is_ok());
        assert!(resolve_in_vault(&vault.0, "../ok.md").is_err());
        assert!(resolve_in_vault(&vault.0, "/etc/passwd").is_err());
        assert!(resolve_in_vault(&vault.0, ".obsidian/app.json").is_err());
        assert!(resolve_in_vault(&vault.0, "").is_err());
        assert_eq!(read_note(&vault.0, "ok.md").expect("read"), "fine");
        assert!(read_note(&vault.0, "nope.md").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_rejects_symlink_escaping_the_vault() {
        let vault = TempVault::new("symlink");
        let outside = TempVault::new("outside");
        outside.write("secret.md", "secret");
        std::os::unix::fs::symlink(outside.0.join("secret.md"), vault.0.join("link.md"))
            .expect("symlink");
        assert!(read_note(&vault.0, "link.md").is_err());
        // Symlink juga tidak ikut terindeks.
        assert!(scan_vault(&vault.0).expect("scan").is_empty());
    }

    #[test]
    fn find_note_accepts_path_name_and_wikilink() {
        let vault = TempVault::new("find");
        vault.write("db/Orders.md", "x");
        assert_eq!(find_note(&vault.0, "db/Orders.md").expect("path"), "db/Orders.md");
        assert_eq!(find_note(&vault.0, "db/Orders").expect("no ext"), "db/Orders.md");
        assert_eq!(find_note(&vault.0, "orders").expect("by name"), "db/Orders.md");
        assert_eq!(
            find_note(&vault.0, "[[Orders#Status|alias]]").expect("wikilink"),
            "db/Orders.md"
        );
        assert!(find_note(&vault.0, "Missing").is_err());
    }

    #[test]
    fn save_memory_note_never_overwrites() {
        let vault = TempVault::new("save");
        let first = save_memory_note(
            &vault.0,
            "Orders: void/status?",
            "status 3 = void",
            &["#db".into()],
        )
        .expect("first");
        assert_eq!(first, "Tabular Memory/Orders void status.md");
        let second =
            save_memory_note(&vault.0, "Orders: void/status?", "second", &[]).expect("second");
        assert_eq!(second, "Tabular Memory/Orders void status 2.md");

        let saved = read_note(&vault.0, &first).expect("read back");
        assert!(saved.contains("status 3 = void"));
        let parsed = parse_note(&first, &saved);
        assert_eq!(parsed.tags, vec!["tabular-memory", "db"]);

        assert!(save_memory_note(&vault.0, "t", "   ", &[]).is_err());
        assert!(save_memory_note(&vault.0.join("missing"), "t", "c", &[]).is_err());
    }
}
