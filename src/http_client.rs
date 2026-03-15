use std::sync::{Arc, Mutex, mpsc};
use eframe::egui;
use crate::models::structs::{
    HttpClientState, HttpClientResponse,
    HttpMethod, HttpBodyType, HttpAuthType, HttpRequestTab, HttpResponseTab,
};

// ─── Persistence ─────────────────────────────────────────────────────────────

pub fn save_http_state(connection_id: i64, state: &HttpClientState) {
    let dir = crate::directory::get_app_data_dir().join("http_state");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(format!("{}.json", connection_id));
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(path, json);
    }
}

pub fn load_http_state(connection_id: i64) -> Option<HttpClientState> {
    let path = crate::directory::get_app_data_dir()
        .join("http_state")
        .join(format!("{}.json", connection_id));
    let json = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&json).ok()
}

// ─── Public entry-point called from window_egui ─────────────────────────────

pub fn render_http_client(ui: &mut egui::Ui, state: &mut HttpClientState) {
    // Default red accent: selected/active controls use rgb(255,0,0) with white text.
    ui.style_mut().visuals.selection.bg_fill = egui::Color32::from_rgb(255, 0, 0);
    ui.style_mut().visuals.selection.stroke.color = egui::Color32::WHITE;

    // Poll background thread for a completed response
    if state.is_loading {
        let received: Option<HttpClientResponse> = state
            .response_receiver
            .as_ref()
            .and_then(|rx| rx.try_lock().ok()?.try_recv().ok());

        if let Some(resp) = received {
            apply_response(state, resp);
        }
    }

    ui.vertical(|ui| {
        render_url_bar(ui, state);
        ui.add_space(4.0);

        // Horizontal split: request left, response right
        let available = ui.available_size();
        let left_w = (available.x * 0.5).max(300.0).min(available.x - 260.0);

        ui.horizontal(|ui| {
            // ── LEFT: Request panel ──────────────────────────────────────
            ui.vertical(|ui| {
                ui.set_width(left_w);
                ui.set_min_height(available.y);
                render_request_panel(ui, state);
            });

            ui.separator();

            // ── RIGHT: Response panel ────────────────────────────────────
            ui.vertical(|ui| {
                ui.set_min_height(available.y);
                render_response_panel(ui, state);
            });
        });
    });
}

// ─── URL bar ────────────────────────────────────────────────────────────────

fn render_url_bar(ui: &mut egui::Ui, state: &mut HttpClientState) {
    ui.horizontal(|ui| {
        let send_w = 88.0;

        // Method selector
        egui::ComboBox::from_id_salt("http_method_combo")
            .width(90.0)
            .selected_text(state.method.label())
            .show_ui(ui, |ui| {
                for method in [
                    HttpMethod::GET,
                    HttpMethod::POST,
                    HttpMethod::PUT,
                    HttpMethod::DELETE,
                    HttpMethod::PATCH,
                    HttpMethod::HEAD,
                    HttpMethod::OPTIONS,
                ] {
                    let label = method.label();
                    ui.selectable_value(&mut state.method, method, label);
                }
            });

        // URL input
        let url_resp = ui.add(
            egui::TextEdit::singleline(&mut state.url)
                .hint_text("https://api.example.com/endpoint")
                .desired_width((ui.available_width() - send_w - 8.0).max(120.0)),
        );

        // SEND button
        let send_label = if state.is_loading {
            "⏳ Sending"
        } else {
            "▶  Send"
        };
        let send_btn = ui.add_enabled(
            !state.is_loading && !state.url.is_empty(),
            egui::Button::new(egui::RichText::new(send_label).color(egui::Color32::WHITE))
                .fill(egui::Color32::from_rgb(255, 0, 0))
                .min_size(egui::vec2(send_w - 10.0, 0.0)),
        );
        if send_btn.clicked() {
            execute_request(state);
        }

        // Allow pressing Enter in the URL field to send
        if url_resp.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
            && !state.is_loading
            && !state.url.is_empty()
        {
            execute_request(state);
        }
    });
}

// ─── Request panel (tabs + content) ─────────────────────────────────────────

fn render_request_panel(ui: &mut egui::Ui, state: &mut HttpClientState) {
    // Tab bar
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.active_tab, HttpRequestTab::Body, "Body");
        ui.selectable_value(&mut state.active_tab, HttpRequestTab::Params, "Params");

        let header_count = state.headers.iter().filter(|(_, _, en)| *en).count();
        let headers_label = if header_count > 0 {
            format!("Headers ({})", header_count)
        } else {
            "Headers".to_string()
        };
        ui.selectable_value(&mut state.active_tab, HttpRequestTab::Headers, headers_label);
        ui.selectable_value(&mut state.active_tab, HttpRequestTab::Auth, "Auth");
    });

    ui.separator();

    // Text-body editors fill the remaining height without a scroll area wrapper
    // (the TextEdit widget itself handles internal scrolling)
    let is_text_body = matches!(state.active_tab, HttpRequestTab::Body)
        && matches!(
            state.body_type,
            HttpBodyType::Json | HttpBodyType::Xml | HttpBodyType::GraphQL | HttpBodyType::OtherText
        );

    if is_text_body {
        render_body_panel(ui, state);
    } else {
        egui::ScrollArea::vertical()
            .id_salt("http_request_scroll")
            .show(ui, |ui| {
                match state.active_tab.clone() {
                    HttpRequestTab::Body => render_body_panel(ui, state),
                    HttpRequestTab::Params => render_kv_table(ui, &mut state.params, "http_params"),
                    HttpRequestTab::Headers => {
                        render_kv_table(ui, &mut state.headers, "http_headers")
                    }
                    HttpRequestTab::Auth => render_auth_panel(ui, state),
                }
            });
    }
}

