use eframe::egui;
use serde_json::Value as JsonValue;
use std::collections::HashSet;

/// Tabs available in the Cell Value Inspector
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum InspectorTab {
    #[default]
    Json,
    Hex,
    Image,
    RawText,
}

impl InspectorTab {
    pub fn label(&self) -> &'static str {
        match self {
            InspectorTab::Json => "{} JSON Tree",
            InspectorTab::Hex => "0x Hex / Binary",
            InspectorTab::Image => "🖼 Image Preview",
            InspectorTab::RawText => "📄 Raw Virtual Text",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            InspectorTab::Json => "{}",
            InspectorTab::Hex => "0x",
            InspectorTab::Image => "🖼",
            InspectorTab::RawText => "📄",
        }
    }
}

/// Decode mode for the Hex/Binary Inspector tab
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HexDecodeMode {
    #[default]
    Auto,
    RawUtf8Bytes,
    Base64,
    HexString,
}

impl HexDecodeMode {
    pub fn label(&self) -> &'static str {
        match self {
            HexDecodeMode::Auto => "Auto Detect",
            HexDecodeMode::RawUtf8Bytes => "Raw UTF-8 Bytes",
            HexDecodeMode::Base64 => "Base64 Decoded",
            HexDecodeMode::HexString => "Hex String Decoded",
        }
    }
}

/// JSON view mode: Tree view or Pretty formatted text view
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum JsonViewMode {
    #[default]
    Tree,
    Formatted,
    Minified,
}

/// Image metadata info
#[derive(Clone, Debug, PartialEq)]
pub struct ImageMetadata {
    pub format_name: String,
    pub width: u32,
    pub height: u32,
    pub byte_size: usize,
    pub is_svg: bool,
    pub svg_xml: Option<String>,
}

/// Full state for the Cell Value Inspector modal / panel
pub struct CellInspectorState {
    pub is_open: bool,
    pub active_tab: InspectorTab,
    pub raw_value: String,
    pub column_name: String,
    pub row_idx: Option<usize>,
    pub col_idx: Option<usize>,

    // JSON tab state
    pub json_parsed: Option<JsonValue>,
    pub json_parse_error: Option<String>,
    pub json_view_mode: JsonViewMode,
    pub json_search_query: String,
    pub json_collapsed_paths: HashSet<String>,
    pub json_formatted_cache: Option<String>,
    pub json_minified_cache: Option<String>,

    // Hex tab state
    pub hex_decode_mode: HexDecodeMode,
    pub hex_bytes: Vec<u8>,
    pub hex_bytes_per_row: usize,
    pub hex_selected_offset: Option<usize>,
    pub hex_search_query: String,

    // Image tab state
    pub image_meta: Option<ImageMetadata>,
    pub image_texture: Option<egui::TextureHandle>,
    pub image_error: Option<String>,
    pub image_zoom: f32,
    pub image_fit_window: bool,

    // Raw Text tab state
    pub text_lines_cache: Vec<String>,
    pub text_search_query: String,
    pub text_word_wrap: bool,
    pub text_show_line_numbers: bool,
    pub text_total_chars: usize,
    pub text_total_words: usize,
    pub text_total_bytes: usize,

    // UI feedback toast/copy message
    pub toast_message: Option<(String, std::time::Instant)>,
}

impl Default for CellInspectorState {
    fn default() -> Self {
        Self {
            is_open: false,
            active_tab: InspectorTab::Json,
            raw_value: String::new(),
            column_name: String::new(),
            row_idx: None,
            col_idx: None,

            json_parsed: None,
            json_parse_error: None,
            json_view_mode: JsonViewMode::Tree,
            json_search_query: String::new(),
            json_collapsed_paths: HashSet::new(),
            json_formatted_cache: None,
            json_minified_cache: None,

            hex_decode_mode: HexDecodeMode::Auto,
            hex_bytes: Vec::new(),
            hex_bytes_per_row: 16,
            hex_selected_offset: None,
            hex_search_query: String::new(),

            image_meta: None,
            image_texture: None,
            image_error: None,
            image_zoom: 1.0,
            image_fit_window: true,

            text_lines_cache: Vec::new(),
            text_search_query: String::new(),
            text_word_wrap: true,
            text_show_line_numbers: true,
            text_total_chars: 0,
            text_total_words: 0,
            text_total_bytes: 0,

            toast_message: None,
        }
    }
}

impl CellInspectorState {
    /// Open the inspector with cell data, auto-detecting initial tab and parsing representations
    pub fn open(
        &mut self,
        value: String,
        column_name: String,
        row_idx: usize,
        col_idx: usize,
    ) {
        self.raw_value = value;
        self.column_name = column_name;
        self.row_idx = Some(row_idx);
        self.col_idx = Some(col_idx);
        self.is_open = true;
        self.image_texture = None; // Reset cached texture so new image loads
        self.json_collapsed_paths.clear();
        self.hex_selected_offset = None;

        self.reparse_all();

        // Auto-select best initial tab
        self.active_tab = self.detect_best_tab();
    }

