//! Timeline pemutaran animasi flow card (double-click card).
//!
//! Murni: tanpa egui dan tanpa `Tabular`. Timeline langsung dimulai dari
//! langkah pertama: satu item per langkah (atau satu langkah semu per tabel untuk
//! card yang belum punya langkah), lalu respons. Gambar ada di
//! `crate::diagram_flow_play_view`.

use crate::models::structs::{FlowCard, FlowPlayback};

/// Fase pemutaran pada satu posisi waktu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlayPhase {
    /// Item ke-`index` sedang berjalan. `t` 0..1.
    Step {
        index: usize,
        t: f32,
    },
    /// Respons keluar dari card. `t` 0..1.
    Response(f32),
    Done,
}

pub const STEP_SECS: f64 = 1.2;
pub const STEP_NO_TARGET_SECS: f64 = 0.5;
pub const RESPONSE_SECS: f64 = 0.6;
/// Lama popup PLAY di awal timeline (detik, kecepatan 1×).
pub const PLAY_POPUP_SECS: f64 = 0.8;
/// Pilihan kecepatan di bilah kontrol.
pub const SPEEDS: [f32; 3] = [0.5, 1.0, 2.0];
/// Kecepatan awal setiap pemutaran baru.
pub const DEFAULT_SPEED: f32 = 0.5;

/// Satu item timeline: langkah card, atau langkah semu per tabel.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayItem {
    /// Indeks di `FlowCard::steps`; `None` untuk langkah semu.
    pub step: Option<usize>,
    /// Tabel target (id node).
    pub table: Option<String>,
}

impl PlayItem {
    fn secs(&self) -> f64 {
        if self.table.is_some() {
            STEP_SECS
        } else {
            STEP_NO_TARGET_SECS
        }
    }
}

/// Item timeline card: langkahnya, atau satu langkah semu per tabel
/// (`tables` = hasil `diagram_flow::tables_of`) bila card belum punya langkah.
pub fn play_items<S: AsRef<str>>(card: &FlowCard, tables: &[S]) -> Vec<PlayItem> {
    if card.steps.is_empty() {
        return tables
            .iter()
            .map(|t| PlayItem {
                step: None,
                table: Some(t.as_ref().to_string()),
            })
            .collect();
    }
    card.steps
        .iter()
        .enumerate()
        .map(|(i, s)| PlayItem {
            step: Some(i),
            table: s
                .target
                .as_ref()
                .and_then(|t| t.table())
                .map(str::to_string),
        })
        .collect()
}

/// Durasi total timeline (detik, kecepatan 1×).
pub fn play_duration(items: &[PlayItem]) -> f64 {
    items.iter().map(PlayItem::secs).sum::<f64>() + RESPONSE_SECS
}

/// Waktu mulai item ke-`index`; di luar batas = awal fase respons.
pub fn step_start(items: &[PlayItem], index: usize) -> f64 {
    items
        .iter()
        .take(index.min(items.len()))
        .map(PlayItem::secs)
        .sum::<f64>()
}

/// Fase pada posisi waktu `position` (detik).
pub fn play_phase(items: &[PlayItem], position: f64) -> PlayPhase {
    let position = position.max(0.0);
    let mut start = 0.0;
    for (index, item) in items.iter().enumerate() {
        let d = item.secs();
        if position < start + d {
            return PlayPhase::Step {
                index,
                t: ((position - start) / d) as f32,
            };
        }
        start += d;
    }
    if position < start + RESPONSE_SECS {
        return PlayPhase::Response(((position - start) / RESPONSE_SECS) as f32);
    }
    PlayPhase::Done
}

/// Popup penanda di atas card selama pemutaran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayPopup {
    /// Awal pemutaran (hijau).
    Play,
    /// Pemutaran selesai (merah).
    End,
}

/// Popup pada posisi waktu `position` beserta progres hidupnya (0..1).
/// PLAY tampil selama `PLAY_POPUP_SECS` pertama; END mulai muncul di paruh
/// akhir fase respons (progres naik ke 1) lalu menetap di `Done`.
pub fn play_popup(items: &[PlayItem], position: f64) -> Option<(PlayPopup, f32)> {
    match play_phase(items, position) {
        PlayPhase::Done => Some((PlayPopup::End, 1.0)),
        PlayPhase::Response(t) if t >= 0.5 => Some((PlayPopup::End, (t - 0.5) * 2.0)),
        _ if position < PLAY_POPUP_SECS => Some((
            PlayPopup::Play,
            (position.max(0.0) / PLAY_POPUP_SECS) as f32,
        )),
        _ => None,
    }
}

/// Item yang sedang berjalan pada `position`, atau item terakhir bila sudah
/// lewat semua. `None` bila tidak ada item.
pub fn current_item(items: &[PlayItem], position: f64) -> Option<usize> {
    match play_phase(items, position) {
        PlayPhase::Step { index, .. } => Some(index),
        PlayPhase::Response(_) | PlayPhase::Done => items.len().checked_sub(1),
    }
}

