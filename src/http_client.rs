use crate::models::structs::{
    CodeLang, HttpAuthType, HttpBodyType, HttpClientResponse, HttpClientState, HttpMethod,
    HttpRequestTab, HttpResponseTab,
};
use eframe::egui;
use std::sync::{Arc, Mutex, mpsc};

// ─── Persistence ─────────────────────────────────────────────────────────────

pub fn save_http_state(connection_id: i64, state: &HttpClientState) {
    let dir = crate::directory::get_app_data_dir().join("http_state");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }

    // Store auth secrets in the encrypted store; replace with sentinel in JSON.
    let mut persisted = state.clone();
    persisted.bearer_token = crate::secrets::store_or_keep(
        &crate::secrets::http_secret_name(connection_id, "bearer_token"),
        &state.bearer_token,
    );
    persisted.basic_pass = crate::secrets::store_or_keep(
        &crate::secrets::http_secret_name(connection_id, "basic_pass"),
        &state.basic_pass,
    );
    persisted.api_key_value = crate::secrets::store_or_keep(
        &crate::secrets::http_secret_name(connection_id, "api_key_value"),
        &state.api_key_value,
    );

    let path = dir.join(format!("{}.json", connection_id));
    let result = serde_json::to_string_pretty(&persisted)
        .map_err(|e| e.to_string())
        .and_then(|json| {
            crate::directory::write_file_atomically(&path, json.as_bytes())
                .map_err(|e| e.to_string())
        });
    if let Err(e) = result {
        log::error!(
            "Failed to save HTTP request state to {}: {}",
            path.display(),
            e
        );
    }
}

pub fn load_http_state(connection_id: i64) -> Option<HttpClientState> {
    let path = crate::directory::get_app_data_dir()
        .join("http_state")
        .join(format!("{}.json", connection_id));
    let json = std::fs::read_to_string(path).ok()?;
    let mut state: HttpClientState = serde_json::from_str(&json).ok()?;

    // Resolve secrets (handles sentinel values and migrates legacy plaintext).
    let (bt, bt_rewrite) = crate::secrets::resolve_stored(
        &crate::secrets::http_secret_name(connection_id, "bearer_token"),
        &state.bearer_token,
    );
    state.bearer_token = bt;

    let (bp, bp_rewrite) = crate::secrets::resolve_stored(
        &crate::secrets::http_secret_name(connection_id, "basic_pass"),
        &state.basic_pass,
    );
    state.basic_pass = bp;

    let (akv, akv_rewrite) = crate::secrets::resolve_stored(
        &crate::secrets::http_secret_name(connection_id, "api_key_value"),
        &state.api_key_value,
    );
    state.api_key_value = akv;

    // If any field was legacy plaintext, rewrite the JSON file with sentinels.
    if bt_rewrite.is_some() || bp_rewrite.is_some() || akv_rewrite.is_some() {
        save_http_state(connection_id, &state);
    }

    // Load saved workspaces (collections) from disk.
    state.workspaces = crate::http_collection::load_workspaces();

    Some(state)
}

// ─── Public entry-point called from window_egui ─────────────────────────────

/// Backend AI yang siap dipakai, atau alasan kenapa belum bisa dipakai
/// (ditampilkan apa adanya di UI).
pub type AiBackend = Result<crate::ai_assistant::ChatBackend, String>;

type Toasts = crate::window_egui::notifications::ToastManager;

/// Render the HTTP client panel.
/// Returns `true` if the user just saved a request to a collection workspace
/// (so the caller can reload `app.yaak_workspaces` from disk).
pub fn render_http_client(
    ui: &mut egui::Ui,
    state: &mut HttpClientState,
    toasts: &mut Toasts,
    connection_id: Option<i64>,
    ai: &AiBackend,
) -> bool {
    ui.style_mut().visuals.selection.bg_fill = crate::window_egui::style::theme_accent(ui.ctx());
    ui.style_mut().visuals.selection.stroke.color = egui::Color32::WHITE;

    // Poll background thread for a completed response
    if state.is_loading {
        let received: Option<HttpClientResponse> = state
            .response_receiver
            .as_ref()
            .and_then(|rx| rx.try_lock().ok()?.try_recv().ok());

        if let Some(resp) = received {
            apply_response(state, resp);
        } else {
            // Timer live di panel response butuh repaint berkala.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    if state.ai.is_busy() {
        if let Some((task, reply)) = state.ai.poll() {
            apply_ai_reply(state, task, reply, toasts);
        } else {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    // Cmd/Ctrl+Enter mengirim request dari mana pun di panel ini.
    if !state.show_save_dialog
        && !state.show_code_dialog
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
        && can_send(state)
    {
        execute_request(state);
    }

    let mut workspaces_saved = false;

    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            if render_url_bar(ui, state, toasts, connection_id) {
                workspaces_saved = true;
            }
            if state.ai.bar_open {
                ui.add_space(8.0);
                render_ai_bar(ui, state, ai);
            }
            ui.add_space(10.0);
            render_split_panels(ui, state, toasts, ai);
        });

    // Render the save dialog (outside the Frame so it can float as a Window)
    if render_save_dialog(ui, state, toasts, connection_id) {
        workspaces_saved = true;
    }

    // Render the "Copy as Code" dialog (outside the Frame so it can float as a Window)
    render_code_dialog(ui, state, toasts);

    workspaces_saved
}

fn can_send(state: &HttpClientState) -> bool {
    !state.is_loading && !state.url.trim().is_empty()
}

/// Label modifier shortcut sesuai platform (`⌘` ada di font Proportional).
fn mod_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl+"
    }
}

const ALL_METHODS: [HttpMethod; 7] = [
    HttpMethod::GET,
    HttpMethod::POST,
    HttpMethod::PUT,
    HttpMethod::PATCH,
    HttpMethod::DELETE,
    HttpMethod::HEAD,
    HttpMethod::OPTIONS,
];

// ─── Bantuan AI (agy / Claude Code / API) ──────────────────────────────────

/// Nama backend AI untuk ditampilkan ke user.
fn ai_backend_label(backend: &crate::ai_assistant::ChatBackend) -> &'static str {
    match backend.backend {
        crate::config::AiBackend::Api => backend.provider.display_name(),
        crate::config::AiBackend::Cli => backend.cli.kind.display_name(),
    }
}

fn ai_consent_id() -> egui::Id {
    egui::Id::new("http_ai_consent_v1")
}

/// User sudah menyetujui bahwa request (tanpa secret) dikirim ke backend AI.
fn ai_consent_given(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.get_persisted::<bool>(ai_consent_id()))
        .unwrap_or(false)
}

/// Pemberitahuan sekali pakai sebelum data request pertama kali dikirim ke AI.
/// Mengembalikan `true` bila user sudah setuju.
fn render_ai_consent(ui: &mut egui::Ui, backend_label: &str) -> bool {
    use crate::window_egui::style;
    if ai_consent_given(ui.ctx()) {
        return true;
    }
    let ctx = ui.ctx().clone();
    style::ai_notice_frame(style::theme_info(&ctx)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(
            egui::RichText::new(format!(
                "{}  The request (URL, headers, body) and response are sent to {backend_label}. \
Tokens, passwords, cookies and other secrets are replaced with {} first.",
                egui_icons::icons::ICON_SHIELD.codepoint,
                crate::http_ai::REDACTED
            ))
            .size(12.0),
        );
        ui.add_space(6.0);
        if ui.add(style::btn_secondary("Got it, continue")).clicked() {
            ctx.data_mut(|d| d.insert_persisted(ai_consent_id(), true));
        }
    });
    false
}

fn start_ai_task(
    state: &mut HttpClientState,
    task: crate::http_ai::HttpAiTask,
    backend: &crate::ai_assistant::ChatBackend,
) {
    let (system, user) = crate::http_ai::build_prompts(task, state, &state.ai.prompt);
    log::info!(
        "[HTTP] AI task '{}' via {}",
        task.label(),
        ai_backend_label(backend)
    );
    let rx = crate::ai_assistant::request_text(backend, system, user);
    state.ai.start(task, rx);
    if task == crate::http_ai::HttpAiTask::ExplainResponse {
        state.response_tab = HttpResponseTab::Ai;
    }
}

fn apply_ai_reply(
    state: &mut HttpClientState,
    task: crate::http_ai::HttpAiTask,
    reply: Result<String, String>,
    toasts: &mut Toasts,
) {
    use crate::http_ai::{self, HttpAiTask};
    let reply = match reply {
        Ok(text) => text,
        Err(e) => {
            log::warn!("[HTTP] AI task '{}' failed: {}", task.label(), e);
            state.ai.error = Some(e);
            return;
        }
    };
    match task {
        HttpAiTask::ExplainResponse => {
            state.ai.explanation = Some(reply.trim().to_string());
            state.response_tab = HttpResponseTab::Ai;
        }
        HttpAiTask::GenerateBody => match http_ai::extract_json(&reply) {
            Ok(json) => {
                state.body_type = HttpBodyType::Json;
                state.body_text = json;
                state.active_tab = HttpRequestTab::Body;
                state.ai.prompt.clear();
                toasts.success("Body generated by AI");
            }
            Err(e) => state.ai.error = Some(e),
        },
        HttpAiTask::BuildRequest => {
            let Some(curl) = http_ai::extract_curl(&reply) else {
                state.ai.error = Some("The AI reply did not contain a curl command".to_string());
                return;
            };
            match http_ai::apply_generated_curl(state, &curl) {
                Ok(warnings) => {
                    state.ai.prompt.clear();
                    toasts.success("Request built by AI. Review it, then press Send.");
                    for w in warnings {
                        toasts.warning(w);
                    }
                }
                Err(e) => state.ai.error = Some(format!("Could not apply the AI request: {e}")),
            }
        }
    }
}

/// Bar prompt AI di bawah URL bar: pilih tugas, tulis instruksi, jalankan.
fn render_ai_bar(ui: &mut egui::Ui, state: &mut HttpClientState, ai: &AiBackend) {
    use crate::http_ai::HttpAiTask;
    use crate::window_egui::style;
    let ctx = ui.ctx().clone();
    let muted = style::nav_text_muted(&ctx);

    egui::Frame::new()
        .fill(style::ai_surface(&ctx))
        .stroke(egui::Stroke::new(1.0, style::ai_border(&ctx)))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let backend = match ai {
                Ok(b) => b,
                Err(msg) => {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}  {msg}",
                                egui_icons::icons::ICON_INFO.codepoint
                            ))
                            .size(12.0)
                            .color(muted),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if style::ai_icon_button(
                                ui,
                                egui_icons::icons::ICON_CLOSE.codepoint,
                                "Close",
                            )
                            .clicked()
                            {
                                state.ai.bar_open = false;
                            }
                        });
                    });
                    return;
                }
            };
            let label = ai_backend_label(backend);
            if !render_ai_consent(ui, label) {
                return;
            }

            let busy = state.ai.is_busy();
            let mut run = false;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(
                    egui::RichText::new(egui_icons::icons::ICON_AUTO_AWESOME.codepoint)
                        .size(16.0)
                        .color(style::theme_accent(&ctx)),
                );
                for task in [HttpAiTask::BuildRequest, HttpAiTask::GenerateBody] {
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::selectable(
                                state.ai.task == task,
                                egui::RichText::new(task.label()).size(12.0),
                            ),
                        )
                        .clicked()
                    {
                        state.ai.task = task;
                    }
                }

                let right_w = 110.0;
                let hint = match state.ai.task {
                    HttpAiTask::GenerateBody => {
                        "e.g. create a user with 3 roles and a nested address"
                    }
                    _ => "e.g. get page 2 of permissions sorted by name",
                };
                let resp = style::render_text_field(
                    ui,
                    egui::TextEdit::singleline(&mut state.ai.prompt)
                        .id_salt("http_ai_prompt")
                        .hint_text(egui::RichText::new(hint).color(muted)),
                    (ui.available_width() - right_w).max(120.0),
                    None,
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    run = true;
                }

                if busy {
                    ui.add(egui::Spinner::new().size(16.0));
                    ui.label(egui::RichText::new("Thinking…").size(12.0).color(muted));
                } else {
                    let can_run = !state.ai.prompt.trim().is_empty();
                    if ui
                        .add_enabled(can_run, style::btn_field_action_primary(ui, "Generate"))
                        .clicked()
                    {
                        run = true;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if style::ai_icon_button(ui, egui_icons::icons::ICON_CLOSE.codepoint, "Close")
                        .clicked()
                    {
                        state.ai.bar_open = false;
                    }
                });
            });

            ui.add_space(2.0);
            match &state.ai.error {
                Some(err) if !busy => {
                    ui.label(
                        egui::RichText::new(err)
                            .size(11.5)
                            .color(style::theme_danger(&ctx)),
                    );
                }
                _ => {
                    ui.label(
                        egui::RichText::new(format!(
                            "via {label} · secrets are redacted before sending"
                        ))
                        .size(11.0)
                        .color(muted),
                    );
                }
            }

            if run && !busy && !state.ai.prompt.trim().is_empty() {
                let task = state.ai.task;
                start_ai_task(state, task, backend);
            }
        });
}

