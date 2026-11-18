import AppKit

// MARK: - File types (open_rules_page.rs)

/// The user's own apps for kinds of files: a list per group of extensions. Its first app
/// opens such a file on a double click (unless that is left to macOS), and all of them stand
/// first under "Open with". Only rustyfiles follows the lists; macOS's defaults are untouched.
@MainActor
enum OpenRules {
    struct Rule: Codable, Equatable {
        var exts: [String]
        var apps: [String]
        /// The first app opens these files on a double click (off: macOS's default does).
        var onOpen: Bool = true
    }

    private static let key = "openRules"

    static var all: [Rule] {
        get {
            guard let data = UserDefaults.standard.data(forKey: key) else { return [] }
            return (try? JSONDecoder().decode([Rule].self, from: data)) ?? []
        }
        set {
            if let data = try? JSONEncoder().encode(newValue) { UserDefaults.standard.set(data, forKey: key) }
            NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": key])
        }
    }

    static func rule(for url: URL) -> Rule? {
        let ext = url.pathExtension.lowercased()
        guard !ext.isEmpty else { return nil }
        return all.first { $0.exts.contains(ext) && !$0.apps.isEmpty }
    }

    /// The app a double click uses for `url`, if a list says so.
    static func opener(for url: URL) -> URL? {
        guard let r = rule(for: url), r.onOpen, let first = r.apps.first, FileManager.default.fileExists(atPath: first) else { return nil }
        return URL(fileURLWithPath: first)
    }

    static func appName(_ path: String) -> String { FileManager.default.displayName(atPath: path) }

    // editing

    static func parse(_ text: String) -> [String] {
        text.split(whereSeparator: { $0 == "," || $0 == " " || $0 == ";" })
            .map { $0.trimmingCharacters(in: CharacterSet(charactersIn: ". ")).lowercased() }
            .filter { !$0.isEmpty }
    }

    static func askExtensions(_ initial: String) -> [String]? {
        guard let text = TagPrompt.ask(String(localized: "Which extensions is this list for? Separate several with commas."), initial: initial) else { return nil }
        let exts = parse(text)
        return exts.isEmpty ? nil : exts
    }

    static func add() {
        guard let exts = askExtensions("") else { return }
        let taken = exts.filter { e in all.contains { $0.exts.contains(e) } }
        guard taken.isEmpty else {
            let a = NSAlert()
            a.messageText = String(localized: "These extensions already have a list: \(taken.map { "." + $0 }.joined(separator: ", "))")
            a.runModal()
            return
        }
        all = all + [Rule(exts: exts, apps: [])]
        addApp(all.count - 1)
    }

    static func addApp(_ i: Int) {
        let panel = NSOpenPanel()
        panel.directoryURL = URL(fileURLWithPath: "/Applications")
        panel.allowedContentTypes = [.application]
        panel.allowsMultipleSelection = true
        panel.prompt = String(localized: "Add")
        guard panel.runModal() == .OK else { return }
        var list = all
        guard list.indices.contains(i) else { return }
        for u in panel.urls where !list[i].apps.contains(u.path) { list[i].apps.append(u.path) }
        all = list
    }

    /// The list's apps: make one first, take one away; whether the first opens on a double click.
    static func appsMenu(_ i: Int) {
        let list = all
        guard list.indices.contains(i) else { return }
        let menu = NSMenu()
        for (j, app) in list[i].apps.enumerated() {
            let item = NSMenuItem(title: appName(app), action: nil, keyEquivalent: "")
            let icon = NSWorkspace.shared.icon(forFile: app)
            icon.size = NSSize(width: 16, height: 16)
            item.image = icon
            let sub = NSMenu()
            if j > 0 {
                sub.addItem(ClosureMenuItem(title: String(localized: "Make first")) {
                    var l = all
                    let a = l[i].apps.remove(at: j)
                    l[i].apps.insert(a, at: 0)
                    all = l
                })
            }
            sub.addItem(ClosureMenuItem(title: String(localized: "Remove")) {
                var l = all
                l[i].apps.remove(at: j)
                all = l
            })
            item.submenu = sub
            menu.addItem(item)
        }
        menu.addItem(.separator())
        let toggle = ClosureMenuItem(title: String(localized: "The first app opens these files on a double click")) {
            var l = all
            l[i].onOpen.toggle()
            all = l
        }
        toggle.state = list[i].onOpen ? .on : .off
        menu.addItem(toggle)
        menu.addItem(ClosureMenuItem(title: String(localized: "Change extensions…")) {
            var l = all
            guard let exts = askExtensions(l[i].exts.map { "." + $0 }.joined(separator: ", ")) else { return }
            l[i].exts = exts
            all = l
        })
        menu.popUp(positioning: nil, at: NSEvent.mouseLocation, in: nil)
    }

