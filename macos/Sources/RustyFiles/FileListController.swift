import AppKit
import Quartz

/// Details (the table), or the grid in one of its styles (folder_view.rs `GridStyle`): Tiles,
/// Cards (wide, with type, size and date beside the picture) and List (a small icon and the
/// name, many to a row).
enum ViewMode: String, CaseIterable {
    case details, tiles, cards, list, columns

    var isGrid: Bool { self == .tiles || self == .cards || self == .list }

    var title: String {
        switch self {
        case .details: String(localized: "Details")
        case .tiles: String(localized: "Tiles")
        case .cards: String(localized: "Cards")
        case .list: String(localized: "List")
        case .columns: String(localized: "Columns")
        }
    }

    var icon: String {
        switch self {
        case .details: "list"
        case .tiles: "layout-grid"
        case .cards: "layout-list"
        case .list: "rows-3"
        case .columns: "columns-3"
        }
    }

    /// A grid cell's size (Windows: Tiles 124 × 150, Cards 300 × 84, List 240 × 32, rows
    /// including the 4 between them).
    var cellSize: NSSize {
        switch self {
        case .cards: NSSize(width: 300, height: 84 - TileMetrics.gap)
        case .list: NSSize(width: 240, height: 32 - TileMetrics.gap)
        default: NSSize(width: TileMetrics.width, height: TileMetrics.rowHeight - TileMetrics.gap)
        }
    }
}