// ─── Layout: panel request/response dengan pembagi yang bisa digeser ─────────

fn render_split_panels(
    ui: &mut egui::Ui,
    state: &mut HttpClientState,
    toasts: &mut Toasts,
    ai: &AiBackend,
) {
    let ctx = ui.ctx().clone();
    let area = ui.available_rect_before_wrap();
    // Panel sempit otomatis ditumpuk supaya editor tidak terlalu kurus.
    let vertical = state.layout_vertical || area.width() < 760.0;
    let handle = 12.0;
    let ratio = state.split_ratio.clamp(0.2, 0.8);

    let (first, handle_rect, second) = if vertical {
        let h1 = ((area.height() - handle) * ratio).round();
        let first = egui::Rect::from_min_size(area.min, egui::vec2(area.width(), h1));
        let handle_rect = egui::Rect::from_min_size(
            egui::pos2(area.left(), first.bottom()),
            egui::vec2(area.width(), handle),
        );
        let second =
            egui::Rect::from_min_max(egui::pos2(area.left(), handle_rect.bottom()), area.max);
        (first, handle_rect, second)
    } else {
        let w1 = ((area.width() - handle) * ratio).round();
        let first = egui::Rect::from_min_size(area.min, egui::vec2(w1, area.height()));
        let handle_rect = egui::Rect::from_min_size(
            egui::pos2(first.right(), area.top()),
            egui::vec2(handle, area.height()),
        );
        let second =
            egui::Rect::from_min_max(egui::pos2(handle_rect.right(), area.top()), area.max);
        (first, handle_rect, second)
    };

    let resp = ui
        .interact(
            handle_rect,
            ui.id().with("http_split_handle"),
            egui::Sense::click_and_drag(),
        )
        .on_hover_cursor(if vertical {
            egui::CursorIcon::ResizeVertical
        } else {
            egui::CursorIcon::ResizeHorizontal
        })
        .on_hover_text("Drag to resize · double-click to reset");
    if resp.dragged()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let r = if vertical {
            (p.y - area.top()) / area.height().max(1.0)
        } else {
            (p.x - area.left()) / area.width().max(1.0)
        };
        state.split_ratio = r.clamp(0.2, 0.8);
    }
    if resp.double_clicked() {
        state.split_ratio = 0.5;
    }

    let active = resp.hovered() || resp.dragged();
    let line_color = if active {
        crate::window_egui::style::theme_accent(&ctx).gamma_multiply(0.8)
    } else {
        crate::window_egui::style::nav_border(&ctx)
    };
    let stroke = egui::Stroke::new(if active { 2.0 } else { 1.0 }, line_color);
    if vertical {
        ui.painter()
            .hline(handle_rect.x_range(), handle_rect.center().y, stroke);
    } else {
        ui.painter()
            .vline(handle_rect.center().x, handle_rect.y_range(), stroke);
    }

    let mut req_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("http_request_panel")
            .max_rect(first)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    req_ui.shrink_clip_rect(first);
    render_request_panel(&mut req_ui, state, toasts);

    let mut resp_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("http_response_panel")
            .max_rect(second)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    resp_ui.shrink_clip_rect(second);
    render_response_panel(&mut resp_ui, state, toasts, ai);

    ui.allocate_rect(area, egui::Sense::hover());
}

// ─── URL bar ────────────────────────────────────────────────────────────────

fn render_url_bar(
    ui: &mut egui::Ui,
    state: &mut HttpClientState,
    toasts: &mut crate::window_egui::notifications::ToastManager,
    connection_id: Option<i64>,
) -> bool {
    use crate::window_egui::style;

    let mut workspaces_saved = false;
    let ctx = ui.ctx().clone();

    let metrics = crate::window_egui::device_profile::DeviceUiMetrics::compute(
        ui.ctx(),
        crate::config::UiModePreference::Auto,
    );
    let bar_h = if metrics.is_touch { 40.0 } else { 34.0 };
    let method_w = if metrics.is_touch { 108.0 } else { 96.0 };
    // Send, Save, Code sengaja berukuran sama.
    let btn_w = if metrics.is_touch { 96.0 } else { 84.0 };
    let font_sz = if metrics.is_touch { 14.5 } else { 13.0 };
    let gap = 6.0;
    let muted = style::nav_text_muted(&ctx);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;

        // ── Field gabungan: [METHOD ▾ | URL] ──
        // Empat tombol berukuran sama: Send, Save, Code, AI.
        let field_w = (ui.available_width() - btn_w * 4.0 - gap * 4.0).max(200.0);
        let (field_rect, _) =
            ui.allocate_exact_size(egui::vec2(field_w, bar_h), egui::Sense::hover());
        let visuals = ui.visuals().clone();
        let painter = ui.painter().clone();
        painter.rect_filled(field_rect, 6.0, visuals.text_edit_bg_color());

        let method_rect = egui::Rect::from_min_size(field_rect.min, egui::vec2(method_w, bar_h));
        let method_resp = ui
            .interact(
                method_rect,
                ui.id().with("http_method_picker"),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("HTTP method");
        if method_resp.hovered() {
            painter.rect_filled(
                method_rect.shrink(3.0),
                4.0,
                style::nav_raised(&ctx).gamma_multiply(0.6),
            );
        }
        painter.text(
            method_rect.left_center() + egui::vec2(12.0, 0.0),
            egui::Align2::LEFT_CENTER,
            state.method.label(),
            egui::FontId::monospace(font_sz),
            style::http_method_color(&ctx, &state.method),
        );
        painter.text(
            method_rect.right_center() - egui::vec2(8.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            egui_icons::icons::ICON_EXPAND_MORE.codepoint,
            egui::FontId::proportional(16.0),
            muted,
        );
        painter.vline(
            method_rect.right(),
            egui::Rangef::new(method_rect.top() + 7.0, method_rect.bottom() - 7.0),
            egui::Stroke::new(1.0, style::nav_border(&ctx)),
        );

        egui::Popup::menu(&method_resp)
            .width(method_w + 30.0)
            .show(|ui| {
                for method in ALL_METHODS {
                    let label = egui::RichText::new(method.label())
                        .family(egui::FontFamily::Monospace)
                        .size(font_sz)
                        .color(style::http_method_color(ui.ctx(), &method));
                    if ui.selectable_label(state.method == method, label).clicked() {
                        state.method = method;
                    }
                }
            });

        let url_rect = egui::Rect::from_min_max(
            egui::pos2(method_rect.right() + 10.0, field_rect.top()),
            egui::pos2(field_rect.right() - 10.0, field_rect.bottom()),
        );
        let url_resp = ui.put(
            url_rect,
            egui::TextEdit::singleline(&mut state.url)
                .id_salt("http_url_field")
                .frame(egui::Frame::NONE)
                .font(egui::FontId::monospace(font_sz))
                .vertical_align(egui::Align::Center)
                .desired_width(f32::INFINITY)
                .hint_text(
                    egui::RichText::new(
                        "https://api.example.com/endpoint  or paste a cURL command",
                    )
                    .color(muted),
                ),
        );

        let border = if url_resp.has_focus() {
            visuals.widgets.active.bg_stroke.color
        } else if ui.rect_contains_pointer(field_rect) {
            visuals.widgets.hovered.bg_stroke.color
        } else {
            visuals.widgets.inactive.bg_stroke.color
        };
        painter.rect_stroke(
            field_rect,
            6.0,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );

        // Pasting a full curl command directly into the URL field auto-converts
        // it into the request form (method/headers/body/auth/params), instead
        // of leaving the raw curl text sitting in the URL field.
        if url_resp.changed() && crate::curl_import::looks_like_curl(&state.url) {
            let raw = state.url.clone();
            match crate::curl_import::apply_to_state(state, &raw) {
                Ok(warnings) => {
                    toasts.success("Imported request from cURL");
                    for w in warnings {
                        toasts.warning(w);
                    }
                }
                Err(e) => {
                    toasts.error(format!("cURL import failed: {e}"));
                }
            }
        }

        // ── SEND / CANCEL ──
        if state.is_loading {
            let cancel_btn = egui::Button::new(
                egui::RichText::new(format!(
                    "{}  Cancel",
                    egui_icons::icons::ICON_STOP.codepoint
                ))
                .size(font_sz)
                .color(style::nav_text_strong(&ctx)),
            )
            .fill(style::nav_raised(&ctx))
            .stroke(egui::Stroke::new(1.0, style::nav_border(&ctx)))
            .corner_radius(6.0);
            if ui
                .add_sized([btn_w, bar_h], cancel_btn)
                .on_hover_text("Cancel the running request")
                .clicked()
            {
                cancel_request(state, toasts);
            }
        } else {
            let send_btn = egui::Button::new(
                egui::RichText::new(format!("{}  Send", egui_icons::icons::ICON_SEND.codepoint))
                    .color(egui::Color32::WHITE)
                    .strong()
                    .size(font_sz),
            )
            .fill(style::theme_accent(&ctx))
            .corner_radius(6.0);
            let can = can_send(state);
            let send_resp = ui
                .add_enabled_ui(can, |ui| ui.add_sized([btn_w, bar_h], send_btn))
                .inner
                .on_hover_text(format!("Send request ({}Enter)", mod_key()));
            if send_resp.clicked() {
                execute_request(state);
            }
        }

        // ── SAVE ──
        let secondary = |label: String| {
            egui::Button::new(
                egui::RichText::new(label)
                    .size(font_sz)
                    .color(style::nav_text_strong(&ctx)),
            )
            .fill(style::nav_raised(&ctx))
            .stroke(egui::Stroke::new(1.0, style::nav_border(&ctx)))
            .corner_radius(6.0)
        };
        if ui
            .add_sized(
                [btn_w, bar_h],
                secondary(format!("{}  Save", egui_icons::icons::ICON_SAVE.codepoint)),
            )
            .on_hover_text(format!("Save / update request ({}S)", mod_key()))
            .clicked()
            && save_or_update_http_tab(connection_id, state, toasts)
        {
            workspaces_saved = true;
        }

        // ── CODE ──
        if ui
            .add_sized(
                [btn_w, bar_h],
                secondary(format!(
                    "{}  Code",
                    egui_icons::icons::MDI_CODE_BRACES.codepoint
                )),
            )
            .on_hover_text("Copy request as code (cURL, Python, Go, …)")
            .clicked()
        {
            state.show_code_dialog = true;
        }

        // ── AI ──
        let ai_label = format!("{}  AI", egui_icons::icons::ICON_AUTO_AWESOME.codepoint);
        let ai_btn = if state.ai.bar_open {
            secondary(ai_label).stroke(egui::Stroke::new(1.0, style::theme_accent(&ctx)))
        } else {
            secondary(ai_label)
        };
        if ui
            .add_sized([btn_w, bar_h], ai_btn)
            .on_hover_text("Build the request or generate a body with AI (agy, Claude Code, …)")
            .clicked()
        {
            state.ai.bar_open = !state.ai.bar_open;
        }

        // Allow pressing Enter in the URL field to send
        if url_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && can_send(state)
        {
            execute_request(state);
        }
    });

    workspaces_saved
}

