//! Generate endpoint HTTP API dari repository kode untuk folder HTTP API.
//!
//! Alurnya mengikuti [`crate::repo_scan`] dan headless supaya bisa diuji:
//! 1. Repository disiapkan dengan [`repo_scan::choose_source`] +
//!    [`repo_scan::resolve_repo`] (folder lokal atau clone di cache).
//! 2. [`scan_routes`]: pencarian route deterministik untuk framework umum
//!    (Express/Nest/Next.js/SvelteKit, FastAPI/Flask/Django, Laravel/Symfony,
//!    Rails, Gin/Echo/Fiber/chi/net-http, Axum/Actix/Rocket, Spring/JAX-RS,
//!    ASP.NET). Hasilnya jadi daftar periksa kelengkapan dan cadangan bila AI
//!    gagal.
//! 3. AI (bila siap) menemukan endpoint yang terlewat lalu mendokumentasikan
//!    semua endpoint per batch: parameter, header, auth, body contoh, respons
//!    contoh, dan tabel database yang disentuh.
//! 4. [`add_endpoints_to_folder`] mengubah hasil menjadi `SavedRequest`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::time::Duration;

use regex::Regex;
use serde_json::Value;

use crate::agent::harness::ProgressStatus;
use crate::ai_assistant::ChatBackend;
use crate::http_collection::{HttpFolder, HttpWorkspace, SavedRequest};
use crate::models::structs::{HttpAuthType, HttpBodyType, HttpMethod};
use crate::repo_scan::{
    self, MAX_FILE_BYTES, MAX_LINE_BYTES, PromptMode, RepoJobEvent, RepoScanError, RepoSource, step,
};

/// Endpoint maksimum per giliran AI saat mendokumentasikan detail.
const BATCH_SIZE: usize = 20;
/// Batch AI yang dikerjakan bersamaan dalam satu job (bawaan).
pub const DEFAULT_PARALLEL_BATCHES: usize = 3;
/// Batas atas batch paralel per job yang bisa dipilih user.
pub const MAX_PARALLEL_BATCHES: usize = 6;
/// Batas giliran AI yang berjalan bersamaan dari semua job generate endpoint
/// (beberapa folder sekaligus), supaya provider tidak kebanjiran permintaan.
pub(crate) const MAX_GLOBAL_AI_TURNS: usize = 6;
/// Batas byte kode per batch pada mode cuplikan (backend API).
pub(crate) const SNIPPET_BYTES_PER_BATCH: usize = 60_000;
/// Batas byte satu file pada mode cuplikan.
const SNIPPET_BYTES_PER_FILE: usize = 16_000;

// ─── Pencarian route deterministik ──────────────────────────────────────────

/// Satu definisi route yang ditemukan di kode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteHit {
    /// Method huruf besar; `ANY` bila route menerima semua method.
    pub method: String,
    /// Path ternormalisasi (`/users/{id}`).
    pub path: String,
    /// Path file relatif terhadap root repository (pemisah `/`).
    pub file: String,
    pub line: usize,
    pub framework: &'static str,
}

impl RouteHit {
    pub fn source(&self) -> String {
        format!("{}:{}", self.file, self.line)
    }
}

/// Hasil pencarian route di seluruh repository.
#[derive(Debug, Clone, Default)]
pub struct RouteScan {
    pub files_scanned: usize,
    pub truncated: bool,
    pub hits: Vec<RouteHit>,
}

const CODE_EXTS: &[&str] = &[
    "js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "py", "php", "rb", "go", "rs", "java",
    "kt", "kts", "scala", "groovy", "cs",
];

/// Penerima pemanggilan `x.get('/path')` yang hampir pasti klien HTTP,
/// bukan definisi route server.
const CLIENT_RECEIVERS: &[&str] = &[
    "axios",
    "http",
    "https",
    "client",
    "fetch",
    "ky",
    "got",
    "superagent",
    "agent",
    "request",
    "req",
    "res",
    "cy",
    "page",
    "instance",
    "$http",
    "httpClient",
    "map",
    "params",
    "searchParams",
    "headers",
    "cache",
    "redis",
    "localStorage",
    "sessionStorage",
    "store",
    "config",
    "env",
    "resty",
    "session",
];

fn is_test_path(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.contains("/test/")
        || lower.contains("/tests/")
        || lower.starts_with("test/")
        || lower.starts_with("tests/")
        || lower.contains("__tests__")
        || lower.contains("/spec/")
        || lower.contains(".spec.")
        || lower.contains(".test.")
        || lower.ends_with("_test.go")
        || lower.contains("/e2e/")
        || lower.contains("/cypress/")
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid route regex")
}

macro_rules! lazy_re {
    ($name:ident, $pat:expr) => {
        fn $name() -> &'static Regex {
            static R: OnceLock<Regex> = OnceLock::new();
            R.get_or_init(|| re($pat))
        }
    };
}