// ─── Body panel ─────────────────────────────────────────────────────────────

fn render_body_panel(ui: &mut egui::Ui, state: &mut HttpClientState) {
    // Body type selector row
    ui.horizontal_wrapped(|ui| {
        ui.label("Form Data:");
        ui.selectable_value(
            &mut state.body_type,
            HttpBodyType::UrlEncoded,
            "Url Encoded",
        );
        ui.selectable_value(&mut state.body_type, HttpBodyType::MultiPart, "Multi-Part");

        ui.separator();
        ui.label("Text:");
        ui.selectable_value(&mut state.body_type, HttpBodyType::GraphQL, "GraphQL");
        ui.selectable_value(&mut state.body_type, HttpBodyType::Json, "JSON");
        ui.selectable_value(&mut state.body_type, HttpBodyType::Xml, "XML");
        ui.selectable_value(&mut state.body_type, HttpBodyType::OtherText, "Other");

        ui.separator();
        ui.label("Other:");
        ui.selectable_value(&mut state.body_type, HttpBodyType::BinaryFile, "Binary File");
        ui.selectable_value(&mut state.body_type, HttpBodyType::NoBody, "No Body");
    });

    ui.separator();

    match &state.body_type {
        HttpBodyType::NoBody => {
            ui.colored_label(
                ui.style().visuals.weak_text_color(),
                "No body will be sent with this request.",
            );
        }
        HttpBodyType::UrlEncoded | HttpBodyType::MultiPart => {
            render_kv_table(ui, &mut state.form_data, "http_form_data");
        }
        HttpBodyType::BinaryFile => {
            ui.colored_label(
                ui.style().visuals.weak_text_color(),
                "Binary file upload is not yet supported.",
            );
        }
        // Text-based body types
        _ => {
            let hint = match state.body_type {
                HttpBodyType::Json => "{ \"key\": \"value\" }",
                HttpBodyType::GraphQL => "{ query { ... } }",
                HttpBodyType::Xml => "<root></root>",
                _ => "",
            };

            let can_beautify = matches!(
                state.body_type,
                HttpBodyType::Json | HttpBodyType::GraphQL | HttpBodyType::Xml
            );
            // ── Editor filling remaining space ──────────────────────────
            let editor_h = ui.available_height().max(80.0);
            let editor_w = ui.available_width();

            let dark = ui.visuals().dark_mode;
            let body_type_cap = state.body_type.clone();
            let mut layouter = move |ui: &egui::Ui,
                                     buf: &dyn egui::TextBuffer,
                                     wrap_width: f32| {
                let s = buf.as_str();
                let font_id =
                    ui.style().text_styles[&egui::TextStyle::Monospace].clone();
                let mut job = match &body_type_cap {
                    HttpBodyType::Json => highlight_body_json(s, dark, font_id),
                    HttpBodyType::GraphQL => highlight_body_graphql(s, dark, font_id),
                    HttpBodyType::Xml => highlight_body_xml(s, dark, font_id),
                    _ => {
                        let col = if dark {
                            egui::Color32::from_rgb(220, 220, 220)
                        } else {
                            egui::Color32::from_rgb(30, 30, 30)
                        };
                        let mut j = egui::text::LayoutJob::default();
                        j.append(
                            s,
                            0.0,
                            egui::TextFormat {
                                font_id,
                                color: col,
                                ..Default::default()
                            },
                        );
                        j
                    }
                };
                job.wrap.max_width = wrap_width;
                ui.fonts(|f| f.layout_job(job))
            };

            let editor_resp = ui.add_sized(
                [editor_w, editor_h],
                egui::TextEdit::multiline(&mut state.body_text)
                    .hint_text(hint)
                    .desired_width(f32::INFINITY)
                    .layouter(&mut layouter),
            );

            if can_beautify {
                let ctx = ui.ctx().clone();
                let rect = editor_resp.rect;
                egui::Area::new(egui::Id::new("http_req_beautify_overlay"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(egui::pos2(rect.right() - 28.0, rect.bottom() - 28.0))
                    .show(&ctx, |ui| {
                    let btn = ui.add_sized(
                        [22.0, 22.0],
                        egui::Button::new(egui::RichText::new("⚡").color(egui::Color32::WHITE))
                            .fill(egui::Color32::from_rgb(255, 0, 0)),
                    );
                    if btn.clicked() {
                        match state.body_type {
                            HttpBodyType::Json | HttpBodyType::GraphQL => {
                                if let Some(pretty) = beautify_json(&state.body_text) {
                                    state.body_text = pretty;
                                }
                            }
                            HttpBodyType::Xml => {
                                let pretty = beautify_xml(&state.body_text);
                                if !pretty.is_empty() {
                                    state.body_text = pretty;
                                }
                            }
                            _ => {}
                        }
                    }
                    btn.on_hover_text("Beautify body");
                });
            }
        }
    }
}

// ─── Key-Value table (params / headers / form data) ─────────────────────────

fn render_kv_table(
    ui: &mut egui::Ui,
    rows: &mut Vec<(String, String, bool)>,
    id: &str,
) {
    let mut to_remove: Vec<usize> = Vec::new();
    let spacing = ui.spacing().item_spacing.x;
    let checkbox_w = 20.0;
    let del_btn_w = 20.0;
    let total_w = ui.available_width();
    // Two fields share the space left after checkbox, delete button, and 3 gaps
    let field_w = ((total_w - checkbox_w - del_btn_w - spacing * 3.0) * 0.5).max(60.0);

    // Header row
    ui.horizontal(|ui| {
        ui.add_space(checkbox_w + spacing);
        ui.add_sized([field_w, ui.spacing().interact_size.y], egui::Label::new(egui::RichText::new("Key").strong()));
        ui.add_sized([field_w, ui.spacing().interact_size.y], egui::Label::new(egui::RichText::new("Value").strong()));
    });
    ui.separator();

    let _ = id;
    for (idx, (key, value, enabled)) in rows.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.checkbox(enabled, "");
            ui.add(egui::TextEdit::singleline(key).desired_width(field_w).hint_text("key"));
            ui.add(egui::TextEdit::singleline(value).desired_width(field_w).hint_text("value"));
            if ui.small_button("✕").clicked() {
                to_remove.push(idx);
            }
        });
    }

    for idx in to_remove.iter().rev() {
        rows.remove(*idx);
    }

    if ui.small_button("+ Add row").clicked() {
        rows.push(("".to_string(), "".to_string(), true));
    }
}

