//! Format pesan commit untuk Git Graph: markdown inline sederhana
//! (`**tebal**`, `*miring*`, `` `kode` ``), shortcode emoji (`:sparkles:`),
//! tautan issue (`#123` → GitHub/GitLab atau pola URL dari Preferences), dan
//! URL biasa.
//!
//! Tokenizer-nya murni supaya bisa dites; perenderan egui ada di
//! [`render_message`].

use eframe::egui;

/// Shortcode emoji yang umum di pesan commit (gitmoji). Hanya glyph yang ada
/// di font emoji bawaan egui; shortcode lain dibiarkan apa adanya.
const EMOJI: &[(&str, &str)] = &[
    ("+1", "👍"),
    ("ambulance", "🚑"),
    ("arrow_down", "⬇"),
    ("arrow_up", "⬆"),
    ("art", "🎨"),
    ("beers", "🍻"),
    ("bento", "🍱"),
    ("boom", "💥"),
    ("bug", "🐛"),
    ("bulb", "💡"),
    ("busts_in_silhouette", "👥"),
    ("camera_flash", "📸"),
    ("card_file_box", "🗃"),
    ("chart_with_upwards_trend", "📈"),
    ("children_crossing", "🚸"),
    ("construction", "🚧"),
    ("construction_worker", "👷"),
    ("dizzy", "💫"),
    ("egg", "🥚"),
    ("fire", "🔥"),
    ("globe_with_meridians", "🌐"),
    ("green_heart", "💚"),
    ("heart", "❤"),
    ("heavy_check_mark", "✔"),
    ("heavy_minus_sign", "➖"),
    ("heavy_plus_sign", "➕"),
    ("iphone", "📱"),
    ("label", "🏷"),
    ("lipstick", "💄"),
    ("lock", "🔒"),
    ("loud_sound", "🔊"),
    ("mag", "🔍"),
    ("memo", "📝"),
    ("mute", "🔇"),
    ("package", "📦"),
    ("pencil", "📝"),
    ("pencil2", "✏"),
    ("poop", "💩"),
    ("pushpin", "📌"),
    ("recycle", "♻"),
    ("rewind", "⏪"),
    ("rocket", "🚀"),
    ("see_no_evil", "🙈"),
    ("seedling", "🌱"),
    ("smile", "😄"),
    ("sparkles", "✨"),
    ("speech_balloon", "💬"),
    ("tada", "🎉"),
    ("triangular_flag_on_post", "🚩"),
    ("truck", "🚚"),
    ("twisted_rightwards_arrows", "🔀"),
    ("warning", "⚠"),
    ("wastebasket", "🗑"),
    ("wheelchair", "♿"),
    ("white_check_mark", "✅"),
    ("wrench", "🔧"),
    ("x", "❌"),
    ("zap", "⚡"),
];

