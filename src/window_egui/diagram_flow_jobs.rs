//! Job generate alur bisnis flow card (AI) untuk diagram yang terbuka.
//!
//! Card dikelompokkan per repository; tiap repository dikerjakan satu
//! [`crate::diagram_flow_gen::spawn_flow_scan`] secara berurutan. Satu job per
//! diagram: permintaan baru untuk diagram yang sama membatalkan yang lama.
//! Kemajuan tampil di jendela [`FlowGenWindow`] (`DiagramState::flow_gen`).

use std::collections::VecDeque;

use eframe::egui;

use crate::diagram_flow_gen::{FlowScanHandle, FlowScanInput, FlowScanOutcome, FlowSeed};
use crate::http_collection::HttpWorkspace;
use crate::models::structs::{DiagramState, FlowGenWindow, FlowTriggerKind};
use crate::repo_scan::RepoJobEvent;

/// Satu repository beserta card yang alurnya di-generate darinya.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RepoRun {
    /// Judul group atau folder HTTP API pemilik repository.
    pub scope: String,
    pub repo_path: Option<String>,
    pub repo_url: Option<String>,
    pub card_ids: Vec<String>,
}

impl RepoRun {
    /// Path lokal atau URL tanpa kredensial, untuk ditampilkan.
    fn label(&self) -> String {
        match crate::repo_scan::choose_source(self.repo_path.as_deref(), self.repo_url.as_deref()) {
            Ok(crate::repo_scan::RepoSource::Local(p)) => p.display().to_string(),
            Ok(crate::repo_scan::RepoSource::Remote(u)) => crate::repo_scan::redact(&u),
            Err(_) => self.scope.clone(),
        }
    }
}

/// Rencana job: repository yang dikerjakan dan jumlah card yang tidak punya
/// repository.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct FlowGenPlan {
    pub runs: Vec<RepoRun>,
    pub no_repo: usize,
}

/// Bahan yang sama untuk semua repository dalam satu job.
struct JobShared {
    backend: crate::ai_assistant::ChatBackend,
    backend_label: String,
    parallel: usize,
    force: bool,
}

/// Generate alur bisnis untuk card sebuah diagram.
pub struct DiagramFlowGenJob {
    conn_id: Option<i64>,
    db_name: Option<String>,
    handle: FlowScanHandle,
    /// Repository yang menunggu giliran.
    queue: VecDeque<RepoRun>,
    shared: JobShared,
}

/// (judul, path lokal, URL) repository sebuah group atau folder.
type RepoSourceInfo = (String, Option<String>, Option<String>);

/// Repository untuk kunci `key`: group diagram lebih dulu, lalu folder HTTP API.
fn repo_for_key(
    state: &DiagramState,
    workspaces: &[HttpWorkspace],
    key: &str,
) -> Option<RepoSourceInfo> {
    let group = state.groups.iter().find(|g| {
        !crate::diagram_links::is_linked_id(&g.id)
            && g.has_repository()
            && g.repo_key().as_deref() == Some(key)
    });
    if let Some(g) = group {
        return Some((
            g.title.clone(),
            g.local_repo_path(),
            g.shared_repo_url().map(str::to_string),
        ));
    }
    crate::http_collection::all_folders(workspaces)
        .into_iter()
        .map(|(_, f)| f)
        .find(|f| f.has_repository() && f.repo_key().as_deref() == Some(key))
        .map(|f| {
            (
                f.name.clone(),
                f.local_repo_path(),
                f.shared_repo_url().map(str::to_string),
            )
        })
}

