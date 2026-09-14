//! Single place that knows how to hand a URL to the platform browser.
//!
//! This used to be duplicated in `self_update::open_url` and
//! `sync::auth::open_url`, and *both* copies fell through to a no-op / error on
//! iOS. That silently broke OAuth sign-in on iPad: `start_oauth_flow` would
//! open nothing, the ticket poller would spin for its full 180s window, and the
//! UI sat on "Opening browser…" forever. Keep the platform matrix here so a new
//! target can only be missed once.

use log::debug;

/// Open `url` in the user's default browser.
///
/// Returns `Err` with a human-readable reason when the platform refused or has
/// no way to open a browser — callers decide whether that is fatal.
pub fn open_url(url: &str) -> Result<(), String> {
    debug!("Opening URL: {}", url);
    open_url_impl(url)
}

#[cfg(target_os = "macos")]
fn open_url_impl(url: &str) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
fn open_url_impl(url: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn open_url_impl(url: &str) -> Result<(), String> {
    // The empty "" is the window-title argument `start` expects; without it a
    // quoted URL is treated as the title and nothing opens.
    std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// iOS has no subprocess to shell out to — the only supported route is
/// `-[UIApplication openURL:options:completionHandler:]`, which must be called
/// on the main thread. egui's update loop *is* the main thread on iOS, so the
/// sign-in button path satisfies that; anything else is rejected rather than
/// risking a UIKit main-thread assertion.
#[cfg(target_os = "ios")]
fn open_url_impl(url: &str) -> Result<(), String> {
    // objc2 0.2.x keeps MainThreadMarker in objc2-foundation; it only moves to
    // the objc2 root in 0.6, which winit does not pin us to.
    use objc2_foundation::{MainThreadMarker, NSDictionary, NSString, NSURL};
    use objc2_ui_kit::UIApplication;

    let mtm = MainThreadMarker::new()
        .ok_or_else(|| "openURL must be called from the main thread".to_string())?;

    let ns_string = NSString::from_str(url);
    let ns_url = unsafe { NSURL::URLWithString(&ns_string) }
        .ok_or_else(|| format!("Not a valid URL: {url}"))?;

    let app = UIApplication::sharedApplication(mtm);
    let options = NSDictionary::new();
    unsafe { app.openURL_options_completionHandler(&ns_url, &options, None) };

    Ok(())
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "windows",
    target_os = "ios"
)))]
fn open_url_impl(_url: &str) -> Result<(), String> {
    Err("Cannot open browser on this platform".to_string())
}