    /// Reparse JSON, Hex bytes, Images, and Text statistics from current raw_value
    pub fn reparse_all(&mut self) {
        let val_trimmed = self.raw_value.trim();

        // 1. JSON Parsing
        if (val_trimmed.starts_with('{') && val_trimmed.ends_with('}'))
            || (val_trimmed.starts_with('[') && val_trimmed.ends_with(']'))
        {
            match serde_json::from_str::<JsonValue>(val_trimmed) {
                Ok(parsed) => {
                    self.json_formatted_cache = serde_json::to_string_pretty(&parsed).ok();
                    self.json_minified_cache = serde_json::to_string(&parsed).ok();
                    self.json_parsed = Some(parsed);
                    self.json_parse_error = None;
                }
                Err(err) => {
                    self.json_parsed = None;
                    self.json_parse_error = Some(err.to_string());
                    self.json_formatted_cache = None;
                    self.json_minified_cache = None;
                }
            }
        } else {
            self.json_parsed = None;
            self.json_parse_error = Some("Not recognized as JSON format".to_string());
            self.json_formatted_cache = None;
            self.json_minified_cache = None;
        }

        // 2. Hex / Binary Decoding
        self.recompute_hex_bytes();

        // 3. Image Decoding
        self.recompute_image_data();

        // 4. Raw Virtual Text Caching & Stats
        self.recompute_text_stats();
    }

    /// Recompute byte payload for the Hex view based on current decode mode
    pub fn recompute_hex_bytes(&mut self) {
        let val = &self.raw_value;
        let val_trimmed = val.trim();

        let bytes = match self.hex_decode_mode {
            HexDecodeMode::Auto => {
                if val_trimmed.starts_with("0x") || val_trimmed.starts_with("0X") || val_trimmed.starts_with("\\x") {
                    try_decode_hex_str(val_trimmed).unwrap_or_else(|| val.as_bytes().to_vec())
                } else if val_trimmed.starts_with("data:") && val_trimmed.contains(";base64,") {
                    if let Some(pos) = val_trimmed.find(";base64,") {
                        try_decode_base64(&val_trimmed[pos + 8..]).unwrap_or_else(|| val.as_bytes().to_vec())
                    } else {
                        val.as_bytes().to_vec()
                    }
                } else {
                    val.as_bytes().to_vec()
                }
            }
            HexDecodeMode::RawUtf8Bytes => val.as_bytes().to_vec(),
            HexDecodeMode::Base64 => try_decode_base64(val_trimmed).unwrap_or_else(|| val.as_bytes().to_vec()),
            HexDecodeMode::HexString => try_decode_hex_str(val_trimmed).unwrap_or_else(|| val.as_bytes().to_vec()),
        };

        self.hex_bytes = bytes;
    }

    /// Recompute image metadata and prepared payload
    pub fn recompute_image_data(&mut self) {
        let val_trimmed = self.raw_value.trim();

        // Check if SVG XML
        if val_trimmed.starts_with("<svg") || val_trimmed.contains("<svg ") || val_trimmed.contains("xmlns=\"http://www.w3.org/2000/svg\"") {
            let width = extract_xml_attr(val_trimmed, "width").and_then(|w| w.parse::<u32>().ok()).unwrap_or(300);
            let height = extract_xml_attr(val_trimmed, "height").and_then(|h| h.parse::<u32>().ok()).unwrap_or(300);
            self.image_meta = Some(ImageMetadata {
                format_name: "SVG (Scalable Vector Graphics)".to_string(),
                width,
                height,
                byte_size: val_trimmed.len(),
                is_svg: true,
                svg_xml: Some(val_trimmed.to_string()),
            });
            self.image_error = None;
            return;
        }

        // Try extracting binary bytes (via Data URI, Base64, Hex, or raw bytes)
        let byte_candidate = if val_trimmed.starts_with("data:image/") {
            if let Some(pos) = val_trimmed.find(";base64,") {
                let b64_part = &val_trimmed[pos + 8..];
                try_decode_base64(b64_part)
            } else {
                None
            }
        } else if let Some(b64) = try_decode_base64(val_trimmed) {
            Some(b64)
        } else if let Some(hex_bytes) = try_decode_hex_str(val_trimmed) {
            Some(hex_bytes)
        } else {
            Some(self.raw_value.as_bytes().to_vec())
        };

        if let Some(bytes) = byte_candidate {
            if bytes.len() >= 4 {
                match image::load_from_memory(&bytes) {
                    Ok(dyn_img) => {
                        let (w, h) = (dyn_img.width(), dyn_img.height());
                        let format_name = detect_image_format_magic(&bytes).unwrap_or("Raster Image").to_string();
                        self.image_meta = Some(ImageMetadata {
                            format_name,
                            width: w,
                            height: h,
                            byte_size: bytes.len(),
                            is_svg: false,
                            svg_xml: None,
                        });
                        self.image_error = None;
                        return;
                    }
                    Err(err) => {
                        self.image_meta = None;
                        self.image_error = Some(format!("Image decode failed: {}", err));
                    }
                }
            }
        }

        self.image_meta = None;
        if self.image_error.is_none() {
            self.image_error = Some("Cell value does not contain valid image data".to_string());
        }
    }

    /// Recompute text line chunks and statistics
    pub fn recompute_text_stats(&mut self) {
        let val = &self.raw_value;
        self.text_total_chars = val.chars().count();
        self.text_total_bytes = val.len();
        self.text_total_words = val.split_whitespace().count();

        // Split lines for virtual scrolling
        self.text_lines_cache = val.lines().map(|s| s.to_string()).collect();
        if self.text_lines_cache.is_empty() && !val.is_empty() {
            self.text_lines_cache.push(val.clone());
        }
    }

