import AppKit
import ServiceManagement
import UniformTypeIdentifiers
import UserNotifications

// MARK: - Developer tools (settings_view.rs `dev`)

extension AppSettings {
    private static var d: UserDefaults { .standard }

    private static func store(_ value: Any?, _ key: String) {
        if let value { d.set(value, forKey: key) } else { d.removeObject(forKey: key) }
        NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": key])
    }

    /// The code editor "Open in editor" uses: "" finds one (VS Code, Cursor, Zed, Sublime Text,
    /// Nova, BBEdit, Xcode), else an app's path.
    static var editorApp: String {
        get { d.string(forKey: "editorApp") ?? "" }
        set { store(newValue.isEmpty ? nil : newValue, "editorApp") }
    }

    /// When the menus offer "Open in editor": "git" (inside a Git repository), "always", "never".
    static var editorMenu: String {
        get { d.string(forKey: "editorMenu") ?? "git" }
        set { store(newValue, "editorMenu") }
    }

    // MARK: Advanced

    static var notifyLongOperations: Bool {
        get { d.object(forKey: "notifyLong") == nil ? true : d.bool(forKey: "notifyLong") }
        set { store(newValue, "notifyLong") }
    }

    /// Where a copy's progress shows besides the Status Center: "status" (the status bar),
    /// "card" (a small floating card), "center" (only the Status Center).
    static var progressPlace: String {
        get { d.string(forKey: "progressPlace") ?? "status" }
        set { store(newValue, "progressPlace") }
    }

    /// Check copies: "off", "fast" (size and date), "full" (content).
    static var verifyCopies: String {
        get { d.string(forKey: "verifyCopies") ?? "off" }
        set { store(newValue, "verifyCopies") }
    }

    /// Copy speed limit in MB/s; 0 is none.
    static var copyLimit: Int {
        get { d.integer(forKey: "copyLimit") }
        set { store(max(newValue, 0), "copyLimit") }
    }

    static var performanceInfo: Bool {
        get { d.bool(forKey: "performanceInfo") }
        set { store(newValue, "performanceInfo") }
    }

    static var keepRunning: Bool {
        get { d.object(forKey: "keepRunning") == nil ? true : d.bool(forKey: "keepRunning") }
        set { store(newValue, "keepRunning") }
    }

    static var flattenOption: Bool {
        get { d.bool(forKey: "flattenOption") }
        set { store(newValue, "flattenOption") }
    }
}

@MainActor
enum Editors {
    private static let known = ["com.microsoft.VSCode", "com.todesktop.230313mzl4w4u92", "dev.zed.Zed", "com.sublimetext.4", "com.panic.Nova",
                                "com.barebones.bbedit", "com.apple.dt.Xcode"]

    static var app: URL? {
        if !AppSettings.editorApp.isEmpty, FileManager.default.fileExists(atPath: AppSettings.editorApp) {
            return URL(fileURLWithPath: AppSettings.editorApp)
        }
        return known.lazy.compactMap { NSWorkspace.shared.urlForApplication(withBundleIdentifier: $0) }.first
    }

    static var name: String { app.map { FileManager.default.displayName(atPath: $0.path) } ?? String(localized: "editor") }

    /// Whether the menus offer it for `dir`.
    static func offered(for dir: URL) -> Bool {
        guard app != nil, dir.isFileURL else { return false }
        switch AppSettings.editorMenu {
        case "always": return true
        case "never": return false
        default:
            var d = dir
            while true {
                if FileManager.default.fileExists(atPath: d.appendingPathComponent(".git").path) { return true }
                let up = d.deletingLastPathComponent()
                if up.path == d.path { return false }
                d = up
            }
        }
    }

    static func open(_ urls: [URL]) {
        guard let app else { return }
        NSWorkspace.shared.open(urls, withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
    }
}

// MARK: - settings files

@MainActor
enum SettingsFile {
    /// Everything rustyfiles keeps in its preferences (not window frames, not the saved session).
    private static var mine: [String: Any] {
        let all = UserDefaults.standard.persistentDomain(forName: Bundle.main.bundleIdentifier ?? "") ?? [:]
        return all.filter { !$0.key.hasPrefix("NS") && $0.key != "lastSession" && !$0.key.hasPrefix("rustyfiles.window") }
    }