/// The content card and everything in it for one window. The rows live in Rust ([`Folder`]);
/// the table and the grid ask for the ones they draw, so a huge folder costs no more than what
/// is on screen.
@MainActor
final class FileListController: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSTextFieldDelegate,
    NSCollectionViewDataSource, NSCollectionViewDelegate, NSCollectionViewDelegateFlowLayout,
    @preconcurrency QLPreviewPanelDataSource, @preconcurrency QLPreviewPanelDelegate
{
    let card = CardView()
    let commandBar = CommandBarView()
    let header = ListHeaderView()
    let table = FileTableView()
    let status = StatusBarView()
    private let scroll = NSScrollView()
    let tiles = TileCollectionView()
    private let tileLayout = NSCollectionViewFlowLayout()
    private let tileScroll = NSScrollView()
    private(set) var viewMode: ViewMode = AppSettings.defaultView
    /// The Settings page, shown in the card in place of the folder.
    private var settingsView: SettingsView?
    /// The Home page, the same way.
    private var homeView: HomeView?
    private var diskView: DiskUsageView?
    /// A page of the app's own (Settings, Home, Disk usage) fills the card.
    private var pageShown: Bool { [settingsView as NSView?, homeView, diskView].contains { $0.map { !$0.isHidden } ?? false } }
    private let message = NSTextField(wrappingLabelWithString: "")
    private let display = ItemDisplay()
    private var geometry = ListGeometry()
    /// The glass lens under the selected row when exactly one row is selected; it slides from
    /// row to row. A 30 px row gets a capsule (glass.rs `LensShape::Row`).
    private let listLens = LensView(cornerRadius: Metrics.rowHeight / 2)

    private(set) var folder: Folder?
    private(set) var url: URL?
    /// The archive and the folder in it, while one is shown.
    private(set) var archive: ArchiveView?
    var sortKey: ListColumn = AppSettings.sortKey
    var ascending = !AppSettings.descending
    var foldersFirst = AppSettings.foldersFirst
    /// Settings › Layout › Group by (or the folder's own, when each folder keeps its layout).
    var groupBy = AppSettings.groupBy
    /// The groups folded (by title), how many items each group has, and the rows that stand
    /// for a folded group (its header alone).
    var collapsedGroups: Set<String> = []
    var groupCounts: [String: Int] = [:]
    var foldedRows: Set<Int> = []
    private var isFolded = false
    private var pinch: CGFloat = 0
    private var hoverAnchor: Int?
    /// Each row's group header, for the first row of every group (details only).
    private var groupTitles: [Int: String] = [:]
    /// Tags and Git states of the folder shown, by name (filled when their columns are on).
    private var tagsByName: [String: String] = [:]
    private var gitByName: [String: String] = [:]
    /// The Spotlight properties of the items, for the property columns that are on.
    private var propsByName: [String: [ListColumn: String]] = [:]
    var showHidden = AppSettings.showHidden { didSet { if showHidden != oldValue { rearrange() } } }
    var filter = "" { didSet { if filter != oldValue { rearrange() } } }

    var onOpenFolder: ((URL) -> Void)?
    var onOpenDiskImage: ((URL) -> Void)?
    var onBack: (() -> Void)?
    var onChange: (() -> Void)?
    var onError: ((String) -> Void)?

    /// Sizes worked out with "Calculate size", remembered across starts as on Windows
    /// (Application Support/rustyfiles/folder_sizes.json). A folder changed since (its own
    /// modification date) is measured again when asked.
    private static var folderSizes: [String: UInt64] = loadFolderSizes()
    private static var folderStamps: [String: Double] = [:]
    private static var savePending = false

    private static var folderSizesFile: URL {
        let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("rustyfiles", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("folder_sizes.json")
    }

    private static func stamp(_ path: String) -> Double {
        ((try? FileManager.default.attributesOfItem(atPath: path)[.modificationDate]) as? Date)?.timeIntervalSince1970 ?? 0
    }

    private static func loadFolderSizes() -> [String: UInt64] {
        guard let data = try? Data(contentsOf: folderSizesFile),
            let saved = try? JSONDecoder().decode([String: [Double]].self, from: data)
        else { return [:] }
        var sizes: [String: UInt64] = [:]
        for (k, v) in saved where v.count == 2 {
            sizes[k] = UInt64(v[0])
            folderStamps[k] = v[1]
        }
        return sizes
    }

    /// A remembered size, if the folder has not changed since it was measured.
    private static func rememberedSize(_ key: String, path: String) -> UInt64? {
        guard let size = folderSizes[key] else { return nil }
        if let s = folderStamps[key], s != stamp(path) {
            folderSizes[key] = nil
            folderStamps[key] = nil
            return nil
        }
        return size
    }

    private static func remember(_ key: String, path: String, size: UInt64) {
        folderSizes[key] = size
        folderStamps[key] = stamp(path)
        guard !savePending else { return }
        savePending = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
            MainActor.assumeIsolated {
                savePending = false
                // the newest 5000 are plenty
                let all = folderSizes.compactMap { k, v in folderStamps[k].map { (k, [Double(v), $0]) } }
                let kept = Dictionary(uniqueKeysWithValues: all.suffix(5000))
                if let data = try? JSONEncoder().encode(kept) { try? data.write(to: folderSizesFile, options: .atomic) }
            }
        }
    }
    private static var sizing: Set<String> = []

    override init() {
        super.init()
        rebuildColumns()
        table.headerView = nil
        table.rowHeight = Metrics.rowHeight
        table.intercellSpacing = .zero
        table.style = .plain
        table.backgroundColor = .clear
        table.gridStyleMask = []
        table.focusRingType = .none
        table.allowsMultipleSelection = true
        table.allowsColumnReordering = false
        // widths are set here (automatic columns, the name takes the rest), never by the table
        table.columnAutoresizingStyle = .noColumnAutoresizing
        table.dataSource = self
        table.delegate = self
        table.list = self
        table.target = self
        table.doubleAction = #selector(doubleClicked(_:))
        table.action = #selector(singleClicked(_:))
        table.setDraggingSourceOperationMask([.copy, .move], forLocal: false)
        table.setDraggingSourceOperationMask([.copy, .move], forLocal: true)
        table.registerForDraggedTypes([.fileURL])

        scroll.documentView = table
        // under the rows (their own drawing is clear where the lens is)
        table.addSubview(listLens, positioned: .below, relativeTo: nil)
        listLens.isHidden = true
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.scrollerStyle = .overlay
        scroll.automaticallyAdjustsContentInsets = false
        scroll.contentInsets = NSEdgeInsets(top: 4, left: 0, bottom: 4, right: 0)

        tileLayout.itemSize = NSSize(width: TileMetrics.width, height: TileMetrics.rowHeight - TileMetrics.gap)
        tileLayout.minimumInteritemSpacing = TileMetrics.gap
        tileLayout.minimumLineSpacing = TileMetrics.gap
        tileLayout.sectionInset = NSEdgeInsets(top: TileMetrics.padY, left: TileMetrics.padX, bottom: TileMetrics.padY, right: TileMetrics.padX)
        tiles.collectionViewLayout = tileLayout
        tiles.register(TileItem.self, forItemWithIdentifier: TileItem.id)
        tiles.register(GridGroupHeader.self, forSupplementaryViewOfKind: NSCollectionView.elementKindSectionHeader, withIdentifier: GridGroupHeader.id)
        tiles.isSelectable = true
        tiles.allowsMultipleSelection = true
        tiles.allowsEmptySelection = true
        tiles.backgroundColors = [.clear]
        tiles.dataSource = self
        tiles.delegate = self
        tiles.list = self
        tiles.setDraggingSourceOperationMask([.copy, .move], forLocal: false)
        tiles.setDraggingSourceOperationMask([.copy, .move], forLocal: true)
        tiles.registerForDraggedTypes([.fileURL])
        tileScroll.documentView = tiles
        tileScroll.drawsBackground = false
        tileScroll.hasVerticalScroller = true
        tileScroll.hasHorizontalScroller = true
        tileScroll.scrollerStyle = .overlay

        header.table = table
        header.onSort = { [weak self] col in if col.sortable { self?.sort(by: col) } }
        header.onResize = { [weak self] col, w in
            ColumnWidths.set(col, w)
            self?.applyColumnWidths()
        }
        header.onResetWidth = { [weak self] col in
            ColumnWidths.set(col, nil)
            self?.applyColumnWidths()
        }

        message.alignment = .center
        message.font = Metrics.textSm
        message.textColor = .secondaryLabelColor
        message.isHidden = true
        if card.subviews.contains(privacyButton) { privacyButton.isHidden = true }

        [commandBar, header, scroll, tileScroll, status, message, previewPane, previewHandle, shelfPane].forEach(card.addSubview)
        for col in millerLeft + [millerNext] {
            col.isHidden = true
            col.icon = { [weak self] item, path in self?.display.icon(item, path: path) ?? NSImage() }
            col.onOpenFolder = { [weak self] u in self?.onOpenFolder?(u) }
            col.onPick = { [weak self] u in self?.onReveal?(u) }
            col.onMenu = { [weak self] u, isDir in self?.sideColumnMenu(u, isDir: isDir) }
            card.addSubview(col)
        }
        shelfPane.isHidden = true
        shelfPane.onClose = { [weak self] in self?.shelfShown = false }
        status.onGit = { [weak self] info, view, rect in
            guard let self else { return }
            self.gitMenu(info).popUp(positioning: nil, at: NSPoint(x: rect.minX, y: rect.minY - 4), in: view)
        }
        previewHandle.currentWidth = { [weak self] in self?.previewWidth ?? PreviewPane.defaultWidth }
        previewHandle.onDrag = { [weak self] w in
            self?.previewWidth = w
            UserDefaults.standard.set(Double(w), forKey: "previewWidth")
            self?.card.needsLayout = true
        }
        applyViewMode()
        card.onLayout = { [weak self] in self?.layoutCard() }
        NotificationCenter.default.addObserver(self, selector: #selector(statusCenterChanged), name: .rfStatusCenterChanged, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(cutChanged), name: .rfCutChanged, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(updateChanged), name: .rfUpdateChanged, object: nil)
        status.onUpdate = { Updater.restart() }
        applyScrollBars()
        card.applyBackground()
        NotificationCenter.default.addObserver(self, selector: #selector(columnsChanged), name: NSTableView.columnDidResizeNotification, object: table)
        wireCommandBar()
    }

    // MARK: layout

    // MARK: preview pane

    let previewPane = PreviewPane()
    /// Columns: the two parents at the left (farthest first) and the selected folder's column.
    let millerLeft = [MillerColumn(), MillerColumn()]
    let millerNext = MillerColumn()

    /// The open folder's parents and the selected folder, in the side columns.
    func updateMillerColumns() {
        guard viewMode == .columns, let url, url.isFileURL, archive == nil else {
            (millerLeft + [millerNext]).forEach { $0.show(nil, showHidden: showHidden) }
            return
        }
        var chain: [(URL, String)] = []
        var child = url
        for _ in 0..<2 {
            let parent = child.deletingLastPathComponent()
            guard parent.path != child.path else { break }
            chain.insert((parent, child.lastPathComponent), at: 0)
            child = parent
        }
        for (i, col) in millerLeft.enumerated() {
            let k = i - (millerLeft.count - chain.count)
            if k >= 0 {
                col.show(chain[k].0, showHidden: showHidden)
                col.marked = chain[k].1
            } else {
                col.show(nil, showHidden: showHidden)
            }
        }
        updateMillerNext()
        card.needsLayout = true
    }

    private func updateMillerNext() {
        guard viewMode == .columns else { return }
        var next: URL?
        if selectedRows.count == 1, let row = selectedRows.first, let item = folder?.item(row: UInt32(row)), item.isDir, !display.isPackage(item),
            let p = folder?.childPath(row: UInt32(row))
        {
            next = URL(fileURLWithPath: p)
        }
        if next != millerNext.dir {
            millerNext.show(next, showHidden: showHidden)
            card.needsLayout = true
        }
    }

    /// A file picked in a side column: its folder, with it selected.
    var onReveal: ((URL) -> Void)?

    /// The shelf pane (shelf_view.rs), at the right of the list, left of the preview pane.
    let shelfPane = ShelfPane()
    var shelfShown = false {
        didSet {
            guard shelfShown != oldValue else { return }
            if shelfShown { ShelfStore.prune() }
            card.needsLayout = true
        }
    }
    private let previewHandle = PreviewResizeHandle()
    private var previewWidth: CGFloat = {
        let w = UserDefaults.standard.double(forKey: "previewWidth")
        return w > 0 ? min(max(w, PreviewPane.minWidth), PreviewPane.maxWidth) : PreviewPane.defaultWidth
    }()
    /// Shown in every window once turned on (⇧⌘P, the command bar's last button), as on Windows.
    var previewShown = UserDefaults.standard.bool(forKey: "previewShown") {
        didSet {
            UserDefaults.standard.set(previewShown, forKey: "previewShown")
            card.needsLayout = true
            updatePreview()
            updateCommands()
        }
    }

    @objc func togglePreviewPane(_ sender: Any?) { previewShown.toggle() }

    func showProperties() {
        guard archive == nil else { return }
        let urls = selectedURLs
        if !urls.isEmpty { PropertiesWindow.show(urls) } else if let url { PropertiesWindow.show([url]) }
    }

    /// The pane shows the focused item: the last one clicked, else the first selected.
    func updatePreview() {
        guard previewShown, archive == nil, folder != nil else { return previewPane.show(nil) }
        let row = viewMode == .details && selectedRows.contains(table.selectedRow) ? table.selectedRow : selectedRows.first
        guard let row, let p = folder?.childPath(row: UInt32(row)) else { return previewPane.show(nil) }
        previewPane.show(URL(fileURLWithPath: p))
    }

    private func layoutCard() {
        var b = card.bounds
        // the pane takes the right of the card, under the command bar, beside the list and its
        // status bar; it never leaves the list less than 320 wide
        // Appearance › Show the command bar / status bar
        let barH = AppSettings.showCommandBar ? Metrics.commandBarHeight : 0
        let statusH = AppSettings.showStatusBar ? Metrics.statusBarHeight : 0
        commandBar.isHidden = barH == 0 || pageShown
        status.isHidden = statusH == 0 || pageShown
        // the pane: at the right of the list (never leaving it under 320 wide), or under it
        let bottom = AppSettings.previewAtBottom
        let ph = min(max(b.height * 0.38, 160), b.height - barH - 160)
        let pw = previewShown ? (bottom ? b.width : min(previewWidth, max(card.bounds.width - 320, 0))) : 0
        let paneShown = previewShown && (bottom ? ph >= 120 : pw >= PreviewPane.minWidth) && !pageShown
        previewPane.isHidden = !paneShown
        previewHandle.isHidden = !paneShown || bottom
        previewPane.atBottom = bottom
        if paneShown && !bottom {
            previewPane.frame = NSRect(x: b.width - pw, y: barH, width: pw, height: b.height - barH)
            previewHandle.frame = NSRect(x: b.width - pw - 3, y: barH, width: 6, height: b.height - barH)
        }
        commandBar.frame = NSRect(x: 0, y: 0, width: b.width, height: Metrics.commandBarHeight)
        if paneShown && !bottom { b.size.width -= pw }
        if paneShown && bottom {
            previewPane.frame = NSRect(x: 0, y: b.height - ph, width: b.width, height: ph)
            b.size.height -= ph
        }
        // the shelf: 260 wide between the list and the preview pane (workspace.rs)
        let shelfW: CGFloat = shelfShown && !pageShown && b.width > 260 + 320 ? 260 : 0
        shelfPane.isHidden = shelfW == 0
        if shelfW > 0 {
            shelfPane.frame = NSRect(x: b.width - shelfW, y: barH, width: shelfW, height: b.height - barH)
            b.size.width -= shelfW
        }
        header.frame = NSRect(x: 0, y: barH, width: b.width, height: Metrics.headerHeight)
        let top = barH + Metrics.headerHeight
        status.frame = NSRect(x: 0, y: b.height - statusH, width: b.width, height: Metrics.statusBarHeight)
        scroll.frame = NSRect(x: 0, y: top, width: b.width, height: b.height - statusH - top)
        if viewMode == .columns && !pageShown {
            // the parents that fit at the left, the open folder's column, then the next one
            let w = ColumnMetrics.width, h = b.height - statusH - barH
            let next = millerNext.dir != nil
            let fit = max(min(millerLeft.filter { $0.dir != nil }.count, Int((b.width - w * (next ? 2 : 1)) / w)), 0)
            var x: CGFloat = 0
            for (i, col) in millerLeft.enumerated() {
                let shown = col.dir != nil && i >= millerLeft.count - fit
                col.isHidden = !shown
                if shown {
                    col.frame = NSRect(x: x, y: barH, width: w, height: h)
                    x += w
                }
            }
            scroll.frame = NSRect(x: x, y: barH, width: w, height: h)
            x += w
            millerNext.isHidden = !next
            millerNext.frame = NSRect(x: x, y: barH, width: max(w, b.width - x), height: h)
        }
        tileScroll.frame = NSRect(x: 0, y: barH, width: b.width, height: b.height - statusH - barH)
        if listByColumns {
            // List: filled column by column, each column as wide as its longest name
            if tileLayout.scrollDirection != .horizontal || tileLayout.sectionInset.right != TileMetrics.padX {
                tileLayout.scrollDirection = .horizontal
                tileLayout.sectionInset.right = TileMetrics.padX
                listWidths = nil
                tileLayout.invalidateLayout()
            }
            if listRowsPerColumn != listWidths?.rows {
                listWidths = nil
                tileLayout.invalidateLayout()
            }
        } else {
            if tileLayout.scrollDirection != .vertical {
                tileLayout.scrollDirection = .vertical
                tileLayout.invalidateLayout()
            }
            // as many columns as fit, left-aligned with 4 between them (not spread out)
            let cellW = viewMode.cellSize.width
            let cols = max(Int((b.width - 2 * TileMetrics.padX + TileMetrics.gap) / (cellW + TileMetrics.gap)), 1)
            let used = CGFloat(cols) * (cellW + TileMetrics.gap) - TileMetrics.gap
            let right = max(b.width - TileMetrics.padX - used, TileMetrics.padX)
            if tileLayout.sectionInset.right != right {
                tileLayout.sectionInset.right = right
                tileLayout.invalidateLayout()
            }
        }
        message.frame = NSRect(x: 40, y: top + 40, width: b.width - 80, height: 80)
        settingsView?.frame = card.bounds
        homeView?.frame = card.bounds
        diskView?.frame = card.bounds

        defer { placeListLens(animated: false) }
        let gutter = Metrics.gutter(listWidth: b.width)
        if gutter != geometry.gutter {
            geometry.gutter = gutter
            header.geometry = geometry
            table.enumerateAvailableRowViews { row, _ in
                (row as? FileRowView)?.geometry = self.geometry
                row.needsDisplay = true
            }
            table.reloadData(forRowIndexes: IndexSet(integersIn: 0..<table.numberOfRows), columnIndexes: [table.tableColumns.count - 1])
        }
        applyColumnWidths()
    }

    // MARK: column widths (folder_view/auto_cols.rs)

    /// What the automatic widths were measured for; measured again when it changes.
    private var measuredFor: (folder: ObjectIdentifier?, date: AppSettings.DateStyle, decimal: Bool)?
    private var measured: [ListColumn: CGFloat] = [:]

    /// `fit`: a column is as wide as its widest text plus some air, never less than its heading
    /// with the sort arrow needs, between 56 and 300, padding included.
    private static func fit(_ content: CGFloat, heading: CGFloat, air: CGFloat) -> CGFloat {
        min(max(ceil(max(content + air, heading + 16) + 16), 56), 300)
    }

    /// Measures the automatic columns for this listing: dates and sizes from the widest forms
    /// their formatters give, types from the kinds of items here (as on Windows).
    private func measure() {
        let key = (folder: folder.map(ObjectIdentifier.init), date: AppSettings.dateStyle, decimal: AppSettings.decimalSizes)
        if let m = measuredFor, m.folder == key.folder, m.date == key.date, m.decimal == key.decimal { return }
        measuredFor = key
        func w(_ s: String, _ f: NSFont? = nil) -> CGFloat { Draw.textWidth(s, font: f ?? Metrics.textSm) }
        func head(_ c: ListColumn) -> CGFloat { w(c.title, Metrics.textXs) }
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        let day: Int64 = 86_400_000
        let dates = [400 * day, 59 * 60_000, 23 * 3_600_000, 6 * day].map { ItemDisplay.date(now - $0, style: AppSettings.dateStyle) }
        let dateW = dates.map { w($0) }.max() ?? 0
        let sizes: [UInt64] = [1023, 1023 << 10, 104_752_742, 10_726_932_152, 1023 << 30]
        var sizeW = sizes.map { w(formatSize(bytes: $0)) }.max() ?? 0
        var types = Set<String>()
        var hasDir = false
        if let folder {
            for r in 0..<min(folder.count(), 20_000) {
                guard let item = folder.item(row: r) else { continue }
                if item.isDir && !display.isPackage(item) { hasDir = true } else { types.insert(display.kind(item)) }
                if types.count > 48 { break }
            }
        }
        if hasDir {
            types.insert(String(localized: "File folder"))
            if archive == nil { sizeW = max(sizeW, w(String(localized: "Calculate size"), Metrics.textXs)) }
        }
        let typeW = types.map { w($0) }.max() ?? 0
        measured = [
            .modified: Self.fit(dateW, heading: head(.modified), air: 14),
            .type: Self.fit(typeW, heading: head(.type), air: 14),
            .size: Self.fit(sizeW, heading: head(.size), air: 0),
        ]
    }

    /// Every column at its width (dragged, else measured) and the name with all that is left.
    func applyColumnWidths() {
        measure()
        let avail = max(scroll.contentSize.width, 200)
        var others: CGFloat = 0
        for c in table.tableColumns {
            guard let col = ListColumn(rawValue: c.identifier.rawValue), col != .name, !c.isHidden else { continue }
            var width = ColumnWidths.get(col) ?? measured[col] ?? col.defaultWidth
            if c == table.tableColumns.last { width += geometry.trail }
            if abs(c.width - width) > 0.5 { c.width = width }
            others += width
        }
        if let name = table.tableColumns.first {
            let width = max(avail - others, 120 + ListGeometry.lead)
            if abs(name.width - width) > 0.5 { name.width = width }
        }
        if abs(table.frame.width - avail) > 0.5 { table.setFrameSize(NSSize(width: max(avail, table.tableColumns.reduce(0) { $0 + $1.width }), height: table.frame.height)) }
        header.needsDisplay = true
    }

    @objc private func columnsChanged() {
        header.needsDisplay = true
        header.window?.invalidateCursorRects(for: header)
    }

    // MARK: view mode

    func setViewMode(_ mode: ViewMode) {
        guard mode != viewMode else { return }
        let keep = selectedRows
        let wasFocused = isFocused
        viewMode = mode
        // one layout everywhere: it becomes the default; else this folder remembers it
        if AppSettings.syncLayout { UserDefaults.standard.set(mode.rawValue, forKey: "viewMode") }
        rememberLayout()
        applyViewMode()
        reloadItems()
        selectRows(keep, scrollToFirst: true)
        if wasFocused { focus() }
        selectionChanged()
    }

    func applyViewModeQuietly() { applyViewMode() }

    private func applyViewMode() {
        let isTiles = viewMode.isGrid
        let columns = viewMode == .columns
        header.isHidden = isTiles || columns
        scroll.isHidden = isTiles
        tileScroll.isHidden = !isTiles
        // Columns: the list is the open folder's column, its name only, beside the others
        for c in table.tableColumns where c.identifier.rawValue != ListColumn.name.rawValue { c.isHidden = columns }
        (millerLeft + [millerNext]).forEach { $0.isHidden = !columns }
        if columns { updateMillerColumns() }
        card.needsLayout = true
        // the grid's cells take the style's size
        let size = viewMode.cellSize
        if tileLayout.itemSize != size {
            tileLayout.itemSize = size
            tileLayout.invalidateLayout()
            card.needsLayout = true
        }
    }

    /// The selected rows of whichever view is showing.
    var selectedRows: IndexSet {
        switch viewMode {
        case .details, .columns: table.selectedRowIndexes
        case .tiles, .cards, .list: IndexSet(tiles.selectionIndexPaths.map(gridRow))
        }
    }

    private func selectRows(_ rows: IndexSet, scrollToFirst: Bool) {
        switch viewMode {
        case .details, .columns:
            let rows = rows.subtracting(IndexSet(foldedRows))
            table.selectRowIndexes(rows, byExtendingSelection: false)
            if scrollToFirst, let first = rows.first { table.scrollRowToVisible(first) }
        case .tiles, .cards, .list:
            let rows = rows.subtracting(IndexSet(foldedRows))
            let paths = Set(rows.map(gridPath))
            tiles.selectionIndexPaths = paths
            if scrollToFirst, let first = rows.first {
                tiles.scrollToItems(at: [gridPath(first)], scrollPosition: .nearestHorizontalEdge)
            }
        }
    }

    private func reloadItems() {
        computeGroups()
        listWidths = nil
        switch viewMode {
        case .details, .columns: table.reloadData()
        case .tiles, .cards, .list: tiles.reloadData()
        }
    }

    /// A newly opened folder starts at its first item.
    private func scrollToTop() {
        switch viewMode {
        case .details, .columns:
            scroll.contentView.scroll(to: NSPoint(x: 0, y: -scroll.contentInsets.top))
            scroll.reflectScrolledClipView(scroll.contentView)
        case .tiles, .cards, .list:
            tiles.layoutSubtreeIfNeeded()
            tileScroll.contentView.scroll(to: .zero)
            tileScroll.reflectScrolledClipView(tileScroll.contentView)
        }
    }

    private var isFocused: Bool {
        let r = table.window?.firstResponder
        return r === table || r === tiles
    }

    // MARK: Settings

    /// The Settings page fills the card; the command bar, the list and the status line go.
    func showSettings(page: SettingsPage? = nil) {
        folder = nil
        url = nil
        homeView?.isHidden = true
        diskView?.isHidden = true
        let v = settingsView ?? SettingsView()
        if settingsView == nil {
            settingsView = v
            card.addSubview(v)
        }
        v.frame = card.bounds
        v.isHidden = false
        if let page { v.show(page) }
        [commandBar, header, scroll, tileScroll, status, message].forEach { $0.isHidden = true }
        selectionChanged()
    }

    /// The Home page fills the card, as Settings does.
    func showHome() {
        folder = nil
        url = nil
        archive = nil
        settingsView?.isHidden = true
        diskView?.isHidden = true
        let v = homeView ?? HomeView()
        if homeView == nil {
            homeView = v
            v.onOpenFolder = { [weak self] url in self?.onOpenFolder?(url) }
            card.addSubview(v)
        }
        v.frame = card.bounds
        v.isHidden = false
        [commandBar, header, scroll, tileScroll, status, message].forEach { $0.isHidden = true }
        selectionChanged()
    }

    func reloadHome() { homeView?.reload() }

    /// The disk usage page for `root` fills the card; returns the walk (the tab keeps it).
    func showDiskUsage(_ root: URL, scan: DiskScan?, onRescan: @escaping (DiskScan) -> Void) -> DiskScan {
        folder = nil
        url = nil
        archive = nil
        settingsView?.isHidden = true
        homeView?.isHidden = true
        let v = diskView ?? DiskUsageView()
        if diskView == nil {
            diskView = v
            v.onShowInFolder = { [weak self] u in self?.onShowInFolder?(u) }
            v.onOpenInNewTab = { [weak self] u in self?.onOpenInNewTab?(u) }
            card.addSubview(v)
        }
        v.onRescan = onRescan
        v.frame = card.bounds
        v.isHidden = false
        [commandBar, header, scroll, tileScroll, status, message].forEach { $0.isHidden = true }
        previewPane.isHidden = true
        previewHandle.isHidden = true
        selectionChanged()
        return v.show(root, scan: scan)
    }

    /// Opens an item's folder with it selected.
    var onShowInFolder: ((URL) -> Void)?
    /// The disk usage page for a folder.
    var onDiskUsage: ((URL) -> Void)?

    private func hideSettings() {
        let pages = [settingsView as NSView?, homeView as NSView?, diskView as NSView?].compactMap { $0 }.filter { !$0.isHidden }
        guard !pages.isEmpty else { return }
        pages.forEach { $0.isHidden = true }
        commandBar.isHidden = false
        status.isHidden = false
        applyViewMode()
    }

    /// A setting changed (`key` as in AppSettings).
    func apply(settingKey key: String) {
        switch key {
        case "sortKey", "descending":
            sort(by: AppSettings.sortKey, ascending: !AppSettings.descending)
        case "foldersFirst":
            foldersFirst = AppSettings.foldersFirst
            rearrange()
        case "showHidden":
            showHidden = AppSettings.showHidden
        case "hoverDetails":
            if folder != nil { reloadItems() }
        case "showSystemFiles":
            setShowSystemFiles(show: AppSettings.showSystemFiles)
            rearrange()
        case "viewMode":
            if settingsView?.isHidden ?? true {
                setViewMode(AppSettings.defaultView)
            } else {
                viewMode = AppSettings.defaultView
            }
        case "columns":
            rebuildColumns()
            if let url { loadColumnData(url) }
            applyColumnWidths()
            reloadItems()
        case "groupBy":
            groupBy = AppSettings.groupBy
            rearrange()
            rememberLayout()
        case "showScrollBars":
            applyScrollBars()
        case "previewDetails":
            previewPane.needsLayout = true
        case "showCommandBar", "showStatusBar", "previewPosition", "toolbarHidden", "toolbarAdded":
            commandBar.needsLayout = true
            card.needsLayout = true
        case "listBackground", "listImage", "listImageVisibility":
            card.applyBackground()
        case "fontFamily", "textPercent":
            table.rowHeight = Metrics.rowHeight
            reloadItems()
            card.needsLayout = true
        case "dateStyle", "decimalSizes":
            applyColumnWidths()
            if folder != nil { reloadItems() }
        default:
            // formats, thumbnails and colours: what is on screen is drawn again
            if folder != nil { reloadItems() }
        }
    }

    // MARK: showing a folder

    func show(_ folder: Folder, at url: URL, selecting names: [String], scrollTo y: CGFloat? = nil, archive: ArchiveView? = nil) {
        let same = self.url == url
        self.folder = folder
        self.url = url
        self.archive = archive
        if isGathered != columnsGathered { rebuildColumns() }
        arrange(folder)
        hideSettings()
        loadColumnData(url)
        message.isHidden = true
        if card.subviews.contains(privacyButton) { privacyButton.isHidden = true }
        header.sortKey = sortKey
        header.ascending = ascending
        if !same { ThumbnailCache.shared.forget(under: url.path) }
        applyColumnWidths()
        reloadItems()
        adaptLayout(for: url)
        updateMillerColumns()
        if !same { refreshGit() }
        let rows = IndexSet(names.compactMap { folder.rowOf(name: $0).map(Int.init) })
        selectRows(rows, scrollToFirst: y == nil)
        if let y {
            let clip = (viewMode == .details ? scroll : tileScroll).contentView
            clip.scroll(to: NSPoint(x: 0, y: y))
            (viewMode == .details ? scroll : tileScroll).reflectScrolledClipView(clip)
        } else if rows.isEmpty && !same {
            scrollToTop()
        }
        selectionChanged()
    }

    /// Under the message when the folder couldn't be read for want of permission.
    private lazy var privacyButton: NSButton = {
        let b = ClosureButton(title: String(localized: "Open Privacy Settings")) { TrashBin.openPrivacySettings() }
        b.bezelStyle = .push
        b.isHidden = true
        card.addSubview(b)
        return b
    }()

    func show(error text: String, at url: URL) {
        defer {
            privacyButton.isHidden = !text.contains("Full Disk Access")
            privacyButton.sizeToFit()
            privacyButton.frame.origin = NSPoint(x: (card.bounds.width - privacyButton.frame.width) / 2, y: message.frame.maxY + 8)
        }
        folder = nil
        archive = nil
        self.url = url
        hideSettings()
        reloadItems()
        message.stringValue = text
        message.isHidden = false
        selectionChanged()
    }

    var scrollY: CGFloat { (viewMode == .details ? scroll : tileScroll).contentView.bounds.origin.y }

    func focus() { table.window?.makeFirstResponder(viewMode.isGrid ? tiles : table) }

    private func rearrange() {
        guard let folder else { return }
        let keep = selectedNames
        arrange(folder)
        reloadItems()
        let rows = IndexSet(keep.compactMap { folder.rowOf(name: $0).map(Int.init) })
        selectRows(rows, scrollToFirst: true)
        selectionChanged()
    }

    func sort(by col: ListColumn, ascending asc: Bool? = nil) {
        if let asc {
            sortKey = col
            ascending = asc
        } else if col == sortKey {
            ascending.toggle()
        } else {
            sortKey = col
            ascending = col.startsAscending
        }
        header.sortKey = sortKey
        header.ascending = ascending
        rearrange()
        rememberLayout()
    }

    var selectedNames: [String] {
        guard let folder else { return [] }
        return selectedRows.compactMap { folder.item(row: UInt32($0))?.name }
    }

    var selectedURLs: [URL] {
        guard let folder else { return [] }
        return selectedRows.compactMap { folder.childPath(row: UInt32($0)).map(URL.init(fileURLWithPath:)) }
    }

    /// Puts the lens under the one selected row, or hides it.
    private func placeListLens(animated: Bool) {
        let rows = viewMode == .details ? table.selectedRowIndexes : []
        let lensRow = rows.count == 1 && AppSettings.isGlass ? rows.first! : -1
        table.enumerateAvailableRowViews { view, row in (view as? FileRowView)?.onLens = row == lensRow }
        guard lensRow >= 0, lensRow < table.numberOfRows else {
            listLens.isHidden = true
            return
        }
        var r = table.rect(ofRow: lensRow)
        if groupTitles[lensRow] != nil {
            r.origin.y += GroupMetrics.header
            r.size.height -= GroupMetrics.header
        }
        let box = NSRect(x: 6, y: r.minY, width: table.bounds.width - 6 - (6 + geometry.gutter), height: r.height)
        listLens.move(to: box, animated: animated && !table.inLiveResize)
    }

    private func selectionChanged() {
        if AppSettings.selectionBoxes, viewMode == .details {
            let sel = table.selectedRowIndexes
            table.enumerateAvailableRowViews { _, row in
                (self.table.view(atColumn: 0, row: row, makeIfNecessary: false) as? NameCellView)?.checked = sel.contains(row)
            }
        }
        placeListLens(animated: true)
        updateMillerNext()
        updateStatus()
        updateCommands()
        refreshQuickLook()
        updatePreview()
        onChange?()
    }

    private func updateStatus() {
        guard let folder else {
            status.count = ""
            status.selection = ""
            return
        }
        let n = isFolded ? groupCounts.values.reduce(0, +) : Int(folder.count())
        status.count = n == 1 ? String(localized: "1 item") : String(localized: "\(n) items")
        let rows = selectedRows
        guard !rows.isEmpty else {
            status.selection = ""
            return
        }
        var text = rows.count == 1 ? String(localized: "1 item selected") : String(localized: "\(rows.count) items selected")
        let files = rows.compactMap { folder.item(row: UInt32($0)) }.filter { !$0.isDir }
        if !files.isEmpty { text += "  " + formatSize(bytes: files.reduce(0) { $0 + $1.size }) }
        status.selection = text
    }

    func updateCommands() {
        let any = !selectedRows.isEmpty && folder != nil
        // an archive is read only here: what is in it can be opened, copied and extracted
        let writable = folder != nil && (archive == nil || archiveEditable)
        let here = writable && !isGathered
        commandBar.cut.isEnabled = any && writable
        commandBar.copy.isEnabled = any
        commandBar.rename.isEnabled = selectedRows.count == 1 && writable
        commandBar.share.isEnabled = any && writable
        commandBar.delete.isEnabled = any && writable
        commandBar.paste.isEnabled = archive != nil ? archiveEditable && Clipboard.hasFiles : here && Self.canPaste
        let inTrash = TrashBin.contains(url) && archive == nil
        commandBar.trashMode = inTrash
        commandBar.new.isEnabled = inTrash ? (folder?.count() ?? 0) > 0 : here
        commandBar.share.isEnabled = any && writable && !inTrash
        commandBar.properties.isEnabled = writable
        commandBar.preview.isEnabled = !pageShown
        commandBar.preview.isOn = previewShown
        commandBar.view.icon = viewMode == .details ? "list" : "layout-grid"
        commandBar.showsGroups = groupsApply && !pageShown && !groupCounts.isEmpty
        let folded = allGroupsFolded
        commandBar.groups.icon = folded ? "chevrons-up-down" : "chevrons-down-up"
        commandBar.groups.toolTip = folded ? String(localized: "Expand all groups") : String(localized: "Collapse all groups")
    }

    // MARK: command bar

    private func wireCommandBar() {
        let b = commandBar
        b.newFolder.action = { [weak self] _ in self?.newFolder() }
        b.terminal.action = { [weak self] _ in if let u = self?.url { MenuActions.openInTerminal(u) } }
        b.hiddenItems.action = { _ in AppSettings.showHidden.toggle() }
        b.refresh.action = { [weak self] _ in self?.onRefresh?() }
        b.groups.action = { [weak self] _ in self?.toggleAllGroups() }
        b.new.action = { [weak self] btn in
            if self?.commandBar.trashMode == true { return self?.emptyTrash() ?? () }
            let m = NSMenu()
            m.addItem(withTitle: String(localized: "Folder"), action: #selector(BrowserWindowController.newFolder(_:)), keyEquivalent: "")
            m.items.first?.image = Icons.menu("folder")
            btn.pop(m)
            _ = self
        }
        b.cut.action = { [weak self] _ in self?.cutItems() }
        b.copy.action = { [weak self] _ in self?.copyItems() }
        b.paste.action = { [weak self] _ in self?.pasteItems() }
        b.rename.action = { [weak self] _ in self?.beginRename() }
        b.preview.action = { [weak self] _ in self?.togglePreviewPane(nil) }
        b.properties.action = { [weak self] _ in self?.showProperties() }
        b.delete.action = { [weak self] _ in self?.trashSelected() }
        b.share.action = { [weak self] btn in
            guard let self else { return }
            NSSharingServicePicker(items: self.selectedURLs).show(relativeTo: btn.bounds, of: btn, preferredEdge: .minY)
        }
        b.sort.action = { [weak self] btn in
            guard let self else { return }
            let m = NSMenu()
            for col in ListColumn.allCases.filter(\.sortable) {
                let i = NSMenuItem(title: col.title, action: #selector(self.sortMenu(_:)), keyEquivalent: "")
                i.target = self
                i.representedObject = col.rawValue
                i.state = col == self.sortKey ? .on : .off
                m.addItem(i)
            }
            m.addItem(.separator())
            for (title, asc) in [(String(localized: "Ascending"), true), (String(localized: "Descending"), false)] {
                let i = NSMenuItem(title: title, action: #selector(self.sortDirection(_:)), keyEquivalent: "")
                i.target = self
                i.tag = asc ? 1 : 0
                i.state = self.ascending == asc ? .on : .off
                m.addItem(i)
            }
            btn.pop(m)
        }
        b.view.action = { [weak self] btn in
            guard let self else { return }
            LayoutFlyout.show(from: btn, list: self)
        }
    }

    @objc private func sortMenu(_ sender: NSMenuItem) {
        if let raw = sender.representedObject as? String, let col = ListColumn(rawValue: raw) { sort(by: col, ascending: col == sortKey ? ascending : col.startsAscending) }
    }

    @objc private func sortDirection(_ sender: NSMenuItem) { sort(by: sortKey, ascending: sender.tag == 1) }

    // MARK: file operations

    func cutItems() {
        Clipboard.put(selectedURLs, cut: true)
        updateCommands()
    }

    func copyItems() {
        if let archive {
            return extractSelected(from: archive) { urls in Clipboard.put(urls, cut: false) }
        }
        Clipboard.put(selectedURLs, cut: false)
        updateCommands()
    }

    /// The paths inside the archive of the selected rows.
    private func selectedInner(_ archive: ArchiveView) -> [String] {
        guard let folder else { return [] }
        return selectedRows.compactMap { folder.item(row: UInt32($0)).map { archive.place.child($0.name) } }
    }

    /// Writes the selected members of the archive into a scratch folder, then hands their
    /// new files to `done` (to open, preview or copy them).
    private func extractSelected(from archive: ArchiveView, done: @escaping ([URL]) -> Void) {
        let inner = selectedInner(archive)
        guard !inner.isEmpty else { return }
        let a = archive.archive
        run {
            let dest = ArchiveStore.scratchFolder()
            _ = try a.extract(innerPaths: inner, dest: dest.path)
            return inner.map { dest.appendingPathComponent($0) }
        } done: { done($0) }
    }

    /// Extracts the selected members (or, with none selected, the whole archive) into `dest`
    /// and opens that folder.
    func extractArchive(_ archive: ArchiveView, to dest: URL, onlySelected: Bool) {
        let inner = onlySelected ? selectedInner(archive) : []
        let a = archive.archive
        run { _ = try a.extract(innerPaths: inner, dest: dest.path) } done: { [weak self] in self?.onOpenFolder?(dest) }
    }

    /// "Extract here" for an archive file in a folder: as on Windows, into a new folder named
    /// after it unless everything in it is under one folder already.
    private func extractHere(_ file: URL) {
        let path = file.path, into = file.deletingLastPathComponent().path
        run {
            let target = smartExtractTarget(archive: path, into: into)
            let a = try Archive.open(path: path, password: nil)
            _ = try a.extractAll(dest: target[0])
            return target[1]
        } done: { [weak self] shown in self?.reloadSelecting([URL(fileURLWithPath: shown).lastPathComponent]) }
    }

    /// Asks where to extract, then extracts there.
    func chooseAndExtract(_ work: @escaping (URL) -> Void) {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.prompt = String(localized: "Extract")
        guard let window = card.window else { return }
        panel.beginSheetModal(for: window) { response in
            if response == .OK, let url = panel.url { work(url) }
        }
    }

    /// Pastes here, or into `target` (a folder that was right-clicked).
    func pasteItems(into target: URL? = nil) {
        guard let dest = target ?? url else { return }
        if archive != nil {
            // into the archive: a copy of the files on the clipboard (a cut stays where it is)
            let pb = NSPasteboard.general
            if let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] { addToArchive(urls) }
            return
        }
        guard let (urls, move) = Clipboard.take() else { return pasteAsFile(into: dest) }
        transfer(urls, into: dest, kind: move ? .move : .copy, selectHere: target == nil)
    }

    /// Text or a picture on the clipboard (no files): a new file with it, "Pasted text.txt"
    /// or "Pasted picture.png", as on Windows; it is selected for renaming.
    func pasteAsFile(into dest: URL) {
        let pb = NSPasteboard.general
        var data: Data?
        var name = "", ext = ""
        if let image = NSImage(pasteboard: pb), pb.availableType(from: [.png, .tiff]) != nil,
            let tiff = image.tiffRepresentation, let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:])
        {
            data = png
            name = String(localized: "Pasted picture")
            ext = "png"
        } else if let text = pb.string(forType: .string), HyperDownload.isLink(text), HyperDownload.isInstalled {
            // a link and no files: HyperDownload offers to download it here
            return HyperDownload.offerPastedLink(text.trimmingCharacters(in: .whitespacesAndNewlines), into: dest)
        } else if let text = pb.string(forType: .string), !text.isEmpty {
            data = Data(text.utf8)
            name = String(localized: "Pasted text")
            ext = "txt"
        }
        guard let data else { return }
        run { [name, ext] in
            let target = FileOps.unique(name, ext: ext, in: dest)
            try data.write(to: target, options: .withoutOverwriting)
            return target
        } done: { [weak self] made in
            FileUndo.shared.record(.trash([made]), title: String(localized: "Paste"))
            self?.reloadSelecting([made.lastPathComponent], rename: true)
        }
    }

    /// Something to paste: files, or text or a picture to make a file of.
    static var canPaste: Bool {
        let pb = NSPasteboard.general
        return Clipboard.hasFiles || pb.availableType(from: [.string, .png, .tiff]) != nil
    }

    /// File › Duplicate (⌘D): a copy of each selected item beside it, "name - Copy".
    func duplicateSelected() {
        guard let url, archive == nil, !selectedURLs.isEmpty else { return }
        transfer(selectedURLs, into: url, kind: .copy, selectHere: true)
    }

    /// Copies or moves `urls` into `dest` with progress, conflicts asked, undo recorded.
    func transfer(_ urls: [URL], into dest: URL, kind: Transfer.Kind, selectHere: Bool) {
        Transfer(kind, urls, into: dest).start(window: card.window) { [weak self] made in
            if selectHere { self?.reloadSelecting(made.map(\.lastPathComponent)) }
        }
    }

    func newFolder() {
        guard let url else { return }
        run { try FileOps.newFolder(in: url) } done: { [weak self] made in
            FileUndo.shared.record(.trash([made]), title: String(localized: "New Folder"))
            self?.reloadSelecting([made.lastPathComponent], rename: true)
        }
    }

    func trashSelected() {
        if archive != nil { return deleteFromArchive() }
        let urls = selectedURLs
        guard !urls.isEmpty else { return }
        // in the Trash, "delete" is for good, asked first
        if TrashBin.contains(url) { return deleteImmediately(urls) }
        if AppSettings.confirmDelete == .always, !Self.confirmTrash(urls) { return }
        // the row after the last one goes away keeps the cursor near where it was
        let next = (selectedRows.last ?? 0) + 1
        let after = folder.flatMap { f in next < Int(f.count()) ? f.item(row: UInt32(next))?.name : nil }
        run { try FileOps.trash(urls) } done: { [weak self] pairs in
            FileUndo.shared.record(.moves(pairs), title: String(localized: "Move to Trash"))
            self?.reloadSelecting(after.map { [$0] } ?? [])
        }
    }

    /// Runs `work` off the main thread; errors are shown, the result goes to `done`.
    func run<T: Sendable>(_ work: @escaping @Sendable () throws -> T, done: @escaping (T) -> Void) {
        Task.detached(priority: .userInitiated) {
            let result = Result { try work() }
            await MainActor.run { [weak self] in
                switch result {
                case .success(let v): done(v)
                case .failure(let e): self?.onError?(e.localizedDescription)
                }
            }
        }
    }

    /// Asks the window to list the folder again, then selects `names` (and renames the first).
    var onReload: (([String], Bool) -> Void)?

    private func reloadSelecting(_ names: [String], rename: Bool = false) { onReload?(names, rename) }

    /// The Layout flyout's tile size changed: the grid's cells follow.
    /// Bigger or smaller icons (⌘+ / ⌘−, a pinch, ⌘ with the wheel; folder_view.rs `zoom`):
    /// another layout grows into Tiles at their smallest, Tiles step through their sizes,
    /// and the smallest Tiles shrink back into Details.
    func zoom(_ delta: Int) {
        guard !pageShown else { return }
        let size = TileMetrics.size
        if viewMode != .tiles {
            guard delta > 0 else { return }
            UserDefaults.standard.set(0, forKey: "tileSize")
            setViewMode(.tiles)
            return
        }
        if delta < 0 && size == 0 { return setViewMode(.details) }
        let n = min(max(size + delta, 0), TileMetrics.sizes.count - 1)
        guard n != size else { return }
        UserDefaults.standard.set(n, forKey: "tileSize")
        tileSizeChanged()
    }

    /// Settings › Select on hover: the row the pointer rested on is selected; with ⌘ held it
    /// joins the selection, with ⇧ the selection runs from the last one to it (Explorer's
    /// "point to select", Windows issue 101).
    func hoverSelect(_ row: Int) {
        guard !foldedRows.contains(row), !selectedRows.contains(row) else { return }
        let mods = NSEvent.modifierFlags
        var rows = IndexSet(integer: row)
        if mods.contains(.shift), let anchor = hoverAnchor ?? selectedRows.first {
            rows = IndexSet(integersIn: min(anchor, row)...max(anchor, row))
        } else if mods.contains(.command) {
            rows = IndexSet(selectedRows).union(rows)
        }
        if !mods.contains(.shift) { hoverAnchor = row }
        selectRows(rows, scrollToFirst: false)
        selectionChanged()
    }

    /// A pinch on the trackpad zooms one step each time it has opened or closed far enough.
    func pinched(_ event: NSEvent) {
        if event.phase == .began { pinch = 0 }
        pinch += event.magnification
        if abs(pinch) >= 0.3 {
            zoom(pinch > 0 ? 1 : -1)
            pinch = 0
        }
    }

    /// ⌘ with the wheel zooms instead of scrolling; false: an ordinary scroll.
    func commandScrolled(_ event: NSEvent) -> Bool {
        guard event.modifierFlags.contains(.command) else { return false }
        pinch += event.scrollingDeltaY * (event.hasPreciseScrollingDeltas ? 0.01 : 0.1)
        if abs(pinch) >= 0.3 {
            zoom(pinch > 0 ? 1 : -1)
            pinch = 0
        }
        return true
    }

    func tileSizeChanged() {
        // the pictures are asked for again at the new size
        ThumbnailCache.shared.clear()
        applyViewMode()
        if viewMode == .tiles { tiles.reloadData() }
    }

    /// Adds the selected items to the shelf, and shows it.
    func addToShelf() {
        guard archive == nil, ShelfStore.add(selectedURLs) > 0 || !selectedURLs.isEmpty else { return }
        shelfShown = true
    }

    /// Bulk rename for the selected items (those in the folder of the first one).
    func bulkRename() {
        let urls = selectedURLs
        guard urls.count > 1 else { return }
        BulkRenameSheet.show(urls, over: table.window) { [weak self] names in
            if !names.isEmpty { self?.reloadSelecting(names) }
        }
    }

    /// "Create archive…": the dialog, then the archive in the background.
    func createArchive(_ urls: [URL]) {
        guard let c = CreateArchiveDialog.ask(for: urls) else { return }
        run {
            try compressArchive(
                sources: urls.map(\.path), dest: c.dest.path, format: c.format, level: c.level, password: c.password, volumeSize: c.volumeSize, dictSize: c.dictSize)
        } done: { [weak self] made in
            let u = URL(fileURLWithPath: made)
            FileUndo.shared.record(.trash([u]), title: String(localized: "Compress"))
            self?.reloadSelecting([u.lastPathComponent])
        }
    }

    // MARK: rename

    private var renaming: (row: Int, original: String)?

    /// The name field of row `row` in the view that is showing.
    private func nameField(_ row: Int, make: Bool) -> NSTextField? {
        switch viewMode {
        case .details, .columns:
            return (table.view(atColumn: 0, row: row, makeIfNecessary: make) as? NameCellView)?.textField
        case .tiles, .cards, .list:
            let path = gridPath(row)
            if make { tiles.scrollToItems(at: [path], scrollPosition: .nearestHorizontalEdge) }
            return (tiles.item(at: path) as? TileItem)?.tile.label
        }
    }

    func beginRename() {
        // several items: the bulk rename (bulk_rename.rs), as F2 does on Windows
        if selectedRows.count > 1, archive == nil { return bulkRename() }
        if archive != nil, !archiveEditable { return }
        guard let folder, selectedRows.count == 1, let row = selectedRows.first,
            let item = folder.item(row: UInt32(row))
        else { return }
        if viewMode == .details { table.scrollRowToVisible(row) }
        guard let field = nameField(row, make: true) else { return }
        renaming = (row, item.name)
        field.stringValue = item.name
        field.isEditable = true
        field.isSelectable = true
        field.isBordered = true
        field.drawsBackground = true
        field.backgroundColor = Palette.of(table).cardBg
        field.delegate = self
        table.window?.makeFirstResponder(field)
        // the name without its extension is selected, as on Windows
        let stem = item.isDir ? item.name.count : ((item.name as NSString).deletingPathExtension as NSString).length
        field.currentEditor()?.selectedRange = NSRange(location: 0, length: stem == 0 ? (item.name as NSString).length : stem)
    }

    private func endRename(commit: Bool) {
        guard let (row, original) = renaming else { return }
        renaming = nil
        guard let field = nameField(row, make: false) else { return }
        let newName = field.stringValue.trimmingCharacters(in: .whitespaces)
        field.isEditable = false
        field.isSelectable = false
        field.isBordered = false
        field.drawsBackground = false
        field.delegate = nil
        focus()
        guard commit, !newName.isEmpty, newName != original, let url else {
            field.stringValue = display.name(original: original)
            return
        }
        if archive != nil {
            let from = memberPath(original)
            editArchive { try $0.rename(from: from, toName: newName) }
            return
        }
        let from = folder?.childPath(row: UInt32(row)).map(URL.init(fileURLWithPath:)) ?? url.appendingPathComponent(original)
        let oldExt = (original as NSString).pathExtension, newExt = (newName as NSString).pathExtension
        let isFile = !((try? from.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) ?? false)
        if isFile, AppSettings.warnExtensionChange, oldExt.lowercased() != newExt.lowercased() {
            let alert = NSAlert()
            alert.messageText = newExt.isEmpty
                ? String(localized: "Remove the extension “.\(oldExt)”?")
                : String(localized: "Change the extension to “.\(newExt)”?")
            alert.informativeText = String(localized: "If you change it, the file may open in another app, or not at all.")
            alert.addButton(withTitle: newExt.isEmpty ? String(localized: "Remove") : String(localized: "Use .\(newExt)"))
            alert.addButton(withTitle: oldExt.isEmpty ? String(localized: "Cancel") : String(localized: "Keep .\(oldExt)"))
            if alert.runModal() != .alertFirstButtonReturn {
                if !oldExt.isEmpty, oldExt.lowercased() != newExt.lowercased() {
                    // keep the old extension: the new name with it
                    let stem = newExt.isEmpty ? newName : (newName as NSString).deletingPathExtension
                    let kept = stem + "." + oldExt
                    if kept != original {
                        run { try FileOps.rename(from, to: kept) } done: { [weak self] made in
                            FileUndo.shared.record(.moves([(from: made, to: from)]), title: String(localized: "Rename"))
                            self?.reloadSelecting([made.lastPathComponent])
                        }
                    }
                }
                field.stringValue = display.name(original: original)
                return
            }
        }
        run { try FileOps.rename(from, to: newName) } done: { [weak self] made in
            FileUndo.shared.record(.moves([(from: made, to: from)]), title: String(localized: "Rename"))
            self?.reloadSelecting([made.lastPathComponent])
        }
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        switch selector {
        case #selector(NSResponder.insertNewline(_:)):
            endRename(commit: true)
            return true
        case #selector(NSResponder.cancelOperation(_:)):
            endRename(commit: false)
            return true
        default:
            return false
        }
    }

    func controlTextDidEndEditing(_ note: Notification) { endRename(commit: true) }

    // MARK: opening

    @objc private func doubleClicked(_ sender: Any?) {
        guard table.clickedRow >= 0 else {
            // Files › Double-click empty space to go up
            if AppSettings.doubleClickEmptyGoesUp { onGoUp?() }
            return
        }
        openSelected()
    }

    /// Files › Open items with a single click: a plain click (no ⌘ or ⇧) opens.
    @objc private func singleClicked(_ sender: Any?) {
        let row = table.clickedRow
        guard row >= 0, AppSettings.singleClick != .never, NSEvent.modifierFlags.intersection([.command, .shift]).isEmpty,
            let item = folder?.item(row: UInt32(row))
        else { return }
        if AppSettings.singleClick == .always || (item.isDir && !display.isPackage(item)) { openSelected() }
    }

    var onGoUp: (() -> Void)?
    /// The list (or its grid) took the keyboard: in a dual pane, its pane becomes the active one.
    var onFocus: (() -> Void)?
    /// Whether the columns were made for a gathered listing (with the Folder column).
    var columnsGathered = false
    /// The grid's groups: the first row of each, and its title.
    var gridStarts: [Int] = [0]
    var gridTitles: [String] = []

    func openSelected() {
        guard let folder else { return }
        let rows = selectedRows
        var files: [URL] = []
        for row in rows {
            guard let item = folder.item(row: UInt32(row)), let path = folder.childPath(row: UInt32(row)) else { continue }
            let u = URL(fileURLWithPath: path)
            if item.isDir && !display.isPackage(item) {
                if rows.count == 1 { (AppSettings.foldersInNewTab && archive == nil ? onOpenInNewTab : onOpenFolder)?(u) }
            } else if archive == nil && rows.count == 1 && AppSettings.archivesAsFolders && isArchive(path: path) {
                // archives open like folders, as on Windows: here, or in a tab or window of their own
                switch AppSettings.archivesOpenIn {
                case "tab": onOpenInNewTab?(u)
                case "window": onOpenInNewWindow?(u)
                default: onOpenFolder?(u)
                }
            } else if archive == nil && rows.count == 1 && DiskImages.isDiskImage(u) {
                onOpenDiskImage?(u)
            } else {
                files.append(u)
            }
        }
        guard !files.isEmpty else { return }
        if let archive {
            // what is in an archive is written out first, then opened
            let fileNames = Set(files.map(\.lastPathComponent))
            let inner = rows.compactMap { folder.item(row: UInt32($0)) }.filter { fileNames.contains($0.name) && !$0.isDir }.map { archive.place.child($0.name) }
            let a = archive.archive
            run {
                let dest = ArchiveStore.scratchFolder()
                _ = try a.extract(innerPaths: inner, dest: dest.path)
                return inner.map { dest.appendingPathComponent($0) }
            } done: { urls in urls.forEach { NSWorkspace.shared.open($0) } }
            return
        }
        for u in files {
            // Settings › File types: the user's own app for this kind of file
            if let app = OpenRules.opener(for: u) {
                NSWorkspace.shared.open([u], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration())
            } else {
                NSWorkspace.shared.open(u)
            }
        }
    }

    // MARK: context menus (ctx_menu.rs)

    var archiveIsNil: Bool { archive == nil }

    // MARK: editing inside an archive (rf_archive::edit: ZIP, 7z, the tar family)

    /// Files can be added, deleted and renamed in the archive shown.
    var archiveEditable: Bool { archive?.archive.isEditable() ?? false }

    /// A member's path inside the archive, for a name in the folder shown.
    private func memberPath(_ name: String) -> String {
        guard let inner = archive?.place.inner, !inner.isEmpty else { return name }
        return inner + "/" + name
    }

    /// Rewrites the archive off the main thread, then lists it again.
    private func editArchive(_ work: @escaping @Sendable (Archive) throws -> Void) {
        guard let a = archive?.archive else { return }
        run { try work(a) } done: { [weak self] _ in self?.onRefresh?() }
    }

    /// Files from the disk into the folder shown in the archive (paste, a drop, "Add files…").
    func addToArchive(_ urls: [URL]) {
        guard archiveEditable, !urls.isEmpty, let inner = archive?.place.inner else { return }
        let paths = urls.map(\.path)
        editArchive { _ = try $0.add(sources: paths, innerDir: inner) }
    }

    /// The selected members taken out of the archive, asked first (it cannot be undone).
    func deleteFromArchive() {
        guard archiveEditable, let folder else { return }
        let names = selectedRows.compactMap { folder.item(row: UInt32($0))?.name }
        guard !names.isEmpty else { return }
        let alert = NSAlert()
        alert.messageText = names.count == 1
            ? String(localized: "Delete “\(names[0])” from the archive?")
            : String(localized: "Delete \(names.count) items from the archive?")
        alert.informativeText = String(localized: "This can't be undone.")
        alert.alertStyle = .warning
        alert.addButton(withTitle: String(localized: "Delete"))
        alert.addButton(withTitle: String(localized: "Cancel"))
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let members = names.map(memberPath)
        editArchive { _ = try $0.delete(innerPaths: members) }
    }
    /// A gathered listing (a tag's files): its items are real, but there is no folder to put
    /// new things in.
    var isGathered: Bool { url.map { !$0.isFileURL } ?? false }
    var onOpenInNewTab: ((URL) -> Void)?
    var onOpenInNewWindow: ((URL) -> Void)?
    var onRefresh: (() -> Void)?

    /// The menu of an item in a side column of the Columns view: open it, here or in a new tab,
    /// show it in its folder, copy it or its path, put it in the Trash, its properties.
    func sideColumnMenu(_ u: URL, isDir: Bool) -> NSMenu {
        let b = MenuBuilder()
        b.add(String(localized: "Open"), "folder-open") { [weak self] in
            if isDir { self?.onOpenFolder?(u) } else { NSWorkspace.shared.open(u) }
        }
        if isDir { b.add(String(localized: "Open in new tab"), "plus") { [weak self] in self?.onOpenInNewTab?(u) } }
        b.add(String(localized: "Show in folder"), "folder-search") { [weak self] in self?.onReveal?(u) }
        b.separator()
        b.add(String(localized: "Copy"), "copy") { Clipboard.put([u], cut: false) }
        b.add(String(localized: "Copy path"), "clipboard-copy") { MenuActions.copyPaths([u]) }
        b.separator()
        b.add(String(localized: "Move to Trash"), "trash") {
            DispatchQueue.global(qos: .userInitiated).async {
                let pairs = (try? FileOps.trash([u])) ?? []
                DispatchQueue.main.async {
                    MainActor.assumeIsolated { FileUndo.shared.record(.moves(pairs), title: String(localized: "Move to Trash")) }
                }
            }
        }
        b.add(String(localized: "Properties"), "wrench") { PropertiesWindow.show([u]) }
        return b.done()
    }

    func contextMenu() -> NSMenu {
        if let archive { return archiveMenu(archive) }
        return selectedRows.isEmpty ? backgroundMenu() : itemsMenu()
    }

    /// Right-click on items (`items_menu`): the quick row, then open, the items' own commands,
    /// compress / extract / tags / Ferry, Terminal; macOS adds its Services at the end.
    // MARK: the Trash

    /// Files › Ask before deleting › Always: asked before the Trash too.
    static func confirmTrash(_ urls: [URL]) -> Bool {
        let alert = NSAlert()
        alert.messageText = urls.count == 1
            ? String(localized: "Move “\(urls[0].lastPathComponent)” to the Trash?")
            : String(localized: "Move these \(urls.count) items to the Trash?")
        alert.informativeText = String(localized: "You can put them back with ⌘Z.")
        alert.addButton(withTitle: String(localized: "Move to Trash"))
        alert.addButton(withTitle: String(localized: "Cancel"))
        return alert.runModal() == .alertFirstButtonReturn
    }

    func deleteImmediately(_ urls: [URL]) {
        guard AppSettings.confirmDelete == .never || TrashBin.confirmDelete(urls, everything: false, in: card.window) else { return }
        run { try TrashBin.delete(urls) } done: { [weak self] in self?.reloadSelecting([]) }
    }

    func emptyTrash() {
        let all = TrashBin.everything()
        guard !all.isEmpty, TrashBin.confirmDelete(all, everything: true, in: card.window) else { return }
        run { try TrashBin.delete(all) } done: { [weak self] in self?.reloadSelecting([]) }
    }

    func putBack(_ urls: [URL]) {
        let pairs = urls.compactMap { u in TrashBin.origin(of: u).map { (trashed: u, origin: $0) } }
        guard !pairs.isEmpty else { return }
        run { try TrashBin.putBack(pairs) } done: { done in
            FileUndo.shared.record(.moves(done), title: String(localized: "Put Back"))
        }
    }

    /// Right-click in the Trash: put back, delete for good, Quick Look, properties.
    private func trashItemsMenu(_ urls: [URL]) -> NSMenu {
        let b = MenuBuilder()
        let known = urls.filter { TrashBin.origin(of: $0) != nil }
        b.quick([
            ("copy", String(localized: "Copy (⌘C)"), true, { [weak self] in self?.copyItems() }),
            ("eraser", String(localized: "Delete immediately (⌘⌫)"), true, { [weak self] in self?.deleteImmediately(urls) }),
            ("wrench", String(localized: "Properties (⌘I)"), true, { PropertiesWindow.show(urls) }),
        ])
        let back = b.add(String(localized: "Put Back"), "undo-2", enabled: !known.isEmpty) { [weak self] in self?.putBack(known) }
        if known.count == 1, let o = TrashBin.origin(of: known[0]) {
            back.title = String(localized: "Put Back to “\(o.deletingLastPathComponent().lastPathComponent)”")
        }
        if known.isEmpty { back.toolTip = String(localized: "Moved to the Trash outside rustyfiles: Finder can put it back.") }
        b.add(String(localized: "Delete Immediately…"), "eraser") { [weak self] in self?.deleteImmediately(urls) }
        b.separator()
        b.add(String(localized: "Quick Look"), "eye", key: " ", mods: []) { [weak self] in self?.toggleQuickLook() }
        b.add(String(localized: "Open"), "square-arrow-out-up-right") { [weak self] in self?.openSelected() }
        b.separator()
        b.add(String(localized: "Empty Trash…"), "trash") { [weak self] in self?.emptyTrash() }
        return b.done()
    }

    /// General › Right-click menu: the commands turned off leave the item menu.
    private static func pruned(_ menu: NSMenu) -> NSMenu {
        typealias C = AppSettings.MenuCommand
        let titles: [C: [String]] = [
            .newTab: [String(localized: "Open in new tab")], .newWindow: [String(localized: "Open in new window")],
            .copyPath: [String(localized: "Copy item path")], .alias: [String(localized: "Make alias")],
            .pin: [String(localized: "Pin to sidebar"), String(localized: "Unpin from sidebar")],
            .archive: [String(localized: "Compress"), String(localized: "Extract")], .tags: [String(localized: "Tags")],
            .ferry: [String(localized: "Send with Ferry")], .terminal: [String(localized: "Open in Terminal")],
        ]
        let hidden = Set(C.allCases.filter { !AppSettings.shows($0) }.flatMap { titles[$0] ?? [] })
        for item in menu.items.reversed() where hidden.contains(item.title) { menu.removeItem(item) }
        // no separator twice in a row, or last
        var last: NSMenuItem?
        for item in menu.items {
            if item.isSeparatorItem, last?.isSeparatorItem ?? true { menu.removeItem(item) } else { last = item }
        }
        while menu.items.last?.isSeparatorItem == true { menu.removeItem(at: menu.items.count - 1) }
        return menu
    }

    private func itemsMenu() -> NSMenu {
        if TrashBin.contains(url) { return trashItemsMenu(selectedURLs) }
        let b = MenuBuilder()
        guard let folder else { return b.done() }
        let urls = selectedURLs
        let items = selectedRows.compactMap { folder.item(row: UInt32($0)) }
        let singleDir = items.count == 1 && items[0].isDir && !display.isPackage(items[0])
        let hasFiles = !singleDir
        let exts = urls.map { $0.pathExtension.lowercased() }
        let allArchives = !urls.isEmpty && urls.allSatisfy { isArchive(path: $0.path) }

        b.quick([
            ("scissors", String(localized: "Cut (⌘X)"), true, { [weak self] in self?.cutItems() }),
            ("copy", String(localized: "Copy (⌘C)"), true, { [weak self] in self?.copyItems() }),
            ("clipboard-paste", String(localized: "Paste (⌘V)"), Clipboard.hasFiles, { [weak self] in self?.pasteItems(into: singleDir ? urls.first : nil) }),
            ("text-cursor-input", String(localized: "Rename (F2)"), items.count >= 1 && archive == nil, { [weak self] in self?.beginRename() }),
            ("share-2", String(localized: "Share"), true, { [weak self] in self?.shareSelected() }),
            ("trash", String(localized: "Move to Trash (⌘⌫)"), true, { [weak self] in self?.trashSelected() }),
            ("wrench", String(localized: "Properties (⌘I)"), true, { PropertiesWindow.show(urls) }),
        ])

        b.add(String(localized: "Open"), "square-arrow-out-up-right", key: "\r", mods: []) { [weak self] in self?.openSelected() }
        let singleArchive = archive == nil && items.count == 1 && !items[0].isDir && AppSettings.archivesAsFolders && urls.first.map { isArchive(path: $0.path) } == true
        if singleDir || singleArchive, let u = urls.first {
            b.add(String(localized: "Open in new tab"), "square-plus") { [weak self] in self?.onOpenInNewTab?(u) }
            b.add(String(localized: "Open in new window"), "app-window") { [weak self] in self?.onOpenInNewWindow?(u) }
        }
        if hasFiles { b.sub(String(localized: "Open with"), "layout-grid") { MenuActions.openWithMenu(urls, $0) } }
        b.add(String(localized: "Quick Look"), "eye", key: " ", mods: []) { [weak self] in self?.toggleQuickLook() }
        b.separator()

        if items.count > 1 && archive == nil {
            b.add(String(localized: "Bulk rename…"), "pencil-line", key: String(Character(UnicodeScalar(NSF2FunctionKey)!)), mods: []) { [weak self] in self?.bulkRename() }
        }
        b.add(String(localized: "Copy item path"), "clipboard-copy", key: "c", mods: [.command, .option]) { MenuActions.copyPaths(urls) }
        if archive == nil { b.add(String(localized: "Add to shelf"), "archive-restore", key: "s", mods: [.command, .shift]) { [weak self] in self?.addToShelf() } }
        b.add(String(localized: "Make alias"), "link") { [weak self] in
            self?.run { try urls.map { try FileOps.makeAlias($0) } } done: { made in
                FileUndo.shared.record(.trash(made), title: String(localized: "Make Alias"))
                self?.reloadSelecting(made.map(\.lastPathComponent))
            }
        }
        if let dir = url, dir.isFileURL {
            b.add(String(localized: "New folder with selection"), "folder-input") { [weak self] in
                self?.run { try FileOps.newFolder(with: urls, in: dir) } done: { made in
                    let back = urls.map { (from: made.appendingPathComponent($0.lastPathComponent), to: $0) }
                    FileUndo.shared.record(.group([.moves(back), .trash([made])]), title: String(localized: "New Folder with Selection"))
                    self?.reloadSelecting([made.lastPathComponent], rename: true)
                }
            }
        }
        if singleDir, let u = urls.first {
            let pinned = MenuActions.isPinned(u)
            b.add(pinned ? String(localized: "Unpin from sidebar") : String(localized: "Pin to sidebar"), pinned ? "pin-off" : "pin") {
                MenuActions.setPinned(u, !pinned)
            }
        }
        if exts.allSatisfy(MenuActions.rotatable.contains) {
            b.add(String(localized: "Rotate right"), "rotate-cw") { [weak self] in
                self?.run { try FileOps.rotate(urls, clockwise: true) } done: { ThumbnailCache.shared.forget(under: urls[0].deletingLastPathComponent().path) }
            }
            b.add(String(localized: "Rotate left"), "rotate-ccw") { [weak self] in
                self?.run { try FileOps.rotate(urls, clockwise: false) } done: { ThumbnailCache.shared.forget(under: urls[0].deletingLastPathComponent().path) }
            }
        }
        if urls.count == 1, MenuActions.pictures.contains(exts[0]) {
            b.add(String(localized: "Set as desktop picture"), "monitor") { [weak self] in
                do { try MenuActions.setDesktopPicture(urls[0]) } catch { self?.onError?(error.localizedDescription) }
            }
        }
        if exts.allSatisfy(MenuActions.fonts.contains) {
            b.add(String(localized: "Install font"), "type") { MenuActions.installFonts(urls) }
        }
        b.separator()

        // a click on Compress makes the chosen format at once (Settings › Files); the submenu has the rest
        let compressOne: (ArchiveFormat, String) -> Void = { [weak self] format, ext in
            let dest = FileOps.archiveName(for: urls, ext: ext)
            self?.run { try compress(sources: urls.map(\.path), dest: dest.path, format: format) } done: {
                FileUndo.shared.record(.trash([dest]), title: String(localized: "Compress"))
                self?.reloadSelecting([dest.lastPathComponent])
            }
        }
        if let quick = ArchiveKinds.format(AppSettings.quickCompress) {
            b.add(String(localized: "Compress to \(quick.title)"), "archive") { compressOne(quick.format, archiveExtension(format: quick.format)) }
        }
        b.sub(AppSettings.quickCompress == "menu" ? String(localized: "Compress") : String(localized: "Other formats"), "file-archive") { m in
            m.add(String(localized: "Create archive…"), "archive") { [weak self] in self?.createArchive(urls) }
            m.separator()
            for kind in ArchiveKinds.offered {
                m.add(String(localized: "Compress to \(kind.title)"), "file-archive") { compressOne(kind.format, archiveExtension(format: kind.format)) }
            }
        }
        if allArchives { b.sub(String(localized: "Extract"), "folder-output") { self.extractMenu(urls, $0) } }
        if urls.count == 1, urls[0].pathExtension.lowercased() == "torrent", HyperDownload.isInstalled, let here = url {
            b.add(String(localized: "Download with HyperDownload"), "download") { HyperDownload.download(.torrentFile(urls[0]), into: here) }
        }
        b.sub(String(localized: "Tags"), "tag") { MenuActions.tagsMenu(urls, $0) }
        b.menu.addItem(FerryMenu.make(for: urls))
        if singleDir, let u = urls.first {
            b.separator()
            b.add(String(localized: "Open in Terminal"), "square-terminal") { MenuActions.openInTerminal(u) }
            b.add(String(localized: "Disk usage"), "chart-pie") { [weak self] in self?.onDiskUsage?(u) }
            if Editors.offered(for: u) { b.add(String(localized: "Open in \(Editors.name)"), "code") { Editors.open([u]) } }
            // Advanced › Show “Flatten folder”
            if AppSettings.flattenOption {
                b.add(String(localized: "Flatten folder"), "layers") { [weak self] in
                    self?.run { try FileOps.flatten(u) } done: { undo in
                        FileUndo.shared.record(undo, title: String(localized: "Flatten Folder"))
                        self?.onRefresh?()
                    }
                }
            }
        }
        return Self.pruned(b.done())
    }

    /// Extract ▸ for archive files: here, smartly, to a folder of its own, to a place chosen.
    private func extractMenu(_ urls: [URL], _ m: MenuBuilder) {
        let run = { [weak self] (work: @escaping @Sendable () throws -> [String]) in
            self?.run(work) { names in
                self?.reloadSelecting(names)
                // Files › Open the folder after extracting
                guard AppSettings.openAfterExtract, let dir = urls.first?.deletingLastPathComponent() else { return }
                for n in names {
                    let u = dir.appendingPathComponent(n)
                    var isDir: ObjCBool = false
                    if FileManager.default.fileExists(atPath: u.path, isDirectory: &isDir), isDir.boolValue { self?.onOpenInNewTab?(u) }
                }
            }
        }
        m.add(String(localized: "Extract here"), "folder-down") {
            run {
                try urls.map { u in
                    _ = try Archive.open(path: u.path, password: nil).extractAll(dest: u.deletingLastPathComponent().path)
                    return u.lastPathComponent
                }
            }
        }
        m.add(String(localized: "Extract smartly"), "zap") {
            run {
                try urls.map { u in
                    let target = smartExtractTarget(archive: u.path, into: u.deletingLastPathComponent().path)
                    _ = try Archive.open(path: u.path, password: nil).extractAll(dest: target[0])
                    return URL(fileURLWithPath: target[1]).lastPathComponent
                }
            }
        }
        let own = urls.count == 1
            ? String(localized: "Extract to folder: \(urls[0].deletingPathExtension().lastPathComponent)/")
            : String(localized: "Extract each to its own folder")
        m.add(own, "folder-input") {
            run {
                try urls.map { u in
                    let dest = FileOps.unique(u.deletingPathExtension().lastPathComponent, ext: "", in: u.deletingLastPathComponent())
                    _ = try Archive.open(path: u.path, password: nil).extractAll(dest: dest.path)
                    return dest.lastPathComponent
                }
            }
        }
        m.add(String(localized: "Extract to…"), "folder-search") { [weak self] in
            self?.chooseAndExtract { dest in
                self?.run {
                    for u in urls { _ = try Archive.open(path: u.path, password: nil).extractAll(dest: dest.path) }
                } done: { self?.onOpenFolder?(dest) }
            }
        }
    }

    /// Right-click on empty space (`background_menu`): paste, layout, sort, refresh, new, the
    /// folder's own commands, Terminal.
    private func backgroundMenu() -> NSMenu {
        let b = MenuBuilder()
        if TrashBin.contains(url) {
            b.add(String(localized: "Empty Trash…"), "trash", enabled: folder.map { $0.count() > 0 } ?? false) { [weak self] in self?.emptyTrash() }
            b.add(String(localized: "Refresh"), "rotate-cw", key: "r") { [weak self] in self?.onRefresh?() }
            return b.done()
        }
        b.quick([
            ("clipboard-paste", String(localized: "Paste (⌘V)"), Clipboard.hasFiles && folder != nil && !isGathered, { [weak self] in self?.pasteItems() }),
            ("wrench", String(localized: "Properties (⌘I)"), url != nil, { [weak self] in if let u = self?.url { PropertiesWindow.show([u]) } }),
        ])
        b.sub(String(localized: "Layout"), "layout-grid") { m in
            m.add(String(localized: "Details"), "list", key: "1", checked: viewMode == .details) { [weak self] in self?.setViewMode(.details) }
            for (i, mode) in ViewMode.allCases.enumerated() where mode != .details {
                m.add(mode.title, mode.icon, key: "\(i + 1)", checked: viewMode == mode) { [weak self] in self?.setViewMode(mode) }
            }
        }
        b.sub(String(localized: "Sort"), "arrow-up-down") { m in
            for col in ListColumn.allCases.filter(\.sortable) {
                m.add(col.title, checked: col == sortKey) { [weak self] in
                    self?.sort(by: col, ascending: col == self?.sortKey ? self?.ascending : col.startsAscending)
                }
            }
            m.separator()
            m.add(String(localized: "Ascending"), checked: ascending) { [weak self] in
                guard let self else { return }
                self.sort(by: self.sortKey, ascending: true)
            }
            m.add(String(localized: "Descending"), checked: !ascending) { [weak self] in
                guard let self else { return }
                self.sort(by: self.sortKey, ascending: false)
            }
            m.separator()
            m.add(String(localized: "Sort folders first"), checked: foldersFirst) { AppSettings.foldersFirst.toggle() }
        }
        b.add(String(localized: "Refresh"), "rotate-cw", key: "r") { [weak self] in self?.onRefresh?() }
        b.separator()
        let um = FileUndo.shared.manager
        if um.canUndo { b.add(um.undoMenuItemTitle, "undo-2", key: "z") { um.undo() } }
        if um.canRedo { b.add(um.redoMenuItemTitle, "redo-2", key: "z", mods: [.command, .shift]) { um.redo() } }
        if !isGathered {
            b.sub(String(localized: "New"), "circle-plus") { m in
                m.add(String(localized: "Folder"), "folder", key: "n", mods: [.command, .shift]) { [weak self] in self?.newFolder() }
                m.add(String(localized: "Text document"), "file-text") { [weak self] in
                    guard let dir = self?.url else { return }
                    self?.run { try FileOps.newTextDocument(in: dir) } done: { made in
                        FileUndo.shared.record(.trash([made]), title: String(localized: "New Text Document"))
                        self?.reloadSelecting([made.lastPathComponent], rename: true)
                    }
                }
            }
        }
        if let dir = url, dir.isFileURL {
            let pinned = MenuActions.isPinned(dir)
            b.add(pinned ? String(localized: "Unpin from sidebar") : String(localized: "Pin to sidebar"), pinned ? "pin-off" : "pin") {
                MenuActions.setPinned(dir, !pinned)
            }
            b.add(String(localized: "Copy path"), "clipboard-copy") { MenuActions.copyPaths([dir]) }
            b.add(String(localized: "Disk usage"), "chart-pie") { [weak self] in self?.onDiskUsage?(dir) }
            // HyperDownload, once installed (Settings › Add-ons)
            if archive == nil, HyperDownload.isInstalled {
                b.add(String(localized: "Download from a link…"), "download") { [weak self] in HyperDownload.askLink(into: dir, window: self?.card.window) }
            }
        }
        if let dir = url, dir.isFileURL {
            b.separator()
            b.add(String(localized: "Open in Terminal"), "square-terminal") { MenuActions.openInTerminal(dir) }
            // Developer tools › “Open in editor”
            if Editors.offered(for: dir) { b.add(String(localized: "Open in \(Editors.name)"), "code") { Editors.open([dir]) } }
        }
        return b.done()
    }

    /// Inside an archive: open, copy and extract (it is read only here).
    private func archiveMenu(_ archive: ArchiveView) -> NSMenu {
        let b = MenuBuilder()
        let any = !selectedRows.isEmpty
        if any {
            b.quick([("copy", String(localized: "Copy (⌘C)"), true, { [weak self] in self?.copyItems() })])
            b.add(String(localized: "Open"), "square-arrow-out-up-right", key: "\r", mods: []) { [weak self] in self?.openSelected() }
            b.add(String(localized: "Quick Look"), "eye", key: " ", mods: []) { [weak self] in self?.toggleQuickLook() }
            b.separator()
            b.add(String(localized: "Extract to…"), "folder-search") { [weak self] in
                self?.chooseAndExtract { self?.extractArchive(archive, to: $0, onlySelected: true) }
            }
            if archiveEditable {
                b.separator()
                b.add(String(localized: "Rename"), "text-cursor-input", key: "\u{F705}", mods: []) { [weak self] in self?.beginRename() }
                b.add(String(localized: "Delete from archive"), "trash") { [weak self] in self?.deleteFromArchive() }
            }
        } else {
            b.add(String(localized: "Extract all to…"), "folder-search") { [weak self] in
                self?.chooseAndExtract { self?.extractArchive(archive, to: $0, onlySelected: false) }
            }
            if archiveEditable {
                b.add(String(localized: "Add files…"), "file-plus") { [weak self] in
                    let panel = NSOpenPanel()
                    panel.canChooseDirectories = true
                    panel.allowsMultipleSelection = true
                    if panel.runModal() == .OK { self?.addToArchive(panel.urls) }
                }
                b.add(String(localized: "Paste (⌘V)"), "clipboard-paste", enabled: Clipboard.hasFiles) { [weak self] in self?.pasteItems() }
            }
            b.add(String(localized: "Refresh"), "rotate-cw", key: "r") { [weak self] in self?.onRefresh?() }
        }
        return b.done()
    }

    func shareSelected() {
        let picker = NSSharingServicePicker(items: selectedURLs)
        let view = viewMode == .details ? table as NSView : tiles
        let row = selectedRows.first ?? 0
        let rect = viewMode == .details ? table.rect(ofRow: row) : (tiles.item(at: gridPath(row))?.view.frame ?? .zero)
        picker.show(relativeTo: rect, of: view, preferredEdge: .minY)
    }

    // MARK: table data

    func numberOfRows(in tableView: NSTableView) -> Int { Int(folder?.count() ?? 0) }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        let v = FileRowView()
        v.geometry = geometry
        v.groupTitle = groupTitles[row]
        v.groupCount = groupTitles[row].flatMap { groupCounts[$0] }
        v.groupFolded = foldedRows.contains(row)
        v.detailTip = AppSettings.hoverDetails && !foldedRows.contains(row) ? { [weak self] in self?.hoverDetails(row: row) } : nil
        v.onLens = AppSettings.isGlass && tableView.selectedRowIndexes.count == 1 && tableView.selectedRowIndexes.first == row
        return v
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let folder, let tableColumn, let col = ListColumn(rawValue: tableColumn.identifier.rawValue),
            let item = folder.item(row: UInt32(row))
        else { return nil }
        if foldedRows.contains(row) { return nil }
        let isLast = tableColumn == tableView.tableColumns.last
        if col == .name {
            let cell = (tableView.makeView(withIdentifier: .init("name"), owner: nil) as? NameCellView) ?? {
                let c = NameCellView()
                c.identifier = .init("name")
                return c
            }()
            cell.lead = ListGeometry.lead + 8
            cell.topInset = groupTitles[row] == nil ? 0 : GroupMetrics.header
            cell.showsBox = AppSettings.selectionBoxes
            cell.checked = tableView.selectedRowIndexes.contains(row)
            cell.textField?.stringValue = display.name(item)
            cell.textField?.textColor = Palette.of(tableView).text
            cell.imageView?.image = display.icon(item, path: folder.childPath(row: UInt32(row)) ?? "")
            cell.alphaValue = faded(item, row: row) ? 0.5 : 1
            return cell
        }
        let id = NSUserInterfaceItemIdentifier(col.rawValue)
        let cell = (tableView.makeView(withIdentifier: id, owner: nil) as? TextCellView) ?? {
            let c = TextCellView()
            c.identifier = id
            return c
        }()
        cell.trailPad = 8 + (isLast ? geometry.trail : 0)
        cell.topInset = groupTitles[row] == nil ? 0 : GroupMetrics.header
        cell.alignRight = col == .size
        cell.isLink = false
        cell.onClick = nil
        cell.alphaValue = faded(item, row: row) ? 0.5 : 1
        switch col {
        case .modified: cell.text = display.date(item.modifiedMs)
        case .created: cell.text = item.createdMs > 0 ? display.date(item.createdMs) : ""
        case .tags: cell.text = tagsByName[item.name] ?? ""
        case .git: cell.text = gitByName[item.name] ?? ""
        case .dimensions, .duration, .artist, .album, .title, .camera: cell.text = propsByName[item.name]?[col] ?? ""
        case .folder:
            let dir = folder.childPath(row: UInt32(row)).map { ($0 as NSString).deletingLastPathComponent } ?? ""
            cell.text = (dir as NSString).abbreviatingWithTildeInPath
        case .type: cell.text = display.kind(item)
        case .size:
            if item.isDir && archive != nil {
                cell.text = ""
            } else if item.isDir && !display.isPackage(item), let path = folder.childPath(row: UInt32(row)) {
                if let size = Self.rememberedSize(Self.sizeKey(path), path: path) {
                    cell.text = formatSize(bytes: size)
                } else if Self.sizing.contains(Self.sizeKey(path)) {
                    cell.text = String(localized: "Calculating…")
                } else if AppSettings.autoFolderSizes {
                    // Files › Calculate folder sizes: asked for as soon as the row shows
                    cell.text = String(localized: "Calculating…")
                    DispatchQueue.main.async { [weak self] in self?.calculateSize(path) }
                } else {
                    cell.text = String(localized: "Calculate size")
                    cell.isLink = true
                    cell.onClick = { [weak self] in self?.calculateSize(path) }
                }
            } else {
                cell.text = item.isDir ? "" : display.size(item)
            }
        case .name: break
        }
        return cell
    }

    /// Sizes on disk and in bytes are kept apart.
    private static func sizeKey(_ path: String) -> String { (AppSettings.folderSizeOnDisk ? "disk:" : "") + path }

    /// Two folders are measured at a time, so a long list doesn't swamp the disk.
    private static let sizeQueue: OperationQueue = {
        let q = OperationQueue()
        q.maxConcurrentOperationCount = 2
        q.qualityOfService = .utility
        return q
    }()

    private func calculateSize(_ path: String) {
        let key = Self.sizeKey(path)
        guard !Self.sizing.contains(key), Self.rememberedSize(key, path: path) == nil else { return }
        Self.sizing.insert(key)
        reloadSize(of: path)
        let onDisk = AppSettings.folderSizeOnDisk
        Self.sizeQueue.addOperation {
            let size = onDisk ? Self.sizeOnDisk(path) : folderSize(path: path)
            DispatchQueue.main.async {
                MainActor.assumeIsolated { [weak self] in
                    Self.sizing.remove(key)
                    Self.remember(key, path: path, size: size)
                    self?.reloadSize(of: path)
                }
            }
        }
    }

    /// What a folder takes on the disk (allocated blocks, clones and sparse files as they are).
    nonisolated private static func sizeOnDisk(_ path: String) -> UInt64 {
        let keys: [URLResourceKey] = [.totalFileAllocatedSizeKey, .isDirectoryKey]
        guard let walker = FileManager.default.enumerator(at: URL(fileURLWithPath: path), includingPropertiesForKeys: keys, options: [], errorHandler: { _, _ in true })
        else { return 0 }
        var total: UInt64 = 0
        for case let u as URL in walker {
            total += UInt64((try? u.resourceValues(forKeys: Set(keys)).totalFileAllocatedSize) ?? 0)
        }
        return total
    }

    private func reloadSize(of path: String) {
        guard let folder, let url else { return }
        let name = URL(fileURLWithPath: path).lastPathComponent
        guard URL(fileURLWithPath: path).deletingLastPathComponent().standardizedFileURL == url.standardizedFileURL,
            let row = folder.rowOf(name: name)
        else { return }
        let col = table.column(withIdentifier: .init(ListColumn.size.rawValue))
        if viewMode == .details, col >= 0 {
            table.reloadData(forRowIndexes: [Int(row)], columnIndexes: [col])
        }
    }

    func tableViewSelectionDidChange(_ notification: Notification) { selectionChanged() }

    func tableView(_ tableView: NSTableView, typeSelectStringFor tableColumn: NSTableColumn?, row: Int) -> String? {
        guard tableColumn?.identifier.rawValue == ListColumn.name.rawValue else { return nil }
        return folder?.item(row: UInt32(row))?.name
    }

    func tableView(_ tableView: NSTableView, pasteboardWriterForRow row: Int) -> NSPasteboardWriting? {
        guard archive == nil else { return nil }
        return folder?.childPath(row: UInt32(row)).map { URL(fileURLWithPath: $0) as NSURL }
    }

    // MARK: Quick Look

    private var previewURLs: [URL] = []

    func toggleQuickLook() {
        guard let panel = QLPreviewPanel.shared() else { return }
        if QLPreviewPanel.sharedPreviewPanelExists() && panel.isVisible {
            panel.orderOut(nil)
        } else {
            panel.makeKeyAndOrderFront(nil)
        }
    }

    private func refreshQuickLook() {
        if let archive {
            previewURLs = []
            guard QLPreviewPanel.sharedPreviewPanelExists(), let panel = QLPreviewPanel.shared(), panel.isVisible, panel.dataSource === self
            else { return }
            let wanted = selectedInner(archive)
            extractSelected(from: archive) { [weak self] urls in
                guard let self, self.archive.map({ self.selectedInner($0) }) == wanted else { return }
                self.previewURLs = urls
                panel.reloadData()
            }
            return
        }
        previewURLs = selectedURLs
        guard QLPreviewPanel.sharedPreviewPanelExists(), let panel = QLPreviewPanel.shared(), panel.isVisible,
            panel.dataSource === self
        else { return }
        panel.reloadData()
    }

    func beginQuickLook(_ panel: QLPreviewPanel) {
        previewURLs = archive == nil ? selectedURLs : []
        panel.dataSource = self
        panel.delegate = self
        if archive != nil { DispatchQueue.main.async { self.refreshQuickLook() } }
    }

    func endQuickLook(_ panel: QLPreviewPanel) {
        panel.dataSource = nil
        panel.delegate = nil
    }

    func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int { previewURLs.count }

    func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> (any QLPreviewItem)! {
        previewURLs[index] as NSURL
    }

    /// Arrow keys pressed while the panel is in front still move through the list.
    func previewPanel(_ panel: QLPreviewPanel!, handle event: NSEvent!) -> Bool {
        guard event.type == .keyDown else { return false }
        (viewMode == .details ? table as NSView : tiles).keyDown(with: event)
        return true
    }

    // MARK: keys (shared by the table and the grid)

    /// Space toggles Quick Look, Enter opens, F2 renames, Backspace goes back and ⌘⌫ / Delete
    /// moves to the Trash, as in the Windows app (on a Mac keyboard). False: not ours.
    func handleKey(_ event: NSEvent) -> Bool {
        let mods = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let chars = event.charactersIgnoringModifiers ?? ""
        switch (chars, mods) {
        case (" ", []): toggleQuickLook()
        case ("\r", []), ("\u{3}", []): openSelected()
        case (String(Character(UnicodeScalar(NSF2FunctionKey)!)), _): beginRename()
        case ("\u{7f}", []): onBack?()
        // Columns: → goes into the selected folder, ← back out to the parent
        case (String(Character(UnicodeScalar(NSRightArrowFunctionKey)!)), _) where viewMode == .columns && millerNext.dir != nil:
            onOpenFolder?(millerNext.dir!)
        case (String(Character(UnicodeScalar(NSLeftArrowFunctionKey)!)), _) where viewMode == .columns:
            onGoUp?()
        case ("\u{7f}", [.command]), (String(Character(UnicodeScalar(NSDeleteFunctionKey)!)), _): trashSelected()
        default: return false
        }
        return true
    }

    // MARK: grid data

    func numberOfSections(in collectionView: NSCollectionView) -> Int { max(gridStarts.count, 1) }

    func collectionView(_ collectionView: NSCollectionView, numberOfItemsInSection section: Int) -> Int {
        let n = Int(folder?.count() ?? 0)
        guard gridStarts.indices.contains(section) else { return section == 0 ? n : 0 }
        // a folded group shows its header alone
        if gridTitles.indices.contains(section), collapsedGroups.contains(gridTitles[section]) { return 0 }
        let end = section + 1 < gridStarts.count ? gridStarts[section + 1] : n
        return max(end - gridStarts[section], 0)
    }

    /// A group's header in the grid (736ae68): its title in the accent, a line after it.
    func collectionView(_ collectionView: NSCollectionView, viewForSupplementaryElementOfKind kind: NSCollectionView.SupplementaryElementKind, at indexPath: IndexPath) -> NSView {
        let v = collectionView.makeSupplementaryView(ofKind: kind, withIdentifier: GridGroupHeader.id, for: indexPath) as! GridGroupHeader
        let title = gridTitles.indices.contains(indexPath.section) ? gridTitles[indexPath.section] : ""
        v.title = title
        v.count = groupCounts[title]
        v.folded = collapsedGroups.contains(title)
        v.onClick = { [weak self] in self?.toggleGroup(title) }
        return v
    }

    /// The grid's row (the folder's) for an index path, and back: with groups, each group is a
    /// section starting at its first row.
    func gridRow(_ p: IndexPath) -> Int { (gridStarts.indices.contains(p.section) ? gridStarts[p.section] : 0) + p.item }

    func gridPath(_ row: Int) -> IndexPath {
        var lo = 0, hi = gridStarts.count - 1
        while lo < hi {
            let mid = (lo + hi + 1) / 2
            if gridStarts[mid] <= row { lo = mid } else { hi = mid - 1 }
        }
        let start = gridStarts.indices.contains(lo) ? gridStarts[lo] : 0
        return IndexPath(item: row - start, section: max(lo, 0))
    }

    // MARK: List, column by column (folder_view.rs `list_column_widths`)

    /// List without groups fills column by column and scrolls sideways, as on Windows.
    var listByColumns: Bool { viewMode == .list && gridTitles.isEmpty }

    /// How many rows a List column holds in the height there is.
    var listRowsPerColumn: Int {
        let h = tileScroll.contentSize.height - 2 * TileMetrics.padY
        let rowH = ViewMode.list.cellSize.height
        return max(Int((h + TileMetrics.gap) / (rowH + TileMetrics.gap)), 1)
    }

    /// Each column's width, kept until the folder, the names or the height change.
    var listWidths: (rows: Int, count: Int, widths: [CGFloat])?

    private func listColumnWidth(_ column: Int) -> CGFloat {
        let n = Int(folder?.count() ?? 0), rows = listRowsPerColumn
        if listWidths == nil || listWidths?.rows != rows || listWidths?.count != n {
            var widths: [CGFloat] = []
            let font = Metrics.textSm
            if let folder, n <= 4000 {
                var start = 0
                while start < n {
                    var most: CGFloat = 0
                    for r in start..<min(start + rows, n) {
                        if let item = folder.item(row: UInt32(r)) { most = max(most, Draw.textWidth(display.name(item), font: font)) }
                    }
                    // the icon, the gaps and the padding (LIST_CELL_CHROME), within 140…460
                    widths.append(min(max(ceil(most) + 8 + 20 + 8 + 8 + 12, 140), 460))
                    start += rows
                }
            }
            listWidths = (rows, n, widths)
        }
        let w = listWidths?.widths ?? []
        return w.indices.contains(column) ? w[column] : ViewMode.list.cellSize.width
    }

    func collectionView(_ collectionView: NSCollectionView, layout collectionViewLayout: NSCollectionViewLayout, sizeForItemAt indexPath: IndexPath) -> NSSize {
        let size = viewMode.cellSize
        guard listByColumns else { return size }
        return NSSize(width: listColumnWidth(indexPath.item / listRowsPerColumn), height: size.height)
    }

    /// The details box for a row (Settings › Files › Show details on hover).
    func hoverDetails(row: Int) -> String? {
        guard let folder, archive == nil, let item = folder.item(row: UInt32(row)), let p = folder.childPath(row: UInt32(row)) else { return nil }
        return PreviewPane.hoverDetails(URL(fileURLWithPath: p), folder: item.isDir)
    }

    @objc func updateChanged() { status.update = Updater.ready }

    /// Hidden items and items cut but not yet pasted are drawn faded.
    func faded(_ item: FileItem, row: Int) -> Bool {
        item.hidden || (!Clipboard.cutPaths.isEmpty && folder?.childPath(row: UInt32(row)).map(Clipboard.isCut) == true)
    }

    /// A cut was made, pasted or replaced: the rows shown fade or come back.
    @objc func cutChanged() {
        if viewMode.isGrid {
            tiles.reloadItems(at: tiles.indexPathsForVisibleItems())
        } else if table.numberOfRows > 0 {
            let rows = table.rows(in: table.visibleRect)
            table.reloadData(forRowIndexes: IndexSet(integersIn: rows.location..<rows.location + rows.length), columnIndexes: IndexSet(integersIn: 0..<table.numberOfColumns))
        }
        millerLeft.forEach { $0.needsDisplay = true; $0.documentView?.needsDisplay = true }
        millerNext.documentView?.needsDisplay = true
    }

    func collectionView(_ collectionView: NSCollectionView, itemForRepresentedObjectAt indexPath: IndexPath) -> NSCollectionViewItem {
        let item = collectionView.makeItem(withIdentifier: TileItem.id, for: indexPath) as! TileItem
        let row = gridRow(indexPath)
        guard let folder, let entry = folder.item(row: UInt32(row)), let path = folder.childPath(row: UInt32(row)) else {
            return item
        }
        item.path = path
        let tile = item.tile
        tile.style = viewMode
        tile.label.stringValue = display.name(entry)
        if viewMode == .cards {
            var facts = [display.kind(entry)]
            if !entry.isDir { facts.append(display.size(entry)) }
            facts.append(display.date(entry.modifiedMs))
            tile.facts.stringValue = facts.joined(separator: "\n")
            tile.facts.textColor = Palette.of(collectionView).textSecondary
        }
        tile.label.textColor = Palette.of(collectionView).text
        tile.alphaValue = entry.hidden || Clipboard.isCut(path) ? 0.6 : 1
        tile.isSelected = item.isSelected
        tile.detailTip = AppSettings.hoverDetails ? { [weak self] in self?.hoverDetails(row: row) } : nil
        if AppSettings.showThumbnails, let thumb = ThumbnailCache.shared.cached(path) {
            tile.picture.image = thumb
            tile.isThumbnail = true
        } else {
            tile.picture.image = display.icon(entry, path: path)
            tile.isThumbnail = false
            if !entry.isDir && AppSettings.showThumbnails && archive == nil {
                let scale = collectionView.window?.backingScaleFactor ?? 2
                ThumbnailCache.shared.request(path, scale: scale) { [weak item] image in
                    guard let item, item.path == path else { return }
                    item.tile.picture.image = image
                    item.tile.isThumbnail = true
                }
            }
        }
        return item
    }

    func collectionView(_ collectionView: NSCollectionView, didSelectItemsAt indexPaths: Set<IndexPath>) { selectionChanged() }
    func collectionView(_ collectionView: NSCollectionView, didDeselectItemsAt indexPaths: Set<IndexPath>) { selectionChanged() }

    func collectionView(_ collectionView: NSCollectionView, canDragItemsAt indexPaths: Set<IndexPath>, with event: NSEvent) -> Bool { true }

    func collectionView(_ collectionView: NSCollectionView, pasteboardWriterForItemAt indexPath: IndexPath) -> NSPasteboardWriting? {
        guard archive == nil else { return nil }
        return folder?.childPath(row: UInt32(gridRow(indexPath))).map { URL(fileURLWithPath: $0) as NSURL }
    }
}

/// A menu item that runs a closure.
final class ClosureMenuItem: NSMenuItem {
    private let handler: () -> Void

    init(title: String, handler: @escaping () -> Void) {
        self.handler = handler
        super.init(title: title, action: #selector(fire), keyEquivalent: "")
        target = self
    }

    @available(*, unavailable)
    required init(coder: NSCoder) { fatalError() }

    @objc private func fire() { handler() }
}

/// Widths the user dragged columns to (the others are automatic). A double click on a
/// column's edge forgets its width.
@MainActor
enum ColumnWidths {
    private static let key = "columnWidths"

    static func get(_ col: ListColumn) -> CGFloat? {
        (UserDefaults.standard.dictionary(forKey: key)?[col.rawValue] as? Double).map { CGFloat($0) }
    }

    static func set(_ col: ListColumn, _ width: CGFloat?) {
        var d = UserDefaults.standard.dictionary(forKey: key) ?? [:]
        d[col.rawValue] = width.map { Double($0) }
        UserDefaults.standard.set(d, forKey: key)
    }
}

// MARK: - drops

extension FileListController {
    /// The files on a drag's pasteboard.
    static func droppedURLs(_ info: NSDraggingInfo) -> [URL] {
        (info.draggingPasteboard.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]) ?? []
    }

    /// Copy or move, as Finder decides it: ⌥ copies, ⌘ moves, else a move within a volume and a
    /// copy across volumes.
    static func dropKind(_ info: NSDraggingInfo, _ urls: [URL], into dest: URL) -> Transfer.Kind {
        let mods = NSEvent.modifierFlags
        if mods.contains(.option) { return .copy }
        if mods.contains(.command) { return .move }
        return Transfer.defaultKind(urls, into: dest)
    }

    /// The folder a drop lands in: the folder row under the pointer, else the folder shown.
    fileprivate func dropTarget(row: Int?) -> URL? {
        guard archive == nil, let folder, let url else { return nil }
        if isGathered && row == nil { return nil }
        if let row, row >= 0, let item = folder.item(row: UInt32(row)), item.isDir, !display.isPackage(item),
            let p = folder.childPath(row: UInt32(row))
        {
            return URL(fileURLWithPath: p)
        }
        return url
    }

    fileprivate func validate(_ info: NSDraggingInfo, into dest: URL?) -> NSDragOperation {
        // files dropped on an archive that can be edited go into the folder shown in it
        if archive != nil {
            let fromHere = info.draggingSource as AnyObject? === table || info.draggingSource as AnyObject? === tiles
            return archiveEditable && !fromHere && !Self.droppedURLs(info).isEmpty ? .copy : []
        }
        guard let dest else { return [] }
        let urls = Self.droppedURLs(info)
        // nothing into itself, nothing back where it already is
        let d = dest.standardizedFileURL.path
        guard !urls.isEmpty, !urls.contains(where: { let s = $0.standardizedFileURL.path; return d == s || d.hasPrefix(s + "/") }) else { return [] }
        if urls.allSatisfy({ $0.deletingLastPathComponent().standardizedFileURL == dest.standardizedFileURL }) && Self.dropKind(info, urls, into: dest) == .move {
            return []
        }
        return Self.dropKind(info, urls, into: dest) == .copy ? .copy : .move
    }

    fileprivate func accept(_ info: NSDraggingInfo, into dest: URL?) -> Bool {
        if archive != nil {
            let urls = Self.droppedURLs(info).filter { !$0.path.hasPrefix(archive?.place.archive.path ?? "\u{0}") }
            guard archiveEditable, !urls.isEmpty, info.draggingSource as AnyObject? !== table, info.draggingSource as AnyObject? !== tiles else { return false }
            addToArchive(urls)
            return true
        }
        guard let dest, validate(info, into: dest) != [] else { return false }
        let urls = Self.droppedURLs(info)
        transfer(urls, into: dest, kind: Self.dropKind(info, urls, into: dest), selectHere: dest == url)
        return true
    }

    func tableView(_ tableView: NSTableView, validateDrop info: NSDraggingInfo, proposedRow row: Int, proposedDropOperation op: NSTableView.DropOperation) -> NSDragOperation {
        let onFolder = op == .on && dropTarget(row: row) != url
        if !onFolder { tableView.setDropRow(-1, dropOperation: .on) }
        return validate(info, into: dropTarget(row: onFolder ? row : nil))
    }

    func tableView(_ tableView: NSTableView, acceptDrop info: NSDraggingInfo, row: Int, dropOperation op: NSTableView.DropOperation) -> Bool {
        accept(info, into: dropTarget(row: row >= 0 && op == .on ? row : nil))
    }

    func collectionView(_ collectionView: NSCollectionView, validateDrop info: NSDraggingInfo, proposedIndexPath path: AutoreleasingUnsafeMutablePointer<NSIndexPath>, dropOperation op: UnsafeMutablePointer<NSCollectionView.DropOperation>) -> NSDragOperation {
        let row = gridRow(path.pointee as IndexPath)
        let onFolder = op.pointee == .on && dropTarget(row: row) != url
        if !onFolder { op.pointee = .before }
        return validate(info, into: dropTarget(row: onFolder ? row : nil))
    }

    func collectionView(_ collectionView: NSCollectionView, acceptDrop info: NSDraggingInfo, indexPath: IndexPath, dropOperation op: NSCollectionView.DropOperation) -> Bool {
        accept(info, into: dropTarget(row: op == .on ? gridRow(indexPath) : nil))
    }
}

// MARK: - scroll bars

extension FileListController {
    /// Appearance › Show scroll bars.
    func applyScrollBars() {
        for s in [scroll, tileScroll] {
            s.hasVerticalScroller = AppSettings.showScrollBars
        }
    }
}

// MARK: - layout per folder, grouping, extra columns

extension FileListController {
    /// The table's columns: Name, then those turned on (Settings › Layout › Details columns).
    func rebuildColumns() {
        for c in table.tableColumns { table.removeTableColumn(c) }
        columnsGathered = isGathered
        let cols = [ListColumn.name] + (isGathered ? [.folder] : []) + AppSettings.columns
        for (i, col) in cols.enumerated() {
            let c = NSTableColumn(identifier: .init(col.rawValue))
            c.width = col.defaultWidth + (col == .name ? ListGeometry.lead : 0) + (i == cols.count - 1 ? geometry.trail : 0)
            c.minWidth = col == .name ? 120 + ListGeometry.lead : 56
            c.resizingMask = col == .name ? .autoresizingMask : .userResizingMask
            c.isHidden = col != .name && viewMode == .columns
            table.addTableColumn(c)
        }
        header.needsDisplay = true
    }

    /// Sorts the folder; a grouping sorts by what it groups by (folders stay first only where
    /// that keeps the groups whole).
    func arrange(_ folder: Folder) {
        let column = groupBy.column?.sort ?? sortKey.sort
        let first = [.date, .name].contains(groupBy) ? false : foldersFirst
        let asc = groupBy == .date ? false : (groupBy == .none ? ascending : (groupBy.column == sortKey ? ascending : groupBy.column?.startsAscending ?? true))
        folder.arrange(column: column, ascending: asc, foldersFirst: first, showHidden: showHidden, filter: filter)
        foldGroups(folder)
        // the header marks the column the list is really sorted by
        header.sortKey = groupBy.column ?? sortKey
        header.ascending = asc
    }

    func computeGroups() {
        // a view without groups shows every row again
        if isFolded, !groupsApply, let folder { arrange(folder) }
        groupTitles = [:]
        foldedRows = []
        gridStarts = [0]
        gridTitles = []
        let grid = viewMode.isGrid
        defer {
            // the grid's sections carry headers only when there are groups
            let size = NSSize(width: 0, height: gridTitles.isEmpty ? 0 : GroupMetrics.header)
            if tileLayout.headerReferenceSize != size { tileLayout.headerReferenceSize = size }
        }
        guard groupBy != .none, viewMode == .details || grid, let folder else { return }
        var last: String?
        for r in 0..<folder.count() {
            guard let item = folder.item(row: r) else { continue }
            let t = Grouping.title(item, by: groupBy, display: display)
            if t != last {
                groupTitles[Int(r)] = t
                if collapsedGroups.contains(t) { foldedRows.insert(Int(r)) }
            }
            last = t
        }
        if grid {
            let starts = groupTitles.keys.sorted()
            if !starts.isEmpty {
                gridStarts = starts
                gridTitles = starts.map { groupTitles[$0] ?? "" }
            }
            groupTitles = [:]
        }
    }

    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
        if foldedRows.contains(row) { return GroupMetrics.header }
        return Metrics.rowHeight + (groupTitles[row] == nil ? 0 : GroupMetrics.header)
    }

    /// A folded group's row is its header: it cannot be selected.
    func tableView(_ tableView: NSTableView, selectionIndexesForProposedSelection proposed: IndexSet) -> IndexSet {
        foldedRows.isEmpty ? proposed : proposed.subtracting(IndexSet(foldedRows))
    }

    // MARK: folding groups

    /// Whether the folder is shown in groups (the "all groups" button and key apply).
    var groupsApply: Bool { groupBy != .none && (viewMode == .details || viewMode.isGrid) }

    var allGroupsFolded: Bool { !groupCounts.isEmpty && groupCounts.keys.allSatisfy(collapsedGroups.contains) }

    /// Folds a group, or opens it again.
    func toggleGroup(_ title: String) {
        if collapsedGroups.contains(title) { collapsedGroups.remove(title) } else { collapsedGroups.insert(title) }
        rearrange()
        updateCommands()
    }

    /// Folds every group, or opens them all when all are folded (⌃⇧G, as Ctrl+Shift+G on Windows).
    func toggleAllGroups() {
        guard groupsApply, !groupCounts.isEmpty else { return }
        if allGroupsFolded { collapsedGroups.subtract(groupCounts.keys) } else { collapsedGroups.formUnion(groupCounts.keys) }
        rearrange()
        updateCommands()
    }

    /// Counts the groups and takes the items of folded ones out of the rows shown, all but each
    /// group's first (which carries the header). Runs right after `Folder.arrange`.
    private func foldGroups(_ folder: Folder) {
        groupCounts = [:]
        isFolded = false
        guard groupsApply else { return }
        var drop: [UInt32] = []
        var last: String?
        for r in 0..<folder.count() {
            guard let item = folder.item(row: r) else { continue }
            let t = Grouping.title(item, by: groupBy, display: display)
            groupCounts[t, default: 0] += 1
            if t == last, collapsedGroups.contains(t) { drop.append(r) }
            last = t
        }
        if !drop.isEmpty {
            folder.fold(rows: drop)
            isFolded = true
        }
    }

    /// Tags and Git states for the folder, read off the main thread when their columns are on.
    func loadColumnData(_ dir: URL) {
        tagsByName = [:]
        gitByName = [:]
        propsByName = [:]
        guard archive == nil else { return }
        let wantTags = AppSettings.showsColumn(.tags), wantGit = AppSettings.showsColumn(.git)
        let props = ListColumn.optional.filter { $0.isProperty && AppSettings.showsColumn($0) }
        guard wantTags || wantGit || !props.isEmpty else { return }
        DispatchQueue.global(qos: .utility).async {
            var tags: [String: String] = [:]
            if wantTags {
                let names = (try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []
                for n in names {
                    if let t = try? dir.appendingPathComponent(n).resourceValues(forKeys: [.tagNamesKey]).tagNames, !t.isEmpty {
                        tags[n] = t.joined(separator: ", ")
                    }
                }
            }
            let git = wantGit ? (GitStatus.read(dir) ?? [:]) : [:]
            let found = props.isEmpty ? [:] : SpotlightProps.read(dir, props)
            DispatchQueue.main.async {
                MainActor.assumeIsolated { [weak self] in
                    guard let self, self.url?.standardizedFileURL == dir.standardizedFileURL else { return }
                    self.tagsByName = tags
                    self.gitByName = git
                    self.propsByName = found
                    if self.viewMode == .details { self.table.reloadData() }
                }
            }
        }
    }

    /// Before a folder opens: its own layout (each folder keeps its own), else the defaults.
    func prepareLayout(for url: URL) {
        guard !AppSettings.syncLayout else { return }
        let l = FolderLayouts.get(url)
        let mode = l?.view ?? AppSettings.defaultView
        if mode != viewMode {
            viewMode = mode
            applyViewModeQuietly()
        }
        sortKey = l?.sort ?? AppSettings.sortKey
        ascending = l?.ascending ?? !AppSettings.descending
        groupBy = l?.group ?? AppSettings.groupBy
    }

    /// Layout › Adaptive: a folder of mostly pictures and videos shows as tiles (unless it
    /// remembers a layout of its own).
    func adaptLayout(for url: URL) {
        guard AppSettings.adaptiveLayout, archive == nil, let folder,
            AppSettings.syncLayout || FolderLayouts.get(url) == nil
        else { return }
        let n = min(folder.count(), 300)
        guard n >= 4 else { return }
        let media: Set<String> = ["jpg", "jpeg", "png", "heic", "heif", "gif", "webp", "tif", "tiff", "bmp", "raw", "cr2", "nef", "arw", "dng",
                                  "mp4", "mov", "m4v", "avi", "mkv", "webm"]
        var count = 0
        for r in 0..<n where media.contains(folder.item(row: r)?.ext ?? "") { count += 1 }
        let want: ViewMode = Double(count) / Double(n) >= 0.6 ? .tiles : .details
        // a folder the user put in another grid style keeps it
        if want == .tiles && viewMode.isGrid { return }
        if want != viewMode {
            viewMode = want
            applyViewModeQuietly()
            reloadItems()
        }
    }

    /// Each folder keeps its own: the current layout is written for it.
    func rememberLayout() {
        guard !AppSettings.syncLayout, let url, archive == nil else { return }
        FolderLayouts.set(url, .init(view: viewMode, sort: sortKey, ascending: ascending, group: groupBy))
    }
}

// MARK: - progress in the status bar

extension FileListController {
    @objc func statusCenterChanged() {
        let running = StatusCenter.shared.cards.filter { $0.state == .running }
        ProgressCard.shared.update(running)
        guard AppSettings.progressPlace == "status", let card = running.first else {
            status.progress = ""
            status.progressFraction = nil
            return
        }
        var text = card.title
        if card.total > 0 { text += String(format: " — %d%%", Int(Double(card.done) / Double(card.total) * 100)) }
        if card.speed > 0 { text += " · " + formatSize(bytes: UInt64(card.speed)) + "/s" }
        if running.count > 1 { text += " (+\(running.count - 1))" }
        status.progress = text
        status.progressFraction = card.total > 0 ? Double(card.done) / Double(card.total) : nil
    }


}

/// Advanced › Show progress › On a floating card: a small card at the bottom right of the
/// front window while something is being copied.
@MainActor
final class ProgressCard {
    static let shared = ProgressCard()
    private var panel: NSPanel?
    private let label = NSTextField(labelWithString: "")
    private let bar = NSProgressIndicator()

    func update(_ running: [StatusCenter.Card]) {
        guard AppSettings.progressPlace == "card", let card = running.first, let window = NSApp.keyWindow ?? NSApp.mainWindow else {
            panel?.orderOut(nil)
            return
        }
        let p = panel ?? make()
        var text = card.title
        if card.speed > 0 { text += "\n" + formatSize(bytes: UInt64(card.speed)) + "/s" }
        if running.count > 1 { text += "  (+\(running.count - 1))" }
        label.stringValue = text
        bar.isIndeterminate = card.total == 0
        bar.doubleValue = card.total > 0 ? Double(card.done) / Double(card.total) * 100 : 0
        let f = window.frame
        p.setFrameOrigin(NSPoint(x: f.maxX - p.frame.width - 20, y: f.minY + 20))
        if !p.isVisible { p.orderFront(nil) }
        if p.parent == nil { window.addChildWindow(p, ordered: .above) }
    }

    private func make() -> NSPanel {
        let p = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 300, height: 74), styleMask: [.nonactivatingPanel, .borderless], backing: .buffered, defer: false)
        p.isFloatingPanel = true
        p.backgroundColor = .clear
        p.hasShadow = true
        let glass = NSGlassEffectView(frame: p.contentView!.bounds)
        glass.cornerRadius = 14
        glass.autoresizingMask = [.width, .height]
        let content = NSView(frame: glass.bounds)
        label.frame = NSRect(x: 14, y: 26, width: 272, height: 38)
        label.font = Metrics.textXs
        label.maximumNumberOfLines = 2
        label.lineBreakMode = .byTruncatingMiddle
        bar.frame = NSRect(x: 14, y: 12, width: 272, height: 8)
        bar.style = .bar
        bar.minValue = 0
        bar.maxValue = 100
        content.addSubview(label)
        content.addSubview(bar)
        glass.contentView = content
        p.contentView = glass
        panel = p
        return p
    }
}

