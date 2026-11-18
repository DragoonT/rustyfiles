import AppKit
import CryptoKit
import UniformTypeIdentifiers

/// Our own Properties window (properties_view.rs), with the Mac's counterparts of the Windows
/// sheet's tabs:
/// - General: the name (renamed in place), type, the app it opens with (and "Change…"),
///   location, size / size on disk / contents, the three times, Locked and Hidden, and
///   "Unblock" for a downloaded file (macOS's quarantine is Windows' mark of the web).
///   Several items show what they have in common and their totals.
/// - Permissions: owner and group, and what each may do (Windows' Security tab).
/// - Details: what Spotlight knows of it (pictures, recordings, documents, apps).
/// - Hashes: MD5, SHA-1 and SHA-256, worked out when asked.
///
/// Changes apply as they are made, as in Finder's Get Info. Every disk access happens off the
/// main thread.
@MainActor
final class PropertiesWindow: NSWindowController, NSWindowDelegate {
    private static var open: [PropertiesWindow] = []

    static func show(_ urls: [URL], page: Int = 0) {
        guard !urls.isEmpty else { return }
        let w = PropertiesWindow(urls)
        if page > 0, page < w.pages.count {
            w.tabs.selectedSegment = page
            w.tabChanged()
        }
        open.append(w)
        w.showWindow(nil)
        w.window?.makeKeyAndOrderFront(nil)
    }

    enum Page: Int, CaseIterable {
        case general, permissions, details, hashes

        var title: String {
            switch self {
            case .general: String(localized: "General")
            case .permissions: String(localized: "Permissions")
            case .details: String(localized: "Details")
            case .hashes: String(localized: "Hashes")
            }
        }
    }

    private var urls: [URL]
    private let pages: [Page]
    private var page: Page = .general
    private let tabs = NSSegmentedControl()
    private let scroll = NSScrollView()
    private let stack = NSStackView()
    private let back = PropertiesBackground()
    private var work = 0

    // General's live values, filled in as they are worked out
    private var sizeValue = NSTextField(labelWithString: "")
    private var diskValue = NSTextField(labelWithString: "")
    private var contentsValue = NSTextField(labelWithString: "")

