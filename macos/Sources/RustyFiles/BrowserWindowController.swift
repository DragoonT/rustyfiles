import AppKit

/// One tab: its folder, its own back/forward history, and what to restore when it comes back.
@MainActor
final class BrowserTab {
    var url: URL
    var back: [URL] = []
    var forward: [URL] = []
    var folder: Folder?
    var error: String?
    /// Set while the tab shows a folder inside an archive.
    var archive: ArchiveView?
    var selection: [String] = []
    var scrollY: CGFloat?
    var loadID = 0
    /// A search's walk and what it has found so far.
    var search: SearchJob?
    /// The disk usage walk of a disk usage tab.
    var disk: DiskScan?
    /// Dual pane (workspace.rs `Tab.right`): the right pane's own place and history.
    var right: BrowserTab?
    var searchHits: [String] = []
    var searchDone = false

    init(url: URL) { self.url = url.standardizedFileURL }

    /// The Settings page is a place a tab goes to, like a folder (Location::Settings on Windows).
    static let settingsURL = URL(string: "rustyfiles:settings")!
    /// The Home page (Location::Home on Windows): quick access, drives, recent items.
    static let homeURL = URL(string: "rustyfiles:home")!
    var isSettings: Bool { url == Self.settingsURL }
    var isHome: Bool { url == Self.homeURL }
    /// A page of the app's own rather than a folder.
    var isPage: Bool { isSettings || isHome || diskRoot != nil }
    /// A tag's results (rustyfiles:tag/<name>): the files that have it, from anywhere.
    var tagName: String? { TagStore.name(of: url) }
    /// A Finder smart folder shown here (rustyfiles:smart/<file>).
    var smartFolder: URL? { SmartFolder.file(of: url) }

    /// A search (rustyfiles:search?root=…&q=…, Location::Search on Windows): names matching
    /// `query` in `root` and every folder under it.
    static func searchURL(root: URL, query: String) -> URL {
        var c = URLComponents()
        c.scheme = "rustyfiles"
        c.path = "search"
        c.queryItems = [URLQueryItem(name: "root", value: root.path), URLQueryItem(name: "q", value: query)]
        return c.url!
    }

    /// Disk usage of a folder (rustyfiles:diskusage?root=…, Location::DiskUsage).
    static func diskUsageURL(_ root: URL) -> URL {
        var c = URLComponents()
        c.scheme = "rustyfiles"
        c.path = "diskusage"
        c.queryItems = [URLQueryItem(name: "root", value: root.path)]
        return c.url!
    }

    var diskRoot: URL? {
        guard url.scheme == "rustyfiles", let c = URLComponents(url: url, resolvingAgainstBaseURL: false), c.path == "diskusage",
            let root = c.queryItems?.first(where: { $0.name == "root" })?.value
        else { return nil }
        return URL(fileURLWithPath: root)
    }

    var searchSpec: (root: URL, query: String)? {
        guard url.scheme == "rustyfiles", let c = URLComponents(url: url, resolvingAgainstBaseURL: false), c.path == "search",
            let root = c.queryItems?.first(where: { $0.name == "root" })?.value, let q = c.queryItems?.first(where: { $0.name == "q" })?.value
        else { return nil }
        return (URL(fileURLWithPath: root), q)
    }

    var info: TabInfo {
        if isSettings { return TabInfo(title: String(localized: "Settings"), icon: "settings") }
        if isHome { return TabInfo(title: String(localized: "Home"), icon: "house") }
        if let tag = tagName { return TabInfo(title: tag, icon: "tag") }
        if let s = smartFolder { return TabInfo(title: s.deletingPathExtension().lastPathComponent, icon: "folder-search") }
        if let s = searchSpec { return TabInfo(title: "“\(s.query)”", icon: "search") }
        if let r = diskRoot { return TabInfo(title: String(localized: "Disk usage — \(FileManager.default.displayName(atPath: r.path))"), icon: "chart-pie") }
        let fm = FileManager.default
        let home = fm.homeDirectoryForCurrentUser.standardizedFileURL
        if url == home { return TabInfo(title: fm.displayName(atPath: url.path), icon: "house") }
        if url.lastPathComponent == ".Trash" { return TabInfo(title: String(localized: "Trash"), icon: "trash") }
        let isVolume = (try? url.resourceValues(forKeys: [.isVolumeKey]).isVolume) ?? false
        if isVolume || url.path == "/" {
            let name = (try? url.resourceValues(forKeys: [.volumeLocalizedNameKey]).volumeLocalizedName) ?? url.lastPathComponent
            return TabInfo(title: name, icon: "hard-drive")
        }
        return TabInfo(title: fm.displayName(atPath: url.path), icon: "folder")
    }
}

/// Everything in the window, on the glass: the palette's `window_bg` wash, then the bars,
/// the sidebar and the card.
@MainActor
final class RootView: FlippedView {
    var onLayout: (() -> Void)?

    /// Settings › Appearance › Interface scale: everything in the window drawn this much
    /// bigger (or smaller), as a higher display resolution would otherwise shrink it. The
    /// views lay out in the smaller (or larger) bounds and AppKit scales the drawing, so text
    /// and icons stay sharp.
    var scale: CGFloat = 1 {
        didSet {
            guard scale != oldValue else { return }
            setFrameSize(frame.size)
            needsLayout = true
            needsDisplay = true
        }
    }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        let b = NSSize(width: newSize.width / scale, height: newSize.height / scale)
        if bounds.size != b { setBoundsSize(b) }
    }

    override func layout() {
        super.layout()
        onLayout?()
    }

    override func draw(_ dirtyRect: NSRect) {
        Palette.of(self).windowBg.setFill()
        dirtyRect.fill()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        needsDisplay = true
        subviews.forEach { $0.needsDisplay = true }
    }
}

/// A browser window laid out as rustyfiles on Windows: title bar with tabs, navigation row,
/// sidebar, and the content card.
@MainActor
final class BrowserWindowController: NSWindowController, NSWindowDelegate, NSMenuItemValidation {
    private let root = RootView()
    private var frameView: FlippedView?
    private var backdrop: NSView?
    private let titleBar = TitleBarView()
    private let navBar = NavBarView()
    private let sidebar = SidebarView()
    private let handle = SidebarResizeHandle()
    private let paneDivider = PaneDivider()
    private var lastPaneArea: NSRect = .zero
    /// The left pane's share of a dual pane's width, kept across starts (0.5: half each).
    private var paneShare: CGFloat = {
        let v = UserDefaults.standard.double(forKey: "paneShare")
        return v > 0.1 && v < 0.9 ? v : 0.5
    }()
    /// The left (or only) pane, and the right one of a dual pane.
    let leftList = FileListController()
    let rightList = FileListController()
    private var tabs: [BrowserTab] = []
    private var active = 0
    /// Dual pane: the right pane has the keyboard (and the address bar, Back, Up…).
    private var rightActive = false
    private var dual: Bool { tabs.indices.contains(active) && tabs[active].right != nil }
    var isDualPane: Bool { dual }
    /// The pane that has the keyboard: what the address bar, the menus and the keys act on.
    var list: FileListController { dual && rightActive ? rightList : leftList }
    private var tab: BrowserTab { dual && rightActive ? tabs[active].right! : tabs[active] }

    /// The list showing `t`, when it is shown (a pane of the active tab).
    private func listFor(_ t: BrowserTab) -> FileListController? {
        guard tabs.indices.contains(active) else { return nil }
        if t === tabs[active] { return leftList }
        if let r = tabs[active].right, t === r { return rightList }
        return nil
    }

