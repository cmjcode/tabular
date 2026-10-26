//! Tab File Viewer: membuka gambar, SVG, Markdown, CSV/XLSX, PDF, source code,
//! dan file biner di tab tersendiri. Logika headless ada di `crate::file_viewer`,
//! job latar di `file_viewer_jobs`.
//!
//! File `.sql` tetap dibuka di editor query biasa agar bisa dieksekusi.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui;
use egui_icons::icons;

use super::Tabular;
use super::file_viewer_jobs::{self as jobs, JobInbox, JobOutput};
use super::style;
use crate::file_viewer::decode::{ImageInfo, PdfInfo, RgbaImage};
use crate::file_viewer::{self, CodeLang, FileKind, TextFile};
use crate::http_client_widgets::{self as widgets, Syntax};

/// Di atas ukuran ini teks ditampilkan tanpa warna agar mengetik tetap ringan.
const CODE_HIGHLIGHT_MAX_BYTES: usize = 1024 * 1024;
const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 32.0;
const ZOOM_STEP: f32 = 1.25;
const PDF_PAGE_GAP: f32 = 12.0;
const MAX_PENDING_PDF_PAGES: usize = 2;
/// Jumlah tekstur halaman PDF yang disimpan sebelum yang jauh dibuang.
const PDF_TEXTURE_BUDGET: usize = 12;

// ─── State tab ───────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct FileViewerTabState {
    pub path: PathBuf,
    pub kind: FileKind,
    pub size: u64,
    pub error: Option<String>,
    pub body: Body,
    inbox: JobInbox,
    /// Pemuatan dimulai saat tab pertama kali dirender (butuh `egui::Context`).
    started: bool,
}

impl std::fmt::Debug for FileViewerTabState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileViewerTabState")
            .field("path", &self.path)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub enum Body {
    Loading,
    Code(TextView),
    Markdown(MarkdownView),
    Image(ImageView),
    Svg(SvgView),
    Pdf(PdfView),
    Table(TableView),
    Binary { dump: String },
}

/// Teks yang bisa diedit (source code, konfigurasi, sumber Markdown).
#[derive(Clone)]
pub struct TextView {
    pub text: String,
    saved_hash: u64,
    pub modified: bool,
    pub lang: CodeLang,
    pub crlf: bool,
    bom: bool,
    pub editable: bool,
    /// Alasan teks baca-saja (terlalu besar / bukan UTF-8).
    pub note: Option<String>,
    /// File yang lazim berisi kredensial (`.env`, kunci privat).
    pub sensitive: bool,
    pub wrap: bool,
    pub find: String,
    find_index: usize,
    scroll_to_char: Option<usize>,
    editor_id: Option<egui::Id>,
}

#[derive(Clone)]
pub struct MarkdownView {
    pub text: TextView,
    pub preview: bool,
    cache: Arc<Mutex<egui_commonmark::CommonMarkCache>>,
}

#[derive(Clone)]
pub struct ImageView {
    rgba: Arc<RgbaImage>,
    pub info: ImageInfo,
    tex: egui::TextureHandle,
    pub zoom: f32,
    pub fit: bool,
    hover: Option<(u32, u32, [u8; 4])>,
}

#[derive(Clone)]
pub struct SvgView {
    bytes: Arc<Vec<u8>>,
    /// Ukuran intrinsik (piksel CSS).
    pub size: (f32, f32),
    tex: egui::TextureHandle,
    tex_scale: f32,
    pub zoom: f32,
    pub fit: bool,
    requested_scale: Option<f32>,
    zoom_changed_at: f64,
}

#[derive(Clone)]
pub struct PdfView {
    bytes: Arc<Vec<u8>>,
    pub info: PdfInfo,
    pub zoom: f32,
    pub fit_width: bool,
    pages: HashMap<usize, PdfPageTex>,
    pending: HashSet<usize>,
    failed: HashMap<usize, String>,
    pub current: usize,
    jump: Option<usize>,
    page_input: String,
}

#[derive(Clone)]
struct PdfPageTex {
    tex: egui::TextureHandle,
    scale: f32,
}

#[derive(Clone, Default)]
pub struct TableView {
    pub meta: Option<TableMeta>,
    pub loading: bool,
}

#[derive(Clone, Debug)]
pub struct TableMeta {
    pub kind: &'static str,
    pub sheets: Vec<String>,
    pub sheet: Option<String>,
    pub rows: usize,
    pub cols: usize,
    pub truncated: bool,
    pub all_rows: bool,
    pub encoding: Option<&'static str>,
    pub delimiter: Option<u8>,
    pub compression: Option<&'static str>,
}

