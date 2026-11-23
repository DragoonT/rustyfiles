import AppKit

// Velopack's hooks, and an update downloaded before put in place, come first (rf-mac updates.rs)
updatesStartup()

MainActor.assumeIsolated {
    let app = NSApplication.shared
    let delegate = AppDelegate()
    app.delegate = delegate
    app.setActivationPolicy(.regular)
    app.run()
}