/// Render the floating save-to-collection dialog.
/// Returns `true` the frame that a request was successfully persisted to disk
/// (so the caller can reload `app.yaak_workspaces` from disk into memory).
fn render_save_dialog(
    ui: &mut egui::Ui,
    state: &mut HttpClientState,
    toasts: &mut crate::window_egui::notifications::ToastManager,
    connection_id: Option<i64>,
) -> bool {
    if !state.show_save_dialog {
        return false;
    }

    let mut close = false;
    let mut save = false;

    // Always fetch fresh workspaces from disk so newly created or imported collections are immediately visible
    let mut workspaces = crate::http_collection::load_workspaces();
    if workspaces.is_empty() {
        let default_ws = crate::http_collection::create_workspace(&mut workspaces, "Collection");
        state.collection_panel.active_workspace_id = Some(default_ws.id);
    }
    state.workspaces = workspaces.clone();

    // Ensure active_workspace_id is set to a valid workspace
    let active_ws_valid = state
        .collection_panel
        .active_workspace_id
        .as_ref()
        .is_some_and(|id| state.workspaces.iter().any(|w| &w.id == id));
    if !active_ws_valid {
        if let Some(first) = state.workspaces.first() {
            state.collection_panel.active_workspace_id = Some(first.id.clone());
        }
    }

    crate::window_egui::style::render_modal_backdrop(
        ui.ctx(),
        "modal_save_request_backdrop",
        state.show_save_dialog,
    );

    egui::Window::new("Save Request to Collection")
        .title_bar(false)
        .frame(crate::window_egui::style::modal_window_frame(ui.ctx()))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(380.0)
        .show(ui.ctx(), |ui| {
            crate::window_egui::style::render_modal_header(
                ui,
                "Save Request to Collection",
                &mut close,
            );
            ui.add_space(8.0);

            crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Request Name:").strong());
                    crate::window_egui::style::render_text_field(
                        ui,
                        egui::TextEdit::singleline(&mut state.save_dialog_name)
                            .hint_text("e.g. Get User Profile"),
                        f32::INFINITY,
                        None,
                    );
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.label("Workspace:");
                        if state.workspaces.is_empty() {
                            ui.label(egui::RichText::new("Default Collection").weak());
                        } else {
                            let current_ws = state
                                .collection_panel
                                .active_workspace_id
                                .clone()
                                .or_else(|| state.workspaces.first().map(|w| w.id.clone()))
                                .unwrap_or_else(|| "default".to_string());

                            let selected_name = state
                                .workspaces
                                .iter()
                                .find(|w| w.id == current_ws)
                                .map(|w| w.name.as_str())
                                .unwrap_or("Collection");

                            egui::ComboBox::from_id_salt("save_dialog_ws_combo")
                                .selected_text(selected_name)
                                .show_ui(ui, |ui| {
                                    for ws in &state.workspaces {
                                        ui.selectable_value(
                                            &mut state.collection_panel.active_workspace_id,
                                            Some(ws.id.clone()),
                                            &ws.name,
                                        );
                                    }
                                });
                        }
                    });
                });
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let save_btn = egui::Button::new(
                        egui::RichText::new("Save")
                            .color(egui::Color32::WHITE)
                            .strong(),
                    )
                    .fill(crate::window_egui::style::theme_accent(ui.ctx()));

                    if ui.add(save_btn).clicked() {
                        save = true;
                        close = true;
                    }
                });
            });
        });

    if save {
        let ws_id = state
            .collection_panel
            .active_workspace_id
            .clone()
            .or_else(|| state.workspaces.first().map(|w| w.id.clone()))
            .unwrap_or_else(|| "default".to_string());

        let req_name = if state.save_dialog_name.trim().is_empty() {
            let endpoint = crate::http_collection::extract_endpoint_url(&state.url);
            if endpoint == "/" || endpoint.is_empty() {
                "New Request".to_string()
            } else {
                endpoint
            }
        } else {
            state.save_dialog_name.trim().to_string()
        };

        let new_req_id = format!("sr_{}", chrono::Utc::now().timestamp_millis());
        let new_req = crate::http_collection::SavedRequest {
            id: new_req_id,
            workspace_id: ws_id.clone(),
            folder_id: None,
            name: req_name.clone(),
            url: state.url.clone(),
            method: state.method.clone(),
            params: state.params.clone(),
            headers: state.headers.clone(),
            body_type: state.body_type.clone(),
            body_text: state.body_text.clone(),
            form_data: state.form_data.clone(),
            auth_type: state.auth_type.clone(),
            bearer_token: state.bearer_token.clone(),
            basic_user: state.basic_user.clone(),
            basic_pass: state.basic_pass.clone(),
            api_key_name: state.api_key_name.clone(),
            api_key_value: state.api_key_value.clone(),
            api_key_in_header: state.api_key_in_header,
            description: String::new(),
        };

        let mut workspaces = crate::http_collection::load_workspaces();
        if let Some(ws) = workspaces.iter_mut().find(|w| w.id == ws_id) {
            ws.requests.push(new_req.clone());
        } else {
            let new_ws = crate::http_collection::HttpWorkspace {
                id: ws_id.clone(),
                name: "Collection".to_string(),
                requests: vec![new_req.clone()],
                folders: Vec::new(),
                environments: Vec::new(),
            };
            workspaces.push(new_ws);
        }
        if let Err(e) = crate::http_collection::save_workspaces(&workspaces) {
            toasts.error(e);
        }
        state.workspaces = workspaces;
        state.saved_request_id = Some(new_req.id.clone());
        state.saved_workspace_id = Some(ws_id.clone());
        state.saved_folder_id = None;
        state.save_dialog_name = req_name.clone();
        state.collection_panel.active_workspace_id = Some(ws_id);

        if let Some(conn_id) = connection_id {
            save_http_state(conn_id, state);
        }

        toasts.success(format!("Request '{}' saved ✓", req_name));
    }

    if close {
        state.show_save_dialog = false;
    }

    save
}

/// Render the floating "Copy as Code" dialog: pick a target language and get
/// a live-generated, ready-to-run snippet reproducing the current request.
fn render_code_dialog(
    ui: &mut egui::Ui,
    state: &mut HttpClientState,
    toasts: &mut crate::window_egui::notifications::ToastManager,
) {
    if !state.show_code_dialog {
        return;
    }

    let mut close_requested = false;
    let mut copy_clicked = false;

    crate::window_egui::style::render_modal_backdrop(
        ui.ctx(),
        "modal_code_dialog_backdrop",
        state.show_code_dialog,
    );

    egui::Window::new("Copy as Code")
        .title_bar(false)
        .frame(crate::window_egui::style::modal_window_frame(ui.ctx()))
        .collapsible(false)
        .resizable(true)
        .default_size(egui::vec2(580.0, 460.0))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ui.ctx(), |ui| {
            crate::window_egui::style::render_modal_header(
                ui,
                "Copy as Code",
                &mut close_requested,
            );
            ui.add_space(8.0);

            let mut code = crate::http_code_export::generate(&state.code_dialog_lang, state);

            crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for lang in CodeLang::all() {
                        let label = lang.label();
                        ui.selectable_value(&mut state.code_dialog_lang, lang, label);
                    }
                });
            });

            ui.add_space(8.0);

            let dark = ui.visuals().dark_mode;
            let lang_for_highlight = state.code_dialog_lang.clone();
            let mut layouter = move |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
                let s = buf.as_str();
                let font_id = ui.style().text_styles[&egui::TextStyle::Monospace].clone();
                let mut job = highlight_code(s, &lang_for_highlight, dark, font_id);
                job.wrap.max_width = wrap_width;
                ui.fonts_mut(|f| f.layout_job(job))
            };

            let avail_h = (ui.available_height() - 44.0).max(180.0);
            crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                egui::ScrollArea::both()
                    .id_salt("http_code_preview_scroll")
                    .max_height(avail_h)
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut code)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .layouter(&mut layouter),
                        );
                    });
            });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let copy_label = format!(
                        "{} Copy to Clipboard",
                        egui_icons::icons::ICON_CONTENT_COPY.codepoint
                    );
                    let copy_btn = egui::Button::new(
                        egui::RichText::new(copy_label)
                            .color(egui::Color32::WHITE)
                            .strong(),
                    )
                    .fill(crate::window_egui::style::theme_accent(ui.ctx()));

                    if ui.add(copy_btn).clicked() {
                        ui.ctx().copy_text(code.clone());
                        copy_clicked = true;
                    }
                });
            });
        });

    if copy_clicked {
        toasts.success(format!("Copied as {} ✓", state.code_dialog_lang.label()));
    }

    if close_requested {
        state.show_code_dialog = false;
    }
}