fn text_hash(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

impl TextView {
    fn from_file(tf: TextFile, lang: CodeLang, path: &Path) -> Self {
        let note = if tf.truncated {
            Some(format!(
                "This file is {}; only the first {} is shown, read-only.",
                file_viewer::human_size(tf.size),
                file_viewer::human_size(file_viewer::MAX_EDITABLE_TEXT_BYTES)
            ))
        } else if tf.lossy {
            Some("This file is not valid UTF-8 text, so it is opened read-only.".to_string())
        } else {
            None
        };
        let editable = tf.editable();
        Self {
            saved_hash: text_hash(&tf.text),
            text: tf.text,
            modified: false,
            lang,
            crlf: tf.crlf,
            bom: tf.bom,
            editable,
            note,
            sensitive: file_viewer::is_sensitive_file(path),
            wrap: matches!(lang, CodeLang::Markdown | CodeLang::PlainText),
            find: String::new(),
            find_index: 0,
            scroll_to_char: None,
            editor_id: None,
        }
    }

    fn mark_saved(&mut self) {
        self.saved_hash = text_hash(&self.text);
        self.modified = false;
    }
}

impl FileViewerTabState {
    pub fn new(path: PathBuf, kind: FileKind, size: u64) -> Self {
        Self {
            path,
            kind,
            size,
            error: None,
            body: Body::Loading,
            inbox: JobInbox::default(),
            started: false,
        }
    }

    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// Ada perubahan teks yang belum disimpan.
    pub fn is_modified(&self) -> bool {
        match &self.body {
            Body::Code(v) => v.modified,
            Body::Markdown(m) => m.text.modified,
            _ => false,
        }
    }

    fn text_view_mut(&mut self) -> Option<&mut TextView> {
        match &mut self.body {
            Body::Code(v) => Some(v),
            Body::Markdown(m) => Some(&mut m.text),
            _ => None,
        }
    }

    /// Mulai (atau ulangi) pemuatan isi file.
    fn start_load(&mut self, ctx: &egui::Context) {
        self.started = true;
        self.error = None;
        self.body = Body::Loading;
        if let Ok(meta) = std::fs::metadata(&self.path) {
            self.size = meta.len();
        }
        let path = self.path.clone();
        let max_side = ctx.input(|i| i.max_texture_side) as u32;
        match self.kind {
            FileKind::Code(_) | FileKind::Markdown => jobs::load_text(&self.inbox, path),
            FileKind::Image => jobs::load_image(&self.inbox, path, max_side),
            FileKind::Svg => {
                let scale = (2.0 * ctx.pixels_per_point()).min(4.0);
                jobs::render_svg(&self.inbox, path, None, scale);
            }
            FileKind::Pdf => jobs::load_pdf(&self.inbox, path),
            FileKind::Table => {
                self.body = Body::Table(TableView {
                    meta: None,
                    loading: true,
                });
                jobs::load_table(&self.inbox, path, None, false);
            }
            FileKind::Binary => jobs::load_binary(&self.inbox, path),
        }
    }

    /// Simpan teks ke disk. `Ok(false)` bila tidak ada yang perlu disimpan.
    pub fn save(&mut self) -> Result<bool, String> {
        let path = self.path.clone();
        let Some(view) = self.text_view_mut() else {
            return Ok(false);
        };
        if !view.editable {
            return Err(view
                .note
                .clone()
                .unwrap_or_else(|| "This file is read-only".to_string()));
        }
        let data = TextFile::to_disk(&view.text, view.crlf, view.bom);
        std::fs::write(&path, data).map_err(|e| format!("Cannot save {}: {e}", path.display()))?;
        view.mark_saved();
        log::info!("[FILE_VIEWER] saved {}", path.display());
        Ok(true)
    }
}

// ─── Membuka file ────────────────────────────────────────────────────────────

/// Buka `path` di tab File Viewer (atau aktifkan tab yang sudah membukanya).
/// File `.sql` dibuka di editor query.
pub fn open_path(t: &mut Tabular, path: &Path) {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if path.is_dir() {
        t.toasts
            .warning(format!("{} is a folder, not a file", path.display()));
        return;
    }
    let is_sql = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("sql"));
    if is_sql {
        if let Err(e) = crate::sidebar_query::open_query_file(t, &path.to_string_lossy()) {
            t.toasts.error(e);
        }
        return;
    }
    if let Some(idx) = t.query_tabs.iter().position(|tab| {
        tab.file_viewer_state
            .as_ref()
            .is_some_and(|s| s.path == path)
    }) {
        crate::editor::switch_to_tab(t, idx);
        return;
    }
    let head = match file_viewer::read_head(&path, 8192) {
        Ok(h) => h,
        Err(e) => {
            t.toasts.error(e.to_string());
            return;
        }
    };
    let kind = file_viewer::classify(&path, &head);
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let st = FileViewerTabState::new(path, kind, size);
    log::debug!("[FILE_VIEWER] open {} as {:?}", st.path.display(), kind);
    crate::editor::create_new_tab(t, st.file_name(), String::new());
    if let Some(tab) = t.query_tabs.get_mut(t.active_tab_index) {
        tab.file_viewer_state = Some(st);
    }
}

/// Buka beberapa file sekaligus (drag and drop, dialog multi-pilih).
pub fn open_paths(t: &mut Tabular, paths: &[PathBuf]) {
    for p in paths {
        open_path(t, p);
    }
}

/// Dialog "Open File..." untuk semua jenis file yang didukung.
pub fn open_dialog(t: &mut Tabular) {
    let Some(paths) = crate::rfd::FileDialog::new()
        .set_title("Open File")
        .pick_files()
    else {
        return;
    };
    open_paths(t, &paths);
}

/// Simpan tab File Viewer yang aktif (Cmd+S). `Err` berisi pesan untuk user.
pub fn save_active(t: &mut Tabular) -> Result<(), String> {
    let idx = t.active_tab_index;
    let Some(tab) = t.query_tabs.get_mut(idx) else {
        return Err("No active tab".to_string());
    };
    let Some(st) = tab.file_viewer_state.as_mut() else {
        return Err("The active tab is not a file".to_string());
    };
    let name = st.file_name();
    match st.save() {
        Ok(true) => {
            tab.is_modified = false;
            t.toasts.success(format!("Saved {name}"));
            Ok(())
        }
        Ok(false) => Ok(()),
        Err(e) => Err(e),
    }
}

// ─── Render ──────────────────────────────────────────────────────────────────

/// Render tab File Viewer yang aktif.
pub fn render_active_tab(t: &mut Tabular, ui: &mut egui::Ui) {
    let idx = t.active_tab_index;
    let Some(tab) = t.query_tabs.get_mut(idx) else {
        return;
    };
    let tab_id = tab.id;
    let Some(mut st) = tab.file_viewer_state.take() else {
        return;
    };

    let ctx = ui.ctx().clone();
    if !st.started {
        st.start_load(&ctx);
    }
    poll(t, &mut st, &ctx);

    let mut reload = false;
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            reload = render_header(t, &mut st, ui);
            ui.add_space(4.0);
            if let Some(err) = st.error.clone() {
                render_error(ui, &st, &err);
            } else {
                render_body(t, &mut st, ui);
            }
        });
    if reload {
        st.start_load(&ctx);
    }
    if st.inbox.pending() > 0 {
        ctx.request_repaint_after(std::time::Duration::from_millis(60));
    }

    let modified = st.is_modified();
    if let Some(tab) = t.query_tabs.iter_mut().find(|x| x.id == tab_id) {
        tab.is_modified = modified;
        tab.file_viewer_state = Some(st);
    }
}

