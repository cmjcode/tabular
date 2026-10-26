//! Penjaga regresi: teks UI ditulis dalam bahasa Inggris (konvensi AGENTS.md);
//! terjemahan lewat `src/i18n/`. Test ini memindai string literal di `src/`
//! (di luar komentar, pesan log, dan modul test) untuk kata Bahasa Indonesia
//! yang lazim. Komentar dan doc-comment memang Bahasa Indonesia, jadi tidak
//! diperiksa.

use std::path::Path;

/// Kata yang hampir pasti Bahasa Indonesia (tidak ada padanan Inggris yang sama).
const WORDS: &[&str] = &[
    "belum",
    "tambah",
    "pilih",
    "hapus",
    "gagal",
    "simpan",
    "berhasil",
    "tidak",
    "sedang",
    "dengan",
    "silakan",
    "masukkan",
    "sudah",
    "harus",
    "terhapus",
    "membaca",
    "menulis",
    "semua",
    "ditemukan",
    "dibagikan",
    "pengguna",
];

/// File yang memang berisi teks non-Inggris.
const ALLOWED_FILES: &[&str] = &["tables.rs"];

/// Baris yang tidak tampil ke user: komentar, log, atribut, assert.
fn skipped_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//")
        || t.starts_with('*')
        || t.starts_with("/*")
        || t.starts_with("#[")
        || [
            "log::",
            "debug!",
            "warn!",
            "error!",
            "info!",
            "trace!",
            "eprintln!",
            "println!",
            "assert",
            "panic!",
            "unreachable!",
        ]
        .iter()
        .any(|m| line.contains(m))
}

fn string_literals(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            break;
        }
        if bytes[i] == b'"' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                if bytes[j] == b'\\' {
                    j += 1;
                }
                j += 1;
            }
            if j <= bytes.len() {
                out.push(&line[start..j.min(line.len())]);
            }
            i = j + 1;
            continue;
        }
        i += 1;
    }
    out
}

fn has_indonesian_word(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    WORDS.iter().copied().find(|w| words.contains(w))
}

fn scan(dir: &Path, hits: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan(&path, hits);
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.ends_with(".rs") || ALLOWED_FILES.contains(&name) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (no, line) in text.lines().enumerate() {
            if line.contains("#[cfg(test)]") {
                break;
            }
            if skipped_line(line) {
                continue;
            }
            for lit in string_literals(line) {
                // Pesan log di baris lanjutan diawali tag seperti "[GRID]".
                if lit.starts_with('[') || !lit.contains(' ') {
                    continue;
                }
                if let Some(word) = has_indonesian_word(lit) {
                    hits.push(format!(
                        "{}:{}: \"{word}\" in {lit:?}",
                        path.display(),
                        no + 1
                    ));
                }
            }
        }
    }
}

#[test]
fn ui_string_literals_are_english() {
    let mut hits = Vec::new();
    scan(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut hits,
    );
    assert!(
        hits.is_empty(),
        "UI strings must be English (translate via src/i18n):\n{}",
        hits.join("\n")
    );
}
