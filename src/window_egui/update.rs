use crate::auto_updater::UpdateStage;
use crate::models;
use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

impl super::Tabular {
    pub fn check_for_updates(&mut self, manual: bool) {
        // iOS ships only through the App Store — see SELF_UPDATE_SUPPORTED.
        // Guarded here as well as at the call sites so no future UI can
        // accidentally reintroduce an out-of-store update path.
        if !crate::self_update::SELF_UPDATE_SUPPORTED {
            return;
        }
        if self.update_check_in_progress {
            return; // Already checking
        }

        self.update_check_in_progress = true;
        self.update_check_error = None;
        self.last_update_check = Some(std::time::Instant::now());
        self.manual_update_check = manual;

        // Persist last check time to avoid multiple checks within 24 hours
        if let (Some(store), Some(rt)) = (self.config_store.as_ref(), self.runtime.as_ref()) {
            rt.block_on(store.set_last_update_check_now());
        }

        // Send background task to check for updates
        if let Some(sender) = &self.background_sender {
            let _ = sender.send(models::enums::BackgroundTask::CheckForUpdates);
        }
    }

    pub fn render_update_dialog(&mut self, ctx: &egui::Context) {
        if !crate::self_update::SELF_UPDATE_SUPPORTED {
            return;
        }
        if !self.show_update_dialog {
            return;
        }
        crate::window_egui::style::render_modal_backdrop(
            ctx,
            "update_dialog_backdrop",
            self.show_update_dialog,
        );

        let mut close = false;

        egui::Window::new("Software Update")
            .title_bar(false)
            .frame(crate::window_egui::style::modal_window_frame(ctx))
            .resizable(true)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .default_width(620.0)
            .default_height(480.0)
            .show(ctx, |ui| {
                crate::window_egui::style::render_modal_header(ui, "Software Update", &mut close);
                ui.add_space(8.0);

                if self.update_check_in_progress {
                    crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Checking for updates from GitHub...");
                        });
                    });
                } else if let Some(error) = &self.update_check_error {
                    crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                        ui.colored_label(
                            egui::Color32::from_rgb(255, 100, 100),
                            format!("Error: {}", error),
                        );
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("View Releases on GitHub").clicked() {
                            crate::self_update::open_url("https://github.com/tabular-id/tabular/releases");
                        }
                    });
                } else if let Some(update_info) = &self.update_info.clone() {
                    if update_info.update_available {
                        crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                            ui.heading("🚀 Tabular Update Available!");
                            ui.add_space(4.0);

                            ui.horizontal(|ui| {
                                ui.label("Current version:");
                                ui.strong(&update_info.current_version);
                                ui.label("➡");
                                ui.label("Latest version:");
                                ui.strong(&update_info.latest_version);
                            });

                            if let Some(published_at) = &update_info.published_at {
                                ui.label(format!("Released: {}", published_at));
                            }
                        });

                        ui.add_space(8.0);

                        crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                            ui.label(egui::RichText::new("Release Notes:").strong());
                            ui.add_space(4.0);
                            let avail_h = (ui.available_height() - 90.0).max(140.0);
                            egui::ScrollArea::vertical()
                                .max_height(avail_h)
                                .show(ui, |ui| {
                                    let mut cache = CommonMarkCache::default();
                                    CommonMarkViewer::new()
                                        .show(ui, &mut cache, &update_info.release_notes.clone());
                                });
                        });

                        ui.add_space(8.0);

                        // Progress or Status UI
                        match &self.update_stage {
                            UpdateStage::Downloading { progress, downloaded, total } => {
                                crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                                    ui.vertical(|ui| {
                                        let mb_downloaded = *downloaded as f32 / (1024.0 * 1024.0);
                                        let progress_text = if let Some(tot) = total {
                                            let mb_total = *tot as f32 / (1024.0 * 1024.0);
                                            format!("{:.1}% ({:.1} MB / {:.1} MB)", progress * 100.0, mb_downloaded, mb_total)
                                        } else {
                                            format!("{:.1} MB downloaded", mb_downloaded)
                                        };
                                        ui.add(egui::ProgressBar::new(*progress).text(progress_text));
                                        ui.horizontal(|ui| {
                                            ui.spinner();
                                            ui.label("Downloading latest release payload...");
                                        });
                                    });
                                });
                            }
                            UpdateStage::Extracting => {
                                crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label("Extracting update archive...");
                                    });
                                });
                            }
                            UpdateStage::Applying => {
                                crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label("Applying update in-place...");
                                    });
                                });
                            }
                            UpdateStage::Completed(_) => {
                                crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(100, 220, 100),
                                        "✅ Update staged successfully! Click \"Restart Now\" to apply.",
                                    );
                                });
                            }
                            UpdateStage::Failed(err) => {
                                crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(255, 100, 100),
                                        format!("Update failed: {}", err),
                                    );
                                });
                            }
                            UpdateStage::Idle => {}
                        }

                        ui.add_space(10.0);

                        ui.horizontal(|ui| {
                            if self.update_installed || matches!(self.update_stage, UpdateStage::Completed(_)) {
                                if ui.button("🚀 Restart Now").clicked() {
                                    let staged = self.staged_update_script.as_ref();
                                    let _ = crate::auto_updater::AutoUpdater::restart_app(staged);
                                }
                            } else if self.update_download_in_progress {
                                ui.add_enabled(false, egui::Button::new("Updating..."));
                            } else if update_info.download_url.is_some() {
                                if ui.button("Update Now").clicked() {
                                    self.start_update_download();
                                }
                            } else {
                                ui.colored_label(
                                    egui::Color32::from_rgb(255, 100, 100),
                                    "Auto-update asset not available for this platform",
                                );
                            }

                            if ui.button("View Release Page").clicked() {
                                crate::self_update::open_release_page(update_info);
                            }
                        });
                    } else {
                        crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                            ui.heading("You're up to date!");
                            ui.add_space(4.0);
                            ui.label(format!(
                                "Tabular {} is the latest version.",
                                update_info.current_version
                            ));
                        });
                    }
                } else {
                    crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                        ui.label("No update information available.");
                    });
                }
            });

        if close {
            self.show_update_dialog = false;
        }
    }

    pub fn start_update_download(&mut self) {
        log::info!("🚀 Starting automatic update process...");

        // Prevent multiple simultaneous downloads
        if self.update_download_in_progress {
            log::warn!("⚠️ Update already in progress, ignoring request");
            return;
        }

        if self.update_installed {
            log::warn!("⚠️ Update already installed, ignoring request");
            return;
        }

        if let Some(update_info) = &self.update_info {
            if let Some(auto_updater) = &self.auto_updater {
                log::info!(
                    "📦 Auto updating Tabular: {} -> {}",
                    update_info.current_version,
                    update_info.latest_version
                );

                self.update_download_in_progress = true;
                self.update_stage = UpdateStage::Downloading {
                    progress: 0.0,
                    downloaded: 0,
                    total: None,
                };

                let (tx, rx) = std::sync::mpsc::channel();
                self.update_stage_receiver = Some(rx);

                let update_info_clone = update_info.clone();
                let auto_updater_clone = auto_updater.clone();

                std::thread::spawn(move || {
                    log::debug!("🔄 Background update thread running");

                    let rt = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(rt) => rt,
                        Err(e) => {
                            log::error!("❌ Failed to create update runtime: {}", e);
                            let _ = tx.send(UpdateStage::Failed(e.to_string()));
                            return;
                        }
                    };

                    let tx_cb = tx.clone();
                    let res = rt.block_on(auto_updater_clone.download_and_stage_update(
                        &update_info_clone,
                        move |stage| {
                            let _ = tx_cb.send(stage);
                        },
                    ));

                    if let Err(e) = res {
                        log::error!("❌ Auto update failed: {}", e);
                        let _ = tx.send(UpdateStage::Failed(e.to_string()));
                    }
                });
            } else {
                log::error!("❌ Auto updater component not available");
                self.update_download_in_progress = false;
                self.update_stage =
                    UpdateStage::Failed("Auto updater component not available".to_string());
            }
        } else {
            log::error!("❌ No update info available");
        }
    }
}
