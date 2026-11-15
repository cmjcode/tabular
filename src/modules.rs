use eframe::egui;

use log::debug;

pub(crate) fn url_encode(input: &str) -> String {
    input
        .replace("%", "%25") // Must be first
        .replace("#", "%23")
        .replace("&", "%26")
        .replace("@", "%40")
        .replace("?", "%3F")
        .replace("=", "%3D")
        .replace("+", "%2B")
        .replace(" ", "%20")
        .replace(":", "%3A")
        .replace("/", "%2F")
}

pub(crate) fn load_icon() -> Option<egui::IconData> {
    let icon_bytes = include_bytes!("../assets/logo.png");

    match image::load_from_memory(icon_bytes) {
        Ok(image) => {
            let rgba = image.to_rgba8();
            let (width, height) = rgba.dimensions();
            Some(egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            })
        }
        Err(e) => {
            debug!("Failed to load icon: {}", e);
            None
        }
    }
}