/// Ambil hasil job yang selesai dan terapkan ke state.
fn poll(t: &mut Tabular, st: &mut FileViewerTabState, ctx: &egui::Context) {
    for out in st.inbox.drain() {
        match out {
            JobOutput::Text(Ok(tf)) => {
                st.body = match st.kind {
                    FileKind::Markdown => Body::Markdown(MarkdownView {
                        text: TextView::from_file(tf, CodeLang::Markdown, &st.path),
                        preview: true,
                        cache: Arc::default(),
                    }),
                    FileKind::Code(lang) => Body::Code(TextView::from_file(tf, lang, &st.path)),
                    _ => Body::Code(TextView::from_file(tf, CodeLang::PlainText, &st.path)),
                };
            }
            JobOutput::Image(Ok((rgba, info))) => {
                let tex = upload(ctx, &st.path, &rgba, image_texture_options());
                st.body = Body::Image(ImageView {
                    rgba,
                    info,
                    tex,
                    zoom: 1.0,
                    fit: true,
                    hover: None,
                });
            }
            JobOutput::Svg { scale, result } => match result {
                Ok(r) => {
                    let tex = upload(ctx, &st.path, &r.image, egui::TextureOptions::LINEAR);
                    match &mut st.body {
                        Body::Svg(v) => {
                            v.tex = tex;
                            v.tex_scale = scale;
                            v.requested_scale = None;
                        }
                        _ => {
                            st.body = Body::Svg(SvgView {
                                bytes: r.bytes,
                                size: r.size,
                                tex,
                                tex_scale: scale,
                                zoom: 1.0,
                                fit: true,
                                requested_scale: None,
                                zoom_changed_at: 0.0,
                            });
                        }
                    }
                }
                Err(e) => match &mut st.body {
                    Body::Svg(v) => {
                        v.requested_scale = None;
                        log::warn!("[FILE_VIEWER] SVG re-render failed: {e}");
                    }
                    _ => st.error = Some(e),
                },
            },
            JobOutput::PdfInfo(Ok((bytes, info))) => {
                if info.pages.is_empty() {
                    st.error = Some("The PDF has no pages".to_string());
                } else {
                    st.body = Body::Pdf(PdfView {
                        bytes,
                        info,
                        zoom: 1.0,
                        fit_width: true,
                        pages: HashMap::new(),
                        pending: HashSet::new(),
                        failed: HashMap::new(),
                        current: 0,
                        jump: None,
                        page_input: "1".to_string(),
                    });
                }
            }
            JobOutput::PdfPage {
                index,
                scale,
                result,
            } => {
                if let Body::Pdf(v) = &mut st.body {
                    v.pending.remove(&index);
                    match result {
                        Ok(img) => {
                            let tex = upload(ctx, &st.path, &img, egui::TextureOptions::LINEAR);
                            v.pages.insert(index, PdfPageTex { tex, scale });
                        }
                        Err(e) => {
                            v.failed.insert(index, e);
                        }
                    }
                }
            }
            JobOutput::Table { all_rows, result } => match result {
                Ok(loaded) => {
                    let meta = TableMeta {
                        kind: loaded.kind.label(),
                        sheets: loaded.sheets.clone(),
                        sheet: loaded.sheet.clone(),
                        rows: loaded.data.rows.len(),
                        cols: loaded.data.headers.len(),
                        truncated: loaded.truncated,
                        all_rows,
                        encoding: loaded.encoding.map(|e| e.label()),
                        delimiter: loaded.delimiter,
                        compression: loaded.compression,
                    };
                    apply_table_to_grid(t, &st.file_name(), loaded);
                    st.body = Body::Table(TableView {
                        meta: Some(meta),
                        loading: false,
                    });
                }
                Err(e) => st.error = Some(e),
            },
            JobOutput::Binary(Ok(head)) => {
                st.body = Body::Binary {
                    dump: file_viewer::hex_dump(&head),
                };
            }
            JobOutput::Text(Err(e))
            | JobOutput::Image(Err(e))
            | JobOutput::PdfInfo(Err(e))
            | JobOutput::Binary(Err(e)) => st.error = Some(e),
        }
    }
}

/// Isi grid hasil (state global tab aktif) dengan data file.
fn apply_table_to_grid(
    t: &mut Tabular,
    name: &str,
    loaded: crate::data_transfer::readers::LoadedFile,
) {
    let crate::data_transfer::TableData { headers, rows, .. } = loaded.data;
    t.current_table_headers = headers;
    t.current_table_name = format!("File: {name}");
    t.is_table_browse_mode = false;
    t.use_server_pagination = false;
    t.current_base_query.clear();
    t.actual_total_rows = None;
    t.current_object_ddl = None;
    crate::data_table::update_pagination_data(t, rows);
    if let Some(tab) = t.query_tabs.get_mut(t.active_tab_index) {
        tab.has_executed_query = true;
    }
}

fn image_texture_options() -> egui::TextureOptions {
    egui::TextureOptions {
        magnification: egui::TextureFilter::Nearest,
        minification: egui::TextureFilter::Linear,
        ..Default::default()
    }
}

fn upload(
    ctx: &egui::Context,
    path: &Path,
    img: &RgbaImage,
    options: egui::TextureOptions,
) -> egui::TextureHandle {
    let color = egui::ColorImage::from_rgba_unmultiplied(
        [img.width as usize, img.height as usize],
        &img.pixels,
    );
    ctx.load_texture(format!("file_viewer:{}", path.display()), color, options)
}

