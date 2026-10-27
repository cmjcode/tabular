//! Font kode yang bisa dipilih pengguna (Preferences > Editor > Font).
//!
//! Font pilihan dipasang di urutan teratas family `Monospace` egui, jadi
//! editor SQL, gutter, body HTTP, blok kode AI, dan teks monospace lain ikut
//! berubah. Font bawaan (Hack), font ikon, dan fallback CJK tetap berada di
//! belakangnya sehingga glyph yang tidak dimiliki font pilihan tetap tampil.
//!
//! Modul ini headless: tidak menyentuh `Tabular`; pemanggil UI cukup memberi
//! `FontDefinitions` dan path font.

use eframe::egui::{FontData, FontDefinitions, FontFamily};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Prefix nama font data milik modul ini di `FontDefinitions::font_data`.
const FONT_NAME_PREFIX: &str = "tabular-code-font:";

#[derive(Debug, thiserror::Error)]
pub enum CodeFontError {
    #[error("Could not read font file {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is not a usable TTF/OTF font: {reason}")]
    Invalid { path: String, reason: String },
}

/// Font monospace yang terpasang di sistem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledFont {
    pub label: String,
    pub path: PathBuf,
}

/// Font monospace populer: label dan nama file (huruf kecil) varian Regular.
const KNOWN_FONTS: &[(&str, &[&str])] = &[
    (
        "SF Mono",
        &["sfnsmono.ttf", "sfmono-regular.otf", "sf-mono-regular.otf"],
    ),
    ("Menlo", &["menlo.ttc"]),
    ("Monaco", &["monaco.ttf"]),
    (
        "JetBrains Mono",
        &["jetbrainsmono-regular.ttf", "jetbrainsmono[wght].ttf"],
    ),
    ("Fira Code", &["firacode-regular.ttf", "firacode[wght].ttf"]),
    (
        "FiraCode Nerd Font Mono",
        &["firacodenerdfontmono-regular.ttf"],
    ),
    (
        "Fira Mono",
        &["firamono-regular.ttf", "firamono-regular.otf"],
    ),
    (
        "Source Code Pro",
        &["sourcecodepro-regular.ttf", "sourcecodepro-regular.otf"],
    ),
    (
        "IBM Plex Mono",
        &["ibmplexmono-regular.ttf", "ibmplexmono-regular.otf"],
    ),
    (
        "Roboto Mono",
        &["robotomono-regular.ttf", "robotomono[wght].ttf"],
    ),
    ("Cascadia Code", &["cascadiacode.ttf"]),
    ("Cascadia Mono", &["cascadiamono.ttf"]),
    ("Consolas", &["consola.ttf"]),
    ("Lucida Console", &["lucon.ttf"]),
    ("Courier New", &["courier new.ttf", "cour.ttf"]),
    ("Andale Mono", &["andale mono.ttf"]),
    ("PT Mono", &["ptmono.ttc"]),
    ("Inconsolata", &["inconsolata-regular.ttf"]),
    ("DejaVu Sans Mono", &["dejavusansmono.ttf"]),
    ("Liberation Mono", &["liberationmono-regular.ttf"]),
    (
        "Ubuntu Mono",
        &["ubuntumono-r.ttf", "ubuntumono-regular.ttf"],
    ),
    ("Noto Sans Mono", &["notosansmono-regular.ttf"]),
    ("Cousine", &["cousine-regular.ttf"]),
];

/// Folder font sistem dan pengguna untuk OS ini.
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let home = dirs::home_dir();
    #[cfg(target_os = "macos")]
    {
        dirs.push("/System/Library/Fonts".into());
        dirs.push("/Library/Fonts".into());
        if let Some(h) = &home {
            dirs.push(h.join("Library/Fonts"));
        }
    }
    #[cfg(target_os = "windows")]
    {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        dirs.push(PathBuf::from(windir).join("Fonts"));
        if let Some(local) = dirs::data_local_dir() {
            dirs.push(local.join("Microsoft\\Windows\\Fonts"));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        dirs.push("/usr/share/fonts".into());
        dirs.push("/usr/local/share/fonts".into());
        if let Some(h) = &home {
            dirs.push(h.join(".local/share/fonts"));
            dirs.push(h.join(".fonts"));
        }
    }
    let _ = home;
    dirs
}

/// Kumpulkan file font (nama huruf kecil, path) dari `dir` sampai kedalaman `depth`.
fn collect_font_files(dir: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth > 0 {
                collect_font_files(&path, depth - 1, out);
            }
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            out.push((name.to_lowercase(), path));
        }
    }
}

/// Cocokkan daftar file dengan [`KNOWN_FONTS`]; urutan mengikuti tabel.
fn match_known_fonts(files: &[(String, PathBuf)]) -> Vec<InstalledFont> {
    KNOWN_FONTS
        .iter()
        .filter_map(|(label, names)| {
            names.iter().find_map(|wanted| {
                files
                    .iter()
                    .find(|(name, _)| name == wanted)
                    .map(|(_, path)| InstalledFont {
                        label: (*label).to_string(),
                        path: path.clone(),
                    })
            })
        })
        .collect()
}

