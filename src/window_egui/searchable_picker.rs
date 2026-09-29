//! Picker dropdown dengan kotak pencarian untuk toolbar (koneksi, database, schema).
//!
//! Pengganti `egui::ComboBox` yang sulit dipakai saat item banyak: popup lebih lebar,
//! field search langsung fokus saat dibuka, pencocokan multi-kata (mis. "flex prod"),
//! highlight bagian yang cocok, serta navigasi keyboard (↑/↓, Enter, Esc).

use eframe::egui;

use super::style;

/// Konfigurasi tampilan satu picker.
pub struct PickerConfig<'a> {
    /// Salt id unik per picker.
    pub id_salt: &'a str,
    /// Ikon di sisi kiri tombol (codepoint `egui_icons`).
    pub icon: &'a str,
    /// Judul kecil di header popup, mis. "Databases".
    pub title: &'a str,
    /// Hint pada field pencarian.
    pub search_hint: &'a str,
    /// Tooltip tombol (nama lengkap item aktif ditambahkan otomatis).
    pub tooltip: &'a str,
    /// Lebar minimum & maksimum tombol.
    pub min_width: f32,
    pub max_width: f32,
    pub is_touch: bool,
}

/// State per picker yang disimpan di memori egui (temp data).
#[derive(Clone, Default)]
struct PickerState {
    query: String,
    /// Indeks baris yang di-highlight dalam daftar hasil filter.
    highlighted: usize,
    /// Popup sudah terbuka pada frame sebelumnya.
    was_open: bool,
    /// Scroll ke baris highlight pada frame ini (setelah navigasi keyboard / buka).
    scroll_to_highlight: bool,
}

/// Menggambar tombol picker dan popup-nya. Mengembalikan indeks `items`
/// yang dipilih pengguna pada frame ini.
pub fn searchable_picker(
    ui: &mut egui::Ui,
    cfg: &PickerConfig<'_>,
    selected_text: &str,
    items: &[String],
    selected: Option<usize>,
) -> Option<usize> {
    let ctx = ui.ctx().clone();
    let id = ui.make_persistent_id(cfg.id_salt);
    let popup_id = id.with("popup");
    let is_open = egui::Popup::is_id_open(&ctx, popup_id);

    let button = draw_button(ui, cfg, selected_text, is_open);
    let button = if selected_text.is_empty() {
        button.on_hover_text(cfg.tooltip)
    } else {
        button.on_hover_text(format!("{}: {}", cfg.tooltip, selected_text))
    };

    let mut state: PickerState = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let mut picked = None;

    let popup_width = button
        .rect
        .width()
        .max(if cfg.is_touch { 360.0 } else { 320.0 });
    let shown = egui::Popup::menu(&button)
        .id(popup_id)
        .width(popup_width)
        .align(egui::RectAlign::BOTTOM_END)
        .gap(4.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(popup_width);
            ui.set_max_width(popup_width);
            picked = popup_contents(ui, cfg, &mut state, items, selected);
        });

    if shown.is_some() {
        state.was_open = true;
    } else if state.was_open {
        // Popup tertutup: bersihkan query supaya pembukaan berikutnya mulai dari awal.
        state = PickerState::default();
    }
    if picked.is_some() {
        egui::Popup::close_id(&ctx, popup_id);
        state = PickerState::default();
    }
    ctx.data_mut(|d| d.insert_temp(id, state));
    picked
}

