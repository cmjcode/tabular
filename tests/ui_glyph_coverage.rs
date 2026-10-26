//! Penjaga regresi: glyph yang tidak ada di font egui (tampil kotak "□") tidak boleh
//! muncul di string literal UI. Pakai `egui_icons::icons::ICON_*` sebagai gantinya.
//! Daftar glyph diambil dari audit cakupan font (Proportional + Monospace).

use std::path::Path;

/// Karakter yang terbukti tidak ada di font manapun yang dimuat Tabular.
const MISSING_GLYPHS: &[char] = &[
    '\u{2715}', // ✕
    '\u{2325}', // ⌥
    '\u{2303}', // ⌃
    // Tidak ada di Proportional maupun fallback Hack (audit fontTools 2026-10).
    '\u{2713}',  // ✓
    '\u{2717}',  // ✗
    '\u{1F9E9}', // 🧩
    '\u{1F91D}', // 🤝
    '\u{1F9F9}', // 🧹
    '\u{1F6D1}', // 🛑
    '\u{1F9E0}', // 🧠
    '\u{1F5C2}', // 🗂
    '\u{1F590}', // 🖐
    '\u{1F6E2}', // 🛢
    '\u{1F573}', // 🕳
    '\u{1F9FE}', // 🧾
];

/// File yang memakai glyph itu sebagai input parser, bukan teks yang digambar.
const ALLOWED_FILES: &[&str] = &["keymap.rs"];

fn scan(dir: &Path, hits: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan(&path, hits);
        } else if path.extension().is_some_and(|e| e == "rs")
            && !path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| ALLOWED_FILES.contains(&n))
        {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (no, line) in text.lines().enumerate() {
                // Kode test memakai unicode sebagai input, bukan teks yang digambar.
                if line.contains("#[cfg(test)]") {
                    break;
                }
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") {
                    continue;
                }
                if let Some(c) = line.chars().find(|c| MISSING_GLYPHS.contains(c)) {
                    hits.push(format!("{}:{}: U+{:04X}", path.display(), no + 1, c as u32));
                }
            }
        }
    }
}

#[test]
fn ui_strings_do_not_use_missing_glyphs() {
    let mut hits = Vec::new();
    scan(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut hits,
    );
    assert!(
        hits.is_empty(),
        "Glyph tanpa font (tampil kotak) ditemukan; ganti dengan egui_icons::icons::ICON_*:\n{}",
        hits.join("\n")
    );
}