    static func export() {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "rustyfiles-settings.plist"
        panel.allowedContentTypes = [.propertyList]
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            let data = try PropertyListSerialization.data(fromPropertyList: mine, format: .xml, options: 0)
            try data.write(to: url)
        } catch { NSAlert(error: error).runModal() }
    }

    static func importFile() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.propertyList]
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            let data = try Data(contentsOf: url)
            guard let dict = try PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any] else { return }
            for (k, v) in dict where !k.hasPrefix("NS") { UserDefaults.standard.set(v, forKey: k) }
            AppSettings.apply()
            Metrics.fontsChanged()
            NSApp.mainMenu = MainMenu.make()
            NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": "import"])
        } catch { NSAlert(error: error).runModal() }
    }
}

// MARK: - log

/// The log file (~/Library/Logs/rustyfiles/rustyfiles.log): what went wrong, with the time.
enum Log {
    static let url: URL = {
        let dir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/rustyfiles")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("rustyfiles.log")
    }()

    private static let queue = DispatchQueue(label: "rustyfiles.log")

    static func write(_ text: String) {
        let line = ISO8601DateFormatter().string(from: Date()) + "  " + text + "\n"
        queue.async {
            if let h = try? FileHandle(forWritingTo: url) {
                h.seekToEndOfFile()
                h.write(Data(line.utf8))
                try? h.close()
            } else {
                try? Data(line.utf8).write(to: url)
            }
        }
    }
}

// MARK: - notifications for long work

@MainActor
enum LongWork {
    /// A copy, move or archive that took more than 10 seconds while rustyfiles was not in front.
    static func finished(_ title: String, seconds: TimeInterval, failed: String? = nil) {
        guard AppSettings.notifyLongOperations, seconds >= 10, !NSApp.isActive else { return }
        let c = UNMutableNotificationContent()
        c.title = failed == nil ? String(localized: "Done") : String(localized: "Stopped")
        c.body = failed.map { title + ": " + $0 } ?? title
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: UUID().uuidString, content: c, trigger: nil))
    }
}

// MARK: - Flatten folder (Advanced › Show the flatten option)

extension FileOps {
    /// Moves every file inside `dir`'s subfolders up into `dir` (a taken name gets " (2)"),
    /// then removes the subfolders left empty. Returns what undoes it.
    nonisolated static func flatten(_ dir: URL) throws -> FileOp {
        let fm = FileManager.default
        var moves: [(from: URL, to: URL)] = []
        let walker = fm.enumerator(at: dir, includingPropertiesForKeys: [.isDirectoryKey], options: [])
        var files: [URL] = []
        var folders: [URL] = []
        while let u = walker?.nextObject() as? URL {
            if (try? u.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true { folders.append(u) } else if u.deletingLastPathComponent().standardizedFileURL != dir.standardizedFileURL { files.append(u) }
        }
        for f in files {
            var target = dir.appendingPathComponent(f.lastPathComponent)
            if fm.fileExists(atPath: target.path) {
                target = unique(f.deletingPathExtension().lastPathComponent, ext: f.pathExtension, in: dir)
            }
            try fm.moveItem(at: f, to: target)
            moves.append((from: target, to: f))
        }
        var removed: [URL] = []
        for folder in folders.sorted(by: { $0.path.count > $1.path.count }) {
            if (try? fm.contentsOfDirectory(atPath: folder.path))?.isEmpty == true {
                try? fm.removeItem(at: folder)
                removed.append(folder)
            }
        }
        // undone: the folders come back as the files return to them
        return .moves(moves)
    }
}

extension SettingsContent {
    static func devRows() -> [SettingsRow] {
        [
            .card(
                title: String(localized: "Code editor"),
                detail: AppSettings.editorApp.isEmpty
                    ? String(localized: "Automatic: \(Editors.name)") : FileManager.default.displayName(atPath: AppSettings.editorApp),
                icon: "code",
                control: .buttons([
                    (String(localized: "Choose…"), {
                        let panel = NSOpenPanel()
                        panel.directoryURL = URL(fileURLWithPath: "/Applications")
                        panel.allowedContentTypes = [.application]
                        if panel.runModal() == .OK, let u = panel.url { AppSettings.editorApp = u.path }
                    }),
                    (String(localized: "Automatic"), { AppSettings.editorApp = "" }),
                ])),
            .card(
                title: String(localized: "“Open in editor” in the menus"), detail: nil, icon: "square-code",
                control: .choice(
                    options: [("git", String(localized: "In Git repositories")), ("always", String(localized: "Always")), ("never", String(localized: "Never"))],
                    get: { AppSettings.editorMenu }, set: { AppSettings.editorMenu = $0 })),
        ]
    }

