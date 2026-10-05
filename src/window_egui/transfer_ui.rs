//! GUI import/ekspor/transfer (bagian H): ekspor hasil dengan opsi (format,
//! encoding, BOM, enkripsi, ukuran INSERT), Data Files, dekripsi file, dan
//! pintu masuk ke dialog transfer, ekspor objek, dan Data Compare.
//!
//! Menu hanya memasukkan [`TransferAction`] ke antrean; tiap frame
//! [`Tabular::render_transfer_ui`] mengosongkannya dan menggambar dialog yang
//! aktif. Pekerjaan berat berjalan di runtime lewat [`Slot`]; logikanya ada di
//! `crate::data_transfer` (headless).

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui;

use super::{Tabular, style};
use crate::data_transfer::catalog::{self, Endpoint};
use crate::data_transfer::data_files::{self, LoadedTable};
use crate::data_transfer::encoding::TextEncoding;
use crate::data_transfer::formats::{self, ExportFormat, ExportOptions};
use crate::data_transfer::object_export::ObjectKind;
use crate::data_transfer::readers::{self, ReadOptions};
use crate::data_transfer::values::InsertLimits;
use crate::data_transfer::{TableData, encrypt};
use crate::models::enums::{DatabaseType, NodeType};
use crate::models::structs::TreeNode;
use crate::rfd;

/// Permintaan dari menu, palet perintah, atau sidebar.
#[derive(Clone, Debug)]
pub enum TransferAction {
    /// Ekspor hasil tab aktif dengan opsi.
    ExportResult,
    /// Buka file data sebagai tabel; `None` = tanya lewat dialog pilih file.
    OpenDataFile(Option<PathBuf>),
    DecryptFile,
    Transfer {
        conn_id: Option<i64>,
        database: Option<String>,
        table: Option<String>,
    },
    ExportObjects {
        conn_id: Option<i64>,
        database: Option<String>,
        preselect: Option<(ObjectKind, String)>,
    },
    CompareData {
        conn_id: Option<i64>,
        database: Option<String>,
        table: Option<String>,
    },
    Backup {
        conn_id: i64,
        database: String,
    },
    Restore {
        conn_id: i64,
        database: String,
    },
}

const QUEUE_ID: &str = "tabular_transfer_actions";

/// Masukkan aksi ke antrean; diproses oleh `render_transfer_ui`.
pub fn queue_action(ctx: &egui::Context, action: TransferAction) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<TransferAction>>(egui::Id::new(QUEUE_ID))
            .push(action)
    });
}

fn drain_actions(ctx: &egui::Context) -> Vec<TransferAction> {
    ctx.data_mut(|d| d.remove_temp::<Vec<TransferAction>>(egui::Id::new(QUEUE_ID)))
        .unwrap_or_default()
}

/// Slot hasil task latar belakang; diisi sekali oleh task, diambil oleh UI.
pub(super) struct Slot<T>(Arc<Mutex<Option<T>>>);

impl<T> Clone for Slot<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

impl<T> Slot<T> {
    fn set(&self, value: T) {
        if let Ok(mut guard) = self.0.lock() {
            *guard = Some(value);
        }
    }

    pub(super) fn take(&self) -> Option<T> {
        self.0.lock().ok().and_then(|mut guard| guard.take())
    }
}

/// State UI transfer milik `Tabular`.
#[derive(Default)]
pub struct TransferUiState {
    pending: Vec<TransferAction>,
    export: Option<ExportDialog>,
    data_file: Option<DataFileJob>,
    /// File data yang menunggu giliran dimuat (mis. beberapa file di-drop
    /// sekaligus); hanya satu yang dimuat pada satu waktu.
    data_file_queue: std::collections::VecDeque<PathBuf>,
    /// `true` = teks `NULL` di file berpemisah dibiarkan sebagai string.
    /// Bawaannya `false` (teks `NULL` dibaca sebagai SQL NULL) karena ekspor
    /// CSV Tabular sendiri menulis `NULL` untuk null dan bolak-balik itu
    /// harus tetap utuh.
    data_file_keep_null_text: bool,
    decrypt: Option<DecryptDialog>,
    pub(super) transfer: Option<super::transfer_dialogs::TransferDialog>,
    pub(super) objects: Option<super::transfer_dialogs::ObjectExportDialog>,
    pub(super) compare: Option<super::transfer_compare_ui::CompareDialog>,
}

impl TransferUiState {
    /// Untuk kode yang memegang `Tabular` tetapi tidak punya `egui::Context`.
    pub fn request(&mut self, action: TransferAction) {
        self.pending.push(action);
    }
}

/// `1234567` -> `1,234,567`.
pub(super) fn fmt_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Bingkai dialog modal standar. `size.y <= 0` berarti tinggi mengikuti isi
/// (untuk dialog kecil). Mengembalikan `true` bila diminta tutup (tombol
/// tutup atau Esc).
pub(super) fn modal(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    size: egui::Vec2,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> bool {
    style::render_modal_backdrop(ctx, id, true);
    let screen = ctx.content_rect();
    let width = (screen.width() - 32.0).min(size.x);
    let auto_height = size.y <= 0.0;
    let mut close = false;
    let window = egui::Window::new(id)
        .id(egui::Id::new(id))
        .title_bar(false)
        .frame(style::modal_window_frame(ctx))
        .resizable(false)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0));
    let window = if auto_height {
        window.default_width(width)
    } else {
        window.fixed_size(egui::vec2(width, (screen.height() - 32.0).min(size.y)))
    };
    window.show(ctx, |ui| {
        if auto_height {
            ui.set_width(width);
        } else {
            ui.set_width(ui.available_width());
        }
        style::render_modal_header(ui, title, &mut close);
        ui.add_space(8.0);
        add_contents(ui);
    });
    close
}

