import AppKit

/// The sidebar (workspace.rs `sidebar`): Home, "Pinned" and its folders, the drives, Trash;
/// Settings below a divider at the bottom. Items are 32 high, rounded 6; the selected one
/// stands on a glass lens that slides from item to item (HyperGlass's sidebar lens). No
/// background of its own: the window's glass shows.
@MainActor
final class SidebarView: FlippedView {
    enum Kind { case home, place, pinned, drive, tag, tree, trash, settings, header }

    struct Item {
        let kind: Kind
        let title: String
        let icon: String
        let url: URL?
        /// A tag's colour (drawn as a dot in place of the icon).
        var color: NSColor? = nil
        /// The folder tree: how deep, and whether it has folders inside (a chevron).
        var depth = 0
        var expandable = false
        var expanded = false
    }

    /// The folder tree's open folders (paths), kept across starts.
    private var expanded: Set<String> = Set(UserDefaults.standard.stringArray(forKey: "treeExpanded") ?? []) {
        didSet { UserDefaults.standard.set(Array(expanded), forKey: "treeExpanded") }
    }

    func toggle(_ item: Item) {
        guard let p = item.url?.standardizedFileURL.path else { return }
        if expanded.contains(p) { expanded.remove(p) } else { expanded.insert(p) }
        reload()
    }

    /// The folders inside `dir` (not hidden, not packages), by name.
    private static func subfolders(_ dir: URL) -> [URL] {
        let keys: [URLResourceKey] = [.isDirectoryKey, .isPackageKey, .isHiddenKey]
        let all = (try? FileManager.default.contentsOfDirectory(at: dir, includingPropertiesForKeys: keys, options: [.skipsHiddenFiles])) ?? []
        return all.filter {
            let v = try? $0.resourceValues(forKeys: Set(keys))
            return v?.isDirectory == true && v?.isPackage != true
        }
        .sorted { $0.lastPathComponent.localizedStandardCompare($1.lastPathComponent) == .orderedAscending }
        .prefix(400).map { $0 }
    }

    private func treeItems(_ dir: URL, depth: Int, title: String? = nil, icon: String = "folder") -> [Item] {
        let path = dir.standardizedFileURL.path
        let open = expanded.contains(path)
        let kids = open ? Self.subfolders(dir) : []
        let hasKids = open ? !kids.isEmpty : true
        var out = [Item(kind: .tree, title: title ?? FileManager.default.displayName(atPath: path), icon: icon, url: dir,
                        depth: depth, expandable: hasKids, expanded: open)]
        for k in kids { out += treeItems(k, depth: depth + 1) }
        return out
    }

    /// Appearance › Follow the open folder: the tree opens down to it.
    func follow(_ url: URL) {
        guard AppSettings.showsSection(.tree), AppSettings.treeFollows, url.isFileURL else { return }
        let target = url.standardizedFileURL.path
        let roots = [FileManager.default.homeDirectoryForCurrentUser.standardizedFileURL.path, "/"]
        guard let root = roots.first(where: { target == $0 || target.hasPrefix($0 == "/" ? "/" : $0 + "/") }) else { return }
        var add: [String] = []
        var d = URL(fileURLWithPath: target).deletingLastPathComponent()
        while d.path.count >= root.count, d.path.hasPrefix(root) {
            add.append(d.standardizedFileURL.path)
            if d.path == root || d.path == "/" { break }
            d = d.deletingLastPathComponent()
        }
        let new = Set(add).subtracting(expanded)
        guard !new.isEmpty else { return }
        expanded.formUnion(new)
        reload()
    }