    static func advancedRows() -> [SettingsRow] {
        let login = SMAppService.mainApp.status == .enabled
        return [
            .section(String(localized: "Starting and closing")),
            .card(
                title: String(localized: "Open rustyfiles when you log in"), detail: nil, icon: "power",
                control: .toggle(get: { login }, set: { on in
                    do {
                        if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
                    } catch { NSAlert(error: error).runModal() }
                    NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": "login"])
                })),
            .card(
                title: String(localized: "Keep running when the last window closes"),
                detail: String(localized: "A new window then opens at once, and Ferry keeps receiving."), icon: "app-window",
                control: .toggle(get: { AppSettings.keepRunning }, set: { AppSettings.keepRunning = $0 })),
            .card(
                title: String(localized: "Default file manager"),
                detail: DefaultFileManager.isDefault
                    ? String(localized: "rustyfiles opens the folders other apps show (“Show in Finder”) and open. The Dock, the desktop and Open / Save panels stay Finder’s.")
                    : String(localized: "Make other apps’ “Show in Finder” and the folders they open come to rustyfiles. The Dock, the desktop and Open / Save panels stay Finder’s."),
                icon: "folder-open",
                control: .buttons(DefaultFileManager.isDefault
                    ? [(String(localized: "Give it back to Finder"), { DefaultFileManager.giveBack() })]
                    : [(String(localized: "Make rustyfiles the default"), { DefaultFileManager.claim() })])),
            .card(
                title: String(localized: "Open archives with rustyfiles"),
                detail: String(localized: "ZIP, 7z and RAR files opened from Finder or other apps open here, like folders."), icon: "archive",
                control: .buttons([(String(localized: "Make rustyfiles the default"), { ArchiveDefaults.claim() })])),
            .section(String(localized: "Copying")),
            .card(
                title: String(localized: "Show progress"), detail: nil, icon: "gauge",
                control: .choice(
                    options: [("status", String(localized: "In the status bar")), ("card", String(localized: "On a floating card")), ("center", String(localized: "Only in the Status Center"))],
                    get: { AppSettings.progressPlace }, set: { AppSettings.progressPlace = $0 })),
            .card(
                title: String(localized: "Notify when long operations finish"),
                detail: String(localized: "Copies, moves and archives that took more than 10 seconds while rustyfiles was in the background."), icon: "bell",
                control: .toggle(get: { AppSettings.notifyLongOperations }, set: { AppSettings.notifyLongOperations = $0 })),
            .card(
                title: String(localized: "Check copies"), detail: String(localized: "Fast: size and date. Full: the content, read back."), icon: "shield-check",
                control: .choice(
                    options: [("off", String(localized: "Off")), ("fast", String(localized: "Fast")), ("full", String(localized: "Full"))],
                    get: { AppSettings.verifyCopies }, set: { AppSettings.verifyCopies = $0 })),
            .card(
                title: String(localized: "Copy speed limit"), detail: String(localized: "So copies to a network share or a USB stick leave room for everything else."), icon: "gauge",
                control: .choice(
                    options: [("0", String(localized: "No limit"))] + [5, 10, 20, 50, 100, 200].map { ("\($0)", "\($0) MB/s") },
                    get: { "\(AppSettings.copyLimit)" }, set: { AppSettings.copyLimit = Int($0) ?? 0 })),
            .section(String(localized: "Tools")),
            .card(
                title: String(localized: "Show “Flatten folder” in the menu"),
                detail: String(localized: "Moves every file in a folder’s subfolders up into it. ⌘Z puts them back."), icon: "layers",
                control: .toggle(get: { AppSettings.flattenOption }, set: { AppSettings.flattenOption = $0 })),
            .card(
                title: String(localized: "Performance in the status bar"), detail: String(localized: "How long the folder took to list."), icon: "activity",
                control: .toggle(get: { AppSettings.performanceInfo }, set: { AppSettings.performanceInfo = $0 })),
            .section(String(localized: "Settings and files")),
            .card(
                title: String(localized: "Settings file"), detail: nil, icon: "file-cog",
                control: .buttons([
                    (String(localized: "Export…"), { SettingsFile.export() }),
                    (String(localized: "Import…"), { SettingsFile.importFile() }),
                ])),
            .card(
                title: String(localized: "Log file"), detail: Log.url.path, icon: "scroll-text",
                control: .buttons([(String(localized: "Show in folder"), { NSWorkspace.shared.activateFileViewerSelecting([Log.url]) })])),
            .card(
                title: String(localized: "Cache"), detail: String(localized: "Thumbnails, and files taken out of archives to be opened."), icon: "database",
                control: .buttons([(String(localized: "Clear"), {
                    ThumbnailCache.shared.clear()
                    try? FileManager.default.removeItem(at: ArchiveStore.scratchFolder())
                })])),
        ]
    }
}

/// Advanced › Open archives with rustyfiles: macOS's default app for ZIP, 7z and RAR.
@MainActor
enum ArchiveDefaults {
    static func claim() {
        let types = ["public.zip-archive", "org.7-zip.7-zip-archive", "com.rarlab.rar-archive"].compactMap { UTType($0) }
        let app = Bundle.main.bundleURL
        for t in types {
            NSWorkspace.shared.setDefaultApplication(at: app, toOpen: t) { error in
                if let error { Log.write("default app for \(t.identifier): \(error.localizedDescription)") }
            }
        }
    }
}

/// Advanced › Default file manager: other apps' "Show in Finder" and the folders they open come
/// to rustyfiles (the global NSFileViewer setting and the folder handler, as ForkLift and Path
/// Finder do it). Finder itself stays: the Dock, the desktop and the Open / Save panels are its.
@MainActor
enum DefaultFileManager {
    private static var id: String { Bundle.main.bundleIdentifier ?? "com.rustyfiles.mac" }

