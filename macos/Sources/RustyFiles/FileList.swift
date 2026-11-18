import AppKit
import Quartz

// MARK: - command bar

/// The bar at the top of the content card (workspace.rs `command_bar`): New ▾ | cut, copy,
/// paste, rename, share, delete, properties … filter, sort ▾, view ▾ | preview pane.
@MainActor
final class CommandBarView: FlippedView {
    let new = IconButton(icon: "circle-plus", size: NSSize(width: 0, height: 32), tooltip: String(localized: "New"))
    let cut = IconButton(icon: "scissors", size: NSSize(width: 36, height: 32), tooltip: String(localized: "Cut"))
    let copy = IconButton(icon: "copy", size: NSSize(width: 36, height: 32), tooltip: String(localized: "Copy"))
    let paste = IconButton(icon: "clipboard-paste", size: NSSize(width: 36, height: 32), tooltip: String(localized: "Paste"))
    let rename = IconButton(icon: "text-cursor-input", size: NSSize(width: 36, height: 32), tooltip: String(localized: "Rename"))
    let share = IconButton(icon: "share-2", size: NSSize(width: 36, height: 32), tooltip: String(localized: "Share"))
    let delete = IconButton(icon: "trash", size: NSSize(width: 36, height: 32), tooltip: String(localized: "Delete"))
    let properties = IconButton(icon: "wrench", size: NSSize(width: 36, height: 32), tooltip: String(localized: "Properties"))
    let filter = IconButton(icon: "list-filter", size: NSSize(width: 36, height: 32), tooltip: String(localized: "Filter"))
    let sort = IconButton(icon: "arrow-up-down", size: NSSize(width: 0, height: 32), tooltip: String(localized: "Sort"))
    let view = IconButton(icon: "layout-grid", size: NSSize(width: 0, height: 32), tooltip: String(localized: "View"))
    let preview = IconButton(icon: "panel-right", size: NSSize(width: 36, height: 32), tooltip: String(localized: "Preview pane"))
    private let sepLeft = Divider()
    private let sepRight = Divider()

    override init(frame: NSRect) {
        super.init(frame: frame)
        new.label = String(localized: "New")
        new.chevron = true
        sort.chevron = true
        view.chevron = true
        [new, sepLeft, cut, copy, paste, rename, share, delete, properties, filter, sort, view, sepRight, preview].forEach(addSubview)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        let h = bounds.height - 1
        func place(_ v: NSView, _ x: CGFloat, _ w: CGFloat, _ height: CGFloat = 32) {
            v.frame = NSRect(x: x, y: (h - height) / 2, width: w, height: height)
        }
        // px_2, gap_0p5; separators 1×20 with mx_1
        var x: CGFloat = 8
        place(new, x, new.intrinsicContentSize.width)
        x += new.frame.width + 2 + 4
        place(sepLeft, x, 1, 20)
        x += 1 + 4 + 2
        for b in [cut, copy, paste, rename, share, delete, properties] {
            place(b, x, 36)
            x += 36 + 2
        }
        var r = bounds.width - 8
        r -= 36
        place(preview, r, 36)
        r -= 2 + 4 + 1
        place(sepRight, r, 1, 20)
        r -= 4 + 2
        for b in [view, sort] {
            let w = b.intrinsicContentSize.width
            r -= w
            place(b, r, w)
            r -= 2
        }
        r -= 36
        place(filter, r, 36)
    }

    override func draw(_ dirtyRect: NSRect) {
        Palette.of(self).divider.setFill()
        NSRect(x: 0, y: bounds.height - 1, width: bounds.width, height: 1).fill()
    }

    override func viewDidChangeEffectiveAppearance() { needsDisplay = true }
}

// MARK: - columns

/// The details columns, in the Windows app's default order and widths (folder_view.rs,
/// settings.rs): Name (flex) · Date modified 150 · Type 150 · Size 100, right-aligned.
enum ListColumn: String, CaseIterable {
    case name, modified, type, size

    var title: String {
        switch self {
        case .name: String(localized: "Name")
        case .modified: String(localized: "Date modified")
        case .type: String(localized: "Type")
        case .size: String(localized: "Size")
        }
    }

    var defaultWidth: CGFloat {
        switch self {
        case .name: 320
        case .modified, .type: 150
        case .size: 100
        }
    }

    var sort: SortColumn {
        switch self {
        case .name: .name
        case .modified: .modified
        case .type: .kind
        case .size: .size
        }
    }