/// Save or update an HTTP client tab.
/// - If associated with an existing collection request (`state.saved_request_id`), updates the request in collection.
///   (Also updates HTTP connection state draft if `connection_id` is present).
/// - Else if associated with an HTTP connection (`connection_id`), saves connection state to disk.
/// - Else (unsaved request), triggers the "Save Request to Collection" dialog.
///   (Also updates HTTP connection state draft if `connection_id` is present).
///
/// Returns `true` if workspace collection or connection state was modified.
pub fn save_or_update_http_tab(
    connection_id: Option<i64>,
    state: &mut HttpClientState,
    toasts: &mut crate::window_egui::notifications::ToastManager,
) -> bool {
    if let Some(req_id) = state.saved_request_id.clone() {
        let mut workspaces = crate::http_collection::load_workspaces();
        let mut updated = false;

        fn update_saved_req(
            req: &mut crate::http_collection::SavedRequest,
            state: &HttpClientState,
        ) {
            req.url = state.url.clone();
            req.method = state.method.clone();
            req.params = state.params.clone();
            req.headers = state.headers.clone();
            req.body_type = state.body_type.clone();
            req.body_text = state.body_text.clone();
            req.form_data = state.form_data.clone();
            req.auth_type = state.auth_type.clone();
            req.bearer_token = state.bearer_token.clone();
            req.basic_user = state.basic_user.clone();
            req.basic_pass = state.basic_pass.clone();
            req.api_key_name = state.api_key_name.clone();
            req.api_key_value = state.api_key_value.clone();
            req.api_key_in_header = state.api_key_in_header;
            if !state.save_dialog_name.trim().is_empty() {
                req.name = state.save_dialog_name.trim().to_string();
            }
        }

        fn update_in_folders(
            folders: &mut [crate::http_collection::HttpFolder],
            req_id: &str,
            state: &HttpClientState,
        ) -> bool {
            for folder in folders.iter_mut() {
                if let Some(req) = folder.requests.iter_mut().find(|r| r.id == req_id) {
                    update_saved_req(req, state);
                    return true;
                }
                if update_in_folders(&mut folder.children, req_id, state) {
                    return true;
                }
            }
            false
        }

        for ws in workspaces.iter_mut() {
            if let Some(req) = ws.requests.iter_mut().find(|r| r.id == req_id) {
                update_saved_req(req, state);
                updated = true;
                break;
            }
            if update_in_folders(&mut ws.folders, &req_id, state) {
                updated = true;
                break;
            }
        }

        if updated {
            if let Err(e) = crate::http_collection::save_workspaces(&workspaces) {
                toasts.error(e);
            }
            state.workspaces = workspaces;
            if let Some(conn_id) = connection_id {
                save_http_state(conn_id, state);
            }
            let display_name = if state.save_dialog_name.trim().is_empty() {
                "request"
            } else {
                state.save_dialog_name.trim()
            };
            toasts.success(format!("Saved '{}' ✓", display_name));
            true
        } else {
            // Request missing from workspaces, fallback to save dialog
            state.show_save_dialog = true;
            if state.save_dialog_name.trim().is_empty() {
                let default_name = crate::http_collection::extract_endpoint_url(&state.url);
                state.save_dialog_name = if default_name == "/" || default_name.is_empty() {
                    "New Request".to_string()
                } else {
                    default_name
                };
            }
            if let Some(conn_id) = connection_id {
                save_http_state(conn_id, state);
            }
            false
        }
    } else if let Some(conn_id) = connection_id {
        save_http_state(conn_id, state);
        toasts.success("HTTP connection state saved ✓");
        true
    } else {
        // Unsaved request: open save dialog so user can name it and choose collection
        state.show_save_dialog = true;
        if state.save_dialog_name.trim().is_empty() {
            let default_name = crate::http_collection::extract_endpoint_url(&state.url);
            state.save_dialog_name = if default_name == "/" || default_name.is_empty() {
                "New Request".to_string()
            } else {
                default_name
            };
        }
        if let Some(conn_id) = connection_id {
            save_http_state(conn_id, state);
        }
        false
    }
}

// ─── Request panel (tabs + content) ─────────────────────────────────────────

fn render_request_panel(
    ui: &mut egui::Ui,
    state: &mut HttpClientState,
    toasts: &mut crate::window_egui::notifications::ToastManager,
) {
    use crate::http_client_widgets::{self as w, TabItem};

    let items = [
        TabItem {
            label: "Body",
            badge: None,
            dot: state.body_type != HttpBodyType::NoBody,
        },
        TabItem {
            label: "Params",
            badge: Some(w::active_count(&state.params)),
            dot: false,
        },
        TabItem {
            label: "Headers",
            badge: Some(w::active_count(&state.headers)),
            dot: false,
        },
        TabItem {
            label: "Auth",
            badge: None,
            dot: !matches!(
                state.auth_type,
                HttpAuthType::NoAuth | HttpAuthType::InheritParent
            ),
        },
    ];
    let active = match state.active_tab {
        HttpRequestTab::Body => 0,
        HttpRequestTab::Params => 1,
        HttpRequestTab::Headers => 2,
        HttpRequestTab::Auth => 3,
    };

    let vertical = state.layout_vertical;
    let mut toggle_layout = false;
    let clicked = w::render_tab_strip(ui, "http_request_tabs", &items, active, |ui| {
        let (icon, tip) = if vertical {
            (
                egui_icons::icons::ICON_VERTICAL_SPLIT,
                "Show request and response side by side",
            )
        } else {
            (
                egui_icons::icons::ICON_HORIZONTAL_SPLIT,
                "Stack response below the request",
            )
        };
        if crate::window_egui::style::ai_icon_button(ui, icon.codepoint, tip).clicked() {
            toggle_layout = true;
        }
    });
    if toggle_layout {
        state.layout_vertical = !state.layout_vertical;
    }
    if let Some(i) = clicked {
        state.active_tab = match i {
            0 => HttpRequestTab::Body,
            1 => HttpRequestTab::Params,
            2 => HttpRequestTab::Headers,
            _ => HttpRequestTab::Auth,
        };
    }

    ui.add_space(8.0);

    // Body punya editor sendiri (dengan ScrollArea internal), jadi tidak
    // dibungkus ScrollArea luar supaya tidak muncul dua scrollbar.
    if matches!(state.active_tab, HttpRequestTab::Body) {
        render_body_panel(ui, state, toasts);
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt("http_request_scroll")
        .auto_shrink([false; 2])
        .show(ui, |ui| match state.active_tab {
            HttpRequestTab::Params => w::render_kv_table(
                ui,
                &mut state.params,
                "http_params",
                &w::KvOptions {
                    suggestions: &[],
                    mask_sensitive: true,
                    key_hint: "parameter",
                    value_hint: "value",
                },
            ),
            HttpRequestTab::Headers => w::render_kv_table(
                ui,
                &mut state.headers,
                "http_headers",
                &w::KvOptions {
                    suggestions: w::COMMON_HEADERS,
                    mask_sensitive: true,
                    key_hint: "Header-Name",
                    value_hint: "value",
                },
            ),
            HttpRequestTab::Auth => render_auth_panel(ui, state),
            HttpRequestTab::Body => {}
        });
}

// ─── Body panel ─────────────────────────────────────────────────────────────

/// Pasangan (tipe body, key segmen, ikon, label) untuk pemilih tipe body.
fn body_type_segments() -> [(HttpBodyType, &'static str, &'static str, &'static str); 8] {
    use egui_icons::icons as i;
    [
        (
            HttpBodyType::NoBody,
            "none",
            i::ICON_BLOCK.codepoint,
            "None",
        ),
        (
            HttpBodyType::Json,
            "json",
            i::MDI_CODE_JSON.codepoint,
            "JSON",
        ),
        (
            HttpBodyType::UrlEncoded,
            "form",
            i::ICON_FORMAT_LIST_BULLETED.codepoint,
            "Form",
        ),
        (
            HttpBodyType::MultiPart,
            "multipart",
            i::ICON_ATTACH_FILE.codepoint,
            "Multipart",
        ),
        (
            HttpBodyType::GraphQL,
            "graphql",
            i::MDI_GRAPHQL.codepoint,
            "GraphQL",
        ),
        (HttpBodyType::Xml, "xml", i::MDI_XML.codepoint, "XML"),
        (
            HttpBodyType::OtherText,
            "raw",
            i::ICON_TEXT_SNIPPET.codepoint,
            "Raw",
        ),
        (
            HttpBodyType::BinaryFile,
            "binary",
            i::ICON_UPLOAD_FILE.codepoint,
            "Binary",
        ),
    ]
}

/// Content-Type yang dikirim `execute_request` untuk tipe body teks.
fn body_mime_label(body_type: &HttpBodyType) -> &'static str {
    match body_type {
        HttpBodyType::Json | HttpBodyType::GraphQL => "application/json",
        HttpBodyType::Xml => "application/xml",
        HttpBodyType::UrlEncoded => "application/x-www-form-urlencoded",
        HttpBodyType::MultiPart => "multipart/form-data",
        HttpBodyType::OtherText => "text/plain",
        HttpBodyType::BinaryFile => "application/octet-stream",
        HttpBodyType::NoBody => "",
    }
}

/// Pesan error parse JSON (dengan baris/kolom), `None` bila valid atau kosong.
/// Body di atas 512 KB tidak divalidasi supaya UI tetap ringan.
fn json_error(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > 512 * 1024 {
        return None;
    }
    serde_json::from_str::<serde::de::IgnoredAny>(trimmed)
        .err()
        .map(|e| e.to_string())
}

fn render_body_panel(
    ui: &mut egui::Ui,
    state: &mut HttpClientState,
    toasts: &mut crate::window_egui::notifications::ToastManager,
) {
    use crate::http_client_widgets::{self as w, CodeView, Syntax};
    use crate::window_egui::style;

    let ctx = ui.ctx().clone();
    let segments_src = body_type_segments();
    let segments: Vec<style::NavSegment<'_>> = segments_src
        .iter()
        .map(|(_, key, icon, label)| style::NavSegment { key, icon, label })
        .collect();
    let selected = segments_src
        .iter()
        .find(|(t, ..)| *t == state.body_type)
        .map(|(_, key, ..)| *key)
        .unwrap_or("none");
    if let Some(key) = style::render_segmented_nav(ui, "http_body_type", &segments, selected, 32.0)
        && let Some((t, ..)) = segments_src.iter().find(|(_, k, ..)| *k == key)
    {
        state.body_type = t.clone();
    }

    ui.add_space(8.0);

    match state.body_type {
        HttpBodyType::NoBody => {
            ui.add_space(36.0);
            w::empty_state(
                ui,
                egui_icons::icons::ICON_DATA_OBJECT.codepoint,
                "This request has no body",
                "Pick a body type above to send JSON, form data, GraphQL, XML or raw text.",
            );
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                if ui
                    .add(
                        style::btn_secondary(format!(
                            "{}  Add JSON body",
                            egui_icons::icons::ICON_ADD.codepoint
                        ))
                        .min_size(egui::vec2(0.0, 30.0)),
                    )
                    .clicked()
                {
                    state.body_type = HttpBodyType::Json;
                    if state.body_text.trim().is_empty() {
                        state.body_text = "{\n  \n}".to_string();
                    }
                }
            });
        }
        HttpBodyType::UrlEncoded | HttpBodyType::MultiPart => {
            egui::ScrollArea::vertical()
                .id_salt("http_form_scroll")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    w::render_kv_table(
                        ui,
                        &mut state.form_data,
                        "http_form_data",
                        &w::KvOptions {
                            suggestions: &[],
                            mask_sensitive: true,
                            key_hint: "field",
                            value_hint: "value",
                        },
                    );
                });
        }
        HttpBodyType::BinaryFile => {
            ui.add_space(36.0);
            w::empty_state(
                ui,
                egui_icons::icons::ICON_UPLOAD_FILE.codepoint,
                "Binary upload is not supported yet",
                "Use Multipart or Raw to send file contents for now.",
            );
        }
        HttpBodyType::Json
        | HttpBodyType::GraphQL
        | HttpBodyType::Xml
        | HttpBodyType::OtherText => {
            let (syntax, hint) = match state.body_type {
                HttpBodyType::Json => (Syntax::Json, "{\n  \"key\": \"value\"\n}"),
                HttpBodyType::GraphQL => (
                    Syntax::GraphQl,
                    "{\n  \"query\": \"{ users { id name } }\"\n}",
                ),
                HttpBodyType::Xml => (Syntax::Xml, "<root>\n  <item>value</item>\n</root>"),
                _ => (Syntax::Plain, "Raw request body"),
            };
            let is_json = matches!(state.body_type, HttpBodyType::Json | HttpBodyType::GraphQL);
            let can_beautify = is_json || matches!(state.body_type, HttpBodyType::Xml);

            // ── Toolbar editor ──
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(body_mime_label(&state.body_type))
                        .size(11.5)
                        .family(egui::FontFamily::Monospace)
                        .color(style::nav_text_muted(&ctx)),
                );
                if is_json && !state.body_text.trim().is_empty() {
                    ui.add_space(6.0);
                    match json_error(&state.body_text) {
                        None => {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} Valid JSON",
                                    egui_icons::icons::ICON_CHECK_CIRCLE.codepoint
                                ))
                                .size(11.5)
                                .color(style::theme_success(&ctx)),
                            );
                        }
                        Some(err) => {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} Invalid JSON",
                                    egui_icons::icons::ICON_ERROR.codepoint
                                ))
                                .size(11.5)
                                .color(style::theme_danger(&ctx)),
                            )
                            .on_hover_text(err);
                        }
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if style::ai_icon_button(
                        ui,
                        egui_icons::icons::ICON_DELETE.codepoint,
                        "Clear body",
                    )
                    .clicked()
                    {
                        state.body_text.clear();
                    }
                    if style::ai_icon_button(
                        ui,
                        egui_icons::icons::ICON_CONTENT_COPY.codepoint,
                        "Copy body",
                    )
                    .clicked()
                    {
                        ui.ctx().copy_text(state.body_text.clone());
                        toasts.success("Request body copied");
                    }
                    if can_beautify
                        && style::ai_icon_button(
                            ui,
                            egui_icons::icons::ICON_AUTO_FIX_HIGH.codepoint,
                            "Beautify (format) body",
                        )
                        .clicked()
                    {
                        beautify_request_body(state, toasts);
                    }
                });
            });
            ui.add_space(4.0);

            w::render_code_view(
                ui,
                &mut state.body_text,
                CodeView {
                    id_salt: "http_request_body_editor",
                    syntax,
                    wrap: true,
                    hint,
                    search: "",
                },
            );
        }
    }
}

