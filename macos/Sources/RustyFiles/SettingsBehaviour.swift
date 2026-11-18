import AppKit

/// The behaviour settings of the General and Files pages (settings.rs), kept with the others in
/// the app's defaults.
extension AppSettings {
    private static var d: UserDefaults { .standard }

    private static func flag(_ key: String, _ fallback: Bool) -> Bool {
        d.object(forKey: key) == nil ? fallback : d.bool(forKey: key)
    }

    private static func store(_ value: Any?, _ key: String) {
        if let value { d.set(value, forKey: key) } else { d.removeObject(forKey: key) }
        NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": key])
    }

    // MARK: General › Startup

    enum Startup: String, CaseIterable { case resume, home, pages }

    /// What a new start opens: the windows and tabs that were open, Home, or chosen pages.
    static var startup: Startup {
        get { d.string(forKey: "startup").flatMap(Startup.init(rawValue:)) ?? .home }
        set { store(newValue.rawValue, "startup") }
    }

    /// "Open specific pages": the folders (or "rustyfiles:home") opened as tabs.
    static var startupPages: [String] {
        get { d.stringArray(forKey: "startupPages") ?? [] }
        set { store(newValue, "startupPages") }
    }

    /// What a new tab shows: "home" (the Home page) or "current" (the folder of the tab it opens from).
    static var newTabOpens: String {
        get { d.string(forKey: "newTabOpens") ?? "home" }
        set { store(newValue, "newTabOpens") }
    }

    /// A tab opened from a menu ("Open in new tab") becomes the active one.
    static var switchToNewTab: Bool {
        get { flag("switchToNewTab", true) }
        set { store(newValue, "switchToNewTab") }
    }

    /// Folders opened from other apps (Dock, `open`, Finder's "Open With") get a window of their
    /// own; off: a tab in the front window.
    static var externalInNewWindow: Bool {
        get { flag("externalInNewWindow", true) }
        set { store(newValue, "externalInNewWindow") }
    }

    // MARK: General › Right-click menu

    /// The item menu's optional commands (settings.rs `ctx_*`), each on unless turned off.
    enum MenuCommand: String, CaseIterable {
        case newTab, newWindow, copyPath, alias, pin, archive, tags, ferry, terminal

        var title: String {
            switch self {
            case .newTab: String(localized: "Open in new tab")
            case .newWindow: String(localized: "Open in new window")
            case .copyPath: String(localized: "Copy item path")
            case .alias: String(localized: "Make alias")
            case .pin: String(localized: "Pin to sidebar")
            case .archive: String(localized: "Compress / Extract")
            case .tags: String(localized: "Tags")
            case .ferry: String(localized: "Send with Ferry")
            case .terminal: String(localized: "Open in Terminal")
            }
        }
    }

    static func shows(_ c: MenuCommand) -> Bool { !(d.stringArray(forKey: "menuHidden") ?? []).contains(c.rawValue) }

    static func setShows(_ c: MenuCommand, _ on: Bool) {
        var hidden = Set(d.stringArray(forKey: "menuHidden") ?? [])
        if on { hidden.remove(c.rawValue) } else { hidden.insert(c.rawValue) }
        store(Array(hidden).sorted(), "menuHidden")
    }

    // MARK: Files › View

    /// A check box before each name that adds the item to the selection or takes it away.
    static var selectionBoxes: Bool {
        get { flag("selectionBoxes", false) }
        set { store(newValue, "selectionBoxes") }
    }

    /// Resting the pointer on a row selects it.
    static var selectOnHover: Bool {
        get { flag("selectOnHover", false) }
        set { store(newValue, "selectOnHover") }
    }

    static var showExtensions: Bool {
        get { flag("showExtensions", true) }
        set { store(newValue, "showExtensions") }
    }

    // MARK: Files › Behaviours

    enum SingleClick: String, CaseIterable { case never, folders, always }

    /// Open with one click instead of two.
    static var singleClick: SingleClick {
        get { d.string(forKey: "singleClick").flatMap(SingleClick.init(rawValue:)) ?? .never }
        set { store(newValue.rawValue, "singleClick") }
    }

    /// A folder opened from the list opens in a new tab.
    static var foldersInNewTab: Bool {
        get { flag("foldersInNewTab", false) }
        set { store(newValue, "foldersInNewTab") }
    }

    enum ConfirmDelete: String, CaseIterable { case never, permanent, always }

    /// Ask before deleting: never, only when it is for good (the default), or also before the Trash.
    static var confirmDelete: ConfirmDelete {
        get { d.string(forKey: "confirmDelete").flatMap(ConfirmDelete.init(rawValue:)) ?? .permanent }
        set { store(newValue.rawValue, "confirmDelete") }
    }

