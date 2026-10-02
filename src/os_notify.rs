//! Notifikasi OS untuk operasi panjang (checklist I4).
//!
//! Tanpa dependensi baru: macOS memakai `osascript` (`display notification`),
//! Linux `notify-send` (libnotify), Windows toast lewat PowerShell/WinRT.
//! Proses dijalankan di thread terpisah dan kegagalan hanya dicatat di log,
//! karena notifikasi bersifat pelengkap untuk toast in-app. iOS tidak didukung
//! (tidak ada proses anak).

use std::time::Duration;

/// Default ambang durasi sebelum notifikasi dikirim.
pub const DEFAULT_THRESHOLD_SECS: u32 = 10;

/// Kirim notifikasi hanya bila fitur aktif, jendela tidak sedang fokus, dan
/// operasi berjalan minimal `threshold_secs` (0 dianggap 1 detik).
pub fn should_notify(
    enabled: bool,
    window_focused: bool,
    elapsed: Duration,
    threshold_secs: u32,
) -> bool {
    enabled && !window_focused && elapsed >= Duration::from_secs(threshold_secs.max(1) as u64)
}

/// Batasi panjang teks agar notifikasi tidak terpotong aneh oleh OS.
fn clip(s: &str, max: usize) -> String {
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        one_line
    } else {
        let mut out: String = one_line.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Literal string AppleScript dengan escape `\` dan `"`.
fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Skrip `osascript` untuk macOS.
pub fn macos_script(title: &str, body: &str) -> String {
    format!(
        "display notification {} with title {}",
        applescript_string(&clip(body, 200)),
        applescript_string(&clip(title, 80))
    )
}

/// Literal string PowerShell ber-kutip tunggal.
fn powershell_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Skrip PowerShell yang menampilkan toast WinRT. AppId PowerShell dipakai
/// karena AppId Tabular belum terdaftar di Start menu pada build unpackaged.
pub fn windows_script(title: &str, body: &str) -> String {
    format!(
        "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null; \
         $t = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
         $x = $t.GetElementsByTagName('text'); \
         $x.Item(0).AppendChild($t.CreateTextNode({})) > $null; \
         $x.Item(1).AppendChild($t.CreateTextNode({})) > $null; \
         [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe').Show([Windows.UI.Notifications.ToastNotification]::new($t))",
        powershell_string(&clip(title, 80)),
        powershell_string(&clip(body, 200))
    )
}

/// Tampilkan notifikasi OS secara asinkron (tidak memblokir UI).
#[cfg(not(target_os = "ios"))]
pub fn send(title: &str, body: &str) {
    let title = title.to_string();
    let body = body.to_string();
    let spawned = std::thread::Builder::new()
        .name("os-notify".into())
        .spawn(move || {
            if let Err(e) = send_blocking(&title, &body) {
                log::warn!("[NOTIFY] OS notification failed: {e}");
            }
        });
    if let Err(e) = spawned {
        log::warn!("[NOTIFY] Cannot spawn notification thread: {e}");
    }
}

#[cfg(target_os = "ios")]
pub fn send(_title: &str, _body: &str) {}

#[cfg(target_os = "macos")]
fn send_blocking(title: &str, body: &str) -> std::io::Result<()> {
    let status = std::process::Command::new("osascript")
        .arg("-e")
        .arg(macos_script(title, body))
        .status()?;
    exit_ok(status)
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
fn send_blocking(title: &str, body: &str) -> std::io::Result<()> {
    let status = std::process::Command::new("notify-send")
        .arg("--app-name=Tabular")
        .arg(clip(title, 80))
        .arg(clip(body, 200))
        .status()?;
    exit_ok(status)
}

#[cfg(target_os = "windows")]
fn send_blocking(title: &str, body: &str) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(windows_script(title, body))
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    exit_ok(status)
}

#[cfg(not(target_os = "ios"))]
fn exit_ok(status: std::process::ExitStatus) -> std::io::Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("exited with {status}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hanya_saat_tidak_fokus_dan_melewati_ambang() {
        let d = Duration::from_secs(12);
        assert!(should_notify(true, false, d, 10));
        assert!(!should_notify(true, true, d, 10));
        assert!(!should_notify(false, false, d, 10));
        assert!(!should_notify(true, false, Duration::from_secs(9), 10));
        // Ambang 0 diperlakukan 1 detik.
        assert!(!should_notify(true, false, Duration::from_millis(200), 0));
    }

    #[test]
    fn skrip_macos_meng_escape_kutip() {
        let s = macos_script("Query \"done\"", "SELECT '\\x'\n FROM t");
        assert_eq!(
            s,
            "display notification \"SELECT '\\\\x' FROM t\" with title \"Query \\\"done\\\"\""
        );
    }

    #[test]
    fn skrip_windows_meng_escape_kutip_tunggal() {
        let s = windows_script("It's done", "a");
        assert!(s.contains("'It''s done'"));
    }

    #[test]
    fn teks_panjang_dipotong() {
        let long = "x".repeat(500);
        assert_eq!(clip(&long, 10).chars().count(), 10);
        assert!(clip(&long, 10).ends_with('…'));
    }
}