    /// Detect the most relevant tab automatically
    pub fn detect_best_tab(&self) -> InspectorTab {
        let val_trimmed = self.raw_value.trim();

        // 1. Image Check
        if val_trimmed.starts_with("data:image/") || val_trimmed.starts_with("<svg") || self.image_meta.is_some() {
            return InspectorTab::Image;
        }

        // 2. JSON Check
        if self.json_parsed.is_some() {
            return InspectorTab::Json;
        }

        // 3. Binary / Hex Check (Contains null bytes or non-printable controls)
        if self.raw_value.bytes().any(|b| b == 0 || (b < 9 && b != 0)) || val_trimmed.starts_with("0x") || val_trimmed.starts_with("\\x") {
            return InspectorTab::Hex;
        }

        // 4. Default: Raw Text
        InspectorTab::RawText
    }

    /// Trigger a small toast notification in inspector footer
    pub fn show_toast(&mut self, msg: impl Into<String>) {
        self.toast_message = Some((msg.into(), std::time::Instant::now()));
    }
}

// ─── Helper Functions for Decoding ──────────────────────────────────────────

/// Try decoding Base64 if string is valid and has reasonable size
pub fn try_decode_base64(input: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let clean: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if clean.len() < 4 || clean.len() % 4 != 0 {
        return None;
    }
    base64::engine::general_purpose::STANDARD.decode(clean).ok()
}

/// Try decoding Hex string (e.g. `0x1a2b3c`, `\x1a\x2b`, or `1a2b3c4d`)
pub fn try_decode_hex_str(input: &str) -> Option<Vec<u8>> {
    let mut clean = input.trim();
    let has_prefix = clean.starts_with("0x") || clean.starts_with("0X") || clean.starts_with("\\x");
    if clean.starts_with("0x") || clean.starts_with("0X") {
        clean = &clean[2..];
    } else if clean.starts_with("\\x") {
        clean = &clean[2..];
    }

    let is_all_hex_chars = clean.chars().all(|c| c.is_ascii_hexdigit() || c.is_whitespace() || c == ':' || c == ',');
    if !has_prefix && (!is_all_hex_chars || clean.len() < 2) {
        return None;
    }

    let sanitized: String = clean
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();

    if sanitized.len() >= 2 && sanitized.len() % 2 == 0 {
        hex::decode(sanitized).ok()
    } else {
        None
    }
}

/// Detect image format by magic header bytes
pub fn detect_image_format_magic(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("PNG (Portable Network Graphics)")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("JPEG / JPG Image")
    } else if bytes.starts_with(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
        Some("WebP Image")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("GIF Animated Image")
    } else if bytes.starts_with(b"BM") {
        Some("BMP Bitmap Image")
    } else if bytes.starts_with(b"\x00\x00\x01\x00") {
        Some("ICO Icon Image")
    } else {
        None
    }
}

/// Extract attribute from simple XML/SVG string
fn extract_xml_attr<'a>(xml: &'a str, attr: &str) -> Option<&'a str> {
    let pattern = format!("{}=\"", attr);
    if let Some(pos) = xml.find(&pattern) {
        let start = pos + pattern.len();
        if let Some(end) = xml[start..].find('"') {
            return Some(&xml[start..start + end]);
        }
    }
    None
}

// ─── Main Inspector Modal Renderer ──────────────────────────────────────────

/// Main renderer for the Value Inspector Window
pub fn render_cell_inspector(tabular: &mut crate::window_egui::Tabular, ctx: &egui::Context) {
    if !tabular.cell_inspector.is_open {
        return;
    }

    let mut is_open = tabular.cell_inspector.is_open;
    let title = if let (Some(r), Some(c)) = (tabular.cell_inspector.row_idx, tabular.cell_inspector.col_idx) {
        format!("🔍 Value Inspector — {} [Row {}, Col {}]", tabular.cell_inspector.column_name, r + 1, c + 1)
    } else {
        format!("🔍 Value Inspector — {}", tabular.cell_inspector.column_name)
    };

    let dark = ctx.global_style().visuals.dark_mode;
    let window_fill = if dark {
        egui::Color32::from_rgb(22, 24, 30)
    } else {
        egui::Color32::from_rgb(250, 250, 252)
    };

    let mut action_copy_text: Option<String> = None;

    egui::Window::new(title)
        .open(&mut is_open)
        .default_size(egui::vec2(860.0, 580.0))
        .min_size(egui::vec2(550.0, 380.0))
        .resizable(true)
        .collapsible(false)
        .frame(
            egui::Frame::window(&ctx.global_style())
                .fill(window_fill)
                .stroke(egui::Stroke::new(1.0, if dark { egui::Color32::from_rgb(55, 60, 75) } else { egui::Color32::from_rgb(210, 215, 225) }))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(14, 12))
        )
        .show(ctx, |ui| {
            // ─── Top Bar: Tabs & Quick Actions ───────────────────────────────
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;

                let tabs = [
                    InspectorTab::Json,
                    InspectorTab::Hex,
                    InspectorTab::Image,
                    InspectorTab::RawText,
                ];

                for tab in tabs {
                    let is_active = tabular.cell_inspector.active_tab == tab;
                    let text = egui::RichText::new(tab.label()).strong();

                    let btn_resp = if is_active {
                        let accent = crate::window_egui::style::theme_accent(ctx);
                        ui.add(egui::Button::new(text.color(egui::Color32::WHITE)).fill(accent).corner_radius(6.0))
                    } else {
                        ui.add(egui::Button::new(text).corner_radius(6.0))
                    };

                    if btn_resp.clicked() {
                        tabular.cell_inspector.active_tab = tab;
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("📋 Copy Raw").on_hover_text("Copy original cell value to clipboard").clicked() {
                        action_copy_text = Some(tabular.cell_inspector.raw_value.clone());
                    }

                    // Format Quick Indicator
                    if tabular.cell_inspector.json_parsed.is_some() {
                        crate::window_egui::style::render_badge(ui, "JSON VALID", egui::Color32::from_rgb(20, 80, 45), egui::Color32::from_rgb(130, 240, 160));
                    } else if tabular.cell_inspector.image_meta.is_some() {
                        crate::window_egui::style::render_badge(ui, "IMAGE", egui::Color32::from_rgb(30, 60, 100), egui::Color32::from_rgb(140, 200, 255));
                    }
                });
            });

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);

            // ─── Active Tab Content ──────────────────────────────────────────
            match tabular.cell_inspector.active_tab {
                InspectorTab::Json => render_tab_json(&mut tabular.cell_inspector, ui, ctx, &mut action_copy_text),
                InspectorTab::Hex => render_tab_hex(&mut tabular.cell_inspector, ui, ctx, &mut action_copy_text),
                InspectorTab::Image => render_tab_image(&mut tabular.cell_inspector, ui, ctx, &mut action_copy_text),
                InspectorTab::RawText => render_tab_raw_text(&mut tabular.cell_inspector, ui, ctx, &mut action_copy_text),
            }

            // ─── Bottom Status Bar & Toast ───────────────────────────────────
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                let stats = format!(
                    "Bytes: {} | Characters: {} | Words: {} | Lines: {}",
                    tabular.cell_inspector.text_total_bytes,
                    tabular.cell_inspector.text_total_chars,
                    tabular.cell_inspector.text_total_words,
                    tabular.cell_inspector.text_lines_cache.len()
                );
                ui.label(egui::RichText::new(stats).size(11.0).color(ui.visuals().weak_text_color()));

                if let Some((msg, instant)) = &tabular.cell_inspector.toast_message {
                    if instant.elapsed().as_secs_f32() < 2.5 {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(format!("✓ {}", msg)).size(11.5).color(egui::Color32::from_rgb(70, 200, 120)).strong());
                        });
                    }
                }
            });
        });

    tabular.cell_inspector.is_open = is_open;

    if let Some(text) = action_copy_text {
        ctx.copy_text(text);
        tabular.cell_inspector.show_toast("Copied to clipboard!");
    }
}

