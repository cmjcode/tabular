import Cocoa

// This Swift entry is optional. We can either:
// 1. Replace this compiled binary with the Rust universal binary in a later build phase; OR
// 2. Keep this as a tiny launcher that execs the Rust binary inside the bundle.
// Strategy here: At build time we overwrite this binary with Rust one for simplicity.

@main
class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        // If we keep wrapper strategy, spawn embedded Rust binary.
        // But with replacement strategy, this code is never executed.
    }
}
