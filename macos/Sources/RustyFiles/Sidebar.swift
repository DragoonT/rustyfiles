import AppKit

/// The sidebar (workspace.rs `sidebar`): Home, "Pinned" and its folders, the drives, Trash;
/// Settings below a divider at the bottom. Items are 32 high, rounded 6; the selected one
/// stands on a glass lens that slides from item to item (HyperGlass's sidebar lens). No
/// background of its own: the window's glass shows.
@MainActor
final class SidebarView: FlippedView {
    enum Kind { case home, place, pinned, drive, trash, settings, header }

    struct Item {
        let kind: Kind
        let title: String
        let icon: String
        let url: URL?
    }

    var onSelect: ((Item) -> Void)?
    var onEject: ((URL) -> Void)?
    var selectedURL: URL? {
        didSet {
            list.needsDisplay = true
            bottom.needsDisplay = true
            placeLens(animated: true)
        }
    }

    private let scroll = NSScrollView()
    /// The scrolled content: the lens, and over it the items.
    private let document = FlippedView()
    /// A row's lens: 32 high, so rounded 14 (glass.rs `LENS_MAX_RADIUS`).
    private let lens = LensView(cornerRadius: 14)
    private let list = SidebarListView()
    private let divider = Divider()
    private let bottom = SidebarListView()

