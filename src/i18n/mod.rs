//! Lokalisasi UI (M6).
//!
//! Gaya gettext: teks sumber bahasa Inggris sekaligus menjadi kunci, jadi
//! string yang belum diterjemahkan otomatis tampil apa adanya. Adopsi bisa
//! bertahap: bungkus literal UI dengan [`tr`] (atau [`trf`] untuk teks
//! berplaceholder `{}`) lalu tambahkan barisnya di `tables.rs`.
//!
//! Font bawaan egui tidak punya glyph Hangul/Han, jadi saat bahasa `ko`/`zh`
//! dipilih, [`install_fonts`] memuat font CJK sistem sebagai fallback.

mod tables;

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Bahasa yang didukung: (kode, nama asli). Indeks 0 = sumber (Inggris).
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("id", "Bahasa Indonesia"),
    ("ko", "한국어"),
    ("tr", "Türkçe"),
    ("vi", "Tiếng Việt"),
    ("zh", "简体中文"),
];

static CURRENT: AtomicUsize = AtomicUsize::new(0);

fn index_of(code: &str) -> Option<usize> {
    let code = code.trim().to_ascii_lowercase();
    // "id_ID.UTF-8", "zh-Hans-CN", "ko-KR" → "id", "zh", "ko"
    let base = code
        .split(['_', '-', '.', '@'])
        .next()
        .unwrap_or_default();
    // Kode lama "in" untuk bahasa Indonesia masih dipakai sebagian OS.
    let base = if base == "in" { "id" } else { base };
    LANGUAGES.iter().position(|(c, _)| *c == base)
}

/// Setel bahasa aktif. Kode kosong berarti ikut bahasa sistem.
pub fn set_language(code: &str) {
    let idx = if code.trim().is_empty() {
        index_of(&system_language()).unwrap_or(0)
    } else {
        index_of(code).unwrap_or(0)
    };
    CURRENT.store(idx, Ordering::Relaxed);
}

/// Kode bahasa aktif (`en`, `id`, ...).
pub fn current_language() -> &'static str {
    LANGUAGES
        .get(CURRENT.load(Ordering::Relaxed))
        .map(|(c, _)| *c)
        .unwrap_or("en")
}

/// Bahasa pilihan sistem operasi, mis. `id-ID`. Kosong bila tidak diketahui.
pub fn system_language() -> String {
    #[cfg(target_os = "macos")]
    if let Some(lang) = crate::platform_macos::preferred_language() {
        return lang;
    }
    ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
        .unwrap_or_default()
}

fn table_for(idx: usize) -> Option<&'static HashMap<&'static str, &'static str>> {
    static MAPS: OnceLock<Vec<HashMap<&'static str, &'static str>>> = OnceLock::new();
    if idx == 0 {
        return None;
    }
    let maps = MAPS.get_or_init(|| {
        (1..LANGUAGES.len())
            .map(|lang| {
                tables::TABLE
                    .iter()
                    .map(|(en, tr)| (*en, tr[lang - 1]))
                    .collect()
            })
            .collect()
    });
    maps.get(idx - 1)
}

