import AppKit

/// Columns (Finder-style Miller columns, folder_view.rs `render_columns`): the open folder is
/// the list's own column, 250 wide; up to two of its parents stand at its left, each marking
/// the folder on the way, and at its right the folder selected in it shows what is inside.
/// The side columns are read only: a folder clicked there is opened, a file is selected in
/// its folder, a double click opens it.
enum ColumnMetrics {
    static let width: CGFloat = 250
    static let rowH: CGFloat = 28
}

/// One side column: a folder's items, the one on the way marked.
@MainActor
final class MillerColumn: NSScrollView {
    /// A folder clicked: go there.
    var onOpenFolder: ((URL) -> Void)?
    /// A file clicked: its folder, with it selected.
    var onPick: ((URL) -> Void)?
    /// A right click on an item: its menu (folder_view.rs, issue 183).
    var onMenu: ((URL, Bool) -> NSMenu?)?
    var icon: ((FileItem, String) -> NSImage)?

    private(set) var dir: URL?
    private var folder: Folder?
    private var loadID = 0
    private let rows = MillerRows()

    override init(frame: NSRect) {
        super.init(frame: frame)
        drawsBackground = false
        hasVerticalScroller = true
        autohidesScrollers = true
        scrollerStyle = .overlay
        documentView = rows
        rows.column = self
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    /// The name marked in it (the folder on the way to the open one).
    var marked: String? { didSet { if marked != oldValue { rows.needsDisplay = true; scrollToMarked() } } }

    /// Lists `dir` (off the main thread); nil empties the column.
    func show(_ dir: URL?, showHidden: Bool) {
        guard dir != self.dir else { return }
        self.dir = dir
        folder = nil
        loadID += 1
        let id = loadID
        resize()
        guard let dir else { return }
        Task.detached(priority: .userInitiated) {
            let f = try? Folder.open(path: dir.path, column: .name, ascending: true, foldersFirst: true, showHidden: showHidden)
            await MainActor.run { [weak self] in
                guard let self, self.loadID == id else { return }
                self.folder = f
                self.resize()
                self.scrollToMarked()
            }
        }
    }

    var count: Int { Int(folder?.count() ?? 0) }

    func item(_ i: Int) -> FileItem? { folder?.item(row: UInt32(i)) }

    private func resize() {
        rows.frame = NSRect(x: 0, y: 0, width: contentSize.width, height: max(CGFloat(count) * ColumnMetrics.rowH + 8, contentSize.height))
        rows.needsDisplay = true
    }

    override func layout() {
        super.layout()
        resize()
    }

    private func scrollToMarked() {
        guard let marked, let folder, let r = folder.rowOf(name: marked) else { return }
        let y = 4 + CGFloat(r) * ColumnMetrics.rowH
        let visible = contentView.bounds
        if y < visible.minY || y + ColumnMetrics.rowH > visible.maxY {
            contentView.scroll(to: NSPoint(x: 0, y: max(y - visible.height / 2, 0)))
            reflectScrolledClipView(contentView)
        }
    }

    func clicked(_ i: Int, double: Bool) {
        guard let dir, let item = item(i) else { return }
        let url = dir.appendingPathComponent(item.name)
        if item.isDir && !(url.pathExtension == "app") {
            onOpenFolder?(url)
        } else if double {
            NSWorkspace.shared.open(url)
        } else {
            onPick?(url)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        // the divider at its right (`border_r_1`)
        Palette.of(self).divider.setFill()
        NSRect(x: bounds.maxX - 1, y: 0, width: 1, height: bounds.height).fill()
    }
}

@MainActor
final class MillerRows: HoverView {
    weak var column: MillerColumn?

    private func index(at p: NSPoint) -> Int? {
        guard let column, p.y >= 4 else { return nil }
        let i = Int((p.y - 4) / ColumnMetrics.rowH)
        return i < column.count ? i : nil
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let column else { return }
        let p = Palette.of(self)
        guard column.dir != nil else { return }
        if column.count == 0 {
            Draw.text(String(localized: "Empty"), in: NSRect(x: 16, y: 12, width: bounds.width - 32, height: 18), font: Metrics.textXs, color: p.textTertiary)
            return
        }
        let hot = hoverPoint.flatMap(index(at:))
        let first = max(Int((dirtyRect.minY - 4) / ColumnMetrics.rowH), 0)
        let last = min(Int((dirtyRect.maxY - 4) / ColumnMetrics.rowH) + 1, column.count)
        guard first < last else { return }
        for i in first..<last {
            guard let item = column.item(i), let dir = column.dir else { continue }
            let r = NSRect(x: 4, y: 4 + CGFloat(i) * ColumnMetrics.rowH, width: bounds.width - 9, height: ColumnMetrics.rowH)
            let on = column.marked == item.name
            if on || hot == i { Draw.roundedRect(r, radius: Metrics.radiusSmall, fill: p.hover) }
            let path = dir.appendingPathComponent(item.name).path
            if let image = column.icon?(item, path) {
                image.draw(in: NSRect(x: r.minX + 8, y: r.midY - 8, width: 16, height: 16), from: .zero, operation: .sourceOver, fraction: item.hidden || Clipboard.isCut(path) ? 0.5 : 1, respectFlipped: true, hints: nil)
            }
            let chevron = item.isDir && !item.name.hasSuffix(".app")
            let font = on ? Metrics.textSmMedium : Metrics.textSm
            Draw.text(item.name, in: NSRect(x: r.minX + 32, y: r.minY, width: r.width - 32 - (chevron ? 24 : 8), height: r.height), font: font, color: on ? p.text : p.textSecondary)
            if chevron { Draw.icon("chevron-right", in: NSRect(x: r.maxX - 20, y: r.minY, width: 12, height: r.height), size: 12, color: p.textTertiary) }
        }
    }

    override func mouseUp(with event: NSEvent) {
        guard let i = index(at: convert(event.locationInWindow, from: nil)) else { return }
        column?.clicked(i, double: event.clickCount >= 2)
    }

    /// A right click in another column opens that item's menu.
    override func menu(for event: NSEvent) -> NSMenu? {
        guard let column, let i = index(at: convert(event.locationInWindow, from: nil)), let dir = column.dir, let item = column.item(i) else { return nil }
        return column.onMenu?(dir.appendingPathComponent(item.name), item.isDir && !item.name.hasSuffix(".app"))
    }
}