// ─── Auth panel ─────────────────────────────────────────────────────────────

fn render_auth_panel(ui: &mut egui::Ui, state: &mut HttpClientState) {
    // Auth type selector
    ui.horizontal_wrapped(|ui| {
        ui.label("Type:");
        for (auth, label) in [
            (HttpAuthType::NoAuth, "No Auth"),
            (HttpAuthType::InheritParent, "Inherit from Parent"),
            (HttpAuthType::BearerToken, "Bearer Token"),
            (HttpAuthType::BasicAuth, "Basic Auth"),
            (HttpAuthType::ApiKey, "API Key"),
            (HttpAuthType::JwtBearer, "JWT Bearer"),
            (HttpAuthType::OAuth1, "OAuth 1.0"),
            (HttpAuthType::OAuth2, "OAuth 2.0"),
            (HttpAuthType::AwsSignature, "AWS Signature"),
            (HttpAuthType::NtlmAuth, "NTLM Auth"),
        ] {
            ui.selectable_value(&mut state.auth_type, auth, label);
        }
    });

    ui.separator();

    match &state.auth_type {
        HttpAuthType::NoAuth => {
            ui.colored_label(
                ui.style().visuals.weak_text_color(),
                "No authentication will be used.",
            );
        }
        HttpAuthType::InheritParent => {
            ui.colored_label(
                ui.style().visuals.weak_text_color(),
                "Auth settings will be inherited from the parent collection.",
            );
        }
        HttpAuthType::BearerToken | HttpAuthType::JwtBearer => {
            ui.label("Token:");
            ui.add(
                egui::TextEdit::singleline(&mut state.bearer_token)
                    .hint_text("Bearer token or JWT string")
                    .desired_width(f32::INFINITY)
                    .password(true),
            );
        }
        HttpAuthType::BasicAuth => {
            egui::Grid::new("http_basic_auth")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Username:");
                    ui.add(
                        egui::TextEdit::singleline(&mut state.basic_user)
                            .hint_text("username")
                            .desired_width(250.0),
                    );
                    ui.end_row();

                    ui.label("Password:");
                    ui.add(
                        egui::TextEdit::singleline(&mut state.basic_pass)
                            .hint_text("password")
                            .password(true)
                            .desired_width(250.0),
                    );
                    ui.end_row();
                });
        }
        HttpAuthType::ApiKey => {
            egui::Grid::new("http_api_key_auth")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Key Name:");
                    ui.add(
                        egui::TextEdit::singleline(&mut state.api_key_name)
                            .hint_text("X-API-Key")
                            .desired_width(250.0),
                    );
                    ui.end_row();

                    ui.label("Key Value:");
                    ui.add(
                        egui::TextEdit::singleline(&mut state.api_key_value)
                            .hint_text("your-api-key")
                            .password(true)
                            .desired_width(250.0),
                    );
                    ui.end_row();

                    ui.label("Add to:");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut state.api_key_in_header, true, "Header");
                        ui.radio_value(&mut state.api_key_in_header, false, "Query Param");
                    });
                    ui.end_row();
                });
        }
        HttpAuthType::AwsSignature
        | HttpAuthType::OAuth1
        | HttpAuthType::OAuth2
        | HttpAuthType::NtlmAuth => {
            ui.colored_label(
                ui.style().visuals.weak_text_color(),
                format!("{:?} authentication is not yet implemented.", state.auth_type),
            );
        }
    }
}

// ─── Response panel ──────────────────────────────────────────────────────────

