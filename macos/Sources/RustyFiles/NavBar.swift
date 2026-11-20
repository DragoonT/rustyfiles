import AppKit

/// The address pill's breadcrumb (workspace.rs `breadcrumb`): ⌂ › Macintosh HD › Users › ali ›
/// Every separator, the one after the last segment too, opens a menu of the folders in that
/// place; the current folder's segment is medium weight in the text colour.
@MainActor
final class BreadcrumbView: HoverView {
    struct Segment {
        let title: String
        let url: URL
    }

    var segments: [Segment] = [] { didSet { needsLayout = true; needsDisplay = true } }
    var onNavigate: ((URL) -> Void)?
    var onHome: (() -> Void)?
    /// A click on empty space: the pill turns into a text field.
    var onEmptyClick: (() -> Void)?

    private enum Part {
        case home
        case ellipsis
        case segment(Int)
        case separator(Int)
    }

    private var parts: [(Part, NSRect)] = []

    override var mouseDownCanMoveWindow: Bool { false }

    static func segments(for url: URL) -> [Segment] {
        var result: [Segment] = []
        var u = url.standardizedFileURL
        while true {
            let isVolume = (try? u.resourceValues(forKeys: [.isVolumeKey]).isVolume) ?? false
            if isVolume || u.path == "/" {
                let name = (try? u.resourceValues(forKeys: [.volumeLocalizedNameKey]).volumeLocalizedName) ?? u.lastPathComponent
                result.append(Segment(title: name, url: u))
                break
            }
            result.append(Segment(title: FileManager.default.displayName(atPath: u.path), url: u))
            u = u.deletingLastPathComponent()
        }
        return result.reversed()
    }

    private static let homeW: CGFloat = 14 + 8
    private static let sepW: CGFloat = 12 + 8

    private func segW(_ s: String, last: Bool) -> CGFloat {
        Draw.textWidth(s, font: last ? Metrics.textSmMedium : Metrics.textSm) + 12
    }

    override func layout() {
        super.layout()
        parts = []
        let h = bounds.height
        let boxH: CGFloat = 26
        let y = (h - boxH) / 2
        // Leading segments fold into "…" while the whole trail does not fit (gap_0p5 between parts).
        let widths = segments.enumerated().map { segW($0.element.title, last: $0.offset == segments.count - 1) + 2 + Self.sepW + 2 }
        var first = 0
        let fixed = Self.homeW + 2 + Self.sepW + 2
        let ellipsisW = Draw.textWidth("…", font: Metrics.textSm) + 12 + 2 + Self.sepW + 2
        while first < segments.count - 1 && fixed + (first > 0 ? ellipsisW : 0) + widths[first...].reduce(0, +) > bounds.width {
            first += 1
        }
        var x: CGFloat = 0
        func add(_ part: Part, _ w: CGFloat) {
            parts.append((part, NSRect(x: x, y: y, width: w, height: boxH)))
            x += w + 2
        }
        add(.home, Self.homeW)
        add(.separator(-1), Self.sepW)
        if first > 0 {
            add(.ellipsis, Draw.textWidth("…", font: Metrics.textSm) + 12)
            add(.separator(first - 1), Self.sepW)
        }
        for i in first..<segments.count {
            add(.segment(i), segW(segments[i].title, last: i == segments.count - 1))
            add(.separator(i), Self.sepW)
        }
    }