// ─── TAB 1: JSON Tree & Formatter ───────────────────────────────────────────

fn render_tab_json(
    state: &mut CellInspectorState,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    action_copy_text: &mut Option<String>,
) {
    if let Some(parse_err) = &state.json_parse_error {
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.label(egui::RichText::new("⚠️ Unable to Parse JSON").size(15.0).color(crate::window_egui::style::theme_warning(ctx)).strong());
            ui.add_space(6.0);
            ui.label(egui::RichText::new(parse_err).size(12.0).color(ui.visuals().weak_text_color()));
            ui.add_space(14.0);
            if ui.button("Switch to Raw Virtual Text").clicked() {
                state.active_tab = InspectorTab::RawText;
            }
        });
        return;
    }

    // JSON Sub-Toolbar
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;

        // View Mode: Tree vs Pretty Formatted vs Minified
        ui.label(egui::RichText::new("Mode:").strong().size(12.0));
        ui.selectable_value(&mut state.json_view_mode, JsonViewMode::Tree, "🌳 Tree View");
        ui.selectable_value(&mut state.json_view_mode, JsonViewMode::Formatted, "📝 Formatted");
        ui.selectable_value(&mut state.json_view_mode, JsonViewMode::Minified, "📦 Minified");

        ui.separator();

        if state.json_view_mode == JsonViewMode::Tree {
            if ui.button("⊞ Expand All").clicked() {
                state.json_collapsed_paths.clear();
            }
            if ui.button("⊟ Collapse All").clicked() {
                if let Some(json_val) = &state.json_parsed {
                    collect_all_expandable_paths(json_val, "$", &mut state.json_collapsed_paths);
                }
            }

            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut state.json_search_query)
                    .hint_text("🔍 Search keys / values...")
                    .desired_width(180.0),
            );
            if !state.json_search_query.is_empty() && ui.button("✖").clicked() {
                state.json_search_query.clear();
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("📋 Copy Formatted").clicked() {
                if let Some(formatted) = &state.json_formatted_cache {
                    *action_copy_text = Some(formatted.clone());
                }
            }
            if ui.button("📋 Copy Minified").clicked() {
                if let Some(min) = &state.json_minified_cache {
                    *action_copy_text = Some(min.clone());
                }
            }
        });
    });

    ui.add_space(6.0);

    let Some(json_val) = state.json_parsed.clone() else { return; };

    match state.json_view_mode {
        JsonViewMode::Tree => {
            egui::ScrollArea::both()
                .id_salt("json_tree_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    render_json_node(
                        ui,
                        "$",
                        None,
                        &json_val,
                        0,
                        &mut state.json_collapsed_paths,
                        &state.json_search_query,
                        action_copy_text,
                    );
                });
        }
        JsonViewMode::Formatted => {
            if let Some(formatted) = &state.json_formatted_cache {
                egui::ScrollArea::both()
                    .id_salt("json_formatted_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut formatted.as_str())
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                    });
            }
        }
        JsonViewMode::Minified => {
            if let Some(minified) = &state.json_minified_cache {
                egui::ScrollArea::both()
                    .id_salt("json_minified_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut minified.as_str())
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                    });
            }
        }
    }
}

