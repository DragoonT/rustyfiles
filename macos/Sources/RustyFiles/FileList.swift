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
    // optional (Settings › Appearance › Command bar buttons)
    let newFolder = IconButton(icon: "folder-plus", size: NSSize(width: 36, height: 32), tooltip: String(localized: "New folder"))
    let terminal = IconButton(icon: "square-terminal", size: NSSize(width: 36, height: 32), tooltip: String(localized: "Open in Terminal"))
    let hiddenItems = IconButton(icon: "eye", size: NSSize(width: 36, height: 32), tooltip: String(localized: "Show or hide hidden items"))
    let refresh = IconButton(icon: "rotate-cw", size: NSSize(width: 36, height: 32), tooltip: String(localized: "Refresh"))
    private let sepLeft = Divider()
    private let sepRight = Divider()

    override init(frame: NSRect) {
        super.init(frame: frame)
        new.label = String(localized: "New")
        new.chevron = true
        sort.chevron = true
        view.chevron = true
        [new, sepLeft, newFolder, cut, copy, paste, rename, share, delete, properties, terminal, hiddenItems, refresh, filter, sort, view, sepRight, preview]
            .forEach(addSubview)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    /// In the Trash the first button empties it (the Recycle Bin's "Empty Recycle Bin"), and
    /// Delete deletes for good.
    var trashMode = false {
        didSet {
            guard trashMode != oldValue else { return }
            new.label = trashMode ? String(localized: "Empty Trash") : String(localized: "New")
            new.icon = trashMode ? "eraser" : "circle-plus"
            new.chevron = !trashMode
            delete.toolTip = trashMode ? String(localized: "Delete immediately") : String(localized: "Delete")
            needsLayout = true
        }
    }

    private func shows(_ b: AppSettings.ToolbarButton) -> Bool { AppSettings.showsButton(b) || (b == .new && trashMode) }

    override func layout() {
        super.layout()
        let h = bounds.height - 1
        func place(_ v: NSView, _ x: CGFloat, _ w: CGFloat, _ height: CGFloat = 32) {
            v.isHidden = false
            v.frame = NSRect(x: x, y: (h - height) / 2, width: w, height: height)
        }
        subviews.forEach { $0.isHidden = true }
        typealias T = AppSettings.ToolbarButton
        // px_2, gap_0p5; separators 1×20 with mx_1
        var x: CGFloat = 8
        if shows(.new) {
            place(new, x, new.intrinsicContentSize.width)
            x += new.frame.width + 2 + 4
            place(sepLeft, x, 1, 20)
            x += 1 + 4 + 2
        }
        let left: [(T, IconButton)] = [
            (.newFolder, newFolder), (.cut, cut), (.copy, copy), (.paste, paste), (.rename, rename), (.share, share), (.delete, delete),
            (.properties, properties), (.terminal, terminal), (.hidden, hiddenItems), (.refresh, refresh),
        ]
        for (k, b) in left where shows(k) {
            place(b, x, 36)
            x += 36 + 2
        }
        var r = bounds.width - 8
        if shows(.preview) {
            r -= 36
            place(preview, r, 36)
            r -= 2 + 4 + 1
            place(sepRight, r, 1, 20)
            r -= 4 + 2
        }
        for (k, b) in [(T.view, view), (.sort, sort)] where shows(k) {
            let w = b.intrinsicContentSize.width
            r -= w
            place(b, r, w)
            r -= 2
        }
        if shows(.filter) {
            r -= 36
            place(filter, r, 36)
        }
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
    case name, modified, created, type, size, tags, git

    /// The columns that can be turned on or off (Settings › Layout › Details columns).
    static let optional: [ListColumn] = [.modified, .created, .type, .size, .tags, .git]

    var title: String {
        switch self {
        case .name: String(localized: "Name")
        case .modified: String(localized: "Date modified")
        case .type: String(localized: "Type")
        case .size: String(localized: "Size")
        case .created: String(localized: "Date created")
        case .tags: String(localized: "Tags")
        case .git: String(localized: "Git status")
        }
    }

    var defaultWidth: CGFloat {
        switch self {
        case .name: 320
        case .modified, .created, .type: 150
        case .size: 100
        case .tags: 120
        case .git: 100
        }
    }

    /// Tags and Git status are shown, not sorted by.
    var sortable: Bool { self != .tags && self != .git }

    var sort: SortColumn {
        switch self {
        case .name, .tags, .git: .name
        case .modified: .modified
        case .created: .created
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
    /// A column was dragged to this width (its content width, without the list's own edge room).
    var onResize: ((ListColumn, CGFloat) -> Void)?
    /// A double click on a column's edge: it is automatic again.
    var onResetWidth: ((ListColumn) -> Void)?

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
            if event.clickCount == 2 {
                onResetWidth?(columns[i].0)
                return
            }
            resizing = (i, event.locationInWindow.x, table.tableColumns[i].width)
            return
        }
        if let (col, _) = columns.first(where: { $0.1.contains(pt) }) { onSort?(col) }
    }

    /// Dragging a column's left edge to the left widens it; the name column gives the room.
    override func mouseDragged(with event: NSEvent) {
        guard let resizing, let table else { return }
        let col = columns[resizing.column].0
        let isLast = resizing.column == table.tableColumns.count - 1
        let w = max(resizing.startWidth - (event.locationInWindow.x - resizing.startX), table.tableColumns[resizing.column].minWidth)
        onResize?(col, w - (isLast ? geometry.trail : 0))
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
    /// The first row of a group (Settings › Layout › Group by) carries the group's header
    /// above itself, in its top `GroupMetrics.header` points.
    var groupTitle: String? { didSet { needsDisplay = true } }

    override func drawBackground(in dirtyRect: NSRect) {}
    override func drawSelection(in dirtyRect: NSRect) {}
    override var isEmphasized: Bool { get { false } set {} }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let inset = groupTitle == nil ? 0 : GroupMetrics.header
        if let groupTitle {
            let r = NSRect(x: 12, y: 10, width: bounds.width - 24 - geometry.gutter, height: 18)
            let w = Draw.textWidth(groupTitle, font: Metrics.textSmMedium)
            Draw.text(groupTitle, in: r, font: Metrics.textSmMedium, color: p.accent)
            p.divider.setFill()
            NSRect(x: r.minX + w + 10, y: r.midY, width: max(r.width - w - 10, 0), height: 1).fill()
        }
        let box = NSRect(x: 6, y: bounds.minY + inset, width: bounds.width - 6 - (6 + geometry.gutter), height: bounds.height - inset)
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
    /// Files › Selection check boxes: drawn before the icon, checked when the row is selected.
    var showsBox = false { didSet { needsLayout = true; needsDisplay = true } }
    var checked = false { didSet { needsDisplay = true } }
    private(set) var boxRect = NSRect.zero
    /// Room above for a group header.
    var topInset: CGFloat = 0 { didSet { needsLayout = true } }

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

    override func draw(_ dirtyRect: NSRect) {
        guard showsBox else { return }
        let p = Palette.of(self)
        let r = boxRect.insetBy(dx: 2, dy: 2)
        let path = NSBezierPath(roundedRect: r.insetBy(dx: 0.5, dy: 0.5), xRadius: 4, yRadius: 4)
        if checked {
            p.accent.setFill()
            path.fill()
            let tick = NSBezierPath()
            tick.move(to: NSPoint(x: r.minX + 3.5, y: r.midY))
            tick.line(to: NSPoint(x: r.minX + 6.5, y: r.minY + 3.5))
            tick.line(to: NSPoint(x: r.maxX - 3, y: r.maxY - 4))
            tick.lineWidth = 1.8
            p.accentText.setStroke()
            tick.stroke()
        } else {
            p.textTertiary.setStroke()
            path.stroke()
        }
    }

    override func layout() {
        super.layout()
        // the cell is not flipped: the row is its lower part, the header room above it
        let rowH = bounds.height - topInset
        let base: CGFloat = isFlipped ? topInset : 0
        let boxW: CGFloat = showsBox ? 16 + 8 : 0
        boxRect = showsBox ? NSRect(x: lead - 2, y: base + (rowH - 16) / 2 - 2, width: 20, height: 20) : .zero
        imageView?.frame = NSRect(x: lead + boxW, y: base + (rowH - 16) / 2, width: 16, height: 16)
        let x = lead + boxW + 16 + 8
        let h: CGFloat = 20
        textField?.frame = NSRect(x: x - 2, y: base + (rowH - h) / 2, width: max(bounds.width - x - 8, 0), height: h)
    }
}

/// The other cells, drawn directly: `px_2`, secondary text, one line. A folder's size cell
/// offers "Calculate size" (`text_xs`, tertiary, accent and underlined on hover).
final class TextCellView: HoverView {
    var text = "" { didSet { needsDisplay = true } }
    var alignRight = false
    var leadPad: CGFloat = 8
    var trailPad: CGFloat = 8
    var topInset: CGFloat = 0 { didSet { needsDisplay = true } }
    var isLink = false { didSet { needsDisplay = true } }
    var onClick: (() -> Void)?

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        let r = NSRect(x: leadPad, y: topInset, width: max(bounds.width - leadPad - trailPad, 0), height: bounds.height - topInset)
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
final class FileTableView: NSTableView, NSServicesMenuRequestor {
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

    private var hoverTimer: Timer?

    private func setHovered(_ r: Int) {
        guard r != hoveredRow else { return }
        (rowView(atRow: hoveredRow, makeIfNecessary: false) as? FileRowView)?.hovered = false
        hoveredRow = r
        (rowView(atRow: r, makeIfNecessary: false) as? FileRowView)?.hovered = true
        // Files › Select on hover: resting on a row for half a second selects it
        hoverTimer?.invalidate()
        guard AppSettings.selectOnHover, r >= 0 else { return }
        hoverTimer = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, self.hoveredRow == r, NSEvent.pressedMouseButtons == 0, !self.selectedRowIndexes.contains(r) else { return }
                let extend = NSEvent.modifierFlags.contains(.command)
                self.selectRowIndexes([r], byExtendingSelection: extend)
            }
        }
    }

    /// Files › Selection check boxes: a click on a row's box adds it to the selection or takes
    /// it away, leaving the rest as it is.
    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let r = row(at: p)
        if AppSettings.selectionBoxes, r >= 0, let cell = view(atColumn: 0, row: r, makeIfNecessary: false) as? NameCellView,
            cell.boxRect.contains(cell.convert(event.locationInWindow, from: nil))
        {
            if selectedRowIndexes.contains(r) { deselectRow(r) } else { selectRowIndexes([r], byExtendingSelection: true) }
            return
        }
        super.mouseDown(with: event)
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

    // macOS's Services and Quick Actions (the Windows app's "Show more options") work on the
    // selected files: the menu shows the ones that take file URLs.
    override func validRequestor(forSendType sendType: NSPasteboard.PasteboardType?, returnType: NSPasteboard.PasteboardType?) -> Any? {
        if sendType == .fileURL, returnType == nil, let list, !list.selectedURLs.isEmpty, list.archiveIsNil { return self }
        return super.validRequestor(forSendType: sendType, returnType: returnType)
    }

    func writeSelection(to pboard: NSPasteboard, types: [NSPasteboard.PasteboardType]) -> Bool {
        guard let urls = list?.selectedURLs, !urls.isEmpty else { return false }
        pboard.clearContents()
        return pboard.writeObjects(urls as [NSURL])
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
    /// At the right: a copy's progress (Advanced › Show progress › In the status bar), and how
    /// long the folder took to list (Advanced › Performance in the status bar).
    var progress = "" { didSet { needsDisplay = true } }
    var progressFraction: Double? { didSet { needsDisplay = true } }
    var performance = "" { didSet { needsDisplay = true } }

    override func draw(_ dirtyRect: NSRect) {
        let p = Palette.of(self)
        p.divider.setFill()
        NSRect(x: 0, y: 0, width: bounds.width, height: 1).fill()
        // px_3, gap_3, text_xs
        var r = NSRect(x: 12, y: 1, width: bounds.width - 24, height: bounds.height - 1)
        let right = [progress, performance].filter { !$0.isEmpty }.joined(separator: "   ")
        if !right.isEmpty {
            let rw = min(Draw.textWidth(right, font: Metrics.textXs), r.width * 0.6)
            var x = r.maxX - rw
            if let f = progressFraction {
                // a thin bar before the text
                let bar = NSRect(x: x - 8 - 80, y: r.midY - 2, width: 80, height: 4)
                Draw.roundedRect(bar, radius: 2, fill: p.hover)
                Draw.roundedRect(NSRect(x: bar.minX, y: bar.minY, width: bar.width * CGFloat(min(max(f, 0), 1)), height: 4), radius: 2, fill: p.accent)
                x = bar.minX
            }
            Draw.text(right, in: NSRect(x: r.maxX - rw, y: r.minY, width: rw, height: r.height), font: Metrics.textXs, color: p.textSecondary)
            r.size.width = max(x - 12 - r.minX, 0)
        }
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
        // Appearance › Background colour: the list's own colour, at the theme's strength
        let own = Palette.parse(AppSettings.listBackground)
        layer?.backgroundColor = (own.map { $0.withAlphaComponent(max(p.contentBg.alphaComponent, 0.85)) } ?? p.contentBg).cgColor
        layer?.borderColor = p.border.cgColor
    }

    /// Appearance › Background picture: behind the files, filling the card, as visible as set.
    private let picture = CALayer()
    private var picturePath = ""

    func applyBackground() {
        needsDisplay = true
        updateLayer()
        let path = AppSettings.listImage
        if picture.superlayer == nil {
            picture.contentsGravity = .resizeAspectFill
            picture.masksToBounds = true
            picture.zPosition = -1
            layer?.insertSublayer(picture, at: 0)
        }
        if path != picturePath {
            picturePath = path
            picture.contents = path.isEmpty ? nil : NSImage(contentsOfFile: path)
        }
        picture.opacity = Float(AppSettings.listImageVisibility) / 100
        picture.isHidden = path.isEmpty
        picture.frame = bounds
    }

    override func layout() {
        super.layout()
        picture.frame = bounds
        onLayout?()
    }
}

enum GroupMetrics {
    /// The group header's height above a group's first row.
    static let header: CGFloat = 36
}