    private func part(at p: NSPoint) -> (Part, NSRect)? { parts.first { $0.1.contains(p) } }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let hoveredRect = hoverPoint.flatMap(part(at:))?.1
        for (part, r) in parts {
            let hot = r == hoveredRect
            if hot { Draw.roundedRect(r, radius: Metrics.radiusSmall, fill: p.hover) }
            switch part {
            case .home:
                Draw.icon("house", in: r, size: 14, color: hot ? p.text : p.textSecondary)
            case .separator:
                Draw.icon("chevron-right", in: r, size: 12, color: p.textTertiary)
            case .ellipsis:
                Draw.text("…", in: r, font: Metrics.textSm, color: hot ? p.text : p.textSecondary, align: .center)
            case .segment(let i):
                let last = i == segments.count - 1
                Draw.text(
                    segments[i].title, in: r.insetBy(dx: 6, dy: 0), font: last ? Metrics.textSmMedium : Metrics.textSm,
                    color: (last || hot) ? p.text : p.textSecondary)
            }
        }
    }

    override func mouseDown(with event: NSEvent) {
        guard let (part, r) = part(at: convert(event.locationInWindow, from: nil)) else {
            onEmptyClick?()
            return
        }
        switch part {
        case .home: onHome?()
        case .segment(let i): onNavigate?(segments[i].url)
        case .ellipsis: popFolders(of: segments.count > 1 ? segments[0].url : nil, at: r, plus: Array(segments.dropLast()))
        case .separator(let i):
            let url = i < 0 ? segments.first?.url : segments[i].url
            popFolders(of: url, at: r, plus: [])
        }
    }

    /// The folders in `url` as a menu under `r` (listed off the main thread).
    private func popFolders(of url: URL?, at r: NSRect, plus earlier: [Segment]) {
        let menu = NSMenu()
        for s in earlier { menu.addItem(item(s.title, s.url)) }
        if !earlier.isEmpty { menu.addItem(.separator()) }
        guard let url else { return }
        let path = url.path
        Task.detached(priority: .userInitiated) {
            let folder = try? Folder.open(path: path, column: .name, ascending: true, foldersFirst: true, showHidden: false)
            let dirs: [(String, String)] = folder.map { f in
                (0..<f.count()).compactMap { r in
                    guard let it = f.item(row: r), it.isDir, let p = f.childPath(row: r) else { return nil }
                    return (it.name, p)
                }
            } ?? []
            await MainActor.run { [weak self] in
                guard let self else { return }
                for (name, p) in dirs.prefix(200) { menu.addItem(self.item(name, URL(fileURLWithPath: p))) }
                if menu.items.isEmpty {
                    let none = NSMenuItem(title: String(localized: "No folders"), action: nil, keyEquivalent: "")
                    none.isEnabled = false
                    menu.addItem(none)
                }
                menu.popUp(positioning: nil, at: NSPoint(x: r.minX, y: r.maxY + 4), in: self)
            }
        }
    }

    private func item(_ title: String, _ url: URL) -> NSMenuItem {
        let i = NSMenuItem(title: title, action: #selector(menuNavigate(_:)), keyEquivalent: "")
        i.target = self
        i.representedObject = url
        i.image = Icons.menu("folder")
        return i
    }

    @objc private func menuNavigate(_ sender: NSMenuItem) {
        if let url = sender.representedObject as? URL { onNavigate?(url) }
    }
}

/// The address pill (workspace.rs `omnibar`): a capsule in `content_bg` with a 1 px border
/// (accent while typing), the breadcrumb, a divider and the two mode buttons.
@MainActor
final class OmnibarView: FlippedView, NSTextFieldDelegate {
    enum Mode { case breadcrumb, path, filter }

    let breadcrumb = BreadcrumbView()
    let field = NSTextField()
    let commandButton = IconButton(icon: "square-chevron-right", size: NSSize(width: 28, height: 28), iconSize: 15, tooltip: String(localized: "Go to a path"))
    let searchButton = IconButton(icon: "search", size: NSSize(width: 28, height: 28), iconSize: 15, tooltip: String(localized: "Filter this folder"))
    private let divider = Divider()

    private(set) var mode: Mode = .breadcrumb
    var currentPath = ""
    var onSubmitPath: ((String) -> Void)?
    var onFilter: ((String) -> Void)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = Metrics.textSm
        field.delegate = self
        field.isHidden = true
        field.cell?.isScrollable = true
        field.cell?.wraps = false
        commandButton.tint = { $0.textSecondary }
        searchButton.tint = { $0.textSecondary }
        commandButton.action = { [weak self] _ in self?.begin(.path) }
        searchButton.action = { [weak self] _ in
            guard let self else { return }
            self.mode == .filter ? self.end() : self.begin(.filter)
        }
        breadcrumb.onEmptyClick = { [weak self] in self?.begin(.path) }
        [breadcrumb, field, divider, commandButton, searchButton].forEach(addSubview)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var mouseDownCanMoveWindow: Bool { false }

