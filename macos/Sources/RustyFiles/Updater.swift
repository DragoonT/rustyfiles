import AppKit

extension Notification.Name {
    static let rfUpdateChanged = Notification.Name("rustyfiles.updateChanged")
}

extension AppSettings {
    /// Settings › About › Download updates by themselves (on by default, as on Windows).
    static var autoUpdate: Bool {
        get { UserDefaults.standard.object(forKey: "autoUpdate") as? Bool ?? true }
        set { UserDefaults.standard.set(newValue, forKey: "autoUpdate"); NotificationCenter.default.post(name: .rfUpdateChanged, object: nil) }
    }

    /// Settings › About › Beta versions (GitHub pre-releases) too. Off by default.
    static var betaUpdates: Bool {
        get { UserDefaults.standard.bool(forKey: "betaUpdates") }
        set { UserDefaults.standard.set(newValue, forKey: "betaUpdates"); NotificationCenter.default.post(name: .rfUpdateChanged, object: nil) }
    }
}

/// Updates (rf-app's installer.rs, through rf-mac's Velopack): a look soon after start, then
/// every two hours while "Download updates by themselves" is on; a downloaded version waits for
/// "Restart to update" (Settings › About, the status bar, the app menu), or the next start.
@MainActor
enum Updater {
    private(set) static var state: UpdateState?
    private(set) static var checking = false
    private static var timer: Timer?

    /// The version running (Velopack's, else the bundle's).
    static var version: String {
        installedVersion() ?? (Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? coreVersion())
    }

    /// The new version waiting for a restart, if there is one.
    static var ready: String? {
        if case .ready(let v) = state { return v }
        return nil
    }

    static func start() {
        // test copies (snapshots, the self-test) never look
        let env = ProcessInfo.processInfo.environment
        guard env["RF_SNAPSHOT"] == nil, env["RF_SELFTEST"] == nil, env["RF_NOFERRY"] == nil else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 30) {
            if AppSettings.autoUpdate { check(manual: false) }
            // a file manager stays open for days; one call every two hours is well within GitHub's limit
            timer = Timer.scheduledTimer(withTimeInterval: 2 * 3600, repeats: true) { _ in
                MainActor.assumeIsolated { if AppSettings.autoUpdate { check(manual: false) } }
            }
        }
    }

    /// Looks (and downloads), off the main thread. A check asked for by hand says what it found.
    static func check(manual: Bool) {
        guard !checking else { return }
        checking = true
        changed()
        let betas = AppSettings.betaUpdates
        Task.detached(priority: .utility) {
            let found = updateCheck(betas: betas)
            await MainActor.run {
                state = found
                checking = false
                changed()
                if manual { tell(found) }
            }
        }
    }

    /// Quits, and Velopack puts the new version in place and starts it.
    static func restart() {
        guard ready != nil else { return }
        AppDelegate.shared.saveSession()
        if updateApplyAndRestart() {
            NSApp.terminate(nil)
        } else {
            state = .failed(message: String(localized: "The update could not be applied."))
            changed()
        }
    }

    private static func changed() {
        NotificationCenter.default.post(name: .rfUpdateChanged, object: nil)
        NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": "updateState"])
    }

    /// The state in a line (Settings › About).
    static var summary: String {
        if checking { return String(localized: "Looking for updates…") }
        switch state {
        case .none: return String(localized: "Version \(version)")
        case .notInstalled: return String(localized: "This copy was not installed from a release, so it does not update itself.")
        case .upToDate: return String(localized: "rustyfiles \(version) is the newest version.")
        case .ready(let v): return String(localized: "rustyfiles \(v) is downloaded and installs when rustyfiles restarts.")
        case .failed(let m): return String(localized: "The update check failed: \(m)")
        }
    }

    private static func tell(_ s: UpdateState) {
        let a = NSAlert()
        a.messageText = summary
        if case .ready = s {
            a.addButton(withTitle: String(localized: "Restart to update"))
            a.addButton(withTitle: String(localized: "Later"))
            if a.runModal() == .alertFirstButtonReturn { restart() }
        } else {
            a.runModal()
        }
    }
}