/// Recursive JSON Node Renderer
fn render_json_node(
    ui: &mut egui::Ui,
    path: &str,
    key_name: Option<&str>,
    value: &JsonValue,
    depth: usize,
    collapsed_paths: &mut HashSet<String>,
    search_query: &str,
    action_copy_text: &mut Option<String>,
) {
    let dark = ui.visuals().dark_mode;
    let indent = (depth as f32) * 16.0;

    let matches_search = if search_query.is_empty() {
        false
    } else {
        let q = search_query.to_lowercase();
        key_name.map(|k| k.to_lowercase().contains(&q)).unwrap_or(false)
            || value.to_string().to_lowercase().contains(&q)
    };

    match value {
        JsonValue::Object(map) => {
            let is_collapsed = collapsed_paths.contains(path);
            let icon = if is_collapsed { "▶ 📁" } else { "▼ 📂" };
            let count_label = format!("{{ {} keys }}", map.len());

            ui.horizontal(|ui| {
                ui.add_space(indent);
                let toggle_resp = ui.selectable_label(
                    false,
                    egui::RichText::new(icon).size(12.0),
                );
                if toggle_resp.clicked() {
                    if is_collapsed {
                        collapsed_paths.remove(path);
                    } else {
                        collapsed_paths.insert(path.to_string());
                    }
                }

                if let Some(k) = key_name {
                    let key_text = egui::RichText::new(format!("\"{}\": ", k))
                        .color(if matches_search { egui::Color32::from_rgb(255, 215, 0) } else if dark { egui::Color32::from_rgb(130, 185, 255) } else { egui::Color32::from_rgb(0, 80, 190) })
                        .strong()
                        .monospace();
                    ui.label(key_text);
                }

                ui.label(egui::RichText::new(count_label).size(11.0).color(ui.visuals().weak_text_color()).monospace());

                // Copy sub-tree button on hover
                if ui.small_button("📋").on_hover_text("Copy sub-tree JSON").clicked() {
                    if let Ok(s) = serde_json::to_string_pretty(value) {
                        *action_copy_text = Some(s);
                    }
                }
            });

            if !is_collapsed {
                for (child_key, child_val) in map {
                    let child_path = format!("{}.{}", path, child_key);
                    render_json_node(
                        ui,
                        &child_path,
                        Some(child_key.as_str()),
                        child_val,
                        depth + 1,
                        collapsed_paths,
                        search_query,
                        action_copy_text,
                    );
                }
            }
        }
        JsonValue::Array(arr) => {
            let is_collapsed = collapsed_paths.contains(path);
            let icon = if is_collapsed { "▶ 📋" } else { "▼ 📑" };
            let count_label = format!("[ {} items ]", arr.len());

            ui.horizontal(|ui| {
                ui.add_space(indent);
                let toggle_resp = ui.selectable_label(
                    false,
                    egui::RichText::new(icon).size(12.0),
                );
                if toggle_resp.clicked() {
                    if is_collapsed {
                        collapsed_paths.remove(path);
                    } else {
                        collapsed_paths.insert(path.to_string());
                    }
                }

                if let Some(k) = key_name {
                    let key_text = egui::RichText::new(format!("\"{}\": ", k))
                        .color(if matches_search { egui::Color32::from_rgb(255, 215, 0) } else if dark { egui::Color32::from_rgb(130, 185, 255) } else { egui::Color32::from_rgb(0, 80, 190) })
                        .strong()
                        .monospace();
                    ui.label(key_text);
                }

                ui.label(egui::RichText::new(count_label).size(11.0).color(ui.visuals().weak_text_color()).monospace());

                if ui.small_button("📋").on_hover_text("Copy array JSON").clicked() {
                    if let Ok(s) = serde_json::to_string_pretty(value) {
                        *action_copy_text = Some(s);
                    }
                }
            });

            if !is_collapsed {
                for (idx, item) in arr.iter().enumerate() {
                    let child_path = format!("{}[{}]", path, idx);
                    let idx_label = format!("[{}]", idx);
                    render_json_node(
                        ui,
                        &child_path,
                        Some(&idx_label),
                        item,
                        depth + 1,
                        collapsed_paths,
                        search_query,
                        action_copy_text,
                    );
                }
            }
        }
        _ => {
            // Leaf Primitive Values (String, Number, Bool, Null)
            ui.horizontal(|ui| {
                ui.add_space(indent + 18.0);

                if let Some(k) = key_name {
                    let key_text = egui::RichText::new(format!("\"{}\": ", k))
                        .color(if matches_search { egui::Color32::from_rgb(255, 215, 0) } else if dark { egui::Color32::from_rgb(140, 190, 255) } else { egui::Color32::from_rgb(0, 80, 190) })
                        .strong()
                        .monospace();
                    ui.label(key_text);
                }

                let val_text = match value {
                    JsonValue::String(s) => {
                        let color = if dark { egui::Color32::from_rgb(150, 225, 150) } else { egui::Color32::from_rgb(30, 130, 40) };
                        egui::RichText::new(format!("\"{}\"", s)).color(color).monospace()
                    }
                    JsonValue::Number(n) => {
                        let color = if dark { egui::Color32::from_rgb(240, 170, 110) } else { egui::Color32::from_rgb(180, 80, 10) };
                        egui::RichText::new(n.to_string()).color(color).monospace()
                    }
                    JsonValue::Bool(b) => {
                        let color = if *b { egui::Color32::from_rgb(100, 200, 255) } else { egui::Color32::from_rgb(255, 120, 120) };
                        egui::RichText::new(b.to_string()).color(color).monospace().strong()
                    }
                    JsonValue::Null => {
                        let color = egui::Color32::from_rgb(160, 160, 160);
                        egui::RichText::new("null").color(color).italics().monospace()
                    }
                    _ => unreachable!(),
                };

                let val_resp = ui.label(val_text);
                if val_resp.clicked() {
                    let copy_val = match value {
                        JsonValue::String(s) => s.clone(),
                        _ => value.to_string(),
                    };
                    *action_copy_text = Some(copy_val);
                }
            });
        }
    }
}