/// Dua kartu berdampingan dengan lebar sama. `add_contents` dipanggil sekali
/// per kartu dengan indeks sisi (0 = kiri, 1 = kanan); satu closure dipakai
/// supaya kedua sisi boleh meminjam state yang sama.
pub(super) fn card_row(
    ui: &mut egui::Ui,
    titles: [&str; 2],
    mut add_contents: impl FnMut(&mut egui::Ui, usize),
) {
    let gap = 8.0;
    let half = ((ui.available_width() - gap) / 2.0).floor();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (side, title) in titles.iter().enumerate() {
            let frame = style::modal_card_frame(ui.ctx());
            let inner = half - frame.total_margin().sum().x;
            frame.show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.set_width(inner);
                    ui.label(egui::RichText::new(*title).strong().size(13.5));
                    ui.add_space(10.0);
                    add_contents(ui, side);
                });
            });
        }
    });
}

/// Baris pesan gagal berwarna bahaya.
pub(super) fn error_label(ui: &mut egui::Ui, message: &str) {
    ui.label(
        egui::RichText::new(message)
            .small()
            .color(style::theme_danger(ui.ctx())),
    );
}

pub(super) fn warning_label(ui: &mut egui::Ui, message: &str) {
    ui.label(
        egui::RichText::new(message)
            .small()
            .color(style::theme_warning(ui.ctx())),
    );
}

pub(super) fn muted_label(ui: &mut egui::Ui, message: impl Into<String>) {
    ui.label(
        egui::RichText::new(message.into())
            .small()
            .color(style::theme_muted_text(ui.ctx())),
    );
}

/// Kolom passphrase ganda untuk ekspor terenkripsi. Mengembalikan pesan
/// validasi bila belum bisa dipakai.
pub(super) fn passphrase_fields(
    ui: &mut egui::Ui,
    encrypt: &mut bool,
    pass1: &mut String,
    pass2: &mut String,
) -> Option<&'static str> {
    ui.checkbox(encrypt, "Encrypt with a passphrase (AES-256-GCM)");
    if !*encrypt {
        return None;
    }
    ui.horizontal(|ui| {
        style::render_text_field(
            ui,
            egui::TextEdit::singleline(pass1)
                .password(true)
                .hint_text("Passphrase"),
            190.0,
            None,
        );
        style::render_text_field(
            ui,
            egui::TextEdit::singleline(pass2)
                .password(true)
                .hint_text("Repeat passphrase"),
            190.0,
            None,
        );
    });
    muted_label(
        ui,
        "The file gets an .enc suffix. It cannot be recovered without the passphrase.",
    );
    if pass1.is_empty() {
        Some("Enter a passphrase")
    } else if pass1 != pass2 {
        Some("Passphrases do not match")
    } else {
        None
    }
}

/// Pilihan dari [`filter_combo`].
pub(super) enum ComboPick {
    /// Entri tetap di puncak daftar (mis. "All tables").
    Pinned,
    Item(usize),
}

/// Daftar sepanjang ini mendapat kotak filter di dalam popup.
const FILTER_COMBO_MIN: usize = 8;

/// ComboBox untuk daftar panjang: kotak filter di dalam popup, dan entri
/// `pinned` opsional di atas daftar.
pub(super) fn filter_combo(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    selected_text: &str,
    width: f32,
    pinned: Option<&str>,
    items: &[String],
) -> Option<ComboPick> {
    let id = ui.make_persistent_id(id_salt);
    let filter_id = id.with("filter");
    let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
    let mut picked = None;
    let response = egui::ComboBox::from_id_salt(id)
        .selected_text(selected_text)
        .width(width)
        .truncate()
        // Klik di kotak filter tidak boleh menutup popup.
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            if items.len() > FILTER_COMBO_MIN {
                ui.add(
                    egui::TextEdit::singleline(&mut filter)
                        .hint_text("Filter")
                        .desired_width(f32::INFINITY),
                );
            }
            let needle = filter.trim().to_lowercase();
            if let Some(label) = pinned
                && needle.is_empty()
                && ui.selectable_label(selected_text == label, label).clicked()
            {
                picked = Some(ComboPick::Pinned);
            }
            let mut shown = 0;
            for (i, item) in items.iter().enumerate() {
                if !needle.is_empty() && !item.to_lowercase().contains(&needle) {
                    continue;
                }
                shown += 1;
                if ui.selectable_label(item == selected_text, item).clicked() {
                    picked = Some(ComboPick::Item(i));
                }
            }
            if shown == 0 && (pinned.is_none() || !needle.is_empty()) {
                ui.label(egui::RichText::new("Nothing to choose").weak().small());
            }
            if picked.is_some() {
                ui.close();
            }
        });
    // Popup tertutup: pembukaan berikutnya mulai tanpa filter.
    if response.inner.is_none() || picked.is_some() {
        filter.clear();
    }
    ui.data_mut(|d| d.insert_temp(filter_id, filter));
    picked
}

/// Pilihan koneksi + database untuk satu sisi operasi, dengan daftar tabel
/// yang dimuat di latar belakang.
#[derive(Default)]
pub(super) struct EndpointPick {
    pub conn_id: Option<i64>,
    pub database: String,
    databases: Vec<String>,
    databases_for: Option<i64>,
    /// Daftar database diambil langsung dari server bila cache kosong.
    databases_slot: Option<Slot<Result<Vec<String>, String>>>,
    databases_error: Option<String>,
    pub tables: Vec<String>,
    tables_key: Option<(i64, String)>,
    tables_slot: Option<Slot<Result<Vec<String>, String>>>,
    pub tables_error: Option<String>,
}

