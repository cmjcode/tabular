use eframe::egui;
use log::error;

use crate::{editor, window_egui};


fn load_logo_texture(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
    if tabular.logo_texture.is_some() {
        return;
    }

    // Try filesystem asset first (useful during dev runs)
    let bytes_from_fs = std::fs::read("assets/logo.png").ok();

    // Fallback to embedded bytes so packaged apps always show the logo
    // SAFETY: the file path is compile-time checked
    let embedded_bytes: &[u8] = include_bytes!("../assets/logo.png");

    let image_bytes: Vec<u8> = bytes_from_fs.unwrap_or_else(|| embedded_bytes.to_vec());

    if let Ok(image) = image::load_from_memory(&image_bytes) {
        let rgba_image = image.to_rgba8();
        let size = [image.width() as usize, image.height() as usize];
        let pixels = rgba_image.as_flat_samples();
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());
        tabular.logo_texture = Some(ctx.load_texture("logo", color_image, Default::default()));
    }
}
pub(crate) fn render_about_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
        if tabular.show_about_dialog {
            // Load logo texture if not already loaded
            load_logo_texture(tabular, ctx);
            
            egui::Window::new("About Tabular")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .default_width(400.0)
                .open(&mut tabular.show_about_dialog)
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(10.0);
                        
                        // App icon/logo - use actual logo if loaded, fallback to emoji
                        if let Some(logo_texture) = &tabular.logo_texture {
                            ui.add(egui::Image::from_texture(logo_texture).max_size(egui::vec2(64.0, 64.0)));
                        } else {
                            ui.label(egui::RichText::new("📊").size(48.0));
                        }
                        ui.add_space(10.0);
                        
                        // App name and version
                        ui.label(egui::RichText::new("Tabular").size(24.0).strong());
                        ui.label(egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).size(14.0).color(egui::Color32::GRAY));
                        ui.add_space(15.0);
                        
                        // Description
                        ui.label("Your SQL Editor, Forged with Rust: Fast, Safe, Efficient.");
                        ui.label("Jayuda");
                        ui.add_space(10.0);
                        
                       
                        ui.label(egui::RichText::new("© 2025 PT. Vneu Teknologi Indonesia ").size(12.0).color(egui::Color32::GRAY));
                        ui.hyperlink_to("https://github.com/tabular-id/tabular", "https://github.com/tabular-id/tabular");
                        ui.label(egui::RichText::new("Built with ❤️ using Rust").size(12.0).color(egui::Color32::GRAY));
                        ui.add_space(15.0);
                    });
                });
        }
    }


pub(crate) fn render_error_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
        if tabular.show_error_message {
            egui::Window::new("Error")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(&tabular.error_message);
                    ui.separator();
                    
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() {
                            tabular.show_error_message = false;
                            tabular.error_message.clear();
                        }
                    });
                });
        }
    }    



pub(crate) fn render_save_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
        if tabular.show_save_dialog {
            egui::Window::new("Save Query")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Enter filename:");
                    ui.text_edit_singleline(&mut tabular.save_filename);
                    
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() && !tabular.save_filename.is_empty() {
                            if let Err(err) = editor::save_current_tab_with_name(tabular,tabular.save_filename.clone()) {
                                error!("Failed to save: {}", err);
                            }
                            tabular.show_save_dialog = false;
                            tabular.save_filename.clear();
                        }
                        
                        if ui.button("Cancel").clicked() {
                            tabular.show_save_dialog = false;
                            tabular.save_filename.clear();
                        }
                    });
                });
        }
    }