    /// A new sort key starts newest / biggest first, as on Windows (rf-core `SortSpec::toggled`).
    var startsAscending: Bool { self == .name || self == .type }
}

/// Where a column's content box lies inside its table column. The header and the rows share
/// it so that labels and cells line up: the list has 10 px before the first column (body
/// `pl 6` + row `px 4`) and `10 + gutter` after the last.
struct ListGeometry {
    var gutter: CGFloat = 24
    static let lead: CGFloat = 10
    var trail: CGFloat { 10 + gutter }

    func box(of column: Int, count: Int, in rect: NSRect) -> NSRect {
        var r = rect
        if column == 0 { r.origin.x += Self.lead; r.size.width -= Self.lead }
        if column == count - 1 { r.size.width -= trail }
        return r
    }
}

// MARK: - header

/// The column header (folder_view.rs `header`): 30 high, `text_xs`, the sort column in the
/// text colour with a chevron after its label, hover fill per column, and an 8 px resize
/// handle on each column's left edge with a 1 px divider in it.
@MainActor
final class ListHeaderView: HoverView {
    weak var table: NSTableView?
    var geometry = ListGeometry() { didSet { needsDisplay = true } }
    var sortKey: ListColumn = .name { didSet { needsDisplay = true } }
    var ascending = true { didSet { needsDisplay = true } }
    var onSort: ((ListColumn) -> Void)?

    private var resizing: (column: Int, startX: CGFloat, startWidth: CGFloat)?

    private var columns: [(ListColumn, NSRect)] {
        guard let table else { return [] }
        let n = table.tableColumns.count
        return table.tableColumns.enumerated().compactMap { i, c in
            guard let col = ListColumn(rawValue: c.identifier.rawValue) else { return nil }
            let r = table.rect(ofColumn: i)
            return (col, geometry.box(of: i, count: n, in: NSRect(x: r.minX, y: 0, width: r.width, height: bounds.height)))
        }
    }

    /// The handle on column `i`'s left edge (none before the first column).
    private func handle(at p: NSPoint) -> Int? {
        columns.indices.dropFirst().first { abs(p.x - columns[$0].1.minX) <= 4 }
    }

    override var mouseDownCanMoveWindow: Bool { false }

    override func resetCursorRects() {
        for (i, (_, r)) in columns.enumerated() where i > 0 {
            addCursorRect(NSRect(x: r.minX - 4, y: 0, width: 8, height: bounds.height), cursor: .resizeLeftRight)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let cols = columns
        let hot = hoverPoint.flatMap { pt in handle(at: pt) == nil ? cols.firstIndex { $0.1.contains(pt) } : nil }
        for (i, (col, r)) in cols.enumerated() {
            let box = r.insetBy(dx: 0, dy: 2)
            if hot == i { Draw.roundedRect(box, radius: Metrics.radiusSmall, fill: p.hover) }
            let active = col == sortKey
            let color = active ? p.text : p.textSecondary
            let w = Draw.textWidth(col.title, font: Metrics.textXs)
            let arrowW: CGFloat = active ? 4 + 12 : 0
            let inner = r.insetBy(dx: 8, dy: 0)
            let labelX = col == .size ? inner.maxX - min(w + arrowW, inner.width) : inner.minX
            Draw.text(col.title, in: NSRect(x: labelX, y: 0, width: min(w, inner.width - arrowW), height: bounds.height), font: Metrics.textXs, color: color)
            if active {
                Draw.icon(ascending ? "chevron-up" : "chevron-down", in: NSRect(x: labelX + w + 4, y: 0, width: 12, height: bounds.height), size: 12, color: p.textSecondary)
            }
            if i > 0 {
                p.divider.setFill()
                NSRect(x: r.minX, y: 7, width: 1, height: bounds.height - 14).fill()
            }
        }
        p.divider.setFill()
        NSRect(x: 0, y: bounds.height - 1, width: bounds.width, height: 1).fill()
    }

    override func mouseDown(with event: NSEvent) {
        let pt = convert(event.locationInWindow, from: nil)
        if let i = handle(at: pt), let table {
            resizing = (i, event.locationInWindow.x, table.tableColumns[i].width)
            return
        }
        if let (col, _) = columns.first(where: { $0.1.contains(pt) }) { onSort?(col) }
    }

    /// Dragging a column's left edge to the left widens it; the name column gives the room.
    override func mouseDragged(with event: NSEvent) {
        guard let resizing, let table else { return }
        let c = table.tableColumns[resizing.column]
        c.width = max(resizing.startWidth - (event.locationInWindow.x - resizing.startX), c.minWidth)
        needsDisplay = true
        window?.invalidateCursorRects(for: self)
    }

    override func mouseUp(with event: NSEvent) { resizing = nil }
}

// MARK: - rows

/// A row (folder_view.rs `row`): 30 high, `px 4`, rounded 6; hover fill, and when selected the
/// selection fill with the 3 px accent pill; the cursor row of a multiple selection also gets
/// a 1 px `selection_border`. A row that is the only selection stands on the list's glass
/// lens instead and draws no fill of its own.
final class FileRowView: NSTableRowView {
    var geometry = ListGeometry()
    var onLens = false { didSet { if onLens != oldValue { needsDisplay = true } } }
    var hovered = false { didSet { if hovered != oldValue { needsDisplay = true } } }
    var isCursor = false