/// Font monospace populer yang terpasang. Folder font dipindai sekali per
/// proses (font yang baru dipasang terlihat setelah restart).
pub fn installed_fonts() -> &'static [InstalledFont] {
    static CACHE: OnceLock<Vec<InstalledFont>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut files = Vec::new();
        for dir in font_dirs() {
            collect_font_files(&dir, 4, &mut files);
        }
        match_known_fonts(&files)
    })
}

/// Label tampilan untuk path font tersimpan ("" = font bawaan).
pub fn label_for(path: &str) -> String {
    if path.is_empty() {
        return "Hack (built-in)".to_string();
    }
    if let Some(font) = installed_fonts().iter().find(|f| f.path == Path::new(path)) {
        return font.label.clone();
    }
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Pastikan `bytes` bisa diparse egui. egui panic (setiap frame) bila font
/// data rusak, jadi validasi ini wajib sebelum font masuk ke `FontDefinitions`.
/// Font juga harus punya huruf Latin dasar agar layak dipakai untuk kode.
pub fn validate(bytes: &[u8], path: &str) -> Result<(), CodeFontError> {
    use skrifa::MetadataProvider;
    let invalid = |reason: String| CodeFontError::Invalid {
        path: path.to_string(),
        reason,
    };
    let font = skrifa::FontRef::from_index(bytes, 0).map_err(|e| invalid(e.to_string()))?;
    let charmap = font.charmap();
    if "AZaz09".chars().any(|c| charmap.map(c).is_none()) {
        return Err(invalid("it has no basic Latin letters".to_string()));
    }
    Ok(())
}

/// Baca dan validasi file font. I/O sinkron; dipanggil hanya saat pilihan
/// font berubah (sekali), bukan setiap frame.
pub fn load(path: &str) -> Result<FontData, CodeFontError> {
    let bytes = std::fs::read(path).map_err(|source| CodeFontError::Read {
        path: path.to_string(),
        source,
    })?;
    validate(&bytes, path)?;
    Ok(FontData::from_owned(bytes))
}

/// Salinan `defs` dengan font kode `font` (path, data) di urutan teratas
/// family `Monospace`; `None` mengembalikan ke font bawaan. Font kode
/// sebelumnya dilepas dari family dan dari `font_data`.
pub fn with_code_font(defs: &FontDefinitions, font: Option<(&str, FontData)>) -> FontDefinitions {
    let mut defs = defs.clone();
    defs.font_data
        .retain(|name, _| !name.starts_with(FONT_NAME_PREFIX));
    for names in defs.families.values_mut() {
        names.retain(|name| !name.starts_with(FONT_NAME_PREFIX));
    }
    if let Some((path, data)) = font {
        let name = format!("{FONT_NAME_PREFIX}{path}");
        defs.font_data
            .insert(name.clone(), std::sync::Arc::new(data));
        defs.families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, name);
    }
    defs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hack_bytes() -> Vec<u8> {
        FontDefinitions::default().font_data["Hack"].font.to_vec()
    }

    #[test]
    fn known_fonts_match_by_lowercase_file_name() {
        let files = vec![
            ("menlo.ttc".to_string(), PathBuf::from("/f/Menlo.ttc")),
            ("arial.ttf".to_string(), PathBuf::from("/f/Arial.ttf")),
            ("consola.ttf".to_string(), PathBuf::from("/w/consola.ttf")),
        ];
        let found = match_known_fonts(&files);
        let labels: Vec<_> = found.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, ["Menlo", "Consolas"]);
        assert_eq!(found[0].path, PathBuf::from("/f/Menlo.ttc"));
    }

    #[test]
    fn validate_accepts_real_font_and_rejects_garbage() {
        assert!(validate(&hack_bytes(), "Hack").is_ok());
        assert!(matches!(
            validate(b"not a font at all", "x.ttf"),
            Err(CodeFontError::Invalid { .. })
        ));
    }

    #[test]
    fn load_reports_missing_file() {
        assert!(matches!(
            load("/definitely/missing/font.ttf"),
            Err(CodeFontError::Read { .. })
        ));
    }

    #[test]
    fn code_font_goes_first_and_switching_replaces_it() {
        let base = FontDefinitions::default();
        let a = with_code_font(&base, Some(("/a.ttf", FontData::from_owned(hack_bytes()))));
        let mono = &a.families[&FontFamily::Monospace];
        assert_eq!(mono[0], "tabular-code-font:/a.ttf");
        // Fallback bawaan tetap ada di belakang font pilihan.
        assert!(mono.iter().any(|n| n == "Hack"));

        let b = with_code_font(&a, Some(("/b.ttf", FontData::from_owned(hack_bytes()))));
        let mono = &b.families[&FontFamily::Monospace];
        assert_eq!(mono[0], "tabular-code-font:/b.ttf");
        assert!(!mono.iter().any(|n| n.ends_with("/a.ttf")));
        assert!(!b.font_data.contains_key("tabular-code-font:/a.ttf"));

        let reset = with_code_font(&b, None);
        assert_eq!(reset, base);
    }

    #[test]
    fn builtin_label() {
        assert_eq!(label_for(""), "Hack (built-in)");
        assert_eq!(label_for("/x/MyFont-Regular.ttf"), "MyFont-Regular");
    }
}
