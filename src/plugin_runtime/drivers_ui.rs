//! Tab "Database Drivers" di Plugin Manager: daftar driver engine plugin,
//! install dari folder, enable/disable, persetujuan sidecar, uninstall.

use crate::driver_api::manifest::{self, DriverKind, DriverStatus, InstalledDriver};
use crate::plugin_runtime::PluginModalState;
use eframe::egui;

fn refresh(state: &mut PluginModalState) {
    state.installed_drivers = Some(manifest::load_from(&manifest::drivers_dir()));
}

fn status_badge(status: &DriverStatus) -> (String, egui::Color32) {
    match status {
        DriverStatus::Loaded => ("Loaded".into(), egui::Color32::from_rgb(60, 170, 90)),
        DriverStatus::Disabled => ("Disabled".into(), egui::Color32::GRAY),
        DriverStatus::NeedsApproval => (
            "Needs approval".into(),
            egui::Color32::from_rgb(220, 150, 40),
        ),
        DriverStatus::Failed(e) => (format!("Failed: {e}"), egui::Color32::from_rgb(210, 70, 60)),
    }
}

fn open_folder(path: &std::path::Path) {
    let _ = std::fs::create_dir_all(path);
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
}

pub fn render_drivers_tab(ui: &mut egui::Ui, state: &mut PluginModalState) {
    if state.installed_drivers.is_none() {
        refresh(state);
    }
    let root = manifest::drivers_dir();

    ui.horizontal(|ui| {
        ui.heading("Database Drivers");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Open Folder").clicked() {
                open_folder(&root);
            }
            if ui.button("Reload").clicked() {
                refresh(state);
                state.status_message = Some("Driver list reloaded.".into());
            }
            #[cfg(not(any(target_os = "ios", target_os = "android")))]
            if ui
                .button("Install from Folder…")
                .on_hover_text("Select a folder that contains manifest.json")
                .clicked()
                && let Some(src) = rfd::FileDialog::new().pick_folder()
            {
                match manifest::install_from_dir(&src, &root) {
                    Ok(m) => {
                        state.status_message =
                            Some(format!("Installed {} {}.", m.engine.name, m.version));
                        state.error_message = None;
                    }
                    Err(e) => state.error_message = Some(e.to_string()),
                }
                refresh(state);
            }
        });
    });
    ui.label(
        egui::RichText::new(
            "New database engines are added as plugins. Wasm drivers run sandboxed and may only \
             reach the hosts listed in their manifest. Sidecar drivers run as native processes \
             and must be approved before they load.",
        )
        .small()
        .weak(),
    );
    ui.add_space(6.0);

    let drivers = state.installed_drivers.clone().unwrap_or_default();
    if drivers.is_empty() {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label("No database drivers installed.");
            ui.label(
                egui::RichText::new(format!("Drivers folder: {}", root.display()))
                    .small()
                    .weak(),
            );
        });
        return;
    }

    let mut action: Option<Box<dyn FnOnce(&mut PluginModalState)>> = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        for driver in &drivers {
            render_driver_card(ui, state, driver, &root, &mut action);
            ui.add_space(6.0);
        }
    });
    if let Some(act) = action {
        act(state);
        refresh(state);
    }
}