pub(crate) fn kind_icon(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Image | FileKind::Svg => icons::ICON_IMAGE.codepoint,
        FileKind::Markdown => icons::ICON_ARTICLE.codepoint,
        FileKind::Table => icons::ICON_TABLE_CHART.codepoint,
        FileKind::Pdf => icons::ICON_PICTURE_AS_PDF.codepoint,
        FileKind::Code(CodeLang::PlainText) => icons::ICON_DESCRIPTION.codepoint,
        FileKind::Code(_) => icons::ICON_CODE.codepoint,
        FileKind::Binary => icons::ICON_INSERT_DRIVE_FILE.codepoint,
    }
}

/// Baris judul: ikon jenis, nama, path, ukuran, dan aksi file. `true` = reload.
fn render_header(t: &mut Tabular, st: &mut FileViewerTabState, ui: &mut egui::Ui) -> bool {
    let ctx = ui.ctx().clone();
    let muted = style::nav_text_muted(&ctx);
    let mut reload = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(kind_icon(st.kind))
                .size(16.0)
                .color(muted),
        );
        ui.label(egui::RichText::new(st.file_name()).strong().size(14.0));
        let mut details = format!(
            "{}  ·  {}",
            st.kind.label(),
            file_viewer::human_size(st.size)
        );
        if st.is_modified() {
            details.push_str("  ·  Modified");
        }
        ui.label(egui::RichText::new(details).size(12.0).color(muted));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            {
                if style::ai_icon_button(ui, icons::ICON_OPEN_IN_NEW.codepoint, "")
                    .on_hover_text("Open with the default app")
                    .clicked()
                    && let Err(e) = crate::url_opener::open_url(&st.path.to_string_lossy())
                {
                    t.toasts.error(format!("Cannot open the file: {e}"));
                }
                if style::ai_icon_button(ui, icons::ICON_FOLDER_OPEN.codepoint, "")
                    .on_hover_text("Show in folder")
                    .clicked()
                    && let Some(dir) = st.path.parent()
                    && let Err(e) = crate::url_opener::open_folder(dir)
                {
                    t.toasts.error(e);
                }
            }
            let reload_tip = if st.is_modified() {
                "Reload from disk (discards unsaved changes)"
            } else {
                "Reload from disk"
            };
            if style::ai_icon_button(ui, icons::ICON_REFRESH.codepoint, "")
                .on_hover_text(reload_tip)
                .clicked()
            {
                reload = true;
            }
            if st.text_view_mut().is_some_and(|v| v.editable) {
                let modified = st.is_modified();
                let save = ui
                    .add_enabled(
                        modified,
                        egui::Button::new(
                            egui::RichText::new(format!("{}  Save", icons::ICON_SAVE.codepoint))
                                .size(12.5),
                        ),
                    )
                    .on_hover_text(format!("Save ({})", crate::keymap::fixed_label("Cmd+S")));
                if save.clicked() {
                    match st.save() {
                        Ok(_) => t.toasts.success(format!("Saved {}", st.file_name())),
                        Err(e) => t.toasts.error(e),
                    }
                }
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(st.path.display().to_string())
                        .size(11.5)
                        .color(muted),
                )
                .truncate(),
            )
            .on_hover_text(st.path.display().to_string());
        });
    });
    reload
}

fn render_error(ui: &mut egui::Ui, st: &FileViewerTabState, err: &str) {
    ui.add_space(40.0);
    widgets::empty_state(
        ui,
        icons::ICON_WARNING.codepoint,
        &format!("Cannot display {}", st.file_name()),
        err,
    );
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    ui.vertical_centered(|ui| {
        ui.add_space(10.0);
        if ui.button("Open with the default app").clicked() {
            let _ = crate::url_opener::open_url(&st.path.to_string_lossy());
        }
    });
}

fn render_body(t: &mut Tabular, st: &mut FileViewerTabState, ui: &mut egui::Ui) {
    let salt = st.path.display().to_string();
    match &mut st.body {
        Body::Loading => {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.spinner();
                ui.label(format!("Loading {}…", st.kind.label()));
            });
        }
        Body::Code(view) => render_text(ui, view, &salt),
        Body::Markdown(md) => render_markdown(ui, md, &st.path, &salt),
        Body::Image(view) => render_image(ui, view, &salt),
        Body::Svg(view) => render_svg(ui, view, &st.inbox, &st.path, &salt),
        Body::Pdf(view) => render_pdf(ui, view, &st.inbox, &salt),
        Body::Table(view) => {
            let path = st.path.clone();
            render_table(t, ui, view, &st.inbox, &path);
        }
        Body::Binary { dump } => render_binary(ui, dump, st.size, &salt),
    }
}

// ─── Teks / kode ─────────────────────────────────────────────────────────────

fn syntax_for(view: &TextView) -> Syntax {
    if view.text.len() > CODE_HIGHLIGHT_MAX_BYTES {
        Syntax::Plain
    } else {
        Syntax::Code(view.lang)
    }
}