lazy_re!(
    re_js_call,
    r#"(?:^|[^\w$.])([A-Za-z_$][\w$]*)\s*\.\s*(get|post|put|patch|delete|del|head|options|all)\s*\(\s*['"`](/[^'"`]*)['"`]"#
);
lazy_re!(
    re_js_route_chain,
    r#"\.route\(\s*['"`](/[^'"`]*)['"`]\s*\)"#
);
lazy_re!(
    re_js_chain_method,
    r#"\.\s*(get|post|put|patch|delete|all)\s*\("#
);
lazy_re!(
    re_nest_controller,
    r#"@Controller\(\s*(?:['"`]([^'"`]*)['"`]|\{[^}]*path\s*:\s*['"`]([^'"`]*)['"`])?"#
);
lazy_re!(
    re_nest_method,
    r#"@(Get|Post|Put|Patch|Delete|Head|Options|All)\(\s*(?:['"`]([^'"`]*)['"`])?\s*\)"#
);
lazy_re!(
    re_export_method,
    r#"export\s+(?:async\s+)?(?:function\s+|const\s+)(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\b"#
);
lazy_re!(
    re_py_decorator,
    r#"^\s*@([A-Za-z_]\w*)\.(get|post|put|patch|delete|head|options|route|api_route)\(\s*(?:(?:path|rule)\s*=\s*)?[rbu]?['"]([^'"]*)['"](.*)$"#
);
lazy_re!(
    re_py_router_var,
    r#"^\s*([A-Za-z_]\w*)\s*=\s*(?:\w+\.)?(APIRouter|Blueprint)\((.*)$"#
);
lazy_re!(
    re_py_prefix,
    r#"(?:url_prefix|prefix)\s*=\s*['"]([^'"]*)['"]"#
);
lazy_re!(re_quoted, r#"['"]([A-Za-z]+)['"]"#);
lazy_re!(re_py_methods, r#"methods\s*=\s*[\[\(\{]([^\]\)\}]*)"#);
lazy_re!(re_django_path, r#"\b(?:re_)?path\(\s*r?['"]([^'"]*)['"]"#);
lazy_re!(re_drf_register, r#"\.register\(\s*r?['"]([^'"]+)['"]"#);
lazy_re!(
    re_laravel_route,
    r#"Route::(get|post|put|patch|delete|options|any)\(\s*['"]([^'"]*)['"]"#
);
lazy_re!(
    re_laravel_match,
    r#"Route::match\(\s*\[([^\]]*)\]\s*,\s*['"]([^'"]*)['"]"#
);
lazy_re!(
    re_laravel_resource,
    r#"Route::(resource|apiResource)\(\s*['"]([^'"]+)['"]"#
);
lazy_re!(re_laravel_prefix, r#"prefix\(\s*['"]([^'"]*)['"]\s*\)"#);
lazy_re!(
    re_symfony_route,
    r#"(?:#\[|@)Route\(\s*(?:path\s*[:=]\s*)?['"]([^'"]*)['"](.*)$"#
);
lazy_re!(re_symfony_methods, r#"methods\s*[:=]\s*[\[\{]([^\]\}]*)"#);
lazy_re!(
    re_rails_verb,
    r#"^\s*(get|post|put|patch|delete|match)\s+['"]([^'"]+)['"](.*)$"#
);
lazy_re!(re_rails_resources, r#"^\s*(resources?)\s+:(\w+)(.*)$"#);
lazy_re!(
    re_rails_namespace,
    r#"^\s*(?:namespace|scope)\s+(?::(\w+)|(?:path:\s*)?['"]([^'"]*)['"])"#
);
lazy_re!(re_rails_root, r#"^\s*root\b"#);
lazy_re!(re_rails_only, r#"only:\s*(?:\[([^\]]*)\]|:(\w+))"#);
lazy_re!(
    re_go_group,
    r#"(\w+)\s*:?=\s*(\w+)\.(?:Group|MapGroup)\(\s*"([^"]*)""#
);
lazy_re!(
    re_go_upper,
    r#"(\w+)\.(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS|Any)\(\s*"([^"]*)""#
);
lazy_re!(
    re_go_title,
    r#"(\w+)\.(Get|Post|Put|Patch|Delete|Head|Options|All)\(\s*"(/[^"]*)""#
);
lazy_re!(
    re_go_handle,
    r#"\.(?:HandleFunc|Handle)\(\s*"([^"]+)"(.*)$"#
);
lazy_re!(re_go_methods, r#"\.Methods\(([^)]*)\)"#);
lazy_re!(re_rs_route, r#"\.route\(\s*"([^"]+)"\s*,(.*)$"#);
lazy_re!(
    re_rs_method_call,
    r#"(?:^|[^\w])(get|post|put|patch|delete|head|options|any)(?:_service)?\s*\("#
);
lazy_re!(
    re_rs_attr,
    r#"#\[\s*(?:\w+::)?(get|post|put|patch|delete|head|options)\s*\(\s*"([^"]*)""#
);
lazy_re!(re_rs_resource, r#"web::resource\(\s*"([^"]+)"\s*\)(.*)$"#);
lazy_re!(
    re_spring_mapping,
    r#"@(Get|Post|Put|Patch|Delete)Mapping\b(?:\s*\((.*))?"#
);
lazy_re!(re_spring_request, r#"@RequestMapping\b(?:\s*\((.*))?"#);
lazy_re!(re_spring_request_method, r#"RequestMethod\.(\w+)"#);
lazy_re!(re_first_string, r#""([^"]*)""#);
lazy_re!(re_jaxrs_path, r#"@Path\(\s*"([^"]*)"\s*\)"#);
lazy_re!(
    re_jaxrs_method,
    r#"^\s*@(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\s*$"#
);
lazy_re!(re_class_decl, r#"\b(?:class|interface|object)\s+\w+"#);
lazy_re!(re_cs_route, r#"\[Route\(\s*"([^"]*)"\s*\)\]"#);
lazy_re!(
    re_cs_http,
    r#"\[Http(Get|Post|Put|Patch|Delete|Head|Options)(?:\(\s*"([^"]*)"[^)]*\))?\]"#
);
lazy_re!(re_cs_controller, r#"class\s+(\w+?)Controller\b"#);
lazy_re!(
    re_cs_minimal,
    r#"(\w+)\.Map(Get|Post|Put|Patch|Delete)\(\s*"([^"]*)""#
);
lazy_re!(
    re_next_app,
    r#"(?:^|/)app/((?:[^/]+/)*)route\.(?:ts|js|tsx|jsx|mjs)$"#
);
lazy_re!(
    re_next_pages,
    r#"(?:^|/)pages/api/(.+)\.(?:ts|js|tsx|jsx)$"#
);
lazy_re!(
    re_sveltekit,
    r#"(?:^|/)src/routes/((?:[^/]+/)*)\+server\.(?:ts|js)$"#
);
lazy_re!(re_django_named, r#"\(\?P<(\w+)>[^)]*\)"#);
lazy_re!(
    re_code_secret,
    r#"(?i)((?:password|passwd|secret|token|api[_-]?key|private[_-]?key|client[_-]?secret)\w*["']?\s*[:=]>?\s*)(["'`])[^"'`\n]{4,}(["'`])"#
);

/// Ubah satu segmen path parameter ke bentuk `{nama}`.
fn convert_segment(seg: &str) -> String {
    if let Some(caps) = re_django_named().captures(seg) {
        return format!("{{{}}}", &caps[1]);
    }
    if let Some(name) = seg.strip_prefix(':') {
        let name = name.trim_end_matches('?');
        return format!("{{{name}}}");
    }
    if seg.starts_with('<') && seg.ends_with('>') {
        let inner = &seg[1..seg.len() - 1];
        let name = inner.rsplit(':').next().unwrap_or(inner);
        return format!("{{{name}}}");
    }
    if seg.starts_with('{') && seg.ends_with('}') {
        let inner = &seg[1..seg.len() - 1];
        let name = inner.split(':').next().unwrap_or(inner);
        let name = name.trim_start_matches('*').trim_end_matches('?');
        return format!("{{{name}}}");
    }
    if seg.starts_with('[') && seg.ends_with(']') && !seg.contains("controller") {
        let name = seg.trim_matches(|c| c == '[' || c == ']');
        let name = name.trim_start_matches("...");
        return format!("{{{name}}}");
    }
    if let Some(name) = seg.strip_prefix('*').filter(|n| !n.is_empty()) {
        return format!("{{{name}}}");
    }
    seg.to_string()
}

/// Gabungkan prefix dan path route lalu normalkan parameter ke `{nama}`.
pub fn normalize_route(prefix: &str, path: &str) -> String {
    let mut segments: Vec<String> = Vec::new();
    for part in [prefix, path] {
        let part = part.trim().trim_start_matches('^').trim_end_matches('$');
        for seg in part.split('/') {
            let seg = seg.trim();
            if seg.is_empty() || seg == "." {
                continue;
            }
            segments.push(convert_segment(seg));
        }
    }
    format!("/{}", segments.join("/"))
}

/// Kunci pembanding endpoint: method + path dengan nama parameter dibuang.
pub fn route_key(method: &str, path: &str) -> String {
    static PARAM: OnceLock<Regex> = OnceLock::new();
    let param = PARAM.get_or_init(|| re(r"\{[^}]*\}"));
    let norm = normalize_route("", path).to_ascii_lowercase();
    format!(
        "{} {}",
        method.trim().to_ascii_uppercase(),
        param.replace_all(&norm, "{}")
    )
}

fn push_hit(
    out: &mut Vec<RouteHit>,
    method: &str,
    prefix: &str,
    path: &str,
    rel: &str,
    line: usize,
    framework: &'static str,
) {
    let method = match method.to_ascii_uppercase().as_str() {
        "DEL" => "DELETE".to_string(),
        "ALL" | "ANY" | "MATCH" => "ANY".to_string(),
        m => m.to_string(),
    };
    out.push(RouteHit {
        method,
        path: normalize_route(prefix, path),
        file: rel.to_string(),
        line,
        framework,
    });
}

/// Nama method di dalam daftar string (`['GET', "post"]`).
fn quoted_methods(list: &str) -> Vec<String> {
    re_quoted()
        .captures_iter(list)
        .map(|c| c[1].to_ascii_uppercase())
        .filter(|m| {
            matches!(
                m.as_str(),
                "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
            )
        })
        .collect()
}

/// Baris `idx` diikuti deklarasi class dalam beberapa baris berikutnya
/// (anotasi di level class = prefix route).
fn class_follows(lines: &[&str], idx: usize) -> bool {
    for l in lines.iter().skip(idx + 1).take(5).map(|l| l.trim()) {
        if re_class_decl().is_match(l) {
            return true;
        }
        let annotation =
            l.is_empty() || l.starts_with('@') || l.starts_with('[') || l.starts_with("#[");
        if !annotation {
            return false;
        }
    }
    false
}

/// Route dari lokasi file (Next.js app/pages router, SvelteKit).
fn scan_file_based(rel: &str, text: &str, out: &mut Vec<RouteHit>) {
    let dir_route = |dir: &str| -> String {
        dir.split('/')
            .filter(|s| !s.is_empty())
            // Route group `(x)` dan slot `@x` tidak masuk URL.
            .filter(|s| !(s.starts_with('@') || (s.starts_with('(') && s.ends_with(')'))))
            .collect::<Vec<_>>()
            .join("/")
    };
    let exported = |framework: &'static str, path: String, out: &mut Vec<RouteHit>| {
        for (i, line) in text.lines().enumerate() {
            if let Some(c) = re_export_method().captures(line) {
                push_hit(out, &c[1], "", &path, rel, i + 1, framework);
            }
        }
    };
    if let Some(c) = re_next_app().captures(rel) {
        exported("nextjs", dir_route(&c[1]), out);
    } else if let Some(c) = re_sveltekit().captures(rel) {
        exported("sveltekit", dir_route(&c[1]), out);
    } else if let Some(c) = re_next_pages().captures(rel) {
        let mut path = format!("api/{}", &c[1]);
        if let Some(stripped) = path.strip_suffix("/index") {
            path = stripped.to_string();
        }
        push_hit(out, "ANY", "", &path, rel, 1, "nextjs");
    }
}

fn scan_js(rel: &str, text: &str, out: &mut Vec<RouteHit>) {
    let lines: Vec<&str> = text.lines().collect();
    let server_markers = [
        "express", "Router(", "fastify", "koa", "hono", "@nestjs", "elysia", "restify",
    ];
    let is_server = server_markers.iter().any(|m| text.contains(m));
    // File klien (axios/fetch) tanpa tanda framework server dilewati.
    let client_only = !is_server && (text.contains("axios") || text.contains("fetch("));

    let mut nest_prefix = String::new();
    for (i, line) in lines.iter().enumerate() {
        if line.len() > MAX_LINE_BYTES {
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        if let Some(c) = re_nest_controller().captures(line) {
            nest_prefix = c
                .get(1)
                .or_else(|| c.get(2))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            continue;
        }
        if let Some(c) = re_nest_method().captures(line) {
            let path = c.get(2).map(|m| m.as_str()).unwrap_or("");
            push_hit(out, &c[1], &nest_prefix, path, rel, i + 1, "nestjs");
            continue;
        }
        if client_only {
            continue;
        }
        if let Some(c) = re_js_route_chain().captures(line) {
            let path = c[1].to_string();
            let start = c.get(0).map_or(0, |m| m.end());
            // Method berantai boleh berlanjut di baris berikutnya sampai `;`.
            let mut found = false;
            for (j, l) in lines.iter().enumerate().skip(i).take(8) {
                let from = if j == i { start.min(l.len()) } else { 0 };
                for m in re_js_chain_method().captures_iter(&l[from..]) {
                    push_hit(out, &m[1], "", &path, rel, j + 1, "express");
                    found = true;
                }
                if j > i && (l.contains(';') || l.contains(".route(")) {
                    break;
                }
            }
            if found {
                continue;
            }
        }
        for c in re_js_call().captures_iter(line) {
            if CLIENT_RECEIVERS.contains(&&c[1]) {
                continue;
            }
            push_hit(out, &c[2], "", &c[3], rel, i + 1, "express");
        }
    }
}

fn scan_python(rel: &str, text: &str, out: &mut Vec<RouteHit>) {
    let is_urls = rel.to_ascii_lowercase().ends_with("urls.py");
    let mut prefixes: HashMap<String, String> = HashMap::new();
    for (i, line) in text.lines().enumerate() {
        if line.len() > MAX_LINE_BYTES || line.trim_start().starts_with('#') {
            continue;
        }
        if let Some(c) = re_py_router_var().captures(line) {
            let prefix = re_py_prefix()
                .captures(&c[3])
                .map(|p| p[1].to_string())
                .unwrap_or_default();
            prefixes.insert(c[1].to_string(), prefix);
            continue;
        }
        if let Some(c) = re_py_decorator().captures(line) {
            let prefix = prefixes.get(&c[1]).cloned().unwrap_or_default();
            let verb = &c[2];
            let path = &c[3];
            let framework = if verb == "route" { "flask" } else { "fastapi" };
            if verb == "route" || verb == "api_route" {
                let methods = re_py_methods()
                    .captures(&c[4])
                    .map(|m| quoted_methods(&m[1]))
                    .unwrap_or_default();
                if methods.is_empty() {
                    push_hit(out, "GET", &prefix, path, rel, i + 1, framework);
                }
                for m in methods {
                    push_hit(out, &m, &prefix, path, rel, i + 1, framework);
                }
            } else {
                push_hit(out, verb, &prefix, path, rel, i + 1, framework);
            }
            continue;
        }
        if is_urls {
            if let Some(c) = re_drf_register().captures(line) {
                push_rest_resource(out, &c[1], rel, i + 1, "django", true, None);
                continue;
            }
            if let Some(c) = re_django_path().captures(line)
                && !line.contains("include(")
            {
                push_hit(out, "ANY", "", &c[1], rel, i + 1, "django");
            }
        }
    }
}

/// Route REST standar untuk resource (`resources :users`, `apiResource`,
/// router DRF). `api_only` = tanpa halaman form `new`/`edit`.
fn push_rest_resource(
    out: &mut Vec<RouteHit>,
    base: &str,
    rel: &str,
    line: usize,
    framework: &'static str,
    api_only: bool,
    only: Option<&HashSet<String>>,
) {
    let base = base.trim_matches('/');
    let item = format!("{base}/{{id}}");
    let actions: [(&str, &str, String); 7] = [
        ("index", "GET", base.to_string()),
        ("create", "POST", base.to_string()),
        ("show", "GET", item.clone()),
        ("update", "PUT", item.clone()),
        ("destroy", "DELETE", item.clone()),
        ("new", "GET", format!("{base}/create")),
        ("edit", "GET", format!("{item}/edit")),
    ];
    for (action, method, path) in actions.iter() {
        if api_only && (*action == "new" || *action == "edit") {
            continue;
        }
        if let Some(only) = only
            && !only.contains(*action)
        {
            continue;
        }
        push_hit(out, method, "", path, rel, line, framework);
    }
}

/// Selisih kurung kurawal di satu baris (untuk group Laravel).
fn brace_delta(line: &str) -> i32 {
    line.chars().fold(0, |d, c| match c {
        '{' => d + 1,
        '}' => d - 1,
        _ => d,
    })
}

fn scan_php(rel: &str, text: &str, out: &mut Vec<RouteHit>) {
    let lower = rel.to_ascii_lowercase();
    let base_prefix = if lower.ends_with("routes/api.php") {
        "api"
    } else {
        ""
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut depth = 0i32;
    // (prefix, kedalaman minimum selama group masih terbuka)
    let mut stack: Vec<(String, i32)> = Vec::new();
    let mut class_prefix = String::new();
    for (i, line) in lines.iter().enumerate() {
        if line.len() > MAX_LINE_BYTES {
            continue;
        }
        let prefix = std::iter::once(base_prefix.to_string())
            .chain(stack.iter().map(|(p, _)| p.clone()))
            .collect::<Vec<_>>()
            .join("/");
        if let Some(c) = re_laravel_route().captures(line) {
            push_hit(out, &c[1], &prefix, &c[2], rel, i + 1, "laravel");
        } else if let Some(c) = re_laravel_match().captures(line) {
            for m in quoted_methods(&c[1]) {
                push_hit(out, &m, &prefix, &c[2], rel, i + 1, "laravel");
            }
        } else if let Some(c) = re_laravel_resource().captures(line) {
            let base = normalize_route(&prefix, &c[2]);
            push_rest_resource(
                out,
                &base,
                rel,
                i + 1,
                "laravel",
                &c[1] == "apiResource",
                None,
            );
        } else if let Some(c) = re_symfony_route().captures(line) {
            if class_follows(&lines, i) {
                class_prefix = c[1].to_string();
            } else {
                let methods = re_symfony_methods()
                    .captures(&c[2])
                    .map(|m| quoted_methods(&m[1]))
                    .unwrap_or_default();
                if methods.is_empty() {
                    push_hit(out, "ANY", &class_prefix, &c[1], rel, i + 1, "symfony");
                }
                for m in methods {
                    push_hit(out, &m, &class_prefix, &c[1], rel, i + 1, "symfony");
                }
            }
        }
        let delta = brace_delta(line);
        if line.contains("group(")
            && let Some(p) = re_laravel_prefix().captures(line)
        {
            stack.push((p[1].to_string(), depth + delta.max(1)));
        }
        depth += delta;
        while stack.last().is_some_and(|(_, d)| depth < *d) {
            stack.pop();
        }
    }
}

fn scan_rails(rel: &str, text: &str, out: &mut Vec<RouteHit>) {
    // Stack blok `do … end`: Some(prefix) untuk namespace/scope.
    let mut stack: Vec<Option<String>> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        let prefix = stack
            .iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>()
            .join("/");
        let opens_block = trimmed.ends_with(" do") || trimmed.contains(" do |");
        if let Some(c) = re_rails_namespace().captures(line) {
            let ns = c.get(1).or_else(|| c.get(2)).map_or("", |m| m.as_str());
            if opens_block {
                stack.push(Some(ns.to_string()));
            }
            continue;
        }
        if let Some(c) = re_rails_verb().captures(line) {
            push_hit(out, &c[1], &prefix, &c[2], rel, i + 1, "rails");
        } else if let Some(c) = re_rails_resources().captures(line) {
            let only: Option<HashSet<String>> =
                re_rails_only().captures(&c[3]).map(|o| match o.get(1) {
                    Some(list) => list
                        .as_str()
                        .split(',')
                        .map(|a| a.trim().trim_start_matches(':').to_string())
                        .collect(),
                    None => o
                        .get(2)
                        .map(|a| a.as_str().to_string())
                        .into_iter()
                        .collect(),
                });
            let base = normalize_route(&prefix, &c[2]);
            push_rest_resource(out, &base, rel, i + 1, "rails", false, only.as_ref());
        } else if re_rails_root().is_match(line) {
            push_hit(out, "GET", &prefix, "/", rel, i + 1, "rails");
        }
        if opens_block {
            stack.push(None);
        } else if trimmed == "end" {
            stack.pop();
        }
    }
}

fn scan_go(rel: &str, text: &str, out: &mut Vec<RouteHit>) {
    let mut groups: HashMap<String, String> = HashMap::new();
    for (i, line) in text.lines().enumerate() {
        if line.len() > MAX_LINE_BYTES || line.trim_start().starts_with("//") {
            continue;
        }
        if let Some(c) = re_go_group().captures(line) {
            let parent = groups.get(&c[2]).cloned().unwrap_or_default();
            groups.insert(c[1].to_string(), normalize_route(&parent, &c[3]));
            continue;
        }
        if let Some(c) = re_go_upper().captures(line) {
            let prefix = groups.get(&c[1]).cloned().unwrap_or_default();
            push_hit(out, &c[2], &prefix, &c[3], rel, i + 1, "go");
            continue;
        }
        if let Some(c) = re_go_title().captures(line)
            && !CLIENT_RECEIVERS.contains(&&c[1])
        {
            let prefix = groups.get(&c[1]).cloned().unwrap_or_default();
            push_hit(out, &c[2], &prefix, &c[3], rel, i + 1, "go");
            continue;
        }
        if let Some(c) = re_go_handle().captures(line) {
            let pattern = c[1].trim();
            // Go 1.22: "GET /users/{id}".
            if let Some((m, p)) = pattern.split_once(' ')
                && !m.is_empty()
                && m.chars().all(|ch| ch.is_ascii_uppercase())
            {
                push_hit(out, m, "", p.trim(), rel, i + 1, "go");
                continue;
            }
            let methods = re_go_methods()
                .captures(&c[2])
                .map(|m| quoted_methods(&m[1]))
                .unwrap_or_default();
            if methods.is_empty() {
                push_hit(out, "ANY", "", pattern, rel, i + 1, "go");
            }
            for m in methods {
                push_hit(out, &m, "", pattern, rel, i + 1, "go");
            }
        }
    }
}

fn scan_rust(rel: &str, text: &str, out: &mut Vec<RouteHit>) {
    for (i, line) in text.lines().enumerate() {
        if line.len() > MAX_LINE_BYTES || line.trim_start().starts_with("//") {
            continue;
        }
        if let Some(c) = re_rs_attr().captures(line) {
            push_hit(out, &c[1], "", &c[2], rel, i + 1, "rust");
            continue;
        }
        let (path, rest, framework) = if let Some(c) = re_rs_route().captures(line) {
            (c[1].to_string(), c[2].to_string(), "axum")
        } else if let Some(c) = re_rs_resource().captures(line) {
            (c[1].to_string(), c[2].to_string(), "actix")
        } else {
            continue;
        };
        let mut any = false;
        for m in re_rs_method_call().captures_iter(&rest) {
            push_hit(out, &m[1], "", &path, rel, i + 1, framework);
            any = true;
        }
        if !any {
            push_hit(out, "ANY", "", &path, rel, i + 1, framework);
        }
    }
}

fn scan_jvm(rel: &str, text: &str, out: &mut Vec<RouteHit>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut prefix = String::new();
    for (i, line) in lines.iter().enumerate() {
        if line.len() > MAX_LINE_BYTES || line.trim_start().starts_with("//") {
            continue;
        }
        if let Some(c) = re_spring_mapping().captures(line) {
            let path = c
                .get(2)
                .and_then(|a| re_first_string().captures(a.as_str()))
                .map(|s| s[1].to_string())
                .unwrap_or_default();
            push_hit(out, &c[1], &prefix, &path, rel, i + 1, "spring");
            continue;
        }
        if let Some(c) = re_spring_request().captures(line) {
            let args = c.get(1).map_or("", |a| a.as_str());
            let path = re_first_string()
                .captures(args)
                .map(|s| s[1].to_string())
                .unwrap_or_default();
            if class_follows(&lines, i) {
                prefix = path;
                continue;
            }
            let methods: Vec<String> = re_spring_request_method()
                .captures_iter(args)
                .map(|m| m[1].to_string())
                .collect();
            if methods.is_empty() {
                push_hit(out, "ANY", &prefix, &path, rel, i + 1, "spring");
            }
            for m in methods {
                push_hit(out, &m, &prefix, &path, rel, i + 1, "spring");
            }
            continue;
        }
        if let Some(c) = re_jaxrs_path().captures(line)
            && class_follows(&lines, i)
        {
            prefix = c[1].to_string();
            continue;
        }
        if let Some(c) = re_jaxrs_method().captures(line) {
            let lo = i.saturating_sub(3);
            let hi = (i + 4).min(lines.len());
            let sub = (lo..hi)
                .filter(|j| *j != i)
                .find_map(|j| {
                    re_jaxrs_path()
                        .captures(lines[j])
                        .filter(|_| !class_follows(&lines, j))
                        .map(|p| p[1].to_string())
                })
                .unwrap_or_default();
            push_hit(out, &c[1], &prefix, &sub, rel, i + 1, "jaxrs");
        }
    }
}

fn scan_csharp(rel: &str, text: &str, out: &mut Vec<RouteHit>) {
    let lines: Vec<&str> = text.lines().collect();
    let controller = re_cs_controller()
        .captures(text)
        .map(|c| c[1].to_string())
        .unwrap_or_default();
    let mut prefix = String::new();
    let mut groups: HashMap<String, String> = HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        if line.len() > MAX_LINE_BYTES || line.trim_start().starts_with("//") {
            continue;
        }
        if let Some(c) = re_cs_route().captures(line)
            && class_follows(&lines, i)
        {
            prefix = c[1]
                .replace("[controller]", &controller.to_ascii_lowercase())
                .replace("[action]", "");
            continue;
        }
        if let Some(c) = re_cs_http().captures(line) {
            let path = c.get(2).map_or("", |m| m.as_str());
            push_hit(out, &c[1], &prefix, path, rel, i + 1, "aspnet");
            continue;
        }
        if let Some(c) = re_go_group().captures(line) {
            let parent = groups.get(&c[2]).cloned().unwrap_or_default();
            groups.insert(c[1].to_string(), normalize_route(&parent, &c[3]));
            continue;
        }
        if let Some(c) = re_cs_minimal().captures(line) {
            let group_prefix = groups.get(&c[1]).cloned().unwrap_or_default();
            push_hit(out, &c[2], &group_prefix, &c[3], rel, i + 1, "aspnet");
        }
    }
}

/// Cari definisi route di satu file. `rel` menentukan bahasa (ekstensi) dan
/// route berbasis lokasi file.
pub fn scan_file(rel: &str, text: &str) -> Vec<RouteHit> {
    let lower = rel.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    let mut out = Vec::new();
    match ext {
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" => {
            scan_file_based(rel, text, &mut out);
            scan_js(rel, text, &mut out);
        }
        "py" => scan_python(rel, text, &mut out),
        "php" => scan_php(rel, text, &mut out),
        "rb" if lower.ends_with("routes.rb") || lower.contains("config/routes/") => {
            scan_rails(rel, text, &mut out)
        }
        "go" => scan_go(rel, text, &mut out),
        "rs" => scan_rust(rel, text, &mut out),
        "java" | "kt" | "kts" | "scala" | "groovy" => scan_jvm(rel, text, &mut out),
        "cs" => scan_csharp(rel, text, &mut out),
        _ => {}
    }
    let mut seen = HashSet::new();
    out.retain(|h| seen.insert((h.method.clone(), h.path.clone(), h.line)));
    out
}

/// Cari route di seluruh file kode repository.
pub fn scan_routes(root: &Path, cancel: &AtomicBool) -> Result<RouteScan, RepoScanError> {
    let (files, truncated) = repo_scan::list_repo_files(root, cancel)?;
    let mut scan = RouteScan {
        truncated,
        ..Default::default()
    };
    for path in files {
        if cancel.load(Ordering::SeqCst) {
            return Err(RepoScanError::Cancelled);
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let ext = rel.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        if !ext.is_some_and(|e| CODE_EXTS.contains(&e.as_str())) || is_test_path(&rel) {
            continue;
        }
        if std::fs::metadata(&path).map_or(true, |m| m.len() > MAX_FILE_BYTES) {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes[..bytes.len().min(8192)].contains(&0) {
            continue;
        }
        scan.files_scanned += 1;
        scan.hits
            .extend(scan_file(&rel, &String::from_utf8_lossy(&bytes)));
    }
    scan_flexurio_routes(root, &mut scan);
    Ok(scan)
}

/// Deteksi route Flexurio NoCode API dari routes.json dan entity/*.json jika ada.
fn scan_flexurio_routes(root: &Path, scan: &mut RouteScan) {
    let Some(routes_file) = crate::flexurio_import::detect_flexurio_config(root) else {
        return;
    };
    let Ok(data) = std::fs::read_to_string(&routes_file) else {
        return;
    };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) else {
        return;
    };
    let Some(routes) = val.get("routes").and_then(|r| r.as_array()) else {
        return;
    };

    scan.files_scanned += 1;
    let config_dir = routes_file.parent().unwrap_or(root);
    let entity_dir = config_dir.join("entity");
    let rel_routes = routes_file
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| routes_file.to_string_lossy().to_string());

    for (idx, r) in routes.iter().enumerate() {
        let Some(route) = r.as_str() else { continue };
        let route = route.trim();
        if route.is_empty() {
            continue;
        }

        let entity_file = entity_dir.join(format!("{route}.json"));
        let (rel_file, ent_opt) = if entity_file.is_file() {
            let rel = entity_file
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| entity_file.to_string_lossy().to_string());
            let parsed = std::fs::read_to_string(&entity_file).ok().and_then(|c| {
                serde_json::from_str::<crate::flexurio_import::FlexurioEntity>(&c).ok()
            });
            (rel, parsed)
        } else {
            (rel_routes.clone(), None)
        };

        let mut add_hit = |method: &str, path: String| {
            scan.hits.push(RouteHit {
                method: method.to_string(),
                path,
                file: rel_file.clone(),
                line: idx + 1,
                framework: "flexurio",
            });
        };

        if let Some(ent) = &ent_opt {
            if ent
                .get
                .as_ref()
                .and_then(|g| g.enable_method)
                .unwrap_or(true)
            {
                add_hit("GET", format!("/{route}"));
                add_hit("GET", format!("/{route}/{{id}}"));
            }
            if ent
                .post
                .as_ref()
                .and_then(|p| p.enable_method)
                .unwrap_or(true)
            {
                add_hit("POST", format!("/{route}"));
            }
            if ent
                .put
                .as_ref()
                .and_then(|p| p.enable_method)
                .unwrap_or(true)
            {
                add_hit("PUT", format!("/{route}/{{id}}"));
            }
            if ent
                .del
                .as_ref()
                .and_then(|d| d.enable_method)
                .unwrap_or(true)
            {
                add_hit("DELETE", format!("/{route}/{{id}}"));
            }
            if ent
                .patch
                .as_ref()
                .and_then(|p| p.enable_method)
                .unwrap_or(false)
            {
                add_hit("PATCH", format!("/{route}"));
            }
        } else {
            add_hit("GET", format!("/{route}"));
            add_hit("POST", format!("/{route}"));
            add_hit("PUT", format!("/{route}/{{id}}"));
            add_hit("DELETE", format!("/{route}/{{id}}"));
        }
    }

    // Tambahkan public routes jika ada (contoh: login, auth/refresh)
    if let Some(public_routes) = val.get("public").and_then(|p| p.as_array()) {
        for (idx, p) in public_routes.iter().enumerate() {
            let Some(pub_route) = p.as_str() else {
                continue;
            };
            let pub_route = pub_route.trim().trim_start_matches('/');
            if pub_route.is_empty() || routes.iter().any(|r| r.as_str() == Some(pub_route)) {
                continue;
            }
            scan.hits.push(RouteHit {
                method: "POST".to_string(),
                path: format!("/{pub_route}"),
                file: rel_routes.clone(),
                line: idx + 1,
                framework: "flexurio",
            });
        }
    }
}

/// Memperkaya GeneratedEndpoint bertipe "flexurio" dengan detail skema tabel,
/// query parameters, headers, authorization, dan contoh body JSON langsung dari
/// entity/*.json dan .env.
pub fn enrich_flexurio_endpoints(root: &Path, endpoints: &mut [GeneratedEndpoint]) {
    let Some(routes_file) = crate::flexurio_import::detect_flexurio_config(root) else {
        return;
    };
    let config_dir = routes_file.parent().unwrap_or(root);
    let entity_dir = config_dir.join("entity");

    let mut entity_cache: HashMap<String, Option<crate::flexurio_import::FlexurioEntity>> =
        HashMap::new();

    for ep in endpoints.iter_mut() {
        if ep.framework != "flexurio" {
            continue;
        }

        // Tentukan nama route dari path (contoh: /users -> users, /users/{id} -> users, /validate/users -> users)
        let route = ep
            .path
            .trim_start_matches('/')
            .split('/')
            .find(|seg| *seg != "validate" && !seg.starts_with('{') && !seg.ends_with('}'))
            .unwrap_or_default()
            .to_string();

        if route.is_empty() {
            continue;
        }

        ep.group = route.clone();

        // Autentikasi default Flexurio: Bearer Token
        if ep.auth.is_empty() {
            ep.auth = "bearer".to_string();
        }
        if ep.headers.is_empty() {
            ep.headers.push(ParamSpec {
                name: "Authorization".to_string(),
                example: "Bearer {{token}}".to_string(),
                required: true,
                description: "Bearer authentication token".to_string(),
            });
        }

        let ent_opt = entity_cache
            .entry(route.clone())
            .or_insert_with(|| {
                let entity_file = entity_dir.join(format!("{route}.json"));
                if entity_file.is_file() {
                    std::fs::read_to_string(&entity_file)
                        .ok()
                        .and_then(|c| serde_json::from_str(&c).ok())
                } else {
                    None
                }
            })
            .clone();

        let is_detail_path = ep.path.contains("{id}") || ep.path.contains("/{");

        if let Some(ent) = ent_opt {
            let mut tables = Vec::new();
            if !ent.table.is_empty() {
                tables.push(ent.table.clone());
            } else {
                tables.push(route.clone());
            }
            for d in &ent.details {
                if !d.target_table.is_empty() && !tables.contains(&d.target_table) {
                    tables.push(d.target_table.clone());
                }
            }
            ep.tables = tables;

            let pk_field = ent
                .primary_key
                .as_ref()
                .and_then(|k| k.columns.first())
                .cloned()
                .unwrap_or_else(|| "id".to_string());

            match ep.method.as_str() {
                "GET" => {
                    if is_detail_path {
                        if ep.name.is_empty() || ep.name.starts_with("GET") {
                            ep.name = format!("Get {route} by ID");
                        }
                        if ep.description.is_empty() {
                            ep.description = format!("Detail record `{route}` by {pk_field}.");
                        }
                        ep.status_codes = vec!["200".into(), "404".into(), "500".into()];
                    } else {
                        if ep.name.is_empty() || ep.name.starts_with("GET") {
                            ep.name = format!("Get {route} List");
                        }
                        if ep.description.is_empty() {
                            ep.description = format!("List & search records for `{route}`.");
                        }
                        ep.status_codes = vec!["200".into(), "500".into()];
                        if ep.query_params.is_empty() {
                            ep.query_params = vec![
                                ParamSpec {
                                    name: "page".to_string(),
                                    example: "1".to_string(),
                                    required: false,
                                    description: "Halaman data".to_string(),
                                },
                                ParamSpec {
                                    name: "per_page".to_string(),
                                    example: "10".to_string(),
                                    required: false,
                                    description: "Jumlah item per halaman".to_string(),
                                },
                                ParamSpec {
                                    name: "sort".to_string(),
                                    example: format!("{pk_field}:desc"),
                                    required: false,
                                    description: "Urutan data".to_string(),
                                },
                                ParamSpec {
                                    name: "filter".to_string(),
                                    example: String::new(),
                                    required: false,
                                    description: "Pencarian umum / filter".to_string(),
                                },
                            ];
                        }
                    }
                }
                "POST" => {
                    if ep.path.contains("validate") {
                        if ep.name.is_empty() || ep.name.starts_with("POST") {
                            ep.name = format!("Validate {route}");
                        }
                        if ep.description.is_empty() {
                            ep.description =
                                format!("Validasi payload record `{route}` tanpa simpan.");
                        }
                    } else {
                        if ep.name.is_empty() || ep.name.starts_with("POST") {
                            ep.name = format!("Create {route}");
                        }
                        if ep.description.is_empty() {
                            ep.description = format!(
                                "Tambah record baru `{route}` (mendukung transaksi atomik)."
                            );
                        }
                    }
                    ep.status_codes = vec!["201".into(), "400".into(), "500".into()];
                    ep.body_type = "json".to_string();
                    if ep.body_example.is_empty() {
                        ep.body_example =
                            crate::flexurio_import::build_entity_sample_body(&ent, true);
                    }
                }
                "PUT" => {
                    if ep.name.is_empty() || ep.name.starts_with("PUT") {
                        ep.name = format!("Update {route}");
                    }
                    if ep.description.is_empty() {
                        ep.description = format!("Perbarui record `{route}`.");
                    }
                    ep.status_codes = vec!["200".into(), "400".into(), "404".into(), "500".into()];
                    ep.body_type = "json".to_string();
                    if ep.body_example.is_empty() {
                        ep.body_example =
                            crate::flexurio_import::build_entity_sample_body(&ent, false);
                    }
                }
                "DELETE" => {
                    if ep.name.is_empty() || ep.name.starts_with("DELETE") {
                        ep.name = format!("Delete {route}");
                    }
                    if ep.description.is_empty() {
                        ep.description = format!("Hapus record `{route}`.");
                    }
                    ep.status_codes = vec!["200".into(), "404".into(), "500".into()];
                }
                "PATCH" => {
                    if ep.name.is_empty() || ep.name.starts_with("PATCH") {
                        ep.name = format!("Patch {route}");
                    }
                    if ep.description.is_empty() {
                        ep.description = format!("Perbarui sebagian record `{route}`.");
                    }
                    ep.status_codes = vec!["200".into(), "400".into(), "500".into()];
                    ep.body_type = "json".to_string();
                    if ep.body_example.is_empty() {
                        ep.body_example =
                            crate::flexurio_import::build_entity_sample_body(&ent, false);
                    }
                }
                _ => {}
            }

            ep.from_ai = true;
        } else {
            if ep.name.is_empty() {
                ep.name = format!("{} {}", ep.method, ep.path);
            }
        }
    }
}

/// Tebakan base URL dari framework yang paling banyak ditemukan.
pub fn default_base_url(hits: &[RouteHit]) -> String {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for h in hits {
        *counts.entry(h.framework).or_default() += 1;
    }
    let top = counts
        .into_iter()
        .max_by_key(|(f, c)| (*c, *f))
        .map(|(f, _)| f)
        .unwrap_or("");
    let port = match top {
        "fastapi" | "django" | "laravel" | "symfony" => 8000,
        "flask" | "aspnet" => 5000,
        "go" | "spring" | "jaxrs" | "actix" | "rust" | "flexurio" => 8080,
        "sveltekit" => 5173,
        _ => 3000,
    };
    format!("http://localhost:{port}")
}

// ─── Model endpoint hasil generate ──────────────────────────────────────────

/// Parameter/header/field form beserta contoh nilainya.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParamSpec {
    pub name: String,
    pub example: String,
    pub required: bool,
    pub description: String,
}

/// Endpoint lengkap hasil AI (atau hanya method + path dari pencarian teks).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GeneratedEndpoint {
    pub method: String,
    pub path: String,
    pub name: String,
    pub description: String,
    /// Nama sub-folder (resource/controller).
    pub group: String,
    pub path_params: Vec<ParamSpec>,
    pub query_params: Vec<ParamSpec>,
    pub headers: Vec<ParamSpec>,
    /// `bearer`, `basic`, `api_key`, `none` (kosong = tidak diketahui).
    pub auth: String,
    /// Nama header/param API key.
    pub auth_name: String,
    /// `json`, `form`, `multipart`, `xml`, `text`, `graphql`, `none`.
    pub body_type: String,
    pub body_example: String,
    pub form_fields: Vec<ParamSpec>,
    pub response_example: String,
    pub status_codes: Vec<String>,
    pub tables: Vec<String>,
    pub source: String,
    /// Didokumentasikan AI (bukan hanya ditemukan pencarian teks).
    pub from_ai: bool,
    pub framework: String,
}

impl GeneratedEndpoint {
    pub fn key(&self) -> String {
        route_key(&self.method, &self.path)
    }

    fn from_hit(hit: &RouteHit) -> Self {
        Self {
            method: hit.method.clone(),
            path: hit.path.clone(),
            source: hit.source(),
            framework: hit.framework.to_string(),
            path_params: path_param_names(&hit.path)
                .into_iter()
                .map(|n| ParamSpec {
                    name: n,
                    example: "1".to_string(),
                    required: true,
                    description: String::new(),
                })
                .collect(),
            ..Default::default()
        }
    }

    /// Nama tampil request.
    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            format!("{} {}", self.method, self.path)
        } else {
            self.name.trim().to_string()
        }
    }
}

/// Nama parameter `{x}` di path.
pub fn path_param_names(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
        .map(str::to_string)
        .collect()
}

fn value_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn str_field(v: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|k| v.get(*k).filter(|x| !x.is_null()))
        .map(value_text)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Contoh body/respons: string apa adanya, JSON di-pretty-print.
fn example_field(v: &Value, keys: &[&str]) -> String {
    match keys.iter().find_map(|k| v.get(*k).filter(|x| !x.is_null())) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(other) => serde_json::to_string_pretty(other).unwrap_or_default(),
        None => String::new(),
    }
}

fn params_field(v: &Value, key: &str) -> Vec<ParamSpec> {
    match v.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(name) => Some(ParamSpec {
                    name: name.clone(),
                    ..Default::default()
                }),
                Value::Object(_) => {
                    let name = str_field(item, &["name", "key", "field"]);
                    (!name.is_empty()).then(|| ParamSpec {
                        name,
                        example: str_field(item, &["example", "value", "default"]),
                        required: item
                            .get("required")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        description: str_field(item, &["description", "desc", "type"]),
                    })
                }
                _ => None,
            })
            .collect(),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(name, example)| ParamSpec {
                name: name.clone(),
                example: value_text(example),
                ..Default::default()
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn string_list(v: &Value, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .map(value_text)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_endpoint(v: &Value) -> Option<GeneratedEndpoint> {
    let method = str_field(v, &["method", "verb"]).to_ascii_uppercase();
    let path = str_field(v, &["path", "route", "url"]);
    if method.is_empty() || path.is_empty() {
        return None;
    }
    // URL lengkap dari AI: ambil path-nya saja.
    let path = if path.contains("://") {
        crate::http_collection::extract_endpoint_url(&path)
    } else {
        path
    };
    Some(GeneratedEndpoint {
        method,
        path: normalize_route("", &path),
        name: str_field(v, &["name", "summary", "title"]),
        description: str_field(v, &["description"]),
        group: str_field(v, &["group", "tag", "resource"]),
        path_params: params_field(v, "path_params"),
        query_params: params_field(v, "query_params"),
        headers: params_field(v, "headers"),
        auth: str_field(v, &["auth"]).to_ascii_lowercase(),
        auth_name: str_field(v, &["auth_name", "api_key_name"]),
        body_type: str_field(v, &["body_type", "content_type"]).to_ascii_lowercase(),
        body_example: example_field(v, &["body_example", "body", "request_body"]),
        form_fields: params_field(v, "form_fields"),
        response_example: example_field(v, &["response_example", "response"]),
        status_codes: string_list(v, "status_codes"),
        tables: string_list(v, "tables"),
        source: str_field(v, &["source", "file"]),
        from_ai: true,
        framework: String::new(),
    })
}

/// Urai jawaban AI: `{"base_url": …, "endpoints": [...]}` atau array endpoint.
pub fn parse_endpoints_reply(
    text: &str,
) -> Result<(Option<String>, Vec<GeneratedEndpoint>), RepoScanError> {
    let obj = repo_scan::slice_between(text, '{', '}')
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .filter(|v| v.get("endpoints").is_some());
    let (base, items) = match obj {
        Some(v) => {
            let base = str_field(&v, &["base_url", "baseUrl"]);
            let items = v
                .get("endpoints")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            ((!base.is_empty()).then_some(base), items)
        }
        None => {
            let arr = repo_scan::slice_between(text, '[', ']')
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .and_then(|v| v.as_array().cloned())
                .ok_or_else(|| {
                    RepoScanError::Parse(format!(
                        "expected a JSON object with \"endpoints\", got: {}",
                        repo_scan::truncate_chars(text.trim(), 200)
                    ))
                })?;
            (None, arr)
        }
    };
    Ok((base, items.iter().filter_map(parse_endpoint).collect()))
}

/// Cocokkan nama tabel dari AI ke daftar tabel diagram (tanpa membedakan
/// huruf besar/kecil, juga tanpa skema). Daftar kosong = terima apa adanya.
pub fn filter_tables(tables: &[String], candidates: &[String]) -> Vec<String> {
    let short = |s: &str| s.rsplit('.').next().unwrap_or(s).to_ascii_lowercase();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for t in tables {
        let t = t.trim().trim_matches(|c| c == '`' || c == '"');
        if t.is_empty() {
            continue;
        }
        let resolved = if candidates.is_empty() {
            Some(t.to_string())
        } else {
            candidates
                .iter()
                .find(|c| c.eq_ignore_ascii_case(t))
                .or_else(|| candidates.iter().find(|c| short(c) == short(t)))
                .cloned()
        };
        if let Some(r) = resolved
            && seen.insert(r.to_ascii_lowercase())
        {
            out.push(r);
        }
    }
    out
}

// ─── Prompt ─────────────────────────────────────────────────────────────────

const ENDPOINT_SCHEMA: &str = r#"{"base_url":"http://localhost:3000","endpoints":[{"method":"POST","path":"/users/{id}/orders","name":"Create order for user","description":"What it does, validation rules and side effects","group":"orders","path_params":[{"name":"id","example":"42","required":true,"description":"user id"}],"query_params":[{"name":"dry_run","example":"false","required":false,"description":""}],"headers":[{"name":"X-Tenant","example":"acme","required":true,"description":""}],"auth":"bearer","auth_name":"","body_type":"json","body_example":{"sku":"ABC-1","qty":2},"form_fields":[],"response_example":{"id":7,"status":"pending"},"status_codes":["201","400","404"],"tables":["orders","order_items"],"source":"src/routes/orders.ts:31"}]}"#;

fn common_rules(tables: &[String]) -> String {
    let mut s = String::from(
        "Rules:\n\
         - `path` uses `{name}` for path parameters, starts with `/` and includes every router, \
           controller, group, blueprint or mount prefix.\n\
         - `method` is one of GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS. A route that accepts \
           several methods is listed once per method.\n\
         - Fill `path_params`, `query_params`, `headers`, `form_fields` and `body_example` from \
           the handler, validators, DTOs, schemas, models and middleware, with realistic example \
           values. `body_type` is json, form, multipart, xml, text, graphql or none.\n\
         - `auth` is bearer, basic, api_key or none, based on middleware/guards; set `auth_name` \
           to the header or query name for api_key.\n\
         - `response_example` shows the success response shape; `status_codes` lists the codes \
           the handler can return.\n\
         - `tables` lists the database tables the endpoint reads or writes, following calls into \
           services, repositories, ORM models and raw SQL. ORM models map to tables by the \
           framework's naming rules.\n\
         - `source` is `path/to/file:line` of the route definition.\n\
         - Never put real secrets, tokens or passwords in examples; use placeholders.\n\
         - Never create, modify or remove files and never run commands that change anything.\n",
    );
    if !tables.is_empty() {
        let mut list = String::new();
        for t in tables {
            if list.len() > 20_000 {
                list.push_str(", …");
                break;
            }
            if !list.is_empty() {
                list.push_str(", ");
            }
            list.push_str(t);
        }
        s.push_str(&format!(
            "- Use table names from this list when they match: {list}\n"
        ));
    }
    s
}

pub(crate) fn repo_location(mode: PromptMode, repo_root: Option<&Path>) -> String {
    match (mode, repo_root) {
        (PromptMode::ReadRepo, Some(root)) => format!(
            "The repository is at `{}`, which is also your current working directory. Stay \
             inside it and inspect it with your file reading and search tools.\n",
            root.display()
        ),
        (PromptMode::ReadRepo, None) => "The repository is checked out in your current working \
             directory. Inspect it with your file reading and search tools.\n"
            .to_string(),
        (PromptMode::Snippets, _) => "You cannot open files. Judge only from the source code \
             included below.\n"
            .to_string(),
    }
}

/// Prompt penemuan: endpoint yang belum ada di daftar hasil pencarian teks.
pub fn build_discovery_prompts(
    folder_name: &str,
    known: &[GeneratedEndpoint],
    repo_root: Option<&Path>,
) -> (String, String) {
    let mut system = String::from(
        "You find every HTTP API endpoint that the server code in a source code repository \
         exposes: REST, RPC-style and GraphQL endpoints, including routes registered through \
         routers, controllers, decorators, annotations, file-based routing and generated \
         resource routes. Ignore client code that only calls other APIs, tests and fixtures.\n",
    );
    system.push_str(&repo_location(PromptMode::ReadRepo, repo_root));
    system.push_str(
        "Reply with ONLY one JSON object, no prose and no markdown fence:\n\
         {\"base_url\":\"http://localhost:<port the server listens on>\",\"endpoints\":\
         [{\"method\":\"GET\",\"path\":\"/full/path/{param}\",\"source\":\"path/to/file:line\"}]}\n\
         `path` includes every prefix from mounts, groups and controllers and uses `{name}` for \
         parameters. List only endpoints that are NOT in the known list below; an empty list is \
         fine. Never create, modify or remove files.\n",
    );
    let mut user = format!("HTTP API folder: {folder_name}\n\nKnown endpoints (already found):\n");
    if known.is_empty() {
        user.push_str("(none)\n");
    }
    for ep in known.iter().take(2_000) {
        user.push_str(&format!("{} {}  ({})\n", ep.method, ep.path, ep.source));
    }
    (system, user)
}

/// Prompt detail untuk satu batch endpoint.
pub fn build_detail_prompts(
    folder_name: &str,
    tables: &[String],
    batch: &[GeneratedEndpoint],
    mode: PromptMode,
    repo_root: Option<&Path>,
    snippets: &str,
) -> (String, String) {
    let mut system = String::from(
        "You document HTTP API endpoints from their server source code so a developer can call \
         them immediately. Read each route's handler and everything it calls.\n",
    );
    system.push_str(&repo_location(mode, repo_root));
    system.push_str(&common_rules(tables));
    system.push_str(
        "Reply with ONLY one JSON object, no prose and no markdown fence, shaped like this \
         example:\n",
    );
    system.push_str(ENDPOINT_SCHEMA);
    system.push('\n');
    system.push_str(
        "Document every endpoint in the list, correcting its method or path when the code \
         shows otherwise. Also include any other endpoint defined in the same files that is \
         missing from the list. `ANY` means the method was not detected; pick the real ones.\n",
    );
    let mut user = format!("HTTP API folder: {folder_name}\n\nEndpoints to document:\n");
    for ep in batch {
        user.push_str(&format!("- {} {}  ({})\n", ep.method, ep.path, ep.source));
    }
    if !snippets.is_empty() {
        user.push_str("\nSource code:\n");
        user.push_str(snippets);
    }
    (system, user)
}

/// Samarkan nilai literal yang tampak seperti secret di kode sebelum masuk
/// prompt backend API (`password = "…"` → `password = "«redacted»"`).
pub fn redact_code_secrets(code: &str) -> String {
    re_code_secret()
        .replace_all(
            code,
            format!("${{1}}${{2}}{}${{3}}", crate::http_ai::REDACTED),
        )
        .into_owned()
}

/// File sumber dari lokasi `path/file:line` (tanpa nomor baris).
pub(crate) fn source_file(source: &str) -> &str {
    source.rsplit_once(':').map_or(source, |(f, _)| f)
}

/// Cuplikan kode untuk mode tanpa akses file: isi file (terpotong) yang
/// memuat route batch ini, maksimal [`SNIPPET_BYTES_PER_BATCH`].
fn batch_snippets(root: &Path, batch: &[GeneratedEndpoint]) -> String {
    let files: Vec<&str> = batch.iter().map(|ep| source_file(&ep.source)).collect();
    file_snippets(root, &files)
}

/// Isi file `files` (relatif ke `root`, terpotong, bernomor baris dan sudah
/// diredaksi), maksimal [`SNIPPET_BYTES_PER_BATCH`]. File duplikat, kosong,
/// absolut atau yang keluar dari `root` dilewati.
pub(crate) fn file_snippets(root: &Path, files: &[&str]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = String::new();
    for &file in files {
        if file.is_empty() || seen.contains(&file) {
            continue;
        }
        seen.push(file);
        if out.len() >= SNIPPET_BYTES_PER_BATCH {
            break;
        }
        // Hanya path relatif di dalam root.
        if file.contains("..") || Path::new(file).is_absolute() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(file)) else {
            continue;
        };
        let budget = SNIPPET_BYTES_PER_FILE.min(SNIPPET_BYTES_PER_BATCH - out.len());
        let mut body = String::new();
        for (i, line) in text.lines().enumerate() {
            if body.len() + line.len() > budget {
                body.push_str("… (truncated)\n");
                break;
            }
            body.push_str(&format!("{:>5} {}\n", i + 1, line));
        }
        out.push_str(&format!("\n=== {file} ===\n{body}"));
    }
    redact_code_secrets(&out)
}

// ─── Job background ─────────────────────────────────────────────────────────

pub struct EndpointScanInput {
    pub repo_path: Option<String>,
    pub repo_url: Option<String>,
    pub folder_name: String,
    /// Tabel diagram bertautan (repository yang sama), untuk nama tabel.
    pub tables: Vec<String>,
    pub backend: Option<ChatBackend>,
    pub backend_label: String,
    pub cache_root: std::path::PathBuf,
    /// Jumlah batch AI yang dikerjakan bersamaan (1..=[`MAX_PARALLEL_BATCHES`]).
    pub parallel: usize,
}

/// Slot giliran AI global lintas job.
struct AiSlots {
    active: Mutex<usize>,
    freed: Condvar,
}

fn ai_slots() -> &'static AiSlots {
    static SLOTS: OnceLock<AiSlots> = OnceLock::new();
    SLOTS.get_or_init(|| AiSlots {
        active: Mutex::new(0),
        freed: Condvar::new(),
    })
}

/// Slot yang dipegang selama satu giliran AI; dilepas saat di-drop.
pub(crate) struct AiSlot;

impl Drop for AiSlot {
    fn drop(&mut self) {
        let slots = ai_slots();
        if let Ok(mut active) = slots.active.lock() {
            *active = active.saturating_sub(1);
        }
        slots.freed.notify_all();
    }
}

/// Tunggu sampai ada slot giliran AI global. Menunggu tetap bisa dibatalkan.
pub(crate) fn acquire_ai_slot(limit: usize, cancel: &AtomicBool) -> Result<AiSlot, RepoScanError> {
    let slots = ai_slots();
    let mut active = slots
        .active
        .lock()
        .map_err(|_| RepoScanError::Ai("AI slot lock poisoned".into()))?;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(RepoScanError::Cancelled);
        }
        if *active < limit {
            *active += 1;
            return Ok(AiSlot);
        }
        active = slots
            .freed
            .wait_timeout(active, Duration::from_millis(250))
            .map_err(|_| RepoScanError::Ai("AI slot lock poisoned".into()))?
            .0;
    }
}

/// Jalankan `work(i)` untuk `i` in `0..total` dengan `workers` thread.
/// Worker berhenti mengambil pekerjaan baru setelah `cancel`. Hasil diurutkan
/// menurut indeks.
pub(crate) fn run_pool<T: Send>(
    total: usize,
    workers: usize,
    cancel: &AtomicBool,
    work: impl Fn(usize) -> T + Sync,
) -> Vec<(usize, T)> {
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, T)>> = Mutex::new(Vec::with_capacity(total));
    std::thread::scope(|scope| {
        for _ in 0..workers.clamp(1, total.max(1)) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= total || cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    let out = work(i);
                    match results.lock() {
                        Ok(mut r) => r.push((i, out)),
                        Err(e) => e.into_inner().push((i, out)),
                    }
                }
            });
        }
    });
    let mut out = results.into_inner().unwrap_or_else(|e| e.into_inner());
    out.sort_by_key(|(i, _)| *i);
    out
}

/// Hasil satu batch detail: `(indeks batch, base URL, endpoint)` atau error.
type BatchResult = (
    usize,
    Result<(Option<String>, Vec<GeneratedEndpoint>), RepoScanError>,
);

#[derive(Debug, Clone, Default)]
pub struct EndpointScanOutcome {
    pub base_url: String,
    pub endpoints: Vec<GeneratedEndpoint>,
    pub note: Option<String>,
    pub files_scanned: usize,
    pub route_hits: usize,
}

pub type EndpointEvent = RepoJobEvent<EndpointScanOutcome>;

pub struct EndpointScanHandle {
    pub rx: mpsc::Receiver<EndpointEvent>,
    cancel: Arc<AtomicBool>,
}

impl EndpointScanHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

pub fn spawn_endpoint_scan(input: EndpointScanInput) -> EndpointScanHandle {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    std::thread::spawn(move || {
        let result = run_endpoint_scan(&input, &tx, &flag).map_err(|e| match e {
            RepoScanError::Cancelled => "Cancelled".to_string(),
            other => {
                log::warn!("[REPO_ENDPOINTS] scan failed: {other}");
                other.to_string()
            }
        });
        let _ = tx.send(RepoJobEvent::Finished(result));
    });
    EndpointScanHandle { rx, cancel }
}

/// Gabungkan endpoint: yang sudah ada didahulukan, yang baru ditambahkan
/// bila kuncinya (method + path) belum ada.
pub fn merge_endpoints(
    base: Vec<GeneratedEndpoint>,
    extra: Vec<GeneratedEndpoint>,
) -> Vec<GeneratedEndpoint> {
    let mut seen: HashSet<String> = base.iter().map(GeneratedEndpoint::key).collect();
    let mut out = base;
    for ep in extra {
        if seen.insert(ep.key()) {
            out.push(ep);
        }
    }
    out
}

/// Kelompokkan endpoint per file sumber lalu potong jadi batch.
fn batches(endpoints: &[GeneratedEndpoint]) -> Vec<Vec<GeneratedEndpoint>> {
    batches_by_file(endpoints, BATCH_SIZE, |ep| {
        source_file(&ep.source).to_string()
    })
}

/// Kelompokkan `items` per file (`file_of`) lalu potong jadi batch berisi
/// paling banyak `size`. Item dari file yang sama diusahakan satu batch.
pub(crate) fn batches_by_file<T: Clone>(
    items: &[T],
    size: usize,
    file_of: impl Fn(&T) -> String,
) -> Vec<Vec<T>> {
    let size = size.max(1);
    let mut by_file: Vec<(String, Vec<T>)> = Vec::new();
    for item in items {
        let file = file_of(item);
        match by_file.iter_mut().find(|(f, _)| *f == file) {
            Some((_, v)) => v.push(item.clone()),
            None => by_file.push((file, vec![item.clone()])),
        }
    }
    let mut out: Vec<Vec<T>> = Vec::new();
    let mut current: Vec<T> = Vec::new();
    for (_, group) in by_file {
        if !current.is_empty() && current.len() + group.len() > size {
            out.push(std::mem::take(&mut current));
        }
        for chunk in group.chunks(size) {
            if current.len() + chunk.len() > size {
                out.push(std::mem::take(&mut current));
            }
            current.extend_from_slice(chunk);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn run_endpoint_scan(
    input: &EndpointScanInput,
    tx: &mpsc::Sender<EndpointEvent>,
    cancel: &AtomicBool,
) -> Result<EndpointScanOutcome, RepoScanError> {
    let source = repo_scan::choose_source(input.repo_path.as_deref(), input.repo_url.as_deref())?;
    let shown = match &source {
        RepoSource::Local(p) => p.display().to_string(),
        RepoSource::Remote(u) => repo_scan::redact(u),
    };
    let prep = match source {
        RepoSource::Remote(_) => "Cloning or updating repository",
        RepoSource::Local(_) => "Opening local folder",
    };
    let _ = tx.send(step(1, prep, Some(shown.clone()), ProgressStatus::Active));
    let resolved = match repo_scan::resolve_repo(&source, &input.cache_root, cancel) {
        Ok(r) => r,
        Err(e) => {
            let _ = tx.send(step(1, prep, Some(e.to_string()), ProgressStatus::Error));
            return Err(e);
        }
    };
    let _ = tx.send(step(1, prep, Some(shown), ProgressStatus::Done));

    let finding = "Finding route definitions";
    let _ = tx.send(step(2, finding, None, ProgressStatus::Active));
    let scan = scan_routes(&resolved.root, cancel)?;
    let _ = tx.send(step(
        2,
        finding,
        Some(format!(
            "{} file(s) scanned{}, {} route(s) found",
            scan.files_scanned,
            if scan.truncated {
                " (limit reached)"
            } else {
                ""
            },
            scan.hits.len()
        )),
        ProgressStatus::Done,
    ));
    log::info!(
        "[REPO_ENDPOINTS] {} route(s) in {}",
        scan.hits.len(),
        resolved.root.display()
    );
    let mut text_endpoints = merge_endpoints(
        Vec::new(),
        scan.hits.iter().map(GeneratedEndpoint::from_hit).collect(),
    );
    enrich_flexurio_endpoints(&resolved.root, &mut text_endpoints);

    let mut base_url =
        if let Some(url) = crate::flexurio_import::detect_flexurio_base_url(&resolved.root) {
            url
        } else {
            default_base_url(&scan.hits)
        };

    let Some(backend) = input.backend.as_ref() else {
        let has_flexurio = text_endpoints.iter().any(|e| e.framework == "flexurio");
        return Ok(EndpointScanOutcome {
            base_url,
            endpoints: text_endpoints,
            note: if has_flexurio {
                Some(
                    "Endpoints, tables, parameters, and bodies loaded directly from Flexurio NoCode API configuration."
                        .into(),
                )
            } else {
                Some(
                    "No AI backend is ready, so only method and path from the text search are \
                     available."
                        .into(),
                )
            },
            files_scanned: scan.files_scanned,
            route_hits: scan.hits.len(),
        });
    };

    let (mode, workspace) =
        repo_scan::ai_workspace(backend, &resolved, &input.cache_root, tx, cancel, 3)?;
    let repo_root = workspace.cwd.clone();
    let mut notes: Vec<String> = Vec::new();

    // Langkah penemuan: hanya bila AI bisa membaca repository sendiri.
    let mut all = text_endpoints.clone();
    if mode == PromptMode::ReadRepo {
        let ask = format!(
            "Asking {} for endpoints the text search missed",
            input.backend_label
        );
        let _ = tx.send(step(4, ask.clone(), None, ProgressStatus::Active));
        let (system, user) =
            build_discovery_prompts(&input.folder_name, &text_endpoints, repo_root.as_deref());
        match repo_scan::ask_ai(backend, system, user, workspace.clone(), tx, cancel)
            .and_then(|t| parse_endpoints_reply(&t))
        {
            Ok((base, found)) => {
                if let Some(b) = base.filter(|b| b.starts_with("http")) {
                    base_url = b;
                }
                let before = all.len();
                all = merge_endpoints(all, found);
                let _ = tx.send(step(
                    4,
                    ask,
                    Some(format!("{} more endpoint(s)", all.len() - before)),
                    ProgressStatus::Done,
                ));
            }
            Err(RepoScanError::Cancelled) => return Err(RepoScanError::Cancelled),
            Err(e) => {
                let _ = tx.send(step(4, ask, Some(e.to_string()), ProgressStatus::Error));
                notes.push(format!("Endpoint discovery failed ({e})."));
            }
        }
    } else {
        notes.push(
            "The AI saw code snippets only (it could not open the repository), so endpoints the \
             text search missed may be absent."
                .into(),
        );
    }
    if all.is_empty() {
        notes.push("No route definitions were found in this repository.".into());
        return Ok(EndpointScanOutcome {
            base_url,
            endpoints: Vec::new(),
            note: Some(notes.join(" ")),
            files_scanned: scan.files_scanned,
            route_hits: scan.hits.len(),
        });
    }

    // Jika semua endpoint berasal dari konfigurasi Flexurio yang sudah lengkap, tidak perlu batch AI
    let non_flexurio_count = all.iter().filter(|e| e.framework != "flexurio").count();
    if non_flexurio_count == 0 && !all.is_empty() {
        let _ = tx.send(step(
            5,
            "Documenting endpoints from Flexurio NoCode API configuration".to_string(),
            Some(format!("{} endpoint(s) fully documented", all.len())),
            ProgressStatus::Done,
        ));
        return Ok(EndpointScanOutcome {
            base_url,
            endpoints: all,
            note: Some("Loaded directly from Flexurio NoCode API configuration (routes.json & entity/*.json).".into()),
            files_scanned: scan.files_scanned,
            route_hits: scan.hits.len(),
        });
    }

    // Detail per batch, beberapa batch sekaligus.
    let batches = batches(&all);
    let total = batches.len();
    let workers = input
        .parallel
        .clamp(1, MAX_PARALLEL_BATCHES)
        .min(total.max(1));
    let overview = format!("Documenting {total} batch(es) with {}", input.backend_label);
    let _ = tx.send(step(
        5,
        overview.clone(),
        Some(format!("{workers} in parallel")),
        ProgressStatus::Active,
    ));
    let results: Vec<BatchResult> = run_pool(total, workers, cancel, |i| {
        run_batch(
            input,
            backend,
            &workspace,
            mode,
            repo_root.as_deref(),
            &resolved.root,
            &batches[i],
            i,
            total,
            tx,
            cancel,
        )
    });
    if cancel.load(Ordering::SeqCst) {
        return Err(RepoScanError::Cancelled);
    }
    let mut detailed: Vec<GeneratedEndpoint> = Vec::new();
    let mut failed = 0usize;
    for (_, outcome) in results {
        match outcome {
            Ok((base, eps)) => {
                if let Some(b) = base.filter(|b| b.starts_with("http")) {
                    base_url = b;
                }
                detailed = merge_endpoints(detailed, eps);
            }
            Err(RepoScanError::Cancelled) => return Err(RepoScanError::Cancelled),
            Err(_) => failed += 1,
        }
    }
    let _ = tx.send(step(
        5,
        overview,
        Some(format!(
            "{} endpoint(s) documented, {failed} batch(es) failed",
            detailed.len()
        )),
        if failed == total {
            ProgressStatus::Error
        } else {
            ProgressStatus::Done
        },
    ));
    if failed > 0 {
        notes.push(format!(
            "{failed} of {total} AI batch(es) failed; those endpoints only have method and path."
        ));
    }
    for ep in &mut detailed {
        ep.tables = filter_tables(&ep.tables, &input.tables);
    }
    // Endpoint yang tidak dijawab AI tetap masuk dari hasil pencarian teks.
    let endpoints = merge_endpoints(detailed, all);
    Ok(EndpointScanOutcome {
        base_url,
        endpoints,
        note: Some(notes.join(" ")).filter(|n| !n.is_empty()),
        files_scanned: scan.files_scanned,
        route_hits: scan.hits.len(),
    })
}

/// Dokumentasikan satu batch: tunggu slot AI global, kirim prompt, urai
/// jawaban. Kemajuan dilaporkan sebagai langkah `10 + i`.
#[allow(clippy::too_many_arguments)]
fn run_batch(
    input: &EndpointScanInput,
    backend: &ChatBackend,
    workspace: &crate::ai_assistant::ChatWorkspace,
    mode: PromptMode,
    repo_root: Option<&Path>,
    root: &Path,
    batch: &[GeneratedEndpoint],
    i: usize,
    total: usize,
    tx: &mpsc::Sender<EndpointEvent>,
    cancel: &AtomicBool,
) -> Result<(Option<String>, Vec<GeneratedEndpoint>), RepoScanError> {
    let idx = 10 + i as u64;
    let label = format!("Batch {}/{}", i + 1, total);
    let _slot = acquire_ai_slot(MAX_GLOBAL_AI_TURNS, cancel)?;
    let _ = tx.send(step(
        idx,
        label.clone(),
        Some(format!("{} endpoint(s)", batch.len())),
        ProgressStatus::Active,
    ));
    let snippets = if mode == PromptMode::Snippets {
        batch_snippets(root, batch)
    } else {
        String::new()
    };
    let (system, user) = build_detail_prompts(
        &input.folder_name,
        &input.tables,
        batch,
        mode,
        repo_root.filter(|_| mode == PromptMode::ReadRepo),
        &snippets,
    );
    // Langkah agent tiap batch diberi rentang nomor sendiri.
    let offset = 1_000 * (i as u64 + 1);
    let result =
        repo_scan::ask_ai_with_offset(backend, system, user, workspace.clone(), tx, cancel, offset)
            .and_then(|t| parse_endpoints_reply(&t));
    match &result {
        Ok((_, eps)) => {
            let _ = tx.send(step(
                idx,
                label,
                Some(format!("{} endpoint(s) documented", eps.len())),
                ProgressStatus::Done,
            ));
        }
        Err(RepoScanError::Cancelled) => {}
        Err(e) => {
            log::warn!("[REPO_ENDPOINTS] batch {} failed: {e}", i + 1);
            let _ = tx.send(step(idx, label, Some(e.to_string()), ProgressStatus::Error));
        }
    }
    result
}

// ─── Konversi ke collection ─────────────────────────────────────────────────

fn http_method(m: &str) -> HttpMethod {
    match m.to_ascii_uppercase().as_str() {
        "POST" => HttpMethod::POST,
        "PUT" => HttpMethod::PUT,
        "DELETE" => HttpMethod::DELETE,
        "PATCH" => HttpMethod::PATCH,
        "HEAD" => HttpMethod::HEAD,
        "OPTIONS" => HttpMethod::OPTIONS,
        _ => HttpMethod::GET,
    }
}

/// Nama sub-folder untuk endpoint: `group` dari AI, atau segmen statis
/// pertama setelah prefix umum (`/api/v1/users/{id}` → `users`).
pub fn group_name(ep: &GeneratedEndpoint) -> String {
    if !ep.group.trim().is_empty() {
        return ep.group.trim().to_string();
    }
    let skip = |s: &str| {
        s.eq_ignore_ascii_case("api")
            || s.eq_ignore_ascii_case("rest")
            || (s.len() <= 3 && s.starts_with('v') && s[1..].chars().all(|c| c.is_ascii_digit()))
    };
    ep.path
        .split('/')
        .filter(|s| !s.is_empty() && !s.starts_with('{'))
        .find(|s| !skip(s))
        .unwrap_or("root")
        .to_string()
}

fn markdown_description(ep: &GeneratedEndpoint) -> String {
    let mut d = String::new();
    if !ep.description.is_empty() {
        d.push_str(&ep.description);
        d.push_str("\n\n");
    }
    d.push_str(&format!("`{} {}`\n", ep.method, ep.path));
    if !ep.path_params.is_empty() {
        d.push_str("\n**Path parameters**\n");
        for p in &ep.path_params {
            d.push_str(&format!("- `{}` {}\n", p.name, p.description));
        }
    }
    if !ep.status_codes.is_empty() {
        d.push_str(&format!(
            "\n**Status codes:** {}\n",
            ep.status_codes.join(", ")
        ));
    }
    if !ep.tables.is_empty() {
        d.push_str(&format!("\n**Tables:** {}\n", ep.tables.join(", ")));
    }
    if !ep.response_example.is_empty() {
        d.push_str(&format!(
            "\n**Response example**\n```json\n{}\n```\n",
            ep.response_example
        ));
    }
    if !ep.source.is_empty() {
        d.push_str(&format!("\n_Source: {}_\n", ep.source));
    }
    d
}

/// URL request: base URL + path dengan parameter diganti contoh nilainya.
pub fn request_url(base_url: &str, ep: &GeneratedEndpoint) -> String {
    let mut path = ep.path.clone();
    for name in path_param_names(&ep.path) {
        let example = ep
            .path_params
            .iter()
            .find(|p| p.name == name && !p.example.is_empty())
            .map(|p| p.example.clone())
            .unwrap_or_else(|| "1".to_string());
        path = path.replace(&format!("{{{name}}}"), &example);
    }
    format!("{}{}", base_url.trim().trim_end_matches('/'), path)
}

/// Ubah endpoint menjadi request tersimpan (id kosong: diisi pemanggil).
pub fn to_saved_request(ep: &GeneratedEndpoint, base_url: &str) -> SavedRequest {
    let kv = |ps: &[ParamSpec], default_on: bool| -> Vec<(String, String, bool)> {
        ps.iter()
            .map(|p| (p.name.clone(), p.example.clone(), default_on || p.required))
            .collect()
    };
    let body_type = match ep.body_type.as_str() {
        "json" | "application/json" => HttpBodyType::Json,
        "form" | "urlencoded" | "x-www-form-urlencoded" => HttpBodyType::UrlEncoded,
        "multipart" | "form-data" | "multipart/form-data" => HttpBodyType::MultiPart,
        "xml" => HttpBodyType::Xml,
        "graphql" => HttpBodyType::GraphQL,
        "text" => HttpBodyType::OtherText,
        _ if !ep.body_example.is_empty() => HttpBodyType::Json,
        _ => HttpBodyType::NoBody,
    };
    let (auth_type, api_key_name) = match ep.auth.as_str() {
        "bearer" | "jwt" | "oauth2" => (HttpAuthType::BearerToken, String::new()),
        "basic" => (HttpAuthType::BasicAuth, String::new()),
        "api_key" | "apikey" => (
            HttpAuthType::ApiKey,
            if ep.auth_name.is_empty() {
                "X-API-Key".to_string()
            } else {
                ep.auth_name.clone()
            },
        ),
        _ => (HttpAuthType::NoAuth, String::new()),
    };
    let mut form_data = kv(&ep.form_fields, true);
    if form_data.is_empty() {
        form_data.push((String::new(), String::new(), true));
    }
    SavedRequest {
        name: ep.display_name(),
        url: request_url(base_url, ep),
        method: http_method(&ep.method),
        params: kv(&ep.query_params, false),
        headers: kv(&ep.headers, false),
        body_text: if matches!(
            body_type,
            HttpBodyType::UrlEncoded | HttpBodyType::MultiPart | HttpBodyType::NoBody
        ) {
            String::new()
        } else {
            ep.body_example.clone()
        },
        body_type,
        form_data,
        auth_type,
        api_key_name,
        api_key_in_header: true,
        description: markdown_description(ep),
        tables: ep.tables.clone(),
        source: (!ep.source.is_empty()).then(|| ep.source.clone()),
        route: Some(ep.path.clone()),
        ..Default::default()
    }
}

/// Kunci endpoint request tersimpan (untuk mendeteksi duplikat).
pub fn saved_request_key(req: &SavedRequest) -> String {
    let path = req
        .route
        .clone()
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| crate::http_collection::extract_endpoint_url(&req.url));
    route_key(req.method.label(), &path)
}

/// Kunci semua request di folder (termasuk sub-folder).
pub fn folder_request_keys(folder: &HttpFolder) -> HashSet<String> {
    folder
        .all_requests()
        .into_iter()
        .map(saved_request_key)
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct AddStats {
    pub added: usize,
    pub updated: usize,
    /// Request hasil (baru atau diperbarui), untuk tautan ke diagram.
    pub requests: Vec<SavedRequest>,
}

fn find_request_by_key<'a>(folder: &'a mut HttpFolder, key: &str) -> Option<&'a mut SavedRequest> {
    if let Some(i) = folder
        .requests
        .iter()
        .position(|r| saved_request_key(r) == key)
    {
        return Some(&mut folder.requests[i]);
    }
    for child in folder.children.iter_mut() {
        if let Some(r) = find_request_by_key(child, key) {
            return Some(r);
        }
    }
    None
}

/// Tambahkan (atau perbarui) endpoint ke folder `folder_id`. Endpoint yang
/// sudah ada (method + path sama) diperbarui di tempatnya; nilai rahasia auth
/// yang sudah diisi user dipertahankan. `subfolders` = kelompokkan per
/// resource di sub-folder.
pub fn add_endpoints_to_folder(
    workspaces: &mut [HttpWorkspace],
    ws_id: &str,
    folder_id: &str,
    endpoints: &[GeneratedEndpoint],
    base_url: &str,
    subfolders: bool,
) -> Result<AddStats, String> {
    let ws = workspaces
        .iter_mut()
        .find(|w| w.id == ws_id)
        .ok_or_else(|| "The HTTP workspace no longer exists".to_string())?;
    let folder = crate::http_collection::find_folder_mut(&mut ws.folders, folder_id)
        .ok_or_else(|| "The HTTP folder no longer exists".to_string())?;
    let mut stats = AddStats::default();
    for ep in endpoints {
        let mut req = to_saved_request(ep, base_url);
        let key = saved_request_key(&req);
        if let Some(existing) = find_request_by_key(folder, &key) {
            req.id = existing.id.clone();
            req.workspace_id = existing.workspace_id.clone();
            req.folder_id = existing.folder_id.clone();
            // Rahasia yang sudah diisi user tidak ditimpa contoh kosong.
            req.bearer_token = std::mem::take(&mut existing.bearer_token);
            req.basic_user = std::mem::take(&mut existing.basic_user);
            req.basic_pass = std::mem::take(&mut existing.basic_pass);
            req.api_key_value = std::mem::take(&mut existing.api_key_value);
            *existing = req.clone();
            stats.updated += 1;
            stats.requests.push(req);
            continue;
        }
        let target: &mut HttpFolder = if subfolders {
            let name = group_name(ep);
            let idx = match folder
                .children
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case(&name))
            {
                Some(i) => i,
                None => {
                    folder.children.push(HttpFolder {
                        id: crate::http_collection::unique_id("fld"),
                        name,
                        parent_folder_id: Some(folder.id.clone()),
                        ..Default::default()
                    });
                    folder.children.len() - 1
                }
            };
            &mut folder.children[idx]
        } else {
            &mut *folder
        };
        req.id = crate::http_collection::unique_id("sr");
        req.workspace_id = ws_id.to_string();
        req.folder_id = Some(target.id.clone());
        target.requests.push(req.clone());
        stats.added += 1;
        stats.requests.push(req);
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routes(rel: &str, text: &str) -> Vec<(String, String)> {
        scan_file(rel, text)
            .into_iter()
            .map(|h| (h.method, h.path))
            .collect()
    }

    fn pair(m: &str, p: &str) -> (String, String) {
        (m.to_string(), p.to_string())
    }

    #[test]
    fn normalizes_route_parameters() {
        assert_eq!(normalize_route("/api/", "users/:id"), "/api/users/{id}");
        assert_eq!(normalize_route("", "<int:user_id>/"), "/{user_id}");
        assert_eq!(normalize_route("", "{id:int}"), "/{id}");
        assert_eq!(
            normalize_route("", "^items/(?P<pk>[0-9]+)/$"),
            "/items/{pk}"
        );
        assert_eq!(normalize_route("", "[...slug]"), "/{slug}");
        assert_eq!(normalize_route("", ""), "/");
        assert_eq!(
            route_key("get", "/users/:id"),
            route_key("GET", "/users/{userId}")
        );
    }

    #[test]
    fn scans_express_and_skips_http_clients() {
        let src = r#"
const express = require('express');
const router = express.Router();
router.get('/users/:id', show);
app.post("/users", create);
router.route('/items/:id').get(h).put(h)
  .delete(h);
axios.get('/not-a-route');
// app.get('/commented', x);
"#;
        let r = routes("src/routes/users.js", src);
        assert!(r.contains(&pair("GET", "/users/{id}")));
        assert!(r.contains(&pair("POST", "/users")));
        assert!(r.contains(&pair("GET", "/items/{id}")));
        assert!(r.contains(&pair("PUT", "/items/{id}")));
        assert!(r.contains(&pair("DELETE", "/items/{id}")));
        assert!(
            !r.iter()
                .any(|(_, p)| p == "/not-a-route" || p == "/commented")
        );
        // File klien murni tidak dipindai.
        assert!(
            routes(
                "web/api.ts",
                "import axios from 'axios';\nconst x = api.get('/users');"
            )
            .is_empty()
        );
    }

    #[test]
    fn scans_nestjs_controllers() {
        let src = "@Controller('orders')\nexport class OrdersController {\n  @Get(':id')\n  find() {}\n  @Post()\n  create() {}\n}";
        assert_eq!(
            routes("src/orders.controller.ts", src),
            vec![pair("GET", "/orders/{id}"), pair("POST", "/orders")]
        );
    }

    #[test]
    fn scans_file_based_routes() {
        let src = "export async function GET(req) {}\nexport const POST = handler;";
        assert_eq!(
            routes("app/(shop)/api/products/[id]/route.ts", src),
            vec![
                pair("GET", "/api/products/{id}"),
                pair("POST", "/api/products/{id}")
            ]
        );
        assert_eq!(
            routes("pages/api/users/index.ts", ""),
            vec![pair("ANY", "/api/users")]
        );
        assert_eq!(
            routes(
                "src/routes/todos/[id]/+server.ts",
                "export function DELETE() {}"
            ),
            vec![pair("DELETE", "/todos/{id}")]
        );
    }

    #[test]
    fn scans_python_frameworks() {
        let fastapi = "router = APIRouter(prefix=\"/items\")\n@router.get(\"/{item_id}\")\nasync def read(item_id: int): ...\n@app.post('/login')\ndef login(): ...";
        assert_eq!(
            routes("app/items.py", fastapi),
            vec![pair("GET", "/items/{item_id}"), pair("POST", "/login")]
        );
        let flask = "bp = Blueprint('users', __name__, url_prefix='/users')\n@bp.route('/<int:id>', methods=['GET', 'DELETE'])\ndef user(id): ...";
        assert_eq!(
            routes("app/users.py", flask),
            vec![pair("GET", "/users/{id}"), pair("DELETE", "/users/{id}")]
        );
        let django = "urlpatterns = [\n  path('articles/<int:pk>/', views.detail),\n  path('api/', include(router.urls)),\n]\nrouter.register(r'books', BookViewSet)";
        let r = routes("shop/urls.py", django);
        assert!(r.contains(&pair("ANY", "/articles/{pk}")));
        assert!(r.contains(&pair("GET", "/books/{id}")));
        assert!(r.contains(&pair("POST", "/books")));
        assert!(!r.iter().any(|(_, p)| p == "/api"));
    }

    #[test]
    fn scans_laravel_and_symfony() {
        let src = "<?php\nRoute::get('/users/{id}', [UserController::class, 'show']);\nRoute::prefix('admin')->group(function () {\n    Route::post('/reports', [R::class, 'store']);\n});\nRoute::get('/after', fn () => 1);\nRoute::apiResource('photos', PhotoController::class);\n";
        let r = routes("routes/api.php", src);
        assert!(r.contains(&pair("GET", "/api/users/{id}")));
        assert!(r.contains(&pair("POST", "/api/admin/reports")));
        assert!(r.contains(&pair("GET", "/api/after")));
        assert!(r.contains(&pair("DELETE", "/api/photos/{id}")));
        assert!(!r.iter().any(|(_, p)| p == "/api/photos/create"));
        let sym = "#[Route('/blog')]\nclass BlogController\n{\n    #[Route('/{slug}', methods: ['GET'])]\n    public function show() {}\n}";
        assert_eq!(
            routes("src/Controller/BlogController.php", sym),
            vec![pair("GET", "/blog/{slug}")]
        );
    }

    #[test]
    fn scans_rails_routes() {
        let src = "Rails.application.routes.draw do\n  root 'home#index'\n  namespace :api do\n    resources :users, only: [:index, :show]\n    post 'login', to: 'sessions#create'\n  end\n  get '/health', to: 'health#show'\nend\n";
        let r = routes("config/routes.rb", src);
        assert!(r.contains(&pair("GET", "/")));
        assert!(r.contains(&pair("GET", "/api/users")));
        assert!(r.contains(&pair("GET", "/api/users/{id}")));
        assert!(!r.contains(&pair("DELETE", "/api/users/{id}")));
        assert!(r.contains(&pair("POST", "/api/login")));
        assert!(r.contains(&pair("GET", "/health")));
    }

    #[test]
    fn scans_go_routers() {
        let src = "v1 := r.Group(\"/v1\")\nusers := v1.Group(\"/users\")\nusers.GET(\"/:id\", show)\napp.Post(\"/login\", login)\nmux.HandleFunc(\"GET /items/{id}\", h)\nr.HandleFunc(\"/orders\", h).Methods(\"POST\", \"PUT\")\nresp, _ := http.Get(\"/x\")";
        let r = routes("main.go", src);
        assert!(r.contains(&pair("GET", "/v1/users/{id}")));
        assert!(r.contains(&pair("POST", "/login")));
        assert!(r.contains(&pair("GET", "/items/{id}")));
        assert!(r.contains(&pair("POST", "/orders")));
        assert!(r.contains(&pair("PUT", "/orders")));
        assert!(!r.iter().any(|(_, p)| p == "/x"));
    }

    #[test]
    fn scans_rust_frameworks() {
        let src = "let app = Router::new()\n    .route(\"/users/:id\", get(show).put(update))\n    .route(\"/health\", routing::get(health));\n#[post(\"/login\")]\nasync fn login() {}";
        let r = routes("src/main.rs", src);
        assert!(r.contains(&pair("GET", "/users/{id}")));
        assert!(r.contains(&pair("PUT", "/users/{id}")));
        assert!(r.contains(&pair("GET", "/health")));
        assert!(r.contains(&pair("POST", "/login")));
    }

    #[test]
    fn scans_spring_and_jaxrs() {
        let spring = "@RestController\n@RequestMapping(\"/api/users\")\npublic class UserController {\n  @GetMapping(\"/{id}\")\n  public User get() {}\n  @PostMapping\n  public User create() {}\n  @RequestMapping(value = \"/x\", method = RequestMethod.PATCH)\n  public void x() {}\n}";
        assert_eq!(
            routes("src/main/java/UserController.java", spring),
            vec![
                pair("GET", "/api/users/{id}"),
                pair("POST", "/api/users"),
                pair("PATCH", "/api/users/x")
            ]
        );
        let jaxrs = "@Path(\"/books\")\npublic class Books {\n  @GET\n  @Path(\"{id}\")\n  public Book get() {}\n}";
        assert_eq!(
            routes("Books.java", jaxrs),
            vec![pair("GET", "/books/{id}")]
        );
    }

    #[test]
    fn scans_aspnet() {
        let src = "[ApiController]\n[Route(\"api/[controller]\")]\npublic class ProductsController : ControllerBase\n{\n    [HttpGet(\"{id:int}\")]\n    public IActionResult Get(int id) {}\n    [HttpPost]\n    public IActionResult Post() {}\n}\nvar g = app.MapGroup(\"/v2\");\ng.MapDelete(\"/items/{id}\", h);";
        let r = routes("Controllers/ProductsController.cs", src);
        assert!(r.contains(&pair("GET", "/api/products/{id}")));
        assert!(r.contains(&pair("POST", "/api/products")));
        assert!(r.contains(&pair("DELETE", "/v2/items/{id}")));
    }

    #[test]
    fn skips_test_files() {
        assert!(is_test_path("src/__tests__/users.test.ts"));
        assert!(is_test_path("pkg/api/handler_test.go"));
        assert!(!is_test_path("src/routes/users.ts"));
    }

    #[test]
    fn redacts_secret_literals_in_snippets() {
        let code = "const password = \"hunter22\";\napiKey: 'abcd-1234'\nconst name = \"alice\";";
        let out = redact_code_secrets(code);
        assert!(!out.contains("hunter22"));
        assert!(!out.contains("abcd-1234"));
        assert!(out.contains("alice"));
    }

    #[test]
    fn parses_ai_reply_leniently() {
        let reply = r#"Here you go:
```json
{"base_url":"http://localhost:8000","endpoints":[
 {"method":"post","path":"/users/:id/orders","name":"Create order","query_params":{"dry":"true"},
  "path_params":[{"name":"id","example":42,"required":true}],"body_type":"json",
  "body_example":{"qty":2},"status_codes":[201,"400"],"tables":"orders, order_items","source":"a.ts:3"},
 {"method":"GET"}
]}
```"#;
        let (base, eps) = parse_endpoints_reply(reply).expect("parse");
        assert_eq!(base.as_deref(), Some("http://localhost:8000"));
        assert_eq!(eps.len(), 1);
        let ep = &eps[0];
        assert_eq!(ep.method, "POST");
        assert_eq!(ep.path, "/users/{id}/orders");
        assert_eq!(ep.path_params[0].example, "42");
        assert_eq!(ep.query_params[0].name, "dry");
        assert_eq!(ep.status_codes, vec!["201", "400"]);
        assert_eq!(ep.tables, vec!["orders", "order_items"]);
        assert!(ep.body_example.contains("\"qty\": 2"));
        assert!(parse_endpoints_reply("no json here").is_err());
    }

    #[test]
    fn filters_tables_against_diagram() {
        let cands = vec!["public.users".to_string(), "orders".to_string()];
        assert_eq!(
            filter_tables(
                &[
                    "USERS".into(),
                    "orders".into(),
                    "unknown".into(),
                    "orders".into()
                ],
                &cands
            ),
            vec!["public.users", "orders"]
        );
        assert_eq!(filter_tables(&["x".into()], &[]), vec!["x"]);
    }

    #[test]
    fn batches_group_by_file_and_respect_size() {
        let mk = |file: &str, n: usize| -> Vec<GeneratedEndpoint> {
            (0..n)
                .map(|i| GeneratedEndpoint {
                    method: "GET".into(),
                    path: format!("/{file}/{i}"),
                    source: format!("{file}.ts:{i}"),
                    ..Default::default()
                })
                .collect()
        };
        let mut all = mk("a", 15);
        all.extend(mk("b", 10));
        all.extend(mk("c", 45));
        let b = batches(&all);
        assert!(b.iter().all(|x| x.len() <= BATCH_SIZE));
        assert_eq!(b.iter().map(Vec::len).sum::<usize>(), 70);
        // File `a` tidak terpecah ke dua batch.
        assert_eq!(b[0].len(), 15);
    }

    #[test]
    fn converts_and_adds_endpoints_to_folder() {
        let ep = GeneratedEndpoint {
            method: "POST".into(),
            path: "/api/users/{id}/orders".into(),
            name: "Create order".into(),
            path_params: vec![ParamSpec {
                name: "id".into(),
                example: "42".into(),
                required: true,
                description: String::new(),
            }],
            query_params: vec![ParamSpec {
                name: "dry".into(),
                example: "1".into(),
                ..Default::default()
            }],
            auth: "api_key".into(),
            body_type: "json".into(),
            body_example: "{\"qty\": 2}".into(),
            tables: vec!["orders".into()],
            source: "a.ts:3".into(),
            from_ai: true,
            ..Default::default()
        };
        let req = to_saved_request(&ep, "http://localhost:3000/");
        assert_eq!(req.url, "http://localhost:3000/api/users/42/orders");
        assert_eq!(req.method, HttpMethod::POST);
        assert_eq!(req.body_type, HttpBodyType::Json);
        assert_eq!(req.auth_type, HttpAuthType::ApiKey);
        assert_eq!(req.api_key_name, "X-API-Key");
        assert_eq!(req.params, vec![("dry".into(), "1".into(), false)]);
        assert_eq!(req.route.as_deref(), Some("/api/users/{id}/orders"));
        assert_eq!(group_name(&ep), "users");

        let mut wss = vec![HttpWorkspace {
            id: "ws".into(),
            name: "W".into(),
            folders: vec![HttpFolder {
                id: "f".into(),
                name: "Shop".into(),
                ..Default::default()
            }],
            ..Default::default()
        }];
        let stats = add_endpoints_to_folder(
            &mut wss,
            "ws",
            "f",
            std::slice::from_ref(&ep),
            "http://h",
            true,
        )
        .expect("add");
        assert_eq!((stats.added, stats.updated), (1, 0));
        let folder = &wss[0].folders[0];
        assert_eq!(folder.children[0].name, "users");
        assert_eq!(
            folder.children[0].requests[0].folder_id.as_deref(),
            Some(folder.children[0].id.as_str())
        );

        // Endpoint yang sama diperbarui, rahasia user dipertahankan.
        wss[0].folders[0].children[0].requests[0].api_key_value = "secret".into();
        let stats =
            add_endpoints_to_folder(&mut wss, "ws", "f", &[ep], "http://h", true).expect("update");
        assert_eq!((stats.added, stats.updated), (0, 1));
        let r = &wss[0].folders[0].children[0].requests;
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].api_key_value, "secret");
        assert!(add_endpoints_to_folder(&mut wss, "ws", "nope", &[], "", false).is_err());
    }

    #[test]
    fn default_base_url_follows_framework() {
        let hit = |f: &'static str| RouteHit {
            method: "GET".into(),
            path: "/".into(),
            file: "x".into(),
            line: 1,
            framework: f,
        };
        assert_eq!(default_base_url(&[hit("fastapi")]), "http://localhost:8000");
        assert_eq!(
            default_base_url(&[hit("go"), hit("go"), hit("express")]),
            "http://localhost:8080"
        );
        assert_eq!(default_base_url(&[]), "http://localhost:3000");
    }

    #[test]
    fn pool_runs_every_item_concurrently_in_order() {
        let cancel = AtomicBool::new(false);
        let live = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let out = run_pool(9, 3, &cancel, |i| {
            let now = live.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(30));
            live.fetch_sub(1, Ordering::SeqCst);
            i * 10
        });
        assert_eq!(
            out.iter().map(|(i, v)| (*i, *v)).collect::<Vec<_>>(),
            (0..9).map(|i| (i, i * 10)).collect::<Vec<_>>()
        );
        let peak = peak.load(Ordering::SeqCst);
        assert!(peak > 1 && peak <= 3, "peak {peak}");
    }

    #[test]
    fn pool_stops_taking_work_after_cancel() {
        let cancel = AtomicBool::new(false);
        let out = run_pool(50, 2, &cancel, |i| {
            if i == 3 {
                cancel.store(true, Ordering::SeqCst);
            }
            i
        });
        assert!(out.len() < 50);
        assert!(run_pool(0, 4, &AtomicBool::new(false), |i| i).is_empty());
    }

    #[test]
    fn ai_slots_limit_wait_and_cancel() {
        let base = ai_slots().active.lock().map(|a| *a).unwrap_or(0);
        let limit = base + 1;
        let never = AtomicBool::new(false);
        let held = acquire_ai_slot(limit, &never).expect("free slot");
        // Slot penuh: yang dibatalkan langsung menyerah.
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            acquire_ai_slot(limit, &cancelled),
            Err(RepoScanError::Cancelled)
        ));
        // Yang menunggu mendapat slot setelah slot lama dilepas.
        let waiter = std::thread::spawn(move || {
            let flag = AtomicBool::new(false);
            acquire_ai_slot(limit, &flag).is_ok()
        });
        std::thread::sleep(Duration::from_millis(100));
        drop(held);
        assert!(waiter.join().expect("join"));
    }

    #[test]
    fn test_enrich_flexurio_endpoints() {
        let temp_dir = std::env::temp_dir().join(format!(
            "flx_ep_test_{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let cfg_dir = temp_dir.join("config");
        let ent_dir = cfg_dir.join("entity");
        std::fs::create_dir_all(&ent_dir).unwrap();

        let routes_file = cfg_dir.join("routes.json");
        std::fs::write(&routes_file, r#"{"routes": ["customers"]}"#).unwrap();

        let customer_ent = ent_dir.join("customers.json");
        std::fs::write(
            &customer_ent,
            r#"{
                "table": "m_customers",
                "primary_key": { "columns": ["id"] },
                "columns": [
                    { "name": "id", "type_data": "int", "auto_increment": true },
                    { "name": "name", "type_data": "varchar(100)" }
                ],
                "details": [
                    { "field": "contacts", "target_table": "m_customer_contacts", "columns": ["phone"] }
                ]
            }"#,
        )
        .unwrap();

        let mut endpoints = vec![
            GeneratedEndpoint {
                method: "GET".to_string(),
                path: "/customers".to_string(),
                framework: "flexurio".to_string(),
                ..Default::default()
            },
            GeneratedEndpoint {
                method: "POST".to_string(),
                path: "/customers".to_string(),
                framework: "flexurio".to_string(),
                ..Default::default()
            },
        ];

        enrich_flexurio_endpoints(&temp_dir, &mut endpoints);

        // GET endpoint
        assert_eq!(endpoints[0].group, "customers");
        assert_eq!(
            endpoints[0].tables,
            vec!["m_customers", "m_customer_contacts"]
        );
        assert_eq!(endpoints[0].auth, "bearer");
        assert!(!endpoints[0].query_params.is_empty());
        assert_eq!(endpoints[0].name, "Get customers List");

        // POST endpoint
        assert_eq!(endpoints[1].group, "customers");
        assert_eq!(endpoints[1].body_type, "json");
        assert!(
            endpoints[1].body_example.contains("m_customers")
                || endpoints[1].body_example.contains("name")
        );
        assert_eq!(endpoints[1].name, "Create customers");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
