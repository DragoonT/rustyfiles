import AppKit

/// The Appearance page's settings beyond theme, material and accent (settings.rs).
extension AppSettings {
    private static var d: UserDefaults { .standard }

    private static func flag(_ key: String, _ fallback: Bool) -> Bool {
        d.object(forKey: key) == nil ? fallback : d.bool(forKey: key)
    }

    private static func store(_ value: Any?, _ key: String) {
        if let value { d.set(value, forKey: key) } else { d.removeObject(forKey: key) }
        Palette.invalidate()
        NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": key])
    }

    /// Short movements: lenses gliding, switches sliding. Off: everything jumps into place.
    static var animations: Bool {
        get { flag("animations", true) }
        set { store(newValue, "animations") }
    }

    /// Scroll bars in the file list (off: scrolling still works, nothing is drawn).
    static var showScrollBars: Bool {
        get { flag("showScrollBars", true) }
        set { store(newValue, "showScrollBars") }
    }

    /// The file list's own background colour ("#RRGGBB"; "" the theme's).
    static var listBackground: String {
        get { d.string(forKey: "listBackground") ?? "" }
        set { store(newValue, "listBackground") }
    }

    /// A picture behind the files, and how much of it shows (0…100).
    static var listImage: String {
        get { d.string(forKey: "listImage") ?? "" }
        set { store(newValue.isEmpty ? nil : newValue, "listImage") }
    }

    static var listImageVisibility: Int {
        get { d.object(forKey: "listImageVisibility") == nil ? 25 : d.integer(forKey: "listImageVisibility") }
        set { store(min(max(newValue, 0), 100), "listImageVisibility") }
    }

    /// The interface's font family ("" the system's).
    static var fontFamily: String {
        get { d.string(forKey: "fontFamily") ?? "" }
        set {
            store(newValue.isEmpty ? nil : newValue, "fontFamily")
            Metrics.fontsChanged()
        }
    }

    static var showCommandBar: Bool {
        get { flag("showCommandBar", true) }
        set { store(newValue, "showCommandBar") }
    }

    static var showStatusBar: Bool {
        get { flag("showStatusBar", true) }
        set { store(newValue, "showStatusBar") }
    }

    /// The preview pane at the right of the list or under it.
    static var previewAtBottom: Bool {
        get { d.string(forKey: "previewPosition") == "bottom" }
        set { store(newValue ? "bottom" : "right", "previewPosition") }
    }

    // MARK: sidebar and Home sections

    enum SidebarSection: String, CaseIterable {
        case libraries, pinned, tree, tags, drives

        var title: String {
            switch self {
            case .libraries: String(localized: "Applications and Library")
            case .pinned: String(localized: "Pinned")
            case .tree: String(localized: "Folder tree")
            case .tags: String(localized: "Tags")
            case .drives: String(localized: "Drives")
            }
        }
    }

    /// The folder tree is off unless asked for, as on Windows.
    static func showsSection(_ s: SidebarSection) -> Bool {
        if s == .tree { return d.bool(forKey: "sidebarTree") }
        return !(d.stringArray(forKey: "sidebarHidden") ?? []).contains(s.rawValue)
    }

    /// The tree opens down to the folder shown.
    static var treeFollows: Bool {
        get { d.object(forKey: "treeFollows") == nil ? true : d.bool(forKey: "treeFollows") }
        set { store(newValue, "treeFollows") }
    }

    static func setShowsSection(_ s: SidebarSection, _ on: Bool) {
        if s == .tree { return store(on, "sidebarTree") }
        var hidden = Set(d.stringArray(forKey: "sidebarHidden") ?? [])
        if on { hidden.remove(s.rawValue) } else { hidden.insert(s.rawValue) }
        store(Array(hidden).sorted(), "sidebarHidden")
    }

    enum HomeSection: String, CaseIterable {
        case quick, drives, tags, recent

        var title: String {
            switch self {
            case .quick: String(localized: "Quick access")
            case .drives: String(localized: "Drives")
            case .tags: String(localized: "Tags")
            case .recent: String(localized: "Recent")
            }
        }
    }