/// Tentukan card mana yang di-generate dari repository mana. `group_id` =
/// hanya card milik repository group itu (card lama tanpa kunci ikut);
/// `card_ids` kosong = semua card HTTP yang lolos filter group.
pub(crate) fn plan_flow_generation(
    state: &DiagramState,
    workspaces: &[HttpWorkspace],
    group_id: Option<&str>,
    card_ids: &[String],
) -> FlowGenPlan {
    let group = group_id.and_then(|gid| state.groups.iter().find(|g| g.id == gid));
    let group_key = group.and_then(|g| g.repo_key());
    let mut plan = FlowGenPlan::default();
    for card in &state.flow_cards {
        if card.trigger.kind != FlowTriggerKind::Http {
            continue;
        }
        if !card_ids.is_empty() && !card_ids.contains(&card.id) {
            continue;
        }
        if group.is_some() && card.repo_key.is_some() && card.repo_key != group_key {
            continue;
        }
        let Some(key) = card.repo_key.clone().or_else(|| group_key.clone()) else {
            plan.no_repo += 1;
            continue;
        };
        let resolved = match group {
            Some(g) if group_key.as_deref() == Some(key.as_str()) => Some((
                g.title.clone(),
                g.local_repo_path(),
                g.shared_repo_url().map(str::to_string),
            )),
            _ => repo_for_key(state, workspaces, &key),
        };
        let Some((scope, repo_path, repo_url)) = resolved else {
            plan.no_repo += 1;
            continue;
        };
        match plan
            .runs
            .iter_mut()
            .find(|r| r.repo_path == repo_path && r.repo_url == repo_url)
        {
            Some(run) => run.card_ids.push(card.id.clone()),
            None => plan.runs.push(RepoRun {
                scope,
                repo_path,
                repo_url,
                card_ids: vec![card.id.clone()],
            }),
        }
    }
    plan
}

/// Bahan AI untuk card-card satu repository.
fn seeds_for(state: &DiagramState, card_ids: &[String]) -> Vec<FlowSeed> {
    state
        .flow_cards
        .iter()
        .filter(|c| card_ids.contains(&c.id))
        .map(|c| FlowSeed {
            card_id: c.id.clone(),
            method: c.trigger.method.clone(),
            path: c.trigger.target.clone(),
            source: c.source.clone(),
            known_tables: crate::diagram_flow::tables_of(state, c),
            previous: c.meta.clone(),
        })
        .collect()
}

/// Id node tabel diagram, tanpa tabel database yang di-link.
fn diagram_tables(state: &DiagramState) -> Vec<String> {
    state
        .nodes
        .iter()
        .filter(|n| !crate::diagram_links::is_linked_id(&n.id))
        .map(|n| n.id.clone())
        .collect()
}

/// Mulai job untuk satu repository dan siapkan jendelanya.
fn spawn_run(state: &mut DiagramState, run: RepoRun, shared: &JobShared) -> FlowScanHandle {
    if let Some(win) = state.flow_gen.as_mut() {
        win.scope = run.scope.clone();
        win.repo_label = run.label();
        win.progress.clear();
        win.last_activity_at = Some(std::time::Instant::now());
    }
    log::info!(
        "[DIAGRAM_FLOW] generating business process for {} card(s) from '{}' (AI: {})",
        run.card_ids.len(),
        run.scope,
        shared.backend_label
    );
    crate::diagram_flow_gen::spawn_flow_scan(FlowScanInput {
        cards: seeds_for(state, &run.card_ids),
        tables: diagram_tables(state),
        repo_path: run.repo_path,
        repo_url: run.repo_url,
        scope_name: run.scope,
        backend: Some(shared.backend.clone()),
        backend_label: shared.backend_label.clone(),
        cache_root: crate::repo_scan::default_cache_root(),
        parallel: shared.parallel,
        force: shared.force,
    })
}

/// Label `METHOD path` sebuah card, atau id-nya bila card sudah hilang.
fn card_label(state: &DiagramState, card_id: &str) -> String {
    state
        .flow_cards
        .iter()
        .find(|c| c.id == card_id)
        .map(|c| format!("{} {}", c.trigger.method, c.trigger.target))
        .unwrap_or_else(|| card_id.to_string())
}

/// Terapkan hasil satu repository ke card dan jendela progress.
pub(crate) fn apply_outcome(state: &mut DiagramState, outcome: FlowScanOutcome) {
    let mut generated = 0;
    for flow in outcome.flows {
        let Some(card) = state.flow_cards.iter_mut().find(|c| c.id == flow.card_id) else {
            continue; // card dihapus selama job berjalan
        };
        card.summary = flow.summary;
        card.steps = flow.steps;
        card.meta = Some(flow.meta);
        crate::diagram_flow::links_from_steps(state, &flow.card_id);
        generated += 1;
    }
    let failed: Vec<(String, String)> = outcome
        .failed
        .into_iter()
        .map(|(id, msg)| (card_label(state, &id), msg))
        .collect();
    if generated > 0 {
        state.save_requested = true;
    }
    if let Some(win) = state.flow_gen.as_mut() {
        win.generated += generated;
        win.skipped_fresh += outcome.skipped_fresh;
        win.failed.extend(failed);
        if let Some(note) = outcome.note {
            win.note = Some(match win.note.take() {
                Some(prev) if prev != note => format!("{prev} {note}"),
                _ => note,
            });
        }
    }
}

