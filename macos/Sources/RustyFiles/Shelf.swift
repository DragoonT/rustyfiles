import AppKit

extension Notification.Name {
    static let rfShelfChanged = Notification.Name("rustyfiles.shelfChanged")
}

/// The shelf (rf-core shelf.rs, Files' "Shelf"): items collected from different folders, to be
/// copied or moved together into the folder that is open. One shelf for every window, kept
/// across starts; items that are gone fall off it.
@MainActor
enum ShelfStore {
    private static let key = "shelfItems"
    private(set) static var items: [URL] = (UserDefaults.standard.stringArray(forKey: key) ?? []).map { URL(fileURLWithPath: $0) }

    private static func save() {
        UserDefaults.standard.set(items.map(\.path), forKey: key)
        NotificationCenter.default.post(name: .rfShelfChanged, object: nil)
    }

    /// Adds `urls` (not twice); returns how many were new.
    @discardableResult
    static func add(_ urls: [URL]) -> Int {
        let have = Set(items.map(\.standardizedFileURL.path))
        let new = urls.filter { $0.isFileURL && !have.contains($0.standardizedFileURL.path) }
        guard !new.isEmpty else { return 0 }
        items += new
        save()
        return new.count
    }

    static func remove(_ url: URL) {
        items.removeAll { $0 == url }
        save()
    }

    static func clear() {
        items = []
        save()
    }

    static func prune() {
        let kept = items.filter { FileManager.default.fileExists(atPath: $0.path) }
        if kept.count != items.count {
            items = kept
            save()
        }
    }
}

/// The shelf pane at the right of the list (shelf_view.rs): "Shelf" and the count, the items
/// (name and folder, an × on the one under the pointer), and Copy here / Move here / Clear.
/// Files dropped on it are added; items dragged from it go anywhere.
@MainActor
final class ShelfPane: FlippedView {
    var onTransfer: ((Transfer.Kind, [URL]) -> Void)?
    var onReveal: ((URL) -> Void)?
    var onClose: (() -> Void)?

    private let close = IconButton(icon: "x", size: NSSize(width: 24, height: 24), iconSize: 14, tooltip: String(localized: "Close"))
    private let scroll = NSScrollView()
    private let rows = ShelfRows()
    private let copy = PlainButton(String(localized: "Copy here"), action: nil)
    private let move = PlainButton(String(localized: "Move here"), action: nil)
    private let clear = PlainButton(String(localized: "Clear"), action: nil)
    private let empty = NSTextField(wrappingLabelWithString: String(localized: "Collect items here with “Add to shelf” in their menu (⇧⌘S), or by dropping them here; then copy or move them all into the folder you are in."))

    override init(frame: NSRect) {
        super.init(frame: frame)
        close.tint = { $0.textSecondary }
        close.action = { [weak self] _ in self?.onClose?() }
        copy.action = { [weak self] in self?.onTransfer?(.copy, ShelfStore.items) }
        move.action = { [weak self] in self?.onTransfer?(.move, ShelfStore.items) }
        clear.action = { ShelfStore.clear() }
        rows.owner = self
        scroll.documentView = rows
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.scrollerStyle = .overlay
        empty.font = Metrics.textXs
        empty.alignment = .center
        [close, scroll, copy, move, clear, empty].forEach(addSubview)
        registerForDraggedTypes([.fileURL])
        NotificationCenter.default.addObserver(self, selector: #selector(changed), name: .rfShelfChanged, object: nil)
        changed()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    @objc private func changed() {
        let none = ShelfStore.items.isEmpty
        empty.isHidden = !none
        [copy, move, clear].forEach { $0.isHidden = none }
        needsLayout = true
        needsDisplay = true
        rows.needsDisplay = true
    }

    override func viewDidUnhide() {
        super.viewDidUnhide()
        ShelfStore.prune()
    }

    override func layout() {
        super.layout()
        close.frame = NSRect(x: bounds.width - 8 - 24, y: 10, width: 24, height: 24)
        let bottom: CGFloat = ShelfStore.items.isEmpty ? 0 : 84
        scroll.frame = NSRect(x: 1, y: 44, width: bounds.width - 1, height: bounds.height - 44 - bottom)
        rows.frame = NSRect(x: 0, y: 0, width: scroll.contentSize.width, height: max(CGFloat(ShelfStore.items.count) * ShelfRows.rowH + 8, scroll.contentSize.height))
        let bw = (bounds.width - 12 * 2 - 8) / 2
        let y = bounds.height - bottom + 10
        copy.frame = NSRect(x: 12, y: y, width: bw, height: 30)
        move.frame = NSRect(x: 12 + bw + 8, y: y, width: bw, height: 30)
        clear.frame = NSRect(x: 12, y: y + 36, width: bounds.width - 24, height: 30)
        empty.preferredMaxLayoutWidth = bounds.width - 40
        let eh = empty.sizeThatFits(NSSize(width: bounds.width - 40, height: 300)).height
        empty.frame = NSRect(x: 20, y: bounds.height / 2 - eh / 2, width: bounds.width - 40, height: eh)
        empty.textColor = Palette.of(self).textSecondary
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        p.divider.setFill()
        NSRect(x: 0, y: 0, width: 1, height: bounds.height).fill()
        Draw.text(String(localized: "Shelf"), in: NSRect(x: 14, y: 12, width: 120, height: 20), font: Metrics.textSmMedium, color: p.text)
        let n = ShelfStore.items.count
        if n > 0 {
            let w = Draw.textWidth(String(localized: "Shelf"), font: Metrics.textSmMedium)
            Draw.text("\(n)", in: NSRect(x: 14 + w + 8, y: 12, width: 60, height: 20), font: Metrics.textXs, color: p.textTertiary)
        }
        if dropping {
            p.accent.setStroke()
            let r = NSBezierPath(roundedRect: bounds.insetBy(dx: 4, dy: 4), xRadius: Metrics.radius, yRadius: Metrics.radius)
            r.lineWidth = 2
            r.stroke()
        }
    }

    // MARK: dropping files on the shelf

    private var dropping = false { didSet { needsDisplay = true } }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        guard !FileListController.droppedURLs(sender).isEmpty, (sender.draggingSource as? ShelfRows) == nil else { return [] }
        dropping = true
        return .link
    }

    override func draggingExited(_ sender: NSDraggingInfo?) { dropping = false }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        dropping = false
        return ShelfStore.add(FileListController.droppedURLs(sender)) > 0
    }

