import AppKit
import Carbon.HIToolbox

// MARK: - smart folders

/// Finder's smart folders (.savedSearch: a Spotlight query and where it looks), opened here as a
/// listing gathered from everywhere, like a tag's files: rustyfiles:smart/<path of the file>.
enum SmartFolder {
    static func url(_ file: URL) -> URL {
        URL(string: "rustyfiles:smart/" + (file.path.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? file.path))!
    }

    /// The .savedSearch file a results page shows, if `url` is one.
    static func file(of url: URL) -> URL? {
        guard url.scheme == "rustyfiles" else { return nil }
        let spec = url.absoluteString.dropFirst("rustyfiles:".count)
        guard spec.hasPrefix("smart/"), let p = String(spec.dropFirst(6)).removingPercentEncoding else { return nil }
        return URL(fileURLWithPath: p)
    }

    static func isSmartFolder(_ url: URL) -> Bool { url.pathExtension.lowercased() == "savedsearch" }

    /// The files the saved search finds now. Blocks: off the main thread.
    static func results(_ file: URL) -> [String] {
        guard let data = try? Data(contentsOf: file),
            let plist = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any],
            let query = plist["RawQuery"] as? String, !query.isEmpty
        else { return [] }
        let criteria = plist["SearchCriteria"] as? [String: Any]
        let scopes = (criteria?["FXScopeArrayOfPaths"] as? [String]) ?? ["kMDQueryScopeComputer"]
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        var found: [String] = []
        var seen: Set<String> = []
        for scope in scopes {
            var args: [String] = []
            switch scope {
            case "kMDQueryScopeHome": args = ["-onlyin", home]
            case "kMDQueryScopeComputer", "kMDQueryScopeAllIndexed": args = []
            case "kMDQueryScopeNetwork": continue
            default: args = ["-onlyin", (scope as NSString).expandingTildeInPath]
            }
            let p = Process()
            p.executableURL = URL(fileURLWithPath: "/usr/bin/mdfind")
            p.arguments = args + [query]
            let out = Pipe()
            p.standardOutput = out
            p.standardError = FileHandle.nullDevice
            guard (try? p.run()) != nil else { continue }
            let text = String(decoding: out.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            p.waitUntilExit()
            for line in text.split(separator: "\n") where found.count < 50_000 {
                let s = String(line)
                if seen.insert(s).inserted { found.append(s) }
            }
        }
        return found
    }
}

// MARK: - the global shortcut

/// A shortcut that opens a rustyfiles window from any app (Windows' Win+E), registered with the
/// system (no Accessibility permission needed). Settings › General chooses it, or none.
@MainActor
enum GlobalHotKey {
    struct Choice {
        let id: String
        let title: String
        let key: Int
        let mods: Int
    }

    static let choices: [Choice] = [
        Choice(id: "ctrl-opt-cmd-f", title: "⌃⌥⌘F", key: kVK_ANSI_F, mods: controlKey | optionKey | cmdKey),
        Choice(id: "ctrl-opt-cmd-e", title: "⌃⌥⌘E", key: kVK_ANSI_E, mods: controlKey | optionKey | cmdKey),
        Choice(id: "opt-cmd-e", title: "⌥⌘E", key: kVK_ANSI_E, mods: optionKey | cmdKey),
        Choice(id: "ctrl-opt-space", title: "⌃⌥Space", key: kVK_Space, mods: controlKey | optionKey),
    ]

    private static var ref: EventHotKeyRef?
    private static var handler: EventHandlerRef?

    /// Registers the chosen shortcut (again, after a change).
    static func apply() {
        if let ref { UnregisterEventHotKey(ref); self.ref = nil }
        guard let c = choices.first(where: { $0.id == AppSettings.globalHotKey }) else { return }
        if handler == nil {
            var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
            InstallEventHandler(GetApplicationEventTarget(), { _, _, _ in
                DispatchQueue.main.async { MainActor.assumeIsolated { GlobalHotKey.pressed() } }
                return noErr
            }, 1, &spec, nil, &handler)
        }
        let id = EventHotKeyID(signature: OSType(0x5246_484B), id: 1)  // "RFHK"
        let status = RegisterEventHotKey(UInt32(c.key), UInt32(c.mods), id, GetApplicationEventTarget(), 0, &ref)
        if status != noErr { Log.write("global shortcut \(c.title) not registered (\(status)): another app may have it") }
    }

    /// Like Win+E: a new window at Home, in front.
    private static func pressed() {
        NSApp.activate()
        AppDelegate.shared.openWindow(at: BrowserTab.homeURL)
    }
}

// MARK: - disks that are connected

/// Settings › General › Open disks when they're connected: a USB stick, a card or a disk image
/// that is mounted opens in a tab of the front window (a window when there is none), as Finder
/// does when it is the file manager.
@MainActor
enum MountWatcher {
    static func start() {
        NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didMountNotification, object: nil, queue: .main) { note in
            guard let url = note.userInfo?[NSWorkspace.volumeURLUserInfoKey] as? URL else { return }
            MainActor.assumeIsolated { mounted(url) }
        }
    }

    private static func mounted(_ url: URL) {
        guard AppSettings.openMountedDisks else { return }
        let v = try? url.resourceValues(forKeys: [.volumeIsRemovableKey, .volumeIsEjectableKey, .volumeIsInternalKey, .volumeIsBrowsableKey])
        // the Mac's own volumes (Recovery, Preboot…) and hidden ones are left alone
        guard v?.volumeIsBrowsable != false, v?.volumeIsRemovable == true || v?.volumeIsEjectable == true || v?.volumeIsInternal == false else { return }
        if let wc = AppDelegate.shared.frontController {
            wc.openTab(url)
            wc.window?.makeKeyAndOrderFront(nil)
        } else {
            AppDelegate.shared.openWindow(at: url)
        }
        NSApp.activate()
    }
}

extension AppSettings {
    /// The global shortcut's id (GlobalHotKey.choices), "" for none. ⌃⌥⌘F to start with.
    static var globalHotKey: String {
        get { UserDefaults.standard.string(forKey: "globalHotKey") ?? "ctrl-opt-cmd-f" }
        set {
            UserDefaults.standard.set(newValue, forKey: "globalHotKey")
            GlobalHotKey.apply()
            NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": "globalHotKey"])
        }
    }

    /// Open a disk when it is connected; on by default once rustyfiles is the file manager.
    static var openMountedDisks: Bool {
        get { UserDefaults.standard.object(forKey: "openMountedDisks") as? Bool ?? DefaultFileManager.isDefault }
        set {
            UserDefaults.standard.set(newValue, forKey: "openMountedDisks")
            NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": "openMountedDisks"])
        }
    }
}
