import AppKit

// MARK: - folder history (folder_history.rs)

/// The folders the user went to, kept across runs, so a few letters find them again: in the
/// command palette and in "Go to Recent Folder". Stored in Application Support as JSON.
@MainActor
enum FolderHistory {
    struct Entry: Codable {
        var path: String
        var visits: Int
        var last: Double
    }

    private static let limit = 500
    private static var entries: [Entry] = load()
    private static var saveScheduled = false

    private static var file: URL {
        let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("rustyfiles", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("folder_history.json")
    }

    private static func load() -> [Entry] {
        guard let data = try? Data(contentsOf: file) else { return [] }
        return (try? JSONDecoder().decode([Entry].self, from: data)) ?? []
    }

    /// A tab landed on `url` (a real folder).
    static func record(_ url: URL) {
        guard url.isFileURL, ProcessInfo.processInfo.environment["RF_NOFERRY"] == nil else { return }
        let path = url.path
        let now = Date().timeIntervalSince1970
        if let i = entries.firstIndex(where: { $0.path == path }) {
            entries[i].visits += 1
            entries[i].last = now
        } else {
            entries.append(Entry(path: path, visits: 1, last: now))
            if entries.count > limit {
                entries.sort { $0.last > $1.last }
                entries.removeLast(entries.count - limit)
            }
        }
        guard !saveScheduled else { return }
        saveScheduled = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
            saveScheduled = false
            let snapshot = entries, target = file
            DispatchQueue.global(qos: .utility).async { try? JSONEncoder().encode(snapshot).write(to: target, options: .atomic) }
        }
    }

    static func clear() {
        entries = []
        try? FileManager.default.removeItem(at: file)
    }

    /// The folders, most visited and most recent first; ones that are gone are left out.
    static var folders: [Entry] {
        let now = Date().timeIntervalSince1970
        func score(_ e: Entry) -> Double { Double(e.visits).squareRoot() * 10 - (now - e.last) / 86_400 }
        return entries.filter { FileManager.default.fileExists(atPath: $0.path) }.sorted { score($0) > score($1) }
    }
}

extension AppSettings {
    /// Settings › General: the palette lists the folders visited before (`palette_folder_history`).
    static var paletteFolderHistory: Bool {
        get { UserDefaults.standard.object(forKey: "paletteFolderHistory") as? Bool ?? true }
        set {
            UserDefaults.standard.set(newValue, forKey: "paletteFolderHistory")
            NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": "paletteFolderHistory"])
        }
    }
}

// MARK: - the palette (command_palette.rs)

/// One row of the palette.
struct PaletteEntry {
    let title: String
    let detail: String
    let icon: String
    let hint: String
    let run: @MainActor () -> Void
}

/// The command palette: one field over the window that filters every command of the menus,
/// the folders visited before and a typed path (⌘K). Opened for folders it lists past folders
/// only; for tabs it lists the windows' open tabs and the ones closed recently.
@MainActor
final class CommandPalette: NSObject, NSTextFieldDelegate, NSTableViewDataSource, NSTableViewDelegate {
    enum Mode { case all, folders, tabs }

    private static var current: CommandPalette?

    static func show(_ mode: Mode, over controller: BrowserWindowController) {
        current?.close()
        guard let parent = controller.window else { return }
        let p = CommandPalette(mode: mode, controller: controller)
        current = p
        p.present(over: parent)
    }

    /// Development: types `text` into the open palette.
    static func demoType(_ text: String) {
        guard let p = current else { return }
        p.field.stringValue = text
        p.refilter()
    }

    private let mode: Mode
    private weak var controller: BrowserWindowController?
    private var all: [PaletteEntry] = []
    private var shown: [PaletteEntry] = []
    private let panel: PalettePanel
    private let field = NSTextField()
    private let table = NSTableView()
    private let scroll = NSScrollView()
    private let width: CGFloat = 620
    private let rowHeight: CGFloat = 40
    private let maxRows = 10

    private init(mode: Mode, controller: BrowserWindowController) {
        self.mode = mode
        self.controller = controller
        panel = PalettePanel(contentRect: NSRect(x: 0, y: 0, width: width, height: 100), styleMask: [.borderless, .fullSizeContentView], backing: .buffered, defer: false)
        super.init()
        all = entries()
        build()
    }

