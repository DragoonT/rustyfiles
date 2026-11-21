import AppKit

/// Bulk rename (bulk_rename.rs): find & replace (plain or regex), a pattern with a counter,
/// case, prefix / suffix. The new names are previewed as they are typed, conflicts in red, and
/// Apply waits until there are none. The renames run in a swap-safe order (rf-core's plan) and
/// ⌘Z takes them all back at once. Items from one folder at a time.
@MainActor
final class BulkRenameSheet: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSTextFieldDelegate {
    private static var open: [BulkRenameSheet] = []

    /// Opens the sheet over `window` for `urls` (all in one folder); `done` gets the new names.
    static func show(_ urls: [URL], over window: NSWindow?, done: @escaping ([String]) -> Void) {
        guard let first = urls.first else { return }
        let dir = first.deletingLastPathComponent()
        let one = urls.filter { $0.deletingLastPathComponent() == dir }
        let s = BulkRenameSheet(urls: one, dir: dir, done: done)
        open.append(s)
        if let window {
            window.beginSheet(s.panel) { _ in open.removeAll { $0 === s } }
        } else {
            s.panel.center()
            s.panel.makeKeyAndOrderFront(nil)
        }
    }

    private let urls: [URL]
    private let dir: URL
    private let done: ([String]) -> Void
    private let inputs: [RenameInput]
    private let existing: [String]
    private var outcomes: [RenameOutcome] = []