/// Toolbar teks: bahasa, bungkus baris, pencarian.
fn text_toolbar(ui: &mut egui::Ui, view: &mut TextView) {
    let ctx = ui.ctx().clone();
    let muted = style::nav_text_muted(&ctx);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(view.lang.label())
                .size(12.0)
                .color(muted),
        );
        if view.crlf {
            ui.label(egui::RichText::new("CRLF").size(11.0).color(muted));
        }
        if view.sensitive {
            ui.label(
                egui::RichText::new(format!("{}  Sensitive", icons::ICON_LOCK.codepoint))
                    .size(11.5)
                    .color(egui::Color32::from_rgb(220, 150, 40)),
            )
            .on_hover_text(
                "This file usually holds credentials. Be careful when sharing screenshots or copying its content.",
            );
        }
        if !view.editable {
            ui.label(
                egui::RichText::new("Read-only")
                    .size(11.5)
                    .color(egui::Color32::from_rgb(220, 150, 40)),
            );
        }
        if widgets::icon_toggle(
            ui,
            icons::ICON_WRAP_TEXT.codepoint,
            "Wrap long lines",
            view.wrap,
        )
        .clicked()
        {
            view.wrap = !view.wrap;
        }
        ui.add_space(8.0);
        ui.label(egui::RichText::new(icons::ICON_SEARCH.codepoint).color(muted));
        let find = ui.add(
            egui::TextEdit::singleline(&mut view.find)
                .hint_text("Find")
                .desired_width(180.0),
        );
        if find.changed() {
            view.find_index = 0;
        }
        let matches = widgets::match_ranges(&view.text, &view.find);
        let mut step: Option<bool> = None;
        if find.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            step = Some(!ui.input(|i| i.modifiers.shift));
            find.request_focus();
        }
        if !view.find.is_empty() {
            let label = if matches.is_empty() {
                "No results".to_string()
            } else {
                format!(
                    "{} of {}",
                    view.find_index.min(matches.len() - 1) + 1,
                    matches.len()
                )
            };
            ui.label(egui::RichText::new(label).size(11.5).color(muted));
            if style::ai_icon_button(ui, icons::ICON_KEYBOARD_ARROW_UP.codepoint, "")
                .on_hover_text("Previous match (Shift+Enter)")
                .clicked()
            {
                step = Some(false);
            }
            if style::ai_icon_button(ui, icons::ICON_KEYBOARD_ARROW_DOWN.codepoint, "")
                .on_hover_text("Next match (Enter)")
                .clicked()
            {
                step = Some(true);
            }
        }
        if let Some(forward) = step
            && !matches.is_empty()
        {
            let n = matches.len();
            view.find_index = if forward {
                (view.find_index + 1) % n
            } else {
                (view.find_index + n - 1) % n
            };
            select_match(ui.ctx(), view, &matches);
        }
    });
}

/// Pilih hasil pencarian ke-`find_index` di editor dan gulir ke sana.
fn select_match(ctx: &egui::Context, view: &mut TextView, matches: &[std::ops::Range<usize>]) {
    let Some(range) = matches.get(view.find_index) else {
        return;
    };
    let start = view.text[..range.start].chars().count();
    let end = start + view.text[range.clone()].chars().count();
    view.scroll_to_char = Some(start);
    if let Some(id) = view.editor_id {
        let mut state = egui::text_edit::TextEditState::load(ctx, id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(start),
                egui::text::CCursor::new(end),
            )));
        state.store(ctx, id);
    }
}

fn render_text(ui: &mut egui::Ui, view: &mut TextView, salt: &str) {
    text_toolbar(ui, view);
    if let Some(note) = &view.note {
        ui.label(
            egui::RichText::new(note)
                .size(11.5)
                .color(egui::Color32::from_rgb(220, 150, 40)),
        );
    }
    ui.add_space(4.0);
    let syntax = syntax_for(view);
    let opts = widgets::CodeView {
        id_salt: salt,
        syntax,
        wrap: view.wrap,
        hint: "",
        search: &view.find,
    };
    let scroll = view.scroll_to_char.take();
    let resp = if view.editable {
        let search = view.find.clone();
        let opts = widgets::CodeView {
            search: &search,
            ..opts
        };
        widgets::render_code_view_scrolled(ui, &mut view.text, opts, scroll)
    } else {
        let mut ro: &str = view.text.as_str();
        widgets::render_code_view_scrolled(ui, &mut ro, opts, scroll)
    };
    view.editor_id = Some(resp.id);
    if resp.changed() {
        view.modified = text_hash(&view.text) != view.saved_hash;
    }
}

// ─── Markdown ────────────────────────────────────────────────────────────────

fn render_markdown(ui: &mut egui::Ui, md: &mut MarkdownView, path: &Path, salt: &str) {
    ui.horizontal(|ui| {
        if widgets::icon_toggle(ui, icons::ICON_VISIBILITY.codepoint, "Preview", md.preview)
            .clicked()
        {
            md.preview = true;
        }
        if widgets::icon_toggle(ui, icons::ICON_EDIT.codepoint, "Edit source", !md.preview)
            .clicked()
        {
            md.preview = false;
        }
    });
    if !md.preview {
        render_text(ui, &mut md.text, salt);
        return;
    }
    // Gambar relatif di Markdown dimuat lewat image loader egui_extras (file://).
    egui_extras::install_image_loaders(ui.ctx());
    let base = path
        .parent()
        .map(|d| format!("file://{}/", d.display()))
        .unwrap_or_default();
    egui::ScrollArea::vertical()
        .id_salt(("fv_md", salt))
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(900.0));
            // `override_font_id` global memaksa satu ukuran Proportional; matikan
            // di area pratinjau supaya heading dan blok kode memakai gaya Markdown.
            ui.style_mut().override_font_id = None;
            let mut cache = md.cache.lock().unwrap_or_else(|e| e.into_inner());
            egui_commonmark::CommonMarkViewer::new()
                .default_implicit_uri_scheme(base)
                .show(ui, &mut cache, &md.text.text);
        });
}

// ─── Gambar & SVG ────────────────────────────────────────────────────────────

fn zoom_toolbar(ui: &mut egui::Ui, zoom: &mut f32, fit: &mut bool) -> bool {
    let mut changed = false;
    if style::ai_icon_button(ui, icons::ICON_ZOOM_OUT.codepoint, "")
        .on_hover_text("Zoom out")
        .clicked()
    {
        *zoom = (*zoom / ZOOM_STEP).max(MIN_ZOOM);
        *fit = false;
        changed = true;
    }
    ui.label(egui::RichText::new(format!("{:.0}%", *zoom * 100.0)).size(12.0));
    if style::ai_icon_button(ui, icons::ICON_ZOOM_IN.codepoint, "")
        .on_hover_text("Zoom in")
        .clicked()
    {
        *zoom = (*zoom * ZOOM_STEP).min(MAX_ZOOM);
        *fit = false;
        changed = true;
    }
    if widgets::icon_toggle(ui, icons::ICON_FIT_SCREEN.codepoint, "Fit to window", *fit).clicked() {
        *fit = !*fit;
        changed = true;
    }
    if ui
        .small_button("1:1")
        .on_hover_text("Actual size")
        .clicked()
    {
        *zoom = 1.0;
        *fit = false;
        changed = true;
    }
    changed
}