    // MARK: entries

    private func entries() -> [PaletteEntry] {
        guard let controller else { return [] }
        switch mode {
        case .tabs:
            return controller.paletteTabs()
        case .folders:
            return folderEntries()
        case .all:
            return commandEntries() + (AppSettings.paletteFolderHistory ? folderEntries() : [])
        }
    }

    private func folderEntries() -> [PaletteEntry] {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        return FolderHistory.folders.map { e in
            let url = URL(fileURLWithPath: e.path)
            let shown = e.path.hasPrefix(home) ? "~" + e.path.dropFirst(home.count) : e.path
            return PaletteEntry(title: url.lastPathComponent.isEmpty ? e.path : url.lastPathComponent, detail: shown, icon: "folder", hint: "") { [weak controller] in
                controller?.navigate(to: url)
            }
        }
    }

    /// Every command of the menu bar that can run now, named by its menu ("View › Tiles").
    private func commandEntries() -> [PaletteEntry] {
        guard let bar = NSApp.mainMenu else { return [] }
        var out: [PaletteEntry] = []
        func walk(_ menu: NSMenu, _ path: [String]) {
            menu.update()
            for item in menu.items where !item.isSeparatorItem && !item.isHidden && !item.isAlternate {
                if let sub = item.submenu {
                    // the Window menu's own list of windows and the Services menu are not commands
                    if sub == NSApp.servicesMenu { continue }
                    walk(sub, path + (item.title.isEmpty ? [] : [item.title]))
                    continue
                }
                guard item.isEnabled, let action = item.action, !item.title.isEmpty else { continue }
                if action == #selector(NSWindow.makeKeyAndOrderFront(_:)) { continue }
                let target = item.target
                out.append(PaletteEntry(title: item.title, detail: path.joined(separator: " › "), icon: "command", hint: Self.shortcut(item)) {
                    NSApp.sendAction(action, to: target, from: item)
                })
            }
        }
        // the app menu (About, Hide, Quit) last
        let tops = bar.items.filter { $0.submenu != nil }
        for top in tops.dropFirst() + tops.prefix(1) { walk(top.submenu!, [top.title]) }
        return out
    }

    static func shortcut(_ item: NSMenuItem) -> String {
        guard !item.keyEquivalent.isEmpty else { return "" }
        let m = item.keyEquivalentModifierMask
        var s = ""
        if m.contains(.control) { s += "⌃" }
        if m.contains(.option) { s += "⌥" }
        if m.contains(.shift) || item.keyEquivalent != item.keyEquivalent.lowercased() { s += "⇧" }
        if m.contains(.command) { s += "⌘" }
        let names: [Character: String] = ["\r": "↩", "\t": "⇥", " ": "Space", "\u{8}": "⌫", "\u{7f}": "⌫", "\u{1b}": "⎋"]
        let k = item.keyEquivalent
        if let c = k.first, let n = names[c] { return s + n }
        if let u = k.unicodeScalars.first?.value {
            switch Int(u) {
            case NSUpArrowFunctionKey: return s + "↑"
            case NSDownArrowFunctionKey: return s + "↓"
            case NSLeftArrowFunctionKey: return s + "←"
            case NSRightArrowFunctionKey: return s + "→"
            case NSF1FunctionKey...NSF12FunctionKey: return s + "F\(Int(u) - NSF1FunctionKey + 1)"
            default: break
            }
        }
        return s + k.uppercased()
    }

    // MARK: filtering

    /// How well `query` matches `text`: the query inside the text (more at its start or at the
    /// start of a word), or the query's letters as runs that each begin a word ("nf" finds
    /// "New Folder"). nil: no match.
    static func score(_ query: String, _ text: String) -> Int? {
        let q = query.lowercased(), t = text.lowercased()
        guard !q.isEmpty else { return 0 }
        func boundary(_ c: Character?) -> Bool { c == nil || !(c!.isLetter || c!.isNumber) }
        if let r = t.range(of: q) {
            let before = r.lowerBound == t.startIndex ? nil : t[t.index(before: r.lowerBound)]
            let at = t.distance(from: t.startIndex, to: r.lowerBound)
            return 100 + (at == 0 ? 60 : boundary(before) ? 30 : 0) - at - t.count / 8
        }
        let qa = Array(q), ta = Array(t)
        var qi = 0, prev: Character? = nil, inRun = false, score = 0
        for c in ta {
            if qi < qa.count && c == qa[qi] && (inRun || boundary(prev)) {
                score += inRun ? 3 : 8
                inRun = true
                qi += 1
            } else {
                inRun = false
            }
            prev = c
        }
        return qi == qa.count ? score - ta.count / 8 : nil
    }