fn beautify_request_body(
    state: &mut HttpClientState,
    toasts: &mut crate::window_egui::notifications::ToastManager,
) {
    match state.body_type {
        HttpBodyType::Json | HttpBodyType::GraphQL => match beautify_json(&state.body_text) {
            Some(pretty) => state.body_text = pretty,
            None => toasts.warning("Body is not valid JSON, nothing to format"),
        },
        HttpBodyType::Xml => {
            let pretty = beautify_xml(&state.body_text);
            if !pretty.is_empty() {
                state.body_text = pretty;
            }
        }
        _ => {}
    }
}

// ─── Auth panel ─────────────────────────────────────────────────────────────

fn auth_label(auth: &HttpAuthType) -> &'static str {
    match auth {
        HttpAuthType::NoAuth => "No Auth",
        HttpAuthType::InheritParent => "Inherit from Parent",
        HttpAuthType::BearerToken => "Bearer Token",
        HttpAuthType::BasicAuth => "Basic Auth",
        HttpAuthType::ApiKey => "API Key",
        HttpAuthType::JwtBearer => "JWT Bearer",
        HttpAuthType::OAuth1 => "OAuth 1.0",
        HttpAuthType::OAuth2 => "OAuth 2.0",
        HttpAuthType::AwsSignature => "AWS Signature",
        HttpAuthType::NtlmAuth => "NTLM Auth",
    }
}

fn auth_description(auth: &HttpAuthType) -> &'static str {
    match auth {
        HttpAuthType::NoAuth => "The request is sent without authentication.",
        HttpAuthType::InheritParent => {
            "Auth settings will be inherited from the parent collection."
        }
        HttpAuthType::BearerToken | HttpAuthType::JwtBearer => {
            "Sent as the header  Authorization: Bearer <token>"
        }
        HttpAuthType::BasicAuth => {
            "Username and password are sent Base64-encoded in the Authorization header."
        }
        HttpAuthType::ApiKey => "The key is sent as a custom header or as a query parameter.",
        HttpAuthType::OAuth1
        | HttpAuthType::OAuth2
        | HttpAuthType::AwsSignature
        | HttpAuthType::NtlmAuth => "This authentication type is not implemented yet.",
    }
}

/// Field secret dengan tombol mata untuk menampilkan/menyembunyikan isinya.
fn secret_field(ui: &mut egui::Ui, id: &str, value: &mut String, hint: &str, width: f32) {
    let reveal_id = egui::Id::new((id, "reveal"));
    let revealed = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(reveal_id))
        .unwrap_or(false);
    ui.horizontal(|ui| {
        crate::window_egui::style::render_text_field(
            ui,
            egui::TextEdit::singleline(value)
                .id_salt(id)
                .hint_text(hint)
                .password(!revealed),
            width - 32.0,
            None,
        );
        let icon = if revealed {
            egui_icons::icons::ICON_VISIBILITY_OFF
        } else {
            egui_icons::icons::ICON_VISIBILITY
        };
        if crate::window_egui::style::ai_icon_button(
            ui,
            icon.codepoint,
            if revealed { "Hide" } else { "Show" },
        )
        .clicked()
        {
            ui.ctx().data_mut(|d| d.insert_temp(reveal_id, !revealed));
        }
    });
}

fn render_auth_panel(ui: &mut egui::Ui, state: &mut HttpClientState) {
    use crate::window_egui::style;
    let ctx = ui.ctx().clone();
    let muted = style::nav_text_muted(&ctx);
    let field_w = ui.available_width().min(460.0);
    let label = |text: &str| egui::RichText::new(text).size(12.0).color(muted);

    ui.horizontal(|ui| {
        ui.label(label("Type"));
        ui.add_space(8.0);
        egui::ComboBox::from_id_salt("http_auth_type")
            .width(220.0)
            .selected_text(auth_label(&state.auth_type))
            .show_ui(ui, |ui| {
                for auth in [
                    HttpAuthType::NoAuth,
                    HttpAuthType::InheritParent,
                    HttpAuthType::BearerToken,
                    HttpAuthType::BasicAuth,
                    HttpAuthType::ApiKey,
                    HttpAuthType::JwtBearer,
                    HttpAuthType::OAuth1,
                    HttpAuthType::OAuth2,
                    HttpAuthType::AwsSignature,
                    HttpAuthType::NtlmAuth,
                ] {
                    let text = auth_label(&auth);
                    ui.selectable_value(&mut state.auth_type, auth, text);
                }
            });
    });
    ui.add_space(6.0);
    ui.label(label(auth_description(&state.auth_type)).size(11.5));
    ui.add_space(12.0);

    match state.auth_type {
        HttpAuthType::BearerToken | HttpAuthType::JwtBearer => {
            ui.label(label("Token"));
            ui.add_space(4.0);
            secret_field(
                ui,
                "http_auth_bearer",
                &mut state.bearer_token,
                "Bearer token or JWT",
                field_w,
            );
        }
        HttpAuthType::BasicAuth => {
            ui.label(label("Username"));
            ui.add_space(4.0);
            style::render_text_field(
                ui,
                egui::TextEdit::singleline(&mut state.basic_user).hint_text("username"),
                field_w - 32.0,
                None,
            );
            ui.add_space(10.0);
            ui.label(label("Password"));
            ui.add_space(4.0);
            secret_field(
                ui,
                "http_auth_basic_pass",
                &mut state.basic_pass,
                "password",
                field_w,
            );
        }
        HttpAuthType::ApiKey => {
            ui.label(label("Key name"));
            ui.add_space(4.0);
            style::render_text_field(
                ui,
                egui::TextEdit::singleline(&mut state.api_key_name).hint_text("X-API-Key"),
                field_w - 32.0,
                None,
            );
            ui.add_space(10.0);
            ui.label(label("Key value"));
            ui.add_space(4.0);
            secret_field(
                ui,
                "http_auth_api_key",
                &mut state.api_key_value,
                "your-api-key",
                field_w,
            );
            ui.add_space(10.0);
            ui.label(label("Add to"));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.radio_value(&mut state.api_key_in_header, true, "Header");
                ui.radio_value(&mut state.api_key_in_header, false, "Query param");
            });
        }
        _ => {}
    }
}

// ─── Response panel ──────────────────────────────────────────────────────────

fn cancel_request(
    state: &mut HttpClientState,
    toasts: &mut crate::window_egui::notifications::ToastManager,
) {
    // Menjatuhkan receiver cukup: thread latar selesai sendiri dan hasilnya
    // dibuang karena `send` ke channel yang sudah ditutup gagal diam-diam.
    state.is_loading = false;
    state.response_receiver = None;
    state.request_started = None;
    log::info!("[HTTP] request cancelled by user");
    toasts.info("Request cancelled");
}

/// Deteksi bahasa body response dari Content-Type, dengan fallback sniffing
/// untuk server yang mengirim JSON tanpa header yang benar.
fn response_syntax(headers: &[(String, String)], body: &str) -> crate::http_client_widgets::Syntax {
    use crate::http_client_widgets::Syntax;
    let ct = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.to_ascii_lowercase())
        .unwrap_or_default();
    if ct.contains("json") {
        return Syntax::Json;
    }
    if ct.contains("xml") || ct.contains("html") {
        return Syntax::Xml;
    }
    let t = body.trim_start();
    if t.starts_with('{') || t.starts_with('[') {
        Syntax::Json
    } else if t.starts_with('<') {
        Syntax::Xml
    } else {
        Syntax::Plain
    }
}

fn render_response_panel(
    ui: &mut egui::Ui,
    state: &mut HttpClientState,
    toasts: &mut Toasts,
    ai: &AiBackend,
) {
    if state.is_loading {
        render_response_loading(ui, state, toasts);
        return;
    }
    if let Some(err) = state.response_error.clone() {
        render_response_error(ui, state, &err, ai);
        return;
    }
    if state.response_status.is_none() {
        render_response_empty(ui);
        return;
    }

    use crate::http_client_widgets::{self as w, CodeView, Syntax, TabItem};
    use crate::window_egui::style;
    let ctx = ui.ctx().clone();
    let muted = style::theme_muted_text(&ctx);
    let syntax = response_syntax(&state.response_headers, &state.response_body);

    // ── Meta: status · waktu · ukuran · aksi ──
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let status = state.response_status.unwrap_or(0);
        let status_text = format!("{} {}", status, state.response_status_text);
        w::pill(ui, None, status_text.trim(), w::status_color(&ctx, status));
        if let Some(ms) = state.response_time_ms {
            w::pill(
                ui,
                Some(egui_icons::icons::ICON_TIMER.codepoint),
                &w::format_duration(ms),
                muted,
            )
            .on_hover_text("Total time");
        }
        if let Some(bytes) = state.response_size_bytes {
            w::pill(
                ui,
                Some(egui_icons::icons::ICON_DATA_OBJECT.codepoint),
                &w::format_bytes(bytes),
                muted,
            )
            .on_hover_text("Response body size");
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            #[cfg(not(target_os = "ios"))]
            if style::ai_icon_button(
                ui,
                egui_icons::icons::ICON_DOWNLOAD.codepoint,
                "Save response body to file",
            )
            .clicked()
            {
                save_response_to_file(state, syntax, toasts);
            }
            if style::ai_icon_button(
                ui,
                egui_icons::icons::ICON_CONTENT_COPY.codepoint,
                "Copy response body",
            )
            .clicked()
            {
                ui.ctx().copy_text(state.response_body.clone());
                toasts.success("Response body copied");
            }
            if matches!(syntax, Syntax::Json | Syntax::Xml)
                && style::ai_icon_button(
                    ui,
                    egui_icons::icons::ICON_AUTO_FIX_HIGH.codepoint,
                    "Beautify response body",
                )
                .clicked()
            {
                if syntax == Syntax::Json {
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
        });
    });
    ui.add_space(6.0);

    // ── Tab ──
    let items = [
        TabItem {
            label: "Body",
            badge: None,
            dot: false,
        },
        TabItem {
            label: "Headers",
            badge: Some(state.response_headers.len()),
            dot: false,
        },
        TabItem {
            label: "Raw",
            badge: None,
            dot: false,
        },
        TabItem {
            label: "AI",
            badge: None,
            dot: state.ai.explanation.is_some(),
        },
    ];
    let active = match state.response_tab {
        HttpResponseTab::Body => 0,
        HttpResponseTab::Headers => 1,
        HttpResponseTab::Raw => 2,
        HttpResponseTab::Ai => 3,
    };
    let show_body_tools = matches!(state.response_tab, HttpResponseTab::Body);
    let match_count = if show_body_tools && !state.response_search.is_empty() {
        Some(w::match_ranges(&state.response_body, &state.response_search).len())
    } else {
        None
    };
    let search = &mut state.response_search;
    let wrap = &mut state.response_wrap;
    let clicked = w::render_tab_strip(ui, "http_response_tabs", &items, active, |ui| {
        if !show_body_tools {
            return;
        }
        if w::icon_toggle(
            ui,
            egui_icons::icons::ICON_WRAP_TEXT.codepoint,
            "Wrap long lines",
            *wrap,
        )
        .clicked()
        {
            *wrap = !*wrap;
        }
        style::render_search_field(ui, search, "Find in body", 180.0);
        if let Some(n) = match_count {
            ui.label(
                egui::RichText::new(match n {
                    1 => "1 match".to_string(),
                    n => format!("{n} matches"),
                })
                .size(11.5)
                .color(muted),
            );
        }
    });
    if let Some(i) = clicked {
        state.response_tab = match i {
            0 => HttpResponseTab::Body,
            1 => HttpResponseTab::Headers,
            2 => HttpResponseTab::Raw,
            _ => HttpResponseTab::Ai,
        };
    }
    ui.add_space(8.0);

    match state.response_tab {
        HttpResponseTab::Body => {
            if state.response_body.is_empty() {
                ui.add_space(24.0);
                w::empty_state(
                    ui,
                    egui_icons::icons::ICON_NOTES.codepoint,
                    "Empty response body",
                    "The server returned no content.",
                );
                return;
            }
            let mut body: &str = state.response_body.as_str();
            w::render_code_view(
                ui,
                &mut body,
                CodeView {
                    id_salt: "http_response_body_view",
                    syntax,
                    wrap: state.response_wrap,
                    hint: "",
                    search: &state.response_search,
                },
            );
        }
        HttpResponseTab::Headers => render_response_headers(ui, state, toasts),
        HttpResponseTab::Raw => {
            let raw = raw_response_text(state);
            let mut raw_ref: &str = raw.as_str();
            w::render_code_view(
                ui,
                &mut raw_ref,
                CodeView {
                    id_salt: "http_response_raw_view",
                    syntax: Syntax::Plain,
                    wrap: state.response_wrap,
                    hint: "",
                    search: "",
                },
            );
        }
        HttpResponseTab::Ai => render_ai_explanation(ui, state, ai),
    }
}

