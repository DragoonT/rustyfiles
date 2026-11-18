import AppKit

/// The folder organizer (organizer/mod.rs): rules that sort the files of a folder. rf-core's
/// engine plans (rf-mac `organize_plan`); this side keeps the rules, shows what a run will do
/// and does it, with undo; automatic rules run when files arrive in their folders.
@MainActor
enum Organizer {
    // MARK: rules, in the engine's shape (organize::Rule, serde)

    struct Condition: Codable, Equatable {
        var type: String  // name, extension, kind, size, age
        var op: String?  // contains, starts_with, ends_with, wildcard
        var text: String?
        var list: [String]?
        var kinds: [String]?  // image, video, audio, document, archive, program, other
        var cmp: String?  // greater, less
        var bytes: UInt64?
        var field: String?  // modified, created
        var days: UInt32?
    }

    struct Action: Codable, Equatable {
        var type: String  // move_to, move_to_template, rename, recycle
        var folder: String?
        var template: String?
    }

    struct Rule: Codable, Equatable {
        var id: UInt32
        var name: String
        var enabled: Bool
        var folder: String
        var subfolders: Bool
        var join: String  // all, any
        var conditions: [Condition]
        var action: Action
        var auto: Bool
    }

    private static let file: URL = {
        let dir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/rustyfiles")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir.appendingPathComponent("organizer.json")
    }()

    static var rules: [Rule] = load() {
        didSet {
            save()
            watch()
            NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": "organizer"])
        }
    }

    private static func load() -> [Rule] {
        guard let data = try? Data(contentsOf: file) else { return [] }
        return (try? JSONDecoder().decode([Rule].self, from: data)) ?? []
    }

    private static func save() {
        let e = JSONEncoder()
        e.outputFormatting = [.prettyPrinted, .sortedKeys]
        try? e.encode(rules).write(to: file, options: .atomic)
    }

    static func json(_ r: [Rule]) -> String { String(decoding: (try? JSONEncoder().encode(r)) ?? Data(), as: UTF8.self) }

    static func summary(_ r: Rule) -> String { organizeSummary(ruleJson: String(decoding: (try? JSONEncoder().encode(r)) ?? Data(), as: UTF8.self)) }

    /// Automatic rules run by themselves only while this is on.
    static var autoOn: Bool {
        get { UserDefaults.standard.bool(forKey: "organizerAuto") }
        set {
            UserDefaults.standard.set(newValue, forKey: "organizerAuto")
            watch()
            NotificationCenter.default.post(name: .rfSettingsChanged, object: nil, userInfo: ["key": "organizer"])
        }
    }

    // MARK: plans

    struct Plan: Decodable {
        struct Step: Decodable {
            let source: String
            let dest: String?
            let rule: String
        }

        let steps: [Step]
        let skipped: [[String]]
        let loops: [String]
        let unusable: [[String]]
        let in_place: Int
        let ignored: Int
    }

    nonisolated static func plan(_ rulesJSON: String, only: UInt32?) -> Plan? {
        try? JSONDecoder().decode(Plan.self, from: Data(organizePlan(rulesJson: rulesJSON, only: only).utf8))
    }

