import AppKit

/// The disk usage page (disk_usage.rs, Location::DiskUsage): where the space under a folder
/// goes. The folder's items as a list, biggest first, with a bar each, and beside it a
/// squarified treemap (folders show their own items inside them, one level deep). A click on
/// a folder goes into it; Up and the path strip go back out. The walk is rf-mac's DiskScan.
@MainActor
final class DiskUsageView: FlippedView {
    var onShowInFolder: ((URL) -> Void)?
    var onOpenInNewTab: ((URL) -> Void)?

    private var scan: DiskScan?
    private var cur: [UInt32] = []
    private var items: [DiskItem] = []
    private var current: DiskItem?
    private var cells: [(rect: NSRect, item: DiskItem, head: Bool)] = []
    private var hot: [UInt32]?
    private var timer: Timer?

    private let up = IconButton(icon: "arrow-up", size: NSSize(width: 28, height: 28), iconSize: 15, tooltip: String(localized: "Up"))
    private let rescan = PlainButton(String(localized: "Scan again"), action: nil)
    private let listScroll = NSScrollView()
    private let list = DiskList()
    private let map = DiskMap()

    override init(frame: NSRect) {
        super.init(frame: frame)
        up.action = { [weak self] _ in self?.goUp() }
        rescan.action = { [weak self] in self?.restart() }
        list.owner = self
        map.owner = self
        listScroll.documentView = list
        listScroll.drawsBackground = false
        listScroll.hasVerticalScroller = true
        listScroll.scrollerStyle = .overlay
        [up, rescan, listScroll, map].forEach(addSubview)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    private(set) var root: URL?

    /// Shows the disk usage of `root`, with the walk the tab keeps (made when it has none).
    func show(_ root: URL, scan existing: DiskScan?) -> DiskScan {
        if self.root != root || scan !== existing {
            self.root = root
            cur = []
            scan = existing ?? DiskScan.start(root: root.path)
        }
        refresh()
        return scan!
    }

    private func restart() {
        guard let root else { return }
        scan?.cancel()
        scan = DiskScan.start(root: root.path)
        cur = []
        onRescan?(scan!)
        refresh()
    }

    var onRescan: ((DiskScan) -> Void)?

    /// Bumped whenever what is shown changes (the treemap lays itself out again).
    private(set) var version = 0

    private func refresh() {
        version += 1
        timer?.invalidate()
        guard let scan else { return }
        if !scan.isDone() {
            items = []
            current = nil
            timer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: false) { [weak self] _ in MainActor.assumeIsolated { self?.refresh() } }
        } else {
            current = scan.item(id: cur)
            items = scan.children(id: cur)
        }
        up.isEnabled = !cur.isEmpty
        list.needsDisplay = true
        needsLayout = true
        needsDisplay = true
        map.needsDisplay = true
    }

    override func viewDidHide() {
        super.viewDidHide()
        timer?.invalidate()
    }

    override func viewDidUnhide() {
        super.viewDidUnhide()
        refresh()
    }

    func open(_ item: DiskItem) {
        if item.isDir && item.hasChildren {
            cur = item.id
            refresh()
        } else if !item.isRest {
            onShowInFolder?(URL(fileURLWithPath: item.path))
        }
    }

    private func goUp() {
        guard !cur.isEmpty else { return }
        cur.removeLast()
        refresh()
    }

    func menu(for item: DiskItem) -> NSMenu? {
        guard !item.isRest else { return nil }
        let url = URL(fileURLWithPath: item.path)
        let b = MenuBuilder()
        if item.isDir && item.hasChildren { b.add(String(localized: "Look inside"), "folder-open") { [weak self] in self?.open(item) } }
        b.add(String(localized: "Show in folder"), "folder-search") { [weak self] in self?.onShowInFolder?(url) }
        if item.isDir { b.add(String(localized: "Open in new tab"), "square-plus") { [weak self] in self?.onOpenInNewTab?(url) } }
        b.separator()
        b.add(String(localized: "Copy item path"), "clipboard-copy") { MenuActions.copyPaths([url]) }
        b.add(String(localized: "Move to Trash"), "trash") { [weak self] in
            guard FileListController.confirmTrash([url]) else { return }
            DispatchQueue.global(qos: .userInitiated).async {
                let pairs = (try? FileOps.trash([url])) ?? []
                DispatchQueue.main.async {
                    MainActor.assumeIsolated {
                        FileUndo.shared.record(.moves(pairs), title: String(localized: "Move to Trash"))
                        self?.restart()
                    }
                }
            }
        }
        return b.done()
    }

    // MARK: drawing