fn render_response_panel(ui: &mut egui::Ui, state: &mut HttpClientState) {
    if state.is_loading {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.spinner();
            ui.label("Sending request…");
        });
        return;
    }

    if state.response_status.is_none() && state.response_error.is_none() {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.colored_label(
                ui.style().visuals.weak_text_color(),
                "Enter a URL and press Send to receive a response.",
            );
        });
        return;
    }

    // Status bar
    ui.horizontal(|ui| {
        if let Some(err) = &state.response_error {
            ui.colored_label(egui::Color32::from_rgb(220, 60, 60), format!("Error: {}", err));
        } else if let Some(status) = state.response_status {
            let color = if status < 300 {
                egui::Color32::from_rgb(50, 205, 50)
            } else if status < 400 {
                egui::Color32::from_rgb(255, 200, 0)
            } else {
                egui::Color32::from_rgb(220, 60, 60)
            };
            ui.colored_label(color, format!("Status: {} {}", status, state.response_status_text));
        }

        if let Some(ms) = state.response_time_ms {
            ui.separator();
            ui.label(format!("Time: {}ms", ms));
        }

        if let Some(bytes) = state.response_size_bytes {
            ui.separator();
            let size_str = if bytes >= 1024 * 1024 {
                format!("Size: {:.1} MB", bytes as f64 / (1024.0 * 1024.0))
            } else if bytes >= 1024 {
                format!("Size: {:.1} KB", bytes as f64 / 1024.0)
            } else {
                format!("Size: {} B", bytes)
            };
            ui.label(size_str);
        }
    });

    ui.separator();

    // Response tab bar
    ui.horizontal(|ui| {
        ui.selectable_value(&mut state.response_tab, HttpResponseTab::Body, "Body");
        ui.selectable_value(&mut state.response_tab, HttpResponseTab::Headers, "Headers");
    });

    ui.separator();

    match state.response_tab {
        HttpResponseTab::Body => {
            // ── Detect content-type from response headers ─────────────
            let content_type = state
                .response_headers
                .iter()
                .find(|(k, _)| k.to_lowercase() == "content-type")
                .map(|(_, v)| v.to_lowercase())
                .unwrap_or_default();
            let is_json = content_type.contains("json");
            let is_xml  = content_type.contains("xml") || content_type.contains("html");

            // ── Syntax-highlighted editor, fills remaining height ──────
            let dark = ui.visuals().dark_mode;
            let mut layouter = move |ui: &egui::Ui,
                                     buf: &dyn egui::TextBuffer,
                                     wrap_width: f32| {
                let s = buf.as_str();
                let font_id =
                    ui.style().text_styles[&egui::TextStyle::Monospace].clone();
                let mut job = if is_json {
                    highlight_body_json(s, dark, font_id)
                } else if is_xml {
                    highlight_body_xml(s, dark, font_id)
                } else {
                    let col = if dark {
                        egui::Color32::from_rgb(220, 220, 220)
                    } else {
                        egui::Color32::from_rgb(30, 30, 30)
                    };
                    let mut j = egui::text::LayoutJob::default();
                    j.append(
                        s,
                        0.0,
                        egui::TextFormat {
                            font_id,
                            color: col,
                            ..Default::default()
                        },
                    );
                    j
                };
                job.wrap.max_width = wrap_width;
                ui.fonts(|f| f.layout_job(job))
            };

            let h = ui.available_height().max(80.0);
            let w = ui.available_width();
            let editor_resp = ui.add_sized(
                [w, h],
                egui::TextEdit::multiline(&mut state.response_body)
                    .desired_width(f32::INFINITY)
                    .interactive(true)
                    .layouter(&mut layouter),
            );

            if is_json || is_xml {
                let ctx = ui.ctx().clone();
                let rect = editor_resp.rect;
                egui::Area::new(egui::Id::new("http_resp_beautify_overlay"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(egui::pos2(rect.right() - 28.0, rect.bottom() - 28.0))
                    .show(&ctx, |ui| {
                    let btn = ui.add_sized(
                        [22.0, 22.0],
                        egui::Button::new(egui::RichText::new("⚡").color(egui::Color32::WHITE))
                            .fill(egui::Color32::from_rgb(255, 0, 0)),
                    );
                    if btn.clicked() {
                        if is_json {
                            if let Some(pretty) = beautify_json(&state.response_body) {
                                state.response_body = pretty;
                            }
                        } else {
                            let pretty = beautify_xml(&state.response_body);
                            if !pretty.is_empty() {
                                state.response_body = pretty;
                            }
                        }
                    }
                    btn.on_hover_text("Beautify response body");
                });
            }
        }
        HttpResponseTab::Headers => {
            egui::ScrollArea::both()
                .id_salt("http_response_headers_scroll")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    if state.response_headers.is_empty() {
                        ui.colored_label(
                            ui.style().visuals.weak_text_color(),
                            "No headers received.",
                        );
                    } else {
                        egui::Grid::new("resp_headers_grid")
                            .num_columns(2)
                            .spacing([8.0, 2.0])
                            .striped(true)
                            .show(ui, |ui| {
                                for (k, v) in &state.response_headers {
                                    ui.label(egui::RichText::new(k).monospace().strong());
                                    ui.label(egui::RichText::new(v).monospace());
                                    ui.end_row();
                                }
                            });
                    }
                });
        }
    }
}

// ─── HTTP execution ──────────────────────────────────────────────────────────

