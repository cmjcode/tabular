//! Single-instance untuk deep link (M1/M2).
//!
//! Di Linux dan Windows, klik `tabular://...` atau `tabular open <url>`
//! menjalankan proses baru. Proses itu mencoba meneruskan URL ke instance GUI
//! yang sudah berjalan lewat TCP loopback, lalu keluar. Bila tidak ada
//! instance, proses baru menjadi GUI dan memproses URL sendiri.
//!
//! macOS memakai Apple Event (lihat `platform_macos.rs`) untuk bundle `.app`,
//! tetapi listener ini tetap aktif supaya binary CLI di luar bundle juga bisa
//! meneruskan URL.
//!
//! Keamanan: listener hanya bind ke 127.0.0.1 dan setiap pesan wajib membawa
//! token acak dari `instance.json` (izin 0600 di Unix) di data dir pengguna.
//! Payload dibatasi 256 KiB dan hanya diteruskan ke kotak masuk deep link;
//! parsing dan konfirmasi terjadi di GUI.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

const FILE_NAME: &str = "instance.json";
const PROTOCOL: &str = "TABULAR/1";
const MAX_PAYLOAD: u64 = 256 * 1024 + 256;
const IO_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(serde::Serialize, serde::Deserialize)]
struct InstanceFile {
    port: u16,
    token: String,
    pid: u32,
}

fn instance_path() -> PathBuf {
    crate::config::get_data_dir().join(FILE_NAME)
}

/// Kirim URL ke instance yang berjalan. `Ok(true)` bila diterima,
/// `Ok(false)` bila tidak ada instance (atau file handshake basi).
pub fn forward(url: &str) -> Result<bool, String> {
    forward_via(&instance_path(), url)
}

fn forward_via(path: &Path, url: &str) -> Result<bool, String> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Ok(false);
    };
    let Ok(info) = serde_json::from_str::<InstanceFile>(&raw) else {
        return Ok(false);
    };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, info.port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return Ok(false);
    };
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    // Framing per baris: buang newline yang mungkin ada di URL.
    let line = url.replace(['\r', '\n'], "");
    write!(stream, "{PROTOCOL} {}\n{line}\n", info.token).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .map_err(|e| e.to_string())?;
    match reply.trim() {
        "OK" => Ok(true),
        other => Err(format!("running instance rejected the link: {other}")),
    }
}

/// Handle listener; file handshake dibuang saat [`Listener::shutdown`].
pub struct Listener {
    path: PathBuf,
}

impl Listener {
    pub fn shutdown(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Jalankan listener untuk instance GUI ini. URL yang diterima masuk ke
/// `deeplink::push_incoming`.
pub fn start() -> Option<Listener> {
    start_at(instance_path(), crate::deeplink::push_incoming)
}

fn global() -> &'static std::sync::Mutex<Option<Listener>> {
    static GLOBAL: std::sync::OnceLock<std::sync::Mutex<Option<Listener>>> =
        std::sync::OnceLock::new();
    GLOBAL.get_or_init(|| std::sync::Mutex::new(None))
}

/// [`start`] untuk proses GUI; handle disimpan global sampai [`shutdown_global`].
pub fn start_global() {
    if let Ok(mut g) = global().lock()
        && g.is_none()
    {
        *g = start();
    }
}

/// Lepas file handshake saat aplikasi keluar.
pub fn shutdown_global() {
    if let Ok(mut g) = global().lock()
        && let Some(l) = g.take()
    {
        l.shutdown();
    }
}

fn start_at(path: PathBuf, sink: impl Fn(String) + Send + 'static) -> Option<Listener> {
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, 0)) {
        Ok(l) => l,
        Err(e) => {
            log::warn!("[DEEPLINK] single-instance listener unavailable: {e}");
            return None;
        }
    };
    let port = listener.local_addr().ok()?.port();
    let token = random_token();
    let info = InstanceFile {
        port,
        token: token.clone(),
        pid: std::process::id(),
    };
    if let Err(e) = write_private(&path, &serde_json::to_string(&info).ok()?) {
        log::warn!("[DEEPLINK] cannot write {}: {e}", path.display());
        return None;
    }

    let spawn = std::thread::Builder::new()
        .name("tabular-deeplink".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Some(url) = handle_client(stream, &token) {
                    sink(url);
                }
            }
        });
    if let Err(e) = spawn {
        log::warn!("[DEEPLINK] cannot spawn listener thread: {e}");
        let _ = std::fs::remove_file(&path);
        return None;
    }
    log::debug!("[DEEPLINK] single-instance listener on 127.0.0.1:{port}");
    Some(Listener { path })
}

fn handle_client(stream: TcpStream, token: &str) -> Option<String> {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let mut writer = stream.try_clone().ok()?;
    let mut reader = BufReader::new(stream.take(MAX_PAYLOAD));
    let mut header = String::new();
    let mut url = String::new();
    let ok = reader.read_line(&mut header).is_ok() && reader.read_line(&mut url).is_ok();
    let presented = header.trim().strip_prefix(PROTOCOL).map(str::trim);
    let authorized = ok && presented.is_some_and(|t| constant_time_eq(t, token));
    let url = url.trim().to_string();
    if !authorized || url.is_empty() {
        let _ = writer.write_all(b"ERR unauthorized\n");
        log::warn!("[DEEPLINK] rejected single-instance message");
        return None;
    }
    let _ = writer.write_all(b"OK\n");
    Some(url)
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn random_token() -> String {
    use rand::RngExt;
    let mut bytes = [0u8; 24];
    rand::rng().fill(&mut bytes);
    hex::encode(bytes)
}

fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(contents.as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn temp_file(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "tabular-si-{tag}-{}-{}.json",
            std::process::id(),
            random_token()
        ))
    }

    #[test]
    fn forwards_url_to_running_listener() {
        let path = temp_file("ok");
        let (tx, rx) = mpsc::channel();
        let listener = start_at(path.clone(), move |u| {
            let _ = tx.send(u);
        })
        .expect("listener");
        assert_eq!(forward_via(&path, "tabular://open?connection=1"), Ok(true));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "tabular://open?connection=1"
        );
        listener.shutdown();
        assert!(!path.exists());
    }

    #[test]
    fn missing_or_stale_file_means_no_instance() {
        let path = temp_file("none");
        assert_eq!(forward_via(&path, "x"), Ok(false));
        // Port tertutup: file basi dari instance yang crash.
        let stale = InstanceFile {
            port: 1,
            token: "t".into(),
            pid: 0,
        };
        std::fs::write(&path, serde_json::to_string(&stale).unwrap()).unwrap();
        assert_eq!(forward_via(&path, "x"), Ok(false));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wrong_token_is_rejected() {
        let path = temp_file("bad");
        let (tx, rx) = mpsc::channel::<String>();
        let listener = start_at(path.clone(), move |u| {
            let _ = tx.send(u);
        })
        .expect("listener");
        let mut info: InstanceFile =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        info.token = "wrong".into();
        std::fs::write(&path, serde_json::to_string(&info).unwrap()).unwrap();
        assert!(forward_via(&path, "tabular://open?connection=1").is_err());
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
        listener.shutdown();
    }

    #[test]
    fn constant_time_eq_basics() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
    }
}