/// The property columns' values (Windows' "More columns…"), read from Spotlight's metadata.
enum SpotlightProps {
    /// For each item in `dir` that has any of `columns`: its texts. Blocks: off the main thread.
    nonisolated static func read(_ dir: URL, _ columns: [ListColumn]) -> [String: [ListColumn: String]] {
        let names = (try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []
        var out: [String: [ListColumn: String]] = [:]
        // a folder of many thousands: only the first 5000 are read, so it never holds up
        for n in names.prefix(5000) {
            let path = dir.appendingPathComponent(n).path
            guard let md = MDItemCreate(nil, path as CFString) else { continue }
            func value(_ key: CFString) -> Any? { MDItemCopyAttribute(md, key) }
            var row: [ListColumn: String] = [:]
            for c in columns {
                switch c {
                case .dimensions:
                    if let w = value(kMDItemPixelWidth) as? Int, let h = value(kMDItemPixelHeight) as? Int { row[c] = "\(w) × \(h)" }
                case .duration:
                    if let s = value(kMDItemDurationSeconds) as? Double, s > 0 {
                        let t = Int(s.rounded())
                        row[c] = t >= 3600 ? String(format: "%d:%02d:%02d", t / 3600, t / 60 % 60, t % 60) : String(format: "%d:%02d", t / 60, t % 60)
                    }
                case .artist:
                    if let a = value(kMDItemAuthors) as? [String], !a.isEmpty { row[c] = a.joined(separator: ", ") }
                case .album:
                    if let a = value(kMDItemAlbum) as? String { row[c] = a }
                case .title:
                    if let t = value(kMDItemTitle) as? String { row[c] = t }
                case .camera:
                    if let m = value(kMDItemAcquisitionModel) as? String { row[c] = m }
                default: break
                }
            }
            if !row.isEmpty { out[n] = row }
        }
        return out
    }
}