    var onSelect: ((Item) -> Void)?
    var onEject: ((URL) -> Void)?
    var onOpenInNewTab: ((URL) -> Void)?
    var onOpenInNewWindow: ((URL) -> Void)?
    var selectedURL: URL? {
        didSet {
            if let u = selectedURL, u != oldValue { follow(u) }
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
        NotificationCenter.default.addObserver(self, selector: #selector(volumesChanged), name: .rfPinsChanged, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(settingsChanged(_:)), name: .rfSettingsChanged, object: nil)
    }

    @objc private func settingsChanged(_ note: Notification) {
        if ["sidebarHidden", "sidebarTree", "tagList"].contains(AppSettings.changedKey(note)) { reload() }
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    @objc private func volumesChanged() { reload() }

    func reload() {
        let fm = FileManager.default
        var items: [Item] = [Item(kind: .home, title: String(localized: "Home"), icon: "house", url: BrowserTab.homeURL)]
        // under Home, the two folders a Mac user goes to that Finder's sidebar hides or buries
        if AppSettings.showsSection(.libraries) {
            items.append(Item(kind: .place, title: fm.displayName(atPath: "/Applications"), icon: "app-window-mac", url: URL(fileURLWithPath: "/Applications")))
            if let lib = fm.urls(for: .libraryDirectory, in: .userDomainMask).first {
                items.append(Item(kind: .place, title: String(localized: "Library"), icon: "library", url: lib))
            }
        }
        if AppSettings.showsSection(.pinned) {
            items.append(Item(kind: .header, title: String(localized: "Pinned"), icon: "", url: nil))
            for url in Pins.load() {
                items.append(Item(kind: .pinned, title: fm.displayName(atPath: url.path), icon: "folder", url: url))
            }
        }
        let keys: [URLResourceKey] = [.volumeLocalizedNameKey, .volumeIsLocalKey]
        let volumes = fm.mountedVolumeURLs(includingResourceValuesForKeys: keys, options: [.skipHiddenVolumes]) ?? []
        if !volumes.isEmpty, AppSettings.showsSection(.drives) {
            items.append(Item(kind: .header, title: String(localized: "Drives"), icon: "", url: nil))
            for v in volumes {
                let values = try? v.resourceValues(forKeys: Set(keys))
                let icon = values?.volumeIsLocal == false ? "server" : "hard-drive"
                items.append(Item(kind: .drive, title: values?.volumeLocalizedName ?? v.lastPathComponent, icon: icon, url: v))
            }
        }
        // the folder tree (Settings › Appearance › Sidebar sections)
        if AppSettings.showsSection(.tree) {
            items.append(Item(kind: .header, title: String(localized: "Folders"), icon: "", url: nil))
            items += treeItems(fm.homeDirectoryForCurrentUser, depth: 0, icon: "house")
            items += treeItems(URL(fileURLWithPath: "/"), depth: 0, title: (try? URL(fileURLWithPath: "/").resourceValues(forKeys: [.volumeLocalizedNameKey]).volumeLocalizedName) ?? "/", icon: "hard-drive")
        }
        // Tags (Settings › Appearance › Sidebar sections): a click lists what has the tag
        if AppSettings.showsSection(.tags), !TagStore.all.isEmpty {
            items.append(Item(kind: .header, title: String(localized: "Tags"), icon: "", url: nil))
            for t in TagStore.all {
                items.append(Item(kind: .tag, title: t.name, icon: "tag", url: TagStore.url(t.name), color: TagStore.color(t.color)))
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

    func clicked(_ item: Item) { onSelect?(item) }

    /// File URLs compared in their standard form; other places (Settings) as they are.
    static func norm(_ u: URL) -> URL { u.isFileURL ? u.standardizedFileURL : u }
}

/// One column of sidebar items (`px_2`, `py_1`, `gap_0p5`).
@MainActor
final class SidebarListView: HoverView {
    weak var owner: SidebarView?
    /// The item a drag is over (drawn as a drop target).
    private var dropIndex: Int? { didSet { if dropIndex != oldValue { needsDisplay = true } } }
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

    override func viewDidMoveToSuperview() {
        super.viewDidMoveToSuperview()
        registerForDraggedTypes([.fileURL])
    }

    /// Where files dropped on item `i` go (a folder, a drive), or nil (Home, Settings).
    private func dropURL(_ i: Int) -> URL? {
        let item = items[i]
        if item.kind == .tag { return item.url }
        guard let url = item.url, url.isFileURL, item.kind != .header else { return nil }
        return url
    }

    private func dragOperation(_ info: NSDraggingInfo) -> NSDragOperation {
        let pt = convert(info.draggingLocation, from: nil)
        dropIndex = index(at: pt).flatMap { dropURL($0) != nil ? $0 : nil }
        guard let i = dropIndex, let dest = dropURL(i) else { return [] }
        let urls = FileListController.droppedURLs(info)
        guard !urls.isEmpty else { return [] }
        if items[i].kind == .trash { return .delete }
        if items[i].kind == .tag { return .link }
        return FileListController.dropKind(info, urls, into: dest) == .copy ? .copy : .move
    }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation { dragOperation(sender) }
    override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation { dragOperation(sender) }
    override func draggingExited(_ sender: NSDraggingInfo?) { dropIndex = nil }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        defer { dropIndex = nil }
        guard let i = dropIndex, let dest = dropURL(i) else { return false }
        let urls = FileListController.droppedURLs(sender)
        if items[i].kind == .tag {
            // dropped on a tag: the files get it
            let name = items[i].title
            for u in urls {
                var tags = (try? u.resourceValues(forKeys: [.tagNamesKey]).tagNames) ?? []
                if !tags.contains(name) { tags.append(name) }
                try? (u as NSURL).setResourceValue(tags, forKey: .tagNamesKey)
            }
            return true
        }
        if items[i].kind == .trash {
            DispatchQueue.global(qos: .userInitiated).async {
                let pairs = (try? FileOps.trash(urls)) ?? []
                DispatchQueue.main.async {
                    MainActor.assumeIsolated { FileUndo.shared.record(.moves(pairs), title: String(localized: "Move to Trash")) }
                }
            }
            return true
        }
        Transfer(FileListController.dropKind(sender, urls, into: dest), urls, into: dest).start(window: window) { _ in }
        return true
    }

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
            if dropIndex == i {
                // a drop target: the selection fill with an accent outline
                Draw.roundedRect(r, radius: Metrics.radiusSmall, fill: p.selection)
                p.accent.setStroke()
                let ring = NSBezierPath(roundedRect: r.insetBy(dx: 0.5, dy: 0.5), xRadius: Metrics.radiusSmall, yRadius: Metrics.radiusSmall)
                ring.lineWidth = 1
                ring.stroke()
            } else if isSelected && (!hasLens || !AppSettings.isGlass) {
                Draw.roundedRect(r, radius: Metrics.radiusSmall, fill: p.selection)
                Draw.selectionPill(in: r, accent: p.accent)
            } else if !isSelected && hot == i {
                Draw.roundedRect(r, radius: Metrics.radiusSmall, fill: p.hover)
            }
            // px_2, icon 16, gap_2, text_sm
            let iconColor: NSColor = (item.kind == .home || item.kind == .place || item.kind == .pinned) ? p.accent : p.textSecondary
            if item.kind == .tag {
                let dot = NSRect(x: r.minX + 8 + 2, y: r.midY - 6, width: 12, height: 12)
                if let c = item.color {
                    c.setFill()
                    NSBezierPath(ovalIn: dot).fill()
                } else {
                    p.textSecondary.setStroke()
                    NSBezierPath(ovalIn: dot.insetBy(dx: 0.5, dy: 0.5)).stroke()
                }
            } else if item.kind == .tree {
                // indented by depth; a chevron before the folders that have folders inside
                let x = r.minX + 4 + CGFloat(item.depth) * 14
                if item.expandable {
                    Draw.icon(item.expanded ? "chevron-down" : "chevron-right", in: NSRect(x: x, y: r.minY, width: 12, height: r.height), size: 12, color: p.textTertiary)
                }
                Draw.icon(item.icon, in: NSRect(x: x + 14, y: r.minY, width: 16, height: r.height), size: 16, color: p.accent)
                Draw.text(item.title, in: NSRect(x: x + 14 + 16 + 8, y: r.minY, width: max(r.maxX - (x + 38) - 4, 0), height: r.height), font: Metrics.textSm, color: p.text)
                continue
            } else {
                Draw.icon(item.icon, in: NSRect(x: r.minX + 8, y: r.minY, width: 16, height: r.height), size: 16, color: iconColor)
            }
            Draw.text(item.title, in: NSRect(x: r.minX + 8 + 16 + 8, y: r.minY, width: r.width - 40, height: r.height), font: Metrics.textSm, color: p.text)
        }
    }

    override func mouseUp(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        guard let i = index(at: p) else { return }
        // the tree's chevron opens and closes; the rest of the row goes there
        if items[i].kind == .tree, items[i].expandable, chevronRect(i).insetBy(dx: -4, dy: -6).contains(p) {
            owner?.toggle(items[i])
            return
        }
        owner?.clicked(items[i])
    }

    private func chevronRect(_ i: Int) -> NSRect {
        let r = rects()[i]
        return NSRect(x: r.minX + 4 + CGFloat(items[i].depth) * 14, y: r.midY - 6, width: 12, height: 12)
    }

    /// A place that is a folder (pinned, a drive, Applications, Library) gets the Windows app's
    /// sidebar menu (workspace.rs): open here / in a new tab / in a new window, pin or unpin,
    /// copy the path, Terminal; a drive that can be ejected also offers Eject. Home, Trash and
    /// Settings have none, as on Windows.
    override func menu(for event: NSEvent) -> NSMenu? {
        guard let i = index(at: convert(event.locationInWindow, from: nil)), let url = items[i].url, url.isFileURL,
            items[i].kind != .trash, let owner
        else { return nil }
        let item = items[i]
        let b = MenuBuilder()
        b.add(String(localized: "Open"), "square-arrow-out-up-right") { owner.clicked(item) }
        b.add(String(localized: "Open in new tab"), "square-plus") { owner.onOpenInNewTab?(url) }
        b.add(String(localized: "Open in new window"), "app-window") { owner.onOpenInNewWindow?(url) }
        b.separator()
        if item.kind != .drive {
            let pinned = MenuActions.isPinned(url)
            b.add(pinned ? String(localized: "Unpin from sidebar") : String(localized: "Pin to sidebar"), pinned ? "pin-off" : "pin") {
                MenuActions.setPinned(url, !pinned)
            }
        }
        b.add(String(localized: "Copy path"), "clipboard-copy") { MenuActions.copyPaths([url]) }
        b.add(String(localized: "Open in Terminal"), "square-terminal") { MenuActions.openInTerminal(url) }
        b.add(String(localized: "Properties"), "wrench", key: "i") { PropertiesWindow.show([url]) }
        if item.kind == .drive {
            let values = try? url.resourceValues(forKeys: [.volumeIsEjectableKey, .volumeIsRemovableKey, .volumeIsLocalKey, .volumeIsInternalKey])
            let ejectable = values?.volumeIsEjectable == true || values?.volumeIsRemovable == true || values?.volumeIsLocal == false
                || values?.volumeIsInternal == false
            if ejectable {
                b.separator()
                b.add(String(localized: "Eject"), "eject") { owner.onEject?(url) }
            }
        }
        return b.done()
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
