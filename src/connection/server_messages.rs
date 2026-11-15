//! Pesan server yang bukan hasil query: `RAISE NOTICE` / `WARNING` PostgreSQL
//! dan `PRINT` / pesan info SQL Server.
//!
//! Driver (sqlx 0.9, mssql-client 0.20) tidak mengekspos pesan ini lewat API;
//! keduanya hanya mengirim event `tracing`. Modul ini memasang subscriber
//! minimal yang hanya tertarik pada dua target itu, lalu menyimpan pesan ke
//! buffer `task_local` milik job yang sedang berjalan. Event terjadi sinkron
//! di dalam poll future query, jadi pesan selalu masuk ke job yang benar.
//! Di luar [`capture`] event dibuang.

use std::cell::RefCell;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::field::{Field, Visit};
use tracing::subscriber::Interest;
use tracing::{Event, Level, Metadata, span};

/// Batas pesan per job agar loop `RAISE NOTICE` tidak menghabiskan memori.
pub const MAX_MESSAGES: usize = 1000;

const PG_TARGET: &str = "sqlx::postgres::notice";
const MSSQL_TARGET_PREFIX: &str = "mssql_client::client::response";

/// Nomor pesan info SQL Server yang hanya derau (ganti database/bahasa).
const MSSQL_NOISE: &[i64] = &[5701, 5703];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerMessage {
    /// `NOTICE`, `WARNING`, `INFO`, `DEBUG`, atau `PRINT`.
    pub severity: String,
    pub text: String,
}

#[derive(Default)]
struct Sink {
    messages: Vec<ServerMessage>,
    dropped: usize,
}

tokio::task_local! {
    static SINK: RefCell<Sink>;
}

/// Jalankan `fut` sambil mengumpulkan pesan server yang dikirim selama itu.
pub async fn capture<F: Future>(fut: F) -> (F::Output, Vec<ServerMessage>) {
    SINK.scope(RefCell::new(Sink::default()), async move {
        let out = fut.await;
        let messages = SINK.with(|s| {
            let mut s = s.borrow_mut();
            let dropped = s.dropped;
            let mut list = std::mem::take(&mut s.messages);
            if dropped > 0 {
                list.push(ServerMessage {
                    severity: "INFO".to_string(),
                    text: format!("{dropped} more messages were not shown"),
                });
            }
            list
        });
        (out, messages)
    })
    .await
}

fn push(message: ServerMessage) {
    let _ = SINK.try_with(|s| {
        let mut s = s.borrow_mut();
        if s.messages.len() < MAX_MESSAGES {
            s.messages.push(message);
        } else {
            s.dropped += 1;
        }
    });
}

fn is_pg(meta: &Metadata<'_>) -> bool {
    meta.target() == PG_TARGET
}

fn is_mssql(meta: &Metadata<'_>) -> bool {
    meta.target().starts_with(MSSQL_TARGET_PREFIX)
}

fn wanted(meta: &Metadata<'_>) -> bool {
    meta.is_event() && (is_pg(meta) || is_mssql(meta))
}

/// Severity PostgreSQL dari level `tracing` yang dipakai sqlx.
fn pg_severity(level: &Level) -> &'static str {
    match *level {
        Level::ERROR => "ERROR",
        Level::WARN => "WARNING",
        Level::INFO => "NOTICE",
        Level::DEBUG => "DEBUG",
        Level::TRACE => "INFO",
    }
}

#[derive(Default)]
struct Fields {
    /// Semua nilai bernama `message`, sesuai urutan.
    messages: Vec<String>,
    number: Option<i64>,
    /// Field lain `nama=nilai`, untuk diteruskan ke log.
    others: Vec<String>,
}

impl Visit for Fields {
    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "number" {
            self.number = Some(value);
        }
        self.others.push(format!("{}={value}", field.name()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "number" {
            self.number = i64::try_from(value).ok();
        }
        self.others.push(format!("{}={value}", field.name()));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.messages.push(value.to_string());
        } else {
            self.others.push(format!("{}={value}", field.name()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.messages.push(format!("{value:?}"));
        } else {
            if field.name() == "number" {
                self.number = format!("{value:?}").parse().ok();
            }
            self.others.push(format!("{}={value:?}", field.name()));
        }
    }
}

/// Ubah satu event menjadi pesan. `None` untuk event yang bukan pesan server.
fn to_message(meta: &Metadata<'_>, fields: Fields) -> Option<ServerMessage> {
    if is_pg(meta) {
        let text = fields.messages.into_iter().next()?;
        return Some(ServerMessage {
            severity: pg_severity(meta.level()).to_string(),
            text,
        });
    }
    if is_mssql(meta) {
        // `message = %info.message` dan format string "server info message"
        // sama-sama bernama `message`; ambil yang bukan label tetap.
        let text = fields
            .messages
            .into_iter()
            .find(|m| !m.ends_with("server info message"))?;
        let number = fields.number?;
        if MSSQL_NOISE.contains(&number) {
            return None;
        }
        let severity = if number == 0 { "PRINT" } else { "INFO" };
        let text = if number == 0 {
            text
        } else {
            format!("{text} (message {number})")
        };
        return Some(ServerMessage {
            severity: severity.to_string(),
            text,
        });
    }
    None
}

fn log_level(level: &Level) -> log::Level {
    match *level {
        Level::ERROR => log::Level::Error,
        Level::WARN => log::Level::Warn,
        Level::INFO => log::Level::Info,
        Level::DEBUG => log::Level::Debug,
        Level::TRACE => log::Level::Trace,
    }
}

/// Apakah event `tracing` ini akan dicatat bila diteruskan ke `log`. Meniru
/// jembatan bawaan fitur `log` milik tracing, yang berhenti bekerja begitu
/// subscriber global terpasang.
fn log_enabled(meta: &Metadata<'_>) -> bool {
    let level = log_level(meta.level());
    level <= log::max_level()
        && log::logger().enabled(
            &log::Metadata::builder()
                .level(level)
                .target(meta.target())
                .build(),
        )
}

/// Teruskan event non-pesan-server ke `log` (dulu dilakukan jembatan tracing).
fn forward_to_log(meta: &Metadata<'_>, fields: Fields) {
    let mut text = fields.messages.join(" ");
    for other in fields.others {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&other);
    }
    log::logger().log(
        &log::Record::builder()
            .level(log_level(meta.level()))
            .target(meta.target())
            .module_path(meta.module_path())
            .file(meta.file())
            .line(meta.line())
            .args(format_args!("{text}"))
            .build(),
    );
}

struct ServerMessageSubscriber {
    next_span: AtomicU64,
}

impl tracing::Subscriber for ServerMessageSubscriber {
    fn register_callsite(&self, meta: &'static Metadata<'static>) -> Interest {
        if wanted(meta) {
            Interest::always()
        } else if meta.is_event() {
            // Level log bisa berubah saat runtime; tanyakan `enabled` tiap kali.
            Interest::sometimes()
        } else {
            Interest::never()
        }
    }

