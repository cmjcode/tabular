use std::collections::HashMap;

use eframe::egui;

use crate::models::structs::{
    RedisBrowserState, RedisBrowserTypeFilter,
};

#[derive(Clone, Debug)]
pub enum RedisBrowserAction {
    Refresh,
    SelectKeyspace { database_name: String },
    SelectKey { key_name: String, key_type: String },
    SearchServer { search_text: String },
}

fn display_key_type(key_type: &str) -> &str {
    match key_type.to_ascii_lowercase().as_str() {
        "string" => "STRING",
        "hash" => "HASH",
        "list" => "LIST",
        "set" => "SET",
        "zset" | "sorted_set" => "ZSET",
        "stream" => "STREAM",
        _ => "OTHER",
    }
}

fn elide_middle(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return "...".to_string();
    }

    let lead = (max_chars - 3) / 2;
    let tail = max_chars - 3 - lead;
    let prefix: String = text.chars().take(lead).collect();
    let suffix: String = text
        .chars()
        .rev()
        .take(tail)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{}...{}", prefix, suffix)
}

fn filtered_key_indices(state: &RedisBrowserState) -> Vec<usize> {
    let needle = state.filter_text.trim().to_ascii_lowercase();
    state
        .keys
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            state.type_filter.matches_type(&entry.key_type)
                && (needle.is_empty()
                    || entry.key_name.to_ascii_lowercase().contains(&needle)
                    || entry.key_type.to_ascii_lowercase().contains(&needle))
        })
        .map(|(index, _)| index)
        .collect()
}

fn render_json_preview(ui: &mut egui::Ui, json_text: &str) {
    let dark = ui.visuals().dark_mode;
    let available_size = ui.available_size();
    let line_count = json_text.lines().count().max(1);
    let gutter_width = ((line_count as f32).log10().floor() as usize + 1) as f32 * 8.0 + 20.0;
    let mut cache: HashMap<u64, egui::text::LayoutJob> = HashMap::new();

    let frame_fill = if dark {
        egui::Color32::from_rgb(18, 10, 10)
    } else {
        egui::Color32::from_rgb(255, 246, 246)
    };
    let gutter_fill = if dark {
        egui::Color32::from_rgb(28, 14, 14)
    } else {
        egui::Color32::from_rgb(252, 236, 236)
    };
    let gutter_text = if dark {
        egui::Color32::from_rgb(180, 120, 120)
    } else {
        egui::Color32::from_rgb(150, 96, 96)
    };

    let mut job = crate::syntax_ts::highlight_text_cached(
        json_text,
        crate::syntax_ts::LanguageKind::Redis,
        dark,
        &mut cache,
    );
    job.wrap.max_width = f32::INFINITY;

    egui::Frame::new()
        .fill(frame_fill)
        .stroke(egui::Stroke::NONE)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(0))
        .show(ui, |ui| {
            ui.set_min_size(available_size);
            egui::ScrollArea::both()
                .id_salt("redis_json_preview_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        egui::Frame::new()
                            .fill(gutter_fill)
                            .inner_margin(egui::Margin::symmetric(10, 6))
                            .show(ui, |ui| {
                                ui.set_width(gutter_width);
                                ui.vertical(|ui| {
                                    for line_number in 1..=line_count {
                                        ui.label(
                                            egui::RichText::new(format!("{:>width$}", line_number, width = line_count.to_string().len()))
                                                .monospace()
                                                .size(12.0)
                                                .color(gutter_text),
                                        );
                                    }
                                });
                            });

                        ui.add_space(8.0);
                        ui.label(job.clone());
                    });
                });
        });
}

