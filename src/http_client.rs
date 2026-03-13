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
                ui.set_min_height(available.y - 40.0);
                render_request_panel(ui, state);
            });

            ui.separator();

            // ── RIGHT: Response panel ────────────────────────────────────
            ui.vertical(|ui| {
                ui.set_min_height(available.y - 40.0);
                render_response_panel(ui, state);
            });
        });
    });
}

// ─── URL bar ────────────────────────────────────────────────────────────────

fn render_url_bar(ui: &mut egui::Ui, state: &mut HttpClientState) {
    ui.horizontal(|ui| {
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
                .desired_width(ui.available_width() - 80.0),
        );

        // SEND button
        let send_label = if state.is_loading {
            "⏳ Sending"
        } else {
            "▶  Send"
        };
        let send_btn = ui.add_enabled(
            !state.is_loading && !state.url.is_empty(),
            egui::Button::new(send_label).min_size(egui::vec2(80.0, 0.0)),
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

    egui::ScrollArea::vertical()
        .id_salt("http_request_scroll")
        .show(ui, |ui| {
            match &state.active_tab {
                HttpRequestTab::Body => render_body_panel(ui, state),
                HttpRequestTab::Params => render_kv_table(ui, &mut state.params, "http_params"),
                HttpRequestTab::Headers => {
                    render_kv_table(ui, &mut state.headers, "http_headers")
                }
                HttpRequestTab::Auth => render_auth_panel(ui, state),
            }
        });
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
            let content_type_hint = match state.body_type {
                HttpBodyType::Json => "application/json",
                HttpBodyType::GraphQL => "application/json",
                HttpBodyType::Xml => "application/xml",
                _ => "text/plain",
            };
            ui.add(
                egui::TextEdit::multiline(&mut state.body_text)
                    .hint_text(hint)
                    .desired_width(f32::INFINITY)
                    .desired_rows(12)
                    .font(egui::TextStyle::Monospace),
            );
            ui.label(
                egui::RichText::new(format!("Content-Type: {}", content_type_hint))
                    .small()
                    .italics()
                    .color(ui.style().visuals.weak_text_color()),
            );
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

    egui::Grid::new(id)
        .num_columns(4)
        .spacing([4.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            // Header row
            ui.label("");
            ui.label(egui::RichText::new("Key").strong());
            ui.label(egui::RichText::new("Value").strong());
            ui.label("");
            ui.end_row();

            for (idx, (key, value, enabled)) in rows.iter_mut().enumerate() {
                ui.checkbox(enabled, "");
                ui.add(
                    egui::TextEdit::singleline(key)
                        .desired_width(180.0)
                        .hint_text("key"),
                );
                ui.add(
                    egui::TextEdit::singleline(value)
                        .desired_width(200.0)
                        .hint_text("value"),
                );
                if ui.small_button("✕").clicked() {
                    to_remove.push(idx);
                }
                ui.end_row();
            }
        });

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

    egui::ScrollArea::both()
        .id_salt("http_response_scroll")
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            match state.response_tab {
                HttpResponseTab::Body => {
                    ui.add(
                        egui::TextEdit::multiline(&mut state.response_body)
                            .desired_width(f32::INFINITY)
                            .desired_rows(20)
                            .font(egui::TextStyle::Monospace)
                            .interactive(true),
                    );
                }
                HttpResponseTab::Headers => {
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
                }
            }
        });
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
        state.response_body = resp.body;
        state.response_headers = resp.headers;
    }
    state.response_time_ms = Some(resp.time_ms);
    state.response_size_bytes = Some(resp.size_bytes);
}