    private init(_ urls: [URL]) {
        self.urls = urls
        let single = urls.count == 1
        let isFile = single && !Self.isFolder(urls[0])
        pages = single ? (isFile ? [.general, .permissions, .details, .hashes] : [.general, .permissions, .details]) : [.general]
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 460, height: 600),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView], backing: .buffered, defer: false)
        window.titlebarAppearsTransparent = true
        window.minSize = NSSize(width: 400, height: 360)
        window.title = single
            ? String(localized: "\(FileManager.default.displayName(atPath: urls[0].path)) Properties")
            : String(localized: "\(urls.count) items Properties")
        super.init(window: window)
        window.delegate = self
        window.isReleasedWhenClosed = false

        back.frame = window.contentView!.bounds
        back.autoresizingMask = [.width, .height]
        window.contentView = back

        tabs.segmentCount = pages.count
        for (i, p) in pages.enumerated() {
            tabs.setLabel(p.title, forSegment: i)
        }
        tabs.selectedSegment = 0
        tabs.target = self
        tabs.action = #selector(tabChanged)
        tabs.isHidden = pages.count == 1
        tabs.translatesAutoresizingMaskIntoConstraints = false

        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 4, left: 16, bottom: 16, right: 16)
        stack.translatesAutoresizingMaskIntoConstraints = false
        let doc = FlippedView()
        doc.translatesAutoresizingMaskIntoConstraints = false
        doc.addSubview(stack)
        scroll.documentView = doc
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.translatesAutoresizingMaskIntoConstraints = false
        back.addSubview(tabs)
        back.addSubview(scroll)
        NSLayoutConstraint.activate([
            tabs.topAnchor.constraint(equalTo: back.topAnchor, constant: 36),
            tabs.centerXAnchor.constraint(equalTo: back.centerXAnchor),
            scroll.topAnchor.constraint(equalTo: pages.count == 1 ? back.topAnchor : tabs.bottomAnchor, constant: pages.count == 1 ? 36 : 12),
            scroll.leadingAnchor.constraint(equalTo: back.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: back.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: back.bottomAnchor),
            doc.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
            doc.trailingAnchor.constraint(equalTo: scroll.contentView.trailingAnchor),
            doc.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
            stack.leadingAnchor.constraint(equalTo: doc.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: doc.trailingAnchor),
            stack.topAnchor.constraint(equalTo: doc.topAnchor),
            stack.bottomAnchor.constraint(equalTo: doc.bottomAnchor),
        ])
        window.center()
        // a new window steps down and right from the last one, as Windows cascades them
        if let last = Self.open.last?.window {
            window.setFrameTopLeftPoint(NSPoint(x: last.frame.minX + 24, y: last.frame.maxY - 24))
        }
        build()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func windowWillClose(_ notification: Notification) {
        work += 1
        Self.open.removeAll { $0 === self }
    }

    @objc private func tabChanged() {
        page = pages[tabs.selectedSegment]
        build()
    }

    private func build() {
        work += 1
        stack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        switch page {
        case .general: buildGeneral()
        case .permissions: buildPermissions()
        case .details: buildDetails()
        case .hashes: buildHashes()
        }
    }

    // MARK: building blocks

    private func add(_ v: NSView) {
        stack.addArrangedSubview(v)
        v.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -32).isActive = true
    }

    private func section(_ title: String) {
        let l = NSTextField(labelWithString: title)
        l.font = Metrics.textXsSemibold
        l.textColor = .secondaryLabelColor
        stack.addArrangedSubview(l)
    }

    /// A rounded card of rows: key on the left (120 wide, secondary), value on the right.
    private func card(_ rows: [(String, NSView)]) {
        let c = PropertiesCard()
        var last: NSView?
        for (i, (k, v)) in rows.enumerated() {
            let key = NSTextField(labelWithString: k)
            key.font = Metrics.textXs
            key.textColor = .secondaryLabelColor
            key.translatesAutoresizingMaskIntoConstraints = false
            v.translatesAutoresizingMaskIntoConstraints = false
            c.addSubview(key)
            c.addSubview(v)
            let top = last.map { $0.bottomAnchor } ?? c.topAnchor
            NSLayoutConstraint.activate([
                key.leadingAnchor.constraint(equalTo: c.leadingAnchor, constant: 12),
                key.widthAnchor.constraint(equalToConstant: 120),
                key.firstBaselineAnchor.constraint(equalTo: v.firstBaselineAnchor),
                v.leadingAnchor.constraint(equalTo: key.trailingAnchor, constant: 8),
                v.trailingAnchor.constraint(lessThanOrEqualTo: c.trailingAnchor, constant: -12),
                v.topAnchor.constraint(equalTo: top, constant: i == 0 ? 10 : 8),
            ])
            if let f = v as? NSTextField, f.cell?.wraps == true {
                f.trailingAnchor.constraint(equalTo: c.trailingAnchor, constant: -12).isActive = true
            }
            last = v
        }
        last?.bottomAnchor.constraint(equalTo: c.bottomAnchor, constant: -10).isActive = true
        add(c)
    }

    private func value(_ text: String) -> NSTextField {
        let f = NSTextField(wrappingLabelWithString: text)
        f.font = Metrics.textXs
        f.isSelectable = true
        f.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return f
    }

    private func button(_ title: String, _ action: @escaping () -> Void) -> NSButton {
        let b = ClosureButton(title: title, action: action)
        b.bezelStyle = .push
        b.controlSize = .small
        b.font = Metrics.textXs
        return b
    }

    private func check(_ title: String, _ on: Bool, _ action: @escaping (Bool) -> Void) -> NSButton {
        let b = ClosureButton(checkboxWithTitle: title) { _ in }
        b.font = Metrics.textXs
        b.state = on ? .on : .off
        b.onClick = { [weak b] in action(b?.state == .on) }
        return b
    }

    private func row(_ views: [NSView]) -> NSStackView {
        let s = NSStackView(views: views)
        s.orientation = .horizontal
        s.spacing = 8
        s.alignment = .firstBaseline
        return s
    }

    /// Runs `slow` off the main thread and hands its result to `done`, unless the page changed.
    private func later<T: Sendable>(_ slow: @escaping @Sendable () -> T, _ done: @escaping (T) -> Void) {
        let token = work
        DispatchQueue.global(qos: .userInitiated).async {
            let r = slow()
            DispatchQueue.main.async { MainActor.assumeIsolated { if token == self.work { done(r) } } }
        }
    }

    private func fail(_ error: Error) { NSAlert(error: error).beginSheetModal(for: window!) }

    // MARK: General

    nonisolated private static func isFolder(_ u: URL) -> Bool {
        let v = try? u.resourceValues(forKeys: [.isDirectoryKey, .isPackageKey])
        return v?.isDirectory == true && v?.isPackage != true
    }

    private static let bytes: NumberFormatter = {
        let f = NumberFormatter()
        f.numberStyle = .decimal
        return f
    }()

    nonisolated private static func long(_ n: UInt64) -> String {
        "\(formatSize(bytes: n)) (\(bytes.string(from: NSNumber(value: n)) ?? "\(n)") " + String(localized: "bytes") + ")"
    }

    private static func when(_ d: Date?) -> String {
        guard let d else { return "—" }
        let f = DateFormatter()
        f.dateStyle = .full
        f.timeStyle = .medium
        return f.string(from: d)
    }

    private func buildGeneral() {
        let fm = FileManager.default
        if urls.count > 1 {
            let dirs = urls.filter(Self.isFolder).count
            let exts = Set(urls.map { $0.pathExtension.lowercased() })
            let kind = dirs == urls.count ? String(localized: "All folders")
                : dirs == 0 && exts.count == 1 && !exts.first!.isEmpty ? String(localized: "All of type .\(exts.first!)")
                : String(localized: "Several types")
            let parents = Set(urls.map { $0.deletingLastPathComponent().path })
            card([
                (String(localized: "Selection"), value(String(localized: "\(urls.count - dirs) files, \(dirs) folders"))),
                (String(localized: "Type"), value(kind)),
                (String(localized: "Location"), value(parents.count == 1 ? String(localized: "All in \(parents.first!)") : String(localized: "Several folders"))),
                (String(localized: "Size"), sizeValue),
                (String(localized: "Size on disk"), diskValue),
                (String(localized: "Contents"), contentsValue),
            ])
            measure()
            return
        }
        let u = urls[0]
        let folder = Self.isFolder(u)
        let v = try? u.resourceValues(forKeys: [
            .localizedTypeDescriptionKey, .contentTypeKey, .fileSizeKey, .totalFileAllocatedSizeKey, .creationDateKey,
            .contentModificationDateKey, .contentAccessDateKey, .isHiddenKey, .isUserImmutableKey, .tagNamesKey, .volumeIsRootFileSystemKey,
        ])

        // the icon and the name, renamed in place on Return
        let icon = NSImageView(image: NSWorkspace.shared.icon(forFile: u.path))
        icon.imageScaling = .scaleProportionallyUpOrDown
        icon.widthAnchor.constraint(equalToConstant: 48).isActive = true
        icon.heightAnchor.constraint(equalToConstant: 48).isActive = true
        let name = NSTextField(string: u.lastPathComponent)
        name.font = Metrics.textSm
        name.target = self
        name.action = #selector(renamed(_:))
        name.isEditable = (try? u.resourceValues(forKeys: [.volumeIsReadOnlyKey]).volumeIsReadOnly) != true
        let head = row([icon, name])
        head.alignment = .centerY
        add(head)

        var kind = v?.localizedTypeDescription ?? (folder ? String(localized: "Folder") : "")
        if !folder, !u.pathExtension.isEmpty { kind += " (.\(u.pathExtension))" }
        var rows: [(String, NSView)] = [(String(localized: "Type"), value(kind))]
        if !folder {
            let app = NSWorkspace.shared.urlForApplication(toOpen: u)
            let appName = app.map { fm.displayName(atPath: $0.path) } ?? String(localized: "Not chosen")
            rows.append((String(localized: "Opens with"), row([value(appName), button(String(localized: "Change…")) { [weak self] in self?.changeApp() }])))
        }
        rows.append((String(localized: "Location"), value(u.deletingLastPathComponent().path)))
        if folder {
            rows += [(String(localized: "Size"), sizeValue), (String(localized: "Size on disk"), diskValue), (String(localized: "Contents"), contentsValue)]
        } else {
            sizeValue.stringValue = Self.long(UInt64(v?.fileSize ?? 0))
            diskValue.stringValue = Self.long(UInt64(v?.totalFileAllocatedSize ?? 0))
            rows += [(String(localized: "Size"), sizeValue), (String(localized: "Size on disk"), diskValue)]
        }
        card(rows)
        if folder { measure() }

        card([
            (String(localized: "Created"), value(Self.when(v?.creationDate))),
            (String(localized: "Modified"), value(Self.when(v?.contentModificationDate))),
            (String(localized: "Accessed"), value(Self.when(v?.contentAccessDate))),
        ])

        section(String(localized: "Attributes"))
        let attrs = PropertiesCard()
        let locked = check(String(localized: "Locked (can’t be changed or moved to the Trash)"), v?.isUserImmutable == true) { [weak self] on in
            self?.set(.isUserImmutableKey, on)
        }
        let hidden = check(String(localized: "Hidden"), v?.isHidden == true) { [weak self] on in self?.set(.isHiddenKey, on) }
        let list = NSStackView(views: [locked, hidden])
        list.orientation = .vertical
        list.alignment = .leading
        list.spacing = 6
        attrs.fill(list)
        add(attrs)

        if let tags = v?.tagNames, !tags.isEmpty {
            card([(String(localized: "Tags"), value(tags.joined(separator: ", ")))])
        }

        // a downloaded file: where it came from, and Unblock (Gatekeeper's quarantine)
        if Self.quarantined(u) {
            section(String(localized: "Security"))
            let note = value(String(localized: "This item came from another computer or the internet. macOS checks it with Gatekeeper and asks before opening it."))
            let unblock = check(String(localized: "Unblock"), false) { [weak self] on in
                guard on else { return }
                removexattr(u.path, "com.apple.quarantine", 0)
                self?.build()
            }
            let col = NSStackView(views: [note, unblock])
            col.orientation = .vertical
            col.alignment = .leading
            col.spacing = 8
            let c = PropertiesCard()
            c.fill(col)
            add(c)
        }
    }

    nonisolated private static func quarantined(_ u: URL) -> Bool { getxattr(u.path, "com.apple.quarantine", nil, 0, 0, 0) > 0 }

    /// Size, size on disk and contents of everything selected, down every folder, updated as it
    /// goes ("Calculating…" until the end).
    private func measure() {
        let calculating = String(localized: "Calculating…")
        [sizeValue, diskValue, contentsValue].forEach { $0.stringValue = calculating }
        let token = work
        let urls = urls
        DispatchQueue.global(qos: .userInitiated).async {
            var total: (bytes: UInt64, disk: UInt64, files: Int, dirs: Int) = (0, 0, 0, 0)
            var last = Date()
            let keys: [URLResourceKey] = [.isDirectoryKey, .fileSizeKey, .totalFileAllocatedSizeKey]
            func post(_ done: Bool) {
                let t = total
                DispatchQueue.main.async {
                    MainActor.assumeIsolated {
                        guard token == self.work else { return }
                        let suffix = done ? "" : " — " + String(localized: "calculating…")
                        self.sizeValue.stringValue = Self.long(t.bytes) + suffix
                        self.diskValue.stringValue = Self.long(t.disk) + suffix
                        self.contentsValue.stringValue = String(localized: "\(t.files) files, \(t.dirs) folders") + suffix
                    }
                }
            }
            for u in urls {
                let top = try? u.resourceValues(forKeys: Set(keys))
                guard top?.isDirectory == true else {
                    total.bytes += UInt64(top?.fileSize ?? 0)
                    total.disk += UInt64(top?.totalFileAllocatedSize ?? 0)
                    total.files += 1
                    continue
                }
                if urls.count > 1 { total.dirs += 1 }
                let walker = FileManager.default.enumerator(at: u, includingPropertiesForKeys: keys, options: [], errorHandler: { _, _ in true })
                while let item = walker?.nextObject() as? URL {
                    let v = try? item.resourceValues(forKeys: Set(keys))
                    if v?.isDirectory == true {
                        total.dirs += 1
                    } else {
                        total.files += 1
                        total.bytes += UInt64(v?.fileSize ?? 0)
                        total.disk += UInt64(v?.totalFileAllocatedSize ?? 0)
                    }
                    if Date().timeIntervalSince(last) > 0.2 {
                        last = Date()
                        post(false)
                        let stop = DispatchQueue.main.sync { MainActor.assumeIsolated { token != self.work } }
                        if stop { return }
                    }
                }
            }
            post(true)
        }
    }

    @objc private func renamed(_ field: NSTextField) {
        let u = urls[0]
        let name = field.stringValue.trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty, name != u.lastPathComponent else { return }
        do {
            let made = try FileOps.rename(u, to: name)
            FileUndo.shared.record(.moves([(from: made, to: u)]), title: String(localized: "Rename"))
            urls = [made]
            window?.title = String(localized: "\(made.lastPathComponent) Properties")
        } catch {
            field.stringValue = u.lastPathComponent
            fail(error)
        }
    }

    private func set(_ key: URLResourceKey, _ on: Bool) {
        do {
            for u in urls {
                var v = URLResourceValues()
                if key == .isHiddenKey { v.isHidden = on } else { v.isUserImmutable = on }
                var url = u
                try url.setResourceValues(v)
            }
        } catch { fail(error) }
    }

    /// Chooses the app for every file of this type (as Finder's "Change All…").
    private func changeApp() {
        let u = urls[0]
        let panel = NSOpenPanel()
        panel.directoryURL = URL(fileURLWithPath: "/Applications")
        panel.allowedContentTypes = [.application]
        panel.prompt = String(localized: "Choose")
        panel.message = String(localized: "Choose the app that opens every “.\(u.pathExtension)” file.")
        panel.beginSheetModal(for: window!) { [weak self] r in
            guard r == .OK, let app = panel.url, let type = (try? u.resourceValues(forKeys: [.contentTypeKey]))?.contentType else { return }
            NSWorkspace.shared.setDefaultApplication(at: app, toOpen: type) { error in
                DispatchQueue.main.async {
                    MainActor.assumeIsolated {
                        if let error { self?.fail(error) } else { self?.build() }
                    }
                }
            }
        }
    }

    // MARK: Permissions

    private enum Access: Int, CaseIterable {
        case readWrite, readOnly, writeOnly, none

        var title: String {
            switch self {
            case .readWrite: String(localized: "Read & Write")
            case .readOnly: String(localized: "Read only")
            case .writeOnly: String(localized: "Write only (Drop Box)")
            case .none: String(localized: "No Access")
            }
        }

        init(bits: Int) {
            switch (bits & 4 != 0, bits & 2 != 0) {
            case (true, true): self = .readWrite
            case (true, false): self = .readOnly
            case (false, true): self = .writeOnly
            case (false, false): self = .none
            }
        }

        var bits: Int {
            switch self {
            case .readWrite: 6
            case .readOnly: 4
            case .writeOnly: 2
            case .none: 0
            }
        }
    }

    private func buildPermissions() {
        let u = urls[0]
        guard let a = try? FileManager.default.attributesOfItem(atPath: u.path) else {
            card([(String(localized: "Error"), value(String(localized: "The permissions could not be read.")))])
            return
        }
        let mode = (a[.posixPermissions] as? Int) ?? 0
        let owner = a[.ownerAccountName] as? String ?? "—"
        let group = a[.groupOwnerAccountName] as? String ?? "—"
        let mine = owner == NSUserName()

        // who may do what: the owner, the group, everyone else (each the read/write bits; the
        // run bit is kept as it is)
        let who: [(String, Int)] = [
            (String(localized: "\(owner) (Owner)"), 6), (String(localized: "\(group) (Group)"), 3), (String(localized: "Everyone"), 0),
        ]
        var rows: [(String, NSView)] = []
        for (title, shift) in who {
            let pop = NSPopUpButton()
            pop.controlSize = .small
            pop.font = Metrics.textXs
            for access in Access.allCases { pop.addItem(withTitle: access.title) }
            pop.selectItem(at: Access(bits: (mode >> shift) & 7).rawValue)
            pop.isEnabled = mine
            let handler = ClosureTarget { [weak self, weak pop] in
                guard let self, let pop, let access = Access(rawValue: pop.indexOfSelectedItem) else { return }
                let current = (try? FileManager.default.attributesOfItem(atPath: u.path)[.posixPermissions] as? Int) ?? mode
                let kept = current & ~(6 << shift)
                do { try FileManager.default.setAttributes([.posixPermissions: kept | (access.bits << shift)], ofItemAtPath: u.path) } catch { self.fail(error) }
                self.build()
            }
            pop.target = handler
            pop.action = #selector(ClosureTarget.fire)
            objc_setAssociatedObject(pop, &ClosureTarget.key, handler, .OBJC_ASSOCIATION_RETAIN)
            rows.append((title, pop))
        }
        card(rows)

        let bits = String(mode, radix: 8)
        let symbolic = (0..<9).map { i -> String in
            let bit = 1 << (8 - i)
            return mode & bit != 0 ? ["r", "w", "x"][i % 3] : "-"
        }.joined()
        card([
            (String(localized: "Owner"), value(owner)),
            (String(localized: "Group"), value(group)),
            (String(localized: "Mode"), value("\(symbolic)  (\(bits))")),
        ])
        if !mine {
            let note = value(String(localized: "Only its owner can change who may use it."))
            note.textColor = .secondaryLabelColor
            add(note)
        }
    }

    // MARK: Details

    /// What Spotlight knows of it, the way Windows' Details tab lists a file's properties.
    private func buildDetails() {
        let u = urls[0]
        let loading = value(String(localized: "Loading…"))
        add(loading)
        later({ Self.details(u) }) { [weak self] groups in
            guard let self else { return }
            loading.removeFromSuperview()
            if groups.isEmpty {
                self.add(self.value(String(localized: "There are no details for this item.")))
            }
            for (title, rows) in groups {
                self.section(title)
                self.card(rows.map { ($0.0, self.value($0.1)) })
            }
        }
    }

    nonisolated private static func details(_ u: URL) -> [(String, [(String, String)])] {
        guard let item = MDItemCreateWithURL(nil, u as CFURL) else { return [] }
        func get(_ key: CFString) -> Any? { MDItemCopyAttribute(item, key) }
        func text(_ key: CFString) -> String? {
            switch get(key) {
            case let s as String: return s.isEmpty ? nil : s
            case let a as [String]: return a.isEmpty ? nil : a.joined(separator: ", ")
            case let n as NSNumber: return n.stringValue
            case let d as Date: return DateFormatter.localizedString(from: d, dateStyle: .medium, timeStyle: .short)
            default: return nil
            }
        }
        func duration(_ s: Double) -> String {
            let t = Int(s.rounded())
            return t >= 3600 ? String(format: "%d:%02d:%02d", t / 3600, t / 60 % 60, t % 60) : String(format: "%d:%02d", t / 60, t % 60)
        }
        var groups: [(String, [(String, String)])] = []
        func group(_ title: String, _ rows: [(String, String?)]) {
            let have = rows.compactMap { r in r.1.map { (r.0, $0) } }
            if !have.isEmpty { groups.append((title, have)) }
        }
        let w = get(kMDItemPixelWidth) as? Int, h = get(kMDItemPixelHeight) as? Int
        group(String(localized: "Picture"), [
            (String(localized: "Dimensions"), w.flatMap { w in h.map { "\(w) × \($0)" } }),
            (String(localized: "Resolution"), (get(kMDItemResolutionWidthDPI) as? Double).map { "\(Int($0)) dpi" }),
            (String(localized: "Colour space"), text(kMDItemColorSpace)),
            (String(localized: "Bit depth"), text(kMDItemBitsPerSample)),
            (String(localized: "Camera"), [text(kMDItemAcquisitionMake), text(kMDItemAcquisitionModel)].compactMap { $0 }.joined(separator: " ").nilIfEmpty),
            (String(localized: "Lens"), text("kMDItemLensModel" as CFString)),
            (String(localized: "Exposure"), (get(kMDItemExposureTimeSeconds) as? Double).map { $0 < 1 ? "1/\(Int((1 / $0).rounded())) s" : "\($0) s" }),
            (String(localized: "Aperture"), (get(kMDItemFNumber) as? Double).map { String(format: "f/%.1f", $0) }),
            (String(localized: "ISO"), text(kMDItemISOSpeed)),
            (String(localized: "Focal length"), (get(kMDItemFocalLength) as? Double).map { "\(Int($0)) mm" }),
        ])
        group(String(localized: "Media"), [
            (String(localized: "Length"), (get(kMDItemDurationSeconds) as? Double).map(duration)),
            (String(localized: "Codecs"), text(kMDItemCodecs)),
            (String(localized: "Bit rate"), (get(kMDItemTotalBitRate) as? Double).map { "\(Int($0 / 1000)) kbps" }),
            (String(localized: "Sample rate"), (get(kMDItemAudioSampleRate) as? Double).map { "\(Int($0)) Hz" }),
            (String(localized: "Channels"), text(kMDItemAudioChannelCount)),
            (String(localized: "Title"), text(kMDItemTitle)),
            (String(localized: "Artist"), text(kMDItemAuthors)),
            (String(localized: "Album"), text(kMDItemAlbum)),
            (String(localized: "Genre"), text(kMDItemMusicalGenre)),
            (String(localized: "Year"), text(kMDItemRecordingYear)),
        ].filter { _ in get(kMDItemDurationSeconds) != nil })
        group(String(localized: "Document"), [
            (String(localized: "Pages"), text(kMDItemNumberOfPages)),
            (String(localized: "Title"), get(kMDItemDurationSeconds) == nil ? text(kMDItemTitle) : nil),
            (String(localized: "Authors"), get(kMDItemDurationSeconds) == nil ? text(kMDItemAuthors) : nil),
            (String(localized: "Created by"), text(kMDItemCreator)),
            (String(localized: "Encoding"), text("kMDItemTextEncoding" as CFString)),
        ])
        group(String(localized: "App"), [
            (String(localized: "Version"), text(kMDItemVersion)),
            (String(localized: "Identifier"), text(kMDItemCFBundleIdentifier)),
            (String(localized: "Copyright"), text(kMDItemCopyright)),
            (String(localized: "Architectures"), text("kMDItemExecutableArchitectures" as CFString)),
        ])
        group(String(localized: "File"), [
            (String(localized: "Kind"), text(kMDItemKind)),
            (String(localized: "Uniform type"), text(kMDItemContentType)),
            (String(localized: "Downloaded from"), text(kMDItemWhereFroms)),
            (String(localized: "Last opened"), text(kMDItemLastUsedDate)),
            (String(localized: "Times opened"), text("kMDItemUseCount" as CFString)),
        ])
        return groups
    }

    // MARK: Hashes

    private func buildHashes() {
        let u = urls[0]
        let fields = ["MD5", "SHA-1", "SHA-256"].map { _ in value("") }
        let go = button(String(localized: "Calculate")) {}
        go.target = nil
        var rows: [(String, NSView)] = []
        for (name, f) in zip(["MD5", "SHA-1", "SHA-256"], fields) {
            f.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
            f.stringValue = "—"
            let copy = button(String(localized: "Copy")) {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(f.stringValue, forType: .string)
            }
            let col = NSStackView(views: [f, copy])
            col.orientation = .vertical
            col.alignment = .leading
            col.spacing = 4
            rows.append((name, col))
        }
        card(rows)
        // the button sits under the card
        (go as? ClosureButton)?.handler = { [weak self, weak go] in
            go?.isEnabled = false
            fields.forEach { $0.stringValue = String(localized: "Calculating…") }
            self?.later({ Self.hashes(u) }) { result in
                guard let result else {
                    fields.forEach { $0.stringValue = String(localized: "The file could not be read.") }
                    return
                }
                for (f, h) in zip(fields, result) { f.stringValue = h }
            }
        }
        stack.addArrangedSubview(go)
    }

    nonisolated private static func hashes(_ u: URL) -> [String]? {
        guard let h = try? FileHandle(forReadingFrom: u) else { return nil }
        defer { try? h.close() }
        var md5 = Insecure.MD5(), sha1 = Insecure.SHA1(), sha256 = SHA256()
        while let chunk = try? h.read(upToCount: 1 << 20), !chunk.isEmpty {
            md5.update(data: chunk)
            sha1.update(data: chunk)
            sha256.update(data: chunk)
        }
        func hex<D: Digest>(_ d: D) -> String { d.map { String(format: "%02x", $0) }.joined() }
        return [hex(md5.finalize()), hex(sha1.finalize()), hex(sha256.finalize())]
    }
}

