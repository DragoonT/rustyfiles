import AppKit

// MARK: - model

/// The Settings pages the Mac version has, in the Windows app's order (settings_view.rs).
enum SettingsPage: CaseIterable {
    /// Marked "Experimental" in the page list, as on Windows.
    var isExperimental: Bool { self == .ferry }

    case general, appearance, layout, files, fileTypes, actions, tags, ferry, dev, advanced, speed, organizer, about

    var title: String {
        switch self {
        case .general: String(localized: "General")
        case .appearance: String(localized: "Appearance")
        case .layout: String(localized: "Layout")
        case .files: String(localized: "Files and folders")
        case .tags: String(localized: "Tags")
        case .fileTypes: String(localized: "File types")
        case .actions: String(localized: "Keyboard shortcuts")
        case .dev: String(localized: "Developer tools")
        case .advanced: String(localized: "Advanced")
        case .speed: String(localized: "Speed test")
        case .organizer: String(localized: "File organizer")
        case .ferry: "Ferry"
        case .about: String(localized: "About")
        }
    }

    var icon: String {
        switch self {
        case .general: "settings"
        case .appearance: "palette"
        case .layout: "layout-grid"
        case .files: "folder"
        case .tags: "tag"
        case .fileTypes: "file-type"
        case .actions: "keyboard"
        case .dev: "square-terminal"
        case .advanced: "wrench"
        case .speed: "gauge"
        case .organizer: "wand-sparkles"
        case .ferry: "send"
        case .about: "info"
        }
    }
}

/// One line of a settings page.
@MainActor
enum SettingsRow {
    case section(String)
    case card(title: String, detail: String?, icon: String?, control: SettingsControl)
    /// A card with a list under it (startup pages, folder aliases, rules…): each entry with a
    /// Remove button, the card's own buttons at its right.
    case list(title: String, detail: String?, icon: String?, entries: [(String, String?)], empty: String, remove: ((Int) -> Void)?, buttons: [(String, () -> Void)])
    /// The same with a colour dot per entry and buttons of its own on each row.
    case table(title: String, detail: String?, icon: String?, entries: [(String, String?, NSColor?)], empty: String, rowActions: [(String, (Int) -> Void)], buttons: [(String, () -> Void)])

    var searchText: String {
        switch self {
        case .section: ""
        case .card(let title, let detail, _, _): (title + " " + (detail ?? "")).lowercased()
        case .list(let title, let detail, _, _, _, _, _): (title + " " + (detail ?? "")).lowercased()
        case .table(let title, let detail, _, _, _, _, _): (title + " " + (detail ?? "")).lowercased()
        }
    }
}

@MainActor
enum SettingsControl {
    case toggle(get: () -> Bool, set: (Bool) -> Void)
    case choice(options: [(String, String)], get: () -> String, set: (String) -> Void, examples: ((String) -> String)? = nil)
    case slider(get: () -> Int, set: (Int) -> Void)
    case accent
    case buttons([(String, () -> Void)])
    case value(String)
    case none
}

@MainActor
enum SettingsContent {
    static func rows(_ page: SettingsPage) -> [SettingsRow] {
        switch page {
        case .general:
            return [
                .card(
                    title: String(localized: "Language"),
                    detail: String(localized: "Takes effect when rustyfiles opens again."),
                    icon: "languages",
                    control: .choice(
                        options: [("system", String(localized: "System")), ("en", "English"), ("tr", "Türkçe")],
                        get: { AppSettings.language },
                        set: { AppSettings.language = $0 })),
                .card(
                    title: String(localized: "Date format"),
                    detail: nil,
                    icon: "calendar",
                    control: .choice(
                        options: [("app", String(localized: "Application")), ("system", String(localized: "System")), ("universal", String(localized: "Universal"))],
                        get: { AppSettings.dateStyle.rawValue },
                        set: { AppSettings.dateStyle = AppSettings.DateStyle(rawValue: $0) ?? .app },
                        examples: { ItemDisplay.example(AppSettings.DateStyle(rawValue: $0) ?? .app) })),
            ] + startupRows() + tabRows() + menuRows()
        case .appearance:
            return [
                .card(
                    title: String(localized: "Theme"), detail: nil, icon: "sun-moon",
                    control: .choice(
                        options: [
                            ("system", String(localized: "System")), ("light", String(localized: "Light")), ("dark", String(localized: "Dark")),
                            ("black", String(localized: "Black (OLED)")),
                        ],
                        get: { AppSettings.theme.rawValue },
                        set: { AppSettings.theme = AppSettings.Theme(rawValue: $0) ?? .system })),
                .card(
                    title: String(localized: "Window material"),
                    detail: String(localized: "Liquid Glass: the screen behind shows through glass. Translucent: the window takes the desktop’s tone. Solid: no transparency, like Finder."),
                    icon: "app-window",
                    control: .choice(
                        options: [("glass", String(localized: "Liquid Glass")), ("finder", String(localized: "Translucent")), ("solid", String(localized: "Solid"))],
                        get: { AppSettings.material.rawValue },
                        set: { AppSettings.material = AppSettings.Material(rawValue: $0) ?? .glass })),
            ] + (AppSettings.isGlass ? [
                .card(
                    title: String(localized: "Surface wash"),
                    detail: String(localized: "How strongly the app’s own surfaces cover the glass. At the far left there is no cover and text can be hard to read."),
                    icon: "sparkles",
                    control: .slider(get: { AppSettings.surfaceWash }, set: { AppSettings.surfaceWash = $0 })),
            ] as [SettingsRow] : []) + [
                .card(title: String(localized: "Accent colour"), detail: nil, icon: "paintbrush", control: .accent),
            ] + appearanceExtraRows()
        case .layout:
            return layoutRows()
        case .files:
            return [
                .section(String(localized: "View")),
                .card(
                    title: String(localized: "Show hidden items"), detail: String(localized: "Items whose name starts with a dot, and items hidden with “chflags hidden”."),
                    icon: "eye-off",
                    control: .toggle(get: { AppSettings.showHidden }, set: { AppSettings.showHidden = $0 })),
                .card(
                    title: String(localized: "Show file extensions"), detail: nil, icon: "file-type",
                    control: .toggle(get: { AppSettings.showExtensions }, set: { AppSettings.showExtensions = $0 })),
                .card(
                    title: String(localized: "Show selection check boxes"), detail: nil, icon: "square-check",
                    control: .toggle(get: { AppSettings.selectionBoxes }, set: { AppSettings.selectionBoxes = $0 })),
                .card(
                    title: String(localized: "Select on hover"), detail: String(localized: "Resting the pointer on an item for a moment selects it."), icon: "mouse-pointer",
                    control: .toggle(get: { AppSettings.selectOnHover }, set: { AppSettings.selectOnHover = $0 })),
                .card(
                    title: String(localized: "Show thumbnails"), detail: String(localized: "Pictures of the files themselves in the Tiles layout."), icon: "image",
                    control: .toggle(get: { AppSettings.showThumbnails }, set: { AppSettings.showThumbnails = $0 })),
                .section(String(localized: "Behaviours")),
                .card(
                    title: String(localized: "Size format"), detail: String(localized: "Binary: 1 KB = 1024 bytes. Decimal: 1 KB = 1000 bytes, as macOS counts."),
                    icon: "hard-drive",
                    control: .choice(
                        options: [("binary", String(localized: "Binary")), ("decimal", String(localized: "Decimal"))],
                        get: { AppSettings.decimalSizes ? "decimal" : "binary" },
                        set: { AppSettings.decimalSizes = $0 == "decimal" })),
            ] + fileBehaviourRows()
        case .tags:
            return tagRows()
        case .fileTypes:
            return fileTypeRows()
        case .actions:
            return shortcutRows()
        case .dev:
            return devRows()
        case .advanced:
            return advancedRows()
        case .speed:
            return speedRows()
        case .organizer:
            return organizerRows()
        case .ferry:
            return FerryPage.rows()
        case .about:
            let info = Bundle.main.infoDictionary
            let version = info?["CFBundleShortVersionString"] as? String ?? coreVersion()
            func open(_ s: String) -> () -> Void { { if let u = URL(string: s) { NSWorkspace.shared.open(u) } } }
            return [
                .card(title: "rustyfiles", detail: String(localized: "A file manager that never freezes. Written in Rust."), icon: "info", control: .value(String(localized: "Version \(version)"))),
                .section(String(localized: "Community")),
                .card(
                    title: String(localized: "Links"), detail: nil, icon: "globe",
                    control: .buttons([
                        ("GitHub", open("https://github.com/gelenmedya/rustyfiles")),
                        (String(localized: "Website"), open("https://rustyfiles.com")),
                        ("Discord", open("https://discord.gg/K2banZ36zJ")),
                    ])),
            ]
        }
    }
}

