//! Daftar proses AI yang berjalan di background, ditampilkan di panel bawah
//! sidebar (`background_dock`).
//!
//! Registry ini hanya *cermin*: tiap jendela progress tetap memegang state
//! job-nya sendiri dan menyalin ringkasannya ke sini sekali per frame lewat
//! [`BackgroundTasks::mirror`]. Panel menulis permintaan (tampilkan, batal,
//! buang) pada entri; pemilik jendela membacanya dari [`MirrorOutcome`].

use std::time::{Duration, Instant};

use crate::agent::harness::{ProgressStatus, ProgressStep};

pub type TaskId = u64;

/// Entri selesai yang disimpan paling banyak; yang lebih lama dibuang.
const MAX_FINISHED: usize = 20;

/// Bagian aplikasi pemilik task; dipakai untuk membuang task yang jendelanya
/// sudah hilang.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskOwner {
    Diagram,
    HttpRepo,
    AiFix,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug)]
pub struct BackgroundTask {
    pub id: TaskId,
    pub owner: TaskOwner,
    pub title: String,
    pub subtitle: String,
    pub status: TaskStatus,
    /// Ringkasan hasil atau pesan gagal setelah selesai.
    pub message: Option<String>,
    pub steps: Vec<ProgressStep>,
    pub started_at: Instant,
    pub last_activity_at: Option<Instant>,
    /// Durasi total setelah selesai.
    pub elapsed: Option<Duration>,
    /// `true` = jendela progress-nya sedang disembunyikan.
    pub detached: bool,
    /// Batal sudah diminta dan job belum melapor berhenti.
    pub cancelling: bool,
    show_requested: bool,
    cancel_requested: bool,
}

impl BackgroundTask {
    pub fn is_running(&self) -> bool {
        self.status == TaskStatus::Running
    }

    /// Satu baris status untuk panel: durasi dan langkah terakhir selama
    /// berjalan, pesan hasil setelah selesai.
    pub fn status_line(&self, now: Instant) -> String {
        match self.status {
            TaskStatus::Running => {
                let mut s = format_duration(now.saturating_duration_since(self.started_at));
                if self.cancelling {
                    s.push_str(" · Cancelling…");
                } else if let Some(step) = self
                    .steps
                    .iter()
                    .rfind(|st| st.status == ProgressStatus::Active)
                    .or(self.steps.last())
                {
                    s.push_str(" · ");
                    s.push_str(&step.description);
                } else if !self.subtitle.is_empty() {
                    s.push_str(" · ");
                    s.push_str(&self.subtitle);
                }
                s
            }
            TaskStatus::Done => self
                .message
                .clone()
                .unwrap_or_else(|| "Finished".to_string()),
            TaskStatus::Failed => self.message.clone().unwrap_or_else(|| "Failed".to_string()),
        }
    }
}

/// Durasi ringkas: `13s` atau `2m 05s`.
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

/// Keadaan sebuah jendela progress pada frame ini.
pub struct Snapshot<'a> {
    pub owner: TaskOwner,
    pub title: &'a str,
    pub subtitle: &'a str,
    pub steps: &'a [ProgressStep],
    pub started_at: Option<Instant>,
    pub last_activity_at: Option<Instant>,
    /// Jendela sedang disembunyikan user.
    pub hidden: bool,
    /// `None` = masih berjalan; `Some` = selesai (ringkasan atau pesan gagal).
    pub result: Option<Result<String, String>>,
}

/// Permintaan dari panel untuk pemilik jendela.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MirrorOutcome {
    /// Tampilkan lagi jendelanya.
    pub show: bool,
    /// Batalkan job-nya.
    pub cancel: bool,
    /// Entri selesai dibuang user; jendela tersembunyi boleh dibuang juga.
    pub dismissed: bool,
    /// Job baru saja selesai saat jendelanya tersembunyi (saatnya toast).
    pub finished_hidden: bool,
}

#[derive(Debug, Default)]
pub struct BackgroundTasks {
    tasks: Vec<BackgroundTask>,
    next_id: TaskId,
    /// Ada jendela yang baru disembunyikan: panel perlu dibuka.
    expand_requested: bool,
}

