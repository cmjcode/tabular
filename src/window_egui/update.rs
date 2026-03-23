use eframe::egui;
use crate::models;

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
            .resizable(false)
            .collapsible(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(400.0);

                if self.update_check_in_progress {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking for updates...");
                    });
                } else if let Some(error) = &self.update_check_error {
                    ui.colored_label(
                        egui::Color32::from_rgb(255, 0, 0),
                        format!("Error: {}", error),
                    );
                    ui.separator();
                    if ui.button("Close").clicked() {
                        self.show_update_dialog = false;
                    }
                } else if let Some(update_info) = &self.update_info.clone() {
                    if update_info.update_available {
                        ui.heading("Update Available!");
                        ui.separator();

                        ui.horizontal(|ui| {
                            ui.label("Current version:");
                            ui.strong(&update_info.current_version);
                        });

                        ui.horizontal(|ui| {
                            ui.label("Latest version:");
                            ui.strong(&update_info.latest_version);
                        });

                        if let Some(published_at) = &update_info.published_at {
                            ui.label(format!("Released: {}", published_at));
                        }

                        ui.separator();

                        ui.label("Release Notes:");
                        egui::ScrollArea::vertical()
                            .max_height(200.0)
                            .show(ui, |ui| {
                                ui.text_edit_multiline(&mut update_info.release_notes.clone());
                            });

                        ui.separator();

                        ui.horizontal(|ui| {
                            if update_info.download_url.is_some() {
                                if self.update_download_in_progress {
                                    ui.add_enabled(false, egui::Button::new("Downloading..."));
                                    ui.spinner();
                                } else if ui.button("Update Now").clicked() {
                                    self.start_update_download();
                                }
                            } else {
                                // No download URL available - show manual download option
                                ui.colored_label(
                                    egui::Color32::from_rgb(255, 0, 0),
                                    "Auto-update not available for this platform",
                                );
                            }

                            if ui.button("View Release").clicked() {
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
        log::info!("🚀 Starting auto update process...");

        // Prevent multiple simultaneous downloads
        if self.update_download_in_progress {
            log::warn!("⚠️ Download already in progress, ignoring request");
            return;
        }

        // Prevent re-downloading if already completed
        if self.update_installed {
            log::warn!("⚠️ Update already downloaded, ignoring request");
            return;
        }

        if let Some(update_info) = &self.update_info {
            if let Some(auto_updater) = &self.auto_updater {
                log::info!(
                    "📦 Update info available: {} -> {}",
                    update_info.current_version,
                    update_info.latest_version
                );
                log::info!("📥 Download URL: {:?}", update_info.download_url);
                log::info!("📄 Asset name: {:?}", update_info.asset_name);

                self.update_download_in_progress = true;
                // Prepare channel to receive completion signal
                let (tx, rx) = std::sync::mpsc::channel();
                self.update_install_receiver = Some(rx);

                let update_info_clone = update_info.clone();
                let auto_updater_clone = auto_updater.clone();

                std::thread::spawn(move || {
                    log::info!("🔄 Background update thread started (auto updater)");

                    // Create a completely new, independent Tokio runtime for the update process
                    let rt = match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(rt) => rt,
                        Err(e) => {
                            log::error!("❌ Failed to create update runtime: {}", e);
                            let _ = tx.send(false);
                            return;
                        }
                    };

                    match rt
                        .block_on(auto_updater_clone.download_and_stage_update(&update_info_clone))
                    {
                        Ok(()) => {
                            log::info!("✅ Update staged successfully");
                            let _ = tx.send(true);
                        }
                        Err(e) => {
                            log::error!("❌ Update failed: {}", e);
                            let _ = tx.send(false);
                        }
                    }
                });
            } else {
                log::error!("❌ Auto updater not available");
                self.update_download_in_progress = false;
            }
        } else {
            log::error!("❌ No update info available");
        }
    }
}