    override func drawBackground(in dirtyRect: NSRect) {}
    override func drawSelection(in dirtyRect: NSRect) {}
    override var isEmphasized: Bool { get { false } set {} }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let box = NSRect(x: 6, y: bounds.minY, width: bounds.width - 6 - (6 + geometry.gutter), height: bounds.height)
        if isSelected && onLens { return }
        if isSelected {
            Draw.roundedRect(box, radius: Metrics.radiusSmall, fill: p.selection)
            if isCursor {
                p.selectionBorder.setStroke()
                let path = NSBezierPath(roundedRect: box.insetBy(dx: 0.5, dy: 0.5), xRadius: Metrics.radiusSmall, yRadius: Metrics.radiusSmall)
                path.lineWidth = 1
                path.stroke()
            }
            Draw.selectionPill(in: box, accent: p.accent)
        } else if hovered {
            Draw.roundedRect(box, radius: Metrics.radiusSmall, fill: p.hover)
        }
    }
}

/// The name cell: 16 px icon, gap 8, the name (an editable field while renaming).
final class NameCellView: NSTableCellView {
    var lead: CGFloat = 18

    override init(frame: NSRect) {
        super.init(frame: frame)
        let icon = NSImageView()
        icon.imageScaling = .scaleProportionallyUpOrDown
        let text = NSTextField(labelWithString: "")
        text.font = Metrics.textSm
        text.lineBreakMode = .byTruncatingTail
        text.cell?.wraps = false
        text.cell?.isScrollable = false
        addSubview(icon)
        addSubview(text)
        imageView = icon
        textField = text
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        imageView?.frame = NSRect(x: lead, y: (bounds.height - 16) / 2, width: 16, height: 16)
        let x = lead + 16 + 8
        let h: CGFloat = 20
        textField?.frame = NSRect(x: x - 2, y: (bounds.height - h) / 2, width: max(bounds.width - x - 8, 0), height: h)
    }
}

/// The other cells, drawn directly: `px_2`, secondary text, one line. A folder's size cell
/// offers "Calculate size" (`text_xs`, tertiary, accent and underlined on hover).
final class TextCellView: HoverView {
    var text = "" { didSet { needsDisplay = true } }
    var alignRight = false
    var leadPad: CGFloat = 8
    var trailPad: CGFloat = 8
    var isLink = false { didSet { needsDisplay = true } }
    var onClick: (() -> Void)?

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let r = NSRect(x: leadPad, y: 0, width: max(bounds.width - leadPad - trailPad, 0), height: bounds.height)
        guard isLink else {
            Draw.text(text, in: r, font: Metrics.textSm, color: p.textSecondary, align: alignRight ? .right : .left)
            return
        }
        let p2 = NSMutableParagraphStyle()
        p2.alignment = alignRight ? .right : .left
        p2.lineBreakMode = .byTruncatingTail
        var attrs: [NSAttributedString.Key: Any] = [.font: Metrics.textXs, .foregroundColor: hovered ? p.accent : p.textTertiary, .paragraphStyle: p2]
        if hovered { attrs[.underlineStyle] = NSUnderlineStyle.single.rawValue }
        let h = ceil(Metrics.textXs.ascender - Metrics.textXs.descender)
        (text as NSString).draw(with: NSRect(x: r.minX, y: r.midY - h / 2, width: r.width, height: h), options: [.usesLineFragmentOrigin], attributes: attrs)
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        // only a link takes clicks; otherwise the row does (selection, drag)
        isLink ? super.hitTest(point) : nil
    }

    // a link takes the whole click (down and up) so the row is not selected by it
    override func mouseDown(with event: NSEvent) {
        if !isLink { super.mouseDown(with: event) }
    }

    override func mouseUp(with event: NSEvent) {
        if isLink, bounds.contains(convert(event.locationInWindow, from: nil)) { onClick?() }
    }

    override func resetCursorRects() {
        if isLink { addCursorRect(bounds, cursor: .pointingHand) }
    }
}