    static func delete(_ i: Int) {
        let a = NSAlert()
        a.messageText = String(localized: "Delete this list?")
        a.informativeText = String(localized: "These files open with macOS’s default app again.")
        a.addButton(withTitle: String(localized: "Delete"))
        a.addButton(withTitle: String(localized: "Cancel"))
        guard a.runModal() == .alertFirstButtonReturn else { return }
        var l = all
        l.remove(at: i)
        all = l
    }
}

extension SettingsContent {
    static func fileTypeRows() -> [SettingsRow] {
        let rules = OpenRules.all
        return [
            .table(
                title: String(localized: "Apps for kinds of files"),
                detail: String(localized: "The first app of a list opens such files on a double click; all of them come first under “Open with”. Only rustyfiles follows these lists."),
                icon: "file-type",
                entries: rules.map { r in
                    let apps = r.apps.isEmpty ? String(localized: "No apps yet: add one.") : r.apps.map(OpenRules.appName).joined(separator: " · ")
                    let note = r.onOpen || r.apps.isEmpty ? apps : apps + " — " + String(localized: "double click left to macOS")
                    return (r.exts.map { "." + $0 }.joined(separator: ", "), note, nil)
                },
                empty: String(localized: "No lists yet"),
                rowActions: [
                    (String(localized: "Add app…"), { OpenRules.addApp($0) }),
                    (String(localized: "Apps…"), { OpenRules.appsMenu($0) }),
                    (String(localized: "Delete"), { OpenRules.delete($0) }),
                ],
                buttons: [(String(localized: "New list…"), { OpenRules.add() })]),
        ]
    }
}

// MARK: - Keyboard shortcuts (settings_view.rs `actions`)

/// The menu bar's shortcuts, each changeable. Overrides are kept by the command's selector
/// and put on the menus whenever they are built.
@MainActor
enum Shortcuts {
    private static let key = "shortcuts"

    struct Combo: Codable, Equatable {
        var key: String
        var mods: UInt

        var flags: NSEvent.ModifierFlags { NSEvent.ModifierFlags(rawValue: mods) }

        var label: String {
            var s = ""
            if flags.contains(.control) { s += "⌃" }
            if flags.contains(.option) { s += "⌥" }
            if flags.contains(.shift) { s += "⇧" }
            if flags.contains(.command) { s += "⌘" }
            return s + Shortcuts.keyName(key)
        }
    }

    static var overrides: [String: Combo] {
        get {
            guard let data = UserDefaults.standard.data(forKey: key) else { return [:] }
            return (try? JSONDecoder().decode([String: Combo].self, from: data)) ?? [:]
        }
        set {
            if let data = try? JSONEncoder().encode(newValue) { UserDefaults.standard.set(data, forKey: key) }
            NSApp.mainMenu = MainMenu.make()
            NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": key])
        }
    }

    nonisolated static func keyName(_ k: String) -> String {
        guard let scalar = k.unicodeScalars.first else { return "" }
        switch Int(scalar.value) {
        case NSF1FunctionKey...NSF12FunctionKey: return "F\(Int(scalar.value) - NSF1FunctionKey + 1)"
        case NSUpArrowFunctionKey: return "↑"
        case NSDownArrowFunctionKey: return "↓"
        case NSLeftArrowFunctionKey: return "←"
        case NSRightArrowFunctionKey: return "→"
        case NSBackspaceCharacter, NSDeleteCharacter: return "⌫"
        case NSDeleteFunctionKey: return "⌦"
        case 13, 3: return "↩"
        case 9: return "⇥"
        case 27: return "⎋"
        case 32: return String(localized: "Space")
        default: return k.uppercased()
        }
    }

    /// Puts the overrides on a freshly made menu bar.
    static func apply(_ bar: NSMenu) {
        let o = overrides
        guard !o.isEmpty else { return }
        for item in allItems(bar) {
            guard let action = item.action, let c = o[NSStringFromSelector(action)] else { continue }
            item.keyEquivalent = c.key
            item.keyEquivalentModifierMask = c.flags
        }
    }

    static func allItems(_ menu: NSMenu) -> [NSMenuItem] {
        menu.items.flatMap { i in [i] + (i.submenu.map(allItems) ?? []) }
    }

    /// The commands that can have a shortcut: every menu command but the system's own.
    static func commands() -> [(selector: String, title: String, menu: String, combo: Combo?)] {
        let bar = MainMenu.make(install: false)
        var out: [(String, String, String, Combo?)] = []
        var seen = Set<String>()
        for top in bar.items {
            guard let sub = top.submenu else { continue }
            for item in allItems(sub) where item.action != nil && !item.isSeparatorItem && item.submenu == nil {
                let sel = NSStringFromSelector(item.action!)
                guard seen.insert(sel).inserted, !["terminate:", "hide:", "hideOtherApplications:", "unhideAllApplications:", "performMiniaturize:", "performZoom:", "orderFrontStandardAboutPanel:"].contains(sel)
                else { continue }
                let combo = item.keyEquivalent.isEmpty ? nil : Combo(key: item.keyEquivalent, mods: item.keyEquivalentModifierMask.rawValue)
                out.append((sel, item.title, top.title, combo))
            }
        }
        return out
    }