fn draw_button(
    ui: &mut egui::Ui,
    cfg: &PickerConfig<'_>,
    selected_text: &str,
    is_open: bool,
) -> egui::Response {
    let ctx = ui.ctx().clone();
    let height = if cfg.is_touch { 38.0 } else { 26.0 };
    let font_size = if cfg.is_touch { 14.5 } else { 12.5 };
    let icon_size = if cfg.is_touch { 16.0 } else { 13.5 };
    let pad_x = 9.0;
    let icon_w = icon_size + 6.0;
    let chevron_w = icon_size + 2.0;

    let strong = style::nav_text_strong(&ctx);
    let muted = style::nav_text_muted(&ctx);

    // Hitung lebar teks untuk menentukan lebar tombol yang pas (dengan batas min/max).
    let natural_text_w = ui
        .painter()
        .layout_no_wrap(
            selected_text.to_string(),
            egui::FontId::proportional(font_size),
            strong,
        )
        .size()
        .x;
    let chrome_w = pad_x * 2.0 + icon_w + chevron_w + 4.0;
    let width = (natural_text_w + chrome_w).clamp(cfg.min_width, cfg.max_width);

    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let fill = if is_open || hovered {
            style::nav_raised(&ctx)
        } else {
            style::nav_track(&ctx)
        };
        let border = if is_open {
            ui.visuals().widgets.active.bg_stroke.color
        } else if hovered {
            ui.visuals().widgets.hovered.bg_stroke.color
        } else {
            style::nav_border(&ctx)
        };
        let radius = egui::CornerRadius::same(6);
        let painter = ui.painter();
        painter.rect_filled(rect, radius, fill);
        painter.rect_stroke(
            rect,
            radius,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );

        // Ikon kiri
        painter.text(
            egui::pos2(rect.left() + pad_x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            cfg.icon,
            egui::FontId::proportional(icon_size),
            if is_open { strong } else { muted },
        );

        // Chevron kanan
        painter.text(
            egui::pos2(rect.right() - pad_x + 2.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            egui_icons::icons::ICON_EXPAND_MORE.codepoint,
            egui::FontId::proportional(icon_size + 2.0),
            muted,
        );

        // Teks item aktif, dipotong dengan elipsis bila tidak muat.
        let text_left = rect.left() + pad_x + icon_w;
        let text_max_w = (rect.right() - pad_x - chevron_w - 4.0 - text_left).max(10.0);
        let mut job = egui::text::LayoutJob::single_section(
            selected_text.to_string(),
            egui::TextFormat::simple(egui::FontId::proportional(font_size), strong),
        );
        job.wrap = egui::text::TextWrapping {
            max_width: text_max_w,
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        let text_pos = egui::pos2(text_left, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(text_pos, galley, strong);
    }

    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn popup_contents(
    ui: &mut egui::Ui,
    cfg: &PickerConfig<'_>,
    state: &mut PickerState,
    items: &[String],
    selected: Option<usize>,
) -> Option<usize> {
    let ctx = ui.ctx().clone();
    let muted = style::nav_text_muted(&ctx);
    let strong = style::nav_text_strong(&ctx);
    let accent = style::theme_accent(&ctx);
    let just_opened = !state.was_open;

    // Header: judul + jumlah item
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(cfg.title.to_uppercase())
                .size(10.5)
                .strong()
                .color(muted),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(items.len().to_string())
                    .size(10.5)
                    .color(muted),
            );
        });
    });
    ui.add_space(2.0);

    let filtered = filter_items(items, &state.query);

    if just_opened {
        state.highlighted = selected
            .and_then(|s| filtered.iter().position(|&i| i == s))
            .unwrap_or(0);
        state.scroll_to_highlight = true;
    }

    // Tangkap tombol navigasi sebelum TextEdit memprosesnya.
    let (down, up, enter) = ui.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
        )
    });
    if !filtered.is_empty() {
        let last = filtered.len() - 1;
        if down {
            state.highlighted = if state.highlighted >= last {
                0
            } else {
                state.highlighted + 1
            };
            state.scroll_to_highlight = true;
        }
        if up {
            state.highlighted = if state.highlighted == 0 {
                last
            } else {
                state.highlighted - 1
            };
            state.scroll_to_highlight = true;
        }
    }
    state.highlighted = state.highlighted.min(filtered.len().saturating_sub(1));
    if enter && let Some(&idx) = filtered.get(state.highlighted) {
        return Some(idx);
    }

    // Field pencarian
    let prev_query = state.query.clone();
    let search =
        style::render_search_field_live(ui, &mut state.query, cfg.search_hint, f32::INFINITY);
    if just_opened {
        search.request_focus();
    }
    if state.query != prev_query {
        state.highlighted = 0;
        state.scroll_to_highlight = true;
    }
    let filtered = if state.query != prev_query {
        filter_items(items, &state.query)
    } else {
        filtered
    };

    ui.add_space(6.0);

    let mut picked = None;
    let row_h = if cfg.is_touch { 38.0 } else { 28.0 };
    let font_size = if cfg.is_touch { 14.5 } else { 13.0 };
    let tokens = query_tokens(&state.query);

    if filtered.is_empty() {
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new("No matches").size(12.5).color(strong));
            ui.label(
                egui::RichText::new(format!(
                    "Nothing matches \u{201C}{}\u{201D}",
                    state.query.trim()
                ))
                .size(11.5)
                .color(muted),
            );
        });
        ui.add_space(10.0);
    } else {
        let max_h = if cfg.is_touch { 460.0 } else { 380.0 };
        egui::ScrollArea::vertical()
            .max_height(max_h)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                let pointer_moved = ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
                for (row, &idx) in filtered.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), row_h),
                        egui::Sense::click(),
                    );
                    // Hover mouse memindahkan highlight, tapi hanya bila mouse benar-benar
                    // bergerak agar tidak bentrok dengan navigasi keyboard.
                    if resp.hovered() && pointer_moved {
                        state.highlighted = row;
                    }
                    let is_highlighted = row == state.highlighted;
                    let is_selected = selected == Some(idx);

                    if state.scroll_to_highlight && is_highlighted {
                        resp.scroll_to_me(Some(egui::Align::Center));
                    }

                    if ui.is_rect_visible(rect) {
                        let painter = ui.painter();
                        let radius = egui::CornerRadius::same(5);
                        if is_highlighted {
                            painter.rect_filled(
                                rect,
                                radius,
                                ui.visuals().widgets.hovered.weak_bg_fill,
                            );
                        } else if is_selected {
                            painter.rect_filled(rect, radius, accent.gamma_multiply(0.10));
                        }
                        if is_selected {
                            // Penanda aktif: garis aksen tipis di kiri + ikon centang.
                            let bar = egui::Rect::from_min_size(
                                rect.left_top() + egui::vec2(0.0, 6.0),
                                egui::vec2(2.5, rect.height() - 12.0),
                            );
                            painter.rect_filled(bar, egui::CornerRadius::same(1), accent);
                            painter.text(
                                egui::pos2(rect.right() - 10.0, rect.center().y),
                                egui::Align2::RIGHT_CENTER,
                                egui_icons::icons::ICON_CHECK.codepoint,
                                egui::FontId::proportional(font_size + 2.0),
                                accent,
                            );
                        }

                        let text_left = rect.left() + 12.0;
                        let text_max_w = rect.right() - 30.0 - text_left;
                        let base = if is_selected || is_highlighted {
                            strong
                        } else {
                            strong.gamma_multiply(0.85)
                        };
                        let mut job = highlight_job(
                            &items[idx],
                            &tokens,
                            font_size,
                            base,
                            accent,
                            is_selected,
                        );
                        job.wrap = egui::text::TextWrapping {
                            max_width: text_max_w.max(10.0),
                            max_rows: 1,
                            break_anywhere: true,
                            overflow_character: Some('…'),
                        };
                        let galley = ui.fonts_mut(|f| f.layout_job(job));
                        let pos = egui::pos2(text_left, rect.center().y - galley.size().y / 2.0);
                        ui.painter().galley(pos, galley, base);
                    }

                    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if resp.clicked() {
                        picked = Some(idx);
                    }
                }
            });
    }
    state.scroll_to_highlight = false;

    // Footer: petunjuk keyboard. Panah & ↵ hanya ada di font monospace.
    ui.add_space(4.0);
    ui.separator();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let key = |ui: &mut egui::Ui, k: &str| {
            ui.label(
                egui::RichText::new(k)
                    .family(egui::FontFamily::Monospace)
                    .size(10.5)
                    .color(strong),
            );
        };
        let hint = |ui: &mut egui::Ui, t: &str| {
            ui.label(egui::RichText::new(t).size(10.5).color(muted));
        };
        key(ui, "↑↓");
        hint(ui, "navigate   ");
        key(ui, "↵");
        hint(ui, "select   ");
        key(ui, "esc");
        hint(ui, "close");
        if !state.query.trim().is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                hint(ui, &format!("{} of {}", filtered.len(), items.len()));
            });
        }
    });

    picked
}