    static var warnExtensionChange: Bool {
        get { flag("warnExtensionChange", true) }
        set { store(newValue, "warnExtensionChange") }
    }

    static var doubleClickEmptyGoesUp: Bool {
        get { flag("doubleClickEmptyGoesUp", false) }
        set { store(newValue, "doubleClickEmptyGoesUp") }
    }

    static var upSelectsPrevious: Bool {
        get { flag("upSelectsPrevious", true) }
        set { store(newValue, "upSelectsPrevious") }
    }

    static var addressCommands: Bool {
        get { flag("addressCommands", false) }
        set { store(newValue, "addressCommands") }
    }

    static var archivesAsFolders: Bool {
        get { flag("archivesAsFolders", true) }
        set { store(newValue, "archivesAsFolders") }
    }

    static var openAfterExtract: Bool {
        get { flag("openAfterExtract", false) }
        set { store(newValue, "openAfterExtract") }
    }

    /// The Size column shows folders' sizes by itself, worked out in the background.
    static var autoFolderSizes: Bool {
        get { flag("autoFolderSizes", false) }
        set { store(newValue, "autoFolderSizes") }
    }

    /// Folder sizes count what they take on the disk instead of their bytes.
    static var folderSizeOnDisk: Bool {
        get { flag("folderSizeOnDisk", false) }
        set { store(newValue, "folderSizeOnDisk") }
    }

    // MARK: Files › Folder aliases

    /// "@name" in the address bar goes to the folder (settings.rs `folder_aliases`).
    static var folderAliases: [String: String] {
        get { d.dictionary(forKey: "folderAliases") as? [String: String] ?? [:] }
        set { store(newValue, "folderAliases") }
    }
}

// MARK: - sessions (Startup › Continue where you left off)

/// The windows and their tabs, saved as they change and read at the next start.
@MainActor
enum Sessions {
    struct Window: Codable {
        var tabs: [String]
        var active: Int
    }

    private static let key = "lastSession"

    static func save(_ windows: [Window]) {
        if let data = try? JSONEncoder().encode(windows) { UserDefaults.standard.set(data, forKey: key) }
    }

    static func last() -> [Window] {
        guard let data = UserDefaults.standard.data(forKey: key) else { return [] }
        return (try? JSONDecoder().decode([Window].self, from: data)) ?? []
    }

    /// A tab's address as it is stored: a path, or "rustyfiles:home" / "rustyfiles:settings".
    static func url(_ s: String) -> URL? {
        if s.hasPrefix("rustyfiles:") { return URL(string: s) }
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: s, isDirectory: &isDir), isDir.boolValue else { return nil }
        return URL(fileURLWithPath: s)
    }

    static func string(_ u: URL) -> String { u.isFileURL ? u.path : u.absoluteString }
}

// MARK: - the address bar runs programs (Files › Run commands from the address bar)

enum AddressCommand {
    /// Text that is not a place: an app ("terminal", "code", "Visual Studio Code") opens the
    /// folder; a program on the PATH ("code", "open", "make") runs in it. False: nothing found.
    @MainActor
    static func run(_ text: String, in folder: URL) -> Bool {
        let parts = text.trimmingCharacters(in: .whitespaces).split(separator: " ", maxSplits: 1).map(String.init)
        guard let name = parts.first, !name.contains("/") else { return false }
        // an app by its name
        let dirs = ["/Applications", "/Applications/Utilities", "/System/Applications", "/System/Applications/Utilities",
                    FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Applications").path]
        let whole = text.trimmingCharacters(in: .whitespaces).lowercased()
        for dir in dirs {
            let apps = (try? FileManager.default.contentsOfDirectory(atPath: dir)) ?? []
            if let app = apps.first(where: { $0.lowercased() == whole + ".app" }) {
                NSWorkspace.shared.open([folder], withApplicationAt: URL(fileURLWithPath: dir).appendingPathComponent(app), configuration: NSWorkspace.OpenConfiguration())
                return true
            }
        }
        // a program on the PATH, run in the folder, out of sight
        let path = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"]
        guard let exe = path.map({ $0 + "/" + name }).first(where: { FileManager.default.isExecutableFile(atPath: $0) }) else { return false }
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/zsh")
        p.arguments = ["-lc", ([exe] + parts.dropFirst()).joined(separator: " ")]
        p.currentDirectoryURL = folder
        p.standardOutput = FileHandle.nullDevice
        p.standardError = FileHandle.nullDevice
        return (try? p.run()) != nil
    }
}