    /// Plans, shows what would happen, and does it when asked ("Run now").
    static func runWithPreview(only: UInt32?) {
        let json = json(rules)
        DispatchQueue.global(qos: .userInitiated).async {
            let p = plan(json, only: only)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let p else { return }
                    if OrganizerPreview.confirm(p) { execute(p, quiet: false) }
                }
            }
        }
    }

    /// Does a plan off the main thread, with a Status Center card; every step is undone by one ⌘Z.
    static func execute(_ p: Plan, quiet: Bool) {
        guard !p.steps.isEmpty else { return }
        let title = p.steps.count == 1 ? String(localized: "Organizing 1 file") : String(localized: "Organizing \(p.steps.count) files")
        let card = StatusCenter.shared.add(title: title, folder: nil, cancel: nil)
        let steps = p.steps
        DispatchQueue.global(qos: .userInitiated).async {
            let fm = FileManager.default
            var undo: [FileOp] = []
            var failures: [String] = []
            for (i, s) in steps.enumerated() {
                let src = URL(fileURLWithPath: s.source)
                guard fm.fileExists(atPath: src.path) else { continue }
                do {
                    if let d = s.dest {
                        let dst = URL(fileURLWithPath: d)
                        // the engine never overwrites; if something took the name since, skip it
                        guard !fm.fileExists(atPath: dst.path) else { continue }
                        try fm.createDirectory(at: dst.deletingLastPathComponent(), withIntermediateDirectories: true)
                        try fm.moveItem(at: src, to: dst)
                        undo.append(.moves([(from: dst, to: src)]))
                    } else {
                        let pairs = try FileOps.trash([src])
                        undo.append(.moves(pairs))
                    }
                } catch {
                    failures.append(src.lastPathComponent + ": " + error.localizedDescription)
                }
                let n = UInt64(i + 1), total = UInt64(steps.count)
                DispatchQueue.main.async { MainActor.assumeIsolated { card.update(done: n, total: total) } }
            }
            let ops = undo, errors = failures
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    if !ops.isEmpty { FileUndo.shared.record(.group(ops.reversed()), title: String(localized: "Organize")) }
                    if errors.isEmpty {
                        card.succeed()
                    } else {
                        card.fail(errors.prefix(3).joined(separator: "\n"))
                        errors.forEach { Log.write("organizer: " + $0) }
                    }
                }
            }
        }
    }

    // MARK: automatic rules

    private static var watchers: [String: DirectoryWatcher] = [:]
    private static var pending: [UInt32: DispatchWorkItem] = [:]

    /// One watcher per folder of an automatic rule, while the master switch is on.
    static func watch() {
        let wanted = autoOn ? Set(rules.filter { $0.enabled && $0.auto }.map(\.folder)) : []
        for k in watchers.keys where !wanted.contains(k) { watchers.removeValue(forKey: k) }
        for folder in wanted where watchers[folder] == nil {
            watchers[folder] = DirectoryWatcher(url: URL(fileURLWithPath: folder)) {
                for r in rules where r.folder == folder && r.enabled && r.auto { schedule(r.id) }
            }
        }
    }

    /// A file that has just arrived may still be written: the rule waits three quiet seconds.
    private static func schedule(_ id: UInt32) {
        pending[id]?.cancel()
        let work = DispatchWorkItem {
            MainActor.assumeIsolated {
                let json = json(rules)
                DispatchQueue.global(qos: .utility).async {
                    guard let p = plan(json, only: id), !p.steps.isEmpty else { return }
                    DispatchQueue.main.async { MainActor.assumeIsolated { execute(p, quiet: true) } }
                }
            }
        }
        pending[id] = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 3, execute: work)
    }

    // MARK: editing

    static func newRule() {
        var r = Rule(
            id: (rules.map(\.id).max() ?? 0) + 1, name: "", enabled: true,
            folder: FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first?.path ?? "",
            subfolders: false, join: "all", conditions: [Condition(type: "kind", kinds: ["image"])],
            action: Action(type: "move_to_template", template: "Pictures/{year}"), auto: false)
        if let edited = RuleEditor.edit(&r, isNew: true) { rules.append(edited) }
    }

    static func edit(_ i: Int) {
        guard rules.indices.contains(i) else { return }
        var r = rules[i]
        if let edited = RuleEditor.edit(&r, isNew: false) { rules[i] = edited }
    }

    static func delete(_ i: Int) {
        guard rules.indices.contains(i) else { return }
        let a = NSAlert()
        a.messageText = String(localized: "Delete the rule “\(rules[i].name)”?")
        a.addButton(withTitle: String(localized: "Delete"))
        a.addButton(withTitle: String(localized: "Cancel"))
        guard a.runModal() == .alertFirstButtonReturn else { return }
        rules.remove(at: i)
    }
}

// MARK: - preview of a run

