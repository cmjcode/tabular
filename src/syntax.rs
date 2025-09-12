//! Simple on-the-fly syntax highlighting (SQL / Redis / Mongo keywords)
use eframe::egui::{Color32, text::LayoutJob, TextFormat};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LanguageKind { Sql, Redis, Mongo, Plain }

pub fn detect_language_from_name(name: &str) -> LanguageKind {
    let lower = name.to_lowercase();
    if lower.ends_with(".sql") { return LanguageKind::Sql; }
    if lower.contains("redis") { return LanguageKind::Redis; }
    if lower.contains("mongo") { return LanguageKind::Mongo; }
    LanguageKind::Plain
}

// Cached highlighting with hash-based lookup
pub fn highlight_text_cached(
    text: &str, 
    lang: LanguageKind, 
    dark: bool,
    cache: &mut std::collections::HashMap<u64, LayoutJob>
) -> LayoutJob {
    // Create hash from text + lang + theme
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    lang.hash(&mut hasher);
    dark.hash(&mut hasher);
    let hash = hasher.finish();
    
    // Check cache first
    if let Some(cached_job) = cache.get(&hash) {
        return cached_job.clone();
    }
    
    // Generate new highlighting
    let job = highlight_text(text, lang, dark);
    
    // Cache the result (limit cache size to prevent memory bloat)
    if cache.len() > 100 {
        cache.clear(); // Simple eviction: clear all when limit reached
    }
    cache.insert(hash, job.clone());
    
    job
}

// Simple highlighting that processes the entire text at once
pub fn highlight_text(text: &str, lang: LanguageKind, dark: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    
    for line in text.lines() {
        if !job.text.is_empty() {
            job.append("\n", 0.0, TextFormat::default());
        }
        highlight_single_line(line, lang, dark, &mut job);
    }
    
    job
}

// Public single-line highlighter (excludes trailing newline). Useful for custom widget per-line rendering.
pub fn highlight_line(line: &str, lang: LanguageKind, dark: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    highlight_single_line(line, lang, dark, &mut job);
    job
}

fn highlight_single_line(line: &str, lang: LanguageKind, dark: bool, job: &mut LayoutJob) {
    // Handle SQL line comments first
    if matches!(lang, LanguageKind::Sql) && line.trim_start().starts_with("--") {
        job.append(line, 0.0, TextFormat { color: comment_color(dark), ..Default::default() });
        return;
    }
    
    let mut chars = line.char_indices().peekable();
    
    while let Some((start_idx, ch)) = chars.next() {
        if ch == '\'' {
            // String literal
            let mut end_idx = start_idx + 1;
            let mut found_end = false;
            
            while let Some((idx, c)) = chars.next() {
                end_idx = idx + c.len_utf8();
                if c == '\'' {
                    found_end = true;
                    break;
                }
            }
            
            if !found_end {
                end_idx = line.len();
            }
            
            job.append(&line[start_idx..end_idx], 0.0, TextFormat { 
                color: string_color(dark), 
                ..Default::default() 
            });
        } else if ch.is_ascii_alphabetic() || ch == '_' {
            // Word/identifier
            let mut end_idx = start_idx;
            let mut word_chars = vec![ch];
            
            while let Some(&(_, next_ch)) = chars.peek() {
                if next_ch.is_ascii_alphanumeric() || next_ch == '_' {
                    word_chars.push(next_ch);
                    let (idx, c) = chars.next().unwrap();
                    end_idx = idx + c.len_utf8();
                } else {
                    break;
                }
            }
            
            if end_idx == start_idx {
                end_idx = start_idx + ch.len_utf8();
            }
            
            let word = &line[start_idx..end_idx];
            let color = word_color(word, lang, dark);
            job.append(word, 0.0, TextFormat { color, ..Default::default() });
        } else if ch.is_whitespace() {
            // Whitespace - preserve exactly
            job.append(&ch.to_string(), 0.0, TextFormat { 
                color: normal_color(dark), 
                ..Default::default() 
            });
        } else {
            // Punctuation
            job.append(&ch.to_string(), 0.0, TextFormat { 
                color: punctuation_color(dark), 
                ..Default::default() 
            });
        }
    }
}

fn word_color(word: &str, lang: LanguageKind, dark: bool) -> Color32 {
    let up = word.to_ascii_uppercase();
    let keyword = match lang {
        LanguageKind::Sql => SQL_KEYWORDS.binary_search(&up.as_str()).is_ok(),
        LanguageKind::Redis => REDIS_CMDS.binary_search(&up.as_str()).is_ok(),
        LanguageKind::Mongo => MONGO_CMDS.binary_search(&up.as_str()).is_ok(),
        LanguageKind::Plain => false,
    };
    if keyword { return keyword_color(dark); }
    if word.chars().all(|c| c.is_ascii_digit()) { return number_color(dark); }
    normal_color(dark)
}

fn keyword_color(dark: bool) -> Color32 { 
    if dark { Color32::from_rgb(220,180,90) } else { Color32::from_rgb(160,60,0) } 
}
fn number_color(dark: bool) -> Color32 { 
    if dark { Color32::from_rgb(120,160,255) } else { Color32::from_rgb(0,90,200) } 
}
fn string_color(dark: bool) -> Color32 { 
    if dark { Color32::from_rgb(200,120,160) } else { Color32::from_rgb(160,0,120) } 
}
fn comment_color(dark: bool) -> Color32 { 
    if dark { Color32::from_rgb(120,120,120) } else { Color32::from_rgb(100,110,120) } 
}
fn punctuation_color(dark: bool) -> Color32 { 
    if dark { Color32::from_rgb(180,180,180) } else { Color32::from_rgb(80,80,80) } 
}
fn normal_color(dark: bool) -> Color32 { 
    if dark { Color32::from_rgb(210,210,210) } else { Color32::from_rgb(30,30,30) } 
}

static SQL_KEYWORDS: &[&str] = &[
    "ALL","ALTER","AND","AS","ASC","BY","CASE","CREATE","DELETE","DESC","DISTINCT","DROP",
    "ELSE","END","EXISTS","FROM","GROUP","HAVING","IF","IN","INDEX","INNER","INSERT","INTO",
    "IS","JOIN","LEFT","LIMIT","NOT","NULL","ON","OR","ORDER","OUTER","RIGHT","SELECT",
    "SET","TABLE","THEN","UNION","UPDATE","VALUES","WHEN","WHERE"
];
static REDIS_CMDS: &[&str] = &[
    "DEL","EXISTS","GET","HGETALL","INCR","LRANGE","RPUSH","SADD","SET","SMEMBERS","ZADD","ZRANGE"
];
static MONGO_CMDS: &[&str] = &[
    "AGGREGATE","COUNT","DELETE","DISTINCT","FIND","INSERT","UPDATE"
];