// MARK: - table

/// The list itself. Space toggles Quick Look, Enter opens, F2 renames, Backspace goes back and
/// ⌘⌫ / Delete moves to the Trash, as in the Windows app (on a Mac keyboard).
@MainActor
final class FileTableView: NSTableView {
    weak var list: FileListController?
    private(set) var hoveredRow = -1

    override func keyDown(with event: NSEvent) {
        if list?.handleKey(event) == true { return }
        super.keyDown(with: event)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInActiveApp, .inVisibleRect], owner: self))
    }

    override func mouseMoved(with event: NSEvent) {
        setHovered(row(at: convert(event.locationInWindow, from: nil)))
    }

    override func mouseExited(with event: NSEvent) { setHovered(-1) }

    private func setHovered(_ r: Int) {
        guard r != hoveredRow else { return }
        (rowView(atRow: hoveredRow, makeIfNecessary: false) as? FileRowView)?.hovered = false
        hoveredRow = r
        (rowView(atRow: r, makeIfNecessary: false) as? FileRowView)?.hovered = true
    }

    /// A table hands clicks to its cells' controls only; "Calculate size" is a link drawn by
    /// its cell, so the table is told to let it have the click.
    override func validateProposedFirstResponder(_ responder: NSResponder, for event: NSEvent?) -> Bool {
        if let cell = responder as? TextCellView, cell.isLink { return true }
        return super.validateProposedFirstResponder(responder, for: event)
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        let r = row(at: convert(event.locationInWindow, from: nil))
        if r >= 0 && !selectedRowIndexes.contains(r) { selectRowIndexes([r], byExtendingSelection: false) }
        if r < 0 { deselectAll(nil) }
        return list?.contextMenu()
    }

    // Quick Look asks the responder chain; the list is the first responder.
    override func acceptsPreviewPanelControl(_ panel: QLPreviewPanel!) -> Bool { true }
    override func beginPreviewPanelControl(_ panel: QLPreviewPanel!) { list?.beginQuickLook(panel) }
    override func endPreviewPanelControl(_ panel: QLPreviewPanel!) { list?.endQuickLook(panel) }
}

// MARK: - status bar

/// The line at the bottom of the card: "N items", then the selection.
@MainActor
final class StatusBarView: FlippedView {
    var count = "" { didSet { needsDisplay = true } }
    var selection = "" { didSet { needsDisplay = true } }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        p.divider.setFill()
        NSRect(x: 0, y: 0, width: bounds.width, height: 1).fill()
        // px_3, gap_3, text_xs
        let r = NSRect(x: 12, y: 1, width: bounds.width - 24, height: bounds.height - 1)
        let w = Draw.textWidth(count, font: Metrics.textXs)
        Draw.text(count, in: NSRect(x: r.minX, y: r.minY, width: w, height: r.height), font: Metrics.textXs, color: p.textSecondary)
        if !selection.isEmpty {
            Draw.text(selection, in: NSRect(x: r.minX + w + 12, y: r.minY, width: r.width - w - 12, height: r.height), font: Metrics.textXs, color: p.text)
        }
    }

    override func viewDidChangeEffectiveAppearance() { needsDisplay = true }
}

// MARK: - the card

/// The content card (workspace.rs): rounded 8, `content_bg`, a 1 px border; inside it the
/// command bar, the header, the list and the status line.
@MainActor
final class CardView: FlippedView {
    var onLayout: (() -> Void)?

    override var wantsUpdateLayer: Bool { true }

    override func layout() {
        super.layout()
        onLayout?()
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = Metrics.radius
        layer?.borderWidth = 1
        layer?.masksToBounds = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func updateLayer() {
        let p = Palette.of(self)
        layer?.backgroundColor = p.contentBg.cgColor
        layer?.borderColor = p.border.cgColor
    }
}