/// Helper to recursively collect all expandable paths for "Collapse All"
fn collect_all_expandable_paths(val: &JsonValue, path: &str, out: &mut HashSet<String>) {
    match val {
        JsonValue::Object(map) => {
            out.insert(path.to_string());
            for (k, v) in map {
                collect_all_expandable_paths(v, &format!("{}.{}", path, k), out);
            }
        }
        JsonValue::Array(arr) => {
            out.insert(path.to_string());
            for (idx, item) in arr.iter().enumerate() {
                collect_all_expandable_paths(item, &format!("{}[{}]", path, idx), out);
            }
        }
        _ => {}
    }
}

// ─── TAB 2: Hex Editor / Binary ─────────────────────────────────────────────

fn render_tab_hex(
    state: &mut CellInspectorState,
    ui: &mut egui::Ui,
    _ctx: &egui::Context,
    action_copy_text: &mut Option<String>,
) {
    // Hex Sub-Toolbar
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;

        ui.label(egui::RichText::new("Decode Mode:").strong().size(12.0));
        let prev_mode = state.hex_decode_mode;
        egui::ComboBox::from_id_salt("hex_decode_mode_combo")
            .selected_text(state.hex_decode_mode.label())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.hex_decode_mode, HexDecodeMode::Auto, HexDecodeMode::Auto.label());
                ui.selectable_value(&mut state.hex_decode_mode, HexDecodeMode::RawUtf8Bytes, HexDecodeMode::RawUtf8Bytes.label());
                ui.selectable_value(&mut state.hex_decode_mode, HexDecodeMode::Base64, HexDecodeMode::Base64.label());
                ui.selectable_value(&mut state.hex_decode_mode, HexDecodeMode::HexString, HexDecodeMode::HexString.label());
            });

        if prev_mode != state.hex_decode_mode {
            state.recompute_hex_bytes();
        }

        ui.separator();
        ui.label("Bytes/Row:");
        ui.selectable_value(&mut state.hex_bytes_per_row, 8, "8");
        ui.selectable_value(&mut state.hex_bytes_per_row, 16, "16");
        ui.selectable_value(&mut state.hex_bytes_per_row, 32, "32");

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("📋 Copy Hex String").on_hover_text("Copy as '0x...' hex string").clicked() {
                let hex_str = format!("0x{}", hex::encode(&state.hex_bytes));
                *action_copy_text = Some(hex_str);
            }
            if ui.button("📋 Copy Base64").clicked() {
                use base64::Engine;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&state.hex_bytes);
                *action_copy_text = Some(b64);
            }
            if ui.button("📋 Copy C/Rust Array").on_hover_text("Copy as &[0x00, 0x01, ...] byte array").clicked() {
                let arr: Vec<String> = state.hex_bytes.iter().map(|b| format!("0x{:02X}", b)).collect();
                *action_copy_text = Some(format!("&[{}]", arr.join(", ")));
            }
        });
    });

    ui.add_space(6.0);

    let total_bytes = state.hex_bytes.len();
    if total_bytes == 0 {
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.label(egui::RichText::new("Empty byte payload").size(13.0).color(ui.visuals().weak_text_color()));
        });
        return;
    }

    let bytes_per_row = state.hex_bytes_per_row.max(8);
    let total_rows = (total_bytes + bytes_per_row - 1) / bytes_per_row;
    let row_height = 20.0;

    // Header column labels
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(" Offset   ").monospace().strong().color(ui.visuals().weak_text_color()));
        ui.add_space(8.0);

        let mut header_hex = String::new();
        for i in 0..bytes_per_row {
            header_hex.push_str(&format!("{:02X} ", i));
            if (i + 1) % 8 == 0 && (i + 1) < bytes_per_row {
                header_hex.push_str(" ");
            }
        }
        ui.label(egui::RichText::new(header_hex).monospace().strong().color(ui.visuals().weak_text_color()));
        ui.add_space(14.0);
        ui.label(egui::RichText::new("Decoded ASCII").monospace().strong().color(ui.visuals().weak_text_color()));
    });
    ui.separator();

    let bytes_slice = state.hex_bytes.clone();
    let dark = ui.visuals().dark_mode;

    egui::ScrollArea::vertical()
        .id_salt("hex_table_scroll")
        .auto_shrink([false, false])
        .show_rows(ui, row_height, total_rows, |ui, row_range| {
            for row_idx in row_range {
                let start_offset = row_idx * bytes_per_row;
                let end_offset = (start_offset + bytes_per_row).min(total_bytes);
                let row_bytes = &bytes_slice[start_offset..end_offset];

                ui.horizontal(|ui| {
                    // 1. Offset Column (e.g. 00000010:)
                    let offset_str = format!("{:08X}: ", start_offset);
                    ui.label(
                        egui::RichText::new(offset_str)
                            .monospace()
                            .color(if dark { egui::Color32::from_rgb(110, 140, 180) } else { egui::Color32::from_rgb(50, 80, 140) })
                    );

                    ui.add_space(6.0);

                    // 2. Hex Bytes Column
                    let mut hex_part = String::new();
                    for (i, b) in row_bytes.iter().enumerate() {
                        hex_part.push_str(&format!("{:02X} ", b));
                        if (i + 1) % 8 == 0 && (i + 1) < bytes_per_row {
                            hex_part.push_str(" ");
                        }
                    }
                    // Pad remaining if last line is short
                    if row_bytes.len() < bytes_per_row {
                        let missing = bytes_per_row - row_bytes.len();
                        for i in 0..missing {
                            hex_part.push_str("   ");
                            if (row_bytes.len() + i + 1) % 8 == 0 && (row_bytes.len() + i + 1) < bytes_per_row {
                                hex_part.push_str(" ");
                            }
                        }
                    }

                    ui.label(egui::RichText::new(hex_part).monospace().color(ui.visuals().text_color()));

                    ui.add_space(12.0);

                    // 3. ASCII Representation Column
                    let mut ascii_part = String::new();
                    for b in row_bytes {
                        if *b >= 32 && *b <= 126 {
                            ascii_part.push(*b as char);
                        } else {
                            ascii_part.push('·');
                        }
                    }
                    ui.label(
                        egui::RichText::new(ascii_part)
                            .monospace()
                            .color(if dark { egui::Color32::from_rgb(160, 210, 160) } else { egui::Color32::from_rgb(40, 120, 50) })
                    );
                });
            }
        });
}