@MainActor
enum OrganizerPreview {
    /// What the run will do, file by file; true: "Apply".
    static func confirm(_ p: Organizer.Plan) -> Bool {
        let a = NSAlert()
        if p.steps.isEmpty {
            a.messageText = String(localized: "Nothing to do")
            var lines: [String] = []
            if p.in_place > 0 { lines.append(String(localized: "\(p.in_place) files are already where the rules want them.")) }
            lines += p.unusable.map { $0.joined(separator: ": ") }
            lines += p.loops.map { String(localized: "“\($0)” would sort its own results again, so it was left out.") }
            a.informativeText = lines.isEmpty ? String(localized: "No file matches the rules.") : lines.joined(separator: "\n")
            a.runModal()
            return false
        }
        a.messageText = p.steps.count == 1 ? String(localized: "1 file will be organized") : String(localized: "\(p.steps.count) files will be organized")
        a.informativeText = String(localized: "⌘Z puts everything back. Nothing is overwritten: a name already taken gets a number.")
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        func short(_ s: String) -> String { s.hasPrefix(home) ? "~" + s.dropFirst(home.count) : s }
        var text = p.steps.prefix(500).map { s in
            "\((s.source as NSString).lastPathComponent)  →  \(s.dest.map(short) ?? String(localized: "Trash"))    (\(s.rule))"
        }
        if p.steps.count > 500 { text.append(String(localized: "…and \(p.steps.count - 500) more")) }
        if !p.skipped.isEmpty {
            text.append("")
            text.append(String(localized: "Left alone:"))
            text += p.skipped.prefix(100).map { "\(($0.first ?? "") as NSString).lastPathComponent  —  \($0.last ?? "")" }
        }
        if !p.unusable.isEmpty {
            text.append("")
            text += p.unusable.map { $0.joined(separator: ": ") }
        }
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 560, height: 260))
        let view = NSTextView(frame: scroll.bounds)
        view.string = text.joined(separator: "\n")
        view.isEditable = false
        view.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        view.autoresizingMask = [.width]
        scroll.documentView = view
        scroll.hasVerticalScroller = true
        a.accessoryView = scroll
        a.addButton(withTitle: String(localized: "Apply"))
        a.addButton(withTitle: String(localized: "Cancel"))
        return a.runModal() == .alertFirstButtonReturn
    }
}

// MARK: - the rule editor (organizer/editor.rs)