    private let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 640, height: 560), styleMask: [.titled, .resizable], backing: .buffered, defer: false)
    private let modes = NSSegmentedControl()
    private let find = NSTextField()
    private let replace = NSTextField()
    private let caseInsensitive = NSButton(checkboxWithTitle: String(localized: "Ignore case"), target: nil, action: nil)
    private let regex = NSButton(checkboxWithTitle: String(localized: "Regular expression ($1 for groups)"), target: nil, action: nil)
    private let pattern = NSTextField(string: "{name}_{n:02}")
    private let start = NSTextField(string: "1")
    private let step = NSTextField(string: "1")
    private let caseKind = NSSegmentedControl()
    private let prefix = NSTextField()
    private let suffix = NSTextField()
    private let includeExt = NSButton(checkboxWithTitle: String(localized: "Change the extension too"), target: nil, action: nil)
    private let pages = NSTabView()
    private let table = NSTableView()
    private let summary = NSTextField(labelWithString: "")
    private let apply = NSButton(title: String(localized: "Apply"), target: nil, action: nil)

    private init(urls: [URL], dir: URL, done: @escaping ([String]) -> Void) {
        self.urls = urls
        self.dir = dir
        self.done = done
        let keys: Set<URLResourceKey> = [.isDirectoryKey, .contentModificationDateKey]
        inputs = urls.map { u in
            let v = try? u.resourceValues(forKeys: keys)
            let ms = Int64((v?.contentModificationDate?.timeIntervalSince1970 ?? 0) * 1000)
            return RenameInput(name: u.lastPathComponent, isDir: v?.isDirectory ?? false, modifiedMs: ms)
        }
        let batch = Set(urls.map(\.lastPathComponent))
        existing = ((try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? []).filter { !batch.contains($0) }
        super.init()
        build()
        refresh()
    }

    // MARK: view

    private func build() {
        panel.title = String(localized: "Bulk rename")
        panel.minSize = NSSize(width: 520, height: 420)
        let root = NSView()
        panel.contentView = root

        modes.segmentStyle = .automatic
        modes.segmentCount = 4
        for (i, t) in [String(localized: "Find and replace"), String(localized: "Pattern"), String(localized: "Case"), String(localized: "Prefix / suffix")].enumerated() {
            modes.setLabel(t, forSegment: i)
        }
        modes.selectedSegment = 0
        modes.target = self
        modes.action = #selector(modeChanged)

        find.placeholderString = String(localized: "Text to find")
        replace.placeholderString = String(localized: "Replace with")
        prefix.placeholderString = String(localized: "Prefix")
        suffix.placeholderString = String(localized: "Suffix (before the extension)")
        for f in [find, replace, pattern, start, step, prefix, suffix] { f.delegate = self }
        for b in [caseInsensitive, regex, includeExt] {
            b.target = self
            b.action = #selector(changed)
        }
        caseInsensitive.state = .on
        caseKind.segmentCount = 3
        for (i, t) in [String(localized: "lower case"), String(localized: "UPPER CASE"), String(localized: "Title Case")].enumerated() { caseKind.setLabel(t, forSegment: i) }
        caseKind.selectedSegment = 2
        caseKind.target = self
        caseKind.action = #selector(changed)

        func row(_ views: [NSView]) -> NSStackView {
            let s = NSStackView(views: views)
            s.orientation = .horizontal
            s.spacing = 8
            return s
        }
        func label(_ s: String) -> NSTextField { NSTextField(labelWithString: s) }
        let hint = NSTextField(wrappingLabelWithString: String(localized: "{n} counter ({n:03} pads with zeros) · {name} the old name · {ext} extension · {date} date modified (YYYY-MM-DD)"))
        hint.font = .systemFont(ofSize: 11)
        hint.textColor = .secondaryLabelColor
        start.widthAnchor.constraint(equalToConstant: 60).isActive = true
        step.widthAnchor.constraint(equalToConstant: 60).isActive = true

        let findPage = NSStackView(views: [row([label(String(localized: "Find")), find, label(String(localized: "Replace")), replace]), row([caseInsensitive, regex])])
        let patternPage = NSStackView(views: [row([label(String(localized: "Pattern")), pattern, label(String(localized: "Start")), start, label(String(localized: "Step")), step]), hint])
        let casePage = NSStackView(views: [caseKind])
        let affixPage = NSStackView(views: [row([label(String(localized: "Prefix")), prefix, label(String(localized: "Suffix")), suffix])])
        for p in [findPage, patternPage, casePage, affixPage] {
            p.orientation = .vertical
            p.alignment = .leading
            p.spacing = 8
        }
        pages.tabViewType = .noTabsNoBorder
        for p in [findPage, patternPage, casePage, affixPage] {
            let item = NSTabViewItem()
            let holder = NSView()
            holder.addSubview(p)
            p.translatesAutoresizingMaskIntoConstraints = false
            NSLayoutConstraint.activate([
                p.leadingAnchor.constraint(equalTo: holder.leadingAnchor), p.trailingAnchor.constraint(equalTo: holder.trailingAnchor),
                p.topAnchor.constraint(equalTo: holder.topAnchor),
            ])
            item.view = holder
            pages.addTabViewItem(item)
        }

        for (id, title) in [("old", String(localized: "Old name")), ("new", String(localized: "New name"))] {
            let c = NSTableColumn(identifier: .init(id))
            c.title = title
            c.width = 290
            c.resizingMask = .autoresizingMask
            table.addTableColumn(c)
        }
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.dataSource = self
        table.delegate = self
        table.usesAlternatingRowBackgroundColors = true
        table.rowHeight = 22
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder

        let cancel = NSButton(title: String(localized: "Cancel"), target: self, action: #selector(cancelled))
        cancel.keyEquivalent = "\u{1b}"
        apply.target = self
        apply.action = #selector(applied)
        apply.keyEquivalent = "\r"
        summary.textColor = .secondaryLabelColor
        summary.lineBreakMode = .byTruncatingTail
        let buttons = row([summary, NSView(), cancel, apply])

        let stack = NSStackView(views: [modes, pages, includeExt, scroll, buttons])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 16, left: 16, bottom: 16, right: 16)
        stack.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor), stack.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            stack.topAnchor.constraint(equalTo: root.topAnchor), stack.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            pages.heightAnchor.constraint(equalToConstant: 64),
            pages.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -32),
            scroll.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -32),
            buttons.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -32),
            find.widthAnchor.constraint(greaterThanOrEqualToConstant: 180), replace.widthAnchor.constraint(greaterThanOrEqualToConstant: 180),
            pattern.widthAnchor.constraint(greaterThanOrEqualToConstant: 220),
            prefix.widthAnchor.constraint(greaterThanOrEqualToConstant: 180), suffix.widthAnchor.constraint(greaterThanOrEqualToConstant: 180),
        ])
        scroll.setContentHuggingPriority(.defaultLow, for: .vertical)
        panel.initialFirstResponder = find
    }

    // MARK: computing

    private var mode: RenameMode {
        switch modes.selectedSegment {
        case 1: .pattern(pattern: pattern.stringValue, start: Int64(start.integerValue), step: Int64(step.stringValue) ?? 1)
        case 2: .case(kind: UInt8(max(caseKind.selectedSegment, 0)))
        case 3: .prefixSuffix(prefix: prefix.stringValue, suffix: suffix.stringValue)
        default: .findReplace(find: find.stringValue, replace: replace.stringValue, caseInsensitive: caseInsensitive.state == .on, regex: regex.state == .on)
        }
    }

    private func refresh() {
        outcomes = renamePreview(items: inputs, mode: mode, includeExt: includeExt.state == .on, existing: existing)
        table.reloadData()
        let conflicts = outcomes.filter { $0.error != nil }.count
        let changes = zip(inputs, outcomes).filter { $0.0.name != $0.1.name && $0.1.error == nil }.count
        if conflicts > 0 {
            summary.stringValue = String(localized: "\(conflicts) conflicts: fix them to apply")
            summary.textColor = .systemRed
        } else {
            summary.stringValue = String(localized: "\(changes) of \(inputs.count) will change")
            summary.textColor = .secondaryLabelColor
        }
        apply.isEnabled = conflicts == 0 && changes > 0
    }

    @objc private func modeChanged() {
        pages.selectTabViewItem(at: modes.selectedSegment)
        refresh()
    }

    @objc private func changed() { refresh() }

    func controlTextDidChange(_ obj: Notification) { refresh() }

    // MARK: table

    func numberOfRows(in tableView: NSTableView) -> Int { inputs.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let isNew = tableColumn?.identifier.rawValue == "new"
        let id = NSUserInterfaceItemIdentifier(isNew ? "new" : "old")
        let cell = (tableView.makeView(withIdentifier: id, owner: nil) as? NSTextField) ?? {
            let f = NSTextField(labelWithString: "")
            f.identifier = id
            f.lineBreakMode = .byTruncatingMiddle
            return f
        }()
        let o = outcomes.indices.contains(row) ? outcomes[row] : nil
        if isNew {
            if let e = o?.error {
                cell.stringValue = e
                cell.textColor = .systemRed
            } else {
                cell.stringValue = o?.name ?? ""
                cell.textColor = o?.name == inputs[row].name ? .secondaryLabelColor : .labelColor
            }
        } else {
            cell.stringValue = inputs[row].name
            cell.textColor = .labelColor
        }
        return cell
    }

    // MARK: finishing

    private func close() {
        if let parent = panel.sheetParent { parent.endSheet(panel) } else { panel.orderOut(nil) }
        Self.open.removeAll { $0 === self }
    }

    @objc private func cancelled() { close() }

    @objc private func applied() {
        let pairs = zip(inputs, outcomes).filter { $0.0.name != $0.1.name }.map { RenameStep(from: $0.0.name, to: $0.1.name) }
        let steps = renamePlan(pairs: pairs)
        let dir = self.dir
        let op = FileOp.moves(steps.map { (from: dir.appendingPathComponent($0.from), to: dir.appendingPathComponent($0.to)) })
        let names = pairs.map(\.to)
        let done = self.done
        close()
        Task.detached(priority: .userInitiated) {
            let result = Result { try op.perform() }
            await MainActor.run {
                switch result {
                case .success(let undo):
                    FileUndo.shared.record(undo, title: String(localized: "Bulk Rename"))
                    done(names)
                case .failure(let e):
                    NSAlert(error: e).runModal()
                    done([])
                }
            }
        }
    }
}