// ─── TAB 3: Image Preview ───────────────────────────────────────────────────

fn render_tab_image(
    state: &mut CellInspectorState,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    action_copy_text: &mut Option<String>,
) {
    if let Some(err) = &state.image_error {
        ui.vertical_centered(|ui| {
            ui.add_space(30.0);
            ui.label(egui::RichText::new("🖼 No Image Detected").size(15.0).strong());
            ui.add_space(6.0);
            ui.label(egui::RichText::new(err).size(12.0).color(ui.visuals().weak_text_color()));
            ui.add_space(16.0);
            ui.label("Inspector supports PNG, JPEG, WebP, GIF, BMP, ICO, and SVG vector formats.");
            ui.add_space(10.0);
            if ui.button("Switch to Raw Hex/Binary").clicked() {
                state.active_tab = InspectorTab::Hex;
            }
        });
        return;
    }

    let Some(meta) = state.image_meta.clone() else { return; };

    // Image Toolbar
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;

        crate::window_egui::style::render_badge(ui, &meta.format_name, egui::Color32::from_rgb(25, 55, 95), egui::Color32::from_rgb(160, 215, 255));
        let dim_str = format!("{} × {} px", meta.width, meta.height);
        crate::window_egui::style::render_badge(ui, &dim_str, egui::Color32::from_rgb(40, 45, 55), egui::Color32::from_rgb(220, 225, 235));
        let size_str = format!("{:.2} KB", (meta.byte_size as f32) / 1024.0);
        crate::window_egui::style::render_badge(ui, &size_str, egui::Color32::from_rgb(40, 45, 55), egui::Color32::from_rgb(220, 225, 235));

        ui.separator();

        if ui.button("🔍-").on_hover_text("Zoom out").clicked() {
            state.image_zoom = (state.image_zoom - 0.25).max(0.25);
            state.image_fit_window = false;
        }
        if ui.button("100%").clicked() {
            state.image_zoom = 1.0;
            state.image_fit_window = false;
        }
        if ui.button("🔍+").on_hover_text("Zoom in").clicked() {
            state.image_zoom = (state.image_zoom + 0.25).min(5.0);
            state.image_fit_window = false;
        }
        ui.selectable_value(&mut state.image_fit_window, true, "📐 Fit Window");

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if meta.is_svg {
                if ui.button("📋 Copy SVG XML").clicked() {
                    if let Some(xml) = &meta.svg_xml {
                        *action_copy_text = Some(xml.clone());
                    }
                }
            } else {
                if ui.button("📋 Copy Data URI").clicked() {
                    use base64::Engine;
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&state.hex_bytes);
                    let uri = format!("data:image/png;base64,{}", b64);
                    *action_copy_text = Some(uri);
                }
            }
        });
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);

    // SVG Special Display
    if meta.is_svg {
        if let Some(svg_xml) = &meta.svg_xml {
            ui.label(egui::RichText::new("SVG XML Source Code & Structure:").strong());
            ui.add_space(4.0);
            egui::ScrollArea::both()
                .id_salt("svg_xml_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut svg_xml.as_str())
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .interactive(false),
                    );
                });
            return;
        }
    }

    // Raster Image Texture Loader
    if state.image_texture.is_none() && !state.hex_bytes.is_empty() {
        if let Ok(dyn_img) = image::load_from_memory(&state.hex_bytes) {
            let rgba = dyn_img.to_rgba8();
            let (w, h) = (rgba.width() as usize, rgba.height() as usize);
            let color_image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba.into_raw());
            state.image_texture = Some(ctx.load_texture(
                "cell_inspector_image_preview",
                color_image,
                egui::TextureOptions::LINEAR,
            ));
        }
    }

    // Render Image Texture inside viewport
    if let Some(texture) = &state.image_texture {
        egui::ScrollArea::both()
            .id_salt("image_preview_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    let orig_size = texture.size_vec2();
                    let target_size = if state.image_fit_window {
                        let avail = ui.available_size() - egui::vec2(20.0, 20.0);
                        let scale = (avail.x / orig_size.x).min(avail.y / orig_size.y).min(1.0);
                        orig_size * scale.max(0.1)
                    } else {
                        orig_size * state.image_zoom
                    };

                    // Subtle background box for transparency
                    let (rect, _) = ui.allocate_exact_size(target_size, egui::Sense::hover());
                    ui.painter().rect_filled(
                        rect,
                        4.0,
                        if ui.visuals().dark_mode { egui::Color32::from_rgb(30, 32, 40) } else { egui::Color32::from_rgb(240, 242, 246) }
                    );

                    let image_widget = egui::Image::new((texture.id(), target_size));
                    ui.put(rect, image_widget);
                });
            });
    }
}