    func reveal(_ url: URL) { onReveal?(url) }
}

/// The shelf's items.
@MainActor
final class ShelfRows: HoverView, NSDraggingSource {
    static let rowH: CGFloat = 40
    weak var owner: ShelfPane?

    private func index(at p: NSPoint) -> Int? {
        let i = Int((p.y - 4) / Self.rowH)
        return ShelfStore.items.indices.contains(i) && p.y >= 4 ? i : nil
    }

    private func rowRect(_ i: Int) -> NSRect { NSRect(x: 6, y: 4 + CGFloat(i) * Self.rowH, width: bounds.width - 12, height: Self.rowH - 2) }
    private func removeRect(_ i: Int) -> NSRect { let r = rowRect(i); return NSRect(x: r.maxX - 26, y: r.midY - 10, width: 20, height: 20) }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let hot = hoverPoint.flatMap(index(at:))
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        for (i, url) in ShelfStore.items.enumerated() {
            let r = rowRect(i)
            guard r.intersects(dirtyRect) else { continue }
            if hot == i { Draw.roundedRect(r, radius: Metrics.radiusSmall, fill: p.hover) }
            let icon = NSWorkspace.shared.icon(forFile: url.path)
            icon.draw(in: NSRect(x: r.minX + 6, y: r.midY - 12, width: 24, height: 24))
            let tw = r.width - 40 - (hot == i ? 28 : 6)
            Draw.text(url.lastPathComponent, in: NSRect(x: r.minX + 38, y: r.minY + 3, width: tw, height: 18), font: Metrics.textSm, color: p.text, truncation: .byTruncatingMiddle)
            var dir = url.deletingLastPathComponent().path
            if dir.hasPrefix(home) { dir = "~" + dir.dropFirst(home.count) }
            Draw.text(dir, in: NSRect(x: r.minX + 38, y: r.minY + 20, width: tw, height: 15), font: Metrics.textXs, color: p.textSecondary, truncation: .byTruncatingMiddle)
            if hot == i { Draw.icon("x", in: removeRect(i), size: 13, color: p.textSecondary) }
        }
    }

    private var downAt: NSPoint?

    override func mouseDown(with event: NSEvent) {
        downAt = convert(event.locationInWindow, from: nil)
    }

    override func mouseDragged(with event: NSEvent) {
        guard let start = downAt, let i = index(at: start) else { return }
        let pt = convert(event.locationInWindow, from: nil)
        guard hypot(pt.x - start.x, pt.y - start.y) > 4 else { return }
        downAt = nil
        let url = ShelfStore.items[i]
        let item = NSDraggingItem(pasteboardWriter: url as NSURL)
        let icon = NSWorkspace.shared.icon(forFile: url.path)
        item.setDraggingFrame(NSRect(x: pt.x - 16, y: pt.y - 16, width: 32, height: 32), contents: icon)
        beginDraggingSession(with: [item], event: event, source: self)
    }

    func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        context == .outsideApplication ? [.copy, .generic] : [.copy, .move, .generic]
    }

    override func mouseUp(with event: NSEvent) {
        defer { downAt = nil }
        let pt = convert(event.locationInWindow, from: nil)
        guard downAt != nil, let i = index(at: pt) else { return }
        if removeRect(i).insetBy(dx: -4, dy: -4).contains(pt) {
            ShelfStore.remove(ShelfStore.items[i])
        } else if event.clickCount == 2 {
            owner?.reveal(ShelfStore.items[i])
        }
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        guard let i = index(at: convert(event.locationInWindow, from: nil)) else { return nil }
        let url = ShelfStore.items[i]
        let b = MenuBuilder()
        b.add(String(localized: "Show in folder"), "folder-search") { [weak self] in self?.owner?.reveal(url) }
        b.add(String(localized: "Remove from shelf"), "x") { ShelfStore.remove(url) }
        return b.done()
    }
}