/// Tab "AI" di panel response: tombol Explain, status, dan jawaban Markdown.
fn render_ai_explanation(ui: &mut egui::Ui, state: &mut HttpClientState, ai: &AiBackend) {
    use crate::http_ai::HttpAiTask;
    use crate::window_egui::style;
    let ctx = ui.ctx().clone();
    let muted = style::nav_text_muted(&ctx);

    let backend = match ai {
        Ok(b) => b,
        Err(msg) => {
            ui.add_space(24.0);
            crate::http_client_widgets::empty_state(
                ui,
                egui_icons::icons::ICON_AUTO_AWESOME.codepoint,
                "AI is not configured",
                msg,
            );
            return;
        }
    };
    let label = ai_backend_label(backend);
    if !render_ai_consent(ui, label) {
        return;
    }

    let explaining = state
        .ai
        .pending
        .as_ref()
        .is_some_and(|(t, _)| *t == HttpAiTask::ExplainResponse);
    let busy = state.ai.is_busy();

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("Explained by {label}"))
                .size(11.5)
                .color(muted),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let text = if state.ai.explanation.is_some() {
                "Explain again"
            } else {
                "Explain this response"
            };
            if ui
                .add_enabled(
                    !busy,
                    style::btn_secondary(format!(
                        "{}  {text}",
                        egui_icons::icons::ICON_AUTO_AWESOME.codepoint
                    )),
                )
                .clicked()
            {
                start_ai_task(state, HttpAiTask::ExplainResponse, backend);
            }
        });
    });
    ui.add_space(8.0);

    if explaining {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.add(egui::Spinner::new().size(20.0));
            ui.add_space(8.0);
            ui.label(egui::RichText::new(format!("Asking {label}…")).color(muted));
        });
        return;
    }
    if let Some(err) = &state.ai.error {
        style::ai_notice_frame(style::theme_danger(&ctx)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(err).size(12.0));
        });
        return;
    }
    match &state.ai.explanation {
        Some(text) => {
            egui::ScrollArea::vertical()
                .id_salt("http_ai_explanation_scroll")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    let mut cache = egui_commonmark::CommonMarkCache::default();
                    egui_commonmark::CommonMarkViewer::new().show(ui, &mut cache, text);
                });
        }
        None => {
            ui.add_space(24.0);
            crate::http_client_widgets::empty_state(
                ui,
                egui_icons::icons::ICON_AUTO_AWESOME.codepoint,
                "Get a plain-English explanation",
                "What the status means, what the body contains, and how to fix errors.",
            );
        }
    }
}

/// Response lengkap dalam format mirip wire: status line, header, baris
/// kosong, lalu body.
fn raw_response_text(state: &HttpClientState) -> String {
    let mut out = format!(
        "HTTP {} {}\n",
        state.response_status.unwrap_or(0),
        state.response_status_text
    );
    for (k, v) in &state.response_headers {
        out.push_str(k);
        out.push_str(": ");
        out.push_str(v);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&state.response_body);
    out
}

fn render_response_headers(
    ui: &mut egui::Ui,
    state: &HttpClientState,
    toasts: &mut crate::window_egui::notifications::ToastManager,
) {
    use crate::window_egui::style;
    let ctx = ui.ctx().clone();
    if state.response_headers.is_empty() {
        ui.add_space(24.0);
        crate::http_client_widgets::empty_state(
            ui,
            egui_icons::icons::ICON_LIST.codepoint,
            "No headers received",
            "",
        );
        return;
    }
    let key_color = style::theme_info(&ctx);
    egui::ScrollArea::both()
        .id_salt("http_response_headers_scroll")
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            egui::Grid::new("resp_headers_grid")
                .num_columns(2)
                .spacing([18.0, 6.0])
                .striped(true)
                .show(ui, |ui| {
                    for (k, v) in &state.response_headers {
                        ui.label(
                            egui::RichText::new(k)
                                .family(egui::FontFamily::Monospace)
                                .color(key_color),
                        );
                        let resp = ui
                            .add(
                                egui::Label::new(
                                    egui::RichText::new(v).family(egui::FontFamily::Monospace),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Click to copy");
                        if resp.clicked() {
                            ui.ctx().copy_text(format!("{k}: {v}"));
                            toasts.success(format!("Copied header {k}"));
                        }
                        ui.end_row();
                    }
                });
        });
}

fn render_response_loading(
    ui: &mut egui::Ui,
    state: &mut HttpClientState,
    toasts: &mut crate::window_egui::notifications::ToastManager,
) {
    use crate::window_egui::style;
    let ctx = ui.ctx().clone();
    ui.add_space(72.0);
    ui.vertical_centered(|ui| {
        ui.add(egui::Spinner::new().size(24.0));
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new("Sending request…")
                .size(14.0)
                .strong()
                .color(style::nav_text_strong(&ctx)),
        );
        if let Some(started) = state.request_started {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(crate::http_client_widgets::format_duration(
                    started.elapsed().as_millis(),
                ))
                .family(egui::FontFamily::Monospace)
                .color(style::nav_text_muted(&ctx)),
            );
        }
        ui.add_space(14.0);
        if ui
            .add(style::btn_secondary(format!(
                "{}  Cancel",
                egui_icons::icons::ICON_STOP.codepoint
            )))
            .clicked()
        {
            cancel_request(state, toasts);
        }
    });
}

fn render_response_error(
    ui: &mut egui::Ui,
    state: &mut HttpClientState,
    err: &str,
    ai: &AiBackend,
) {
    use crate::window_egui::style;
    let ctx = ui.ctx().clone();
    let danger = style::theme_danger(&ctx);
    ui.add_space(4.0);
    style::theme_alert_frame(&ctx, true).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(egui_icons::icons::ICON_ERROR.codepoint)
                    .size(18.0)
                    .color(danger),
            );
            ui.label(
                egui::RichText::new("Request failed")
                    .size(14.0)
                    .strong()
                    .color(danger),
            );
            if let Some(ms) = state.response_time_ms {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(crate::http_client_widgets::format_duration(ms))
                            .size(11.5)
                            .color(style::nav_text_muted(&ctx)),
                    );
                });
            }
        });
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(egui::RichText::new(err).family(egui::FontFamily::Monospace)).wrap(),
        );
        if let Some(hint) = crate::http_client_widgets::error_hint(err) {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(format!(
                    "{}  {}",
                    egui_icons::icons::ICON_LIGHTBULB.codepoint,
                    hint
                ))
                .size(12.0)
                .color(style::nav_text_strong(&ctx)),
            );
        }
    });

    // Error jaringan juga bisa dijelaskan AI (tanpa tab, langsung di bawah).
    if ai.is_ok() {
        ui.add_space(12.0);
        render_ai_explanation(ui, state, ai);
    }
}

fn render_response_empty(ui: &mut egui::Ui) {
    use crate::window_egui::style;
    let ctx = ui.ctx().clone();
    ui.add_space((ui.available_height() * 0.22).clamp(24.0, 140.0));
    crate::http_client_widgets::empty_state(
        ui,
        egui_icons::icons::ICON_SEND.codepoint,
        "Send a request to see the response",
        "Status, timing, headers and the body will show up here.",
    );
    ui.add_space(18.0);

    let send_keys = if cfg!(target_os = "macos") {
        "⌘ ↵"
    } else {
        "Ctrl ↵"
    };
    let save_keys = if cfg!(target_os = "macos") {
        "⌘ S"
    } else {
        "Ctrl S"
    };
    let rows: [(&str, Option<&str>); 3] = [
        ("Send request", Some(send_keys)),
        ("Save request", Some(save_keys)),
        ("Import cURL: paste it into the URL bar", None),
    ];
    let block_w = 280.0_f32.min(ui.available_width());
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - block_w) / 2.0).max(0.0));
        ui.vertical(|ui| {
            ui.set_width(block_w);
            for (label, keys) in rows {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(label)
                            .size(12.0)
                            .color(style::nav_text_muted(&ctx)),
                    );
                    if let Some(keys) = keys {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            style::render_shortcut_badge(ui, keys);
                        });
                    }
                });
                ui.add_space(2.0);
            }
        });
    });
}