/// Pecah query jadi token huruf kecil; semua token harus cocok (urutan bebas).
fn query_tokens(query: &str) -> Vec<String> {
    query.split_whitespace().map(|t| t.to_lowercase()).collect()
}

/// Kembalikan indeks item yang cocok. Item yang diawali token pertama
/// ditaruh di atas; selebihnya mengikuti urutan asli.
fn filter_items(items: &[String], query: &str) -> Vec<usize> {
    let tokens = query_tokens(query);
    if tokens.is_empty() {
        return (0..items.len()).collect();
    }
    let mut matched: Vec<(bool, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, item)| {
            let lower = item.to_lowercase();
            tokens
                .iter()
                .all(|t| lower.contains(t.as_str()))
                .then(|| (!lower.starts_with(tokens[0].as_str()), i))
        })
        .collect();
    matched.sort();
    matched.into_iter().map(|(_, i)| i).collect()
}

/// Bangun `LayoutJob` dengan bagian yang cocok diberi warna aksen & tebal.
fn highlight_job(
    text: &str,
    tokens: &[String],
    font_size: f32,
    base: egui::Color32,
    accent: egui::Color32,
    is_selected: bool,
) -> egui::text::LayoutJob {
    let mut marks = vec![false; text.len()];
    let lower = text.to_lowercase();
    // Offset byte hanya bisa dipetakan balik bila panjang lowercase sama.
    if lower.len() == text.len() {
        for t in tokens {
            let mut from = 0;
            while let Some(pos) = lower[from..].find(t.as_str()) {
                let start = from + pos;
                let end = start + t.len();
                marks[start..end].iter_mut().for_each(|m| *m = true);
                from = end;
                if t.is_empty() || from >= lower.len() {
                    break;
                }
            }
        }
    }

    let font = egui::FontId::proportional(font_size);
    let normal = egui::TextFormat {
        font_id: font.clone(),
        color: base,
        ..Default::default()
    };
    let hit = egui::TextFormat {
        font_id: font,
        color: if is_selected {
            base
        } else {
            accent.linear_multiply(0.95)
        },
        underline: egui::Stroke::new(1.0, accent),
        ..Default::default()
    };

    let mut job = egui::text::LayoutJob::default();
    let mut seg_start = 0;
    let mut seg_hit = marks.first().copied().unwrap_or(false);
    for (i, _) in text.char_indices().skip(1) {
        if marks[i] != seg_hit {
            job.append(
                &text[seg_start..i],
                0.0,
                if seg_hit { hit.clone() } else { normal.clone() },
            );
            seg_start = i;
            seg_hit = marks[i];
        }
    }
    if seg_start < text.len() {
        job.append(&text[seg_start..], 0.0, if seg_hit { hit } else { normal });
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<String> {
        [
            "CHIRON_TL_PROD",
            "ERP_FLEXURIO_DEV",
            "ERP_FLEXURIO_PROD",
            "FLEXURIO_STUDIO_PROD",
            "REPORTWH_MF_PROD",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn query_kosong_mengembalikan_semua_item() {
        assert_eq!(filter_items(&items(), "  "), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn multi_token_tidak_peka_huruf_dan_urutan() {
        assert_eq!(filter_items(&items(), "prod flex"), vec![2, 3]);
    }

    #[test]
    fn prefix_match_didahulukan() {
        assert_eq!(filter_items(&items(), "flexurio"), vec![3, 1, 2]);
    }

    #[test]
    fn highlight_menandai_bagian_yang_cocok() {
        let job = highlight_job(
            "ERP_FLEXURIO_PROD",
            &query_tokens("flex"),
            13.0,
            egui::Color32::WHITE,
            egui::Color32::RED,
            false,
        );
        let parts: Vec<&str> = job
            .sections
            .iter()
            .map(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0])
            .collect();
        assert_eq!(parts, vec!["ERP_", "FLEX", "URIO_PROD"]);
    }
}
