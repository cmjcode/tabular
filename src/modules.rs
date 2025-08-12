use eframe::egui;

use log::{debug};


pub(crate) fn url_encode(input: &str) -> String {
       input
       .replace("%", "%25")  // Must be first
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

// Basic percent-decoder for credentials in connection URLs
pub(crate) fn url_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h1 = bytes[i + 1];
            let h2 = bytes[i + 2];
            let hex = |c: u8| -> Option<u8> {
                match c {
                    b'0'..=b'9' => Some(c - b'0'),
                    b'a'..=b'f' => Some(10 + c - b'a'),
                    b'A'..=b'F' => Some(10 + c - b'A'),
                    _ => None,
                }
            };
            if let (Some(x), Some(y)) = (hex(h1), hex(h2)) {
                out.push((x * 16 + y) as char);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
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

