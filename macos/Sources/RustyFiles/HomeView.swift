import AppKit

/// The Home page, in place of the folder in the content card (folder_view.rs `render_home`):
/// Quick access, Drives and Recent, one under the other, 24 from the card's edges and 16 apart.
/// No command bar, no status line.
@MainActor
final class HomeView: FlippedView {
    var onOpenFolder: ((URL) -> Void)?

    private let scroll = NSScrollView()
    private let content = FlippedView()
    private var quick: [HomeTile] = []
    private var drives: [HomeTile] = []
    private var tagTiles: [HomeTile] = []
    private var recent: [RecentRow] = []
    private var headers: [HomeHeader] = []
    private let clear = PlainLinkButton(String(localized: "Clear"))
    private var query: NSMetadataQuery?

    override init(frame: NSRect) {
        super.init(frame: frame)
        scroll.documentView = content
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.scrollerStyle = .overlay
        addSubview(scroll)
        clear.action = { [weak self] in
            UserDefaults.standard.set(Date().timeIntervalSince1970, forKey: "recentClearedAt")
            self?.reloadRecent()
        }
        let center = NSWorkspace.shared.notificationCenter
        for name in [NSWorkspace.didMountNotification, NSWorkspace.didUnmountNotification, NSWorkspace.didRenameVolumeNotification] {
            center.addObserver(self, selector: #selector(volumesChanged), name: name, object: nil)
        }
        NotificationCenter.default.addObserver(self, selector: #selector(volumesChanged), name: .rfPinsChanged, object: nil)
        NotificationCenter.default.addObserver(forName: .rfSettingsChanged, object: nil, queue: .main) { [weak self] note in
            MainActor.assumeIsolated { if ["homeHidden", "tagList"].contains(AppSettings.changedKey(note)) { self?.reload() } }
        }
        reload()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    @objc private func volumesChanged() { reload() }

    /// Lists everything again (F5 on Home, as on Windows).
    func reload() {
        content.subviews.forEach { $0.removeFromSuperview() }
        headers = []
        quick = !AppSettings.showsHome(.quick) ? [] : Self.quickAccess().map { place in
            HomeTile(kind: .folder(icon: place.icon), title: place.title, url: place.url) { [weak self] in self?.onOpenFolder?(place.url) }
        }
        drives = !AppSettings.showsHome(.drives) ? [] : Self.volumes().map { v in
            HomeTile(kind: .drive(icon: v.icon, used: v.used, detail: v.detail), title: v.title, url: v.url) { [weak self] in self?.onOpenFolder?(v.url) }
        }
        tagTiles = !AppSettings.showsHome(.tags) ? [] : TagStore.all.map { t in
            HomeTile(kind: .folder(icon: "tag"), title: t.name, url: TagStore.url(t.name)) { [weak self] in self?.onOpenFolder?(TagStore.url(t.name)) }
        }
        headers.append(HomeHeader(String(localized: "Quick access")))
        headers.append(HomeHeader(String(localized: "Drives")))
        headers.append(HomeHeader(String(localized: "Tags")))
        (quick + drives + tagTiles).forEach(content.addSubview)
        headers.forEach(content.addSubview)
        reloadRecent()
    }

    // MARK: data

    struct Place {
        let title: String
        let icon: String
        let url: URL
    }

    /// The pinned folders (the usual places until the user pins others), known ones with
    /// their own icon (places.rs).
    static func quickAccess() -> [Place] {
        let fm = FileManager.default
        var known: [String: String] = [:]
        for (dir, icon) in [
            (FileManager.SearchPathDirectory.desktopDirectory, "monitor"), (.downloadsDirectory, "download"),
            (.documentDirectory, "file-text"), (.picturesDirectory, "image"), (.musicDirectory, "music"), (.moviesDirectory, "video"),
        ] {
            if let u = fm.urls(for: dir, in: .userDomainMask).first { known[u.standardizedFileURL.path] = icon }
        }
        return Pins.load().map { u in
            Place(title: fm.displayName(atPath: u.path), icon: known[u.standardizedFileURL.path] ?? "folder", url: u)
        }
    }

    struct Volume {
        let title: String
        let icon: String
        let url: URL
        let used: Double?
        let detail: String
    }

    static func volumes() -> [Volume] {
        let keys: [URLResourceKey] = [
            .volumeLocalizedNameKey, .volumeIsLocalKey, .volumeIsRemovableKey, .volumeIsEjectableKey, .volumeIsInternalKey,
            .volumeTotalCapacityKey, .volumeAvailableCapacityForImportantUsageKey, .volumeAvailableCapacityKey,
        ]
        let urls = FileManager.default.mountedVolumeURLs(includingResourceValuesForKeys: keys, options: [.skipHiddenVolumes]) ?? []
        return urls.map { url in
            let v = try? url.resourceValues(forKeys: Set(keys))
            let icon: String
            if v?.volumeIsLocal == false {
                icon = "network"
            } else if v?.volumeIsRemovable == true || v?.volumeIsEjectable == true || v?.volumeIsInternal == false {
                icon = "usb"
            } else {
                icon = "hard-drive"
            }
            let total = Int64(v?.volumeTotalCapacity ?? 0)
            let free = v?.volumeAvailableCapacityForImportantUsage ?? Int64(v?.volumeAvailableCapacity ?? 0)
            let used: Double? = total > 0 ? 1 - Double(free) / Double(total) : nil
            let detail = total > 0
                ? String(localized: "\(formatSize(bytes: UInt64(max(free, 0)))) free / \(formatSize(bytes: UInt64(total)))") : ""
            return Volume(title: v?.volumeLocalizedName ?? url.lastPathComponent, icon: icon, url: url, used: used, detail: detail)
        }
    }

    /// Recently used items under the home folder, newest first, at most 14 (Spotlight's "last
    /// used" date, as Finder's Recents; the Windows app reads Windows' Recent folder).
    private func reloadRecent() {
        query?.stop()
        guard AppSettings.showsHome(.recent) else { return showRecent([]) }
        let q = NSMetadataQuery()
        let since = max(Date().addingTimeInterval(-30 * 86_400).timeIntervalSince1970, UserDefaults.standard.double(forKey: "recentClearedAt"))
        q.predicate = NSPredicate(format: "kMDItemLastUsedDate >= %@", Date(timeIntervalSince1970: since) as NSDate)
        q.sortDescriptors = [NSSortDescriptor(key: "kMDItemLastUsedDate", ascending: false)]
        q.searchScopes = [NSMetadataQueryUserHomeScope]
        NotificationCenter.default.addObserver(self, selector: #selector(recentFound(_:)), name: .NSMetadataQueryDidFinishGathering, object: q)
        query = q
        q.start()
        showRecent([])
    }

    @objc private func recentFound(_ note: Notification) {
        guard let q = note.object as? NSMetadataQuery, q === query else { return }
        q.disableUpdates()
        var items: [(URL, Date)] = []
        var seen = Set<String>()
        for i in 0..<q.resultCount {
            guard let r = q.result(at: i) as? NSMetadataItem, let path = r.value(forAttribute: NSMetadataItemPathKey) as? String,
                let date = r.value(forAttribute: "kMDItemLastUsedDate") as? Date
            else { continue }
            // and nothing that is gone
            // nothing inside the Library, no apps (Recent is files and folders), nothing twice
            if path.contains("/Library/") || path.contains(".app/") || path.hasSuffix(".app") || !seen.insert(path).inserted { continue }
            guard FileManager.default.fileExists(atPath: path) else { continue }
            items.append((URL(fileURLWithPath: path), date))
            if items.count == 14 { break }
        }
        q.stop()
        showRecent(items)
    }

    private func showRecent(_ items: [(URL, Date)]) {
        recent.forEach { $0.removeFromSuperview() }
        recent = items.map { url, date in
            RecentRow(url: url, date: date) { [weak self] in
                var isDir: ObjCBool = false
                if FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir), isDir.boolValue,
                    !NSWorkspace.shared.isFilePackage(atPath: url.path)
                {
                    self?.onOpenFolder?(url)
                } else {
                    NSWorkspace.shared.open(url)
                }
            }
        }
        recent.forEach(content.addSubview)
        if items.isEmpty {
            clear.removeFromSuperview()
            headers.filter { $0.isRecent }.forEach { $0.removeFromSuperview() }
            headers.removeAll { $0.isRecent }
        } else if !headers.contains(where: \.isRecent) {
            let h = HomeHeader(String(localized: "Recent"), isRecent: true)
            headers.append(h)
            content.addSubview(h)
            content.addSubview(clear)
        }
        needsLayout = true
    }

    // MARK: layout

    /// `tile_cols`: as many equal columns as fit between the tile's least and most width.
    private static func columns(_ avail: CGFloat, min: CGFloat, max: CGFloat) -> Int {
        let byMax = Int(ceil((avail + 12) / (max + 12)))
        let byMin = Int(floor((avail + 12) / (min + 12)))
        return Swift.max(1, Swift.min(byMax, byMin))
    }

    override func layout() {
        super.layout()
        scroll.frame = bounds
        let avail = max(scroll.contentSize.width - 48, 100)
        var y: CGFloat = 24
        func header(_ i: Int) {
            guard headers.indices.contains(i) else { return }
            headers[i].frame = NSRect(x: 24, y: y, width: min(avail, 900), height: 20)
            y += 20 + 16
        }
        func grid(_ tiles: [HomeTile], min: CGFloat, max: CGFloat, height: CGFloat) {
            let cols = Self.columns(avail, min: min, max: max)
            let w = (avail - CGFloat(cols - 1) * 12) / CGFloat(cols)
            for (i, t) in tiles.enumerated() {
                t.frame = NSRect(x: 24 + CGFloat(i % cols) * (w + 12), y: y + CGFloat(i / cols) * (height + 12), width: w, height: height)
            }
            let rows = (tiles.count + cols - 1) / cols
            y += CGFloat(rows) * height + CGFloat(Swift.max(rows - 1, 0)) * 12 + 16
        }
        headers[0].isHidden = quick.isEmpty
        headers[1].isHidden = drives.isEmpty
        if !quick.isEmpty {
            header(0)
            grid(quick, min: 140, max: 220, height: 80)
            y += 8 + 16
        }
        if !drives.isEmpty {
            header(1)
            grid(drives, min: 240, max: 340, height: 72)
        }
        headers[2].isHidden = tagTiles.isEmpty
        if !tagTiles.isEmpty {
            y += 8 + 16
            header(2)
            grid(tagTiles, min: 140, max: 220, height: 56)
        }
        if let i = headers.firstIndex(where: \.isRecent) {
            y += 8 + 16
            header(i)
            let hw = min(avail, 900)
            let cw = clear.intrinsicContentSize.width
            clear.frame = NSRect(x: 24 + hw - cw, y: headers[i].frame.minY - 2, width: cw, height: 24)
            for r in recent {
                r.frame = NSRect(x: 24, y: y, width: hw, height: 30)
                y += 32
            }
        }
        content.frame = NSRect(x: 0, y: 0, width: scroll.contentSize.width, height: max(y + 24, scroll.contentSize.height))
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        func redraw(_ v: NSView) {
            v.needsDisplay = true
            v.subviews.forEach(redraw)
        }
        redraw(self)
    }
}

/// A section header: `text_sm` semibold.
final class HomeHeader: FlippedView {
    let title: String
    let isRecent: Bool

    init(_ title: String, isRecent: Bool = false) {
        self.title = title
        self.isRecent = isRecent
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ dirtyRect: NSRect) {
        Draw.text(title, in: bounds, font: NSFont.systemFont(ofSize: 14, weight: .semibold), color: Palette.of(self).text)
    }
}

/// A Home card (`p_3 gap_2`, radius 8, `tile_bg`, 1 px border, `tile_hover` on hover).
/// A folder: a 28 px accent icon over its name. A drive: its icon beside the name, a 6 px
/// usage bar (red above 90 %) and "X free / Y".
@MainActor
final class HomeTile: HoverView {
    enum Kind {
        case folder(icon: String)
        case drive(icon: String, used: Double?, detail: String)
    }

    let kind: Kind
    let title: String
    let url: URL
    let action: () -> Void

    init(kind: Kind, title: String, url: URL, action: @escaping () -> Void) {
        self.kind = kind
        self.title = title
        self.url = url
        self.action = action
        super.init(frame: .zero)
        toolTip = url.path
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var mouseDownCanMoveWindow: Bool { false }
    override func resetCursorRects() { addCursorRect(bounds, cursor: .pointingHand) }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let r = bounds.insetBy(dx: 0.5, dy: 0.5)
        let path = NSBezierPath(roundedRect: r, xRadius: Metrics.radius, yRadius: Metrics.radius)
        (hovered ? p.tileHover : p.tileBg).setFill()
        path.fill()
        p.border.setStroke()
        path.stroke()
        let inner = bounds.insetBy(dx: 12, dy: 12)
        switch kind {
        case .folder(let icon):
            Draw.icon(icon, in: NSRect(x: inner.minX, y: inner.minY, width: 28, height: 28), size: 28, color: p.accent)
            Draw.text(title, in: NSRect(x: inner.minX, y: inner.minY + 28 + 8, width: inner.width, height: 20), font: Metrics.textSm, color: p.text)
        case .drive(let icon, let used, let detail):
            Draw.icon(icon, in: NSRect(x: inner.minX, y: inner.minY, width: 28, height: inner.height), size: 28, color: p.textSecondary)
            let x = inner.minX + 28 + 12
            let w = inner.width - 28 - 12
            Draw.text(title, in: NSRect(x: x, y: inner.minY, width: w, height: 18), font: Metrics.textSm, color: p.text)
            let bar = NSRect(x: x, y: inner.minY + 18 + 4, width: w, height: 6)
            Draw.roundedRect(bar, radius: 3, fill: p.pressed)
            if let used {
                let fill = NSRect(x: bar.minX, y: bar.minY, width: bar.width * CGFloat(min(max(used, 0), 1)), height: 6)
                Draw.roundedRect(fill, radius: 3, fill: used > 0.9 ? p.danger : p.accent)
            }
            Draw.text(detail, in: NSRect(x: x, y: bar.maxY + 4, width: w, height: 16), font: Metrics.textXs, color: p.textSecondary)
        }
    }

    override func mouseUp(with event: NSEvent) {
        if bounds.contains(convert(event.locationInWindow, from: nil)) { action() }
    }
}

/// A Recent row (30 high, `px_2 gap_3`, radius 6, hover fill): the icon, the name in a 260
/// column, the folder it is in (tertiary), the date in a 130 column (secondary).
@MainActor
final class RecentRow: HoverView {
    let url: URL
    let date: Date
    let action: () -> Void
    private lazy var icon = NSWorkspace.shared.icon(forFile: url.path)

    init(url: URL, date: Date, action: @escaping () -> Void) {
        self.url = url
        self.date = date
        self.action = action
        super.init(frame: .zero)
        toolTip = url.path
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var mouseDownCanMoveWindow: Bool { false }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        if hovered { Draw.roundedRect(bounds, radius: Metrics.radiusSmall, fill: p.hover) }
        var x: CGFloat = 8
        icon.draw(in: NSRect(x: x, y: (bounds.height - 16) / 2, width: 16, height: 16), from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
        x += 16 + 12
        let name = FileManager.default.displayName(atPath: url.path)
        Draw.text(name, in: NSRect(x: x, y: 0, width: 260, height: bounds.height), font: Metrics.textSm, color: p.text)
        x += 260 + 12
        let dateW: CGFloat = 130
        let folderW = max(bounds.width - x - 12 - dateW - 8, 0)
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        var folder = url.deletingLastPathComponent().path
        if folder.hasPrefix(home) { folder = "~" + folder.dropFirst(home.count) }
        Draw.text(folder, in: NSRect(x: x, y: 0, width: folderW, height: bounds.height), font: Metrics.textSm, color: p.textTertiary, truncation: .byTruncatingMiddle)
        let ms = Int64(date.timeIntervalSince1970 * 1000)
        Draw.text(ItemDisplay.date(ms, style: AppSettings.dateStyle), in: NSRect(x: bounds.width - 8 - dateW, y: 0, width: dateW, height: bounds.height), font: Metrics.textSm, color: p.textSecondary)
    }

    override func mouseUp(with event: NSEvent) {
        if bounds.contains(convert(event.locationInWindow, from: nil)) { action() }
    }
}

/// "Clear" beside the Recent header (`px_2 py_0p5`, `text_xs`, secondary; hover fill and text colour).
@MainActor
final class PlainLinkButton: HoverView {
    let title: String
    var action: (() -> Void)?

    init(_ title: String) {
        self.title = title
        super.init(frame: .zero)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override var mouseDownCanMoveWindow: Bool { false }
    override var intrinsicContentSize: NSSize { NSSize(width: Draw.textWidth(title, font: Metrics.textXs) + 16, height: 24) }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        if hovered { Draw.roundedRect(bounds, radius: Metrics.radiusSmall, fill: p.hover) }
        Draw.text(title, in: bounds, font: Metrics.textXs, color: hovered ? p.text : p.textSecondary, align: .center)
    }

    override func mouseUp(with event: NSEvent) {
        if bounds.contains(convert(event.locationInWindow, from: nil)) { action?() }
    }
}