/// Item tujuan tombol Previous. Di tengah item yang sudah berjalan lebih
/// dari 0,3 detik, kembali ke awal item itu sendiri.
pub fn previous_item(items: &[PlayItem], position: f64) -> usize {
    match current_item(items, position) {
        None => 0,
        Some(i) if position - step_start(items, i) > 0.3 && position < play_duration(items) => i,
        Some(i) => i.saturating_sub(1),
    }
}

/// Item tujuan tombol Next; `items.len()` = fase respons.
pub fn next_item(items: &[PlayItem], position: f64) -> usize {
    match play_phase(items, position) {
        PlayPhase::Step { index, .. } => index + 1,
        PlayPhase::Response(_) | PlayPhase::Done => items.len(),
    }
}

/// Pemutaran baru dari awal.
pub fn new_playback(card_id: &str) -> FlowPlayback {
    FlowPlayback {
        card_id: card_id.to_string(),
        position: 0.0,
        last_tick: None,
        speed: DEFAULT_SPEED,
        playing: true,
    }
}

/// Majukan pemutaran ke waktu `now`. Frame pertama setelah play hanya
/// mencatat `last_tick`. Sampai `duration`, pemutaran berhenti sendiri
/// (`playing = false`). Mengembalikan `true` bila masih berjalan.
pub fn advance(play: &mut FlowPlayback, now: f64, duration: f64) -> bool {
    if !play.playing {
        play.last_tick = None;
        return false;
    }
    if let Some(last) = play.last_tick {
        play.position += (now - last).max(0.0) * play.speed as f64;
    }
    if play.position >= duration {
        play.position = duration;
        play.playing = false;
        play.last_tick = None;
        return false;
    }
    play.last_tick = Some(now);
    true
}

/// Jeda / lanjutkan. Di `Done`, play mengulang dari awal.
pub fn toggle_play(play: &mut FlowPlayback, duration: f64) {
    if play.playing {
        play.playing = false;
    } else {
        if play.position >= duration {
            play.position = 0.0;
        }
        play.playing = true;
    }
    play.last_tick = None;
}

/// Pindahkan posisi ke awal item `index` (atau awal respons bila di luar
/// batas); `last_tick` di-reset supaya waktu tidak melompat.
pub fn seek_item(play: &mut FlowPlayback, items: &[PlayItem], index: usize) {
    play.position = step_start(items, index);
    play.last_tick = None;
}