    static func name(_ item: DiskItem) -> String { item.isRest ? String(localized: "\(item.files.formatted()) smaller files") : item.name }

    var rows: [DiskItem] { items }
    var total: UInt64 { current?.size ?? 0 }

    override func layout() {
        super.layout()
        let top: CGFloat = 64
        up.frame = NSRect(x: 16, y: 18, width: 28, height: 28)
        let rs = rescan.intrinsicContentSize
        rescan.frame = NSRect(x: bounds.width - 16 - rs.width, y: (top - rs.height) / 2, width: rs.width, height: rs.height)
        let listW = min(max(bounds.width * 0.42, 300), 460)
        listScroll.frame = NSRect(x: 8, y: top, width: listW, height: bounds.height - top - 8)
        list.frame = NSRect(x: 0, y: 0, width: listScroll.contentSize.width, height: max(CGFloat(items.count) * DiskList.rowH + 8, listScroll.contentSize.height))
        map.frame = NSRect(x: listW + 16, y: top, width: max(bounds.width - listW - 32, 0), height: bounds.height - top - 12)
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let x: CGFloat = 52
        let title: String
        let detail: String
        if let scan, !scan.isDone() {
            title = String(localized: "Disk usage — \(root?.lastPathComponent ?? "")")
            detail = String(localized: "Looking through… \(Int(scan.entries()).formatted()) items, \(formatSize(bytes: scan.bytes()))")
        } else if let current {
            let path = cur.isEmpty ? (root?.path ?? "") : current.path
            title = String(localized: "Disk usage — \(URL(fileURLWithPath: path).lastPathComponent)")
            detail = String(localized: "\(formatSize(bytes: current.size)) in \(Int(current.files).formatted()) files · \((path as NSString).abbreviatingWithTildeInPath)")
        } else {
            title = String(localized: "Disk usage")
            detail = ""
        }
        let w = bounds.width - x - 160
        Draw.text(title, in: NSRect(x: x, y: 12, width: w, height: 22), font: Metrics.textSmMedium, color: p.text)
        Draw.text(detail, in: NSRect(x: x, y: 34, width: w, height: 18), font: Metrics.textXs, color: p.textSecondary, truncation: .byTruncatingMiddle)
        p.divider.setFill()
        NSRect(x: 0, y: 63, width: bounds.width, height: 1).fill()
    }

    /// The treemap's rectangles for the shown folder: each item, and inside a folder that is
    /// big enough its own items under a name strip.
    func layoutCells(in box: NSRect) -> [(rect: NSRect, item: DiskItem, head: Bool)] {
        guard let scan, !items.isEmpty else { return [] }
        var out: [(NSRect, DiskItem, Bool)] = []
        let shown = items.filter { $0.size > 0 }
        let rects = treemapLayout(weights: shown.map(\.size), w: box.width, h: box.height)
        for (item, r) in zip(shown, rects) {
            let rect = NSRect(x: box.minX + r.x, y: box.minY + r.y, width: r.w, height: r.h)
            guard rect.width >= 3, rect.height >= 3 else { continue }
            if item.isDir && item.hasChildren && rect.width >= 70 && rect.height >= 46 {
                out.append((rect, item, true))
                let inner = NSRect(x: rect.minX + 2, y: rect.minY + 18, width: rect.width - 4, height: rect.height - 20)
                let kids = scan.children(id: item.id).filter { $0.size > 0 }
                let krs = treemapLayout(weights: kids.map(\.size), w: inner.width, h: inner.height)
                for (k, kr) in zip(kids, krs) {
                    let kr2 = NSRect(x: inner.minX + kr.x, y: inner.minY + kr.y, width: kr.w, height: kr.h)
                    if kr2.width >= 3 && kr2.height >= 3 { out.append((kr2, k, false)) }
                }
            } else {
                out.append((rect, item, false))
            }
        }
        return out
    }
}

/// The list: icon, name, a bar for the share, the size.
@MainActor
final class DiskList: HoverView {
    static let rowH: CGFloat = 34
    weak var owner: DiskUsageView?