    private func refilter() {
        let q = field.stringValue.trimmingCharacters(in: .whitespaces)
        var list: [PaletteEntry]
        if q.isEmpty {
            list = all
        } else {
            list = all.compactMap { e -> (Int, PaletteEntry)? in
                let a = Self.score(q, e.title).map { $0 + 10 }
                let b = Self.score(q, e.detail + " " + e.title)
                guard let s = [a, b].compactMap({ $0 }).max() else { return nil }
                return (s, e)
            }
            .sorted { $0.0 > $1.0 }
            .map(\.1)
        }
        // a typed path goes there
        if mode != .tabs, q.hasPrefix("/") || q.hasPrefix("~") {
            let path = (q as NSString).expandingTildeInPath
            var dir: ObjCBool = false
            if FileManager.default.fileExists(atPath: path, isDirectory: &dir) {
                let url = URL(fileURLWithPath: path)
                list.insert(PaletteEntry(title: String(localized: "Go to “\(url.lastPathComponent)”"), detail: q, icon: dir.boolValue ? "folder" : "file", hint: "↩") { [weak controller] in
                    if dir.boolValue { controller?.navigate(to: url) } else { controller?.navigate(to: url.deletingLastPathComponent(), selecting: url.lastPathComponent) }
                }, at: 0)
            }
        }
        shown = Array(list.prefix(200))
        table.reloadData()
        if !shown.isEmpty { table.selectRowIndexes([0], byExtendingSelection: false) }
        resize()
    }

    // MARK: view

    private func build() {
        let root = PaletteBackground()
        panel.contentView = root
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.onResign = { [weak self] in self?.close() }

        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = .systemFont(ofSize: 17)
        field.placeholderString = switch mode {
        case .all: String(localized: "Type a command, a folder or a path")
        case .folders: String(localized: "Go to a folder you visited")
        case .tabs: String(localized: "Search open and closed tabs")
        }
        field.delegate = self
        field.cell?.isScrollable = true
        field.cell?.wraps = false
        root.addSubview(field)

        let col = NSTableColumn(identifier: .init("c"))
        table.addTableColumn(col)
        table.headerView = nil
        table.rowHeight = rowHeight
        table.intercellSpacing = .zero
        table.backgroundColor = .clear
        table.style = .plain
        table.selectionHighlightStyle = .regular
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.action = #selector(clicked)
        scroll.documentView = table
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        root.addSubview(scroll)
    }

    private func present(over parent: NSWindow) {
        refilter()
        let f = parent.frame
        let h = panel.frame.height
        panel.setFrame(NSRect(x: f.midX - width / 2, y: f.maxY - 90 - h, width: width, height: h), display: true)
        parent.addChildWindow(panel, ordered: .above)
        panel.makeKeyAndOrderFront(nil)
        panel.makeFirstResponder(field)
    }

    private func resize() {
        let rows = CGFloat(min(max(shown.count, 1), maxRows))
        let h = 56 + rows * rowHeight + 8
        var f = panel.frame
        let top = f.maxY
        f.size.height = h
        f.origin.y = top - h
        panel.setFrame(f, display: true)
        field.frame = NSRect(x: 18, y: h - 44, width: width - 36, height: 26)
        scroll.frame = NSRect(x: 6, y: 6, width: width - 12, height: h - 56 - 4)
        table.tableColumns.first?.width = scroll.frame.width
    }

    func close() {
        guard Self.current === self else { return }
        Self.current = nil
        panel.onResign = nil
        panel.parent?.removeChildWindow(panel)
        panel.orderOut(nil)
    }

    private func runSelected() {
        let row = table.selectedRow
        guard shown.indices.contains(row) else { return }
        let entry = shown[row]
        let parent = panel.parent
        close()
        parent?.makeKeyAndOrderFront(nil)
        entry.run()
    }