    override func layout() {
        super.layout()
        let h = bounds.height
        // pl_2 … gap_1 · divider (mx_1) · 28 · 28 · pr_1
        var right = bounds.width - 4
        searchButton.frame = NSRect(x: right - 28, y: (h - 28) / 2, width: 28, height: 28)
        right -= 28
        commandButton.frame = NSRect(x: right - 28, y: (h - 28) / 2, width: 28, height: 28)
        right -= 28 + 4
        divider.frame = NSRect(x: right - 1, y: (h - 18) / 2, width: 1, height: 18)
        right -= 1 + 4 + 4
        let content = NSRect(x: 8, y: 0, width: max(right - 8, 0), height: h)
        breadcrumb.frame = content
        let fh: CGFloat = 20
        field.frame = NSRect(x: content.minX + 4, y: (h - fh) / 2, width: content.width - 4, height: fh)
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let r = bounds.insetBy(dx: 0.5, dy: 0.5)
        let path = NSBezierPath(roundedRect: r, xRadius: r.height / 2, yRadius: r.height / 2)
        p.contentBg.setFill()
        path.fill()
        (mode == .breadcrumb ? p.border : p.accent).setStroke()
        path.lineWidth = 1
        path.stroke()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        needsDisplay = true
    }

    func begin(_ m: Mode) {
        mode = m
        breadcrumb.isHidden = true
        field.isHidden = false
        searchButton.isOn = m == .filter
        commandButton.isOn = m == .path
        if m == .path {
            field.placeholderString = nil
            field.stringValue = currentPath
        } else {
            field.placeholderString = String(localized: "Filter by name")
            field.stringValue = ""
        }
        window?.makeFirstResponder(field)
        field.currentEditor()?.selectAll(nil)
        needsDisplay = true
    }

    /// Back to the breadcrumb (a filter is cleared).
    func end() {
        if mode == .filter { onFilter?("") }
        mode = .breadcrumb
        field.isHidden = true
        breadcrumb.isHidden = false
        searchButton.isOn = false
        commandButton.isOn = false
        needsDisplay = true
    }

    func controlTextDidChange(_ note: Notification) {
        if mode == .filter { onFilter?(field.stringValue) }
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)):
            if mode == .path {
                let text = field.stringValue
                end()
                onSubmitPath?(text)
            } else {
                // Enter in the filter moves into the list, keeping the filter.
                window?.makeFirstResponder(nil)
            }
            return true
        case #selector(NSResponder.cancelOperation(_:)):
            end()
            return true
        default:
            return false
        }
    }

    func controlTextDidEndEditing(_ note: Notification) {
        if mode == .path { end() }
    }
}

/// The 48 px row under the tabs: sidebar toggle · back · forward · up · refresh · address
/// pill · status centre. (On Windows the toggle is in the title bar's lead column; here the
/// window buttons are there.)
@MainActor
final class NavBarView: FlippedView {
    let sidebarToggle = IconButton(icon: "panel-left", size: NSSize(width: 32, height: 32), tooltip: String(localized: "Show or hide the sidebar"))
    let back = IconButton(icon: "arrow-left", size: NSSize(width: 32, height: 32), tooltip: String(localized: "Back"))
    let forward = IconButton(icon: "arrow-right", size: NSSize(width: 32, height: 32), tooltip: String(localized: "Forward"))
    let up = IconButton(icon: "arrow-up", size: NSSize(width: 32, height: 32), tooltip: String(localized: "Up"))
    let refresh = IconButton(icon: "rotate-cw", size: NSSize(width: 32, height: 32), tooltip: String(localized: "Refresh"))
    let omnibar = OmnibarView()
    let statusCenter = IconButton(icon: "list-checks", size: NSSize(width: 32, height: 32), tooltip: String(localized: "Operations"))

    override init(frame: NSRect) {
        super.init(frame: frame)
        [sidebarToggle, back, forward, up, refresh, omnibar, statusCenter].forEach(addSubview)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        // px_2, gap_1; the pill is flex with mx_1
        let h = bounds.height
        var x: CGFloat = 8
        for b in [sidebarToggle, back, forward, up, refresh] {
            b.frame = NSRect(x: x, y: (h - 32) / 2, width: 32, height: 32)
            x += 32 + 4
        }
        let right = bounds.width - 8 - 32
        statusCenter.frame = NSRect(x: right, y: (h - 32) / 2, width: 32, height: 32)
        x += 4
        omnibar.frame = NSRect(x: x, y: (h - 36) / 2, width: max(right - 4 - 4 - x, 160), height: 36)
    }

    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 2 { window?.performZoom(nil) } else { window?.performDrag(with: event) }
    }
}