/// Ganti `:shortcode:` yang dikenal dengan emoji-nya.
pub fn emojify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find(':') {
        let (before, after) = rest.split_at(start);
        out.push_str(before);
        let tail = &after[1..];
        match tail.find(':') {
            Some(end)
                if end > 0
                    && tail[..end]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '+' || c == '-') =>
            {
                let code = &tail[..end];
                match EMOJI.binary_search_by(|(k, _)| k.cmp(&code)) {
                    Ok(i) => {
                        out.push_str(EMOJI[i].1);
                        rest = &tail[end + 1..];
                    }
                    Err(_) => {
                        out.push(':');
                        rest = tail;
                    }
                }
            }
            _ => {
                out.push(':');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Span {
    Text(String),
    Bold(String),
    Italic(String),
    Code(String),
    Link { text: String, url: String },
}

/// Pembuat tautan issue: regex nomor issue dan template URL (`$1` = grup 1).
pub struct IssueLinker {
    re: regex::Regex,
    template: String,
}

impl IssueLinker {
    /// `pattern` kosong = `#(\d+)`. `template` kosong = tanpa tautan issue.
    pub fn new(pattern: &str, template: &str) -> Option<Self> {
        if template.trim().is_empty() {
            return None;
        }
        let pattern = if pattern.trim().is_empty() {
            r"#(\d+)"
        } else {
            pattern.trim()
        };
        Some(Self {
            re: regex::Regex::new(pattern).ok()?,
            template: template.trim().to_string(),
        })
    }

    /// Template otomatis untuk repository GitHub/GitLab dari kunci `host/path`.
    pub fn template_for_key(key: &str, gitlab_host: &str) -> Option<String> {
        let (host, path) = key.split_once('/')?;
        if host == "github.com" {
            Some(format!("https://github.com/{path}/issues/$1"))
        } else if host == "gitlab.com" || host == gitlab_host {
            Some(format!("https://{host}/{path}/-/issues/$1"))
        } else {
            None
        }
    }

    fn url(&self, caps: &regex::Captures) -> String {
        let mut out = self.template.clone();
        for i in (1..caps.len()).rev() {
            let v = caps.get(i).map_or("", |m| m.as_str());
            out = out.replace(&format!("${i}"), v);
        }
        out
    }
}

/// Pecah teks polos menjadi teks + tautan (URL dan issue).
fn linkify(text: &str, issues: Option<&IssueLinker>, out: &mut Vec<Span>) {
    // Kumpulkan rentang tautan, lalu isi celahnya dengan teks biasa.
    let mut links: Vec<(usize, usize, String)> = Vec::new();
    let url_re = regex::Regex::new(r"https?://[^\s<>()]+[^\s<>().,;:!?]").ok();
    if let Some(re) = &url_re {
        for m in re.find_iter(text) {
            links.push((m.start(), m.end(), m.as_str().to_string()));
        }
    }
    if let Some(il) = issues {
        for caps in il.re.captures_iter(text) {
            let Some(m) = caps.get(0) else { continue };
            if links.iter().any(|(s, e, _)| m.start() < *e && m.end() > *s) {
                continue;
            }
            links.push((m.start(), m.end(), il.url(&caps)));
        }
    }
    links.sort_by_key(|l| l.0);
    let mut pos = 0;
    for (s, e, url) in links {
        if s > pos {
            out.push(Span::Text(text[pos..s].to_string()));
        }
        out.push(Span::Link {
            text: text[s..e].to_string(),
            url,
        });
        pos = e;
    }
    if pos < text.len() {
        out.push(Span::Text(text[pos..].to_string()));
    }
}

/// Tokenisasi satu baris pesan commit.
pub fn parse_line(line: &str, issues: Option<&IssueLinker>) -> Vec<Span> {
    let line = emojify(line);
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut rest = line.as_str();
    let flush = |plain: &mut String, out: &mut Vec<Span>| {
        if !plain.is_empty() {
            linkify(plain, issues, out);
            plain.clear();
        }
    };
    while !rest.is_empty() {
        let (marker, ctor): (&str, fn(String) -> Span) = if rest.starts_with("**") {
            ("**", Span::Bold)
        } else if rest.starts_with('`') {
            ("`", Span::Code)
        } else if rest.starts_with('*') || rest.starts_with('_') && plain.ends_with(' ') {
            (&rest[..1], Span::Italic)
        } else {
            let ch = rest.chars().next().unwrap_or(' ');
            plain.push(ch);
            rest = &rest[ch.len_utf8()..];
            continue;
        };
        let body = &rest[marker.len()..];
        match body.find(marker) {
            Some(end) if end > 0 => {
                flush(&mut plain, &mut out);
                out.push(ctor(body[..end].to_string()));
                rest = &body[end + marker.len()..];
            }
            _ => {
                plain.push_str(marker);
                rest = body;
            }
        }
    }
    flush(&mut plain, &mut out);
    out
}

/// Teks tampilan satu baris tanpa penanda markdown (untuk tabel graf dan
/// daftar History yang digambar dengan satu gaya).
pub fn plain_line(line: &str) -> String {
    parse_line(line, None)
        .into_iter()
        .map(|s| match s {
            Span::Text(t) | Span::Bold(t) | Span::Italic(t) | Span::Code(t) => t,
            Span::Link { text, .. } => text,
        })
        .collect()
}

/// Render pesan commit multi-baris.
pub fn render_message(ui: &mut egui::Ui, text: &str, issues: Option<&IssueLinker>, size: f32) {
    let code_bg = super::style::nav_track(ui.ctx());
    for line in text.lines() {
        if line.trim().is_empty() {
            ui.add_space(size * 0.6);
            continue;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for span in parse_line(line, issues) {
                match span {
                    Span::Text(s) => {
                        ui.label(egui::RichText::new(s).size(size));
                    }
                    Span::Bold(s) => {
                        ui.label(egui::RichText::new(s).size(size).strong());
                    }
                    Span::Italic(s) => {
                        ui.label(egui::RichText::new(s).size(size).italics());
                    }
                    Span::Code(s) => {
                        ui.label(
                            egui::RichText::new(s)
                                .size(size - 0.5)
                                .family(egui::FontFamily::Monospace)
                                .background_color(code_bg),
                        );
                    }
                    Span::Link { text, url } => {
                        ui.hyperlink_to(egui::RichText::new(text).size(size), url);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_table_is_sorted_for_binary_search() {
        assert!(EMOJI.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn replaces_known_shortcodes_only() {
        assert_eq!(emojify(":sparkles: add x"), "✨ add x");
        assert_eq!(emojify("time 10:30:00"), "time 10:30:00");
        assert_eq!(emojify(":unknown: :bug:"), ":unknown: 🐛");
        assert_eq!(emojify("a: b"), "a: b");
    }

    #[test]
    fn parses_markdown_and_links() {
        let il = IssueLinker::new("", "https://github.com/o/r/issues/$1").expect("linker");
        let spans = parse_line(
            "fix **login** in `auth.rs` (#12) see https://x.io/a.",
            Some(&il),
        );
        assert!(spans.contains(&Span::Bold("login".into())));
        assert!(spans.contains(&Span::Code("auth.rs".into())));
        assert!(spans.contains(&Span::Link {
            text: "#12".into(),
            url: "https://github.com/o/r/issues/12".into()
        }));
        assert!(spans.contains(&Span::Link {
            text: "https://x.io/a".into(),
            url: "https://x.io/a".into()
        }));
        // Penanda tanpa pasangan tetap teks.
        assert_eq!(parse_line("a * b", None), vec![Span::Text("a * b".into())]);
        assert_eq!(
            parse_line("snake_case_name", None),
            vec![Span::Text("snake_case_name".into())]
        );
    }

    #[test]
    fn plain_line_drops_markers() {
        assert_eq!(
            plain_line(":bug: add **users** in `api.rs`"),
            "🐛 add users in api.rs"
        );
    }

    #[test]
    fn issue_templates() {
        assert_eq!(
            IssueLinker::template_for_key("github.com/o/r", "gitlab.x").as_deref(),
            Some("https://github.com/o/r/issues/$1")
        );
        assert_eq!(
            IssueLinker::template_for_key("gitlab.x/g/s/r", "gitlab.x").as_deref(),
            Some("https://gitlab.x/g/s/r/-/issues/$1")
        );
        assert!(IssueLinker::template_for_key("bitbucket.org/o/r", "gitlab.x").is_none());
        let jira = IssueLinker::new(r"([A-Z]+-\d+)", "https://jira.x/browse/$1").expect("jira");
        let spans = parse_line("PROJ-7 done", Some(&jira));
        assert_eq!(
            spans[0],
            Span::Link {
                text: "PROJ-7".into(),
                url: "https://jira.x/browse/PROJ-7".into()
            }
        );
    }
}