    @objc private func clicked() { runSelected() }

    func controlTextDidChange(_ obj: Notification) { refilter() }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy sel: Selector) -> Bool {
        switch sel {
        case #selector(NSResponder.moveDown(_:)): move(1)
        case #selector(NSResponder.moveUp(_:)): move(-1)
        case #selector(NSResponder.pageDown(_:)), #selector(NSResponder.scrollPageDown(_:)): move(maxRows)
        case #selector(NSResponder.pageUp(_:)), #selector(NSResponder.scrollPageUp(_:)): move(-maxRows)
        case #selector(NSResponder.insertNewline(_:)): runSelected()
        case #selector(NSResponder.cancelOperation(_:)): close()
        default: return false
        }
        return true
    }

    private func move(_ by: Int) {
        guard !shown.isEmpty else { return }
        let row = min(max(table.selectedRow + by, 0), shown.count - 1)
        table.selectRowIndexes([row], byExtendingSelection: false)
        table.scrollRowToVisible(row)
    }

    func numberOfRows(in tableView: NSTableView) -> Int { shown.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = (tableView.makeView(withIdentifier: PaletteRow.id, owner: nil) as? PaletteRow) ?? PaletteRow()
        cell.entry = shown[row]
        return cell
    }
}

private final class PalettePanel: NSPanel {
    var onResign: (() -> Void)?
    override var canBecomeKey: Bool { true }
    override func resignKey() {
        super.resignKey()
        onResign?()
    }
}

/// The palette's glass card.
private final class PaletteBackground: NSView {
    private let glass = NSGlassEffectView()

    override init(frame: NSRect) {
        super.init(frame: frame)
        glass.cornerRadius = 14
        glass.autoresizingMask = [.width, .height]
        addSubview(glass)
        wantsLayer = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        glass.frame = bounds
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let r = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5), xRadius: 14, yRadius: 14)
        p.border.setStroke()
        r.stroke()
        p.divider.setFill()
        NSRect(x: 12, y: bounds.height - 54, width: bounds.width - 24, height: 1).fill()
    }
}

private final class PaletteRow: NSTableCellView {
    static let id = NSUserInterfaceItemIdentifier("paletteRow")

    var entry: PaletteEntry? { didSet { needsDisplay = true } }

    init() {
        super.init(frame: .zero)
        identifier = Self.id
    }

    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ dirtyRect: NSRect) {
        guard let e = entry else { return }
        let p = Palette.of(self)
        let h = bounds.height
        Draw.icon(e.icon, in: NSRect(x: 8, y: 0, width: 24, height: h), size: 16, color: p.textSecondary)
        let titleFont = NSFont.systemFont(ofSize: 13.5)
        let detailFont = NSFont.systemFont(ofSize: 11.5)
        let hintW: CGFloat = e.hint.isEmpty ? 0 : 90
        let textW = bounds.width - 44 - hintW - 12
        let para = NSMutableParagraphStyle()
        para.lineBreakMode = .byTruncatingMiddle
        if e.detail.isEmpty {
            (e.title as NSString).draw(
                in: NSRect(x: 40, y: (h - 18) / 2, width: textW, height: 18), withAttributes: [.font: titleFont, .foregroundColor: p.text, .paragraphStyle: para])
        } else {
            (e.title as NSString).draw(
                in: NSRect(x: 40, y: h / 2 - 1, width: textW, height: 18), withAttributes: [.font: titleFont, .foregroundColor: p.text, .paragraphStyle: para])
            (e.detail as NSString).draw(
                in: NSRect(x: 40, y: h / 2 - 16, width: textW, height: 15), withAttributes: [.font: detailFont, .foregroundColor: p.textSecondary, .paragraphStyle: para])
        }
        if !e.hint.isEmpty {
            let right = NSMutableParagraphStyle()
            right.alignment = .right
            (e.hint as NSString).draw(
                in: NSRect(x: bounds.width - hintW - 12, y: (h - 16) / 2, width: hintW, height: 16),
                withAttributes: [.font: NSFont.systemFont(ofSize: 12), .foregroundColor: p.textTertiary, .paragraphStyle: right])
        }
    }
}