fn execute_request(state: &mut HttpClientState) {
    state.is_loading = true;
    state.response_status = None;
    state.response_status_text.clear();
    state.response_body.clear();
    state.response_headers.clear();
    state.response_time_ms = None;
    state.response_size_bytes = None;
    state.response_error = None;

    let (tx, rx) = mpsc::channel::<HttpClientResponse>();
    state.response_receiver = Some(Arc::new(Mutex::new(rx)));

    // Gather all request data before moving into thread
    let url = state.url.clone();
    let method = state.method.clone();
    let body_type = state.body_type.clone();
    let body_text = state.body_text.clone();
    let form_data: Vec<(String, String)> = state
        .form_data
        .iter()
        .filter(|(k, _, en)| *en && !k.is_empty())
        .map(|(k, v, _)| (k.clone(), v.clone()))
        .collect();
    let params: Vec<(String, String)> = state
        .params
        .iter()
        .filter(|(k, _, en)| *en && !k.is_empty())
        .map(|(k, v, _)| (k.clone(), v.clone()))
        .collect();
    let custom_headers: Vec<(String, String)> = state
        .headers
        .iter()
        .filter(|(k, _, en)| *en && !k.is_empty())
        .map(|(k, v, _)| (k.clone(), v.clone()))
        .collect();
    let auth_type = state.auth_type.clone();
    let bearer_token = state.bearer_token.clone();
    let basic_user = state.basic_user.clone();
    let basic_pass = state.basic_pass.clone();
    let api_key_name = state.api_key_name.clone();
    let api_key_value = state.api_key_value.clone();
    let api_key_in_header = state.api_key_in_header;

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async move {
            let client = reqwest::Client::builder()
                .danger_accept_invalid_certs(false)
                .build()
                .unwrap_or_default();

            let start = std::time::Instant::now();

            // Build URL with query params
            let mut full_url = url.clone();
            if !params.is_empty() {
                let query_str: String = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join("&");
                if full_url.contains('?') {
                    full_url.push('&');
                } else {
                    full_url.push('?');
                }
                full_url.push_str(&query_str);
            }

            // Add API key to URL if needed
            if matches!(auth_type, HttpAuthType::ApiKey) && !api_key_in_header {
                let q = format!("{}={}", api_key_name, api_key_value);
                if full_url.contains('?') {
                    full_url.push('&');
                } else {
                    full_url.push('?');
                }
                full_url.push_str(&q);
            }

            let mut req_builder = match method {
                HttpMethod::GET => client.get(&full_url),
                HttpMethod::POST => client.post(&full_url),
                HttpMethod::PUT => client.put(&full_url),
                HttpMethod::DELETE => client.delete(&full_url),
                HttpMethod::PATCH => client.patch(&full_url),
                HttpMethod::HEAD => client.head(&full_url),
                HttpMethod::OPTIONS => client.request(reqwest::Method::OPTIONS, &full_url),
            };

            // Custom headers
            for (k, v) in &custom_headers {
                req_builder = req_builder.header(k.as_str(), v.as_str());
            }

            // Auth headers
            match auth_type {
                HttpAuthType::BearerToken | HttpAuthType::JwtBearer => {
                    req_builder =
                        req_builder.header("Authorization", format!("Bearer {}", bearer_token));
                }
                HttpAuthType::BasicAuth => {
                    req_builder = req_builder.basic_auth(&basic_user, Some(&basic_pass));
                }
                HttpAuthType::ApiKey => {
                    if api_key_in_header && !api_key_name.is_empty() {
                        req_builder = req_builder.header(api_key_name.as_str(), api_key_value.as_str());
                    }
                }
                _ => {}
            }

            // Body
            req_builder = match &body_type {
                HttpBodyType::Json => req_builder
                    .header("Content-Type", "application/json")
                    .body(body_text.clone()),
                HttpBodyType::Xml => req_builder
                    .header("Content-Type", "application/xml")
                    .body(body_text.clone()),
                HttpBodyType::GraphQL => req_builder
                    .header("Content-Type", "application/json")
                    .body(body_text.clone()),
                HttpBodyType::OtherText => req_builder.body(body_text.clone()),
                HttpBodyType::UrlEncoded => {
                    req_builder.form(&form_data)
                }
                HttpBodyType::MultiPart => {
                    let mut form = reqwest::multipart::Form::new();
                    for (k, v) in form_data {
                        form = form.text(k, v);
                    }
                    req_builder.multipart(form)
                }
                HttpBodyType::NoBody | HttpBodyType::BinaryFile => req_builder,
            };

            match req_builder.send().await {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let status_text = response
                        .status()
                        .canonical_reason()
                        .unwrap_or("")
                        .to_string();
                    let resp_headers: Vec<(String, String)> = response
                        .headers()
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.to_string(),
                                v.to_str().unwrap_or("<binary>").to_string(),
                            )
                        })
                        .collect();
                    let body = response.text().await.unwrap_or_default();
                    let time_ms = start.elapsed().as_millis();
                    let size_bytes = body.len();
                    HttpClientResponse {
                        status,
                        status_text,
                        body,
                        headers: resp_headers,
                        time_ms,
                        size_bytes,
                        error: None,
                    }
                }
                Err(e) => {
                    let time_ms = start.elapsed().as_millis();
                    HttpClientResponse {
                        status: 0,
                        status_text: String::new(),
                        body: String::new(),
                        headers: Vec::new(),
                        time_ms,
                        size_bytes: 0,
                        error: Some(e.to_string()),
                    }
                }
            }
        });

        let _ = tx.send(result);
    });
}

fn apply_response(state: &mut HttpClientState, resp: HttpClientResponse) {
    state.is_loading = false;
    state.response_receiver = None;
    if let Some(err) = resp.error {
        state.response_error = Some(err);
        state.response_status = None;
    } else {
        state.response_error = None;
        state.response_status = Some(resp.status);
        state.response_status_text = resp.status_text;
        state.response_body = maybe_beautify_json_response(&resp.headers, &resp.body)
            .unwrap_or(resp.body);
        state.response_headers = resp.headers;
    }
    state.response_time_ms = Some(resp.time_ms);
    state.response_size_bytes = Some(resp.size_bytes);
}

