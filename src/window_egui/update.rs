use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use crate::models;
use crate::auto_updater::UpdateStage;

impl super::Tabular {
    pub fn check_for_updates(&mut self, manual: bool) {
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
        if !self.show_update_dialog {
            return;
        }

        egui::Window::new("Software Update")
            .resizable(true)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .min_size(egui::vec2(620.0, 480.0))
            .show(ctx, |ui| {
                ui.set_min_width(620.0);

                if self.update_check_in_progress {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking for updates from GitHub...");
                    });
                } else if let Some(error) = &self.update_check_error {
                    ui.colored_label(
                        egui::Color32::from_rgb(255, 100, 100),
                        format!("Error: {}", error),
                    );
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("View Releases on GitHub").clicked() {
                            crate::self_update::open_url("https://github.com/tabular-id/tabular/releases");
                        }
                        if ui.button("Close").clicked() {
                            self.show_update_dialog = false;
                        }
                    });
                } else if let Some(update_info) = &self.update_info.clone() {
                    if update_info.update_available {
                        ui.heading("🚀 Tabular Update Available!");
                        ui.separator();

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

                        ui.separator();

                        ui.label("Release Notes:");
                        egui::ScrollArea::vertical()
                            .max_height(280.0)
                            .show(ui, |ui| {
                                let mut cache = CommonMarkCache::default();
                                CommonMarkViewer::new()
                                    .show(ui, &mut cache, &update_info.release_notes.clone());
                            });

                        ui.separator();

                        // Progress or Status UI
                        match &self.update_stage {
                            UpdateStage::Downloading { progress, downloaded, total } => {
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
                            }
                            UpdateStage::Extracting => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label("Extracting update archive...");
                                });
                            }
                            UpdateStage::Applying => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label("Applying update in-place...");
                                });
                            }
                        UpdateStage::Completed(_) => {
                                ui.colored_label(
                                    egui::Color32::from_rgb(100, 220, 100),
                                    "✅ Update staged successfully! Click \"Restart Now\" to apply.",
                                );
                            }
                            UpdateStage::Failed(err) => {
                                ui.colored_label(
                                    egui::Color32::from_rgb(255, 100, 100),
                                    format!("Update failed: {}", err),
                                );
                            }
                            UpdateStage::Idle => {}
                        }

                        ui.separator();

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

                            if ui.button("Later").clicked() {
                                self.show_update_dialog = false;
                            }
                        });
                    } else {
                        ui.heading("You're up to date!");
                        ui.separator();
                        ui.label(format!(
                            "Tabular {} is the latest version.",
                            update_info.current_version
                        ));
                        ui.separator();
                        if ui.button("Close").clicked() {
                            self.show_update_dialog = false;
                        }
                    }
                } else {
                    ui.label("No update information available.");
                    if ui.button("Close").clicked() {
                        self.show_update_dialog = false;
                    }
                }
            });
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
                self.update_stage = UpdateStage::Failed("Auto updater component not available".to_string());
            }
        } else {
            log::error!("❌ No update info available");
        }
    }
}