    static func change(_ selector: String, title: String) {
        guard let combo = ShortcutRecorder.record(title) else { return }
        // the same keys on another command: say so, and move them here if wanted
        if let other = commands().first(where: { $0.selector != selector && $0.combo == combo }) {
            let a = NSAlert()
            a.messageText = String(localized: "Same shortcut: \(other.title)")
            a.informativeText = String(localized: "\(combo.label) is already “\(other.title)”. Use it here and take it from there?")
            a.addButton(withTitle: String(localized: "Use it here"))
            a.addButton(withTitle: String(localized: "Cancel"))
            guard a.runModal() == .alertFirstButtonReturn else { return }
            var o = overrides
            o[other.selector] = Combo(key: "", mods: 0)
            o[selector] = combo
            overrides = o
            return
        }
        var o = overrides
        o[selector] = combo
        overrides = o
    }

    static func clear(_ selector: String) {
        var o = overrides
        o[selector] = Combo(key: "", mods: 0)
        overrides = o
    }

    static func reset(_ selector: String) {
        var o = overrides
        o.removeValue(forKey: selector)
        overrides = o
    }
}

/// "Press the keys…": an alert that takes the next key press as the shortcut.
@MainActor
final class ShortcutRecorder: NSView {
    private(set) var combo: Shortcuts.Combo?
    private let label = NSTextField(labelWithString: String(localized: "Press the keys…"))

    override init(frame: NSRect) {
        super.init(frame: NSRect(x: 0, y: 0, width: 260, height: 40))
        label.font = NSFont.systemFont(ofSize: 18, weight: .medium)
        label.alignment = .center
        label.frame = bounds
        addSubview(label)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var acceptsFirstResponder: Bool { true }

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        take(event)
        return true
    }

    override func keyDown(with event: NSEvent) { take(event) }

    private func take(_ event: NSEvent) {
        let mods = event.modifierFlags.intersection([.command, .option, .control, .shift])
        guard let chars = event.charactersIgnoringModifiers?.lowercased(), !chars.isEmpty else { return }
        // a plain letter would type in every text field: one modifier at least, unless F-key
        let isFn = chars.unicodeScalars.first.map { Int($0.value) >= NSF1FunctionKey && Int($0.value) <= NSF35FunctionKey } ?? false
        guard !mods.isEmpty || isFn else {
            label.stringValue = String(localized: "Add ⌘, ⌥ or ⌃")
            return
        }
        combo = Shortcuts.Combo(key: chars, mods: mods.rawValue)
        label.stringValue = combo!.label
    }

    static func record(_ title: String) -> Shortcuts.Combo? {
        let a = NSAlert()
        a.messageText = String(localized: "New shortcut for “\(title)”")
        let rec = ShortcutRecorder()
        a.accessoryView = rec
        a.addButton(withTitle: String(localized: "Save"))
        a.addButton(withTitle: String(localized: "Cancel"))
        a.window.initialFirstResponder = rec
        // the buttons must not take Return / Escape while recording: they are clicked
        a.buttons.forEach { $0.keyEquivalent = "" }
        guard a.runModal() == .alertFirstButtonReturn else { return nil }
        return rec.combo
    }
}

extension SettingsContent {
    static func shortcutRows() -> [SettingsRow] {
        let commands = Shortcuts.commands()
        let o = Shortcuts.overrides
        var rows: [SettingsRow] = []
        var lastMenu = ""
        var group: [(String, String, String, Shortcuts.Combo?)] = []
        func flush() {
            guard !group.isEmpty else { return }
            let g = group
            rows.append(.table(
                title: lastMenu, detail: nil, icon: nil,
                entries: g.map { ($0.1, $0.3.map(\.label) ?? String(localized: "None") + (o[$0.0] != nil ? " · " + String(localized: "changed") : ""), nil) },
                empty: "",
                rowActions: [
                    (String(localized: "Change…"), { Shortcuts.change(g[$0].0, title: g[$0].1) }),
                    (String(localized: "Clear"), { Shortcuts.clear(g[$0].0) }),
                    (String(localized: "Reset"), { Shortcuts.reset(g[$0].0) }),
                ],
                buttons: []))
            group = []
        }
        for c in commands {
            if c.menu != lastMenu {
                flush()
                lastMenu = c.menu
            }
            group.append((c.selector, c.title, c.menu, c.combo))
        }
        flush()
        return [
            .card(
                title: String(localized: "Keyboard shortcuts"),
                detail: String(localized: "Every menu command can have keys of its own. A shortcut changed here shows in the menu bar at once."),
                icon: "keyboard",
                control: .buttons([(String(localized: "Reset all"), { Shortcuts.overrides = [:] })])),
        ] + rows
    }
}