/// Tekstur papan catur kecil (berulang) untuk latar gambar transparan.
fn checker_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("file_viewer_checker");
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return tex;
    }
    let (a, b) = if ctx.global_style().visuals.dark_mode {
        ([58, 58, 62, 255], [44, 44, 48, 255])
    } else {
        ([255, 255, 255, 255], [222, 222, 226, 255])
    };
    let pixels: Vec<u8> = [a, b, b, a].concat();
    let img = egui::ColorImage::from_rgba_unmultiplied([2, 2], &pixels);
    let tex = ctx.load_texture(
        "file_viewer_checker",
        img,
        egui::TextureOptions::NEAREST_REPEAT,
    );
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

/// Kanvas gambar yang bisa di-zoom dan digulir. Mengembalikan posisi kursor
/// dalam koordinat `natural` (piksel gambar / piksel CSS SVG) bila di atas gambar.
fn image_canvas(
    ui: &mut egui::Ui,
    tex: &egui::TextureHandle,
    natural: egui::Vec2,
    zoom: &mut f32,
    fit: &mut bool,
    fit_upscale: bool,
    salt: &str,
) -> (Option<egui::Vec2>, bool) {
    let avail = ui.available_size() - egui::vec2(4.0, 24.0);
    if *fit && natural.x > 0.0 && natural.y > 0.0 {
        let z = (avail.x / natural.x).min(avail.y / natural.y);
        *zoom = if fit_upscale { z } else { z.min(1.0) }.clamp(MIN_ZOOM, MAX_ZOOM);
    }
    let mut zoomed = false;
    let out = egui::ScrollArea::both()
        .id_salt(("fv_canvas", salt))
        .auto_shrink([false; 2])
        .max_height(avail.y.max(50.0))
        .show(ui, |ui| {
            let size = natural * *zoom;
            let canvas = egui::vec2(
                size.x.max(ui.available_width()),
                size.y.max(ui.available_height()),
            );
            let (rect, resp) = ui.allocate_exact_size(canvas, egui::Sense::hover());
            let img_rect = egui::Rect::from_center_size(rect.center(), size);
            let checker = checker_texture(ui.ctx());
            let uv = egui::Rect::from_min_max(
                egui::Pos2::ZERO,
                egui::pos2(size.x / 16.0, size.y / 16.0),
            );
            let painter = ui.painter();
            painter.image(checker.id(), img_rect, uv, egui::Color32::WHITE);
            painter.image(
                tex.id(),
                img_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            painter.rect_stroke(
                img_rect,
                0.0,
                egui::Stroke::new(1.0, style::nav_border(ui.ctx())),
                egui::StrokeKind::Outside,
            );
            if resp.hovered() {
                let zd = ui.input(|i| i.zoom_delta());
                if (zd - 1.0).abs() > f32::EPSILON {
                    *zoom = (*zoom * zd).clamp(MIN_ZOOM, MAX_ZOOM);
                    *fit = false;
                    zoomed = true;
                }
            }
            resp.hover_pos()
                .filter(|p| img_rect.contains(*p))
                .map(|p| (p - img_rect.min) / *zoom)
        });
    (out.inner, zoomed)
}

fn render_image(ui: &mut egui::Ui, view: &mut ImageView, salt: &str) {
    let ctx = ui.ctx().clone();
    let muted = style::nav_text_muted(&ctx);
    ui.horizontal(|ui| {
        zoom_toolbar(ui, &mut view.zoom, &mut view.fit);
        ui.separator();
        let mut info = format!(
            "{}  ·  {} × {} px  ·  {}",
            view.info.format, view.info.width, view.info.height, view.info.color
        );
        if view.info.animated {
            info.push_str("  ·  first frame");
        }
        ui.label(egui::RichText::new(info).size(12.0).color(muted));
        if let Some((x, y, [r, g, b, a])) = view.hover {
            ui.separator();
            let (sw, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(sw, 2.0, egui::Color32::from_rgba_unmultiplied(r, g, b, a));
            ui.label(
                egui::RichText::new(format!(
                    "x {x}, y {y}  #{r:02X}{g:02X}{b:02X}{}",
                    if a < 255 {
                        format!("  alpha {a}")
                    } else {
                        String::new()
                    }
                ))
                .size(12.0)
                .family(egui::FontFamily::Monospace),
            );
        }
    });
    // Zoom 100% = 1 piksel gambar asli per point. Salinan tampilan bisa lebih
    // kecil dari aslinya (batas tekstur GPU), jadi kanvas memakai ukuran asli.
    let natural = egui::vec2(view.info.width as f32, view.info.height as f32);
    let (hover, _) = image_canvas(
        ui,
        &view.tex,
        natural,
        &mut view.zoom,
        &mut view.fit,
        false,
        salt,
    );
    view.hover = hover.and_then(|p| {
        let (x, y) = (p.x.max(0.0) as u32, p.y.max(0.0) as u32);
        let sx = view.rgba.width as f32 / natural.x.max(1.0);
        let sy = view.rgba.height as f32 / natural.y.max(1.0);
        let px = view
            .rgba
            .pixel((x as f32 * sx) as u32, (y as f32 * sy) as u32)?;
        Some((x, y, px))
    });
}

fn render_svg(ui: &mut egui::Ui, view: &mut SvgView, inbox: &JobInbox, path: &Path, salt: &str) {
    let ctx = ui.ctx().clone();
    let now = ui.input(|i| i.time);
    let muted = style::nav_text_muted(&ctx);
    let mut changed = false;
    ui.horizontal(|ui| {
        changed = zoom_toolbar(ui, &mut view.zoom, &mut view.fit);
        ui.separator();
        ui.label(
            egui::RichText::new(format!("SVG  ·  {:.0} × {:.0}", view.size.0, view.size.1))
                .size(12.0)
                .color(muted),
        );
    });
    let natural = egui::vec2(view.size.0.max(1.0), view.size.1.max(1.0));
    let before = view.zoom;
    let (_, zoomed) = image_canvas(
        ui,
        &view.tex,
        natural,
        &mut view.zoom,
        &mut view.fit,
        true,
        salt,
    );
    if changed || zoomed || (before - view.zoom).abs() > f32::EPSILON {
        view.zoom_changed_at = now;
    }
    // Rasterisasi ulang setelah zoom berhenti berubah sebentar, agar tetap tajam.
    let max_side = ctx.input(|i| i.max_texture_side) as f32;
    let longest = natural.x.max(natural.y);
    let desired = (view.zoom * ctx.pixels_per_point()).min(max_side / longest);
    let off = (view.tex_scale - desired).abs() / desired.max(0.001);
    if off > 0.25 && view.requested_scale.is_none() {
        if now - view.zoom_changed_at > 0.2 {
            view.requested_scale = Some(desired);
            jobs::render_svg(inbox, path.to_path_buf(), Some(view.bytes.clone()), desired);
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(220));
        }
    }
}

// ─── PDF ─────────────────────────────────────────────────────────────────────

fn render_pdf(ui: &mut egui::Ui, view: &mut PdfView, inbox: &JobInbox, salt: &str) {
    let ctx = ui.ctx().clone();
    let muted = style::nav_text_muted(&ctx);
    let n = view.info.pages.len();
    ui.horizontal(|ui| {
        if style::ai_icon_button(ui, icons::ICON_CHEVRON_LEFT.codepoint, "")
            .on_hover_text("Previous page")
            .clicked()
            && view.current > 0
        {
            view.jump = Some(view.current - 1);
        }
        let input = ui.add(
            egui::TextEdit::singleline(&mut view.page_input)
                .desired_width(36.0)
                .horizontal_align(egui::Align::Center),
        );
        if input.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
            && let Ok(p) = view.page_input.trim().parse::<usize>()
        {
            view.jump = Some(p.clamp(1, n) - 1);
        }
        if !input.has_focus() {
            view.page_input = (view.current + 1).to_string();
        }
        ui.label(
            egui::RichText::new(format!("of {n}"))
                .size(12.0)
                .color(muted),
        );
        if style::ai_icon_button(ui, icons::ICON_CHEVRON_RIGHT.codepoint, "")
            .on_hover_text("Next page")
            .clicked()
            && view.current + 1 < n
        {
            view.jump = Some(view.current + 1);
        }
        ui.separator();
        zoom_toolbar(ui, &mut view.zoom, &mut view.fit_width);
    });

    let avail_w = ui.available_width();
    let max_w = view.info.pages.iter().map(|p| p.0).fold(1.0_f32, f32::max);
    if view.fit_width {
        view.zoom = ((avail_w - 2.0 * PDF_PAGE_GAP - 16.0) / max_w).clamp(MIN_ZOOM, MAX_ZOOM);
    }
    let zoom = view.zoom;
    let mut tops = Vec::with_capacity(n);
    let mut y = PDF_PAGE_GAP;
    for (_, h) in &view.info.pages {
        tops.push(y);
        y += h * zoom + PDF_PAGE_GAP;
    }
    let total_h = y;
    let content_w = (max_w * zoom + 2.0 * PDF_PAGE_GAP).max(avail_w - 16.0);
    let ppp = ctx.pixels_per_point();
    let max_side = ctx.input(|i| i.max_texture_side) as f32;

    let mut area = egui::ScrollArea::both()
        .id_salt(("fv_pdf", salt))
        .auto_shrink([false; 2]);
    if let Some(p) = view.jump.take() {
        area = area.vertical_scroll_offset((tops[p] - PDF_PAGE_GAP).max(0.0));
        view.current = p;
    }
    area.show_viewport(ui, |ui, viewport| {
        let (rect, resp) =
            ui.allocate_exact_size(egui::vec2(content_w, total_h), egui::Sense::hover());
        if resp.hovered() {
            let zd = ui.input(|i| i.zoom_delta());
            if (zd - 1.0).abs() > f32::EPSILON {
                view.zoom = (view.zoom * zd).clamp(MIN_ZOOM, MAX_ZOOM);
                view.fit_width = false;
            }
        }
        let painter = ui.painter();
        let dark = ui.visuals().dark_mode;
        let mut visible = Vec::new();
        for (i, (w, h)) in view.info.pages.iter().enumerate() {
            let size = egui::vec2(w * zoom, h * zoom);
            let local =
                egui::Rect::from_min_size(egui::pos2((content_w - size.x) / 2.0, tops[i]), size);
            if !local.intersects(viewport) {
                continue;
            }
            visible.push(i);
            let page_rect = local.translate(rect.min.to_vec2());
            painter.rect_filled(
                page_rect.translate(egui::vec2(0.0, 2.0)),
                2.0,
                egui::Color32::from_black_alpha(if dark { 120 } else { 40 }),
            );
            painter.rect_filled(page_rect, 0.0, egui::Color32::WHITE);
            match (view.pages.get(&i), view.failed.get(&i)) {
                (Some(pt), _) => {
                    painter.image(
                        pt.tex.id(),
                        page_rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                (None, Some(err)) => {
                    painter.text(
                        page_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("Page {} could not be rendered:\n{err}", i + 1),
                        egui::FontId::proportional(13.0),
                        egui::Color32::from_rgb(180, 40, 40),
                    );
                }
                (None, None) => {
                    painter.text(
                        page_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("Loading page {}…", i + 1),
                        egui::FontId::proportional(13.0),
                        egui::Color32::from_gray(120),
                    );
                }
            }
            // Render (ulang) bila belum ada atau resolusinya jauh dari zoom sekarang.
            let desired = (zoom * ppp).min(max_side / w.max(*h).max(1.0));
            let stale = match view.pages.get(&i) {
                Some(pt) => (pt.scale - desired).abs() / desired.max(0.001) > 0.2,
                None => !view.failed.contains_key(&i),
            };
            if stale && !view.pending.contains(&i) && view.pending.len() < MAX_PENDING_PDF_PAGES {
                view.pending.insert(i);
                jobs::render_pdf_page(inbox, view.bytes.clone(), i, desired);
            }
        }
        // Halaman aktif: halaman pertama yang menempati sepertiga atas viewport.
        let probe = viewport.top() + viewport.height() * 0.3;
        if let Some(&p) = visible
            .iter()
            .find(|&&i| tops[i] + view.info.pages[i].1 * zoom >= probe)
        {
            view.current = p;
        }
        // Buang tekstur halaman yang jauh dari viewport.
        if view.pages.len() > PDF_TEXTURE_BUDGET
            && let (Some(&lo), Some(&hi)) = (visible.first(), visible.last())
        {
            view.pages.retain(|&i, _| i + 2 >= lo && i <= hi + 2);
        }
    });
}

// ─── Tabel ───────────────────────────────────────────────────────────────────

fn render_table(
    t: &mut Tabular,
    ui: &mut egui::Ui,
    view: &mut TableView,
    inbox: &JobInbox,
    path: &Path,
) {
    let ctx = ui.ctx().clone();
    let muted = style::nav_text_muted(&ctx);
    let mut reload: Option<(Option<String>, bool)> = None;
    ui.horizontal(|ui| {
        if let Some(m) = &view.meta {
            let mut info = format!("{}  ·  {} rows  ·  {} columns", m.kind, m.rows, m.cols);
            if let Some(enc) = m.encoding {
                info.push_str(&format!("  ·  {enc}"));
            }
            if let Some(d) = m.delimiter {
                let shown = match d {
                    b'\t' => "Tab".to_string(),
                    b' ' => "Space".to_string(),
                    other => (other as char).to_string(),
                };
                info.push_str(&format!("  ·  delimiter {shown}"));
            }
            if let Some(c) = m.compression {
                info.push_str(&format!("  ·  {c}"));
            }
            ui.label(egui::RichText::new(info).size(12.0).color(muted));
            if m.sheets.len() > 1 {
                let current = m.sheet.clone().unwrap_or_default();
                egui::ComboBox::from_id_salt(("fv_sheet", path))
                    .selected_text(current.clone())
                    .show_ui(ui, |ui| {
                        for s in &m.sheets {
                            if ui.selectable_label(*s == current, s).clicked() && *s != current {
                                reload = Some((Some(s.clone()), m.all_rows));
                            }
                        }
                    });
            }
            if m.truncated && !m.all_rows {
                ui.label(
                    egui::RichText::new(format!(
                        "Showing the first {} rows",
                        file_viewer::TABLE_PREVIEW_ROWS
                    ))
                    .size(12.0)
                    .color(egui::Color32::from_rgb(220, 150, 40)),
                );
                if ui.button("Load all rows").clicked() {
                    reload = Some((m.sheet.clone(), true));
                }
            }
        }
        if view.loading {
            ui.spinner();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button(format!(
                    "{}  Import to database…",
                    icons::ICON_UPLOAD.codepoint
                ))
                .on_hover_text("Open this file in the Data Files import dialog")
                .clicked()
            {
                t.transfer_ui
                    .request(super::transfer_ui::TransferAction::OpenDataFile(Some(
                        path.to_path_buf(),
                    )));
            }
        });
    });
    if let Some((sheet, all_rows)) = reload {
        view.loading = true;
        jobs::load_table(inbox, path.to_path_buf(), sheet, all_rows);
    }
    ui.add_space(4.0);
    if view.meta.is_some() {
        crate::data_table::render_table_data(t, ui);
    } else if view.loading {
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.spinner();
            ui.label("Reading file…");
        });
    }
}