/// Terjemahkan teks UI. Teks tanpa terjemahan dikembalikan apa adanya.
pub fn tr(text: &'static str) -> &'static str {
    table_for(CURRENT.load(Ordering::Relaxed))
        .and_then(|m| m.get(text).copied())
        .unwrap_or(text)
}

/// Terjemahkan lalu isi placeholder `{}` berurutan.
pub fn trf(text: &'static str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut rest = tr(text);
    for arg in args {
        match rest.split_once("{}") {
            Some((head, tail)) => {
                out.push_str(head);
                out.push_str(arg);
                rest = tail;
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// Apakah bahasa butuh font CJK tambahan.
pub fn needs_cjk_font(code: &str) -> bool {
    matches!(code, "ko" | "zh")
}

fn cjk_font_candidates(code: &str) -> Vec<(&'static str, u32)> {
    // iOS/Android tidak punya kandidat, jadi `mut` tidak terpakai di sana.
    #[cfg_attr(
        not(any(target_os = "macos", target_os = "windows", target_os = "linux")),
        allow(unused_mut)
    )]
    let mut v: Vec<(&'static str, u32)> = Vec::new();
    #[cfg(target_os = "macos")]
    {
        if code == "ko" {
            v.push(("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0));
            v.push(("/System/Library/Fonts/Supplemental/AppleGothic.ttf", 0));
        }
        v.push(("/System/Library/Fonts/Hiragino Sans GB.ttc", 0));
        v.push(("/System/Library/Fonts/STHeiti Light.ttc", 0));
        v.push(("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0));
    }
    #[cfg(target_os = "windows")]
    {
        if code == "ko" {
            v.push(("C:\\Windows\\Fonts\\malgun.ttf", 0));
        }
        v.push(("C:\\Windows\\Fonts\\msyh.ttc", 0));
        v.push(("C:\\Windows\\Fonts\\simsun.ttc", 0));
        v.push(("C:\\Windows\\Fonts\\malgun.ttf", 0));
    }
    #[cfg(target_os = "linux")]
    {
        let _ = code;
        for p in [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
        ] {
            v.push((p, 0));
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let _ = code;
    v
}

/// Pasang font fallback untuk bahasa aktif (sekali per proses). Aman
/// dipanggil tiap frame; hanya bekerja saat bahasa CJK pertama kali aktif.
pub fn install_fonts(ctx: &eframe::egui::Context) {
    use eframe::egui;
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    static INSTALLED: OnceLock<bool> = OnceLock::new();

    let code = current_language();
    if !needs_cjk_font(code) || INSTALLED.get().is_some() {
        return;
    }
    let found = cjk_font_candidates(code)
        .into_iter()
        .find_map(|(path, index)| std::fs::read(path).ok().map(|bytes| (path, index, bytes)));
    let ok = match found {
        Some((path, index, bytes)) => {
            let mut data = egui::FontData::from_owned(bytes);
            data.index = index;
            ctx.add_font(FontInsert::new(
                "tabular-cjk-fallback",
                data,
                vec![
                    InsertFontFamily {
                        family: egui::FontFamily::Proportional,
                        priority: FontPriority::Lowest,
                    },
                    InsertFontFamily {
                        family: egui::FontFamily::Monospace,
                        priority: FontPriority::Lowest,
                    },
                ],
            ));
            log::info!("[I18N] loaded CJK fallback font {path}");
            true
        }
        None => {
            log::warn!("[I18N] no CJK font found; {code} text may render as boxes");
            false
        }
    };
    let _ = INSTALLED.set(ok);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_translation_keeps_placeholders() {
        for (en, trs) in tables::TABLE {
            let want = en.matches("{}").count();
            for (i, t) in trs.iter().enumerate() {
                assert_eq!(
                    t.matches("{}").count(),
                    want,
                    "placeholder mismatch for {en:?} in {}",
                    LANGUAGES[i + 1].0
                );
                assert!(!t.trim().is_empty(), "empty translation for {en:?}");
            }
        }
    }

    #[test]
    fn keys_are_unique() {
        let mut keys: Vec<_> = tables::TABLE.iter().map(|(en, _)| *en).collect();
        keys.sort();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len());
    }

    #[test]
    fn locale_codes_map_to_languages() {
        assert_eq!(index_of("id_ID.UTF-8"), Some(1));
        assert_eq!(index_of("in"), Some(1));
        assert_eq!(index_of("zh-Hans-CN"), Some(5));
        assert_eq!(index_of("ko-KR"), Some(2));
        assert_eq!(index_of("fr_FR"), None);
    }

    #[test]
    fn translate_and_fallback() {
        set_language("id");
        assert_eq!(tr("Preferences"), "Preferensi");
        assert_eq!(tr("A string nobody translated"), "A string nobody translated");
        assert_eq!(trf("Update {} available", &["1.2.0"]), "Pembaruan 1.2.0 tersedia");
        set_language("en");
        assert_eq!(tr("Preferences"), "Preferences");
        assert_eq!(trf("Update {} available", &["1.2.0"]), "Update 1.2.0 available");
    }
}