@MainActor
final class RuleEditor: NSObject, NSWindowDelegate {
    private var rule: Organizer.Rule
    private let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 620, height: 560), styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
    private let stack = NSStackView()
    private let summary = NSTextField(wrappingLabelWithString: "")
    private let problem = NSTextField(wrappingLabelWithString: "")
    private var saved = false

    private init(_ r: Organizer.Rule) {
        rule = r
        super.init()
    }

    static func edit(_ r: inout Organizer.Rule, isNew: Bool) -> Organizer.Rule? {
        let e = RuleEditor(r)
        e.panel.delegate = e
        e.panel.title = isNew ? String(localized: "New rule") : String(localized: "Edit rule")
        e.build()
        NSApp.runModal(for: e.panel)
        e.panel.orderOut(nil)
        return e.saved ? e.rule : nil
    }

    /// The close button cancels.
    func windowWillClose(_ notification: Notification) { NSApp.stopModal() }

    private func label(_ s: String) -> NSTextField {
        let l = NSTextField(labelWithString: s)
        l.font = Metrics.textXsSemibold
        l.textColor = .secondaryLabelColor
        return l
    }

    private func row(_ views: [NSView]) -> NSStackView {
        let s = NSStackView(views: views)
        s.orientation = .horizontal
        s.spacing = 8
        s.alignment = .centerY
        return s
    }

    private func popup(_ options: [(String, String)], _ current: String?, _ set: @escaping (String) -> Void) -> NSPopUpButton {
        let p = NSPopUpButton()
        for (_, title) in options { p.addItem(withTitle: title) }
        if let i = options.firstIndex(where: { $0.0 == current }) { p.selectItem(at: i) }
        let t = ClosureTarget { [weak p] in if let p { set(options[p.indexOfSelectedItem].0) } }
        p.target = t
        p.action = #selector(ClosureTarget.fire)
        objc_setAssociatedObject(p, &ClosureTarget.key, t, .OBJC_ASSOCIATION_RETAIN)
        return p
    }

    private func field(_ value: String, width: CGFloat = 220, _ set: @escaping (String) -> Void) -> NSTextField {
        let f = NSTextField(string: value)
        f.widthAnchor.constraint(equalToConstant: width).isActive = true
        let t = FieldTarget(set) { [weak self] in self?.refresh() }
        f.delegate = t
        objc_setAssociatedObject(f, &ClosureTarget.key, t, .OBJC_ASSOCIATION_RETAIN)
        return f
    }

    private func check(_ title: String, _ on: Bool, _ set: @escaping (Bool) -> Void) -> NSButton {
        let b = ClosureButton(checkboxWithTitle: title) { _ in }
        b.state = on ? .on : .off
        b.onClick = { [weak b, weak self] in
            set(b?.state == .on)
            self?.refresh()
        }
        return b
    }

    /// Lays the form out again (a condition added or removed, the action changed).
    private func build() {
        stack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 10
        stack.edgeInsets = NSEdgeInsets(top: 16, left: 20, bottom: 16, right: 20)

        stack.addArrangedSubview(label(String(localized: "Name")))
        stack.addArrangedSubview(field(rule.name, width: 360) { [weak self] in self?.rule.name = $0 })

        stack.addArrangedSubview(label(String(localized: "Folder")))
        let folder = NSTextField(labelWithString: rule.folder.isEmpty ? String(localized: "Choose a folder") : rule.folder)
        folder.lineBreakMode = .byTruncatingMiddle
        folder.widthAnchor.constraint(lessThanOrEqualToConstant: 380).isActive = true
        let choose = ClosureButton(title: String(localized: "Choose…")) { [weak self] in
            let p = NSOpenPanel()
            p.canChooseDirectories = true
            p.canChooseFiles = false
            if p.runModal() == .OK, let u = p.url { self?.rule.folder = u.path; self?.build() }
        }
        choose.bezelStyle = .push
        stack.addArrangedSubview(row([folder, choose]))
        stack.addArrangedSubview(check(String(localized: "Also the files in the folders below it"), rule.subfolders) { [weak self] in self?.rule.subfolders = $0 })

        stack.addArrangedSubview(label(String(localized: "Which files")))
        stack.addArrangedSubview(row([
            NSTextField(labelWithString: String(localized: "Files that match")),
            popup([("all", String(localized: "all of these")), ("any", String(localized: "any of these"))], rule.join) { [weak self] in self?.rule.join = $0; self?.refresh() },
        ]))
        for (i, c) in rule.conditions.enumerated() { stack.addArrangedSubview(conditionRow(i, c)) }
        let add = ClosureButton(title: String(localized: "Add a condition")) { [weak self] in
            self?.rule.conditions.append(Organizer.Condition(type: "extension", list: []))
            self?.build()
        }
        add.bezelStyle = .push
        stack.addArrangedSubview(add)

        stack.addArrangedSubview(label(String(localized: "What happens")))
        let actions = [
            ("move_to", String(localized: "Move to a folder")), ("move_to_template", String(localized: "Move to a folder made from a pattern")),
            ("rename", String(localized: "Rename by a pattern")), ("recycle", String(localized: "Move to the Trash")),
        ]
        var actionRow: [NSView] = [popup(actions, rule.action.type) { [weak self] t in
            guard let self else { return }
            self.rule.action = Organizer.Action(type: t, folder: t == "move_to" ? self.rule.action.folder ?? "" : nil,
                                                template: t == "move_to_template" ? "Pictures/{year}/{month}" : (t == "rename" ? "{year}-{month}-{day} {name}.{ext}" : nil))
            self.build()
        }]
        switch rule.action.type {
        case "move_to":
            actionRow.append(field(rule.action.folder ?? "", width: 260) { [weak self] in self?.rule.action.folder = $0 })
            actionRow.append(ClosureButton(title: String(localized: "Choose…")) { [weak self] in
                let p = NSOpenPanel()
                p.canChooseDirectories = true
                p.canChooseFiles = false
                p.canCreateDirectories = true
                if p.runModal() == .OK, let u = p.url { self?.rule.action.folder = u.path; self?.build() }
            })
        case "move_to_template", "rename":
            actionRow.append(field(rule.action.template ?? "", width: 300) { [weak self] in self?.rule.action.template = $0 })
        default: break
        }
        stack.addArrangedSubview(row(actionRow))
        if ["move_to_template", "rename"].contains(rule.action.type) {
            let vars = NSTextField(wrappingLabelWithString: String(localized: "Patterns may use: ") + organizeTemplateVars().map { "{\($0)}" }.joined(separator: " "))
            vars.font = Metrics.textXs
            vars.textColor = .secondaryLabelColor
            vars.preferredMaxLayoutWidth = 560
            stack.addArrangedSubview(vars)
        }

        stack.addArrangedSubview(check(String(localized: "Run by itself when files arrive"), rule.auto) { [weak self] in self?.rule.auto = $0 })
        stack.addArrangedSubview(check(String(localized: "Turned on"), rule.enabled) { [weak self] in self?.rule.enabled = $0 })

        summary.font = Metrics.textSm
        summary.preferredMaxLayoutWidth = 560
        problem.font = Metrics.textXs
        problem.textColor = .systemRed
        problem.preferredMaxLayoutWidth = 560
        stack.addArrangedSubview(summary)
        stack.addArrangedSubview(problem)

        let cancel = ClosureButton(title: String(localized: "Cancel")) { [weak self] in
            guard let self else { return }
            NSApp.stopModal()
            self.panel.orderOut(nil)
        }
        cancel.bezelStyle = .push
        cancel.keyEquivalent = "\u{1b}"
        let save = ClosureButton(title: String(localized: "Save")) { [weak self] in self?.trySave() }
        save.bezelStyle = .push
        save.keyEquivalent = "\r"
        stack.addArrangedSubview(row([cancel, save]))

        let scroll = NSScrollView()
        let doc = FlippedView()
        stack.translatesAutoresizingMaskIntoConstraints = false
        doc.translatesAutoresizingMaskIntoConstraints = false
        doc.addSubview(stack)
        scroll.documentView = doc
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        panel.contentView = scroll
        NSLayoutConstraint.activate([
            doc.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
            doc.trailingAnchor.constraint(equalTo: scroll.contentView.trailingAnchor),
            doc.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
            stack.leadingAnchor.constraint(equalTo: doc.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: doc.trailingAnchor),
            stack.topAnchor.constraint(equalTo: doc.topAnchor),
            stack.bottomAnchor.constraint(equalTo: doc.bottomAnchor),
        ])
        if !panel.isVisible { panel.center() }
        refresh()
    }

    private func conditionRow(_ i: Int, _ c: Organizer.Condition) -> NSStackView {
        let types = [
            ("name", String(localized: "Name")), ("extension", String(localized: "Extension")), ("kind", String(localized: "Kind")),
            ("size", String(localized: "Size")), ("age", String(localized: "Age")),
        ]
        var views: [NSView] = [popup(types, c.type) { [weak self] t in
            guard let self else { return }
            switch t {
            case "name": self.rule.conditions[i] = .init(type: t, op: "contains", text: "")
            case "extension": self.rule.conditions[i] = .init(type: t, list: [])
            case "kind": self.rule.conditions[i] = .init(type: t, kinds: ["image"])
            case "size": self.rule.conditions[i] = .init(type: t, cmp: "greater", bytes: 100 * 1_048_576)
            default: self.rule.conditions[i] = .init(type: t, cmp: "greater", field: "modified", days: 30)
            }
            self.build()
        }]
        switch c.type {
        case "name":
            views.append(popup([("contains", String(localized: "contains")), ("starts_with", String(localized: "starts with")), ("ends_with", String(localized: "ends with")), ("wildcard", String(localized: "matches (* ?)"))], c.op) { [weak self] in self?.rule.conditions[i].op = $0; self?.refresh() })
            views.append(field(c.text ?? "", width: 180) { [weak self] in self?.rule.conditions[i].text = $0 })
        case "extension":
            views.append(field((c.list ?? []).joined(separator: ", "), width: 240) { [weak self] in
                self?.rule.conditions[i].list = OpenRules.parse($0)
            })
        case "kind":
            let kinds = [("image", String(localized: "Images")), ("video", String(localized: "Videos")), ("audio", String(localized: "Audio")), ("document", String(localized: "Documents")), ("archive", String(localized: "Archives")), ("program", String(localized: "Programs")), ("other", String(localized: "Other"))]
            views.append(popup(kinds, c.kinds?.first) { [weak self] in self?.rule.conditions[i].kinds = [$0]; self?.refresh() })
        case "size":
            views.append(popup([("greater", String(localized: "larger than")), ("less", String(localized: "smaller than"))], c.cmp) { [weak self] in self?.rule.conditions[i].cmp = $0; self?.refresh() })
            views.append(field("\((c.bytes ?? 0) / 1_048_576)", width: 70) { [weak self] in self?.rule.conditions[i].bytes = (UInt64($0) ?? 0) * 1_048_576 })
            views.append(NSTextField(labelWithString: "MB"))
        default:
            views.append(popup([("modified", String(localized: "modified")), ("created", String(localized: "created"))], c.field) { [weak self] in self?.rule.conditions[i].field = $0; self?.refresh() })
            views.append(popup([("greater", String(localized: "more than")), ("less", String(localized: "less than"))], c.cmp) { [weak self] in self?.rule.conditions[i].cmp = $0; self?.refresh() })
            views.append(field("\(c.days ?? 0)", width: 60) { [weak self] in self?.rule.conditions[i].days = UInt32($0) ?? 0 })
            views.append(NSTextField(labelWithString: String(localized: "days ago")))
        }
        let remove = ClosureButton(title: "−") { [weak self] in
            self?.rule.conditions.remove(at: i)
            self?.build()
        }
        remove.bezelStyle = .push
        views.append(remove)
        return row(views)
    }

    /// The rule in words, and what is wrong with it if anything.
    private func refresh() {
        summary.stringValue = Organizer.summary(rule)
        let p = Organizer.plan(Organizer.json([rule]), only: rule.id)
        problem.stringValue = p?.unusable.first?.last ?? ""
    }

    private func trySave() {
        refresh()
        guard problem.stringValue.isEmpty else {
            NSSound.beep()
            return
        }
        if rule.name.trimmingCharacters(in: .whitespaces).isEmpty { rule.name = Organizer.summary(rule) }
        saved = true
        NSApp.stopModal()
        panel.orderOut(nil)
    }
}

