import AppKit

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    static var shared: AppDelegate { NSApp.delegate as! AppDelegate }

    /// Every open browser window. AppKit does not keep window controllers alive by itself.
    private(set) var controllers: [BrowserWindowController] = []

    /// New windows open the Home page, as on Windows.
    private var home: URL { BrowserTab.homeURL }

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Sizes and dates in rf-core's English wording until the interface is translated.
        // sizes and dates speak the interface's language ("bayt", decimal comma)
        setEnglish(on: !AppSettings.speaksTurkish)
        AppSettings.apply()
        // RF_NOFERRY (development): a second copy of the app must not join the network as this Mac
        if ProcessInfo.processInfo.environment["RF_NOFERRY"] == nil { FerryService.shared.startIfEnabled() }
        // RF_SELFTEST (development): copies, moves, undoes and redoes in a scratch folder, prints, quits
        if ProcessInfo.processInfo.environment["RF_SELFTEST"] != nil { return SelfTest.run() }
        // RF_DEMO_MENU=item|background (development): the list's menu opens by itself, to be looked at
        // RF_DEMO_PREVIEW=row (development): the preview pane open on that row, not remembered
        if let row = ProcessInfo.processInfo.environment["RF_DEMO_PREVIEW"].flatMap(Int.init) {
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) { [weak self] in self?.controllers.first?.demoPreview(row: row) }
        }
        // RF_DEMO_PROPS=path[:page] (development): a Properties window on that path
        if let spec = ProcessInfo.processInfo.environment["RF_DEMO_PROPS"] {
            let parts = spec.split(separator: ":", maxSplits: 1).map(String.init)
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
                PropertiesWindow.show([URL(fileURLWithPath: parts[0])], page: parts.count > 1 ? Int(parts[1]) ?? 0 : 0)
            }
        }
        if let which = ProcessInfo.processInfo.environment["RF_DEMO_MENU"] {
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) { [weak self] in self?.controllers.first?.demoMenu(item: which == "item") }
        }
        if ProcessInfo.processInfo.environment["RF_DEMO_STATUS"] != nil {
            StatusCenter.demo()
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { [weak self] in
                self?.controllers.first?.toggleStatusCenter()
            }
        }
        NotificationCenter.default.addObserver(forName: NSColor.systemColorsDidChangeNotification, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated {
                Palette.invalidate()
                NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": "accent"])
            }
        }
        NSApp.mainMenu = MainMenu.make()
        DefaultFileManager.listenForReveal()
        // Settings › File organizer: automatic rules watch their folders
        if ProcessInfo.processInfo.environment["RF_NOFERRY"] == nil { Organizer.watch() }
        if controllers.isEmpty {
            // RF_START (development): a folder to open, or a file to open its folder with it selected
            let env = ProcessInfo.processInfo.environment["RF_START"]
            if let env, env.hasPrefix("settings") {
                let wc = openWindow(at: home)
                wc.openSettings(nil)
                let page = env.split(separator: ":").dropFirst().first.map(String.init) ?? ""
                if let p = SettingsPage.allCases.first(where: { "\($0)" == page }) { wc.showSettingsPage(p) }
            } else if let start = env.map(URL.init(fileURLWithPath:)) {
                application(NSApp, open: [start])
            } else {
                openAtStart()
            }
        }
        NSApp.activate()
        if let path = ProcessInfo.processInfo.environment["RF_SNAPSHOT"] {
            snapshotAndQuit(to: path)
        }
    }

    /// Development aid: `RF_SNAPSHOT=out.png` draws the first window into a PNG after it has
    /// settled, then quits (for checking the layout without screen-recording permission;
    /// glass and vibrancy are not drawn this way).
    private func snapshotAndQuit(to path: String) {
        // RF_SNAPSHOT_DELAY (seconds) waits longer, RF_DEMO_SPEED runs the speed test first
        if ProcessInfo.processInfo.environment["RF_DEMO_SPEED"] != nil { SpeedTest.shared.run() }
        let delay = Double(ProcessInfo.processInfo.environment["RF_SNAPSHOT_DELAY"] ?? "") ?? 2
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in
            guard let view = self?.controllers.first?.window?.contentView?.superview,
                let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds)
            else { exit(1) }
            view.cacheDisplay(in: view.bounds, to: rep)
            try? rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
            NSApp.terminate(nil)
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        saveSession()
        FerryService.shared.stop()
    }

    // MARK: sessions (General › When rustyfiles starts)

    private var restoring = false

    /// Remembers the windows and their tabs (for "Continue where you left off"); a window that
    /// closes while others stay leaves the session.
    func saveSession() {
        // development copies (RF_NOFERRY) leave the owner's session alone
        guard !restoring, ProcessInfo.processInfo.environment["RF_NOFERRY"] == nil else { return }
        Sessions.save(controllers.map { Sessions.Window(tabs: $0.tabAddresses, active: $0.activeTab) })
    }

    /// The first windows, as General › When rustyfiles starts says.
    private func openAtStart() {
        switch AppSettings.startup {
        case .resume:
            let windows = Sessions.last().filter { !$0.tabs.compactMap(Sessions.url).isEmpty }
            guard !windows.isEmpty else { openWindow(at: home); return }
            restoring = true
            for w in windows {
                openWindow(at: home).restore(w.tabs, active: w.active)
            }
            restoring = false
            saveSession()
        case .pages:
            let pages = AppSettings.startupPages
            if pages.compactMap(Sessions.url).isEmpty { openWindow(at: home) } else { openWindow(at: home).restore(pages, active: 0) }
        case .home:
            openWindow(at: home)
        }
    }

    /// The window in front (or the first one).
    var frontController: BrowserWindowController? {
        (NSApp.keyWindow?.windowController as? BrowserWindowController) ?? (NSApp.mainWindow?.windowController as? BrowserWindowController)
            ?? controllers.first
    }

    /// Advanced › Keep running when the last window closes.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { !AppSettings.keepRunning }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { openWindow(at: home) }
        return true
    }

    func applicationSupportsSecureRestorableState(_ app: NSApplication) -> Bool { true }

    /// Folders dropped on the Dock icon or opened with `open -a rustyfiles <folder>`.
    func application(_ application: NSApplication, open urls: [URL]) {
        for url in urls {
            var isDir: ObjCBool = false
            if FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir), isDir.boolValue {
                // General › Folders opened from other apps open in a new window; off: a tab
                if !AppSettings.externalInNewWindow, let front = frontController {
                    front.openTab(url, activate: true)
                    front.window?.makeKeyAndOrderFront(nil)
                } else {
                    openWindow(at: url)
                }
            } else if isArchive(path: url.path), AppSettings.archivesAsFolders {
                // an archive sent here (Advanced › Open archives with rustyfiles) opens like a folder
                if !AppSettings.externalInNewWindow, let front = frontController {
                    front.openTab(url, activate: true)
                    front.window?.makeKeyAndOrderFront(nil)
                } else {
                    openWindow(at: url)
                }
            } else {
                openWindow(at: url.deletingLastPathComponent(), selecting: url.lastPathComponent)
            }
        }
    }

    // MARK: windows

    @discardableResult
    func openWindow(at url: URL, selecting name: String? = nil) -> BrowserWindowController {
        let wc = makeController(at: url, selecting: name)
        wc.showWindow(nil)
        return wc
    }

    /// A controller kept alive until its window closes; the caller shows it.
    func makeController(at url: URL, selecting name: String? = nil) -> BrowserWindowController {
        let wc = BrowserWindowController(url: url, selecting: name)
        wc.onClose = { [weak self] closed in
            guard let self else { return }
            self.controllers.removeAll { $0 === closed }
            // the last window closing keeps its tabs for the next start
            if !self.controllers.isEmpty { self.saveSession() }
        }
        controllers.append(wc)
        return wc
    }

    @objc func newWindow(_ sender: Any?) {
        let key = NSApp.keyWindow?.windowController as? BrowserWindowController
        openWindow(at: key?.current ?? home)
    }
}