impl EndpointPick {
    pub fn new(conn_id: Option<i64>, database: Option<String>) -> Self {
        Self {
            conn_id,
            database: database.unwrap_or_default(),
            ..Default::default()
        }
    }

    pub fn db_type(&self, app: &Tabular) -> Option<DatabaseType> {
        let id = self.conn_id?;
        app.connections
            .iter()
            .find(|c| c.id == Some(id))
            .map(|c| c.connection_type.clone())
    }

    pub fn endpoint(&self, app: &Tabular) -> Option<Endpoint> {
        app.transfer_endpoint(self.conn_id?, &self.database)
    }

    /// Daftar tabel sedang dimuat.
    pub fn loading(&self) -> bool {
        self.tables_slot.is_some()
    }

    /// Paksa muat ulang daftar tabel (setelah transfer membuat tabel baru).
    pub fn invalidate_tables(&mut self) {
        self.tables_key = None;
    }

    /// Ambil hasil task daftar tabel dan mulai task baru bila koneksi atau
    /// database berubah.
    pub fn sync_tables(&mut self, app: &mut Tabular, ctx: &egui::Context) {
        if let Some(slot) = &self.tables_slot
            && let Some(result) = slot.take()
        {
            self.tables_slot = None;
            match result {
                Ok(tables) => {
                    self.tables = tables;
                    self.tables_error = None;
                }
                Err(e) => {
                    self.tables.clear();
                    self.tables_error = Some(e);
                }
            }
        }
        let Some(conn_id) = self.conn_id else {
            return;
        };
        let key = (conn_id, self.database.trim().to_string());
        if self.tables_slot.is_some() || self.tables_key.as_ref() == Some(&key) {
            return;
        }
        self.tables_key = Some(key);
        self.tables.clear();
        self.tables_error = None;
        let Some(endpoint) = self.endpoint(app) else {
            return;
        };
        // MySQL/SQL Server tanpa database: belum ada yang bisa didaftar.
        if needs_database(endpoint.db_type())
            && endpoint.database.is_none()
            && endpoint.conn.database.trim().is_empty()
        {
            return;
        }
        self.tables_slot = Some(app.spawn_slot(ctx, async move {
            let endpoint = endpoint.connect().await?;
            catalog::list_tables(&endpoint).await
        }));
    }

    /// Gambar pemilih koneksi dan database. Mengembalikan `true` bila berubah.
    pub fn ui(&mut self, ui: &mut egui::Ui, app: &mut Tabular, salt: &str) -> bool {
        self.ui_with(ui, app, salt, |_, _, _| {})
    }

    /// Seperti [`Self::ui`], dengan baris tambahan di grid yang sama (mis.
    /// pemilih tabel). `extra_rows` menerima lebar combobox dan menutup tiap
    /// barisnya sendiri dengan `end_row`.
    pub fn ui_with(
        &mut self,
        ui: &mut egui::Ui,
        app: &mut Tabular,
        salt: &str,
        extra_rows: impl FnOnce(&mut egui::Ui, &Self, f32),
    ) -> bool {
        if let Some(slot) = &self.databases_slot
            && let Some(result) = slot.take()
        {
            self.databases_slot = None;
            match result {
                Ok(databases) => self.databases = databases,
                Err(e) => self.databases_error = Some(e),
            }
        }
        let connections = app.sql_connections();
        let before = (self.conn_id, self.database.clone());
        // Sisakan tempat untuk label di kolom kiri grid.
        let combo_width = (ui.available_width() - 100.0).clamp(150.0, 300.0);
        egui::Grid::new((salt, "endpoint_grid"))
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Connection").weak().small());
                let current = self
                    .conn_id
                    .and_then(|id| connections.iter().find(|c| c.0 == id))
                    .map(|c| c.1.clone())
                    .unwrap_or_else(|| "Select a connection".to_string());
                egui::ComboBox::from_id_salt((salt, "endpoint_conn"))
                    .selected_text(current)
                    .width(combo_width)
                    .show_ui(ui, |ui| {
                        for (id, name, db_type) in &connections {
                            let label = format!("{}  ({})", name, db_type.as_db_str());
                            if ui
                                .selectable_label(self.conn_id == Some(*id), label)
                                .clicked()
                            {
                                self.conn_id = Some(*id);
                            }
                        }
                    });
                ui.end_row();

                // Baris Database selalu ada supaya kedua sisi dialog sama
                // bentuknya; dimatikan bila engine hanya punya satu database.
                let wants_database = self.db_type(app).as_ref().is_some_and(needs_database);
                ui.label(egui::RichText::new("Database").weak().small());
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(wants_database, |ui| {
                        let text = if self.conn_id.is_none() {
                            "Select a connection first".to_string()
                        } else if !wants_database {
                            "Single database".to_string()
                        } else if self.database.trim().is_empty() {
                            "Select a database".to_string()
                        } else {
                            self.database.clone()
                        };
                        if let Some(ComboPick::Item(i)) = filter_combo(
                            ui,
                            (salt, "endpoint_db"),
                            &text,
                            combo_width,
                            None,
                            &self.databases,
                        ) {
                            self.database = self.databases[i].clone();
                        }
                    });
                    if self.databases_slot.is_some() {
                        ui.add(egui::Spinner::new());
                    }
                });
                ui.end_row();

                extra_rows(ui, self, combo_width);
            });
        if let Some(e) = &self.databases_error {
            error_label(ui, e);
        }

        if self.conn_id != before.0 {
            // Koneksi baru: pakai database bawaannya dan daftar dari cache.
            self.database = self
                .conn_id
                .and_then(|id| app.connections.iter().find(|c| c.id == Some(id)))
                .filter(|c| needs_database(&c.connection_type))
                .map(|c| c.database.clone())
                .unwrap_or_default();
        }
        if self.databases_for != self.conn_id {
            self.databases_for = self.conn_id;
            // Hasil untuk koneksi sebelumnya dibuang bersama slot-nya.
            self.databases_slot = None;
            self.databases_error = None;
            self.databases = self
                .conn_id
                .and_then(|id| crate::cache_data::get_databases_from_cache(app, id))
                .unwrap_or_default();
            // Koneksi yang belum pernah dibuka di sidebar belum punya cache.
            if self.databases.is_empty()
                && self.db_type(app).as_ref().is_some_and(needs_database)
                && let Some(endpoint) = self.conn_id.and_then(|id| app.transfer_endpoint(id, ""))
            {
                let ctx = ui.ctx().clone();
                self.databases_slot = Some(app.spawn_slot(&ctx, async move {
                    let endpoint = endpoint.connect().await?;
                    catalog::list_databases(&endpoint).await
                }));
            }
        }
        (self.conn_id, self.database.clone()) != before
    }
}