    static func showsHome(_ s: HomeSection) -> Bool { !(d.stringArray(forKey: "homeHidden") ?? []).contains(s.rawValue) }

    static func setShowsHome(_ s: HomeSection, _ on: Bool) {
        var hidden = Set(d.stringArray(forKey: "homeHidden") ?? [])
        if on { hidden.remove(s.rawValue) } else { hidden.insert(s.rawValue) }
        store(Array(hidden).sorted(), "homeHidden")
    }

    // MARK: command bar buttons

    /// The command bar's buttons (settings_view.rs "Toolbar buttons"); the last four are off
    /// unless asked for.
    enum ToolbarButton: String, CaseIterable {
        case new, newFolder, cut, copy, paste, rename, share, delete, properties, terminal, hidden, refresh, filter, sort, view, preview

        var title: String {
            switch self {
            case .new: String(localized: "New")
            case .newFolder: String(localized: "New folder")
            case .cut: String(localized: "Cut")
            case .copy: String(localized: "Copy")
            case .paste: String(localized: "Paste")
            case .rename: String(localized: "Rename")
            case .share: String(localized: "Share")
            case .delete: String(localized: "Delete")
            case .properties: String(localized: "Properties")
            case .terminal: String(localized: "Open in Terminal")
            case .hidden: String(localized: "Show or hide hidden items")
            case .refresh: String(localized: "Refresh")
            case .filter: String(localized: "Filter")
            case .sort: String(localized: "Sort")
            case .view: String(localized: "View")
            case .preview: String(localized: "Preview pane")
            }
        }

        var optional: Bool { [.newFolder, .terminal, .hidden, .refresh].contains(self) }
    }

    static func showsButton(_ b: ToolbarButton) -> Bool {
        let list = d.stringArray(forKey: b.optional ? "toolbarAdded" : "toolbarHidden") ?? []
        return b.optional ? list.contains(b.rawValue) : !list.contains(b.rawValue)
    }

    static func setShowsButton(_ b: ToolbarButton, _ on: Bool) {
        let key = b.optional ? "toolbarAdded" : "toolbarHidden"
        var set = Set(d.stringArray(forKey: key) ?? [])
        if on == b.optional { set.insert(b.rawValue) } else { set.remove(b.rawValue) }
        store(Array(set).sorted(), key)
    }
}