pub fn render_redis_browser(
    ui: &mut egui::Ui,
    state: &mut RedisBrowserState,
) -> Option<RedisBrowserAction> {
    let mut action = None;
    ui.style_mut().visuals.selection.bg_fill = egui::Color32::from_rgb(169, 169, 169);
    ui.style_mut().visuals.selection.stroke.color = egui::Color32::TRANSPARENT;

    let filtered = filtered_key_indices(state);
    let trimmed_filter = state.filter_text.trim().to_string();
    if trimmed_filter.is_empty() {
        state.last_remote_search = None;
        state.remote_search_in_progress = false;
    } else if filtered.is_empty()
        && !state.remote_search_in_progress
        && state.last_remote_search.as_deref() != Some(trimmed_filter.as_str())
    {
        state.remote_search_in_progress = true;
        action = Some(RedisBrowserAction::SearchServer {
            search_text: trimmed_filter.clone(),
        });
    }

    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("Total: {}", state.keys.len())).strong(),
            );
            if !state.available_keyspaces.is_empty() {
                ui.separator();
                let mut selected_keyspace = state.keyspace_label.clone();
                egui::ComboBox::from_id_salt("redis_browser_keyspace")
                    .selected_text(selected_keyspace.clone())
                    .width(96.0)
                    .show_ui(ui, |ui| {
                        for keyspace in &state.available_keyspaces {
                            ui.selectable_value(
                                &mut selected_keyspace,
                                keyspace.clone(),
                                keyspace,
                            );
                        }
                    });
                if selected_keyspace != state.keyspace_label {
                    action = Some(RedisBrowserAction::SelectKeyspace {
                        database_name: selected_keyspace,
                    });
                }
            }
            if !state.status_text.is_empty() {
                ui.separator();
                ui.label(state.status_text.clone());
            }
            if state.remote_search_in_progress {
                ui.separator();
                ui.label("Searching Redis server...");
            }
            ui.separator();
            ui.label(format!("Visible: {}", filtered.len()));
            ui.separator();
            if ui.checkbox(&mut state.auto_refresh_enabled, "Auto Refresh").changed() {
                state.auto_refresh_last_run = None;
            }
            let mut selected_interval = state.auto_refresh_interval_seconds.max(1);
            egui::ComboBox::from_id_salt("redis_browser_auto_refresh_interval")
                .selected_text(format!("{}s", selected_interval))
                .width(72.0)
                .show_ui(ui, |ui| {
                    for seconds in [1_u32, 2, 5, 10, 15, 30, 60, 120, 300] {
                        ui.selectable_value(&mut selected_interval, seconds, format!("{}s", seconds));
                    }
                });
            if selected_interval != state.auto_refresh_interval_seconds.max(1) {
                state.auto_refresh_interval_seconds = selected_interval;
                state.auto_refresh_last_run = None;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Refresh").clicked() {
                    action = Some(RedisBrowserAction::Refresh);
                }
            });
        });

        ui.add_space(4.0);

        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("redis_browser_type_filter")
                .selected_text(state.type_filter.label())
                .width(160.0)
                .show_ui(ui, |ui| {
                    for filter in [
                        RedisBrowserTypeFilter::All,
                        RedisBrowserTypeFilter::String,
                        RedisBrowserTypeFilter::Hash,
                        RedisBrowserTypeFilter::List,
                        RedisBrowserTypeFilter::Set,
                        RedisBrowserTypeFilter::SortedSet,
                        RedisBrowserTypeFilter::Stream,
                        RedisBrowserTypeFilter::Other,
                    ] {
                        ui.selectable_value(&mut state.type_filter, filter.clone(), filter.label());
                    }
                });

            ui.add(
                egui::TextEdit::singleline(&mut state.filter_text)
                    .desired_width(f32::INFINITY)
                    .hint_text("Filter by key name or pattern"),
            );
        });

        ui.add_space(8.0);

        ui.columns(2, |columns| {
            let (left_slice, right_slice) = columns.split_at_mut(1);
            let left = &mut left_slice[0];
            let right = &mut right_slice[0];

            left.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Type").strong().size(11.0));
                    ui.add_space(48.0);
                    ui.label(egui::RichText::new("Key").strong().size(11.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new("Size").strong().size(11.0));
                        ui.add_space(36.0);
                        ui.label(egui::RichText::new("TTL").strong().size(11.0));
                    });
                });
                ui.separator();

                egui::ScrollArea::vertical()
                    .id_salt("redis_browser_left")
                    .show(ui, |ui| {
                        for index in filtered {
                            let entry = &state.keys[index];
                            let is_selected = state.selected_key.as_deref() == Some(&entry.key_name);
                            let dark = ui.visuals().dark_mode;
                            let fill = if is_selected {
                                if dark {
                                    egui::Color32::from_rgb(70, 70, 70)
                                } else {
                                    egui::Color32::from_rgb(200, 200, 200)
                                }
                            } else {
                                egui::Color32::TRANSPARENT
                            };

                            egui::Frame::new()
                                .fill(fill)
                                .corner_radius(egui::CornerRadius::same(4))
                                .inner_margin(egui::Margin::symmetric(8, 6))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        let badge = egui::RichText::new(display_key_type(&entry.key_type))
                                            .size(10.0)
                                            .color(egui::Color32::WHITE)
                                            .background_color(egui::Color32::from_rgb(255, 0, 0))
                                            .strong();
                                        ui.label(badge);

                                        let display_key = elide_middle(&entry.key_name, 72);
                                        let response = ui.selectable_label(
                                            false,
                                            egui::RichText::new(display_key).size(13.0),
                                        );
                                        let response = response.on_hover_text(&entry.key_name);
                                        if response.clicked() {
                                            action = Some(RedisBrowserAction::SelectKey {
                                                key_name: entry.key_name.clone(),
                                                key_type: entry.key_type.clone(),
                                            });
                                        }

                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.label(entry.size_label.clone());
                                            ui.add_space(24.0);
                                            ui.label(entry.ttl_label.clone());
                                        });
                                    });
                                });
                            ui.add_space(2.0);
                        }
                    });
            });

            right.vertical(|ui| {
                if let Some(preview) = &state.preview {
                    ui.horizontal(|ui| {
                        let badge = egui::RichText::new(display_key_type(&preview.key_type))
                            .size(10.0)
                            .color(egui::Color32::WHITE)
                            .background_color(egui::Color32::from_rgb(255, 0, 0))
                            .strong();
                        ui.label(badge);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(preview.key_name.clone())
                                    .heading()
                                    .strong(),
                            )
                            .wrap(),
                        )
                        .on_hover_text(&preview.key_name);
                    });
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("Keyspace: {}", preview.database_name));
                        ui.separator();
                        ui.label(format!("Length: {}", preview.length_label));
                        ui.separator();
                        ui.label(format!("TTL: {}", preview.ttl_label));
                        ui.separator();
                        ui.label(format!("Size: {}", preview.size_label));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Copy JSON").clicked() {
                                ui.ctx().copy_text(preview.json_text.clone());
                            }
                        });
                    });
                    ui.separator();

                    let preview_size = ui.available_size();
                    ui.allocate_ui(preview_size, |ui| {
                        render_json_preview(ui, &preview.json_text);
                    });
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.label("Select a Redis key to preview its JSON value.");
                    });
                }

                if let Some(error) = &state.last_error {
                    ui.add_space(8.0);
                    ui.colored_label(egui::Color32::from_rgb(255, 120, 120), error);
                }
            });
        });
    });

    action
}