/// Kecepatan berikutnya di `SPEEDS` (berputar).
pub fn next_speed(speed: f32) -> f32 {
    let i = SPEEDS
        .iter()
        .position(|s| (s - speed).abs() < 1e-3)
        .map_or(1, |i| (i + 1) % SPEEDS.len());
    SPEEDS[i]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{FlowOp, FlowStep, FlowStepKind, FlowTarget};

    fn card(steps: Vec<FlowStep>) -> FlowCard {
        FlowCard {
            id: "flw_1".into(),
            steps,
            ..Default::default()
        }
    }

    fn db(table: &str) -> FlowStep {
        FlowStep {
            kind: FlowStepKind::Db,
            title: format!("read {table}"),
            target: Some(FlowTarget::Table(table.into())),
            op: Some(FlowOp::Read),
            ..Default::default()
        }
    }

    fn logic() -> FlowStep {
        FlowStep {
            title: "compute".into(),
            ..Default::default()
        }
    }

    #[test]
    fn phase_boundaries_and_step_start() {
        let items = play_items(&card(vec![db("users"), logic()]), &["users"]);
        assert_eq!(items.len(), 2);
        let total = STEP_SECS + STEP_NO_TARGET_SECS + RESPONSE_SECS;
        assert!((play_duration(&items) - total).abs() < 1e-9);
        // Tanpa fase request: posisi 0 langsung di langkah pertama.
        assert_eq!(
            play_phase(&items, 0.0),
            PlayPhase::Step { index: 0, t: 0.0 }
        );
        assert_eq!(step_start(&items, 0), 0.0);
        assert_eq!(step_start(&items, 1), STEP_SECS);
        assert_eq!(step_start(&items, 99), STEP_SECS + STEP_NO_TARGET_SECS);
        assert!(matches!(
            play_phase(&items, STEP_SECS + 0.1),
            PlayPhase::Step { index: 1, .. }
        ));
        assert!(matches!(
            play_phase(&items, total - 0.1),
            PlayPhase::Response(_)
        ));
        assert_eq!(play_phase(&items, total), PlayPhase::Done);
        assert_eq!(current_item(&items, total), Some(1));
        assert_eq!(current_item(&items, 0.1), Some(0));
        assert_eq!(current_item(&[], 0.0), None);
    }

    #[test]
    fn card_without_steps_plays_one_pseudo_step_per_table() {
        let c = card(vec![]);
        let items = play_items(&c, &["users", "orders"]);
        assert_eq!(
            items,
            vec![
                PlayItem {
                    step: None,
                    table: Some("users".into())
                },
                PlayItem {
                    step: None,
                    table: Some("orders".into())
                },
            ]
        );
        let none: [&str; 0] = [];
        assert!(play_items(&c, &none).is_empty());
        assert_eq!(play_duration(&[]), RESPONSE_SECS);
        assert!(matches!(play_phase(&[], 0.1), PlayPhase::Response(_)));
    }

    #[test]
    fn playback_stops_by_itself_and_pause_does_not_advance() {
        let items = play_items(&card(vec![db("users")]), &["users"]);
        let total = play_duration(&items);
        let mut p = new_playback("flw_1");
        assert_eq!(p.speed, DEFAULT_SPEED);
        // Frame pertama hanya mencatat waktu.
        assert!(advance(&mut p, 10.0, total));
        assert_eq!(p.position, 0.0);
        assert!(advance(&mut p, 11.0, total));
        assert!((p.position - 0.5).abs() < 1e-9);
        // Jeda: waktu tidak maju walau frame berikutnya jauh kemudian.
        toggle_play(&mut p, total);
        assert!(!advance(&mut p, 20.0, total));
        assert!((p.position - 0.5).abs() < 1e-9);
        // Lanjut dengan kecepatan 2x sampai habis, lalu berhenti sendiri.
        toggle_play(&mut p, total);
        p.speed = 2.0;
        assert!(advance(&mut p, 30.0, total));
        assert!(!advance(&mut p, 40.0, total));
        assert_eq!(p.position, total);
        assert!(!p.playing);
        assert_eq!(play_phase(&items, p.position), PlayPhase::Done);
        assert!(!advance(&mut p, 50.0, total));
        // Play di Done mengulang dari awal.
        toggle_play(&mut p, total);
        assert_eq!(p.position, 0.0);
        assert!(p.playing);
    }

    #[test]
    fn seek_previous_and_next() {
        let items = play_items(&card(vec![db("users"), logic(), db("orders")]), &["users"]);
        let mut p = new_playback("flw_1");
        p.last_tick = Some(3.0);
        seek_item(&mut p, &items, 2);
        assert_eq!(p.position, step_start(&items, 2));
        assert_eq!(p.last_tick, None);
        assert!(matches!(
            play_phase(&items, p.position),
            PlayPhase::Step { index: 2, .. }
        ));
        assert_eq!(next_item(&items, p.position), 3);
        // Tepat di awal item 2: Previous ke item 1.
        assert_eq!(previous_item(&items, p.position), 1);
        // Di tengah item 2: Previous kembali ke awal item 2.
        assert_eq!(previous_item(&items, p.position + 0.6), 2);
        assert_eq!(previous_item(&items, 0.1), 0);
        assert_eq!(next_item(&items, 0.1), 1);
        assert_eq!(next_item(&items, play_duration(&items)), 3);
    }

    #[test]
    fn popup_play_at_start_and_end_when_done() {
        let items = play_items(&card(vec![db("users"), db("orders")]), &["users"]);
        let total = play_duration(&items);
        assert_eq!(play_popup(&items, 0.0), Some((PlayPopup::Play, 0.0)));
        assert!(matches!(
            play_popup(&items, PLAY_POPUP_SECS / 2.0),
            Some((PlayPopup::Play, t)) if (t - 0.5).abs() < 1e-6
        ));
        // Di tengah timeline tidak ada popup.
        assert_eq!(play_popup(&items, PLAY_POPUP_SECS), None);
        assert_eq!(play_popup(&items, total - RESPONSE_SECS * 0.75), None);
        // END muncul di paruh akhir respons dan menetap setelah selesai.
        assert!(matches!(
            play_popup(&items, total - RESPONSE_SECS * 0.25),
            Some((PlayPopup::End, t)) if (t - 0.5).abs() < 1e-4
        ));
        assert_eq!(play_popup(&items, total), Some((PlayPopup::End, 1.0)));
        // Card tanpa item: END tetap menang atas PLAY di akhir respons.
        assert_eq!(play_popup(&[], 0.1).map(|p| p.0), Some(PlayPopup::Play));
        assert_eq!(
            play_popup(&[], RESPONSE_SECS * 0.9).map(|p| p.0),
            Some(PlayPopup::End)
        );
    }

    #[test]
    fn speed_cycles() {
        assert_eq!(next_speed(1.0), 2.0);
        assert_eq!(next_speed(2.0), 0.5);
        assert_eq!(next_speed(0.5), 1.0);
        assert_eq!(next_speed(3.0), 1.0);
    }
}