    fn enabled(&self, meta: &Metadata<'_>) -> bool {
        wanted(meta) || (meta.is_event() && log_enabled(meta))
    }

    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::TRACE)
    }

    fn new_span(&self, _span: &span::Attributes<'_>) -> span::Id {
        span::Id::from_u64(self.next_span.fetch_add(1, Ordering::Relaxed).max(1))
    }

    fn record(&self, _span: &span::Id, _values: &span::Record<'_>) {}

    fn record_follows_from(&self, _span: &span::Id, _follows: &span::Id) {}

    fn event(&self, event: &Event<'_>) {
        let meta = event.metadata();
        let mut fields = Fields::default();
        event.record(&mut fields);
        if !wanted(meta) {
            forward_to_log(meta, fields);
            return;
        }
        if log_enabled(meta) {
            let mut copy = Fields::default();
            event.record(&mut copy);
            forward_to_log(meta, copy);
        }
        if let Some(message) = to_message(meta, fields) {
            push(message);
        }
    }

    fn enter(&self, _span: &span::Id) {}

    fn exit(&self, _span: &span::Id) {}
}

/// Pasang subscriber global. Aman dipanggil lebih dari sekali; bila subscriber
/// lain sudah terpasang, pesan server tidak ditangkap dan hal itu dicatat.
pub fn install() {
    let subscriber = ServerMessageSubscriber {
        next_span: AtomicU64::new(1),
    };
    if tracing::subscriber::set_global_default(subscriber).is_err() {
        log::debug!("[EXEC] tracing subscriber already set; server messages are not captured");
    }
}

/// Teks untuk panel pesan: satu baris per pesan dengan severity di depan.
pub fn format_for_panel(messages: &[ServerMessage]) -> String {
    messages
        .iter()
        .map(|m| format!("{}: {}", m.severity, m.text))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn captures_only_inside_scope_and_caps_count() {
        // Tanpa subscriber global: uji jalur push langsung.
        push(ServerMessage {
            severity: "NOTICE".into(),
            text: "outside".into(),
        });
        let ((), messages) = capture(async {
            for i in 0..(MAX_MESSAGES + 3) {
                push(ServerMessage {
                    severity: "NOTICE".into(),
                    text: format!("n{i}"),
                });
            }
        })
        .await;
        assert_eq!(messages.len(), MAX_MESSAGES + 1);
        assert_eq!(messages[0].text, "n0");
        assert_eq!(
            messages.last().unwrap().text,
            "3 more messages were not shown"
        );
    }

    #[test]
    fn subscriber_turns_events_into_messages() {
        let subscriber = ServerMessageSubscriber {
            next_span: AtomicU64::new(1),
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let ((), messages) = tracing::subscriber::with_default(subscriber, || {
            rt.block_on(capture(async {
                tracing::info!(target: "sqlx::postgres::notice", message = "hello from plpgsql");
                tracing::warn!(target: "sqlx::postgres::notice", message = "careful");
                tracing::info!(target: "mssql_client::client::response", number = 0i64, message = %"printed", "server info message");
                tracing::info!(target: "mssql_client::client::response", number = 5701i64, message = %"Changed database context", "server info message");
                tracing::info!(target: "other::crate", message = "ignored");
            }))
        });
        let got: Vec<(String, String)> =
            messages.into_iter().map(|m| (m.severity, m.text)).collect();
        assert_eq!(
            got,
            vec![
                ("NOTICE".to_string(), "hello from plpgsql".to_string()),
                ("WARNING".to_string(), "careful".to_string()),
                ("PRINT".to_string(), "printed".to_string()),
            ]
        );
    }

    #[test]
    fn panel_format() {
        let text = format_for_panel(&[
            ServerMessage {
                severity: "NOTICE".into(),
                text: "a".into(),
            },
            ServerMessage {
                severity: "PRINT".into(),
                text: "b".into(),
            },
        ]);
        assert_eq!(text, "NOTICE: a\nPRINT: b");
    }
}