    /// The panes of the active tab with their lists.
    private var panes: [(BrowserTab, FileListController)] {
        guard tabs.indices.contains(active) else { return [] }
        return [(tabs[active], leftList)] + (tabs[active].right.map { [($0, rightList)] } ?? [])
    }
    private var watcher: DirectoryWatcher?

    private var sidebarWidth: CGFloat = {
        let w = UserDefaults.standard.double(forKey: "sidebarWidth")
        return w > 0 ? w : Metrics.sidebarWidth
    }()
    private var sidebarShown = !UserDefaults.standard.bool(forKey: "sidebarHidden")

    var onClose: ((BrowserWindowController) -> Void)?

    var current: URL { tab.url }

    init(url: URL, selecting name: String? = nil) {
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1280, height: 800),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered, defer: false)
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.tabbingMode = .disallowed
        // An empty compact toolbar makes the title bar 40 px tall with the window buttons
        // centred in it, where the Windows app has its title bar. (Moving AppKit's title bar
        // views by hand instead stops the window from redrawing.)
        window.toolbar = NSToolbar(identifier: "rustyfiles.titlebar")
        window.toolbarStyle = .unifiedCompact
        window.minSize = NSSize(width: 720, height: 420)
        super.init(window: window)

        let frame = FlippedView()
        frameView = frame
        root.frame = frame.bounds
        root.autoresizingMask = [.width, .height]
        frame.addSubview(root)
        window.contentView = frame
        applyMaterial()
        applyScale()
        [sidebar, handle, leftList.card, rightList.card, paneDivider, navBar, titleBar].forEach(root.addSubview)
        paneDivider.onDrag = { [weak self] x in
            guard let self else { return }
            if let x, self.lastPaneArea.width > 0 {
                self.paneShare = min(max((x - self.lastPaneArea.minX - 3) / (self.lastPaneArea.width - 6), 0.15), 0.85)
            } else {
                self.paneShare = 0.5
            }
            UserDefaults.standard.set(Double(self.paneShare), forKey: "paneShare")
            self.root.needsLayout = true
        }
        rightList.card.isHidden = true
        root.onLayout = { [weak self] in self?.layoutRoot() }

        window.center()
        window.setFrameAutosaveName("rustyfiles.window")
        window.delegate = self
        wire()
        NotificationCenter.default.addObserver(self, selector: #selector(settingsChanged(_:)), name: .rfSettingsChanged, object: nil)

        tabs = [BrowserTab(url: url)]
        tab.selection = name.map { [$0] } ?? []
        load(tab)
        refreshTabs()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func showWindow(_ sender: Any?) {
        super.showWindow(sender)
        list.focus()
    }

    // MARK: layout

    private func layoutRoot() {
        let b = root.bounds
        titleBar.frame = NSRect(x: 0, y: 0, width: b.width, height: Metrics.titleBarHeight)
        navBar.frame = NSRect(x: 0, y: Metrics.titleBarHeight, width: b.width, height: Metrics.navRowHeight)
        let top = Metrics.titleBarHeight + Metrics.navRowHeight
        let sw = sidebarShown ? sidebarWidth : 0
        sidebar.isHidden = !sidebarShown
        handle.isHidden = !sidebarShown
        sidebar.frame = NSRect(x: 0, y: top, width: sw, height: b.height - top)
        handle.frame = NSRect(x: sw - 3, y: top, width: 6, height: b.height - top)
        // no sidebar: the card keeps the 8 px margin on the left too
        let left = sidebarShown ? sw : 8
        let area = NSRect(x: left, y: top, width: b.width - left - Metrics.cardRightMargin, height: b.height - top - Metrics.bottomSpacer)
        rightList.card.isHidden = !dual
        paneDivider.isHidden = !dual
        if dual {
            // two cards side by side, 6 apart (workspace.rs dual pane); the gap moves the split
            let w = floor(min(max((area.width - 6) * paneShare, 240), area.width - 6 - 240))
            leftList.card.frame = NSRect(x: area.minX, y: area.minY, width: w, height: area.height)
            rightList.card.frame = NSRect(x: area.minX + w + 6, y: area.minY, width: area.width - w - 6, height: area.height)
            paneDivider.frame = NSRect(x: area.minX + w, y: area.minY, width: 6, height: area.height)
            lastPaneArea = area
        } else {
            leftList.card.frame = area
        }
        leftList.card.isActivePane = dual && !rightActive
        rightList.card.isActivePane = dual && rightActive
    }

    /// The interface scale; a bigger one takes the taller title bar, so the window buttons
    /// stay level with the grown tabs.
    private func applyScale() {
        let s = AppSettings.interfaceScale
        window?.toolbarStyle = s >= 1.2 ? .unified : .unifiedCompact
        root.scale = s
        titleBar.needsLayout = true
    }

    func windowDidBecomeKey(_ notification: Notification) { list.updateCommands() }

    /// Edit › Undo and Redo, ⌘Z and ⇧⌘Z, work on file operations (text being edited keeps its own).
    func windowWillReturnUndoManager(_ window: NSWindow) -> UndoManager? { FileUndo.shared.manager }

    func windowWillClose(_ notification: Notification) { onClose?(self) }

    /// A setting changed: the list takes what concerns it; colours redraw everywhere (and
    /// lenses find their places again, or go, when the material changed).
    @objc private func settingsChanged(_ note: Notification) {
        let key = AppSettings.changedKey(note)
        if key == "material" || key == "theme" { applyMaterial() }
        if key == "interfacePercent" { applyScale() }
        if key == "tabPath" { refreshTabs() }
        if key == "compactWindow" {
            [leftList, rightList].forEach { $0.card.applyCompact(); $0.card.needsLayout = true }
            root.needsLayout = true
            titleBar.needsLayout = true
            titleBar.strip.needsDisplay = true
            navBar.needsLayout = true
        }
        leftList.apply(settingKey: key)
        rightList.apply(settingKey: key)
        func redraw(_ v: NSView) {
            v.needsDisplay = true
            v.needsLayout = true
            v.subviews.forEach(redraw)
        }
        redraw(root)
    }

    /// What the window is made of (Settings › Appearance › Window material).
    ///
    /// - Liquid Glass: one sheet of glass for the whole window, as HyperGlass is one sheet on
    ///   Windows; frosted so text reads over anything behind it, reaching the window's edge,
    ///   where it bends the light. No window shadow: with it macOS draws a black 1 px outline
    ///   round the window, a dark line round the glass.
    /// - Translucent: macOS's translucent window material, the desktop's tone showing.
    /// - Solid: an opaque window, as Finder; the palette paints it.
    private func applyMaterial() {
        guard let window, let frameView else { return }
        backdrop?.removeFromSuperview()
        backdrop = nil
        // Black (OLED) is opaque whatever the material
        let material: AppSettings.Material = AppSettings.theme == .black ? .solid : AppSettings.material
        let view: NSView?
        switch material {
        case .glass:
            let g = NSGlassEffectView()
            g.style = .regular
            g.cornerRadius = Metrics.windowRadius
            view = g
        case .finder:
            let v = NSVisualEffectView()
            v.material = .sidebar
            v.blendingMode = .behindWindow
            v.state = .followsWindowActiveState
            view = v
        case .solid:
            view = nil
        }
        let glass = material == .glass
        window.isOpaque = material == .solid
        window.backgroundColor = material == .solid ? .windowBackgroundColor : .clear
        window.hasShadow = !glass
        if let view {
            view.frame = frameView.bounds
            view.autoresizingMask = [.width, .height]
            frameView.addSubview(view, positioned: .below, relativeTo: nil)
            backdrop = view
        }
        window.invalidateShadow()
    }

    // MARK: wiring

    private func wire() {
        navBar.sidebarToggle.action = { [weak self] _ in self?.toggleSidebar(nil) }
        titleBar.plus.action = { [weak self] _ in self?.newTab(nil) }
        titleBar.strip.onSelect = { [weak self] i in self?.activate(i) }
        titleBar.strip.onClose = { [weak self] i in self?.closeTab(at: i) }

        navBar.back.action = { [weak self] _ in self?.goBack(nil) }
        navBar.forward.action = { [weak self] _ in self?.goForward(nil) }
        navBar.up.action = { [weak self] _ in self?.goUp(nil) }
        navBar.refresh.action = { [weak self] _ in self?.refresh(nil) }
        navBar.statusCenter.action = { [weak self] _ in self?.toggleStatusCenter() }
        NotificationCenter.default.addObserver(self, selector: #selector(statusChanged), name: .rfStatusCenterChanged, object: nil)
        let omni = navBar.omnibar
        omni.breadcrumb.onNavigate = { [weak self] url in self?.navigate(to: url) }
        omni.breadcrumb.onHome = { [weak self] in self?.goHome(nil) }
        omni.breadcrumb.onOpenInNewTab = { [weak self] url in self?.openTab(url) }
        omni.breadcrumb.folderMenu = { [weak self] url in
            guard let self else { return nil }
            return self.sidebar.folderMenu(url) { [weak self] in self?.navigate(to: url) }.done()
        }
        omni.onSubmitPath = { [weak self] text in self?.submitPath(text) }
        omni.onSearch = { [weak self] text in self?.runSearch(text) }
        omni.onFilter = { [weak self] text in
            self?.list.filter = text
            self?.list.commandBar.filter.isOn = !text.isEmpty
        }

        sidebar.onSelect = { [weak self] item in
            guard let self else { return }
            if let url = item.url { self.navigate(to: url) }
        }
        sidebar.onEject = { [weak self] url in self?.eject(url) }
        handle.currentWidth = { [weak self] in self?.sidebarWidth ?? Metrics.sidebarWidth }
        handle.onDrag = { [weak self] w in
            guard let self else { return }
            self.sidebarWidth = w
            UserDefaults.standard.set(Double(w), forKey: "sidebarWidth")
            self.root.needsLayout = true
        }

        wireList(leftList, right: false)
        wireList(rightList, right: true)
        sidebar.onOpenInNewTab = { [weak self] url in self?.openTab(url) }
        sidebar.onOpenInNewWindow = { url in AppDelegate.shared.openWindow(at: url) }
        sidebar.onDiskUsage = { [weak self] url in self?.navigate(to: BrowserTab.diskUsageURL(url)) }
        titleBar.strip.menuFor = { [weak self] i in self?.tabMenu(i) }
        titleBar.strip.onMove = { [weak self] from, to in self?.moveTab(from, to) }
        titleBar.strip.onTearOff = { [weak self] i, at in self?.tearOff(i, at: at) }
        titleBar.strip.dropFolder = { [weak self] i in
            guard let t = self?.tabs[safe: i], !t.isPage, t.url.isFileURL, t.archive == nil else { return nil }
            return t.url
        }
        titleBar.strip.onDropFiles = { [weak self] i, info in
            guard let self, let t = self.tabs[safe: i] else { return false }
            let urls = FileListController.droppedURLs(info)
            self.list.transfer(urls, into: t.url, kind: FileListController.dropKind(info, urls, into: t.url), selectHere: false)
            return true
        }
    }

    /// One pane's list: what it asks for is done in its pane, which first takes the keyboard.
    private func wireList(_ l: FileListController, right: Bool) {
        func pane(_ f: @escaping (BrowserWindowController) -> Void) -> () -> Void {
            { [weak self] in
                guard let self else { return }
                self.focusPane(right: right)
                f(self)
            }
        }
        l.onFocus = { [weak self] in self?.focusPane(right: right) }
        l.onOpenFolder = { url in pane { $0.navigate(to: url) }() }
        l.onOpenInNewTab = { [weak self] url in self?.openTab(url) }
        l.onReveal = { url in pane { $0.navigate(to: url.deletingLastPathComponent(), selecting: url.lastPathComponent) }() }
        l.onShowInFolder = { [weak self] url in self?.openTab(url.deletingLastPathComponent(), activate: true, selecting: url.lastPathComponent) }
        l.onOpenInNewWindow = { url in AppDelegate.shared.openWindow(at: url) }
        l.onRefresh = pane { $0.refresh(nil) }
        l.onGoUp = pane { $0.goUp(nil) }
        l.shelfPane.onTransfer = { [weak self] kind, urls in
            guard let self, let dest = l.url, dest.isFileURL, !urls.isEmpty else { return }
            Transfer(kind, urls, into: dest).start(window: self.window) { _ in
                // moved items have left their places: the shelf is done with them
                if kind == .move { ShelfStore.clear() }
            }
        }
        l.shelfPane.onReveal = { url in pane { $0.navigate(to: url.deletingLastPathComponent(), selecting: url.lastPathComponent) }() }
        l.onDiskUsage = { url in pane { $0.navigate(to: BrowserTab.diskUsageURL(url)) }() }
        l.onOpenDiskImage = { url in pane { $0.openDiskImage(url) }() }
        l.onBack = pane { $0.goBack(nil) }
        l.onChange = { [weak self] in
            guard let self, self.list === l else { return }
            self.updateChrome()
        }
        l.onError = { [weak self] text in self?.showError(text) }
        l.onReload = { names, rename in pane { $0.reload(selecting: names, rename: rename) }() }
        l.commandBar.filter.action = { _ in pane { $0.focusSearch(nil) }() }
    }

    // MARK: dual pane

    /// The pane `right` takes the keyboard: the address bar, Back, Up and the menus follow it.
    private func focusPane(right: Bool) {
        guard dual, right != rightActive else { return }
        navBar.omnibar.end()
        rightActive = right
        watch()
        updateChrome()
        root.needsLayout = true
        leftList.card.needsDisplay = true
        rightList.card.needsDisplay = true
    }

    /// View › Dual Pane: a second folder beside the first in this tab (the same folder to
    /// start with), or back to one.
    @objc func toggleDualPane(_ sender: Any?) {
        let t = tabs[active]
        if t.right != nil {
            saveTabState()
            t.right?.search?.cancel()
            t.right = nil
            rightActive = false
            leftList.card.isActivePane = false
            root.needsLayout = true
            showTab()
        } else {
            saveTabState()
            let r = BrowserTab(url: tab.isPage ? FileManager.default.homeDirectoryForCurrentUser : tab.url)
            t.right = r
            rightActive = true
            root.needsLayout = true
            load(r)
            updateChrome()
        }
    }

    // MARK: tabs

    private func refreshTabs() {
        defer { AppDelegate.shared.saveSession() }
        // tabs of folders with the same name each get the folder above that tells them apart
        let hints = TabStripView.hints(tabs.map { $0.isPage || !$0.url.isFileURL ? nil : $0.url })
        titleBar.strip.tabs = tabs.enumerated().map { i, t in
            var info = t.info
            if !t.isPage, t.url.isFileURL {
                info.path = (t.url.path as NSString).abbreviatingWithTildeInPath
                info.hint = hints[i]
            }
            return info
        }
        titleBar.strip.active = active
        titleBar.needsLayout = true
        window?.title = tab.info.title
    }

    private func saveTabState() {
        for (t, l) in panes where l.url == t.url {
            t.selection = l.selectedNames
            t.scrollY = l.scrollY
        }
    }

    private func activate(_ i: Int) {
        guard i != active, tabs.indices.contains(i) else { return }
        saveTabState()
        active = i
        navBar.omnibar.end()
        showTab()
        refreshTabs()
    }

    /// Shows the active tab: its pane, or both of a dual pane.
    private func showTab() {
        if !dual { rightActive = false }
        root.needsLayout = true
        for (t, l) in panes { show(t, in: l) }
        watch()
        updateChrome()
        list.focus()
    }

    /// Shows `t` in its list `l`.
    private func show(_ t: BrowserTab, in l: FileListController) {
        if t.isSettings {
            l.showSettings()
        } else if t.isHome {
            l.showHome()
        } else if let root = t.diskRoot {
            t.disk = l.showDiskUsage(root, scan: t.disk) { [weak t] scan in t?.disk = scan }
        } else if let folder = t.folder {
            l.show(folder, at: t.url, selecting: t.selection, scrollTo: t.scrollY, archive: t.archive)
        } else if let error = t.error {
            l.show(error: error, at: t.url)
        }
    }

    /// `t` changed: shown again where it is shown (and the chrome, when it has the keyboard).
    private func refreshPane(_ t: BrowserTab) {
        guard let l = listFor(t) else { return }
        show(t, in: l)
        if t === tab {
            watch()
            updateChrome()
            l.focus()
        }
    }

    @objc func newTab(_ sender: Any?) {
        saveTabState()
        // General › A new tab opens: Home, or the folder of the tab it opens from
        let here = tab.isPage ? BrowserTab.homeURL : tab.url
        let t = BrowserTab(url: AppSettings.newTabOpens == "current" ? here : BrowserTab.homeURL)
        tabs.insert(t, at: active + 1)
        active += 1
        navBar.omnibar.end()
        load(t)
        refreshTabs()
        if AppSettings.newTabAddress { DispatchQueue.main.async { [weak self] in self?.navBar.omnibar.begin(.path) } }
    }

    override func newWindowForTab(_ sender: Any?) { newTab(sender) }

    @objc func closeTab(_ sender: Any?) { closeTab(at: active) }

    /// Tabs closed in this window, newest last ("Reopen closed tab", ⇧⌘T).
    private var closedTabs: [URL] = []

    /// A new tab right after the active one, showing `url`.
    func openTab(_ url: URL, activate: Bool? = nil, selecting name: String? = nil) {
        saveTabState()
        let t = BrowserTab(url: url)
        if let name { t.selection = [name] }
        tabs.insert(t, at: active + 1)
        // General › Always switch to a newly opened tab; off: it waits behind the current one
        guard activate ?? AppSettings.switchToNewTab else {
            refreshTabs()
            return
        }
        active += 1
        navBar.omnibar.end()
        load(t)
        refreshTabs()
    }

    /// The tabs' addresses, in order (paths, or rustyfiles:home / :settings).
    var tabAddresses: [String] { tabs.map { Sessions.string($0.url) } }
    var activeTab: Int { active }

    /// Opens these addresses as the window's tabs (a saved session).
    func restore(_ addresses: [String], active a: Int) {
        let urls = addresses.compactMap(Sessions.url)
        guard !urls.isEmpty else { return }
        tabs = urls.map { BrowserTab(url: $0) }
        active = min(max(a, 0), tabs.count - 1)
        load(tab)
        refreshTabs()
    }

    // MARK: command palette (command_palette.rs)

    @objc func toggleShelf(_ sender: Any?) { list.shelfShown.toggle() }
    @objc func addToShelf(_ sender: Any?) { list.addToShelf() }

    /// Go › Disk Usage: where the space under the open folder goes (or the home folder's).
    @objc func showDiskUsage(_ sender: Any?) {
        let root = tab.url.isFileURL ? tab.url : (tab.searchSpec?.root ?? tab.diskRoot ?? FileManager.default.homeDirectoryForCurrentUser)
        navigate(to: BrowserTab.diskUsageURL(root))
    }

    @objc func showCommandPalette(_ sender: Any?) { CommandPalette.show(.all, over: self) }
    @objc func showFolderHistory(_ sender: Any?) { CommandPalette.show(.folders, over: self) }
    @objc func searchTabs(_ sender: Any?) { CommandPalette.show(.tabs, over: self) }

    /// The open tabs of every window, then the ones closed here (most recent first), for the
    /// palette's tab search (issue 79).
    func paletteTabs() -> [PaletteEntry] {
        var out: [PaletteEntry] = []
        for c in AppDelegate.shared.controllers {
            for (i, t) in c.tabs.enumerated() {
                let info = t.info
                let detail = t.url.isFileURL ? t.url.path : ""
                out.append(PaletteEntry(title: info.title, detail: detail, icon: info.icon, hint: c === self && i == c.active ? String(localized: "Current") : "") { [weak c] in
                    guard let c else { return }
                    c.window?.makeKeyAndOrderFront(nil)
                    c.activate(i)
                })
            }
        }
        for url in closedTabs.reversed() {
            let info = BrowserTab(url: url).info
            out.append(PaletteEntry(title: info.title, detail: url.isFileURL ? url.path : "", icon: "rotate-ccw", hint: String(localized: "Closed")) { [weak self] in
                guard let self else { return }
                if let i = self.closedTabs.lastIndex(of: url) { self.closedTabs.remove(at: i) }
                self.openTab(url, activate: true)
            })
        }
        return out
    }

    @objc func reopenClosedTab(_ sender: Any?) {
        guard let url = closedTabs.popLast() else { return }
        openTab(url)
    }

    private func closeTabs(_ indices: [Int]) {
        guard !indices.isEmpty else { return }
        let keep = tabs.indices.filter { !indices.contains($0) }
        guard !keep.isEmpty else { return window?.performClose(nil) ?? () }
        saveTabState()
        let current = tabs[active]
        closedTabs += indices.map { tabs[$0].url }
        tabs = keep.map { tabs[$0] }
        active = tabs.firstIndex { $0 === current } ?? min(keep.firstIndex { $0 > indices.min()! } ?? 0, tabs.count - 1)
        showTab()
        refreshTabs()
    }

    /// The tab menu (workspace.rs `tab_menu`).
    private func tabMenu(_ i: Int) -> NSMenu {
        let b = MenuBuilder()
        let n = tabs.count
        b.add(String(localized: "New tab"), "plus", key: "t") { [weak self] in self?.newTab(nil) }
        b.add(String(localized: "Duplicate tab"), "copy") { [weak self] in
            guard let self else { return }
            self.activate(i)
            self.openTab(self.tabs[i].url)
        }
        b.add(String(localized: "Move to new window"), "app-window", enabled: n > 1) { [weak self] in
            guard let self, self.tabs.indices.contains(i) else { return }
            let url = self.tabs[i].url
            self.closeTabs([i])
            AppDelegate.shared.openWindow(at: url)
        }
        b.separator()
        b.add(String(localized: "Save session…"), "save") { [weak self] in self?.saveNamedSession() }
        let saved = NamedSessions.all
        if !saved.isEmpty {
            b.sub(String(localized: "Open session"), "folder-open") { m in
                for s in saved { m.add(s.name) { [weak self] in self?.openNamedSession(s) } }
            }
            b.sub(String(localized: "Delete session"), "trash") { m in
                for s in saved { m.add(s.name) { [weak self] in
                    NamedSessions.delete(s.name)
                    if self?.sessionName == s.name { self?.sessionName = nil }
                } }
            }
        }
        b.separator()
        b.add(String(localized: "Close other tabs"), enabled: n > 1) { [weak self] in self?.closeTabs((0..<n).filter { $0 != i }) }
        b.add(String(localized: "Close tabs to the left"), enabled: i > 0) { [weak self] in self?.closeTabs(Array(0..<i)) }
        b.add(String(localized: "Close tabs to the right"), enabled: i < n - 1) { [weak self] in self?.closeTabs(Array((i + 1)..<n)) }
        b.add(String(localized: "Close all tabs")) { [weak self] in self?.window?.performClose(nil) }
        b.add(String(localized: "Close tab"), "x", key: "w") { [weak self] in self?.closeTab(at: i) }
        b.separator()
        b.add(String(localized: "Reopen closed tab"), "rotate-ccw", key: "t", mods: [.command, .shift], enabled: !closedTabs.isEmpty) { [weak self] in
            self?.reopenClosedTab(nil)
        }
        return b.done()
    }

    // MARK: named sessions

    /// The saved session this window was opened from or saved as; its name stands beside the tabs.
    var sessionName: String? {
        didSet {
            titleBar.sessionName = sessionName
            titleBar.needsLayout = true
        }
    }

    /// Tab menu › Save session…: this window's tabs under a name.
    private func saveNamedSession() {
        let alert = NSAlert()
        alert.messageText = String(localized: "Save session")
        alert.informativeText = String(localized: "This window's tabs are saved under this name; Open session in the tab menu opens them in a new window.")
        let field = NSTextField(string: sessionName ?? String(localized: "Session \(NamedSessions.all.count + 1)"))
        field.frame = NSRect(x: 0, y: 0, width: 260, height: 24)
        alert.accessoryView = field
        alert.addButton(withTitle: String(localized: "Save"))
        alert.addButton(withTitle: String(localized: "Cancel"))
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let name = field.stringValue.trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty else { return }
        saveTabState()
        NamedSessions.save(.init(name: name, tabs: tabAddresses, active: active))
        sessionName = name
    }

    /// Tab menu › Open session: in a new window, or in this one (Settings).
    private func openNamedSession(_ s: NamedSessions.Saved) {
        if AppSettings.sessionsInThisWindow {
            closedTabs += tabs.map(\.url)
            restore(s.tabs, active: s.active)
            sessionName = s.name
        } else {
            let home = BrowserTab.homeURL
            let wc = AppDelegate.shared.openWindow(at: home)
            wc.restore(s.tabs, active: s.active)
            wc.sessionName = s.name
        }
    }

    /// A tab dragged to another place in the strip.
    private func moveTab(_ from: Int, _ to: Int) {
        guard tabs.indices.contains(from), tabs.indices.contains(to), from != to else { return }
        let activeTab = tabs[active]
        tabs.insert(tabs.remove(at: from), at: to)
        active = tabs.firstIndex { $0 === activeTab } ?? active
        refreshTabs()
    }

    /// A tab dragged out of the strip: a window of its own, its top left where it was let go.
    private func tearOff(_ i: Int, at point: NSPoint) {
        guard tabs.count > 1, tabs.indices.contains(i) else { return }
        if i == active { saveTabState() }
        let t = tabs[i]
        closeTabs([i])
        let wc = AppDelegate.shared.openWindow(at: t.url, selecting: t.selection.first)
        wc.window?.setFrameTopLeftPoint(NSPoint(x: point.x - 60, y: point.y + 20))
    }

    private func closeTab(at i: Int) {
        guard tabs.indices.contains(i) else { return }
        if tabs.count == 1 {
            window?.performClose(nil)
            return
        }
        if i == active { saveTabState() }
        closedTabs.append(tabs[i].url)
        tabs.remove(at: i)
        if i < active || active >= tabs.count { active = max(active - 1, 0) }
        showTab()
        refreshTabs()
    }

    @objc func selectNextTab(_ sender: Any?) { activate((active + 1) % tabs.count) }
    @objc func selectPreviousTab(_ sender: Any?) { activate((active - 1 + tabs.count) % tabs.count) }

    // MARK: navigation

    func navigate(to url: URL, selecting name: String? = nil) {
        let url = url.isFileURL ? url.standardizedFileURL : url
        guard url != tab.url else { return }
        saveTabState()
        tab.back.append(tab.url)
        tab.forward.removeAll()
        tab.url = url
        tab.selection = name.map { [$0] } ?? []
        tab.scrollY = nil
        navBar.omnibar.end()
        load(tab)
        refreshTabs()
    }

    /// Lists the tab's folder off the main thread: a slow or hung disk never stalls the window.
    private func load(_ t: BrowserTab, keepScroll: Bool = false) {
        t.loadID += 1
        let id = t.loadID
        let url = t.url
        if t.isPage {
            refreshPane(t)
            return
        }
        if t.searchSpec == nil { t.search?.cancel(); t.search = nil }
        if let spec = t.searchSpec {
            if t === tab { updateChrome() }
            return startSearch(t, spec, id: id)
        }
        let pl = listFor(t) ?? list
        if listFor(t) != nil, ArchivePlace.locate(url) == nil { pl.prepareLayout(for: url) }
        let (column, ascending, first, hidden) = (pl.sortKey.sort, pl.ascending, pl.foldersFirst, pl.showHidden)
        if t === tab { updateChrome() }
        Task.detached(priority: .userInitiated) {
            // a place inside an archive is listed from the archive (once; then from memory)
            if let place = ArchivePlace.locate(url) {
                let (cached, pw) = await MainActor.run { (ArchiveStore.cached(place.archive), ArchiveStore.password(for: place.archive)) }
                let opened: Result<Archive, Error> = cached.map { .success($0) } ?? Result { try Archive.open(path: place.archive.path, password: pw) }
                await MainActor.run { [weak self] in
                    guard let self, t.loadID == id else { return }
                    self.finishArchiveLoad(t, place: place, opened: opened, triedPassword: pw != nil)
                }
                return
            }
            let began = Date()
            let result: Result<Folder, Error>
            if let smart = SmartFolder.file(of: url) {
                let paths = SmartFolder.results(smart)
                result = .success(Folder.gathered(label: url.absoluteString, paths: paths, column: column, ascending: ascending, foldersFirst: first, showHidden: hidden))
            } else if let tag = TagStore.name(of: url) {
                let paths = TagStore.files(tagged: tag)
                result = .success(Folder.gathered(label: url.absoluteString, paths: paths, column: column, ascending: ascending, foldersFirst: first, showHidden: hidden))
            } else {
                result = Result { try Folder.open(path: url.path, column: column, ascending: ascending, foldersFirst: first, showHidden: hidden) }
            }
            await MainActor.run { [weak self] in
                guard let self, t.loadID == id else { return }
                t.archive = nil
                switch result {
                case .success(let f):
                    t.folder = f
                    t.error = nil
                    if url.isFileURL { FolderHistory.record(url) }
                case .failure(let e):
                    t.folder = nil
                    t.error = Self.describe(e, at: url)
                }
                guard let l = self.listFor(t) else { return }
                if keepScroll { t.scrollY = l.scrollY }
                self.refreshPane(t)
                // Advanced › Performance in the status bar
                let ms = Date().timeIntervalSince(began) * 1000
                l.status.performance = AppSettings.performanceInfo
                    ? String(localized: "Listed in \(String(format: "%.0f", ms)) ms") : ""
                if self.pendingRename && l === self.list {
                    self.pendingRename = false
                    l.beginRename()
                }
            }
        }
    }

    private var pendingRename = false

    // MARK: search (Location::Search)

    /// Searches from the filter box: Enter looks through the folder and every folder under it.
    func runSearch(_ query: String) {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return }
        let root = tab.searchSpec?.root ?? (tab.url.isFileURL ? tab.url : FileManager.default.homeDirectoryForCurrentUser)
        navigate(to: BrowserTab.searchURL(root: root, query: q))
    }

    /// Starts the walk and shows what it finds as it finds it (four times a second).
    private func startSearch(_ t: BrowserTab, _ spec: (root: URL, query: String), id: Int) {
        t.search?.cancel()
        t.searchHits = []
        t.searchDone = false
        let pl = listFor(t) ?? list
        let job = SearchJob.start(root: spec.root.path, query: spec.query, includeHidden: pl.showHidden)
        t.search = job
        if t.folder == nil || t.searchHits.isEmpty {
            t.folder = Folder.gathered(label: t.url.absoluteString, paths: [], column: pl.sortKey.sort, ascending: pl.ascending, foldersFirst: pl.foldersFirst, showHidden: pl.showHidden)
            t.error = nil
            refreshPane(t)
        }
        pollSearch(t, id: id, shown: 0)
    }

    private func pollSearch(_ t: BrowserTab, id: Int, shown: Int) {
        guard t.loadID == id, let job = t.search else { return }
        let fresh = job.take()
        let done = job.isDone()
        t.searchHits += fresh
        let count = t.searchHits.count
        if let l = listFor(t) {
            l.status.performance = done
                ? String(localized: "Search finished · \(count.formatted()) found")
                : String(localized: "Searching… \(count.formatted()) found in \(Int(job.foldersSeen()).formatted()) folders")
        }
        if count != shown || done {
            let paths = t.searchHits, label = t.url.absoluteString
            let pl = listFor(t) ?? list
            let (column, ascending, first, hidden) = (pl.sortKey.sort, pl.ascending, pl.foldersFirst, pl.showHidden)
            Task.detached(priority: .userInitiated) {
                let f = Folder.gathered(label: label, paths: paths, column: column, ascending: ascending, foldersFirst: first, showHidden: hidden)
                await MainActor.run { [weak self] in
                    guard let self, t.loadID == id else { return }
                    if let l = self.listFor(t), l.url == t.url {
                        t.selection = l.selectedNames
                        t.scrollY = l.scrollY
                    }
                    t.folder = f
                    t.searchDone = done
                    self.listFor(t)?.show(f, at: t.url, selecting: t.selection, scrollTo: t.scrollY, archive: nil)
                }
            }
        }
        guard !done else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { [weak self] in self?.pollSearch(t, id: id, shown: count) }
    }

    /// The archive is listed (or not): show the folder in it, or ask for its password.
    private func finishArchiveLoad(_ t: BrowserTab, place: ArchivePlace, opened: Result<Archive, Error>, triedPassword: Bool) {
        switch opened {
        case .success(let archive):
            ArchiveStore.store(archive, for: place.archive)
            if archive.hasFolder(inner: place.inner),
                let f = archive.folder(inner: place.inner, column: (listFor(t) ?? list).sortKey.sort, ascending: (listFor(t) ?? list).ascending, foldersFirst: (listFor(t) ?? list).foldersFirst, showHidden: (listFor(t) ?? list).showHidden)
            {
                t.folder = f
                t.error = nil
                t.archive = ArchiveView(place: place, archive: archive)
            } else {
                t.folder = nil
                t.archive = nil
                t.error = String(localized: "“\(place.inner)” is not in “\(place.archive.lastPathComponent)”.")
            }
        case .failure(ArchiveError.PasswordNeeded):
            guard listFor(t) != nil else { return }
            if let pw = ArchiveStore.askPassword(for: place.archive, in: window, wrong: triedPassword) {
                ArchiveStore.setPassword(pw, for: place.archive)
                return load(t)
            }
            t.folder = nil
            t.archive = nil
            t.error = String(localized: "“\(place.archive.lastPathComponent)” needs a password.")
        case .failure(let e):
            t.folder = nil
            t.archive = nil
            t.error = String(localized: "“\(place.archive.lastPathComponent)” can’t be opened: \(e.localizedDescription)")
        }
        refreshPane(t)
    }

    /// Ejects a disk; a tab showing it goes Home.
    func eject(_ volume: URL) {
        let path = volume.standardizedFileURL.path
        for t in tabs where t.url.path == path || t.url.path.hasPrefix(path + "/") {
            if t === tab { goHome(nil) } else { t.url = FileManager.default.homeDirectoryForCurrentUser }
        }
        watcher = nil
        Task.detached(priority: .userInitiated) {
            let result = Result { try NSWorkspace.shared.unmountAndEjectDevice(at: volume) }
            if case .failure(let e) = result {
                await MainActor.run { [weak self] in self?.showError(e.localizedDescription) }
            }
        }
    }

    /// Mounts a disk image and shows the disk (it also appears under Drives).
    func openDiskImage(_ url: URL) {
        Task.detached(priority: .userInitiated) {
            let result = Result { try DiskImages.attach(url) }
            await MainActor.run { [weak self] in
                switch result {
                case .success(let mount): self?.navigate(to: mount)
                case .failure(let e): self?.showError(e.localizedDescription)
                }
            }
        }
    }

    /// Lists the folder again in place: selection and scroll kept, or `names` selected.
    private func reload(selecting names: [String]? = nil, rename: Bool = false) {
        if let names, !names.isEmpty {
            tab.selection = names
            tab.scrollY = nil
        } else {
            tab.selection = list.selectedNames
        }
        pendingRename = rename
        load(tab, keepScroll: names?.isEmpty ?? true)
    }

    private func watch() {
        watcher = tab.isPage || !tab.url.isFileURL ? nil : DirectoryWatcher(url: tab.url) { [weak self] in self?.reload() }
    }

    private static func describe(_ error: Error, at url: URL) -> String {
        let name = FileManager.default.displayName(atPath: url.path)
        switch error as? FsError {
        case .NotFound:
            return String(localized: "“\(name)” can’t be found.")
        case .PermissionDenied:
            return String(localized: "rustyfiles can’t open “\(name)” because it doesn’t have permission. You can give it access in System Settings › Privacy & Security › Full Disk Access.")
        default:
            return String(localized: "“\(name)” can’t be opened: \(error.localizedDescription)")
        }
    }

    private func updateChrome() {
        navBar.back.isEnabled = !tab.back.isEmpty
        navBar.forward.isEnabled = !tab.forward.isEmpty
        navBar.up.isEnabled = !tab.isHome && (tab.isSettings || tab.tagName != nil || tab.smartFolder != nil || tab.searchSpec != nil || tab.diskRoot != nil || tab.url.path != "/")
        if tab.isSettings {
            navBar.omnibar.breadcrumb.segments = [BreadcrumbView.Segment(title: String(localized: "Settings"), url: BrowserTab.settingsURL)]
        } else if tab.isHome {
            navBar.omnibar.breadcrumb.segments = [BreadcrumbView.Segment(title: String(localized: "Home"), url: BrowserTab.homeURL)]
        } else if let tag = tab.tagName {
            navBar.omnibar.breadcrumb.segments = [BreadcrumbView.Segment(title: String(localized: "Tags"), url: BrowserTab.homeURL), BreadcrumbView.Segment(title: tag, url: tab.url)]
        } else if let s = tab.smartFolder {
            navBar.omnibar.breadcrumb.segments = BreadcrumbView.segments(for: s.deletingLastPathComponent()) + [BreadcrumbView.Segment(title: s.deletingPathExtension().lastPathComponent, url: tab.url)]
        } else if let r = tab.diskRoot {
            navBar.omnibar.breadcrumb.segments = BreadcrumbView.segments(for: r) + [BreadcrumbView.Segment(title: String(localized: "Disk usage"), url: tab.url)]
        } else if let s = tab.searchSpec {
            // the folder searched, then the search
            navBar.omnibar.breadcrumb.segments = BreadcrumbView.segments(for: s.root) + [BreadcrumbView.Segment(title: String(localized: "Search: “\(s.query)”"), url: tab.url)]
        } else {
            navBar.omnibar.breadcrumb.segments = BreadcrumbView.segments(for: tab.url)
        }
        navBar.omnibar.currentPath = tab.isPage || !tab.url.isFileURL ? "" : tab.url.path
        sidebar.selectedURL = tab.url
    }

    private func submitPath(_ text: String) {
        // "home" is the Home page, as on Windows
        if text.trimmingCharacters(in: .whitespaces).lowercased() == "home" { return goHome(nil) }
        // "@name": a folder alias (Files › Folder aliases), "@name/sub" goes inside it
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        if trimmed.hasPrefix("@") {
            let parts = trimmed.dropFirst().split(separator: "/", maxSplits: 1).map(String.init)
            if let first = parts.first, let target = AppSettings.folderAliases[first] ?? AppSettings.folderAliases.first(where: { $0.key.lowercased() == first.lowercased() })?.value {
                return submitPath(parts.count > 1 ? target + "/" + parts[1] : target)
            }
        }
        let path = (text.trimmingCharacters(in: .whitespaces) as NSString).expandingTildeInPath
        let url = path.hasPrefix("/") ? URL(fileURLWithPath: path) : tab.url.appendingPathComponent(path)
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir) else {
            // Files › Run programs from the address bar
            if AppSettings.addressCommands, !tab.isPage, AddressCommand.run(text, in: tab.url) { return }
            showError(String(localized: "“\(text)” can’t be found."))
            return
        }
        if isDir.boolValue {
            navigate(to: url)
        } else {
            navigate(to: url.deletingLastPathComponent(), selecting: url.lastPathComponent)
        }
    }

    func showError(_ text: String) {
        Log.write(text)
        guard let window else { return }
        let alert = NSAlert()
        alert.messageText = text
        alert.alertStyle = .warning
        alert.beginSheetModal(for: window)
    }

    // MARK: actions (menus, buttons, keys)

    @objc func goBack(_ sender: Any?) {
        guard let url = tab.back.popLast() else { return }
        saveTabState()
        tab.forward.append(tab.url)
        tab.url = url
        tab.selection = []
        tab.scrollY = nil
        load(tab)
        refreshTabs()
    }

    @objc func goForward(_ sender: Any?) {
        guard let url = tab.forward.popLast() else { return }
        saveTabState()
        tab.back.append(tab.url)
        tab.url = url
        tab.selection = []
        tab.scrollY = nil
        load(tab)
        refreshTabs()
    }

    @objc func goUp(_ sender: Any?) {
        // Settings goes up to Home, as on Windows
        if tab.isSettings { return goHome(sender) }
        if tab.isHome { return }
        if tab.tagName != nil { return goHome(sender) }
        if let s = tab.smartFolder { return navigate(to: s.deletingLastPathComponent(), selecting: s.lastPathComponent) }
        // a search goes up to the folder it searched
        if let s = tab.searchSpec { return navigate(to: s.root) }
        if let r = tab.diskRoot { return navigate(to: r) }
        let parent = tab.url.deletingLastPathComponent()
        guard parent.path != tab.url.path else { return }
        navigate(to: parent, selecting: AppSettings.upSelectsPrevious ? tab.url.lastPathComponent : nil)
    }

    @objc func goHome(_ sender: Any?) { navigate(to: BrowserTab.homeURL) }
    @objc func goHomeFolder(_ sender: Any?) { navigate(to: FileManager.default.homeDirectoryForCurrentUser) }
    @objc func goDocuments(_ sender: Any?) { go(.documentDirectory) }
    @objc func goDesktop(_ sender: Any?) { go(.desktopDirectory) }
    /// Go › Connect to Server…: a network share mounted and opened here.
    @objc func connectToServer(_ sender: Any?) {
        ConnectServer.ask(in: window) { [weak self] url in self?.navigate(to: url) }
    }
    @objc func goDownloads(_ sender: Any?) { go(.downloadsDirectory) }
    @objc func goApplications(_ sender: Any?) { navigate(to: URL(fileURLWithPath: "/Applications")) }

    private func go(_ dir: FileManager.SearchPathDirectory) {
        if let url = FileManager.default.urls(for: dir, in: .userDomainMask).first { navigate(to: url) }
    }

    @objc func refresh(_ sender: Any?) {
        if tab.isHome { return list.reloadHome() }
        reload()
    }
    @objc func openSelection(_ sender: Any?) { list.openSelected() }
    @objc func toggleHidden(_ sender: Any?) { AppSettings.showHidden.toggle() }
    @objc func toggleAllGroups(_ sender: Any?) { list.toggleAllGroups() }
    @objc func zoomIn(_ sender: Any?) { list.zoom(1) }
    /// Settings › Appearance › Interface scale, a step at a time (⌥⌘+, ⌥⌘−, ⌥⌘0).
    @objc func interfaceBigger(_ sender: Any?) { stepScale(1) }
    @objc func interfaceSmaller(_ sender: Any?) { stepScale(-1) }
    @objc func interfaceActual(_ sender: Any?) { AppSettings.interfacePercent = 100 }
    private func stepScale(_ d: Int) {
        let all = AppSettings.interfaceScales
        let i = all.firstIndex(of: AppSettings.interfacePercent) ?? all.firstIndex(of: 100)!
        AppSettings.interfacePercent = all[min(max(i + d, 0), all.count - 1)]
    }
    @objc func zoomOut(_ sender: Any?) { list.zoom(-1) }
    @objc func openSettings(_ sender: Any?) { navigate(to: BrowserTab.settingsURL) }
    func showSettingsPage(_ page: SettingsPage) { list.showSettings(page: page) }

    /// Development aid (RF_DEMO_PREVIEW): the pane open on `row`, the setting left as it was.
    func demoPreview(row: Int) {
        let was = UserDefaults.standard.bool(forKey: "previewShown")
        list.previewShown = true
        UserDefaults.standard.set(was, forKey: "previewShown")
        list.table.selectRowIndexes([row], byExtendingSelection: false)
    }

    /// Development aid (RF_DEMO_MENU): opens the list's menu as a right-click would.
    func demoMenu(item: Bool) {
        if item { list.table.selectRowIndexes([0], byExtendingSelection: false) } else { list.table.deselectAll(nil) }
        let menu = list.contextMenu()
        menu.popUp(positioning: nil, at: NSPoint(x: 300, y: item ? 20 : 200), in: list.table)
    }

    func openSettings(page: SettingsPage) {
        openSettings(nil)
        showSettingsPage(page)
    }

    // MARK: Status Center

    private lazy var statusPopover: NSPopover = {
        let p = NSPopover()
        p.behavior = .transient
        let v = StatusCenterView()
        let vc = NSViewController()
        vc.view = v
        v.onResize = { [weak p] size in p?.contentSize = size }
        p.contentViewController = vc
        p.contentSize = v.frame.size
        return p
    }()

    func toggleStatusCenter() {
        if statusPopover.isShown { return statusPopover.performClose(nil) }
        StatusCenter.shared.unseenFailure = false
        statusChanged()
        statusPopover.show(relativeTo: navBar.statusCenter.bounds, of: navBar.statusCenter, preferredEdge: .maxY)
    }

    /// The ☑ button: accent while something runs, ⚠ after a failure nobody has seen.
    @objc private func statusChanged() {
        let sc = StatusCenter.shared
        navBar.statusCenter.icon = sc.unseenFailure ? "circle-alert" : "list-checks"
        navBar.statusCenter.tint = { p in sc.unseenFailure ? p.danger : (sc.isBusy ? p.accent : p.text) }
        // a ring fills round the button while something runs
        navBar.statusCenter.ring = sc.isBusy ? (sc.overallProgress ?? 0) : nil
        navBar.statusCenter.iconSize = sc.isBusy ? 14 : 16
        navBar.statusCenter.needsDisplay = true
    }
    @objc func focusSearch(_ sender: Any?) { navBar.omnibar.mode == .filter ? navBar.omnibar.end() : navBar.omnibar.begin(.filter) }
    @objc func focusAddress(_ sender: Any?) { navBar.omnibar.begin(.path) }
    @objc func newFolder(_ sender: Any?) { list.newFolder() }
    @objc func cut(_ sender: Any?) { list.cutItems() }
    @objc func copy(_ sender: Any?) { list.copyItems() }
    @objc func paste(_ sender: Any?) { list.pasteItems() }
    @objc func duplicate(_ sender: Any?) { list.duplicateSelected() }

    /// The folder shown in the pane without the keyboard (dual pane), if it is a folder.
    var otherPaneURL: URL? {
        guard dual else { return nil }
        let other = rightActive ? leftList : rightList
        return other.url?.isFileURL == true && other.archiveIsNil ? other.url : nil
    }

    /// ⇧F5 / ⇧F6: the selection copied or moved into the other pane's folder.
    @objc func copyToOtherPane(_ sender: Any?) { toOtherPane(.copy) }
    @objc func moveToOtherPane(_ sender: Any?) { toOtherPane(.move) }
    private func toOtherPane(_ kind: Transfer.Kind) {
        guard let dest = otherPaneURL else { return }
        list.transfer(list.selectedURLs, into: dest, kind: kind, selectHere: false)
    }

    /// F6: the keyboard goes to the other pane.
    @objc func switchPane(_ sender: Any?) {
        guard dual else { return }
        (rightActive ? leftList : rightList).focus()
    }
    @objc func rename(_ sender: Any?) { list.beginRename() }
    @objc func moveToTrash(_ sender: Any?) { list.trashSelected() }
    @objc func toggleQuickLook(_ sender: Any?) { list.toggleQuickLook() }
    @objc func showDetails(_ sender: Any?) { list.setViewMode(.details) }
    @objc func showTiles(_ sender: Any?) { list.setViewMode(.tiles) }
    @objc func showCards(_ sender: Any?) { list.setViewMode(.cards) }
    @objc func showListStyle(_ sender: Any?) { list.setViewMode(.list) }
    @objc func showColumns(_ sender: Any?) { list.setViewMode(.columns) }

    /// Properties of the selection, or of the folder shown when nothing is selected (⌘I).
    @objc func showProperties(_ sender: Any?) { list.showProperties() }

    @objc func togglePreviewPane(_ sender: Any?) {
        list.togglePreviewPane(nil)
        // every window follows
        for c in AppDelegate.shared.controllers where c !== self { c.list.previewShown = list.previewShown }
    }

    @objc func toggleSidebar(_ sender: Any?) {
        sidebarShown.toggle()
        UserDefaults.standard.set(!sidebarShown, forKey: "sidebarHidden")
        root.needsLayout = true
    }

    func validateMenuItem(_ item: NSMenuItem) -> Bool {
        let selected = !list.selectedNames.isEmpty
        switch item.action {
        case #selector(goBack(_:)): return !tab.back.isEmpty
        case #selector(goForward(_:)): return !tab.forward.isEmpty
        case #selector(goUp(_:)): return tab.url.path != "/"
        case #selector(openSelection(_:)), #selector(cut(_:)), #selector(BrowserWindowController.copy(_:) as (BrowserWindowController) -> (Any?) -> Void), #selector(moveToTrash(_:)),
            #selector(toggleQuickLook(_:)):
            return selected
        case #selector(rename(_:)): return !list.selectedNames.isEmpty && (list.archiveIsNil || list.archiveEditable)
        case #selector(addToShelf(_:)): return selected && list.archiveIsNil
        case #selector(toggleDualPane(_:)):
            item.state = dual ? .on : .off
            return true
        case #selector(toggleShelf(_:)):
            item.title = list.shelfShown ? String(localized: "Hide Shelf") : String(localized: "Show Shelf")
            return true
        case #selector(paste(_:)): return list.folder != nil && (list.archiveIsNil ? FileListController.canPaste : list.archiveEditable && Clipboard.hasFiles)
        case #selector(duplicate(_:)): return selected && list.archiveIsNil && list.url?.isFileURL == true
        case #selector(copyToOtherPane(_:)), #selector(moveToOtherPane(_:)): return dual && selected && list.archiveIsNil && otherPaneURL != nil
        case #selector(switchPane(_:)): return dual
        case #selector(newFolder(_:)): return list.folder != nil
        case #selector(selectNextTab(_:)), #selector(selectPreviousTab(_:)): return tabs.count > 1
        case #selector(reopenClosedTab(_:)): return !closedTabs.isEmpty
        case #selector(showDetails(_:)):
            item.state = list.viewMode == .details ? .on : .off
            return true
        case #selector(showTiles(_:)):
            item.state = list.viewMode == .tiles ? .on : .off
            return true
        case #selector(showCards(_:)):
            item.state = list.viewMode == .cards ? .on : .off
            return true
        case #selector(showListStyle(_:)):
            item.state = list.viewMode == .list ? .on : .off
            return true
        case #selector(showColumns(_:)):
            item.state = list.viewMode == .columns ? .on : .off
            return true
        case #selector(zoomIn(_:)), #selector(zoomOut(_:)): return !tab.isPage
        case #selector(toggleAllGroups(_:)): return list.groupsApply && !list.groupCounts.isEmpty
        case #selector(toggleHidden(_:)):
            item.state = AppSettings.showHidden ? .on : .off
            return true
        case #selector(showProperties(_:)): return list.archiveIsNil && list.url != nil
        case #selector(togglePreviewPane(_:)):
            item.title = list.previewShown ? String(localized: "Hide Preview") : String(localized: "Show Preview")
            return true
        case #selector(toggleSidebar(_:)):
            item.title = sidebarShown ? String(localized: "Hide Sidebar") : String(localized: "Show Sidebar")
            return true
        default: return true
        }
    }
}

extension Array {
    subscript(safe i: Int) -> Element? { indices.contains(i) ? self[i] : nil }
}