/// Ringkasan hasil untuk toast.
pub(crate) fn summary_message(win: &FlowGenWindow) -> String {
    format!(
        "Business process generated for {} endpoint(s); {} unchanged, {} failed",
        win.generated,
        win.skipped_fresh,
        win.failed.len()
    )
}

/// Kelanjutan setelah satu repository selesai.
enum RunEnd {
    Next(RepoRun),
    Done,
}

impl super::Tabular {
    /// Mulai generate alur bisnis untuk card diagram (conn, db).
    pub(crate) fn start_flow_generation(
        &mut self,
        conn_id: Option<i64>,
        db_name: Option<String>,
        group_id: Option<&str>,
        card_ids: &[String],
        force: bool,
    ) {
        // Satu job per diagram; permintaan baru menggantikan yang lama.
        self.diagram_flow_jobs.retain(|job| {
            let same = job.conn_id == conn_id && job.db_name == db_name;
            if same {
                job.handle.cancel();
            }
            !same
        });

        let target = self.effective_chat_target();
        if let Err(e) = crate::ai_assistant::backend_ready_for(self, target) {
            self.toasts.error(format!(
                "Generating a business process needs an AI backend: {e}"
            ));
            return;
        }
        let shared = JobShared {
            backend: crate::ai_assistant::chat_backend_for(self, target),
            backend_label: crate::ai_assistant::backend_label_for(self, target),
            parallel: crate::repo_endpoints::DEFAULT_PARALLEL_BATCHES,
            force,
        };

        let workspaces = self.yaak_workspaces.clone();
        let Some(state) = self.diagram_state_for_mut(conn_id, db_name.as_deref()) else {
            self.toasts.error("Diagram tab is no longer open");
            return;
        };
        if let Some(gid) = group_id {
            let Some(group) = state.groups.iter().find(|g| g.id == gid) else {
                self.toasts.error("Group not found");
                return;
            };
            if !group.has_repository() {
                state.group_repo_editor = Some(crate::models::structs::GroupRepoDraft {
                    group_id: gid.to_string(),
                    ..Default::default()
                });
                return;
            }
        }
        let plan = plan_flow_generation(state, &workspaces, group_id, card_ids);
        let card_count: usize = plan.runs.iter().map(|r| r.card_ids.len()).sum();
        if card_count == 0 {
            let msg = if plan.no_repo > 0 {
                format!(
                    "{} API card(s) have no repository. Set a git repository on the diagram \
                     group or the HTTP API folder first.",
                    plan.no_repo
                )
            } else {
                "No API cards to generate. Generate endpoints from an HTTP API folder first."
                    .to_string()
            };
            self.toasts.info(msg);
            return;
        }
        state.flow_gen = Some(FlowGenWindow {
            repo_index: 1,
            repo_total: plan.runs.len(),
            card_count,
            running: true,
            note: (plan.no_repo > 0)
                .then(|| format!("{} card(s) skipped: no repository.", plan.no_repo)),
            started_at: Some(std::time::Instant::now()),
            ..Default::default()
        });
        let mut queue: VecDeque<RepoRun> = plan.runs.into();
        let Some(first) = queue.pop_front() else {
            return;
        };
        let handle = spawn_run(state, first, &shared);
        self.diagram_flow_jobs.push(DiagramFlowGenJob {
            conn_id,
            db_name,
            handle,
            queue,
            shared,
        });
    }