fn render_driver_card(
    ui: &mut egui::Ui,
    state: &PluginModalState,
    driver: &InstalledDriver,
    root: &std::path::Path,
    action: &mut Option<Box<dyn FnOnce(&mut PluginModalState)>>,
) {
    let m = &driver.manifest;
    let id = m.engine.id.clone();
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let icon = m.engine.icon.clone().unwrap_or_else(|| "🧩".into());
            ui.label(egui::RichText::new(format!("{icon} {}", m.engine.name)).strong());
            ui.label(egui::RichText::new(format!("v{}", m.version)).weak());
            let kind = match m.kind {
                DriverKind::Wasm => "Wasm",
                DriverKind::Sidecar => "Sidecar",
            };
            ui.label(egui::RichText::new(kind).small());
            let (text, color) = status_badge(&driver.status);
            ui.colored_label(color, text);
        });
        if !m.description.is_empty() {
            ui.label(&m.description);
        }
        let mut meta = format!("id: {id}");
        if !m.author.is_empty() {
            meta.push_str(&format!(" · by {}", m.author));
        }
        meta.push_str(&format!(
            " · sha256 {}…",
            &driver.sha256[..12.min(driver.sha256.len())]
        ));
        ui.label(egui::RichText::new(meta).small().weak());
        if m.kind == DriverKind::Wasm {
            let hosts = if m.permissions.http_hosts.is_empty() {
                "none".to_string()
            } else {
                m.permissions.http_hosts.join(", ")
            };
            ui.label(egui::RichText::new(format!("Network access: {hosts}")).small());
        }

        let awaiting = state.pending_sidecar_approval.as_deref() == Some(id.as_str());
        if awaiting {
            egui::Frame::NONE
                .fill(ui.visuals().warn_fg_color.gamma_multiply(0.12))
                .inner_margin(8.0)
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new("Run native code from this plugin?")
                            .strong()
                            .color(ui.visuals().warn_fg_color),
                    );
                    ui.label(
                        "Sidecar drivers are not sandboxed. They run with your user account's \
                         permissions and can read files and reach the network. Approve only \
                         drivers from a source you trust.",
                    );
                    ui.label(
                        egui::RichText::new(format!("Executable: {}", driver.entry_path.display()))
                            .monospace()
                            .small(),
                    );
                    ui.label(
                        egui::RichText::new(format!("SHA-256: {}", driver.sha256))
                            .monospace()
                            .small(),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Approve & Load").clicked() {
                            let (id, sha, root) =
                                (id.clone(), driver.sha256.clone(), root.to_path_buf());
                            *action = Some(Box::new(move |st: &mut PluginModalState| {
                                st.pending_sidecar_approval = None;
                                match manifest::approve_sidecar(&id, &sha, &root) {
                                    Ok(()) => st.status_message = Some(format!("Approved '{id}'.")),
                                    Err(e) => st.error_message = Some(e.to_string()),
                                }
                            }));
                        }
                        if ui.button("Cancel").clicked() {
                            *action = Some(Box::new(|st: &mut PluginModalState| {
                                st.pending_sidecar_approval = None;
                            }));
                        }
                    });
                });
        }

        ui.horizontal(|ui| {
            match driver.status {
                DriverStatus::NeedsApproval
                    if !awaiting && ui.button("Review & Approve…").clicked() =>
                {
                    let id = id.clone();
                    *action = Some(Box::new(move |st: &mut PluginModalState| {
                        st.pending_sidecar_approval = Some(id);
                    }));
                }
                DriverStatus::Disabled if ui.button("Enable").clicked() => {
                    let (id, root) = (id.clone(), root.to_path_buf());
                    *action = Some(Box::new(move |st: &mut PluginModalState| {
                        if let Err(e) = manifest::set_enabled(&id, true, &root) {
                            st.error_message = Some(e.to_string());
                        }
                    }));
                }
                DriverStatus::Loaded | DriverStatus::Failed(_)
                    if ui.button("Disable").clicked() =>
                {
                    let (id, root) = (id.clone(), root.to_path_buf());
                    *action = Some(Box::new(move |st: &mut PluginModalState| {
                        if let Err(e) = manifest::set_enabled(&id, false, &root) {
                            st.error_message = Some(e.to_string());
                        }
                    }));
                }
                _ => {}
            }
            if ui
                .button("Uninstall")
                .on_hover_text(
                    "Connections using this engine are kept and show 'Driver not installed'",
                )
                .clicked()
            {
                let (id, root) = (id.clone(), root.to_path_buf());
                *action = Some(Box::new(
                    move |st: &mut PluginModalState| match manifest::uninstall(&id, &root) {
                        Ok(()) => st.status_message = Some(format!("Uninstalled '{id}'.")),
                        Err(e) => st.error_message = Some(e.to_string()),
                    },
                ));
            }
        });
    });
}