// ─── TAB 4: Raw Virtual Text ────────────────────────────────────────────────

fn render_tab_raw_text(
    state: &mut CellInspectorState,
    ui: &mut egui::Ui,
    _ctx: &egui::Context,
    action_copy_text: &mut Option<String>,
) {
    // Text Sub-Toolbar
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;

        ui.add(
            egui::TextEdit::singleline(&mut state.text_search_query)
                .hint_text("🔍 Find in text...")
                .desired_width(200.0),
        );
        if !state.text_search_query.is_empty() && ui.button("✖").clicked() {
            state.text_search_query.clear();
        }

        ui.separator();
        ui.checkbox(&mut state.text_show_line_numbers, "Line Numbers");
        ui.checkbox(&mut state.text_word_wrap, "Word Wrap");

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("📋 Copy All Text").clicked() {
                *action_copy_text = Some(state.raw_value.clone());
            }
        });
    });

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);

    let total_lines = state.text_lines_cache.len();
    if total_lines == 0 {
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.label(egui::RichText::new("Empty cell text").size(13.0).color(ui.visuals().weak_text_color()));
        });
        return;
    }

    let row_height = 18.0;
    let dark = ui.visuals().dark_mode;
    let search_q = state.text_search_query.to_lowercase();
    let show_ln = state.text_show_line_numbers;
    let lines_ref = &state.text_lines_cache;

    egui::ScrollArea::both()
        .id_salt("raw_virtual_text_scroll")
        .auto_shrink([false, false])
        .show_rows(ui, row_height, total_lines, |ui, row_range| {
            for line_idx in row_range {
                if let Some(line_content) = lines_ref.get(line_idx) {
                    ui.horizontal(|ui| {
                        if show_ln {
                            let gutter = format!("{:>5} ", line_idx + 1);
                            ui.label(
                                egui::RichText::new(gutter)
                                    .monospace()
                                    .size(12.0)
                                    .color(if dark { egui::Color32::from_rgb(100, 110, 130) } else { egui::Color32::from_rgb(160, 170, 190) })
                            );
                        }

                        let matches = !search_q.is_empty() && line_content.to_lowercase().contains(&search_q);

                        let text_style = egui::RichText::new(line_content)
                            .monospace()
                            .size(12.0)
                            .color(if matches {
                                egui::Color32::from_rgb(255, 220, 80)
                            } else {
                                ui.visuals().text_color()
                            });

                        if matches {
                            ui.label(text_style.strong());
                        } else {
                            ui.label(text_style);
                        }
                    });
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inspector_open_json_auto_detection() {
        let mut state = CellInspectorState::default();
        let json_sample = r#"{"id": 101, "name": "Tabular Engine", "tags": ["rust", "database"]}"#;

        state.open(json_sample.to_string(), "metadata".to_string(), 0, 1);

        assert_eq!(state.active_tab, InspectorTab::Json);
        assert!(state.json_parsed.is_some());
        assert!(state.json_parse_error.is_none());
        assert_eq!(state.column_name, "metadata");
        assert_eq!(state.row_idx, Some(0));
        assert_eq!(state.col_idx, Some(1));
    }

    #[test]
    fn test_inspector_hex_bytes_and_ascii() {
        let mut state = CellInspectorState::default();
        let raw = "Hello Tabular!";
        state.open(raw.to_string(), "name".to_string(), 2, 3);

        assert_eq!(state.hex_bytes, b"Hello Tabular!");
        assert_eq!(state.text_total_chars, 14);
        assert_eq!(state.text_total_bytes, 14);
    }

    #[test]
    fn test_base64_and_hex_decoders() {
        use base64::Engine;
        let original = b"Binary Payload 12345";
        let b64 = base64::engine::general_purpose::STANDARD.encode(original);

        let decoded_b64 = try_decode_base64(&b64);
        assert_eq!(decoded_b64, Some(original.to_vec()));

        let hex_str = "0x48656c6c6f"; // "Hello"
        let decoded_hex = try_decode_hex_str(hex_str);
        assert_eq!(decoded_hex, Some(b"Hello".to_vec()));
    }

    #[test]
    fn test_image_format_detection_png() {
        let png_bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
        let detected = detect_image_format_magic(png_bytes);
        assert_eq!(detected, Some("PNG (Portable Network Graphics)"));
    }

    #[test]
    fn test_svg_detection() {
        let mut state = CellInspectorState::default();
        let svg_sample = r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><circle cx="50" cy="50" r="40" fill="red"/></svg>"#;

        state.open(svg_sample.to_string(), "icon".to_string(), 0, 0);

        assert_eq!(state.active_tab, InspectorTab::Image);
        assert!(state.image_meta.is_some());
        let meta = state.image_meta.unwrap();
        assert!(meta.is_svg);
        assert_eq!(meta.width, 200);
        assert_eq!(meta.height, 100);
    }
}