    /// Terima kemajuan dan hasil generate alur bisnis. Dipanggil tiap frame.
    pub fn poll_diagram_flow_jobs(&mut self, ctx: &egui::Context) {
        if self.diagram_flow_jobs.is_empty() {
            return;
        }
        let jobs = std::mem::take(&mut self.diagram_flow_jobs);
        let mut keep = Vec::with_capacity(jobs.len());
        let mut toasts: Vec<(bool, String)> = Vec::new();
        for mut job in jobs {
            let Some(state) = self.diagram_state_for_mut(job.conn_id, job.db_name.as_deref())
            else {
                job.handle.cancel(); // tab ditutup
                continue;
            };
            match poll_job(state, &mut job) {
                Some(end) => toasts.extend(end),
                None => keep.push(job),
            }
        }
        keep.append(&mut self.diagram_flow_jobs);
        self.diagram_flow_jobs = keep;
        for (ok, msg) in toasts {
            log::info!("[DIAGRAM_FLOW] {msg}");
            if ok {
                self.toasts.success(msg);
            } else {
                self.toasts.error(msg);
            }
        }
        if !self.diagram_flow_jobs.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

/// Hasil job generate alur bisnis yang sudah selesai, untuk panel background.
fn flow_gen_result(win: &FlowGenWindow) -> Result<String, String> {
    match win.error.clone().filter(|_| win.generated == 0) {
        Some(e) => Err(e),
        None => Ok(summary_message(win)),
    }
}

/// Hasil pemindaian saran tabel yang sudah selesai, untuk panel background.
fn suggestions_result(
    sugg: &crate::models::structs::GroupTableSuggestions,
) -> Result<String, String> {
    match &sugg.error {
        Some(e) if sugg.items.is_empty() => Err(e.clone()),
        _ => Ok(format!("{} table suggestion(s) ready", sugg.items.len())),
    }
}

impl super::Tabular {
    /// Cerminkan jendela progress AI semua diagram ke panel Background
    /// Processes dan jalankan permintaan panel (tampilkan, batal, buang).
    /// Dipanggil tiap frame setelah poller job diagram.
    pub fn sync_diagram_background_tasks(&mut self) {
        use super::background_tasks::{Snapshot, TaskOwner};

        let mut tasks = std::mem::take(&mut self.background_tasks);
        let mut seen = Vec::new();
        let mut activate: Option<usize> = None;
        let mut ready: Vec<String> = Vec::new();
        for (idx, tab) in self.query_tabs.iter_mut().enumerate() {
            let Some(state) = tab.diagram_state.as_mut() else {
                continue;
            };
            if let Some(win) = state.flow_gen.as_mut() {
                let title = format!("Business process: {}", win.scope);
                let subtitle = crate::diagram_flow_gen_view::subtitle(win);
                let result = (!win.running).then(|| flow_gen_result(win));
                let out = tasks.mirror(
                    &mut win.task_id,
                    Snapshot {
                        owner: TaskOwner::Diagram,
                        title: &title,
                        subtitle: &subtitle,
                        steps: &win.progress,
                        started_at: win.started_at,
                        last_activity_at: win.last_activity_at,
                        hidden: win.hidden,
                        result,
                    },
                );
                seen.extend(win.task_id);
                if out.cancel {
                    win.cancel_requested = true;
                }
                if out.show {
                    win.hidden = false;
                    activate = Some(idx);
                }
                if out.dismissed {
                    state.flow_gen = None;
                }
            }
            if let Some(sugg) = state.group_table_suggestions.as_mut() {
                let title = format!("Suggested tables for {}", sugg.group_title);
                let result = (!sugg.running).then(|| suggestions_result(sugg));
                let out = tasks.mirror(
                    &mut sugg.task_id,
                    Snapshot {
                        owner: TaskOwner::Diagram,
                        title: &title,
                        subtitle: "",
                        steps: &sugg.progress,
                        started_at: sugg.started_at,
                        last_activity_at: sugg.last_activity_at,
                        hidden: sugg.hidden,
                        result,
                    },
                );
                seen.extend(sugg.task_id);
                if out.cancel {
                    sugg.cancel_requested = true;
                }
                if out.show {
                    sugg.hidden = false;
                    activate = Some(idx);
                }
                if out.finished_hidden {
                    ready.push(sugg.group_title.clone());
                }
                if out.dismissed {
                    state.group_table_suggestions = None;
                }
            }
        }
        tasks.retain_owner(TaskOwner::Diagram, &seen);
        self.background_tasks = tasks;
        for group in ready {
            self.toasts.info(format!(
                "Table suggestions for '{group}' are ready. Open them from Background Processes."
            ));
        }
        if let Some(idx) = activate
            && idx != self.active_tab_index
        {
            crate::editor::switch_to_tab(self, idx);
        }
    }
}

/// Proses event satu job. `None` = masih berjalan; `Some(toast)` = selesai
/// (toast opsional: (sukses, pesan)).
fn poll_job(
    state: &mut DiagramState,
    job: &mut DiagramFlowGenJob,
) -> Option<Option<(bool, String)>> {
    let Some(win) = state.flow_gen.as_mut() else {
        job.handle.cancel(); // jendela hilang (diagram dimuat ulang)
        return Some(None);
    };
    if win.cancel_requested {
        job.handle.cancel();
        job.queue.clear();
    }
    loop {
        let Some(win) = state.flow_gen.as_mut() else {
            job.handle.cancel();
            return Some(None);
        };
        let result = match job.handle.rx.try_recv() {
            Ok(RepoJobEvent::Progress(step)) => {
                win.last_activity_at = Some(std::time::Instant::now());
                super::diagram::upsert_progress(&mut win.progress, step);
                continue;
            }
            Ok(RepoJobEvent::Activity) => {
                win.last_activity_at = Some(std::time::Instant::now());
                continue;
            }
            Ok(RepoJobEvent::Finished(result)) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("Business process generation stopped unexpectedly".to_string())
            }
        };
        let cancelled = matches!(&result, Err(e) if e == "Cancelled");
        match result {
            Ok(outcome) => {
                for s in &mut win.progress {
                    if s.status == crate::agent::harness::ProgressStatus::Active {
                        s.status = crate::agent::harness::ProgressStatus::Done;
                    }
                }
                apply_outcome(state, outcome);
            }
            Err(e) if !cancelled => {
                log::warn!("[DIAGRAM_FLOW] '{}': {e}", win.scope);
                win.error = Some(if win.repo_total > 1 {
                    format!("{}: {e}", win.scope)
                } else {
                    e
                });
            }
            Err(_) => {}
        }
        let end = match job.queue.pop_front() {
            Some(next) if !cancelled => RunEnd::Next(next),
            _ => RunEnd::Done,
        };
        match end {
            RunEnd::Next(next) => {
                if let Some(win) = state.flow_gen.as_mut() {
                    win.repo_index += 1;
                }
                job.handle = spawn_run(state, next, &job.shared);
            }
            RunEnd::Done => return Some(finish(state, cancelled)),
        }
    }
}

/// Tandai jendela selesai dan tentukan toast-nya.
fn finish(state: &mut DiagramState, cancelled: bool) -> Option<(bool, String)> {
    let win = state.flow_gen.as_mut()?;
    win.running = false;
    win.elapsed = win.started_at.map(|t| t.elapsed());
    let toast = if cancelled {
        None
    } else if let Some(e) = win.error.clone().filter(|_| win.generated == 0) {
        Some((false, format!("Business process generation failed: {e}")))
    } else {
        Some((true, summary_message(win)))
    };
    // Jendela tersembunyi tetap tersembunyi (toast cukup); hasilnya bisa
    // dibuka dari panel Background Processes.
    if cancelled {
        state.flow_gen = None;
    }
    toast
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{
        DiagramGroup, FlowCard, FlowMeta, FlowStep, FlowStepKind, FlowTarget, FlowTrigger,
    };

    fn card(id: &str, key: Option<&str>) -> FlowCard {
        FlowCard {
            id: id.to_string(),
            trigger: FlowTrigger {
                kind: FlowTriggerKind::Http,
                method: "GET".into(),
                target: format!("/{id}"),
            },
            repo_key: key.map(str::to_string),
            ..Default::default()
        }
    }

    fn group(id: &str, url: &str) -> DiagramGroup {
        DiagramGroup {
            id: id.to_string(),
            title: format!("Group {id}"),
            color: egui::Color32::RED,
            manual_pos: None,
            repo_url: Some(url.to_string()),
        }
    }

    fn key(url: &str) -> String {
        crate::repo_scan::repo_key(url).expect("repo key")
    }

    #[test]
    fn plan_groups_cards_by_repository() {
        let a = "https://github.com/acme/orders.git";
        let b = "https://github.com/acme/billing.git";
        let state = DiagramState {
            groups: vec![group("g1", a), group("g2", b)],
            flow_cards: vec![
                card("flw_1", Some(&key(a))),
                card("flw_2", Some(&key(b))),
                card("flw_3", Some(&key(a))),
                card("flw_4", Some("github.com/acme/unknown")),
                card("flw_5", None),
            ],
            ..Default::default()
        };
        let plan = plan_flow_generation(&state, &[], None, &[]);
        assert_eq!(plan.runs.len(), 2);
        assert_eq!(plan.runs[0].scope, "Group g1");
        assert_eq!(plan.runs[0].card_ids, vec!["flw_1", "flw_3"]);
        assert_eq!(plan.runs[1].card_ids, vec!["flw_2"]);
        assert_eq!(plan.no_repo, 2);
    }

    #[test]
    fn plan_for_group_and_single_card() {
        let a = "https://github.com/acme/orders.git";
        let b = "https://github.com/acme/billing.git";
        let state = DiagramState {
            groups: vec![group("g1", a), group("g2", b)],
            flow_cards: vec![
                card("flw_1", Some(&key(a))),
                card("flw_2", Some(&key(b))),
                card("flw_3", None),
            ],
            ..Default::default()
        };
        // Group: hanya card repository-nya; card lama tanpa kunci ikut.
        let plan = plan_flow_generation(&state, &[], Some("g1"), &[]);
        assert_eq!(plan.runs.len(), 1);
        assert_eq!(plan.runs[0].card_ids, vec!["flw_1", "flw_3"]);
        assert_eq!(plan.no_repo, 0);
        // Satu card.
        let plan = plan_flow_generation(&state, &[], None, &["flw_2".to_string()]);
        assert_eq!(plan.runs.len(), 1);
        assert_eq!(plan.runs[0].scope, "Group g2");
        assert_eq!(plan.runs[0].card_ids, vec!["flw_2"]);
    }

    #[test]
    fn outcome_fills_cards_and_counts() {
        let mut state = DiagramState {
            flow_cards: vec![card("flw_1", None), card("flw_2", None)],
            flow_gen: Some(FlowGenWindow::default()),
            ..Default::default()
        };
        let step = FlowStep {
            kind: FlowStepKind::Db,
            title: "Load".into(),
            target: Some(FlowTarget::Table("users".into())),
            ..Default::default()
        };
        apply_outcome(
            &mut state,
            FlowScanOutcome {
                flows: vec![crate::diagram_flow_gen::GeneratedFlow {
                    card_id: "flw_1".into(),
                    summary: "Loads users".into(),
                    steps: vec![step],
                    meta: FlowMeta {
                        generated_at: "2026-09-30T00:00:00Z".into(),
                        backend: "test".into(),
                        ..Default::default()
                    },
                }],
                skipped_fresh: 3,
                failed: vec![("flw_2".into(), "no reply".into())],
                note: None,
            },
        );
        assert_eq!(state.flow_cards[0].summary, "Loads users");
        assert_eq!(state.flow_cards[0].steps.len(), 1);
        assert!(state.flow_cards[0].meta.is_some());
        assert!(state.save_requested);
        let win = state.flow_gen.as_ref().expect("window");
        assert_eq!((win.generated, win.skipped_fresh), (1, 3));
        assert_eq!(
            win.failed,
            vec![("GET /flw_2".to_string(), "no reply".to_string())]
        );
        assert_eq!(
            summary_message(win),
            "Business process generated for 1 endpoint(s); 3 unchanged, 1 failed"
        );
    }

    #[test]
    fn finish_hidden_window_stays_hidden_and_reports() {
        let mut state = DiagramState {
            flow_gen: Some(FlowGenWindow {
                running: true,
                hidden: true,
                generated: 2,
                ..Default::default()
            }),
            ..Default::default()
        };
        let toast = finish(&mut state, false);
        assert_eq!(
            toast,
            Some((
                true,
                "Business process generated for 2 endpoint(s); 0 unchanged, 0 failed".into()
            ))
        );
        // Hasil tetap tersimpan supaya bisa dibuka dari panel background.
        assert!(
            state
                .flow_gen
                .as_ref()
                .is_some_and(|w| w.hidden && !w.running)
        );
        // Dibatalkan: tanpa toast, jendela ditutup.
        state.flow_gen = Some(FlowGenWindow {
            running: true,
            ..Default::default()
        });
        assert_eq!(finish(&mut state, true), None);
        assert!(state.flow_gen.is_none());
    }
}