fn maybe_beautify_json_response(headers: &[(String, String)], body: &str) -> Option<String> {
    let content_type_is_json = headers.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("content-type") && v.to_ascii_lowercase().contains("json")
    });

    if content_type_is_json {
        return beautify_json(body);
    }

    // Fallback for servers that return JSON without a proper content-type header.
    let trimmed = body.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return beautify_json(trimmed);
    }

    None
}

// ─── Beautify helpers ─────────────────────────────────────────────────────────

/// Pretty-print a JSON string. Returns `None` if parsing fails.
fn beautify_json(input: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(input.trim()).ok()?;
    serde_json::to_string_pretty(&value).ok()
}

/// Pretty-print an XML string with 2-space indentation.
fn beautify_xml(input: &str) -> String {
    // ── tokenise into tags and text nodes ──────────────────────────────
    let mut tokens: Vec<String> = Vec::new();
    let mut remaining = input.trim();

    while !remaining.is_empty() {
        if remaining.starts_with('<') {
            let end = xml_tag_end(remaining);
            tokens.push(remaining[..end].to_string());
            remaining = remaining[end..].trim_start();
        } else {
            let end = remaining.find('<').unwrap_or(remaining.len());
            let text = remaining[..end].trim();
            if !text.is_empty() {
                tokens.push(text.to_string());
            }
            remaining = &remaining[end..];
        }
    }

    // ── rebuild with indentation ───────────────────────────────────────
    let mut output = String::new();
    let mut depth: i32 = 0;
    const IND: &str = "  ";

    for (i, token) in tokens.iter().enumerate() {
        if token.starts_with('<') {
            let tag_upper = token.to_ascii_uppercase();
            let is_close = token.starts_with("</");
            let is_self_close = token.ends_with("/>")
                || token.starts_with("<?")
                || token.starts_with("<!--")
                || tag_upper.starts_with("<!D"); // DOCTYPE

            if is_close {
                depth = (depth - 1).max(0);
                // Keep closing tag on the same line when previous token was text
                let prev_is_text = i > 0 && !tokens[i - 1].starts_with('<');
                if prev_is_text {
                    output.push_str(token);
                } else {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    for _ in 0..depth {
                        output.push_str(IND);
                    }
                    output.push_str(token);
                }
            } else {
                // Opening / self-closing / PI / comment
                if !output.is_empty() {
                    output.push('\n');
                }
                for _ in 0..depth {
                    output.push_str(IND);
                }
                output.push_str(token);
                if !is_self_close {
                    depth += 1;
                }
            }
        } else {
            // Text content – always appended inline after its opening tag
            output.push_str(token);
        }
    }

    output.trim().to_string()
}

/// Find the byte-offset just past the closing `>` of one XML tag.
fn xml_tag_end(input: &str) -> usize {
    if input.starts_with("<!--") {
        return input.find("-->").map(|p| p + 3).unwrap_or(input.len());
    }
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 1;
    let mut in_quote = false;
    let mut quote_char = b'"';
    while i < len {
        if in_quote {
            if bytes[i] == quote_char {
                in_quote = false;
            }
        } else {
            match bytes[i] {
                b'"' | b'\'' => {
                    in_quote = true;
                    quote_char = bytes[i];
                }
                b'>' => return i + 1,
                _ => {}
            }
        }
        i += 1;
    }
    len
}

// ─── HTTP Body Syntax Highlighting ───────────────────────────────────────────

/// JSON syntax highlighter.
/// Colors: cyan = keys, green = string values, orange = numbers,
///         purple = true/false/null, gray = punctuation.
fn highlight_body_json(
    text: &str,
    dark: bool,
    font_id: egui::FontId,
) -> egui::text::LayoutJob {
    use egui::{text::LayoutJob, Color32, TextFormat};
    let mut job = LayoutJob::default();

    let key_col   = Color32::from_rgb(130, 200, 255); // cyan   – keys
    let str_col   = Color32::from_rgb(152, 195, 121); // green  – string values
    let num_col   = Color32::from_rgb(209, 154, 102); // orange – numbers
    let kw_col    = Color32::from_rgb(198, 120, 221); // purple – true/false/null
    let punct_col = Color32::from_rgb(171, 178, 191); // gray   – brackets/commas
    let norm_col  = if dark { Color32::from_rgb(220, 220, 220) } else { Color32::from_rgb(30, 30, 30) };

    macro_rules! tf {
        ($c:expr) => {
            TextFormat { font_id: font_id.clone(), color: $c, ..Default::default() }
        };
    }

    let bs = text.as_bytes();
    let n  = bs.len();
    let mut i = 0;

    while i < n {
        match bs[i] {
            // ── double-quoted string ────────────────────────────────────
            b'"' => {
                let start = i;
                i += 1;
                while i < n {
                    if bs[i] == b'\\' { i += 2; continue; }
                    if bs[i] == b'"'  { i += 1; break; }
                    i += 1;
                }
                // look-ahead: if next non-ws char is ':', this is an object key
                let mut k = i;
                while k < n && bs[k].is_ascii_whitespace() { k += 1; }
                let color = if k < n && bs[k] == b':' { key_col } else { str_col };
                if let Some(s) = text.get(start..i) { job.append(s, 0.0, tf!(color)); }
            }
            // ── positive number ─────────────────────────────────────────
            b'0'..=b'9' => {
                let start = i;
                while i < n && (bs[i].is_ascii_digit() || bs[i] == b'.' || bs[i] == b'e' || bs[i] == b'E') { i += 1; }
                if let Some(s) = text.get(start..i) { job.append(s, 0.0, tf!(num_col)); }
            }
            // ── negative number ─────────────────────────────────────────
            b'-' if i + 1 < n && bs[i + 1].is_ascii_digit() => {
                let start = i;
                i += 1;
                while i < n && (bs[i].is_ascii_digit() || bs[i] == b'.' || bs[i] == b'e' || bs[i] == b'E') { i += 1; }
                if let Some(s) = text.get(start..i) { job.append(s, 0.0, tf!(num_col)); }
            }
            // ── keyword (true / false / null) ───────────────────────────
            b'a'..=b'z' | b'A'..=b'Z' => {
                let start = i;
                while i < n && bs[i].is_ascii_alphanumeric() { i += 1; }
                let word = text.get(start..i).unwrap_or("");
                let col  = if matches!(word, "true" | "false" | "null") { kw_col } else { norm_col };
                job.append(word, 0.0, tf!(col));
            }
            // ── structural punctuation ──────────────────────────────────
            b'{' | b'}' | b'[' | b']' | b':' | b',' => {
                if let Some(s) = text.get(i..i + 1) { job.append(s, 0.0, tf!(punct_col)); }
                i += 1;
            }
            // ── whitespace / other ──────────────────────────────────────
            _ => {
                let start = i;
                i += 1;
                while i < n && matches!(bs[i], b' ' | b'\t' | b'\n' | b'\r') { i += 1; }
                if let Some(s) = text.get(start..i) { job.append(s, 0.0, tf!(norm_col)); }
            }
        }
    }
    job
}