#[cfg(not(target_os = "ios"))]
fn save_response_to_file(
    state: &HttpClientState,
    syntax: crate::http_client_widgets::Syntax,
    toasts: &mut crate::window_egui::notifications::ToastManager,
) {
    use crate::http_client_widgets::Syntax;
    let (name, ext) = match syntax {
        Syntax::Json => ("response.json", "json"),
        Syntax::Xml => ("response.xml", "xml"),
        _ => ("response.txt", "txt"),
    };
    let Some(path) = crate::rfd::FileDialog::new()
        .set_file_name(name)
        .add_filter(ext.to_uppercase(), &[ext])
        .save_file()
    else {
        return;
    };
    match std::fs::write(&path, state.response_body.as_bytes()) {
        Ok(()) => toasts.success(format!("Saved to {}", path.display())),
        Err(e) => {
            log::error!(
                "[HTTP] failed to save response to {}: {}",
                path.display(),
                e
            );
            toasts.error(format!("Could not save response: {e}"));
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
    state.request_started = Some(std::time::Instant::now());

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
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                log::error!("[HTTP] failed to start tokio runtime: {}", e);
                let _ = tx.send(HttpClientResponse {
                    status: 0,
                    status_text: String::new(),
                    body: String::new(),
                    headers: Vec::new(),
                    time_ms: 0,
                    size_bytes: 0,
                    error: Some(format!("Could not start HTTP runtime: {e}")),
                });
                return;
            }
        };
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
                HttpAuthType::ApiKey if api_key_in_header && !api_key_name.is_empty() => {
                    req_builder = req_builder.header(api_key_name.as_str(), api_key_value.as_str());
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
                HttpBodyType::UrlEncoded => req_builder.form(&form_data),
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
                        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
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
    state.request_started = None;
    if let Some(err) = resp.error {
        state.response_error = Some(err);
        state.response_status = None;
    } else {
        state.response_error = None;
        state.response_status = Some(resp.status);
        state.response_status_text = resp.status_text;
        state.response_body =
            maybe_beautify_json_response(&resp.headers, &resp.body).unwrap_or(resp.body);
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
pub(crate) fn highlight_body_json(
    text: &str,
    dark: bool,
    font_id: egui::FontId,
) -> egui::text::LayoutJob {
    use egui::{Color32, TextFormat, text::LayoutJob};
    let mut job = LayoutJob::default();

    let key_col = Color32::from_rgb(130, 200, 255); // cyan   – keys
    let str_col = Color32::from_rgb(152, 195, 121); // green  – string values
    let num_col = Color32::from_rgb(209, 154, 102); // orange – numbers
    let kw_col = Color32::from_rgb(198, 120, 221); // purple – true/false/null
    let punct_col = Color32::from_rgb(171, 178, 191); // gray   – brackets/commas
    let norm_col = if dark {
        Color32::from_rgb(220, 220, 220)
    } else {
        Color32::from_rgb(30, 30, 30)
    };

    macro_rules! tf {
        ($c:expr) => {
            TextFormat {
                font_id: font_id.clone(),
                color: $c,
                ..Default::default()
            }
        };
    }

    let bs = text.as_bytes();
    let n = bs.len();
    let mut i = 0;

    while i < n {
        match bs[i] {
            // ── double-quoted string ────────────────────────────────────
            b'"' => {
                let start = i;
                i += 1;
                while i < n {
                    if bs[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bs[i] == b'"' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                // look-ahead: if next non-ws char is ':', this is an object key
                let mut k = i;
                while k < n && bs[k].is_ascii_whitespace() {
                    k += 1;
                }
                let color = if k < n && bs[k] == b':' {
                    key_col
                } else {
                    str_col
                };
                if let Some(s) = text.get(start..i) {
                    job.append(s, 0.0, tf!(color));
                }
            }
            // ── positive number ─────────────────────────────────────────
            b'0'..=b'9' => {
                let start = i;
                while i < n
                    && (bs[i].is_ascii_digit() || bs[i] == b'.' || bs[i] == b'e' || bs[i] == b'E')
                {
                    i += 1;
                }
                if let Some(s) = text.get(start..i) {
                    job.append(s, 0.0, tf!(num_col));
                }
            }
            // ── negative number ─────────────────────────────────────────
            b'-' if i + 1 < n && bs[i + 1].is_ascii_digit() => {
                let start = i;
                i += 1;
                while i < n
                    && (bs[i].is_ascii_digit() || bs[i] == b'.' || bs[i] == b'e' || bs[i] == b'E')
                {
                    i += 1;
                }
                if let Some(s) = text.get(start..i) {
                    job.append(s, 0.0, tf!(num_col));
                }
            }
            // ── keyword (true / false / null) ───────────────────────────
            b'a'..=b'z' | b'A'..=b'Z' => {
                let start = i;
                while i < n && bs[i].is_ascii_alphanumeric() {
                    i += 1;
                }
                let word = text.get(start..i).unwrap_or("");
                let col = if matches!(word, "true" | "false" | "null") {
                    kw_col
                } else {
                    norm_col
                };
                job.append(word, 0.0, tf!(col));
            }
            // ── structural punctuation ──────────────────────────────────
            b'{' | b'}' | b'[' | b']' | b':' | b',' => {
                if let Some(s) = text.get(i..i + 1) {
                    job.append(s, 0.0, tf!(punct_col));
                }
                i += 1;
            }
            // ── whitespace / other ──────────────────────────────────────
            _ => {
                let start = i;
                i += 1;
                while i < n && matches!(bs[i], b' ' | b'\t' | b'\n' | b'\r') {
                    i += 1;
                }
                if let Some(s) = text.get(start..i) {
                    job.append(s, 0.0, tf!(norm_col));
                }
            }
        }
    }
    job
}

/// XML syntax highlighter.
/// Colors: blue = tag names, light-blue = attr names, green = attr values,
///         gray = punctuation, muted-green = comments, yellow = CDATA,
///         purple = processing instructions.
pub(crate) fn highlight_body_xml(
    text: &str,
    dark: bool,
    font_id: egui::FontId,
) -> egui::text::LayoutJob {
    use egui::{Color32, TextFormat, text::LayoutJob};
    let mut job = LayoutJob::default();

    let tag_col = Color32::from_rgb(86, 156, 214); // blue        – tag names
    let attr_key_col = Color32::from_rgb(146, 202, 245); // light blue  – attr names
    let attr_val_col = Color32::from_rgb(152, 195, 121); // green       – attr values
    let punct_col = Color32::from_rgb(171, 178, 191); // gray        – <, >, /, =
    let comment_col = Color32::from_rgb(106, 153, 85); // muted green – comments
    let cdata_col = Color32::from_rgb(220, 220, 170); // pale yellow – CDATA
    let pi_col = Color32::from_rgb(198, 120, 221); // purple      – <?...?>
    let norm_col = if dark {
        Color32::from_rgb(220, 220, 220)
    } else {
        Color32::from_rgb(30, 30, 30)
    };

    macro_rules! tf {
        ($c:expr) => {
            TextFormat {
                font_id: font_id.clone(),
                color: $c,
                ..Default::default()
            }
        };
    }

    let bs = text.as_bytes();
    let n = bs.len();
    let mut i = 0;

    while i < n {
        if bs[i] != b'<' {
            // ── text content ─────────────────────────────────────────────
            let start = i;
            while i < n && bs[i] != b'<' {
                i += 1;
            }
            if let Some(s) = text.get(start..i)
                && !s.is_empty()
            {
                job.append(s, 0.0, tf!(norm_col));
            }
            continue;
        }

        // ── comment ───────────────────────────────────────────────────
        if text[i..].starts_with("<!--") {
            let start = i;
            i += 4;
            while i < n {
                if text[i..].starts_with("-->") {
                    i += 3;
                    break;
                }
                i += 1;
            }
            if let Some(s) = text.get(start..i) {
                job.append(s, 0.0, tf!(comment_col));
            }
            continue;
        }

        // ── CDATA ────────────────────────────────────────────────────
        if text[i..].starts_with("<![CDATA[") {
            let start = i;
            i += 9;
            while i < n {
                if text[i..].starts_with("]]>") {
                    i += 3;
                    break;
                }
                i += 1;
            }
            if let Some(s) = text.get(start..i) {
                job.append(s, 0.0, tf!(cdata_col));
            }
            continue;
        }

        // ── regular tag ───────────────────────────────────────────────
        job.append("<", 0.0, tf!(punct_col));
        i += 1;

        let is_pi = i < n && bs[i] == b'?';
        let is_closing = i < n && bs[i] == b'/';
        if is_closing || is_pi {
            if let Some(s) = text.get(i..i + 1) {
                job.append(s, 0.0, tf!(punct_col));
            }
            i += 1;
        }

        // tag name
        let name_start = i;
        while i < n
            && !bs[i].is_ascii_whitespace()
            && bs[i] != b'>'
            && bs[i] != b'/'
            && bs[i] != b'?'
        {
            i += 1;
        }
        if let Some(name) = text.get(name_start..i)
            && !name.is_empty()
        {
            job.append(name, 0.0, tf!(if is_pi { pi_col } else { tag_col }));
        }

        // attributes
        while i < n && bs[i] != b'>' {
            if bs[i].is_ascii_whitespace() {
                let s = i;
                while i < n && bs[i].is_ascii_whitespace() {
                    i += 1;
                }
                if let Some(ws) = text.get(s..i) {
                    job.append(ws, 0.0, tf!(norm_col));
                }
            } else if bs[i] == b'/' || bs[i] == b'?' {
                if let Some(s) = text.get(i..i + 1) {
                    job.append(s, 0.0, tf!(punct_col));
                }
                i += 1;
            } else if bs[i] == b'=' {
                job.append("=", 0.0, tf!(punct_col));
                i += 1;
            } else if bs[i] == b'"' || bs[i] == b'\'' {
                let q = bs[i];
                let s = i;
                i += 1;
                while i < n && bs[i] != q {
                    i += 1;
                }
                if i < n {
                    i += 1;
                }
                if let Some(slice) = text.get(s..i) {
                    job.append(slice, 0.0, tf!(attr_val_col));
                }
            } else {
                let s = i;
                while i < n
                    && bs[i] != b'='
                    && bs[i] != b'>'
                    && !bs[i].is_ascii_whitespace()
                    && bs[i] != b'/'
                {
                    i += 1;
                }
                if let Some(name) = text.get(s..i)
                    && !name.is_empty()
                {
                    job.append(name, 0.0, tf!(attr_key_col));
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
pub(crate) fn highlight_body_graphql(
    text: &str,
    dark: bool,
    font_id: egui::FontId,
) -> egui::text::LayoutJob {
    use egui::{Color32, TextFormat, text::LayoutJob};
    let mut job = LayoutJob::default();

    let kw_col = Color32::from_rgb(198, 120, 221); // purple
    let str_col = Color32::from_rgb(152, 195, 121); // green
    let comment_col = Color32::from_rgb(106, 153, 85); // muted green
    let type_col = Color32::from_rgb(230, 180, 80); // orange  – TYPE names
    let field_col = Color32::from_rgb(130, 200, 255); // cyan    – field names
    let num_col = Color32::from_rgb(209, 154, 102); // orange  – numbers
    let punct_col = Color32::from_rgb(171, 178, 191); // gray
    let norm_col = if dark {
        Color32::from_rgb(220, 220, 220)
    } else {
        Color32::from_rgb(30, 30, 30)
    };

    macro_rules! tf {
        ($c:expr) => {
            TextFormat {
                font_id: font_id.clone(),
                color: $c,
                ..Default::default()
            }
        };
    }

    let bs = text.as_bytes();
    let n = bs.len();
    let mut i = 0;

    while i < n {
        match bs[i] {
            // ── line comment ────────────────────────────────────────────
            b'#' => {
                let start = i;
                while i < n && bs[i] != b'\n' {
                    i += 1;
                }
                if let Some(s) = text.get(start..i) {
                    job.append(s, 0.0, tf!(comment_col));
                }
            }
            // ── triple-quoted or regular string ─────────────────────────
            b'"' => {
                let start = i;
                if text[i..].starts_with("\"\"\"") {
                    i += 3;
                    while i < n {
                        if text[i..].starts_with("\"\"\"") {
                            i += 3;
                            break;
                        }
                        i += 1;
                    }
                } else {
                    i += 1;
                    while i < n {
                        if bs[i] == b'\\' {
                            i += 2;
                            continue;
                        }
                        if bs[i] == b'"' {
                            i += 1;
                            break;
                        }
                        i += 1;
                    }
                }
                if let Some(s) = text.get(start..i) {
                    job.append(s, 0.0, tf!(str_col));
                }
            }
            // ── number ─────────────────────────────────────────────────
            b'0'..=b'9' => {
                let start = i;
                while i < n && (bs[i].is_ascii_digit() || bs[i] == b'.') {
                    i += 1;
                }
                if let Some(s) = text.get(start..i) {
                    job.append(s, 0.0, tf!(num_col));
                }
            }
            b'-' if i + 1 < n && bs[i + 1].is_ascii_digit() => {
                let start = i;
                i += 1;
                while i < n && (bs[i].is_ascii_digit() || bs[i] == b'.') {
                    i += 1;
                }
                if let Some(s) = text.get(start..i) {
                    job.append(s, 0.0, tf!(num_col));
                }
            }
            // ── identifier (keyword / type / field) ─────────────────────
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                let start = i;
                while i < n && (bs[i].is_ascii_alphanumeric() || bs[i] == b'_') {
                    i += 1;
                }
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
                if let Some(s) = text.get(i..i + 1) {
                    job.append(s, 0.0, tf!(punct_col));
                }
                i += 1;
            }
            // ── whitespace / other ──────────────────────────────────────
            _ => {
                let start = i;
                i += 1;
                while i < n && matches!(bs[i], b' ' | b'\t' | b'\n' | b'\r') {
                    i += 1;
                }
                if let Some(s) = text.get(start..i) {
                    job.append(s, 0.0, tf!(norm_col));
                }
            }
        }
    }
    job
}

fn is_graphql_keyword(word: &str) -> bool {
    matches!(
        word,
        "query"
            | "mutation"
            | "subscription"
            | "fragment"
            | "on"
            | "type"
            | "interface"
            | "union"
            | "enum"
            | "input"
            | "extend"
            | "schema"
            | "scalar"
            | "directive"
            | "implements"
            | "true"
            | "false"
            | "null"
            | "if"
            | "include"
            | "skip"
            | "repeatable"
    )
}

// ─── "Copy as Code" preview syntax highlighting ──────────────────────────────

/// Generic multi-language syntax highlighter for the "Copy as Code" preview.
/// Not a full parser — same heuristic byte-scanning approach as the
/// JSON/XML/GraphQL body highlighters above, tuned per target language via a
/// small (comment-style, keyword-list) profile.
/// Colors: green = strings, muted-green = comments, orange = numbers,
///         purple = keywords / curl flags, yellow = Capitalized identifiers,
///         cyan = $variables (PHP), gray = punctuation.
pub(crate) fn highlight_code(
    text: &str,
    lang: &CodeLang,
    dark: bool,
    font_id: egui::FontId,
) -> egui::text::LayoutJob {
    use egui::{Color32, TextFormat, text::LayoutJob};
    let mut job = LayoutJob::default();

    let str_col = Color32::from_rgb(152, 195, 121); // green   – strings
    let comment_col = Color32::from_rgb(106, 153, 85); // muted green – comments
    let num_col = Color32::from_rgb(209, 154, 102); // orange  – numbers
    let kw_col = Color32::from_rgb(198, 120, 221); // purple  – keywords / curl flags
    let type_col = Color32::from_rgb(230, 180, 80); // yellow  – Capitalized identifiers
    let var_col = Color32::from_rgb(130, 200, 255); // cyan    – $variables (PHP)
    let punct_col = Color32::from_rgb(171, 178, 191); // gray    – punctuation
    let norm_col = if dark {
        Color32::from_rgb(220, 220, 220)
    } else {
        Color32::from_rgb(30, 30, 30)
    };

    macro_rules! tf {
        ($c:expr) => {
            TextFormat {
                font_id: font_id.clone(),
                color: $c,
                ..Default::default()
            }
        };
    }

    let (line_comment, block_comment, keywords): (Option<&str>, Option<(&str, &str)>, &[&str]) =
        match lang {
            CodeLang::Curl => (Some("#"), None, &[]),
            CodeLang::Python => (
                Some("#"),
                None,
                &[
                    "import", "as", "def", "return", "if", "else", "elif", "for", "while", "in",
                    "True", "False", "None", "and", "or", "not", "print",
                ],
            ),
            CodeLang::JavaScript | CodeLang::NodeJs => (
                Some("//"),
                Some(("/*", "*/")),
                &[
                    "const",
                    "let",
                    "var",
                    "function",
                    "return",
                    "new",
                    "require",
                    "then",
                    "catch",
                    "true",
                    "false",
                    "null",
                    "undefined",
                    "async",
                    "await",
                ],
            ),
            CodeLang::Go => (
                Some("//"),
                Some(("/*", "*/")),
                &[
                    "package", "import", "func", "return", "if", "err", "nil", "var", "true",
                    "false", "defer", "struct", "type",
                ],
            ),
            CodeLang::Php => (
                Some("//"),
                Some(("/*", "*/")),
                &["php", "echo", "if", "else", "true", "false", "null"],
            ),
            CodeLang::Rust => (
                Some("//"),
                Some(("/*", "*/")),
                &[
                    "use", "fn", "let", "mut", "match", "struct", "impl", "return", "if", "else",
                    "true", "false", "Some", "None", "Ok", "Err", "panic",
                ],
            ),
        };

    let bs = text.as_bytes();
    let n = bs.len();
    let mut i = 0;

    while i < n {
        // ── line comment ─────────────────────────────────────────────
        if let Some(lc) = line_comment
            && text[i..].starts_with(lc)
        {
            let start = i;
            while i < n && bs[i] != b'\n' {
                i += 1;
            }
            if let Some(s) = text.get(start..i) {
                job.append(s, 0.0, tf!(comment_col));
            }
            continue;
        }

        // ── block comment ────────────────────────────────────────────
        if let Some((bstart, bend)) = block_comment
            && text[i..].starts_with(bstart)
        {
            let start = i;
            i += bstart.len();
            while i < n && !text[i..].starts_with(bend) {
                i += 1;
            }
            i = (i + bend.len()).min(n);
            if let Some(s) = text.get(start..i) {
                job.append(s, 0.0, tf!(comment_col));
            }
            continue;
        }

        match bs[i] {
            // ── string literal (single/double/backtick) ─────────────────
            b'"' | b'\'' | b'`' => {
                let quote = bs[i];
                let start = i;
                i += 1;
                while i < n {
                    if quote != b'`' && bs[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bs[i] == quote {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                if let Some(s) = text.get(start..i) {
                    job.append(s, 0.0, tf!(str_col));
                }
            }
            // ── number ────────────────────────────────────────────────
            b'0'..=b'9' => {
                let start = i;
                while i < n && (bs[i].is_ascii_digit() || bs[i] == b'.') {
                    i += 1;
                }
                if let Some(s) = text.get(start..i) {
                    job.append(s, 0.0, tf!(num_col));
                }
            }
            // ── curl-style flag (-X, --header, …) ────────────────────────
            b'-' if matches!(lang, CodeLang::Curl)
                && i + 1 < n
                && (bs[i + 1] == b'-' || bs[i + 1].is_ascii_alphabetic()) =>
            {
                let start = i;
                i += 1;
                while i < n && (bs[i].is_ascii_alphanumeric() || bs[i] == b'-' || bs[i] == b'.') {
                    i += 1;
                }
                if let Some(s) = text.get(start..i) {
                    job.append(s, 0.0, tf!(kw_col));
                }
            }
            // ── identifier / keyword / $variable ─────────────────────────
            b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'$' => {
                let start = i;
                i += 1;
                while i < n && (bs[i].is_ascii_alphanumeric() || bs[i] == b'_') {
                    i += 1;
                }
                let word = text.get(start..i).unwrap_or("");
                let col = if word.starts_with('$') {
                    var_col
                } else if keywords.contains(&word) {
                    kw_col
                } else if word.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                    type_col
                } else {
                    norm_col
                };
                job.append(word, 0.0, tf!(col));
            }
            // ── punctuation ───────────────────────────────────────────────
            b'{' | b'}' | b'[' | b']' | b'(' | b')' | b':' | b',' | b';' | b'=' | b'.' | b'<'
            | b'>' | b'&' | b'|' | b'!' | b'+' | b'*' | b'/' | b'@' | b'#' => {
                if let Some(s) = text.get(i..i + 1) {
                    job.append(s, 0.0, tf!(punct_col));
                }
                i += 1;
            }
            // ── whitespace / other ────────────────────────────────────────
            _ => {
                let start = i;
                i += 1;
                while i < n && matches!(bs[i], b' ' | b'\t' | b'\n' | b'\r') {
                    i += 1;
                }
                if let Some(s) = text.get(start..i) {
                    job.append(s, 0.0, tf!(norm_col));
                }
            }
        }
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_client_widgets::Syntax;

    #[test]
    fn legacy_state_json_loads_with_layout_defaults() {
        // State yang disimpan sebelum field layout ditambahkan.
        let mut value = serde_json::to_value(HttpClientState::default()).unwrap();
        let obj = value.as_object_mut().unwrap();
        for field in ["split_ratio", "layout_vertical", "response_wrap"] {
            assert!(obj.remove(field).is_some(), "{field} should be serialized");
        }
        let state: HttpClientState = serde_json::from_value(value).unwrap();
        assert_eq!(state.split_ratio, 0.5);
        assert!(!state.layout_vertical);
        assert!(state.response_wrap);
        assert!(state.request_started.is_none());
    }

    #[test]
    fn json_error_reports_position_and_ignores_empty() {
        assert!(json_error("").is_none());
        assert!(json_error("  {\"a\": [1, 2]} ").is_none());
        let err = json_error("{\n  \"a\": ,\n}").unwrap();
        assert!(err.contains("line 2"), "{err}");
    }

    #[test]
    fn response_syntax_uses_content_type_then_sniffs() {
        let ct = |v: &str| vec![("Content-Type".to_string(), v.to_string())];
        assert_eq!(
            response_syntax(&ct("application/problem+json"), "x"),
            Syntax::Json
        );
        assert_eq!(
            response_syntax(&ct("text/html; charset=utf-8"), "x"),
            Syntax::Xml
        );
        assert_eq!(response_syntax(&[], "  [1,2]"), Syntax::Json);
        assert_eq!(response_syntax(&[], "<a/>"), Syntax::Xml);
        assert_eq!(response_syntax(&ct("text/plain"), "hello"), Syntax::Plain);
    }

    #[test]
    fn raw_response_has_status_headers_and_body() {
        let state = HttpClientState {
            response_status: Some(201),
            response_status_text: "Created".into(),
            response_headers: vec![("x-id".into(), "7".into())],
            response_body: "{}".into(),
            ..Default::default()
        };
        assert_eq!(raw_response_text(&state), "HTTP 201 Created\nx-id: 7\n\n{}");
    }

    #[test]
    fn body_segments_cover_every_body_type_once() {
        let segs = body_type_segments();
        for t in [
            HttpBodyType::NoBody,
            HttpBodyType::Json,
            HttpBodyType::UrlEncoded,
            HttpBodyType::MultiPart,
            HttpBodyType::GraphQL,
            HttpBodyType::Xml,
            HttpBodyType::OtherText,
            HttpBodyType::BinaryFile,
        ] {
            assert_eq!(segs.iter().filter(|(s, ..)| *s == t).count(), 1, "{t:?}");
        }
    }
}
