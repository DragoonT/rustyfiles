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

    init(url: URL) { self.url = url.standardizedFileURL }

    /// The Settings page is a place a tab goes to, like a folder (Location::Settings on Windows).
    static let settingsURL = URL(string: "rustyfiles:settings")!
    /// The Home page (Location::Home on Windows): quick access, drives, recent items.
    static let homeURL = URL(string: "rustyfiles:home")!
    var isSettings: Bool { url == Self.settingsURL }
    var isHome: Bool { url == Self.homeURL }
    /// A page of the app's own rather than a folder.
    var isPage: Bool { isSettings || isHome }

    var info: TabInfo {
        if isSettings { return TabInfo(title: String(localized: "Settings"), icon: "settings") }
        if isHome { return TabInfo(title: String(localized: "Home"), icon: "house") }
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
    private let list = FileListController()
    private var tabs: [BrowserTab] = []
    private var active = 0
    private var tab: BrowserTab { tabs[active] }
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
        [sidebar, handle, list.card, navBar, titleBar].forEach(root.addSubview)
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
        list.card.frame = NSRect(x: left, y: top, width: b.width - left - Metrics.cardRightMargin, height: b.height - top - Metrics.bottomSpacer)
    }

    func windowDidBecomeKey(_ notification: Notification) { list.updateCommands() }

    func windowWillClose(_ notification: Notification) { onClose?(self) }

    /// A setting changed: the list takes what concerns it; colours redraw everywhere (and
    /// lenses find their places again, or go, when the material changed).
    @objc private func settingsChanged(_ note: Notification) {
        let key = AppSettings.changedKey(note)
        if key == "material" { applyMaterial() }
        list.apply(settingKey: key)
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
        let view: NSView?
        switch AppSettings.material {
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
        let glass = AppSettings.material == .glass
        window.isOpaque = AppSettings.material == .solid
        window.backgroundColor = AppSettings.material == .solid ? .windowBackgroundColor : .clear
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
        omni.onSubmitPath = { [weak self] text in self?.submitPath(text) }
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

        list.onOpenFolder = { [weak self] url in self?.navigate(to: url) }
        list.onOpenDiskImage = { [weak self] url in self?.openDiskImage(url) }
        list.onBack = { [weak self] in self?.goBack(nil) }
        list.onChange = { [weak self] in self?.updateChrome() }
        list.onError = { [weak self] text in self?.showError(text) }
        list.onReload = { [weak self] names, rename in self?.reload(selecting: names, rename: rename) }
        list.commandBar.filter.action = { [weak self] _ in self?.focusSearch(nil) }
    }

    // MARK: tabs

    private func refreshTabs() {
        titleBar.strip.tabs = tabs.map(\.info)
        titleBar.strip.active = active
        titleBar.needsLayout = true
        window?.title = tab.info.title
    }

    private func saveTabState() {
        guard tabs.indices.contains(active), list.url == tab.url else { return }
        tab.selection = list.selectedNames
        tab.scrollY = list.scrollY
    }

    private func activate(_ i: Int) {
        guard i != active, tabs.indices.contains(i) else { return }
        saveTabState()
        active = i
        navBar.omnibar.end()
        showTab()
        refreshTabs()
    }

    private func showTab() {
        if tab.isSettings {
            list.showSettings()
        } else if tab.isHome {
            list.showHome()
        } else if let folder = tab.folder {
            list.show(folder, at: tab.url, selecting: tab.selection, scrollTo: tab.scrollY, archive: tab.archive)
        } else if let error = tab.error {
            list.show(error: error, at: tab.url)
        }
        watch()
        updateChrome()
        list.focus()
    }

    @objc func newTab(_ sender: Any?) {
        saveTabState()
        // a new tab opens Home, as on Windows
        let t = BrowserTab(url: BrowserTab.homeURL)
        tabs.insert(t, at: active + 1)
        active += 1
        navBar.omnibar.end()
        load(t)
        refreshTabs()
    }

    override func newWindowForTab(_ sender: Any?) { newTab(sender) }

    @objc func closeTab(_ sender: Any?) { closeTab(at: active) }

    private func closeTab(at i: Int) {
        guard tabs.indices.contains(i) else { return }
        if tabs.count == 1 {
            window?.performClose(nil)
            return
        }
        if i == active { saveTabState() }
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
            if t === tab { showTab() }
            return
        }
        let (column, ascending, first, hidden) = (list.sortKey.sort, list.ascending, list.foldersFirst, list.showHidden)
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
            let result = Result { try Folder.open(path: url.path, column: column, ascending: ascending, foldersFirst: first, showHidden: hidden) }
            await MainActor.run { [weak self] in
                guard let self, t.loadID == id else { return }
                t.archive = nil
                switch result {
                case .success(let f):
                    t.folder = f
                    t.error = nil
                case .failure(let e):
                    t.folder = nil
                    t.error = Self.describe(e, at: url)
                }
                guard t === self.tab else { return }
                if keepScroll { t.scrollY = self.list.scrollY }
                self.showTab()
                if self.pendingRename {
                    self.pendingRename = false
                    self.list.beginRename()
                }
            }
        }
    }

    private var pendingRename = false

    /// The archive is listed (or not): show the folder in it, or ask for its password.
    private func finishArchiveLoad(_ t: BrowserTab, place: ArchivePlace, opened: Result<Archive, Error>, triedPassword: Bool) {
        switch opened {
        case .success(let archive):
            ArchiveStore.store(archive, for: place.archive)
            if archive.hasFolder(inner: place.inner),
                let f = archive.folder(inner: place.inner, column: list.sortKey.sort, ascending: list.ascending, foldersFirst: list.foldersFirst, showHidden: list.showHidden)
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
            guard t === tab else { return }
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
        guard t === tab else { return }
        showTab()
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
        watcher = tab.isPage ? nil : DirectoryWatcher(url: tab.url) { [weak self] in self?.reload() }
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
        navBar.up.isEnabled = !tab.isHome && (tab.isSettings || tab.url.path != "/")
        if tab.isSettings {
            navBar.omnibar.breadcrumb.segments = [BreadcrumbView.Segment(title: String(localized: "Settings"), url: BrowserTab.settingsURL)]
        } else if tab.isHome {
            navBar.omnibar.breadcrumb.segments = [BreadcrumbView.Segment(title: String(localized: "Home"), url: BrowserTab.homeURL)]
        } else {
            navBar.omnibar.breadcrumb.segments = BreadcrumbView.segments(for: tab.url)
        }
        navBar.omnibar.currentPath = tab.isPage ? "" : tab.url.path
        sidebar.selectedURL = tab.url
    }

    private func submitPath(_ text: String) {
        // "home" is the Home page, as on Windows
        if text.trimmingCharacters(in: .whitespaces).lowercased() == "home" { return goHome(nil) }
        let path = (text.trimmingCharacters(in: .whitespaces) as NSString).expandingTildeInPath
        let url = path.hasPrefix("/") ? URL(fileURLWithPath: path) : tab.url.appendingPathComponent(path)
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDir) else {
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
        let parent = tab.url.deletingLastPathComponent()
        guard parent.path != tab.url.path else { return }
        navigate(to: parent, selecting: tab.url.lastPathComponent)
    }

    @objc func goHome(_ sender: Any?) { navigate(to: BrowserTab.homeURL) }
    @objc func goHomeFolder(_ sender: Any?) { navigate(to: FileManager.default.homeDirectoryForCurrentUser) }
    @objc func goDocuments(_ sender: Any?) { go(.documentDirectory) }
    @objc func goDesktop(_ sender: Any?) { go(.desktopDirectory) }
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
    @objc func openSettings(_ sender: Any?) { navigate(to: BrowserTab.settingsURL) }
    func showSettingsPage(_ page: SettingsPage) { list.showSettings(page: page) }

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
    @objc func rename(_ sender: Any?) { list.beginRename() }
    @objc func moveToTrash(_ sender: Any?) { list.trashSelected() }
    @objc func toggleQuickLook(_ sender: Any?) { list.toggleQuickLook() }
    @objc func showDetails(_ sender: Any?) { list.setViewMode(.details) }
    @objc func showTiles(_ sender: Any?) { list.setViewMode(.tiles) }

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
        case #selector(rename(_:)): return list.selectedNames.count == 1
        case #selector(paste(_:)): return Clipboard.hasFiles && list.folder != nil
        case #selector(newFolder(_:)): return list.folder != nil
        case #selector(selectNextTab(_:)), #selector(selectPreviousTab(_:)): return tabs.count > 1
        case #selector(showDetails(_:)):
            item.state = list.viewMode == .details ? .on : .off
            return true
        case #selector(showTiles(_:)):
            item.state = list.viewMode == .tiles ? .on : .off
            return true
        case #selector(toggleHidden(_:)):
            item.state = AppSettings.showHidden ? .on : .off
            return true
        case #selector(toggleSidebar(_:)):
            item.title = sidebarShown ? String(localized: "Hide Sidebar") : String(localized: "Show Sidebar")
            return true
        default: return true
        }
    }
}