/// XML syntax highlighter.
/// Colors: blue = tag names, light-blue = attr names, green = attr values,
///         gray = punctuation, muted-green = comments, yellow = CDATA,
///         purple = processing instructions.
fn highlight_body_xml(
    text: &str,
    dark: bool,
    font_id: egui::FontId,
) -> egui::text::LayoutJob {
    use egui::{text::LayoutJob, Color32, TextFormat};
    let mut job = LayoutJob::default();

    let tag_col      = Color32::from_rgb( 86, 156, 214); // blue        – tag names
    let attr_key_col = Color32::from_rgb(146, 202, 245); // light blue  – attr names
    let attr_val_col = Color32::from_rgb(152, 195, 121); // green       – attr values
    let punct_col    = Color32::from_rgb(171, 178, 191); // gray        – <, >, /, =
    let comment_col  = Color32::from_rgb(106, 153,  85); // muted green – comments
    let cdata_col    = Color32::from_rgb(220, 220, 170); // pale yellow – CDATA
    let pi_col       = Color32::from_rgb(198, 120, 221); // purple      – <?...?>
    let norm_col     = if dark { Color32::from_rgb(220, 220, 220) } else { Color32::from_rgb(30, 30, 30) };

    macro_rules! tf {
        ($c:expr) => {
            TextFormat { font_id: font_id.clone(), color: $c, ..Default::default() }
        };
    }

    let bs = text.as_bytes();
    let n  = bs.len();
    let mut i = 0;

    while i < n {
        if bs[i] != b'<' {
            // ── text content ─────────────────────────────────────────────
            let start = i;
            while i < n && bs[i] != b'<' { i += 1; }
            if let Some(s) = text.get(start..i) {
                if !s.is_empty() { job.append(s, 0.0, tf!(norm_col)); }
            }
            continue;
        }

        // ── comment ───────────────────────────────────────────────────
        if text[i..].starts_with("<!--") {
            let start = i;
            i += 4;
            while i < n {
                if text[i..].starts_with("-->") { i += 3; break; }
                i += 1;
            }
            if let Some(s) = text.get(start..i) { job.append(s, 0.0, tf!(comment_col)); }
            continue;
        }

        // ── CDATA ────────────────────────────────────────────────────
        if text[i..].starts_with("<![CDATA[") {
            let start = i;
            i += 9;
            while i < n {
                if text[i..].starts_with("]]>") { i += 3; break; }
                i += 1;
            }
            if let Some(s) = text.get(start..i) { job.append(s, 0.0, tf!(cdata_col)); }
            continue;
        }

        // ── regular tag ───────────────────────────────────────────────
        job.append("<", 0.0, tf!(punct_col));
        i += 1;

        let is_pi      = i < n && bs[i] == b'?';
        let is_closing = i < n && bs[i] == b'/';
        if is_closing || is_pi {
            if let Some(s) = text.get(i..i + 1) { job.append(s, 0.0, tf!(punct_col)); }
            i += 1;
        }

        // tag name
        let name_start = i;
        while i < n && !bs[i].is_ascii_whitespace() && bs[i] != b'>' && bs[i] != b'/' && bs[i] != b'?' { i += 1; }
        if let Some(name) = text.get(name_start..i) {
            if !name.is_empty() {
                job.append(name, 0.0, tf!(if is_pi { pi_col } else { tag_col }));
            }
        }

        // attributes
        while i < n && bs[i] != b'>' {
            if bs[i].is_ascii_whitespace() {
                let s = i;
                while i < n && bs[i].is_ascii_whitespace() { i += 1; }
                if let Some(ws) = text.get(s..i) { job.append(ws, 0.0, tf!(norm_col)); }
            } else if bs[i] == b'/' || bs[i] == b'?' {
                if let Some(s) = text.get(i..i + 1) { job.append(s, 0.0, tf!(punct_col)); }
                i += 1;
            } else if bs[i] == b'=' {
                job.append("=", 0.0, tf!(punct_col));
                i += 1;
            } else if bs[i] == b'"' || bs[i] == b'\'' {
                let q = bs[i];
                let s = i;
                i += 1;
                while i < n && bs[i] != q { i += 1; }
                if i < n { i += 1; }
                if let Some(slice) = text.get(s..i) { job.append(slice, 0.0, tf!(attr_val_col)); }
            } else {
                let s = i;
                while i < n && bs[i] != b'=' && bs[i] != b'>' && !bs[i].is_ascii_whitespace() && bs[i] != b'/' { i += 1; }
                if let Some(name) = text.get(s..i) {
                    if !name.is_empty() { job.append(name, 0.0, tf!(attr_key_col)); }
                }
            }
        }

        if i < n && bs[i] == b'>' {
            job.append(">", 0.0, tf!(punct_col));
            i += 1;
        }
    }
    job
}

