import AppKit
import Quartz

/// Details (the table) or the grid's Tiles.
enum ViewMode: String {
    case details, tiles
}

/// The content card and everything in it for one window. The rows live in Rust ([`Folder`]);
/// the table and the grid ask for the ones they draw, so a huge folder costs no more than what
/// is on screen.
@MainActor
final class FileListController: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSTextFieldDelegate,
    NSCollectionViewDataSource, NSCollectionViewDelegate,
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
    /// Each row's group header, for the first row of every group (details only).
    private var groupTitles: [Int: String] = [:]
    /// Tags and Git states of the folder shown, by name (filled when their columns are on).
    private var tagsByName: [String: String] = [:]
    private var gitByName: [String: String] = [:]
    var showHidden = AppSettings.showHidden { didSet { if showHidden != oldValue { rearrange() } } }
    var filter = "" { didSet { if filter != oldValue { rearrange() } } }

    var onOpenFolder: ((URL) -> Void)?
    var onOpenDiskImage: ((URL) -> Void)?
    var onBack: (() -> Void)?
    var onChange: (() -> Void)?
    var onError: ((String) -> Void)?

    /// Sizes worked out with "Calculate size", kept for the session.
    private static var folderSizes: [String: UInt64] = [:]
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

        [commandBar, header, scroll, tileScroll, status, message, previewPane, previewHandle].forEach(card.addSubview)
        previewHandle.currentWidth = { [weak self] in self?.previewWidth ?? PreviewPane.defaultWidth }
        previewHandle.onDrag = { [weak self] w in
            self?.previewWidth = w
            UserDefaults.standard.set(Double(w), forKey: "previewWidth")
            self?.card.needsLayout = true
        }
        applyViewMode()
        card.onLayout = { [weak self] in self?.layoutCard() }
        NotificationCenter.default.addObserver(self, selector: #selector(statusCenterChanged), name: .rfStatusCenterChanged, object: nil)
        applyScrollBars()
        card.applyBackground()
        NotificationCenter.default.addObserver(self, selector: #selector(columnsChanged), name: NSTableView.columnDidResizeNotification, object: table)
        wireCommandBar()
    }

    // MARK: layout

    // MARK: preview pane

    let previewPane = PreviewPane()
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
        commandBar.isHidden = barH == 0 || settingsView != nil || homeView != nil
        status.isHidden = statusH == 0 || settingsView != nil || homeView != nil
        // the pane: at the right of the list (never leaving it under 320 wide), or under it
        let bottom = AppSettings.previewAtBottom
        let ph = min(max(b.height * 0.38, 160), b.height - barH - 160)
        let pw = previewShown ? (bottom ? b.width : min(previewWidth, max(card.bounds.width - 320, 0))) : 0
        let paneShown = previewShown && (bottom ? ph >= 120 : pw >= PreviewPane.minWidth) && settingsView == nil && homeView == nil
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
        header.frame = NSRect(x: 0, y: barH, width: b.width, height: Metrics.headerHeight)
        let top = barH + Metrics.headerHeight
        status.frame = NSRect(x: 0, y: b.height - statusH, width: b.width, height: Metrics.statusBarHeight)
        scroll.frame = NSRect(x: 0, y: top, width: b.width, height: b.height - statusH - top)
        tileScroll.frame = NSRect(x: 0, y: barH, width: b.width, height: b.height - statusH - barH)
        // as many columns as fit, left-aligned with 4 between them (not spread out)
        let cols = max(Int((b.width - 2 * TileMetrics.padX + TileMetrics.gap) / (TileMetrics.width + TileMetrics.gap)), 1)
        let used = CGFloat(cols) * (TileMetrics.width + TileMetrics.gap) - TileMetrics.gap
        let right = max(b.width - TileMetrics.padX - used, TileMetrics.padX)
        if tileLayout.sectionInset.right != right {
            tileLayout.sectionInset.right = right
            tileLayout.invalidateLayout()
        }
        message.frame = NSRect(x: 40, y: top + 40, width: b.width - 80, height: 80)
        settingsView?.frame = card.bounds
        homeView?.frame = card.bounds

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
            guard let col = ListColumn(rawValue: c.identifier.rawValue), col != .name else { continue }
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
        let isTiles = viewMode == .tiles
        header.isHidden = isTiles
        scroll.isHidden = isTiles
        tileScroll.isHidden = !isTiles
    }

    /// The selected rows of whichever view is showing.
    var selectedRows: IndexSet {
        switch viewMode {
        case .details: table.selectedRowIndexes
        case .tiles: IndexSet(tiles.selectionIndexPaths.map(\.item))
        }
    }

    private func selectRows(_ rows: IndexSet, scrollToFirst: Bool) {
        switch viewMode {
        case .details:
            table.selectRowIndexes(rows, byExtendingSelection: false)
            if scrollToFirst, let first = rows.first { table.scrollRowToVisible(first) }
        case .tiles:
            let paths = Set(rows.map { IndexPath(item: $0, section: 0) })
            tiles.selectionIndexPaths = paths
            if scrollToFirst, let first = rows.first {
                tiles.scrollToItems(at: [IndexPath(item: first, section: 0)], scrollPosition: .nearestHorizontalEdge)
            }
        }
    }

    private func reloadItems() {
        computeGroups()
        switch viewMode {
        case .details: table.reloadData()
        case .tiles: tiles.reloadData()
        }
    }

    /// A newly opened folder starts at its first item.
    private func scrollToTop() {
        switch viewMode {
        case .details:
            scroll.contentView.scroll(to: NSPoint(x: 0, y: -scroll.contentInsets.top))
            scroll.reflectScrolledClipView(scroll.contentView)
        case .tiles:
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

    private func hideSettings() {
        let pages = [settingsView as NSView?, homeView as NSView?].compactMap { $0 }.filter { !$0.isHidden }
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
        case "showCommandBar", "showStatusBar", "previewPosition", "toolbarHidden", "toolbarAdded":
            commandBar.needsLayout = true
            card.needsLayout = true
        case "listBackground", "listImage", "listImageVisibility":
            card.applyBackground()
        case "fontFamily":
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

    func focus() { table.window?.makeFirstResponder(viewMode == .details ? table : tiles) }

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
        let n = Int(folder.count())
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
        let writable = folder != nil && archive == nil
        let here = writable && !isGathered
        commandBar.cut.isEnabled = any && writable
        commandBar.copy.isEnabled = any
        commandBar.rename.isEnabled = selectedRows.count == 1 && writable
        commandBar.share.isEnabled = any && writable
        commandBar.delete.isEnabled = any && writable
        commandBar.paste.isEnabled = here && Clipboard.hasFiles
        let inTrash = TrashBin.contains(url) && archive == nil
        commandBar.trashMode = inTrash
        commandBar.new.isEnabled = inTrash ? (folder?.count() ?? 0) > 0 : here
        commandBar.share.isEnabled = any && writable && !inTrash
        commandBar.properties.isEnabled = writable
        commandBar.preview.isEnabled = settingsView == nil && homeView == nil
        commandBar.preview.isOn = previewShown
        commandBar.view.icon = viewMode == .details ? "list" : "layout-grid"
    }

    // MARK: command bar

    private func wireCommandBar() {
        let b = commandBar
        b.newFolder.action = { [weak self] _ in self?.newFolder() }
        b.terminal.action = { [weak self] _ in if let u = self?.url { MenuActions.openInTerminal(u) } }
        b.hiddenItems.action = { _ in AppSettings.showHidden.toggle() }
        b.refresh.action = { [weak self] _ in self?.onRefresh?() }
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
            for col in ListColumn.allCases {
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
            let m = NSMenu()
            for (mode, title, icon) in [(ViewMode.details, String(localized: "Details"), "list"), (.tiles, String(localized: "Tiles"), "layout-grid")] {
                let i = ClosureMenuItem(title: title) { [weak self] in self?.setViewMode(mode) }
                i.image = Icons.menu(icon)
                i.state = self.viewMode == mode ? .on : .off
                m.addItem(i)
            }
            btn.pop(m)
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
        guard let dest = target ?? url, let (urls, move) = Clipboard.take() else { return }
        transfer(urls, into: dest, kind: move ? .move : .copy, selectHere: target == nil)
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

    // MARK: rename

    private var renaming: (row: Int, original: String)?

    /// The name field of row `row` in the view that is showing.
    private func nameField(_ row: Int, make: Bool) -> NSTextField? {
        switch viewMode {
        case .details:
            return (table.view(atColumn: 0, row: row, makeIfNecessary: make) as? NameCellView)?.textField
        case .tiles:
            let path = IndexPath(item: row, section: 0)
            if make { tiles.scrollToItems(at: [path], scrollPosition: .nearestHorizontalEdge) }
            return (tiles.item(at: path) as? TileItem)?.tile.label
        }
    }

    func beginRename() {
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
                // archives open like folders, as on Windows
                onOpenFolder?(u)
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
    /// A gathered listing (a tag's files): its items are real, but there is no folder to put
    /// new things in.
    var isGathered: Bool { url.map { !$0.isFileURL } ?? false }
    var onOpenInNewTab: ((URL) -> Void)?
    var onOpenInNewWindow: ((URL) -> Void)?
    var onRefresh: (() -> Void)?

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
            ("text-cursor-input", String(localized: "Rename (F2)"), items.count == 1, { [weak self] in self?.beginRename() }),
            ("share-2", String(localized: "Share"), true, { [weak self] in self?.shareSelected() }),
            ("trash", String(localized: "Move to Trash (⌘⌫)"), true, { [weak self] in self?.trashSelected() }),
            ("wrench", String(localized: "Properties (⌘I)"), true, { PropertiesWindow.show(urls) }),
        ])

        b.add(String(localized: "Open"), "square-arrow-out-up-right", key: "\r", mods: []) { [weak self] in self?.openSelected() }
        if singleDir, let u = urls.first {
            b.add(String(localized: "Open in new tab"), "square-plus") { [weak self] in self?.onOpenInNewTab?(u) }
            b.add(String(localized: "Open in new window"), "app-window") { [weak self] in self?.onOpenInNewWindow?(u) }
        }
        if hasFiles { b.sub(String(localized: "Open with"), "layout-grid") { MenuActions.openWithMenu(urls, $0) } }
        b.add(String(localized: "Quick Look"), "eye", key: " ", mods: []) { [weak self] in self?.toggleQuickLook() }
        b.separator()

        b.add(String(localized: "Copy item path"), "clipboard-copy", key: "c", mods: [.command, .option]) { MenuActions.copyPaths(urls) }
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

        b.sub(String(localized: "Compress"), "archive") { m in
            for (title, format, ext) in [("ZIP", ArchiveFormat.zip, "zip"), ("7z", .sevenZ, "7z"), ("RAR", .rar, "rar")] {
                if format == .rar && !canMakeRar() { continue }
                m.add(String(localized: "Compress to \(title)"), "file-archive") { [weak self] in
                    let dest = FileOps.archiveName(for: urls, ext: ext)
                    self?.run { try compress(sources: urls.map(\.path), dest: dest.path, format: format) } done: {
                        FileUndo.shared.record(.trash([dest]), title: String(localized: "Compress"))
                        self?.reloadSelecting([dest.lastPathComponent])
                    }
                }
            }
        }
        if allArchives { b.sub(String(localized: "Extract"), "folder-output") { self.extractMenu(urls, $0) } }
        b.sub(String(localized: "Tags"), "tag") { MenuActions.tagsMenu(urls, $0) }
        b.menu.addItem(FerryMenu.make(for: urls))
        if singleDir, let u = urls.first {
            b.separator()
            b.add(String(localized: "Open in Terminal"), "square-terminal") { MenuActions.openInTerminal(u) }
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
            m.add(String(localized: "Tiles"), "layout-grid", key: "2", checked: viewMode == .tiles) { [weak self] in self?.setViewMode(.tiles) }
        }
        b.sub(String(localized: "Sort"), "arrow-up-down") { m in
            for col in ListColumn.allCases {
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
        } else {
            b.add(String(localized: "Extract all to…"), "folder-search") { [weak self] in
                self?.chooseAndExtract { self?.extractArchive(archive, to: $0, onlySelected: false) }
            }
            b.add(String(localized: "Refresh"), "rotate-cw", key: "r") { [weak self] in self?.onRefresh?() }
        }
        return b.done()
    }

    func shareSelected() {
        let picker = NSSharingServicePicker(items: selectedURLs)
        let view = viewMode == .details ? table as NSView : tiles
        let row = selectedRows.first ?? 0
        let rect = viewMode == .details ? table.rect(ofRow: row) : (tiles.item(at: IndexPath(item: row, section: 0))?.view.frame ?? .zero)
        picker.show(relativeTo: rect, of: view, preferredEdge: .minY)
    }

    // MARK: table data

    func numberOfRows(in tableView: NSTableView) -> Int { Int(folder?.count() ?? 0) }

    func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        let v = FileRowView()
        v.geometry = geometry
        v.groupTitle = groupTitles[row]
        v.onLens = AppSettings.isGlass && tableView.selectedRowIndexes.count == 1 && tableView.selectedRowIndexes.first == row
        return v
    }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        guard let folder, let tableColumn, let col = ListColumn(rawValue: tableColumn.identifier.rawValue),
            let item = folder.item(row: UInt32(row))
        else { return nil }
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
            cell.alphaValue = item.hidden ? 0.5 : 1
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
        cell.alphaValue = item.hidden ? 0.5 : 1
        switch col {
        case .modified: cell.text = display.date(item.modifiedMs)
        case .created: cell.text = item.createdMs > 0 ? display.date(item.createdMs) : ""
        case .tags: cell.text = tagsByName[item.name] ?? ""
        case .git: cell.text = gitByName[item.name] ?? ""
        case .type: cell.text = display.kind(item)
        case .size:
            if item.isDir && archive != nil {
                cell.text = ""
            } else if item.isDir && !display.isPackage(item), let path = folder.childPath(row: UInt32(row)) {
                if let size = Self.folderSizes[Self.sizeKey(path)] {
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
        guard !Self.sizing.contains(key), Self.folderSizes[key] == nil else { return }
        Self.sizing.insert(key)
        reloadSize(of: path)
        let onDisk = AppSettings.folderSizeOnDisk
        Self.sizeQueue.addOperation {
            let size = onDisk ? Self.sizeOnDisk(path) : folderSize(path: path)
            DispatchQueue.main.async {
                MainActor.assumeIsolated { [weak self] in
                    Self.sizing.remove(key)
                    Self.folderSizes[key] = size
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
        case ("\u{7f}", [.command]), (String(Character(UnicodeScalar(NSDeleteFunctionKey)!)), _): trashSelected()
        default: return false
        }
        return true
    }

    // MARK: grid data

    func collectionView(_ collectionView: NSCollectionView, numberOfItemsInSection section: Int) -> Int {
        Int(folder?.count() ?? 0)
    }

    func collectionView(_ collectionView: NSCollectionView, itemForRepresentedObjectAt indexPath: IndexPath) -> NSCollectionViewItem {
        let item = collectionView.makeItem(withIdentifier: TileItem.id, for: indexPath) as! TileItem
        guard let folder, let entry = folder.item(row: UInt32(indexPath.item)), let path = folder.childPath(row: UInt32(indexPath.item)) else {
            return item
        }
        item.path = path
        let tile = item.tile
        tile.label.stringValue = display.name(entry)
        tile.label.textColor = Palette.of(collectionView).text
        tile.alphaValue = entry.hidden ? 0.6 : 1
        tile.isSelected = item.isSelected
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
        return folder?.childPath(row: UInt32(indexPath.item)).map { URL(fileURLWithPath: $0) as NSURL }
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
        let row = path.pointee.item
        let onFolder = op.pointee == .on && dropTarget(row: row) != url
        if !onFolder { op.pointee = .before }
        return validate(info, into: dropTarget(row: onFolder ? row : nil))
    }

    func collectionView(_ collectionView: NSCollectionView, acceptDrop info: NSDraggingInfo, indexPath: IndexPath, dropOperation op: NSCollectionView.DropOperation) -> Bool {
        accept(info, into: dropTarget(row: op == .on ? indexPath.item : nil))
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
        let cols = [ListColumn.name] + AppSettings.columns
        for (i, col) in cols.enumerated() {
            let c = NSTableColumn(identifier: .init(col.rawValue))
            c.width = col.defaultWidth + (col == .name ? ListGeometry.lead : 0) + (i == cols.count - 1 ? geometry.trail : 0)
            c.minWidth = col == .name ? 120 + ListGeometry.lead : 56
            c.resizingMask = col == .name ? .autoresizingMask : .userResizingMask
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
        // the header marks the column the list is really sorted by
        header.sortKey = groupBy.column ?? sortKey
        header.ascending = asc
    }

    func computeGroups() {
        groupTitles = [:]
        guard groupBy != .none, viewMode == .details, let folder else { return }
        var last: String?
        for r in 0..<folder.count() {
            guard let item = folder.item(row: r) else { continue }
            let t = Grouping.title(item, by: groupBy, display: display)
            if t != last { groupTitles[Int(r)] = t }
            last = t
        }
    }

    func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
        Metrics.rowHeight + (groupTitles[row] == nil ? 0 : GroupMetrics.header)
    }

    /// Tags and Git states for the folder, read off the main thread when their columns are on.
    func loadColumnData(_ dir: URL) {
        tagsByName = [:]
        gitByName = [:]
        guard archive == nil else { return }
        let wantTags = AppSettings.showsColumn(.tags), wantGit = AppSettings.showsColumn(.git)
        guard wantTags || wantGit else { return }
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
            DispatchQueue.main.async {
                MainActor.assumeIsolated { [weak self] in
                    guard let self, self.url?.standardizedFileURL == dir.standardizedFileURL else { return }
                    self.tagsByName = tags
                    self.gitByName = git
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