/// File yang ekstensinya dikenal pembaca Data Files.
fn is_data_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            readers::FILE_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(ext))
        })
}

/// Engine yang punya beberapa database per koneksi.
pub(super) fn needs_database(db: &DatabaseType) -> bool {
    matches!(
        db,
        DatabaseType::MySQL | DatabaseType::PostgreSQL | DatabaseType::MsSQL
    )
}

// ─── Ekspor hasil dengan opsi ───────────────────────────────────────────────

/// Nama dasar file/tabel ekspor dari judul tab: "Table: users [Prod]" ->
/// "users". Tab query bebas memakai "query_result", karena judulnya bukan
/// nama tabel.
fn export_caption(tab_title: &str) -> String {
    let title = tab_title.trim();
    let named = title
        .strip_prefix("Table:")
        .or_else(|| title.strip_prefix("View:"));
    match named {
        Some(rest) => {
            // Akhiran " [nama koneksi]" ditambahkan saat tab dibuka. Spasi
            // setelah "Table:" dibuang dulu supaya nama berkurung siku
            // (`[dbo].[t]`) tidak terbaca sebagai akhiran itu.
            let rest = rest.trim_start();
            let name = rest.split_once(" [").map_or(rest, |(name, _)| name).trim();
            if name.is_empty() {
                "query_result".to_string()
            } else {
                name.to_string()
            }
        }
        None => "query_result".to_string(),
    }
}

struct ExportDialog {
    data: TableData,
    caption: String,
    db_type: Option<DatabaseType>,
    format: ExportFormat,
    encoding: TextEncoding,
    bom: bool,
    encrypt: bool,
    pass1: String,
    pass2: String,
    max_insert_kb: u32,
    max_insert_rows: u32,
    error: Option<String>,
}

// ─── Data Files ─────────────────────────────────────────────────────────────

type DataFileResult = Result<Vec<LoadedTable>, (String, bool)>;

struct DataFileJob {
    path: PathBuf,
    passphrase: String,
    slot: Option<Slot<DataFileResult>>,
    error: Option<String>,
}

// ─── Dekripsi file ──────────────────────────────────────────────────────────

#[derive(Default)]
struct DecryptDialog {
    path: Option<PathBuf>,
    passphrase: String,
    error: Option<String>,
}

impl Tabular {
    /// Koneksi yang bisa menjadi sumber/tujuan operasi SQL.
    pub(super) fn sql_connections(&self) -> Vec<(i64, String, DatabaseType)> {
        self.connections
            .iter()
            .filter(|c| catalog::supports_sql(&c.connection_type))
            .filter_map(|c| Some((c.id?, c.name.clone(), c.connection_type.clone())))
            .collect()
    }

    /// Endpoint headless untuk koneksi `conn_id`; pool yang sudah terbuka
    /// dipakai ulang.
    pub(crate) fn transfer_endpoint(&self, conn_id: i64, database: &str) -> Option<Endpoint> {
        let conn = self
            .connections
            .iter()
            .find(|c| c.id == Some(conn_id))
            .cloned()?;
        let pool = self.connection_pools.get(&conn_id).cloned().or_else(|| {
            crate::connection::pool::lock_or_recover(&self.shared_connection_pools)
                .get(&conn_id)
                .cloned()
        });
        let database = database.trim();
        Some(Endpoint::new(
            conn,
            pool,
            (!database.is_empty()).then(|| database.to_string()),
        ))
    }

