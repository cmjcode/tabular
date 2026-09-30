//! Avatar author untuk Git Graph.
//!
//! Default memakai inisial berwarna (tanpa jaringan). Bila user memilih
//! Gravatar di Preferences, hash MD5 email dikirim ke gravatar.com; gambar
//! diunduh di thread latar dan di-cache di `{data_dir}/git_avatars/`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;

use crate::git::repos::AvatarSource;

enum Slot {
    Loading,
    Ready(egui::TextureHandle),
    Failed,
}

pub struct AvatarCache {
    slots: HashMap<String, Slot>,
    tx: mpsc::Sender<(String, Option<egui::ColorImage>)>,
    rx: mpsc::Receiver<(String, Option<egui::ColorImage>)>,
    loading: usize,
}

impl Default for AvatarCache {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            slots: HashMap::new(),
            tx,
            rx,
            loading: 0,
        }
    }
}

fn cache_dir() -> PathBuf {
    crate::config::get_data_dir().join("git_avatars")
}

fn email_hash(email: &str) -> String {
    format!("{:x}", md5::compute(email.trim().to_lowercase().as_bytes()))
}

fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

fn fetch(hash: &str) -> Option<egui::ColorImage> {
    let file = cache_dir().join(format!("{hash}.png"));
    if let Ok(bytes) = std::fs::read(&file) {
        return decode(&bytes);
    }
    let url = format!("https://www.gravatar.com/avatar/{hash}?s=64&d=identicon");
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;
    let resp = client.get(url).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().ok()?;
    if let Err(e) = std::fs::create_dir_all(cache_dir()).and_then(|_| std::fs::write(&file, &bytes))
    {
        log::debug!("[GIT] avatar cache write failed: {e}");
    }
    decode(&bytes)
}

impl AvatarCache {
    pub fn is_loading(&self) -> bool {
        self.loading > 0
    }

    /// Terima gambar yang selesai diunduh.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((email, img)) = self.rx.try_recv() {
            self.loading = self.loading.saturating_sub(1);
            let slot = match img {
                Some(img) => Slot::Ready(ctx.load_texture(
                    format!("git-avatar-{email}"),
                    img,
                    egui::TextureOptions::LINEAR,
                )),
                None => Slot::Failed,
            };
            self.slots.insert(email, slot);
        }
    }

    /// Tekstur avatar bila sudah tersedia; memulai unduhan bila belum.
    pub fn get(&mut self, source: AvatarSource, email: &str) -> Option<egui::TextureHandle> {
        if source == AvatarSource::Initials || email.trim().is_empty() {
            return None;
        }
        let key = email.trim().to_lowercase();
        match self.slots.get(&key) {
            Some(Slot::Ready(t)) => return Some(t.clone()),
            Some(_) => return None,
            None => {}
        }
        self.slots.insert(key.clone(), Slot::Loading);
        self.loading += 1;
        let tx = self.tx.clone();
        let hash = email_hash(&key);
        std::thread::spawn(move || {
            let _ = tx.send((key, fetch(&hash)));
        });
        None
    }
}

/// Warna stabil per nama (untuk inisial).
pub fn name_color(name: &str) -> egui::Color32 {
    let h = name
        .bytes()
        .fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
    let hue = (h % 360) as f32 / 360.0;
    egui::ecolor::Hsva::new(hue, 0.45, 0.62, 1.0).into()
}

/// Inisial nama: huruf pertama dua kata pertama.
pub fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|w| w.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

/// Gambar avatar bulat di `rect`: gambar Gravatar atau inisial.
pub fn paint(
    ui: &mut egui::Ui,
    cache: &mut AvatarCache,
    source: AvatarSource,
    rect: egui::Rect,
    name: &str,
    email: &str,
) {
    let painter = ui.painter();
    if let Some(tex) = cache.get(source, email) {
        egui::Image::new(&tex)
            .corner_radius(rect.width() / 2.0)
            .paint_at(ui, rect);
        return;
    }
    painter.circle_filled(rect.center(), rect.width() / 2.0, name_color(name));
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initials(name),
        egui::FontId::proportional(rect.height() * 0.42),
        egui::Color32::WHITE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_and_hash() {
        assert_eq!(initials("yulius jayuda"), "YJ");
        assert_eq!(initials("Antigravity Agent Bot"), "AA");
        assert_eq!(initials(""), "");
        assert_eq!(
            email_hash(" MyEmailAddress@example.com "),
            "0bc83cb571cd1c50ba6f3e8a78ef1346"
        );
    }
}