extension SettingsContent {
    static func appearanceExtraRows() -> [SettingsRow] {
        var rows: [SettingsRow] = [
            .card(
                title: String(localized: "Animations"), detail: String(localized: "Selections glide and switches slide. Nothing waits for them."), icon: "sparkles",
                control: .toggle(get: { AppSettings.animations }, set: { AppSettings.animations = $0 })),
            .card(
                title: String(localized: "Show scroll bars"), detail: nil, icon: "move-vertical",
                control: .toggle(get: { AppSettings.showScrollBars }, set: { AppSettings.showScrollBars = $0 })),
            .card(
                title: String(localized: "Font"), detail: nil, icon: "type",
                control: .choice(
                    options: [("", String(localized: "System"))] + NSFontManager.shared.availableFontFamilies.filter { !$0.hasPrefix(".") }.map { ($0, $0) },
                    get: { AppSettings.fontFamily }, set: { AppSettings.fontFamily = $0 })),
            .section(String(localized: "File list background")),
            .card(
                title: String(localized: "Background colour"), detail: AppSettings.listBackground.isEmpty ? nil : AppSettings.listBackground, icon: "paint-bucket",
                control: .buttons([
                    (String(localized: "Choose…"), { ColourChooser.choose(AppSettings.listBackground) { AppSettings.listBackground = $0 } }),
                    (String(localized: "Default"), { AppSettings.listBackground = "" }),
                ])),
            .card(
                title: String(localized: "Background picture"),
                detail: AppSettings.listImage.isEmpty ? String(localized: "None") : (AppSettings.listImage as NSString).lastPathComponent, icon: "image",
                control: .buttons([
                    (String(localized: "Browse…"), {
                        let panel = NSOpenPanel()
                        panel.allowedContentTypes = [.image]
                        if panel.runModal() == .OK, let u = panel.url { AppSettings.listImage = u.path }
                    }),
                ] + (AppSettings.listImage.isEmpty ? [] : [(String(localized: "Remove"), { AppSettings.listImage = "" })]))),
        ]
        if !AppSettings.listImage.isEmpty {
            rows.append(.card(
                title: String(localized: "Picture visibility"), detail: nil, icon: "eye",
                control: .slider(get: { AppSettings.listImageVisibility }, set: { AppSettings.listImageVisibility = $0 })))
        }
        rows += [
            .section(String(localized: "Bars and panes")),
            .card(
                title: String(localized: "Show the command bar"), detail: nil, icon: "panel-top",
                control: .toggle(get: { AppSettings.showCommandBar }, set: { AppSettings.showCommandBar = $0 })),
            .card(
                title: String(localized: "Show the status bar"), detail: nil, icon: "panel-bottom",
                control: .toggle(get: { AppSettings.showStatusBar }, set: { AppSettings.showStatusBar = $0 })),
            .card(
                title: String(localized: "Preview pane position"), detail: nil, icon: "panel-right",
                control: .choice(
                    options: [("right", String(localized: "At the right")), ("bottom", String(localized: "At the bottom"))],
                    get: { AppSettings.previewAtBottom ? "bottom" : "right" }, set: { AppSettings.previewAtBottom = $0 == "bottom" })),
            .section(String(localized: "Command bar buttons")),
        ]
        rows += AppSettings.ToolbarButton.allCases.map { b in
            .card(title: b.title, detail: nil, icon: nil, control: .toggle(get: { AppSettings.showsButton(b) }, set: { AppSettings.setShowsButton(b, $0) }))
        }
        rows.append(.section(String(localized: "Sidebar sections")))
        rows += AppSettings.SidebarSection.allCases.filter { $0 != .tags || TagsSupport.ready }.map { s in
            .card(title: s.title, detail: nil, icon: nil, control: .toggle(get: { AppSettings.showsSection(s) }, set: { AppSettings.setShowsSection(s, $0) }))
        }
        if AppSettings.showsSection(.tree) {
            rows.append(.card(
                title: String(localized: "Follow the open folder"), detail: String(localized: "The folder tree opens down to the folder shown."), icon: nil,
                control: .toggle(get: { AppSettings.treeFollows }, set: { AppSettings.treeFollows = $0 })))
        }
        rows.append(.section(String(localized: "Home sections")))
        rows += AppSettings.HomeSection.allCases.filter { $0 != .tags || TagsSupport.ready }.map { s in
            .card(title: s.title, detail: nil, icon: nil, control: .toggle(get: { AppSettings.showsHome(s) }, set: { AppSettings.setShowsHome(s, $0) }))
        }
        return rows
    }
}

/// The system colour panel, for a colour setting.
@MainActor
final class ColourChooser: NSObject {
    private static var current: ColourChooser?
    private let done: (String) -> Void

    private init(_ done: @escaping (String) -> Void) { self.done = done }

    static func choose(_ start: String, _ done: @escaping (String) -> Void) {
        let c = ColourChooser(done)
        current = c
        let panel = NSColorPanel.shared
        panel.showsAlpha = false
        panel.color = Palette.parse(start) ?? .windowBackgroundColor
        panel.setTarget(c)
        panel.setAction(#selector(changed(_:)))
        panel.orderFront(nil)
    }

    @objc private func changed(_ panel: NSColorPanel) {
        guard let c = panel.color.usingColorSpace(.sRGB) else { return }
        done(String(format: "#%02X%02X%02X", Int(c.redComponent * 255), Int(c.greenComponent * 255), Int(c.blueComponent * 255)))
    }
}

/// Whether the Tags sections exist yet (the sidebar's and Home's).
enum TagsSupport {
    static let ready = true
}