    private func index(at p: NSPoint) -> Int? {
        let i = Int((p.y - 4) / Self.rowH)
        return (owner?.rows.indices.contains(i) ?? false) ? i : nil
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let owner else { return }
        let p = Palette.of(self)
        let total = max(owner.total, 1)
        let hot = hoverPoint.flatMap(index(at:))
        for (i, item) in owner.rows.enumerated() {
            let r = NSRect(x: 4, y: 4 + CGFloat(i) * Self.rowH, width: bounds.width - 8, height: Self.rowH - 2)
            guard r.intersects(dirtyRect) else { continue }
            if hot == i { Draw.roundedRect(r, radius: Metrics.radiusSmall, fill: p.hover) }
            let icon = item.isRest ? "files" : item.isDir ? "folder" : "file"
            Draw.icon(icon, in: NSRect(x: r.minX + 8, y: r.minY, width: 16, height: r.height), size: 16, color: item.isDir ? p.accent : p.textSecondary)
            let sizeText = formatSize(bytes: item.size)
            let sw: CGFloat = 76
            Draw.text(DiskUsageView.name(item), in: NSRect(x: r.minX + 32, y: r.minY + 2, width: r.width - 32 - sw - 8, height: 18), font: Metrics.textSm, color: p.text)
            Draw.text(sizeText, in: NSRect(x: r.maxX - sw - 8, y: r.minY + 2, width: sw, height: 18), font: Metrics.textXs, color: p.textSecondary, align: .right)
            // the share of the folder
            let share = CGFloat(Double(item.size) / Double(total))
            let bar = NSRect(x: r.minX + 32, y: r.minY + 24, width: r.width - 32 - 8, height: 3)
            Draw.roundedRect(bar, radius: 1.5, fill: p.divider)
            Draw.roundedRect(NSRect(x: bar.minX, y: bar.minY, width: max(bar.width * share, 2), height: 3), radius: 1.5, fill: p.accent)
        }
    }

    override func mouseUp(with event: NSEvent) {
        guard let i = index(at: convert(event.locationInWindow, from: nil)), let owner else { return }
        owner.open(owner.rows[i])
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        guard let i = index(at: convert(event.locationInWindow, from: nil)), let owner else { return nil }
        return owner.menu(for: owner.rows[i])
    }
}

/// The treemap.
@MainActor
final class DiskMap: HoverView {
    weak var owner: DiskUsageView?
    private var cells: [(rect: NSRect, item: DiskItem, head: Bool)] = []
    private var laidFor: NSSize = .zero
    private var laidVersion = -1

    override func draw(_ dirtyRect: NSRect) {
        guard let owner else { return }
        let p = Palette.of(self)
        if laidFor != bounds.size || laidVersion != owner.version {
            cells = owner.layoutCells(in: bounds)
            laidFor = bounds.size
            laidVersion = owner.version
        }
        let hotItem = hoverPoint.flatMap { pt in cells.last { $0.rect.contains(pt) } }?.item.id
        let colours: [NSColor] = [.systemBlue, .systemTeal, .systemIndigo, .systemPurple, .systemGreen, .systemOrange, .systemPink, .systemYellow]
        var topIndex = -1
        for c in cells {
            if c.item.id.count == (cells.first?.item.id.count ?? 0) { topIndex += 1 }
            let base = colours[max(topIndex, 0) % colours.count]
            let r = c.rect.insetBy(dx: 1, dy: 1)
            let fill: NSColor
            if c.head {
                fill = base.withAlphaComponent(p.dark ? 0.22 : 0.16)
            } else {
                fill = base.withAlphaComponent(c.item.isDir ? (p.dark ? 0.55 : 0.45) : (p.dark ? 0.38 : 0.3))
            }
            Draw.roundedRect(r, radius: 3, fill: hotItem == c.item.id ? fill.blended(withFraction: 0.25, of: .white) ?? fill : fill)
            let label = DiskUsageView.name(c.item)
            if c.head {
                Draw.text("\(label)  \(formatSize(bytes: c.item.size))", in: NSRect(x: r.minX + 6, y: r.minY + 1, width: r.width - 12, height: 16), font: Metrics.textXsSemibold, color: p.text)
            } else if r.width > 50 && r.height > 22 {
                Draw.text(label, in: NSRect(x: r.minX + 5, y: r.minY + 3, width: r.width - 10, height: 15), font: Metrics.textXs, color: p.text)
                if r.height > 38 {
                    Draw.text(formatSize(bytes: c.item.size), in: NSRect(x: r.minX + 5, y: r.minY + 18, width: r.width - 10, height: 15), font: Metrics.textXs, color: p.textSecondary)
                }
            }
        }
    }

    private func cell(at event: NSEvent) -> DiskItem? {
        let pt = convert(event.locationInWindow, from: nil)
        return cells.last { $0.rect.contains(pt) }?.item
    }

    override func mouseUp(with event: NSEvent) {
        guard let item = cell(at: event), let owner else { return }
        owner.open(item)
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        guard let item = cell(at: event) else { return nil }
        return owner?.menu(for: item)
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        needsDisplay = true
    }
}