// ─── Biner ───────────────────────────────────────────────────────────────────

fn render_binary(ui: &mut egui::Ui, dump: &str, size: u64, salt: &str) {
    let ctx = ui.ctx().clone();
    let muted = style::nav_text_muted(&ctx);
    let shown = (file_viewer::HEX_PREVIEW_BYTES as u64).min(size);
    ui.label(
        egui::RichText::new(format!(
            "Binary file. Showing the first {} as hex.",
            file_viewer::human_size(shown)
        ))
        .size(12.0)
        .color(muted),
    );
    ui.add_space(4.0);
    egui::ScrollArea::both()
        .id_salt(("fv_hex", salt))
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(dump)
                        .family(egui::FontFamily::Monospace)
                        .size(12.0),
                )
                .extend(),
            );
        });
}

#[cfg(test)]
mod tests {
    /// Glyph non-ASCII yang terverifikasi ada di font Proportional egui
    /// (lihat audit cakupan font). Glyph lain bisa tampil sebagai kotak kosong.
    const SAFE_GLYPHS: &[char] = &['·', '…', '×'];

    #[test]
    fn ui_strings_use_only_safe_glyphs() {
        let src = include_str!("file_viewer_ui.rs");
        let body = src.split("#[cfg(test)]").next().unwrap_or(src);
        for (n, line) in body.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            for c in code.chars().filter(|c| !c.is_ascii()) {
                assert!(
                    SAFE_GLYPHS.contains(&c),
                    "line {}: glyph {c:?} (U+{:04X}) may render as an empty box; use an egui_icons constant",
                    n + 1,
                    c as u32
                );
            }
        }
    }
}