    override init(frame: NSRect) {
        super.init(frame: frame)
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay
        document.addSubview(lens)
        document.addSubview(list)
        lens.isHidden = true
        scroll.documentView = document
        list.owner = self
        bottom.owner = self
        [scroll, divider, bottom].forEach(addSubview)
        bottom.items = [Item(kind: .settings, title: String(localized: "Settings"), icon: "settings", url: BrowserTab.settingsURL)]
        bottom.hasLens = false
        reload()
        let center = NSWorkspace.shared.notificationCenter
        for name in [NSWorkspace.didMountNotification, NSWorkspace.didUnmountNotification, NSWorkspace.didRenameVolumeNotification] {
            center.addObserver(self, selector: #selector(volumesChanged), name: name, object: nil)
        }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    @objc private func volumesChanged() { reload() }

    func reload() {
        let fm = FileManager.default
        var items: [Item] = [Item(kind: .home, title: String(localized: "Home"), icon: "house", url: BrowserTab.homeURL)]
        // under Home, the two folders a Mac user goes to that Finder's sidebar hides or buries
        items.append(Item(kind: .place, title: fm.displayName(atPath: "/Applications"), icon: "app-window-mac", url: URL(fileURLWithPath: "/Applications")))
        if let lib = fm.urls(for: .libraryDirectory, in: .userDomainMask).first {
            items.append(Item(kind: .place, title: String(localized: "Library"), icon: "library", url: lib))
        }
        items.append(Item(kind: .header, title: String(localized: "Pinned"), icon: "", url: nil))
        for url in Pins.load() {
            items.append(Item(kind: .pinned, title: fm.displayName(atPath: url.path), icon: "folder", url: url))
        }
        let keys: [URLResourceKey] = [.volumeLocalizedNameKey, .volumeIsLocalKey]
        let volumes = fm.mountedVolumeURLs(includingResourceValuesForKeys: keys, options: [.skipHiddenVolumes]) ?? []
        if !volumes.isEmpty {
            items.append(Item(kind: .header, title: String(localized: "Drives"), icon: "", url: nil))
            for v in volumes {
                let values = try? v.resourceValues(forKeys: Set(keys))
                let icon = values?.volumeIsLocal == false ? "server" : "hard-drive"
                items.append(Item(kind: .drive, title: values?.volumeLocalizedName ?? v.lastPathComponent, icon: icon, url: v))
            }
        }
        defer { needsLayout = true }
        items.append(Item(kind: .trash, title: String(localized: "Trash"), icon: "trash", url: fm.homeDirectoryForCurrentUser.appendingPathComponent(".Trash")))
        list.items = items
        needsLayout = true
    }

    override func layout() {
        super.layout()
        // bottom block: divider (mx_2) then Settings in a px_2 py_1 wrapper
        let bottomH: CGFloat = 4 + 32 + 4
        bottom.frame = NSRect(x: 0, y: bounds.height - bottomH, width: bounds.width, height: bottomH)
        divider.frame = NSRect(x: 8, y: bottom.frame.minY - 1, width: bounds.width - 16, height: 1)
        scroll.frame = NSRect(x: 0, y: 0, width: bounds.width, height: divider.frame.minY)
        document.frame = NSRect(x: 0, y: 0, width: scroll.contentSize.width, height: max(list.contentHeight, scroll.contentSize.height))
        list.frame = document.bounds
        placeLens(animated: false)
    }

    private func placeLens(animated: Bool) {
        lens.move(to: list.selectedRect ?? .zero, animated: animated && !inLiveResize)
    }

    fileprivate func clicked(_ item: Item) { onSelect?(item) }

    /// File URLs compared in their standard form; other places (Settings) as they are.
    static func norm(_ u: URL) -> URL { u.isFileURL ? u.standardizedFileURL : u }
}

/// One column of sidebar items (`px_2`, `py_1`, `gap_0p5`).
@MainActor
final class SidebarListView: HoverView {
    weak var owner: SidebarView?
    var items: [SidebarView.Item] = [] { didSet { needsDisplay = true } }
    /// The list in the scroll view has a lens under its selected item; the bottom one draws
    /// the Fluent selection (fill and pill) itself.
    var hasLens = true

    private func rects() -> [NSRect] {
        var y: CGFloat = 4
        return items.map { item in
            let h: CGFloat = item.kind == .header ? 12 + 16 + 4 : 32
            let r = NSRect(x: 8, y: y, width: bounds.width - 16, height: h)
            y += h + 2
            return r
        }
    }

    var contentHeight: CGFloat { (rects().last?.maxY ?? 0) + 4 }

    /// The selected item's box, where the lens goes.
    var selectedRect: NSRect? {
        guard hasLens else { return nil }
        let selected = owner?.selectedURL.map(SidebarView.norm)
        return zip(items, rects()).first { $0.0.url.map(SidebarView.norm) == selected && $0.0.kind != .header }?.1
    }

    private func index(at p: NSPoint) -> Int? {
        rects().firstIndex { $0.contains(p) }.flatMap { items[$0].kind == .header ? nil : $0 }
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let hot = hoverPoint.flatMap(index(at:))
        let selected = owner?.selectedURL.map(SidebarView.norm)
        for (i, (item, r)) in zip(items, rects()).enumerated() {
            if item.kind == .header {
                // px_2, pt_3, pb_1, text_xs semibold, tertiary
                Draw.text(item.title, in: NSRect(x: r.minX + 8, y: r.minY + 12, width: r.width - 16, height: 16), font: Metrics.textXsSemibold, color: p.textTertiary)
                continue
            }
            // the selected item stands on the lens: no fill, no pill
            let isSelected = item.url.map { SidebarView.norm($0) == selected } ?? false
            if isSelected && (!hasLens || !AppSettings.isGlass) {
                Draw.roundedRect(r, radius: Metrics.radiusSmall, fill: p.selection)
                Draw.selectionPill(in: r, accent: p.accent)
            } else if !isSelected && hot == i {
                Draw.roundedRect(r, radius: Metrics.radiusSmall, fill: p.hover)
            }
            // px_2, icon 16, gap_2, text_sm
            let iconColor: NSColor = (item.kind == .home || item.kind == .place || item.kind == .pinned) ? p.accent : p.textSecondary
            Draw.icon(item.icon, in: NSRect(x: r.minX + 8, y: r.minY, width: 16, height: r.height), size: 16, color: iconColor)
            Draw.text(item.title, in: NSRect(x: r.minX + 8 + 16 + 8, y: r.minY, width: r.width - 40, height: r.height), font: Metrics.textSm, color: p.text)
        }
    }

    override func mouseUp(with event: NSEvent) {
        guard let i = index(at: convert(event.locationInWindow, from: nil)) else { return }
        owner?.clicked(items[i])
    }

    /// A drive that can be ejected (a disk image, a USB disk, a network share) offers Eject.
    override func menu(for event: NSEvent) -> NSMenu? {
        guard let i = index(at: convert(event.locationInWindow, from: nil)), items[i].kind == .drive, let url = items[i].url else { return nil }
        let values = try? url.resourceValues(forKeys: [.volumeIsEjectableKey, .volumeIsRemovableKey, .volumeIsLocalKey, .volumeIsInternalKey])
        let ejectable = values?.volumeIsEjectable == true || values?.volumeIsRemovable == true || values?.volumeIsLocal == false
            || values?.volumeIsInternal == false
        guard ejectable else { return nil }
        let m = NSMenu()
        let item = ClosureMenuItem(title: String(localized: "Eject")) { [weak self] in self?.owner?.onEject?(url) }
        item.image = Icons.named("eject")
        m.addItem(item)
        return m
    }
}

/// The pinned folders, kept in the user defaults. The first run pins the usual places.
@MainActor
enum Pins {
    private static let key = "pinned"

    static func load() -> [URL] {
        if let paths = UserDefaults.standard.stringArray(forKey: key) {
            return paths.map { URL(fileURLWithPath: $0) }
        }
        let fm = FileManager.default
        let dirs: [FileManager.SearchPathDirectory] = [.desktopDirectory, .downloadsDirectory, .documentDirectory, .picturesDirectory, .musicDirectory, .moviesDirectory]
        return dirs.compactMap { fm.urls(for: $0, in: .userDomainMask).first }.filter { fm.fileExists(atPath: $0.path) }
    }

    static func save(_ urls: [URL]) {
        UserDefaults.standard.set(urls.map(\.path), forKey: key)
    }
}

/// The 6 px handle on the sidebar's right edge.
@MainActor
final class SidebarResizeHandle: FlippedView {
    var onDrag: ((CGFloat) -> Void)?
    private var start: (x: CGFloat, width: CGFloat)?
    var currentWidth: () -> CGFloat = { Metrics.sidebarWidth }

    override var mouseDownCanMoveWindow: Bool { false }

    override func resetCursorRects() { addCursorRect(bounds, cursor: .resizeLeftRight) }

    override func mouseDown(with event: NSEvent) { start = (event.locationInWindow.x, currentWidth()) }

    override func mouseDragged(with event: NSEvent) {
        guard let start else { return }
        onDrag?(min(max(start.width + event.locationInWindow.x - start.x, Metrics.sidebarMin), Metrics.sidebarMax))
    }

    override func mouseUp(with event: NSEvent) { start = nil }
}