impl BackgroundTasks {
    pub fn tasks(&self) -> &[BackgroundTask] {
        &self.tasks
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub fn running_count(&self) -> usize {
        self.tasks.iter().filter(|t| t.is_running()).count()
    }

    pub fn contains(&self, id: TaskId) -> bool {
        self.tasks.iter().any(|t| t.id == id)
    }

    /// `true` sekali setelah sebuah jendela disembunyikan.
    pub fn take_expand_request(&mut self) -> bool {
        std::mem::take(&mut self.expand_requested)
    }

    fn start(&mut self, owner: TaskOwner, started_at: Option<Instant>) -> TaskId {
        self.next_id += 1;
        let id = self.next_id;
        self.tasks.push(BackgroundTask {
            id,
            owner,
            title: String::new(),
            subtitle: String::new(),
            status: TaskStatus::Running,
            message: None,
            steps: Vec::new(),
            started_at: started_at.unwrap_or_else(Instant::now),
            last_activity_at: None,
            elapsed: None,
            detached: false,
            cancelling: false,
            show_requested: false,
            cancel_requested: false,
        });
        id
    }

    /// Salin keadaan jendela ke entri `slot` (dibuat bila belum ada) dan
    /// kembalikan permintaan panel. Dipanggil sekali per frame per jendela.
    pub fn mirror(&mut self, slot: &mut Option<TaskId>, snap: Snapshot<'_>) -> MirrorOutcome {
        let mut out = MirrorOutcome::default();
        let running = snap.result.is_none();
        let id = match *slot {
            Some(id) => id,
            None if running => {
                let id = self.start(snap.owner, snap.started_at);
                *slot = Some(id);
                id
            }
            None => {
                // Selesai tanpa entri: jendela tersembunyi tidak bisa dibuka lagi.
                out.dismissed = snap.hidden;
                return out;
            }
        };
        let Some(idx) = self.tasks.iter().position(|t| t.id == id) else {
            // Entri dibuang dari panel.
            *slot = None;
            out.dismissed = snap.hidden && !running;
            return out;
        };
        let task = &mut self.tasks[idx];
        out.show = std::mem::take(&mut task.show_requested);
        out.cancel = std::mem::take(&mut task.cancel_requested);
        let hidden = snap.hidden && !out.show;
        if hidden && !task.detached {
            self.expand_requested = true;
        }
        task.detached = hidden;
        if task.title != snap.title {
            task.title = snap.title.to_string();
        }
        if task.subtitle != snap.subtitle {
            task.subtitle = snap.subtitle.to_string();
        }
        if task.steps != snap.steps {
            task.steps = snap.steps.to_vec();
        }
        task.last_activity_at = snap.last_activity_at;
        match snap.result {
            None => {}
            Some(_) if !hidden => {
                // Hasil sudah terlihat di jendelanya sendiri.
                self.tasks.remove(idx);
                *slot = None;
            }
            Some(result) if task.is_running() => {
                out.finished_hidden = true;
                task.elapsed = Some(task.started_at.elapsed());
                task.cancelling = false;
                match result {
                    Ok(msg) => {
                        task.status = TaskStatus::Done;
                        task.message = Some(msg);
                    }
                    Err(msg) => {
                        task.status = TaskStatus::Failed;
                        task.message = Some(msg);
                    }
                }
                self.prune();
            }
            // Sudah dilaporkan pada frame sebelumnya.
            Some(_) => {}
        }
        out
    }

    /// Buang task `owner` yang jendelanya sudah tidak ada (`seen` = yang masih ada).
    pub fn retain_owner(&mut self, owner: TaskOwner, seen: &[TaskId]) {
        self.tasks
            .retain(|t| t.owner != owner || seen.contains(&t.id));
    }

    pub fn request_show(&mut self, id: TaskId) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id) {
            t.show_requested = true;
        }
    }

    pub fn request_cancel(&mut self, id: TaskId) {
        if let Some(t) = self.tasks.iter_mut().find(|t| t.id == id && t.is_running()) {
            t.cancel_requested = true;
            t.cancelling = true;
        }
    }

    /// Buang entri yang sudah selesai; entri berjalan tidak bisa dibuang.
    pub fn dismiss(&mut self, id: TaskId) {
        self.tasks.retain(|t| t.id != id || t.is_running());
    }

    /// Buang semua entri yang sudah selesai.
    pub fn dismiss_finished(&mut self) {
        self.tasks.retain(|t| t.is_running());
    }

    fn prune(&mut self) {
        let mut finished = self.tasks.iter().filter(|t| !t.is_running()).count();
        self.tasks.retain(|t| {
            if finished > MAX_FINISHED && !t.is_running() {
                finished -= 1;
                return false;
            }
            true
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(desc: &str, status: ProgressStatus) -> ProgressStep {
        ProgressStep {
            step_index: None,
            description: desc.to_string(),
            detail: None,
            status,
            tool_name: None,
        }
    }

    fn snap<'a>(
        steps: &'a [ProgressStep],
        hidden: bool,
        result: Option<Result<String, String>>,
    ) -> Snapshot<'a> {
        Snapshot {
            owner: TaskOwner::Diagram,
            title: "Business process: Orders",
            subtitle: "2 endpoint(s)",
            steps,
            started_at: None,
            last_activity_at: None,
            hidden,
            result,
        }
    }

    #[test]
    fn mirror_creates_and_updates_a_running_task() {
        let mut tasks = BackgroundTasks::default();
        let mut slot = None;
        let out = tasks.mirror(&mut slot, snap(&[], false, None));
        assert_eq!(out, MirrorOutcome::default());
        assert_eq!(tasks.running_count(), 1);
        assert!(!tasks.take_expand_request());

        let steps = [step("Opening local folder", ProgressStatus::Active)];
        tasks.mirror(&mut slot, snap(&steps, true, None));
        let t = &tasks.tasks()[0];
        assert_eq!(t.title, "Business process: Orders");
        assert!(t.detached);
        assert!(
            t.status_line(Instant::now())
                .ends_with("Opening local folder")
        );
        // Menyembunyikan jendela membuka panel, sekali saja.
        assert!(tasks.take_expand_request());
        tasks.mirror(&mut slot, snap(&steps, true, None));
        assert!(!tasks.take_expand_request());
    }

    #[test]
    fn finishing_while_visible_removes_the_task() {
        let mut tasks = BackgroundTasks::default();
        let mut slot = None;
        tasks.mirror(&mut slot, snap(&[], false, None));
        let out = tasks.mirror(&mut slot, snap(&[], false, Some(Ok("done".into()))));
        assert!(!out.finished_hidden);
        assert!(tasks.is_empty());
        assert_eq!(slot, None);
        // Frame berikutnya tidak membuat entri baru.
        tasks.mirror(&mut slot, snap(&[], false, Some(Ok("done".into()))));
        assert!(tasks.is_empty());
    }

    #[test]
    fn finishing_while_hidden_keeps_the_result_until_shown() {
        let mut tasks = BackgroundTasks::default();
        let mut slot = None;
        tasks.mirror(&mut slot, snap(&[], true, None));
        let out = tasks.mirror(&mut slot, snap(&[], true, Some(Err("boom".into()))));
        assert!(out.finished_hidden);
        assert_eq!(tasks.tasks()[0].status, TaskStatus::Failed);
        assert_eq!(tasks.tasks()[0].status_line(Instant::now()), "boom");
        // Dilaporkan sekali saja.
        let out = tasks.mirror(&mut slot, snap(&[], true, Some(Err("boom".into()))));
        assert!(!out.finished_hidden);

        tasks.request_show(slot.expect("task id"));
        let out = tasks.mirror(&mut slot, snap(&[], true, Some(Err("boom".into()))));
        assert!(out.show);
        assert!(tasks.is_empty());
    }

    #[test]
    fn cancel_request_is_delivered_once() {
        let mut tasks = BackgroundTasks::default();
        let mut slot = None;
        tasks.mirror(&mut slot, snap(&[], true, None));
        let id = slot.expect("task id");
        tasks.request_cancel(id);
        assert!(tasks.tasks()[0].cancelling);
        assert!(tasks.mirror(&mut slot, snap(&[], true, None)).cancel);
        assert!(!tasks.mirror(&mut slot, snap(&[], true, None)).cancel);
        assert!(
            tasks.tasks()[0]
                .status_line(Instant::now())
                .ends_with("Cancelling…")
        );
    }

    #[test]
    fn dismiss_only_removes_finished_tasks() {
        let mut tasks = BackgroundTasks::default();
        let mut running = None;
        let mut finished = None;
        tasks.mirror(&mut running, snap(&[], true, None));
        tasks.mirror(&mut finished, snap(&[], true, None));
        tasks.mirror(&mut finished, snap(&[], true, Some(Ok("ok".into()))));

        tasks.dismiss(running.expect("task id"));
        assert_eq!(tasks.tasks().len(), 2);
        tasks.dismiss(finished.expect("task id"));
        assert_eq!(tasks.tasks().len(), 1);
        // Pemilik jendela tersembunyi diberi tahu supaya membuang jendelanya.
        let out = tasks.mirror(&mut finished, snap(&[], true, Some(Ok("ok".into()))));
        assert!(out.dismissed);
        assert_eq!(finished, None);
    }

    #[test]
    fn retain_owner_drops_orphans_of_that_owner_only() {
        let mut tasks = BackgroundTasks::default();
        let mut a = None;
        let mut b = None;
        tasks.mirror(&mut a, snap(&[], false, None));
        tasks.mirror(
            &mut b,
            Snapshot {
                owner: TaskOwner::AiFix,
                ..snap(&[], false, None)
            },
        );
        tasks.retain_owner(TaskOwner::Diagram, &[]);
        assert_eq!(tasks.tasks().len(), 1);
        assert_eq!(tasks.tasks()[0].owner, TaskOwner::AiFix);
    }

    #[test]
    fn prune_keeps_running_tasks() {
        let mut tasks = BackgroundTasks::default();
        let mut running = None;
        tasks.mirror(&mut running, snap(&[], true, None));
        for _ in 0..MAX_FINISHED + 5 {
            let mut slot = None;
            tasks.mirror(&mut slot, snap(&[], true, None));
            tasks.mirror(&mut slot, snap(&[], true, Some(Ok("ok".into()))));
        }
        assert_eq!(tasks.running_count(), 1);
        assert_eq!(tasks.tasks().len(), MAX_FINISHED + 1);
    }

    #[test]
    fn duration_format() {
        assert_eq!(format_duration(Duration::from_secs(13)), "13s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 05s");
    }
}