    /// Jalankan `future` di runtime; hasilnya masuk ke slot dan memicu repaint.
    pub(super) fn spawn_slot<T, F>(&mut self, ctx: &egui::Context, future: F) -> Slot<T>
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        let slot = Slot::default();
        let task_slot = slot.clone();
        let ctx = ctx.clone();
        self.get_runtime().spawn(async move {
            task_slot.set(future.await);
            ctx.request_repaint();
        });
        slot
    }

    /// Dipanggil setiap frame dari `app_impl`.
    pub(crate) fn render_transfer_ui(&mut self, ctx: &egui::Context) {
        let mut actions = std::mem::take(&mut self.transfer_ui.pending);
        actions.extend(drain_actions(ctx));
        // File data yang di-drop ke jendela dibuka sebagai tabel Data Files.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .filter(|path| is_data_file(path))
                .collect()
        });
        actions.extend(
            dropped
                .into_iter()
                .map(|path| TransferAction::OpenDataFile(Some(path))),
        );
        for action in actions {
            self.start_transfer_action(action);
        }
        if self.transfer_ui.data_file.is_none()
            && let Some(path) = self.transfer_ui.data_file_queue.pop_front()
        {
            self.start_data_file(ctx, path, String::new());
        }
        self.render_export_dialog(ctx);
        self.render_data_file_job(ctx);
        self.render_decrypt_dialog(ctx);
        self.render_transfer_dialog(ctx);
        self.render_object_export_dialog(ctx);
        self.render_compare_dialog(ctx);
    }

    fn start_transfer_action(&mut self, action: TransferAction) {
        match action {
            TransferAction::ExportResult => self.open_export_dialog(),
            TransferAction::OpenDataFile(path) => {
                let path = path.or_else(|| {
                    rfd::FileDialog::new()
                        .add_filter(
                            "Data files (CSV, JSON, Excel, Parquet)",
                            readers::FILE_EXTENSIONS,
                        )
                        .add_filter("All files", &["*"])
                        .pick_file()
                });
                if let Some(path) = path {
                    self.transfer_ui.data_file_queue.push_back(path);
                }
            }
            TransferAction::DecryptFile => {
                self.transfer_ui.decrypt = Some(DecryptDialog::default());
            }
            TransferAction::Transfer {
                conn_id,
                database,
                table,
            } => {
                self.transfer_ui.transfer = Some(super::transfer_dialogs::TransferDialog::new(
                    conn_id, database, table,
                ));
            }
            TransferAction::ExportObjects {
                conn_id,
                database,
                preselect,
            } => {
                self.transfer_ui.objects = Some(super::transfer_dialogs::ObjectExportDialog::new(
                    conn_id, database, preselect,
                ));
            }
            TransferAction::CompareData {
                conn_id,
                database,
                table,
            } => {
                self.transfer_ui.compare = Some(super::transfer_compare_ui::CompareDialog::new(
                    conn_id, database, table,
                ));
            }
            TransferAction::Backup { conn_id, database } => {
                self.show_backup_dialog = true;
                self.backup_state = Some(crate::dialog_backup_restore::BackupDialogState::new(
                    conn_id,
                    database,
                    &self.connections,
                ));
            }
            TransferAction::Restore { conn_id, database } => {
                self.show_restore_dialog = true;
                self.restore_state = Some(crate::dialog_backup_restore::RestoreDialogState::new(
                    conn_id,
                    database,
                    &self.connections,
                ));
            }
        }
    }

    // ── Ekspor hasil ────────────────────────────────────────────────────────

    fn open_export_dialog(&mut self) {
        if self.current_table_headers.is_empty() {
            self.toasts
                .warning("There is no result to export in this tab");
            return;
        }
        let db_type = self
            .current_connection_id
            .and_then(|id| self.connections.iter().find(|c| c.id == Some(id)))
            .map(|c| c.connection_type.clone());
        self.transfer_ui.export = Some(ExportDialog {
            data: TableData::new(
                self.current_table_headers.clone(),
                self.all_table_data.clone(),
            ),
            caption: export_caption(
                self.query_tabs
                    .get(self.active_tab_index)
                    .map(|tab| tab.title.as_str())
                    .unwrap_or(""),
            ),
            db_type,
            format: ExportFormat::Csv,
            encoding: TextEncoding::Utf8,
            bom: false,
            encrypt: false,
            pass1: String::new(),
            pass2: String::new(),
            max_insert_kb: 1024,
            max_insert_rows: 500,
            error: None,
        });
    }

    fn render_export_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.transfer_ui.export.take() else {
            return;
        };
        let mut export_clicked = false;
        let mut cancel = false;
        let close = modal(
            ctx,
            "transfer_export_dialog",
            "Export Result",
            egui::vec2(520.0, 0.0),
            |ui| {
                muted_label(
                    ui,
                    format!(
                        "{} rows, {} columns as \"{}\" (the rows loaded in the grid)",
                        fmt_count(dialog.data.rows.len() as u64),
                        dialog.data.headers.len(),
                        formats::table_name_from_caption(&dialog.caption)
                    ),
                );
                ui.add_space(8.0);
                style::render_modal_card(ui, Some("Format"), None, |ui| {
                    egui::Grid::new("transfer_export_grid")
                        .num_columns(2)
                        .spacing([12.0, 8.0])
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("File format").weak().small());
                            egui::ComboBox::from_id_salt("transfer_export_format")
                                .selected_text(dialog.format.label())
                                .width(220.0)
                                .show_ui(ui, |ui| {
                                    for format in ExportFormat::ALL {
                                        ui.selectable_value(
                                            &mut dialog.format,
                                            format,
                                            format.label(),
                                        );
                                    }
                                });
                            ui.end_row();

                            if dialog.format.is_text() {
                                ui.label(egui::RichText::new("Encoding").weak().small());
                                ui.horizontal(|ui| {
                                    egui::ComboBox::from_id_salt("transfer_export_encoding")
                                        .selected_text(dialog.encoding.label())
                                        .width(140.0)
                                        .show_ui(ui, |ui| {
                                            for encoding in TextEncoding::ALL {
                                                ui.selectable_value(
                                                    &mut dialog.encoding,
                                                    encoding,
                                                    encoding.label(),
                                                );
                                            }
                                        });
                                    // Windows-1252 tidak punya byte order mark.
                                    ui.add_enabled(
                                        dialog.encoding != TextEncoding::Windows1252,
                                        egui::Checkbox::new(&mut dialog.bom, "Write BOM"),
                                    );
                                });
                                ui.end_row();
                            }

                            if dialog.format == ExportFormat::SqlInsert {
                                ui.label(egui::RichText::new("Max INSERT size").weak().small());
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::DragValue::new(&mut dialog.max_insert_kb)
                                            .range(1..=65_536)
                                            .suffix(" KB"),
                                    );
                                    ui.label("or");
                                    ui.add(
                                        egui::DragValue::new(&mut dialog.max_insert_rows)
                                            .range(1..=100_000)
                                            .suffix(" rows"),
                                    );
                                    ui.label("per statement");
                                });
                                ui.end_row();
                            }
                        });
                    if dialog.format.is_text() && dialog.encoding == TextEncoding::Windows1252 {
                        muted_label(ui, "Characters outside Windows-1252 are written as \"?\".");
                    }
                });
                ui.add_space(6.0);
                let mut pass_problem = None;
                style::render_modal_card(ui, Some("Protection"), None, |ui| {
                    pass_problem = passphrase_fields(
                        ui,
                        &mut dialog.encrypt,
                        &mut dialog.pass1,
                        &mut dialog.pass2,
                    );
                });
                if let Some(error) = &dialog.error {
                    ui.add_space(6.0);
                    error_label(ui, error);
                }
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            pass_problem.is_none(),
                            style::btn_primary_ctx(ctx, "Export..."),
                        )
                        .clicked()
                    {
                        export_clicked = true;
                    }
                    if ui.add(style::btn_secondary("Cancel")).clicked() {
                        cancel = true;
                    }
                    if let Some(problem) = pass_problem {
                        muted_label(ui, problem);
                    }
                });
            },
        );

        if export_clicked {
            let name = formats::table_name_from_caption(&dialog.caption);
            let picked = rfd::FileDialog::new()
                .add_filter(dialog.format.label(), &[dialog.format.extension()])
                .set_file_name(format!("{}.{}", name, dialog.format.extension()))
                .save_file();
            if let Some(path) = picked {
                let opts = ExportOptions {
                    encoding: dialog.encoding,
                    bom: dialog.bom && dialog.encoding != TextEncoding::Windows1252,
                    table_name: dialog.caption.clone(),
                    db_type: dialog.db_type.clone(),
                    insert_limits: InsertLimits {
                        max_rows: dialog.max_insert_rows as usize,
                        max_bytes: dialog.max_insert_kb as usize * 1024,
                    },
                    passphrase: dialog.encrypt.then(|| dialog.pass1.clone()),
                };
                match formats::write_file(&path, dialog.format, &dialog.data, &opts) {
                    Ok(outcome) => {
                        let mut message = format!(
                            "Exported {} rows to {}",
                            fmt_count(outcome.rows as u64),
                            outcome.path.display()
                        );
                        if outcome.encrypted {
                            message.push_str(" (encrypted)");
                        }
                        self.toasts.success(message);
                        if outcome.unmappable > 0 {
                            self.toasts.warning(format!(
                                "{} characters could not be written in {} and were replaced with \"?\"",
                                fmt_count(outcome.unmappable as u64),
                                dialog.encoding.label()
                            ));
                        }
                        return;
                    }
                    Err(e) => {
                        log::warn!("[TRANSFER] export failed: {}", e);
                        dialog.error = Some(e);
                    }
                }
            }
        }
        if !(close || cancel) {
            self.transfer_ui.export = Some(dialog);
        }
    }

    // ── Data Files ──────────────────────────────────────────────────────────

    fn start_data_file(&mut self, ctx: &egui::Context, path: PathBuf, passphrase: String) {
        let workspace = data_files::workspace_path();
        let file = path.clone();
        let opts = ReadOptions {
            passphrase: (!passphrase.is_empty()).then(|| passphrase.clone()),
            null_text: (!self.transfer_ui.data_file_keep_null_text).then(|| "NULL".to_string()),
            ..Default::default()
        };
        let slot = self.spawn_slot(ctx, async move {
            data_files::load_file(&workspace, &file, &opts)
                .await
                .map_err(|e| (e.to_string(), e.needs_passphrase()))
        });
        self.transfer_ui.data_file = Some(DataFileJob {
            path,
            passphrase,
            slot: Some(slot),
            error: None,
        });
    }

    /// Koneksi SQLite yang menunjuk workspace Data Files; dibuat bila belum ada.
    fn ensure_data_files_connection(&mut self) -> Option<i64> {
        let workspace = data_files::workspace_path().to_string_lossy().into_owned();
        let find = |app: &Tabular| {
            app.connections
                .iter()
                .find(|c| c.connection_type == DatabaseType::SQLite && c.database == workspace)
                .and_then(|c| c.id)
        };
        if let Some(id) = find(self) {
            return Some(id);
        }
        let connection = crate::models::structs::ConnectionConfig {
            name: data_files::WORKSPACE_CONNECTION_NAME.to_string(),
            connection_type: DatabaseType::SQLite,
            database: workspace.clone(),
            host: String::new(),
            port: String::new(),
            ..Default::default()
        };
        if !crate::sidebar_database::save_connection_to_database(self, &connection) {
            return None;
        }
        crate::sidebar_database::load_connections(self);
        find(self)
    }

    fn finish_data_file(&mut self, tables: Vec<LoadedTable>) {
        let Some(conn_id) = self.ensure_data_files_connection() else {
            self.toasts
                .error("Could not register the Data Files connection");
            return;
        };
        self.refresh_after_schema_change(conn_id, Some("main"));
        let rows: u64 = tables.iter().map(|t| t.rows as u64).sum();
        let message = match tables.as_slice() {
            [one] => format!(
                "Opened \"{}\" ({} rows) in {}",
                one.table,
                fmt_count(rows),
                data_files::WORKSPACE_CONNECTION_NAME
            ),
            many => format!(
                "Opened {} tables ({} rows) in {}",
                many.len(),
                fmt_count(rows),
                data_files::WORKSPACE_CONNECTION_NAME
            ),
        };
        self.toasts.success(message);
        if let Some(first) = tables.first() {
            let item = crate::quick_open::QuickOpenItem::new(
                format!("datafile:{}", first.table),
                first.table.clone(),
                String::new(),
                crate::quick_open::QuickOpenKind::Table,
                Some(conn_id),
                Some(data_files::WORKSPACE_CONNECTION_NAME.to_string()),
                None,
                Some(first.table.clone()),
                None,
                None,
                None,
            );
            crate::quick_open::execute_quick_open_item(self, &item);
        }
    }

    fn render_data_file_job(&mut self, ctx: &egui::Context) {
        let Some(mut job) = self.transfer_ui.data_file.take() else {
            return;
        };
        if let Some(slot) = &job.slot
            && let Some(result) = slot.take()
        {
            job.slot = None;
            match result {
                Ok(tables) => {
                    self.finish_data_file(tables);
                    return;
                }
                Err((message, true)) => job.error = Some(message),
                Err((message, false)) => {
                    log::warn!("[TRANSFER] opening data file failed: {}", message);
                    self.toasts.error(format!(
                        "Could not open {}: {}",
                        job.path.display(),
                        message
                    ));
                    return;
                }
            }
        }

        let file_name = job
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut unlock = false;
        let mut cancel = false;
        let mut null_as_sql_null = !self.transfer_ui.data_file_keep_null_text;
        let loading = job.slot.is_some();
        let close = modal(
            ctx,
            "transfer_data_file_dialog",
            "Open Data File",
            egui::vec2(440.0, 0.0),
            |ui| {
                ui.label(egui::RichText::new(&file_name).strong());
                ui.add_space(8.0);
                if loading {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label("Loading file into the Data Files workspace...");
                    });
                    return;
                }
                ui.label("This file is an encrypted Tabular export.");
                ui.add_space(6.0);
                let response = style::render_text_field(
                    ui,
                    egui::TextEdit::singleline(&mut job.passphrase)
                        .password(true)
                        .hint_text("Passphrase"),
                    260.0,
                    None,
                );
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    unlock = true;
                }
                if let Some(error) = &job.error
                    && !job.passphrase.is_empty()
                {
                    error_label(ui, error);
                }
                ui.add_space(6.0);
                ui.checkbox(&mut null_as_sql_null, "Treat the text NULL as SQL NULL");
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !job.passphrase.is_empty(),
                            style::btn_primary_ctx(ctx, "Open"),
                        )
                        .clicked()
                    {
                        unlock = true;
                    }
                    if ui.add(style::btn_secondary("Cancel")).clicked() {
                        cancel = true;
                    }
                });
            },
        );
        if loading {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
        self.transfer_ui.data_file_keep_null_text = !null_as_sql_null;
        if unlock && !job.passphrase.is_empty() {
            self.start_data_file(ctx, job.path, job.passphrase);
            return;
        }
        // Saat masih memuat, tutup hanya menyembunyikan dialog; task tetap
        // menulis ke slot yang dibuang dan hasilnya diabaikan.
        if !(close || cancel) {
            self.transfer_ui.data_file = Some(job);
        }
    }

    // ── Dekripsi file ───────────────────────────────────────────────────────

    fn render_decrypt_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.transfer_ui.decrypt.take() else {
            return;
        };
        let mut choose = false;
        let mut decrypt = false;
        let mut cancel = false;
        let close = modal(
            ctx,
            "transfer_decrypt_dialog",
            "Decrypt Exported File",
            egui::vec2(480.0, 0.0),
            |ui| {
                muted_label(
                    ui,
                    "Turns an encrypted .enc export back into the original file.",
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(style::btn_field_action(ui, "Choose File..."))
                        .clicked()
                    {
                        choose = true;
                    }
                    match &dialog.path {
                        Some(path) => ui.label(path.display().to_string()),
                        None => ui.label(
                            egui::RichText::new("No file selected")
                                .color(style::theme_muted_text(ctx)),
                        ),
                    };
                });
                ui.add_space(6.0);
                style::render_text_field(
                    ui,
                    egui::TextEdit::singleline(&mut dialog.passphrase)
                        .password(true)
                        .hint_text("Passphrase"),
                    260.0,
                    None,
                );
                if let Some(error) = &dialog.error {
                    ui.add_space(4.0);
                    error_label(ui, error);
                }
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            dialog.path.is_some() && !dialog.passphrase.is_empty(),
                            style::btn_primary_ctx(ctx, "Decrypt and Save As..."),
                        )
                        .clicked()
                    {
                        decrypt = true;
                    }
                    if ui.add(style::btn_secondary("Cancel")).clicked() {
                        cancel = true;
                    }
                });
            },
        );

        if choose
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Encrypted export", &[encrypt::ENCRYPTED_EXTENSION])
                .add_filter("All files", &["*"])
                .pick_file()
        {
            dialog.error = (!readers::file_is_encrypted(&path))
                .then(|| "This file is not a Tabular encrypted export".to_string());
            dialog.path = Some(path);
        }
        if decrypt && let Some(path) = dialog.path.clone() {
            let result = std::fs::read(&path)
                .map_err(|e| format!("Cannot read file: {e}"))
                .and_then(|bytes| {
                    encrypt::decrypt_bytes(&dialog.passphrase, &bytes).map_err(|e| e.to_string())
                });
            match result {
                Ok(plain) => {
                    let default_name = path
                        .file_stem()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "decrypted".to_string());
                    if let Some(target) = rfd::FileDialog::new()
                        .set_file_name(default_name)
                        .save_file()
                    {
                        match std::fs::write(&target, plain) {
                            Ok(()) => {
                                self.toasts
                                    .success(format!("Decrypted to {}", target.display()));
                                return;
                            }
                            Err(e) => dialog.error = Some(format!("Cannot write file: {e}")),
                        }
                    }
                }
                Err(e) => dialog.error = Some(e),
            }
        }
        if !(close || cancel) {
            self.transfer_ui.decrypt = Some(dialog);
        }
    }
}