/// GraphQL syntax highlighter.
/// Colors: purple = keywords, green = strings, muted-green = comments,
///         orange = types (uppercase), cyan = fields, gray = punctuation.
fn highlight_body_graphql(
    text: &str,
    dark: bool,
    font_id: egui::FontId,
) -> egui::text::LayoutJob {
    use egui::{text::LayoutJob, Color32, TextFormat};
    let mut job = LayoutJob::default();

    let kw_col      = Color32::from_rgb(198, 120, 221); // purple
    let str_col     = Color32::from_rgb(152, 195, 121); // green
    let comment_col = Color32::from_rgb(106, 153,  85); // muted green
    let type_col    = Color32::from_rgb(230, 180,  80); // orange  – TYPE names
    let field_col   = Color32::from_rgb(130, 200, 255); // cyan    – field names
    let num_col     = Color32::from_rgb(209, 154, 102); // orange  – numbers
    let punct_col   = Color32::from_rgb(171, 178, 191); // gray
    let norm_col    = if dark { Color32::from_rgb(220, 220, 220) } else { Color32::from_rgb(30, 30, 30) };

    macro_rules! tf {
        ($c:expr) => {
            TextFormat { font_id: font_id.clone(), color: $c, ..Default::default() }
        };
    }

    let bs = text.as_bytes();
    let n  = bs.len();
    let mut i = 0;

    while i < n {
        match bs[i] {
            // ── line comment ────────────────────────────────────────────
            b'#' => {
                let start = i;
                while i < n && bs[i] != b'\n' { i += 1; }
                if let Some(s) = text.get(start..i) { job.append(s, 0.0, tf!(comment_col)); }
            }
            // ── triple-quoted or regular string ─────────────────────────
            b'"' => {
                let start = i;
                if text[i..].starts_with("\"\"\"") {
                    i += 3;
                    while i < n {
                        if text[i..].starts_with("\"\"\"") { i += 3; break; }
                        i += 1;
                    }
                } else {
                    i += 1;
                    while i < n {
                        if bs[i] == b'\\' { i += 2; continue; }
                        if bs[i] == b'"'  { i += 1; break; }
                        i += 1;
                    }
                }
                if let Some(s) = text.get(start..i) { job.append(s, 0.0, tf!(str_col)); }
            }
            // ── number ─────────────────────────────────────────────────
            b'0'..=b'9' => {
                let start = i;
                while i < n && (bs[i].is_ascii_digit() || bs[i] == b'.') { i += 1; }
                if let Some(s) = text.get(start..i) { job.append(s, 0.0, tf!(num_col)); }
            }
            b'-' if i + 1 < n && bs[i + 1].is_ascii_digit() => {
                let start = i;
                i += 1;
                while i < n && (bs[i].is_ascii_digit() || bs[i] == b'.') { i += 1; }
                if let Some(s) = text.get(start..i) { job.append(s, 0.0, tf!(num_col)); }
            }
            // ── identifier (keyword / type / field) ─────────────────────
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let start = i;
                while i < n && (bs[i].is_ascii_alphanumeric() || bs[i] == b'_') { i += 1; }
                let word = text.get(start..i).unwrap_or("");
                let col = if is_graphql_keyword(word) {
                    kw_col
                } else if word.starts_with(|c: char| c.is_ascii_uppercase()) {
                    type_col
                } else {
                    field_col
                };
                job.append(word, 0.0, tf!(col));
            }
            // ── punctuation ─────────────────────────────────────────────
            b'{' | b'}' | b'(' | b')' | b'[' | b']' | b':' | b',' | b'!' | b'@' | b'$' | b'.' => {
                if let Some(s) = text.get(i..i + 1) { job.append(s, 0.0, tf!(punct_col)); }
                i += 1;
            }
            // ── whitespace / other ──────────────────────────────────────
            _ => {
                let start = i;
                i += 1;
                while i < n && matches!(bs[i], b' ' | b'\t' | b'\n' | b'\r') { i += 1; }
                if let Some(s) = text.get(start..i) { job.append(s, 0.0, tf!(norm_col)); }
            }
        }
    }
    job
}

fn is_graphql_keyword(word: &str) -> bool {
    matches!(
        word,
        "query" | "mutation" | "subscription" | "fragment" | "on"
        | "type" | "interface" | "union" | "enum" | "input" | "extend"
        | "schema" | "scalar" | "directive" | "implements"
        | "true" | "false" | "null"
        | "if" | "include" | "skip" | "repeatable"
    )
}