    static var isDefault: Bool {
        UserDefaults(suiteName: UserDefaults.globalDomain)?.string(forKey: "NSFileViewer") == id
    }

    private static func defaults(_ args: [String]) {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/defaults")
        p.arguments = args
        p.standardOutput = FileHandle.nullDevice
        p.standardError = FileHandle.nullDevice
        try? p.run()
        p.waitUntilExit()
    }

    static func claim() {
        defaults(["write", "-g", "NSFileViewer", "-string", id])
        NSWorkspace.shared.setDefaultApplication(at: Bundle.main.bundleURL, toOpen: .folder) { error in
            if let error { Log.write("default folder app: \(error.localizedDescription)") }
            DispatchQueue.main.async { MainActor.assumeIsolated { changed() } }
        }
    }

    static func giveBack() {
        defaults(["delete", "-g", "NSFileViewer"])
        if let finder = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.finder") {
            NSWorkspace.shared.setDefaultApplication(at: finder, toOpen: .folder) { error in
                if let error { Log.write("default folder app back to Finder: \(error.localizedDescription)") }
                DispatchQueue.main.async { MainActor.assumeIsolated { changed() } }
            }
        } else {
            changed()
        }
    }

    private static func changed() {
        NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": "defaultFileManager"])
    }

    /// "Reveal" from other apps (the Apple event Finder gets for "Show in Finder"): the items'
    /// folder opens with them selected.
    static func listenForReveal() {
        NSAppleEventManager.shared().setEventHandler(
            RevealHandler.shared, andSelector: #selector(RevealHandler.reveal(_:reply:)),
            forEventClass: AEEventClass(kAEMiscStandards), andEventID: AEEventID(kAEMakeObjectsVisible))
    }
}

@MainActor
final class RevealHandler: NSObject {
    static let shared = RevealHandler()

    @objc func reveal(_ event: NSAppleEventDescriptor, reply: NSAppleEventDescriptor) {
        guard let list = event.paramDescriptor(forKeyword: keyDirectObject) else { return }
        var urls: [URL] = []
        let count = list.numberOfItems
        if count == 0 {
            if let u = list.fileURLValue { urls.append(u) }
        } else {
            for i in 1...count { if let u = list.atIndex(i)?.fileURLValue { urls.append(u) } }
        }
        guard !urls.isEmpty else { return }
        NSApp.activate()
        AppDelegate.shared.application(NSApp, open: urls)
    }
}