/// A text field's edits, handed on as they are typed.
@MainActor
final class FieldTarget: NSObject, NSTextFieldDelegate {
    let set: (String) -> Void
    let changed: () -> Void
    init(_ set: @escaping (String) -> Void, changed: @escaping () -> Void) {
        self.set = set
        self.changed = changed
    }

    func controlTextDidChange(_ note: Notification) {
        guard let f = note.object as? NSTextField else { return }
        set(f.stringValue)
        changed()
    }
}

extension SettingsContent {
    static func organizerRows() -> [SettingsRow] {
        let rules = Organizer.rules
        return [
            .card(
                title: String(localized: "Run automatic rules"),
                detail: String(localized: "Rules marked “Run by itself” sort new files as they arrive in their folders, while rustyfiles is open."), icon: "wand-sparkles",
                control: .toggle(get: { Organizer.autoOn }, set: { Organizer.autoOn = $0 })),
            .table(
                title: String(localized: "Rules"),
                detail: String(localized: "Each rule sorts one folder. Run now shows every move before it happens, and ⌘Z undoes a run."),
                icon: "list",
                entries: rules.map { r in
                    let marks = [r.enabled ? nil : String(localized: "off"), r.auto ? String(localized: "automatic") : nil].compactMap { $0 }
                    return (r.name + (marks.isEmpty ? "" : "  (" + marks.joined(separator: ", ") + ")"), Organizer.summary(r), nil)
                },
                empty: String(localized: "No rules yet"),
                rowActions: [
                    (String(localized: "Run now"), { Organizer.runWithPreview(only: Organizer.rules[$0].id) }),
                    (String(localized: "Edit…"), { Organizer.edit($0) }),
                    (String(localized: "Delete"), { Organizer.delete($0) }),
                ],
                buttons: [(String(localized: "New rule…"), { Organizer.newRule() })] + (rules.isEmpty ? [] : [(String(localized: "Run all now"), { Organizer.runWithPreview(only: nil) })])),
        ]
    }
}