// MARK: - pieces

private extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}

/// The window's background: the palette's window colour (the app's own look, not a sheet).
@MainActor
final class PropertiesBackground: FlippedView {
    override func draw(_ dirtyRect: NSRect) {
        Palette.of(self).contentBg.setFill()
        dirtyRect.fill()
    }
}

/// A card: the tile colour, rounded, with a hairline border.
@MainActor
final class PropertiesCard: FlippedView {
    override var wantsUpdateLayer: Bool { true }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = Metrics.radius
        layer?.borderWidth = 1
        translatesAutoresizingMaskIntoConstraints = false
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func updateLayer() {
        let p = Palette.of(self)
        layer?.backgroundColor = p.tileBg.cgColor
        layer?.borderColor = p.border.cgColor
    }

    /// Holds one view with 12 / 10 padding.
    func fill(_ v: NSView) {
        v.translatesAutoresizingMaskIntoConstraints = false
        addSubview(v)
        NSLayoutConstraint.activate([
            v.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            v.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -12),
            v.topAnchor.constraint(equalTo: topAnchor, constant: 10),
            v.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -10),
        ])
    }
}

/// A button (or checkbox) with its action as a closure.
@MainActor
final class ClosureButton: NSButton {
    var handler: () -> Void
    var onClick: (() -> Void)? {
        get { handler }
        set { handler = newValue ?? {} }
    }

    init(title: String, action: @escaping () -> Void) {
        handler = action
        super.init(frame: .zero)
        self.title = title
        target = self
        self.action = #selector(fire)
    }

    convenience init(checkboxWithTitle title: String, _ action: @escaping (Bool) -> Void) {
        self.init(title: title) {}
        setButtonType(.switch)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    @objc private func fire() { handler() }
}

/// A target for controls that need a closure (pop-up buttons).
@MainActor
final class ClosureTarget: NSObject {
    nonisolated(unsafe) static var key = 0
    let handler: () -> Void
    init(_ handler: @escaping () -> Void) { self.handler = handler }
    @objc func fire() { handler() }
}