// MARK: - General and Files rows

extension SettingsContent {
    static func startupRows() -> [SettingsRow] {
        var rows: [SettingsRow] = [
            .section(String(localized: "Startup")),
            .card(
                title: String(localized: "When rustyfiles starts"), detail: nil, icon: "power",
                control: .choice(
                    options: [
                        ("resume", String(localized: "Continue where you left off")), ("home", String(localized: "Open a new tab")),
                        ("pages", String(localized: "Open specific pages")),
                    ],
                    get: { AppSettings.startup.rawValue },
                    set: { AppSettings.startup = AppSettings.Startup(rawValue: $0) ?? .home })),
        ]
        if AppSettings.startup == .pages {
            let pages = AppSettings.startupPages
            rows.append(.list(
                title: String(localized: "Specific pages"), detail: nil, icon: "list",
                entries: pages.map { p in
                    p == BrowserTab.homeURL.absoluteString ? (String(localized: "Home"), nil) : (FileManager.default.displayName(atPath: p), p)
                },
                empty: String(localized: "Home opens when the list is empty."),
                remove: { i in AppSettings.startupPages.remove(at: i) },
                buttons: [
                    (String(localized: "Add folder…"), {
                        let panel = NSOpenPanel()
                        panel.canChooseDirectories = true
                        panel.canChooseFiles = false
                        panel.allowsMultipleSelection = true
                        if panel.runModal() == .OK { AppSettings.startupPages += panel.urls.map(\.path) }
                    }),
                    (String(localized: "Use current tabs"), {
                        guard let w = AppDelegate.shared.frontController else { return }
                        AppSettings.startupPages = w.tabAddresses.filter { !$0.hasPrefix("rustyfiles:settings") }
                    }),
                ]))
        }
        return rows
    }

    static func tabRows() -> [SettingsRow] {
        [
            .section(String(localized: "Tabs and windows")),
            .card(
                title: String(localized: "A new tab opens"), detail: nil, icon: "square-plus",
                control: .choice(
                    options: [("home", String(localized: "Home")), ("current", String(localized: "The current folder"))],
                    get: { AppSettings.newTabOpens }, set: { AppSettings.newTabOpens = $0 })),
            .card(
                title: String(localized: "Always switch to a newly opened tab"), detail: nil, icon: "arrow-right-left",
                control: .toggle(get: { AppSettings.switchToNewTab }, set: { AppSettings.switchToNewTab = $0 })),
            .card(
                title: String(localized: "Folders opened from other apps open in a new window"),
                detail: String(localized: "Off: they open as a tab in the front window."), icon: "app-window",
                control: .toggle(get: { AppSettings.externalInNewWindow }, set: { AppSettings.externalInNewWindow = $0 })),
        ]
    }

    static func menuRows() -> [SettingsRow] {
        [.section(String(localized: "Right-click menu"))] + AppSettings.MenuCommand.allCases.map { c in
            .card(
                title: c.title, detail: nil, icon: nil,
                control: .toggle(get: { AppSettings.shows(c) }, set: { AppSettings.setShows(c, $0) }))
        }
    }

    static func fileBehaviourRows() -> [SettingsRow] {
        let aliases = AppSettings.folderAliases.sorted { $0.key < $1.key }
        return [
            .card(
                title: String(localized: "Open items with a single click"), detail: nil, icon: "mouse-pointer-click",
                control: .choice(
                    options: [("never", String(localized: "Never")), ("folders", String(localized: "Folders only")), ("always", String(localized: "Always"))],
                    get: { AppSettings.singleClick.rawValue },
                    set: { AppSettings.singleClick = AppSettings.SingleClick(rawValue: $0) ?? .never })),
            .card(
                title: String(localized: "Open folders in a new tab"), detail: nil, icon: "square-plus",
                control: .toggle(get: { AppSettings.foldersInNewTab }, set: { AppSettings.foldersInNewTab = $0 })),
            .card(
                title: String(localized: "Ask before deleting"), detail: nil, icon: "trash",
                control: .choice(
                    options: [
                        ("never", String(localized: "Never")), ("permanent", String(localized: "Only when deleting for good")),
                        ("always", String(localized: "Always")),
                    ],
                    get: { AppSettings.confirmDelete.rawValue },
                    set: { AppSettings.confirmDelete = AppSettings.ConfirmDelete(rawValue: $0) ?? .permanent })),
            .card(
                title: String(localized: "Warn when an extension changes"), detail: nil, icon: "triangle-alert",
                control: .toggle(get: { AppSettings.warnExtensionChange }, set: { AppSettings.warnExtensionChange = $0 })),
            .card(
                title: String(localized: "Double-click empty space to go up"), detail: nil, icon: "arrow-up",
                control: .toggle(get: { AppSettings.doubleClickEmptyGoesUp }, set: { AppSettings.doubleClickEmptyGoesUp = $0 })),
            .card(
                title: String(localized: "Select the folder you came from when going up"), detail: nil, icon: "corner-left-up",
                control: .toggle(get: { AppSettings.upSelectsPrevious }, set: { AppSettings.upSelectsPrevious = $0 })),
            .card(
                title: String(localized: "Run programs from the address bar"),
                detail: String(localized: "Text that is not a place is looked for as an app (Terminal, Visual Studio Code…) or a program (code, make…) and started in the open folder."),
                icon: "square-terminal",
                control: .toggle(get: { AppSettings.addressCommands }, set: { AppSettings.addressCommands = $0 })),
            .section(String(localized: "Archives")),
            .card(
                title: String(localized: "Open archives like folders"), detail: String(localized: "Off: archives open in the app macOS chooses for them."), icon: "archive",
                control: .toggle(get: { AppSettings.archivesAsFolders }, set: { AppSettings.archivesAsFolders = $0 })),
            .card(
                title: String(localized: "Open the folder after extracting"), detail: String(localized: "The new folder opens in a new tab."), icon: "folder-output",
                control: .toggle(get: { AppSettings.openAfterExtract }, set: { AppSettings.openAfterExtract = $0 })),
            .section(String(localized: "Sizes")),
            .card(
                title: String(localized: "Calculate folder sizes"), detail: String(localized: "The Size column shows each folder’s total, worked out in the background."),
                icon: "calculator",
                control: .toggle(get: { AppSettings.autoFolderSizes }, set: { AppSettings.autoFolderSizes = $0 })),
            .card(
                title: String(localized: "Show the size on disk for folders"), detail: nil, icon: "hard-drive",
                control: .toggle(get: { AppSettings.folderSizeOnDisk }, set: { AppSettings.folderSizeOnDisk = $0 })),
            .section(String(localized: "Folder aliases")),
            .list(
                title: String(localized: "Aliases"), detail: String(localized: "Type @name in the address bar to go to the folder."), icon: "at-sign",
                entries: aliases.map { ("@" + $0.key, $0.value) },
                empty: String(localized: "No aliases yet"),
                remove: { i in
                    var a = AppSettings.folderAliases
                    a.removeValue(forKey: aliases[i].key)
                    AppSettings.folderAliases = a
                },
                buttons: [(String(localized: "Add…"), { FolderAliasPrompt.add() })]),
        ]
    }
}