// ─── Item menu ──────────────────────────────────────────────────────────────

fn push(ui: &mut egui::Ui, action: TransferAction) {
    queue_action(ui.ctx(), action);
    ui.close();
}

fn node_database(node: &TreeNode) -> Option<String> {
    node.database_name
        .clone()
        .or_else(|| (node.node_type == NodeType::Database).then(|| node.name.clone()))
}

/// Item menu export tambahan pada grid hasil.
pub(crate) fn result_menu_items(ui: &mut egui::Ui) {
    if ui
        .button("Export with Options...")
        .on_hover_text(
            "More formats (HTML, XML, NDJSON, Parquet), encoding and BOM, encryption, \
             and INSERT size",
        )
        .clicked()
    {
        push(ui, TransferAction::ExportResult);
    }
}

/// Item menu node tabel di sidebar.
pub(crate) fn table_menu_items(ui: &mut egui::Ui, node: &TreeNode, db_type: Option<&DatabaseType>) {
    let (Some(conn_id), Some(db_type)) = (node.connection_id, db_type) else {
        return;
    };
    if !catalog::supports_sql(db_type) {
        return;
    }
    let table = node.table_name.clone().unwrap_or_else(|| node.name.clone());
    let database = node.database_name.clone();
    if ui.button("Transfer To...").clicked() {
        push(
            ui,
            TransferAction::Transfer {
                conn_id: Some(conn_id),
                database: database.clone(),
                table: Some(table.clone()),
            },
        );
    }
    if ui.button("Compare Data...").clicked() {
        push(
            ui,
            TransferAction::CompareData {
                conn_id: Some(conn_id),
                database: database.clone(),
                table: Some(table.clone()),
            },
        );
    }
    if ui.button("Export as SQL...").clicked() {
        push(
            ui,
            TransferAction::ExportObjects {
                conn_id: Some(conn_id),
                database,
                preselect: Some((ObjectKind::Table, table)),
            },
        );
    }
}