/// Asks for a folder and a name for it.
@MainActor
enum FolderAliasPrompt {
    static func add() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.prompt = String(localized: "Choose")
        guard panel.runModal() == .OK, let folder = panel.url else { return }
        let alert = NSAlert()
        alert.messageText = String(localized: "Name for “\(folder.lastPathComponent)”")
        alert.informativeText = String(localized: "You’ll type it after @ in the address bar.")
        let field = NSTextField(string: folder.lastPathComponent.lowercased().replacingOccurrences(of: " ", with: "-"))
        field.frame = NSRect(x: 0, y: 0, width: 240, height: 24)
        alert.accessoryView = field
        alert.addButton(withTitle: String(localized: "Add"))
        alert.addButton(withTitle: String(localized: "Cancel"))
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let name = field.stringValue.trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "@"))
        guard !name.isEmpty else { return }
        var a = AppSettings.folderAliases
        a[name] = folder.path
        AppSettings.folderAliases = a
    }
}

// MARK: - controls (the "liquid" look, liquid.rs)

/// The switch: 46×20, a capsule thumb 0.58×0.9 of the track, sliding in 180 ms.
@MainActor
final class LiquidSwitch: HoverView {
    var isOn: Bool { didSet { if isOn != oldValue { animate() } } }
    var onChange: ((Bool) -> Void)?
    private var progress: CGFloat
    private var held = false { didSet { needsDisplay = true } }
    private var timer: Timer?