/// Item menu node database di sidebar.
pub(crate) fn database_menu_items(
    ui: &mut egui::Ui,
    node: &TreeNode,
    db_type: Option<&DatabaseType>,
) {
    let (Some(conn_id), Some(db_type)) = (node.connection_id, db_type) else {
        return;
    };
    let Some(database) = node_database(node) else {
        return;
    };
    if matches!(db_type, DatabaseType::MongoDB) {
        // Menu backup engine SQL sudah ada di sidebar; MongoDB memakai dialog
        // yang sama lewat mongodump/mongorestore.
        if ui.button("Backup Database (mongodump)...").clicked() {
            push(
                ui,
                TransferAction::Backup {
                    conn_id,
                    database: database.clone(),
                },
            );
        }
        if ui.button("Restore Database (mongorestore)...").clicked() {
            push(ui, TransferAction::Restore { conn_id, database });
        }
        return;
    }
    if !catalog::supports_sql(db_type) {
        return;
    }
    if ui.button("Transfer Tables To...").clicked() {
        push(
            ui,
            TransferAction::Transfer {
                conn_id: Some(conn_id),
                database: Some(database.clone()),
                table: None,
            },
        );
    }
    if ui.button("Export Objects as SQL...").clicked() {
        push(
            ui,
            TransferAction::ExportObjects {
                conn_id: Some(conn_id),
                database: Some(database.clone()),
                preselect: None,
            },
        );
    }
    if ui.button("Compare Data...").clicked() {
        push(
            ui,
            TransferAction::CompareData {
                conn_id: Some(conn_id),
                database: Some(database),
                table: None,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_get_thousand_separators() {
        assert_eq!(fmt_count(0), "0");
        assert_eq!(fmt_count(999), "999");
        assert_eq!(fmt_count(1000), "1,000");
        assert_eq!(fmt_count(1234567), "1,234,567");
    }

    #[test]
    fn export_name_comes_from_table_tabs_only() {
        assert_eq!(export_caption("Table: sales [Data Files]"), "sales");
        assert_eq!(export_caption("View: v_users"), "v_users");
        assert_eq!(
            export_caption("Table: dbo.orders [Prod [EU]]"),
            "dbo.orders"
        );
        assert_eq!(
            export_caption("Table: [dbo].[orders] [Prod]"),
            "[dbo].[orders]"
        );
        assert_eq!(export_caption("Untitled Query"), "query_result");
        assert_eq!(export_caption("Table:  "), "query_result");
    }

    #[test]
    fn only_known_data_extensions_are_opened_on_drop() {
        assert!(is_data_file(std::path::Path::new("/tmp/Sales.CSV")));
        assert!(is_data_file(std::path::Path::new("a.parquet")));
        assert!(is_data_file(std::path::Path::new("dump.json.gz")));
        assert!(!is_data_file(std::path::Path::new("query.sql")));
        assert!(!is_data_file(std::path::Path::new("README")));
    }

    #[test]
    fn slot_delivers_value_once() {
        let slot: Slot<u32> = Slot::default();
        assert!(slot.take().is_none());
        slot.clone().set(7);
        assert_eq!(slot.take(), Some(7));
        assert!(slot.take().is_none());
    }
}