    init(isOn: Bool) {
        self.isOn = isOn
        progress = isOn ? 1 : 0
        super.init(frame: NSRect(x: 0, y: 0, width: 46, height: 20))
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var mouseDownCanMoveWindow: Bool { false }

    private func animate() {
        timer?.invalidate()
        guard AppSettings.animations else {
            progress = isOn ? 1 : 0
            needsDisplay = true
            return
        }
        let start = progress, target: CGFloat = isOn ? 1 : 0, began = Date()
        timer = Timer.scheduledTimer(withTimeInterval: 1.0 / 120, repeats: true) { [weak self] t in
            MainActor.assumeIsolated {
                guard let self else { return t.invalidate() }
                let x = min(Date().timeIntervalSince(began) / 0.18, 1)
                let eased = 1 - pow(1 - x, 3)
                self.progress = start + (target - start) * eased
                self.needsDisplay = true
                if x >= 1 { t.invalidate() }
            }
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let track = bounds
        let off = p.dark ? NSColor(white: 1, alpha: 0.16) : NSColor(white: 0, alpha: 0.16)
        let fill = off.blended(withFraction: progress, of: p.accent) ?? p.accent
        Draw.roundedRect(track, radius: track.height / 2, fill: fill)
        let tw = track.width * 0.58, th = track.height * 0.9
        let x = 1 + (track.width - 2 - tw) * progress
        var thumb = NSRect(x: x, y: (track.height - th) / 2, width: tw, height: th)
        if held { thumb = thumb.insetBy(dx: -tw * 0.15, dy: -th * 0.15) }
        let shadow = NSShadow()
        shadow.shadowOffset = NSSize(width: 0, height: held ? -3 : -1)
        shadow.shadowBlurRadius = held ? 10 : 2
        shadow.shadowColor = NSColor(white: 0, alpha: held ? 0.28 : 0.18)
        NSGraphicsContext.saveGraphicsState()
        shadow.set()
        Draw.roundedRect(thumb, radius: thumb.height / 2, fill: NSColor(white: 1, alpha: held ? 0.55 : 1))
        NSGraphicsContext.restoreGraphicsState()
        if held {
            NSColor(white: 1, alpha: 0.9).setStroke()
            let rim = NSBezierPath(roundedRect: thumb.insetBy(dx: 0.5, dy: 0.5), xRadius: thumb.height / 2, yRadius: thumb.height / 2)
            rim.lineWidth = 1
            rim.stroke()
        }
    }

    override func mouseDown(with event: NSEvent) { held = true }

    override func mouseUp(with event: NSEvent) {
        held = false
        guard bounds.insetBy(dx: -6, dy: -6).contains(convert(event.locationInWindow, from: nil)) else { return }
        isOn.toggle()
        onChange?(isOn)
    }
}

/// The dropdown: a capsule 32 high, 180…320 wide, label and ▾; the options in a menu.
@MainActor
final class LiquidDropdown: HoverView {
    var options: [(String, String)]
    var value: String { didSet { needsDisplay = true } }
    var onChange: ((String) -> Void)?
    /// An example for each option, shown in the menu beside its name.
    var examples: ((String) -> String)?

    init(options: [(String, String)], value: String) {
        self.options = options
        self.value = value
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var mouseDownCanMoveWindow: Bool { false }

    private var label: String { options.first { $0.0 == value }?.1 ?? value }

    override var intrinsicContentSize: NSSize {
        let widest = options.map { Draw.textWidth($0.1, font: Metrics.textSm) }.max() ?? 0
        return NSSize(width: min(max(14 + widest + 8 + 14 + 14, 180), 320), height: 32)
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let fill = p.dark ? NSColor(white: 1, alpha: hovered ? 0.15 : 0.10) : NSColor(white: 0, alpha: hovered ? 0.10 : 0.06)
        Draw.roundedRect(bounds, radius: bounds.height / 2, fill: fill)
        Draw.text(label, in: NSRect(x: 14, y: 0, width: bounds.width - 14 - 8 - 14 - 14, height: bounds.height), font: Metrics.textSm, color: p.text)
        Draw.icon("chevron-down", in: NSRect(x: bounds.width - 14 - 14, y: 0, width: 14, height: bounds.height), size: 14, color: p.textSecondary)
    }

    override func mouseDown(with event: NSEvent) {
        let m = NSMenu()
        for (key, title) in options {
            let i = ClosureMenuItem(title: title) { [weak self] in
                self?.value = key
                self?.onChange?(key)
            }
            i.state = key == value ? .on : .off
            if let examples {
                let t = NSMutableAttributedString(string: title, attributes: [.font: NSFont.menuFont(ofSize: 0)])
                t.append(NSAttributedString(
                    string: "   " + examples(key),
                    attributes: [.font: NSFont.menuFont(ofSize: 0), .foregroundColor: NSColor.secondaryLabelColor]))
                i.attributedTitle = t
            }
            m.addItem(i)
        }
        m.minimumWidth = bounds.width
        m.popUp(positioning: m.items.first { $0.state == .on }, at: NSPoint(x: 0, y: bounds.height - 4), in: self)
    }
}

/// The slider: 220×24, a 4 px track (filled part in the accent), a white capsule thumb that
/// grows while it is dragged; the value after it. Arrows ±1, Shift ±10.
@MainActor
final class LiquidSlider: HoverView {
    var value: Int { didSet { needsDisplay = true } }
    var onChange: ((Int) -> Void)?
    private var dragging = false { didSet { needsDisplay = true } }
    private let valueWidth: CGFloat = 32
    private let trackWidth: CGFloat = 220

    init(value: Int) {
        self.value = value
        super.init(frame: NSRect(x: 0, y: 0, width: 220 + 12 + 32, height: 24))
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var mouseDownCanMoveWindow: Bool { false }
    override var acceptsFirstResponder: Bool { true }

    private let thumbRest = NSSize(width: 22, height: 14)
    private let thumbHeld = NSSize(width: 34, height: 22)

    private var span: (min: CGFloat, max: CGFloat) { (thumbHeld.width / 2, trackWidth - thumbHeld.width / 2) }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let (lo, hi) = span
        let x = lo + (hi - lo) * CGFloat(value) / 100
        let track = NSRect(x: lo, y: 10, width: hi - lo, height: 4)
        Draw.roundedRect(track, radius: 2, fill: p.dark ? NSColor(white: 1, alpha: 0.16) : NSColor(white: 0, alpha: 0.14))
        Draw.roundedRect(NSRect(x: lo, y: 10, width: x - lo, height: 4), radius: 2, fill: p.accent)
        let size = dragging ? thumbHeld : thumbRest
        let thumb = NSRect(x: x - size.width / 2, y: 12 - size.height / 2, width: size.width, height: size.height)
        let shadow = NSShadow()
        shadow.shadowOffset = NSSize(width: 0, height: dragging ? -3 : -1)
        shadow.shadowBlurRadius = dragging ? 10 : 2
        shadow.shadowColor = NSColor(white: 0, alpha: dragging ? 0.28 : 0.18)
        NSGraphicsContext.saveGraphicsState()
        shadow.set()
        Draw.roundedRect(thumb, radius: size.height / 2, fill: NSColor(white: 1, alpha: dragging ? 0.55 : 1))
        NSGraphicsContext.restoreGraphicsState()
        if dragging {
            NSColor(white: 1, alpha: 0.9).setStroke()
            let rim = NSBezierPath(roundedRect: thumb.insetBy(dx: 0.5, dy: 0.5), xRadius: size.height / 2, yRadius: size.height / 2)
            rim.lineWidth = 1
            rim.stroke()
        }
        Draw.text("\(value)", in: NSRect(x: trackWidth + 12, y: 0, width: valueWidth, height: bounds.height), font: Metrics.textSm, color: p.textSecondary, align: .right)
    }

    private func setFrom(_ event: NSEvent) {
        let (lo, hi) = span
        let x = convert(event.locationInWindow, from: nil).x
        let v = Int(((x - lo) / (hi - lo) * 100).rounded())
        set(min(max(v, 0), 100))
    }

    private func set(_ v: Int) {
        guard v != value else { return }
        value = v
        onChange?(v)
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        dragging = true
        setFrom(event)
    }

    override func mouseDragged(with event: NSEvent) { setFrom(event) }
    override func mouseUp(with event: NSEvent) { dragging = false }

    override func keyDown(with event: NSEvent) {
        let step = event.modifierFlags.contains(.shift) ? 10 : 1
        switch Int(event.keyCode) {
        case 123, 125: set(max(value - step, 0))
        case 124, 126: set(min(value + step, 100))
        case 115: set(0)
        case 119: set(100)
        default: super.keyDown(with: event)
        }
    }
}

/// A push button (settings_view.rs `button`): 32 high, `px_4`, radius 6, hover fill and border.
@MainActor
final class PlainButton: HoverView {
    var title: String { didSet { invalidateIntrinsicContentSize(); needsDisplay = true } }
    var action: (() -> Void)?
    private var pressed = false { didSet { needsDisplay = true } }

    init(_ title: String, action: (() -> Void)?) {
        self.title = title
        self.action = action
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var mouseDownCanMoveWindow: Bool { false }
    override var intrinsicContentSize: NSSize { NSSize(width: Draw.textWidth(title, font: Metrics.textSm) + 32, height: 32) }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let r = bounds.insetBy(dx: 0.5, dy: 0.5)
        let path = NSBezierPath(roundedRect: r, xRadius: Metrics.radiusSmall, yRadius: Metrics.radiusSmall)
        (hovered ? p.pressed : p.hover).setFill()
        path.fill()
        p.border.setStroke()
        path.stroke()
        alphaValue = pressed ? 0.8 : 1
        Draw.text(title, in: bounds, font: Metrics.textSm, color: p.text, align: .center)
    }

    override func mouseDown(with event: NSEvent) { pressed = true }

    override func mouseUp(with event: NSEvent) {
        pressed = false
        if bounds.contains(convert(event.locationInWindow, from: nil)) { action?() }
    }
}

/// The accent picker: a "System colour" chip, the nine swatches and "Custom…".
@MainActor
final class AccentPicker: HoverView {
    private var chips: [(rect: NSRect, value: String?)] = []

    override var mouseDownCanMoveWindow: Bool { false }

    private static let systemTitle = String(localized: "System colour")
    private static let customTitle = String(localized: "Custom…")

    override var intrinsicContentSize: NSSize {
        let w = Draw.textWidth(Self.systemTitle, font: Metrics.textSm) + 24 + 8 + CGFloat(AppSettings.swatches.count) * 32
            + Draw.textWidth(Self.customTitle, font: Metrics.textSm) + 24
        return NSSize(width: w, height: 28)
    }

    override func layout() {
        super.layout()
        var x: CGFloat = 0
        chips = []
        let sw = Draw.textWidth(Self.systemTitle, font: Metrics.textSm) + 24
        chips.append((NSRect(x: x, y: 0, width: sw, height: 28), ""))
        x += sw + 8
        for c in AppSettings.swatches {
            chips.append((NSRect(x: x, y: 2, width: 24, height: 24), String(format: "#%06X", c)))
            x += 32
        }
        chips.append((NSRect(x: x, y: 0, width: Draw.textWidth(Self.customTitle, font: Metrics.textSm) + 24, height: 28), nil))
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let current = AppSettings.accent.uppercased()
        let isSwatch = AppSettings.swatches.contains { String(format: "#%06X", $0) == current }
        for (r, value) in chips {
            switch value {
            case let v? where v.hasPrefix("#"):
                let c = Palette.parse(v) ?? .gray
                let circle = NSBezierPath(ovalIn: r)
                c.setFill()
                circle.fill()
                if v == current {
                    p.text.setStroke()
                    let ring = NSBezierPath(ovalIn: r.insetBy(dx: -3, dy: -3))
                    ring.lineWidth = 2
                    ring.stroke()
                }
            default:
                let selected = value == "" ? current.isEmpty : (!current.isEmpty && !isSwatch)
                let path = NSBezierPath(roundedRect: r.insetBy(dx: 0.5, dy: 0.5), xRadius: Metrics.radiusSmall, yRadius: Metrics.radiusSmall)
                p.hover.setFill()
                path.fill()
                (selected ? p.accent : p.border).setStroke()
                path.stroke()
                Draw.text(value == "" ? Self.systemTitle : Self.customTitle, in: r, font: Metrics.textSm, color: p.text, align: .center)
            }
        }
    }

    override func mouseUp(with event: NSEvent) {
        let pt = convert(event.locationInWindow, from: nil)
        guard let hit = chips.first(where: { $0.rect.insetBy(dx: -3, dy: -3).contains(pt) }) else { return }
        if let v = hit.value {
            AppSettings.accent = v
        } else {
            let panel = NSColorPanel.shared
            panel.showsAlpha = false
            panel.setTarget(self)
            panel.setAction(#selector(colourPicked(_:)))
            panel.color = Palette.parse(AppSettings.accent) ?? .controlAccentColor
            panel.orderFront(nil)
        }
        needsDisplay = true
    }

    @objc private func colourPicked(_ sender: NSColorPanel) {
        guard let c = sender.color.usingColorSpace(.sRGB) else { return }
        AppSettings.accent = String(format: "#%02X%02X%02X", Int(c.redComponent * 255), Int(c.greenComponent * 255), Int(c.blueComponent * 255))
        needsDisplay = true
    }
}

// MARK: - cards

/// A settings card (settings_view.rs `card`): radius 8, `hover` fill, 1 px border; at least 64
/// high, `px_4 py_3`; an 18 px icon, the title (`text_sm`) over the description (`text_xs`,
/// secondary); the control on the right.
@MainActor
final class SettingsCardView: FlippedView {
    let title: String
    var detail: String? { didSet { needsDisplay = true; superview?.superview?.superview?.needsLayout = true } }
    let icon: String?
    let control: NSView?

    init(title: String, detail: String?, icon: String?, control: NSView?) {
        self.title = title
        self.detail = detail
        self.icon = icon
        self.control = control
        super.init(frame: .zero)
        if let control { addSubview(control) }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    private var controlSize: NSSize { control.map { $0.intrinsicContentSize.width > 0 ? $0.intrinsicContentSize : $0.frame.size } ?? .zero }
    private var textX: CGFloat { 16 + (icon != nil ? 18 + 16 : 0) }
    private func textWidth(_ width: CGFloat) -> CGFloat { max(width - textX - 16 - (controlSize.width > 0 ? controlSize.width + 16 : 0), 120) }

    private func detailHeight(_ width: CGFloat) -> CGFloat {
        guard let detail else { return 0 }
        let r = (detail as NSString).boundingRect(
            with: NSSize(width: textWidth(width), height: 1000), options: [.usesLineFragmentOrigin], attributes: [.font: Metrics.textXs])
        return ceil(r.height)
    }

    func height(for width: CGFloat) -> CGFloat {
        let titleH = ceil(Metrics.textSm.ascender - Metrics.textSm.descender)
        let text = titleH + (detail != nil ? 2 + detailHeight(width) : 0)
        return max(64, 12 + max(text, controlSize.height) + 12)
    }

    override func layout() {
        super.layout()
        guard let control else { return }
        let s = controlSize
        control.frame = NSRect(x: bounds.width - 16 - s.width, y: (bounds.height - s.height) / 2, width: s.width, height: s.height)
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let r = bounds.insetBy(dx: 0.5, dy: 0.5)
        let path = NSBezierPath(roundedRect: r, xRadius: Metrics.radius, yRadius: Metrics.radius)
        p.hover.setFill()
        path.fill()
        p.border.setStroke()
        path.stroke()
        if let icon {
            Draw.icon(icon, in: NSRect(x: 16, y: 0, width: 18, height: bounds.height), size: 18, color: p.text)
        }
        let tw = textWidth(bounds.width)
        let titleH = ceil(Metrics.textSm.ascender - Metrics.textSm.descender)
        let dh = detail != nil ? 2 + detailHeight(bounds.width) : 0
        let top = (bounds.height - titleH - dh) / 2
        Draw.text(title, in: NSRect(x: textX, y: top, width: tw, height: titleH), font: Metrics.textSm, color: p.text)
        if let detail {
            let para = NSMutableParagraphStyle()
            para.lineBreakMode = .byWordWrapping
            (detail as NSString).draw(
                with: NSRect(x: textX, y: top + titleH + 2, width: tw, height: dh), options: [.usesLineFragmentOrigin],
                attributes: [.font: Metrics.textXs, .foregroundColor: p.textSecondary, .paragraphStyle: para])
        }
    }

    override func viewDidChangeEffectiveAppearance() { needsDisplay = true }
}

/// A row of buttons, or a plain value, as a card's control.
@MainActor
final class ControlRow: FlippedView {
    private let views: [NSView]

    init(_ views: [NSView]) {
        self.views = views
        super.init(frame: .zero)
        views.forEach(addSubview)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var intrinsicContentSize: NSSize {
        let w = views.map(\.intrinsicContentSize.width).reduce(0, +) + CGFloat(max(views.count - 1, 0)) * 8
        return NSSize(width: w, height: views.map(\.intrinsicContentSize.height).max() ?? 32)
    }

    override func layout() {
        super.layout()
        var x: CGFloat = 0
        for v in views {
            let s = v.intrinsicContentSize
            v.frame = NSRect(x: x, y: (bounds.height - s.height) / 2, width: s.width, height: s.height)
            x += s.width + 8
        }
    }
}

/// A plain text value (secondary), as a card's control.
final class ValueLabel: FlippedView {
    let text: String
    init(_ text: String) {
        self.text = text
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var intrinsicContentSize: NSSize { NSSize(width: Draw.textWidth(text, font: Metrics.textSm), height: 20) }
    override func draw(_ dirtyRect: NSRect) {
        Draw.text(text, in: bounds, font: Metrics.textSm, color: Palette.of(self).textSecondary, align: .right)
    }
}

// MARK: - the page

/// The Settings page, in place of the folder in the content card (settings_view.rs `render`):
/// the page list on the left (232 wide: title, search box, pages) and the page on the right
/// (at most 980 wide).
@MainActor
final class SettingsView: FlippedView, NSTextFieldDelegate {
    private var page: SettingsPage = .general
    private let nav = SettingsNavView()
    private let search = NSTextField()
    private let scroll = NSScrollView()
    private let content = FlippedView()
    private var query = ""

    override init(frame: NSRect) {
        super.init(frame: frame)
        nav.onSelect = { [weak self] p in self?.show(p) }
        search.isBordered = false
        search.drawsBackground = false
        search.focusRingType = .none
        search.font = Metrics.textSm
        search.placeholderString = String(localized: "Search settings")
        search.delegate = self
        nav.addSubview(search)
        addSubview(nav.lens)
        scroll.documentView = content
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.scrollerStyle = .overlay
        [nav, scroll].forEach(addSubview)
        NotificationCenter.default.addObserver(self, selector: #selector(settingsChanged(_:)), name: .rfSettingsChanged, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(ferryChanged), name: .rfFerryChanged, object: nil)
        rebuild()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    /// Another window or the menu changed a setting: the controls show the new values and the
    /// cards redraw (the page is not rebuilt, so a slider being dragged stays in hand).
    @objc private func settingsChanged(_ note: Notification) {
        // a setting can decide which rows a page has: rebuilt, unless a slider or the colour
        // panel is being dragged
        if !["surfaceWash", "accent", "listBackground", "listImageVisibility"].contains(AppSettings.changedKey(note)) {
            let y = scroll.contentView.bounds.origin.y
            rebuild()
            layoutSubtreeIfNeeded()
            scroll.contentView.scroll(to: NSPoint(x: 0, y: y))
            return
        }
        refreshers.forEach { $0() }
        func redraw(_ v: NSView) {
            v.needsDisplay = true
            v.subviews.forEach(redraw)
        }
        redraw(self)
    }

    private var refreshers: [() -> Void] = []

    /// Ferry's devices and state are rows of its page: the page is made again.
    @objc private func ferryChanged() {
        if page == .ferry || !query.isEmpty { rebuild() }
    }

    func show(_ p: SettingsPage) {
        defer { reportFerryPage() }
        page = p
        nav.selected = p
        search.stringValue = ""
        query = ""
        rebuild()
        scroll.contentView.scroll(to: .zero)
    }

    /// Whether the Ferry page is what this view shows right now (in a window, not hidden).
    private func reportFerryPage() {
        let showing = window != nil && !isHiddenOrHasHiddenAncestor && page == .ferry && query.isEmpty
        FerryService.shared.settingsPage(self, isShowing: showing)
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        reportFerryPage()
        // a window closed with the page open puts the visibility back too
        if let window {
            NotificationCenter.default.addObserver(forName: NSWindow.willCloseNotification, object: window, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated {
                    guard let self else { return }
                    FerryService.shared.settingsPage(self, isShowing: false)
                }
            }
        }
    }

    override func viewDidHide() {
        super.viewDidHide()
        reportFerryPage()
    }

    override func viewDidUnhide() {
        super.viewDidUnhide()
        reportFerryPage()
    }

    func controlTextDidChange(_ note: Notification) {
        query = search.stringValue.trimmingCharacters(in: .whitespaces).lowercased()
        nav.selected = query.isEmpty ? page : nil
        rebuild()
        reportFerryPage()
    }

    override func layout() {
        super.layout()
        nav.frame = NSRect(x: 0, y: 0, width: 232, height: bounds.height)
        // the search box sits under the title (px_3, pt_5, title 20 + pb_3)
        search.frame = NSRect(x: 12 + 8 + 14 + 6, y: 20 + 28 + 12 + 6, width: 232 - 24 - 28 - 8, height: 20)
        scroll.frame = NSRect(x: 232, y: 0, width: bounds.width - 232, height: bounds.height)
        layoutContent()
    }

    private var rows: [(SettingsRow, NSView)] = []

    /// Rebuilds the cards (a page change, a search, or a setting changed elsewhere).
    private func rebuild() {
        rows.forEach { $0.1.removeFromSuperview() }
        rows = []
        refreshers = []
        let source: [SettingsRow]
        if query.isEmpty {
            source = SettingsContent.rows(page)
        } else {
            source = SettingsPage.allCases.flatMap { SettingsContent.rows($0) }.filter { $0.searchText.contains(query) }
        }
        for row in source {
            let v: NSView
            switch row {
            case .section(let title): v = SectionHeader(title)
            case .card(let title, let detail, let icon, let control):
                let card = SettingsCardView(title: title, detail: detail, icon: icon, control: make(control))
                // a choice with examples says in its card what the chosen one looks like
                if case .choice(_, let get, _, let examples?) = control {
                    let update: () -> Void = { [weak card] in card?.detail = String(localized: "Example: \(examples(get()))") }
                    update()
                    refreshers.append(update)
                }
                v = card
            case .list(let title, let detail, let icon, let entries, let empty, let remove, let buttons):
                v = SettingsListCard(
                    title: title, detail: detail, icon: icon, entries: entries.map { ($0.0, $0.1, nil) }, empty: empty,
                    rowActions: remove.map { r in [(String(localized: "Remove"), r)] } ?? [],
                    control: buttons.isEmpty ? nil : ControlRow(buttons.map { PlainButton($0.0, action: $0.1) }))
            case .table(let title, let detail, let icon, let entries, let empty, let rowActions, let buttons):
                v = SettingsListCard(
                    title: title, detail: detail, icon: icon, entries: entries, empty: empty, rowActions: rowActions,
                    control: buttons.isEmpty ? nil : ControlRow(buttons.map { PlainButton($0.0, action: $0.1) }))
            }
            content.addSubview(v)
            rows.append((row, v))
        }
        if source.isEmpty {
            let none = SectionHeader(String(localized: "No matching settings"), secondary: true)
            content.addSubview(none)
            rows.append((.section(""), none))
        }
        needsLayout = true
    }

    private func make(_ c: SettingsControl) -> NSView {
        switch c {
        case .toggle(let get, let set):
            let s = LiquidSwitch(isOn: get())
            s.onChange = set
            refreshers.append { [weak s] in s?.isOn = get() }
            return s
        case .choice(let options, let get, let set, let examples):
            let d = LiquidDropdown(options: options, value: get())
            d.examples = examples
            d.onChange = set
            refreshers.append { [weak d] in d?.value = get() }
            return d
        case .slider(let get, let set):
            let s = LiquidSlider(value: get())
            s.onChange = set
            refreshers.append { [weak s] in s?.value = get() }
            return s
        case .accent:
            return AccentPicker()
        case .buttons(let list):
            return ControlRow(list.map { PlainButton($0.0, action: $0.1) })
        case .value(let text):
            return ValueLabel(text)
        case .none:
            return ValueLabel("")
        }
    }

    private let titleHeight: CGFloat = 28

    private func layoutContent() {
        let avail = scroll.contentSize.width
        let width = min(avail - 48, 980)
        var y: CGFloat = 20 + titleHeight + 8
        for (_, v) in rows {
            let h: CGFloat
            if let card = v as? SettingsCardView {
                h = card.height(for: width)
            } else if let list = v as? SettingsListCard {
                h = list.height(for: width)
            } else {
                h = 20 + 20 + 4
            }
            v.frame = NSRect(x: 24, y: y, width: width, height: h)
            y += h + 8
        }
        content.frame = NSRect(x: 0, y: 0, width: avail, height: max(y + 24, scroll.contentSize.height))
        content.needsDisplay = true
        pageTitle.stringValue = query.isEmpty ? page.title : String(localized: "Search results")
        if pageTitle.superview == nil { content.addSubview(pageTitle) }
        pageTitle.frame = NSRect(x: 24, y: 20, width: width, height: titleHeight)
        pageTitle.textColor = Palette.of(self).text
    }

    private lazy var pageTitle: NSTextField = {
        let t = NSTextField(labelWithString: "")
        t.font = NSFont.systemFont(ofSize: 20, weight: .semibold)
        return t
    }()

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        needsLayout = true
    }
}

/// A section header (`pt_5 pb_1`, `text_sm` semibold).
final class SectionHeader: FlippedView {
    let title: String
    let secondary: Bool

    init(_ title: String, secondary: Bool = false) {
        self.title = title
        self.secondary = secondary
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        Draw.text(
            title, in: NSRect(x: 0, y: 20, width: bounds.width, height: bounds.height - 24),
            font: secondary ? Metrics.textSm : NSFont.systemFont(ofSize: 14, weight: .semibold), color: secondary ? p.textSecondary : p.text)
    }
}

/// The page list: "Settings" (20 semibold), the search box (32, hover fill, border), then the
/// pages: 36 high, `px_3 gap_3`, 16 px icons; the selected page stands on a glass lens.
@MainActor
final class SettingsNavView: HoverView {
    var selected: SettingsPage? = .general { didSet { needsDisplay = true; placeLens(animated: true) } }
    var onSelect: ((SettingsPage) -> Void)?
    /// The selected page's lens. It lies in the settings view *under* this one, so it never
    /// bends the text drawn here; the page list is at the settings view's origin.
    let lens = LensView(cornerRadius: 14)

    private var searchBox: NSRect { NSRect(x: 12, y: 20 + 28 + 12, width: bounds.width - 24, height: 32) }

    private func itemRect(_ i: Int) -> NSRect {
        NSRect(x: 12, y: searchBox.maxY + 8 + CGFloat(i) * 38, width: bounds.width - 24, height: 36)
    }

    private func placeLens(animated: Bool) {
        guard let s = selected, let i = SettingsPage.allCases.firstIndex(of: s) else {
            lens.isHidden = true
            return
        }
        lens.move(to: itemRect(i), animated: animated)
    }

    override func layout() {
        super.layout()
        placeLens(animated: false)
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        Draw.text(String(localized: "Settings"), in: NSRect(x: 20, y: 20, width: bounds.width - 40, height: 28), font: NSFont.systemFont(ofSize: 20, weight: .semibold), color: p.text)
        let box = searchBox
        let path = NSBezierPath(roundedRect: box.insetBy(dx: 0.5, dy: 0.5), xRadius: Metrics.radiusSmall, yRadius: Metrics.radiusSmall)
        p.hover.setFill()
        path.fill()
        p.border.setStroke()
        path.stroke()
        Draw.icon("search", in: NSRect(x: box.minX + 8, y: box.minY, width: 14, height: box.height), size: 14, color: p.textSecondary)
        let hot = hoverPoint.flatMap { pt in SettingsPage.allCases.indices.first { itemRect($0).contains(pt) } }
        for (i, page) in SettingsPage.allCases.enumerated() {
            let r = itemRect(i)
            let isSelected = page == selected
            if isSelected && !AppSettings.isGlass {
                Draw.roundedRect(r, radius: Metrics.radiusSmall, fill: p.selection)
                Draw.selectionPill(in: r, accent: p.accent)
            } else if !isSelected && hot == i {
                Draw.roundedRect(r, radius: Metrics.radiusSmall, fill: p.hover)
            }
            Draw.icon(page.icon, in: NSRect(x: r.minX + 12, y: r.minY, width: 16, height: r.height), size: 16, color: isSelected ? p.accent : p.textSecondary)
            Draw.text(page.title, in: NSRect(x: r.minX + 12 + 16 + 12, y: r.minY, width: r.width - 52, height: r.height), font: Metrics.textSm, color: p.text)
            if page.isExperimental {
                // the "Experimental" pill: px 6, 1 px border, 10 pt secondary
                let label = String(localized: "Experimental")
                let font = NSFont.systemFont(ofSize: 10)
                let w = Draw.textWidth(label, font: font) + 12
                let x = r.minX + 12 + 16 + 12 + Draw.textWidth(page.title, font: Metrics.textSm) + 8
                let pill = NSRect(x: x, y: r.midY - 8, width: w, height: 16)
                let path = NSBezierPath(roundedRect: pill.insetBy(dx: 0.5, dy: 0.5), xRadius: 8, yRadius: 8)
                p.border.setStroke()
                path.stroke()
                Draw.text(label, in: pill, font: font, color: p.textSecondary, align: .center)
            }
        }
    }

    override func mouseUp(with event: NSEvent) {
        let pt = convert(event.locationInWindow, from: nil)
        guard let i = SettingsPage.allCases.indices.first(where: { itemRect($0).contains(pt) }) else { return }
        onSelect?(SettingsPage.allCases[i])
    }
}

/// Settings › Ferry (ferry.rs `page`): the switch, then, while Ferry is on, who can see this
/// Mac, its name, where received files go, taking files from trusted devices, the devices
/// nearby and the trusted ones.
@MainActor
enum FerryPage {
    static func rows() -> [SettingsRow] {
        let svc = FerryService.shared
        var rows: [SettingsRow] = [
            .card(
                title: String(localized: "Use Ferry"),
                detail: String(localized: "rustyfiles on the same network find each other; files go encrypted straight from device to device, with no server or account. When you turn it on, macOS may ask to let rustyfiles find devices on your local network: allow it."),
                icon: "send",
                control: .toggle(get: { FerrySettings.enabled }, set: { FerrySettings.enabled = $0 })),
        ]
        guard FerrySettings.enabled else { return rows }
        if let problem = svc.problem {
            rows.append(.card(title: String(localized: "Can’t look for nearby devices"), detail: problem, icon: "triangle-alert", control: .none))
        }
        rows += [
            .card(
                title: String(localized: "Who can see this Mac"),
                detail: String(localized: "Everyone lasts 10 minutes, then it goes back to trusted devices only. To take files from a device you haven’t paired with, or to pair with one, choose Everyone."),
                icon: "eye",
                control: .choice(
                    options: [("off", String(localized: "No one")), ("trusted", String(localized: "Trusted devices only")), ("everyone", String(localized: "Everyone (10 minutes)"))],
                    get: { switch FerrySettings.visibility { case .off: "off"; case .everyone: "everyone"; case .trusted: "trusted" } },
                    set: { svc.setVisibility($0 == "off" ? .off : $0 == "everyone" ? .everyone : .trusted) })),
            .card(
                title: String(localized: "Device name"), detail: FerrySettings.effectiveName, icon: "pencil",
                control: .buttons([(String(localized: "Change…"), { askName() })])),
            .card(
                title: String(localized: "Received files"), detail: FerrySettings.effectiveInbox.path, icon: "folder-down",
                control: .buttons(
                    (FerrySettings.inbox.isEmpty ? [] : [(String(localized: "Default"), { FerrySettings.inbox = "" })])
                        + [(String(localized: "Change…"), { askInbox() })])),
            .card(
                title: String(localized: "Take files from trusted devices without asking"),
                detail: String(localized: "Files from devices you paired with go straight to the Received files folder; other devices are asked about every time."),
                icon: "shield-check",
                control: .toggle(get: { FerrySettings.autoAcceptTrusted }, set: { FerrySettings.autoAcceptTrusted = $0 })),
            .section(String(localized: "Nearby devices")),
            .card(
                title: String(localized: "Look again"),
                detail: String(localized: "Forgets the devices listed, looks for them again and announces this Mac again."),
                icon: "rotate-cw", control: .buttons([(String(localized: "Refresh"), { svc.refresh() })])),
        ]
        if svc.peers.isEmpty {
            rows.append(.card(
                title: String(localized: "No devices yet"),
                detail: String(localized: "rustyfiles must be open on the other device, with Ferry on. A device you haven’t paired with is listed only while its visibility is Everyone."),
                icon: "wifi", control: .none))
        }
        for peer in svc.peers {
            var detail = osName(peer.os) + (peer.trusted ? " · " + String(localized: "Trusted") : "")
            if let speed = speedText(peer) { detail += " · " + speed }
            let test = svc.measuring.contains(peer.id) ? String(localized: "Measuring…") : String(localized: "Test speed")
            var buttons: [(String, () -> Void)] = [(test, { svc.measureSpeed(peer) })]
            if !peer.trusted { buttons.append((String(localized: "Pair"), { svc.pair(peer) })) }
            rows.append(.card(title: peer.name, detail: detail, icon: peer.trusted ? "shield-check" : "monitor", control: .buttons(buttons)))
        }
        if !svc.trusted.isEmpty {
            rows.append(.section(String(localized: "Trusted devices")))
            for peer in svc.trusted {
                rows.append(.card(title: peer.name, detail: nil, icon: "shield-check", control: .buttons([(String(localized: "Forget"), { svc.forget(peer) })])))
            }
        }
        return rows
    }

    /// What the device's line says about speed, best first (as on Windows): a test's result
    /// "↑ 48 MB/s · ↓ 52 MB/s · 3 ms", a measured or transfer estimate "~48 MB/s", a link
    /// estimate "≈ 52 MB/s (Wi-Fi)", else the round trip "12 ms".
    static func speedText(_ peer: FerryPeer) -> String? {
        let rate = { (bps: UInt64) in formatSize(bytes: bps) + "/s" }
        if let r = FerryService.shared.speeds[peer.id] {
            if !r.ok { return r.error.map { FerryService.failureText($0) } ?? String(localized: "The speed test did not finish.") }
            return "↑ \(rate(r.upBps)) · ↓ \(rate(r.downBps))" + (r.rttMs > 0 ? " · \(r.rttMs) ms" : "")
        }
        if peer.estimateBps > 0 {
            switch peer.estimateSource {
            case .measured, .transfer: return "~" + rate(peer.estimateBps)
            case .link:
                let kind: String
                switch peer.linkKind {
                case .wifi: kind = "Wi-Fi"
                case .ethernet: kind = "Ethernet"
                default: kind = String(localized: "network")
                }
                return "≈ \(rate(peer.estimateBps)) (\(kind))"
            case .unknown: break
            }
        }
        return peer.rttMs > 0 ? "\(peer.rttMs) ms" : nil
    }

    static func osName(_ os: String) -> String {
        switch os {
        case "windows": "Windows"
        case "macos": "macOS"
        case "linux": "Linux"
        default: os
        }
    }

    private static func askName() {
        let alert = NSAlert()
        alert.messageText = String(localized: "Device name")
        alert.informativeText = String(localized: "Other devices see this Mac by this name. Leave it empty to use the computer’s name.")
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 260, height: 24))
        field.stringValue = FerrySettings.name
        field.placeholderString = Host.current().localizedName
        alert.accessoryView = field
        alert.addButton(withTitle: String(localized: "Save"))
        alert.addButton(withTitle: String(localized: "Cancel"))
        alert.window.initialFirstResponder = field
        if alert.runModal() == .alertFirstButtonReturn {
            FerrySettings.name = field.stringValue.trimmingCharacters(in: .whitespaces)
        }
    }

    private static func askInbox() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.message = String(localized: "Choose the folder for received files")
        panel.directoryURL = FerrySettings.effectiveInbox
        if panel.runModal() == .OK, let url = panel.url { FerrySettings.inbox = url.path }
    }
}

/// A card with a list of entries under its title (settings_view.rs: startup pages, aliases…).
@MainActor
final class SettingsListCard: FlippedView {
    private let head: SettingsCardView
    private let entries: [(String, String?, NSColor?)]
    private let empty: String
    private var rowViews: [(NSTextField, NSTextField?, ControlRow?)] = []
    private var dots: [NSColor?] = []
    private static let rowH: CGFloat = 40

    init(title: String, detail: String?, icon: String?, entries: [(String, String?, NSColor?)], empty: String,
         rowActions: [(String, (Int) -> Void)], control: NSView?) {
        head = SettingsCardView(title: title, detail: detail, icon: icon, control: control)
        self.entries = entries
        self.empty = empty
        super.init(frame: .zero)
        addSubview(head)
        let shown: [(String, String?, NSColor?)] = entries.isEmpty ? [(empty, nil, nil)] : entries
        dots = shown.map(\.2)
        for (i, e) in shown.enumerated() {
            let t = NSTextField(labelWithString: e.0)
            t.font = Metrics.textSm
            t.lineBreakMode = .byTruncatingMiddle
            t.textColor = entries.isEmpty ? .secondaryLabelColor : .labelColor
            addSubview(t)
            var sub: NSTextField?
            if let s = e.1 {
                let f = NSTextField(labelWithString: s)
                f.font = Metrics.textXs
                f.textColor = .secondaryLabelColor
                f.lineBreakMode = .byTruncatingMiddle
                addSubview(f)
                sub = f
            }
            var buttons: ControlRow?
            if !rowActions.isEmpty, !entries.isEmpty {
                let b = ControlRow(rowActions.map { a in PlainButton(a.0) { a.1(i) } })
                addSubview(b)
                buttons = b
            }
            rowViews.append((t, sub, buttons))
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func height(for width: CGFloat) -> CGFloat { head.height(for: width) + CGFloat(rowViews.count) * Self.rowH + 6 }

    override func layout() {
        super.layout()
        let hh = head.height(for: bounds.width)
        head.frame = NSRect(x: 0, y: 0, width: bounds.width, height: hh)
        var y = hh
        for (t, sub, b) in rowViews {
            let bw = b?.intrinsicContentSize.width ?? 0
            b?.frame = NSRect(x: bounds.width - 16 - bw, y: y + (Self.rowH - 28) / 2, width: bw, height: 28)
            let tw = bounds.width - 16 - 52 - (bw > 0 ? bw + 16 : 0)
            if let sub {
                t.frame = NSRect(x: 52, y: y + 3, width: tw, height: 18)
                sub.frame = NSRect(x: 52, y: y + 21, width: tw, height: 16)
            } else {
                t.frame = NSRect(x: 52, y: y + (Self.rowH - 18) / 2, width: tw, height: 18)
            }
            y += Self.rowH
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        // one card: the head draws its own box; the list continues it below with dividers
        let p = Palette.of(self)
        let hh = head.height(for: bounds.width)
        let r = NSRect(x: 0.5, y: hh - 8, width: bounds.width - 1, height: bounds.height - hh + 8 - 0.5)
        let path = NSBezierPath(roundedRect: r, xRadius: Metrics.radius, yRadius: Metrics.radius)
        p.hover.setFill()
        path.fill()
        p.border.setStroke()
        path.stroke()
        p.divider.setFill()
        for i in 0..<rowViews.count {
            NSRect(x: 16, y: hh + CGFloat(i) * Self.rowH, width: bounds.width - 32, height: 1).fill()
        }
        for (i, c) in dots.enumerated() {
            guard let c else { continue }
            c.setFill()
            NSBezierPath(ovalIn: NSRect(x: 26, y: hh + CGFloat(i) * Self.rowH + (Self.rowH - 12) / 2, width: 12, height: 12)).fill()
        }
    }
}
